// SPDX-License-Identifier: GPL-2.0

//! Bedrock - A Rust-based x86-64 hypervisor Linux kernel module

use core::pin::Pin;

use kernel::alloc::flags::GFP_KERNEL;
use kernel::bindings;
use kernel::c_str;
use kernel::fs::File;
use kernel::ioctl::_IOW;
use kernel::miscdevice::{MiscDevice, MiscDeviceOptions, MiscDeviceRegistration};
use kernel::prelude::*;

// Internal modules - log must be first for macro availability
#[macro_use]
mod log;
mod adaptive_instruction_counter;
mod c_helpers;
mod ept;
mod factory;
mod instruction_counter;
mod machine;
mod memory;
mod page;
mod pebs_instruction_counter;
mod vm_file;
mod vmcs;
mod vmx;
mod vmx_asm;
mod vmxon;

// Re-exports from internal modules
use adaptive_instruction_counter::AdaptiveInstructionCounter;
use factory::{create_vm, KernelFrameAllocator};
use machine::MACHINE;
use vm_file::{
    create_forked_vm_fd, create_vm_fd, read_vm_file_type, BedrockForkedVmFile, BedrockVmFile,
    VmFileType,
};
use vmx::traits::{Kernel, Machine, Vmx, VmxCpu};
use vmx::BedrockHandler;
use vmx::VmxoffError;
use vmx::{ForkableVm, ForkedVm};
use vmx_asm::VmxContextExt;
use vmxon::RealVmx;

/// Ioctl magic number for bedrock ('B' for Bedrock).
const BEDROCK_IOC_MAGIC: u32 = b'B' as u32;

/// Ioctl number for CREATE_ROOT_VM command.
/// This is _IOW('B', 0, u64) - takes memory size as argument, returns FD via return value.
const BEDROCK_CREATE_ROOT_VM: u32 = _IOW::<u64>(BEDROCK_IOC_MAGIC, 0);

/// Ioctl number for CREATE_FORKED_VM command.
/// This is _IOW('B', 1, u64) - takes parent VM ID as argument, returns FD via return value.
const BEDROCK_CREATE_FORKED_VM: u32 = _IOW::<u64>(BEDROCK_IOC_MAGIC, 1);

module! {
    type: Bedrock,
    name: "bedrock",
    authors: ["bedrock-rs"],
    description: "A Rust-based x86-64 hypervisor",
    license: "GPL",
}

/// Register a misc device with custom mode permissions.
///
/// The standard `MiscDeviceRegistration::register` doesn't allow setting the mode,
/// so we need this helper to create world-accessible device files.
fn register_miscdev_with_mode(
    name: &'static kernel::str::CStr,
    mode: u16,
) -> impl PinInit<MiscDeviceRegistration<BedrockFile>, Error> {
    // SAFETY: We properly initialize and register the miscdevice, and the
    // MiscDeviceRegistration's Drop will call misc_deregister.
    unsafe {
        ::pin_init::pin_init_from_closure(move |slot: *mut MiscDeviceRegistration<BedrockFile>| {
            // Get a pointer to the inner miscdevice struct
            let inner_ptr = slot.cast::<bindings::miscdevice>();

            // Create the base miscdevice from options
            let opts = MiscDeviceOptions { name };
            inner_ptr.write(opts.into_raw::<BedrockFile>());

            // Set the mode for world-accessible permissions
            (*inner_ptr).mode = mode;

            // Register the misc device
            kernel::error::to_result(bindings::misc_register(inner_ptr))
        })
    }
}

// Define a global mutex for the handler using the kernel's global_lock! macro.
// SAFETY: Initialized in module init before first use.
kernel::sync::global_lock! {
    unsafe(uninit) static HANDLER: Mutex<Option<BedrockHandler<'static, RealVmx, 64>>> = None;
}

/// Private data for an open bedrock device file.
///
/// Each open file descriptor gets its own instance of this struct.
/// The actual VM management is handled by the global HANDLER.
#[pin_data]
struct BedrockFile {}

/// Handle CREATE_ROOT_VM ioctl - separated to isolate stack usage.
#[inline(never)]
fn handle_create_root_vm(memory_size: usize) -> Result<isize> {
    if memory_size == 0 {
        log_err!("Invalid memory size: 0\n");
        return Err(EINVAL);
    }

    // Allocate a VM ID from the handler
    let vm_id = {
        let mut guard = HANDLER.lock();
        let handler = guard.as_mut().ok_or(ENODEV)?;
        handler.alloc_vm_id().ok_or(ENOSPC)?
    };

    // Create the VM with the specified memory size
    let vm = create_vm(&MACHINE, memory_size).ok_or_else(|| {
        log_err!(
            "Failed to create VM {} with {} bytes of memory\n",
            vm_id,
            memory_size
        );
        ENOMEM
    })?;

    // Create anonymous inode FD for the VM
    let fd = create_vm_fd(vm, vm_id).inspect_err(|e| {
        log_err!("Failed to create VM FD: {:?}\n", e);
    })?;

    log_info!(
        "Created VM {} with fd {} ({} bytes memory)\n",
        vm_id,
        fd,
        memory_size
    );
    Ok(fd as isize)
}

/// Handle CREATE_FORKED_VM ioctl - separated to isolate stack usage.
///
/// This function is designed for parallel fork creation. The handler lock is
/// only held briefly to:
/// 1. Allocate a VM ID
/// 2. Find and validate the parent VM
/// 3. Increment the parent's children_count (atomic)
/// 4. Get a raw pointer to the parent
///
/// The expensive work (EPT cloning, VMCS copying, etc.) happens outside the lock,
/// allowing multiple forks from the same parent to proceed in parallel.
///
/// # Safety Invariants
///
/// - Once children_count > 0, the parent cannot be run (can_run() returns false)
/// - Concurrent forks only READ parent state, which is safe
/// - The caller must not close the parent FD while forks are in progress
#[inline(never)]
fn handle_create_forked_vm(parent_vm_id: u64) -> Result<isize> {
    log_info!("FORK: Starting fork from parent {}\n", parent_vm_id);

    // Phase 1: Under lock - allocate ID, find parent, increment children_count, get pointer
    // This is the only serialized part of fork creation.
    let (vm_id, parent_ptr, parent_type) = {
        let mut guard = HANDLER.lock();
        let handler = guard.as_mut().ok_or(ENODEV)?;

        // Allocate VM ID
        let vm_id = handler.alloc_vm_id().ok_or(ENOSPC)?;

        // Find the parent VM by ID
        let parent_ref = handler.find_vm_by_id(parent_vm_id).ok_or_else(|| {
            log_err!("Parent VM {} not found\n", parent_vm_id);
            ENOENT
        })?;

        // Determine parent type
        // SAFETY: parent_ref points to a valid BedrockVmFile or BedrockForkedVmFile
        // that was registered via add_vm. Both structs have vm_file_type as their
        // first field (guaranteed by #[repr(C)]). We hold the handler lock.
        let parent_type = unsafe { read_vm_file_type(parent_ref.as_ptr()) };

        // Increment parent's children_count BEFORE releasing lock.
        // This prevents the parent from being run while we fork from it.
        // The atomic increment is the key synchronization point.
        match parent_type {
            VmFileType::Root => {
                // SAFETY: parent_ref points to a valid BedrockVmFile registered via add_vm.
                // BedrockVmFile has vm_file_type as its first field (#[repr(C)]). We hold the handler lock.
                let parent_file = unsafe { &*(parent_ref.as_ptr() as *const BedrockVmFile) };
                parent_file.vm.add_child();
            }
            VmFileType::Forked => {
                // SAFETY: parent_ref points to a valid BedrockForkedVmFile registered via add_vm.
                // BedrockForkedVmFile has vm_file_type as its first field (#[repr(C)]). We hold the handler lock.
                let parent_file = unsafe { &*(parent_ref.as_ptr() as *const BedrockForkedVmFile) };
                parent_file.vm.add_child();
            }
        }

        log_info!(
            "FORK: VM {} - found parent {} (type {:?}), incremented children_count\n",
            vm_id,
            parent_vm_id,
            parent_type as u8
        );

        (vm_id, parent_ref.as_ptr(), parent_type)
    }; // Lock released here - expensive work can now proceed in parallel

    // Phase 2: Without lock - do the expensive fork work
    // Multiple threads can execute this phase concurrently for the same parent.
    let fork_result = {
        let mut allocator = KernelFrameAllocator::new(MACHINE.kernel());
        let exit_handler_rip = vmx::VmxContext::exit_handler_addr();
        // Forked VMs always use the perf_event counter: the PEBS DS area sits
        // in guest memory, which is COW-shared with the parent. The forked
        // VM's EPT clone has R+X permissions, so PEBS writes during a forked
        // run would trigger a COW copy and diverge from the host pointer we
        // have for updating DS management fields.
        let instruction_counter = AdaptiveInstructionCounter::PerfEvent(
            instruction_counter::LinuxInstructionCounter::new()
                .unwrap_or_else(instruction_counter::LinuxInstructionCounter::null),
        );

        match parent_type {
            VmFileType::Root => {
                // SAFETY: parent_ptr is valid because:
                // 1. We found it in the handler's vm_list while holding the lock
                // 2. children_count > 0 prevents it from being run
                // 3. User contract: parent FD must not be closed during fork
                let parent_file = unsafe { &*(parent_ptr as *const BedrockVmFile) };
                // Use new_with_incremented_parent since we already incremented
                // children_count in phase 1 while holding the lock.
                ForkedVm::new_with_incremented_parent(
                    &parent_file.vm,
                    &MACHINE,
                    &mut allocator,
                    exit_handler_rip,
                    instruction_counter,
                )
            }
            VmFileType::Forked => {
                // SAFETY: parent_ptr is valid because we found it in the handler's vm_list while
                // holding the lock and children_count > 0 prevents it from being run or freed.
                let parent_file = unsafe { &*(parent_ptr as *const BedrockForkedVmFile) };
                ForkedVm::new_with_incremented_parent(
                    &parent_file.vm,
                    &MACHINE,
                    &mut allocator,
                    exit_handler_rip,
                    instruction_counter,
                )
            }
        }
    };

    // Handle fork result - on failure, we need to decrement children_count
    let forked_vm = match fork_result {
        Ok(vm) => vm,
        Err(e) => {
            log_err!(
                "Failed to create forked VM from parent {}: {:?}\n",
                parent_vm_id,
                e
            );
            // Decrement children_count since ForkedVm wasn't created
            // (normally ForkedVm::drop does this, but creation failed)
            match parent_type {
                VmFileType::Root => {
                    // SAFETY: parent_ptr is valid - it was found in the handler's vm_list and
                    // children_count > 0 prevents it from being freed. We need to decrement
                    // because ForkedVm creation failed.
                    let parent_file = unsafe { &*(parent_ptr as *const BedrockVmFile) };
                    parent_file.vm.remove_child();
                }
                VmFileType::Forked => {
                    // SAFETY: parent_ptr is valid - it was found in the handler's vm_list and
                    // children_count > 0 prevents it from being freed. We need to decrement
                    // because ForkedVm creation failed.
                    let parent_file = unsafe { &*(parent_ptr as *const BedrockForkedVmFile) };
                    parent_file.vm.remove_child();
                }
            }
            return Err(ENOMEM);
        }
    };

    // Phase 3: Create FD (re-acquires lock briefly to register VM)
    log_info!("FORK: Creating FD for forked VM {}\n", vm_id);
    let fd = create_forked_vm_fd(forked_vm, vm_id).inspect_err(|e| {
        log_err!("Failed to create forked VM FD: {:?}\n", e);
    })?;

    log_info!(
        "Created forked VM {} (from parent {}) with fd {}\n",
        vm_id,
        parent_vm_id,
        fd
    );
    Ok(fd as isize)
}

#[vtable]
impl MiscDevice for BedrockFile {
    type Ptr = Pin<KBox<Self>>;

    fn open(_file: &File, _misc: &MiscDeviceRegistration<Self>) -> Result<Pin<KBox<Self>>> {
        log_info!("Bedrock device opened\n");
        KBox::try_pin_init(try_pin_init!(BedrockFile {}), GFP_KERNEL)
    }

    fn ioctl(_me: Pin<&BedrockFile>, _file: &File, cmd: u32, arg: usize) -> Result<isize> {
        match cmd {
            BEDROCK_CREATE_ROOT_VM => handle_create_root_vm(arg),
            BEDROCK_CREATE_FORKED_VM => handle_create_forked_vm(arg as u64),
            _ => {
                log_err!("Unknown ioctl command: {:#x}\n", cmd);
                Err(ENOTTY)
            }
        }
    }
}

/// The bedrock kernel module.
#[pin_data(PinnedDrop)]
struct Bedrock {
    #[pin]
    _miscdev: MiscDeviceRegistration<BedrockFile>,
}

impl kernel::InPlaceModule for Bedrock {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        // SAFETY: The closure initializes all fields and handles errors properly.
        unsafe {
            ::pin_init::pin_init_from_closure(|slot: *mut Self| {
                log_info!("Bedrock module loading...\n");

                // Register perf guest callbacks for instruction counting
                c_helpers::bedrock_register_perf_callbacks();
                log_info!("Registered perf guest callbacks\n");

                // SAFETY: Called exactly once during module initialization.
                HANDLER.init();

                // Initialize VMX and create the handler
                let handler = match BedrockHandler::<RealVmx, 64>::new(&MACHINE) {
                    Ok(h) => {
                        log_info!("VMX initialized successfully\n");
                        h
                    }
                    Err(e) => {
                        log_err!("Failed to initialize VMX: {:?}\n", e);
                        c_helpers::bedrock_unregister_perf_callbacks();
                        return Err(EINVAL);
                    }
                };

                // Store the handler in the global
                {
                    let mut guard = HANDLER.lock();
                    *guard = Some(handler);
                }

                // Initialize the miscdev field with world-readable/writable permissions
                let miscdev_slot = core::ptr::addr_of_mut!((*slot)._miscdev);
                register_miscdev_with_mode(c_str!("bedrock"), 0o666).__pinned_init(miscdev_slot)?;

                log_info!("Bedrock module loaded\n");

                Ok(())
            })
        }
    }
}

#[pinned_drop]
impl PinnedDrop for Bedrock {
    fn drop(self: Pin<&mut Self>) {
        log_info!("Bedrock module unloading...\n");

        // Clear the global handler first
        {
            let mut guard = HANDLER.lock();
            *guard = None;
        }

        // Deinitialize VMX on all CPUs
        match MACHINE.kernel().call_on_all_cpus_with_data(
            &MACHINE,
            |machine| -> Result<(), VmxoffError> {
                let vcpu = RealVmx::current_vcpu();
                vcpu.deinitialize(machine)?;
                Ok(())
            },
        ) {
            Ok(()) => log_info!("VMX deinitialized successfully\n"),
            Err(e) => log_err!("Error during VMX deinit: {:?}\n", e),
        }

        // Unregister perf guest callbacks
        // SAFETY: Called exactly once during module cleanup.
        unsafe { c_helpers::bedrock_unregister_perf_callbacks() };
        log_info!("Bedrock module unloaded successfully\n");
    }
}
