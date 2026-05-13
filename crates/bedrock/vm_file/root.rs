// SPDX-License-Identifier: GPL-2.0

//! Root VM file operations and handlers.
//!
//! This module provides the file_operations callbacks and root-VM-specific
//! ioctl handlers for bedrock-vm anonymous inodes.

use core::ffi::c_int;
use core::mem::{size_of, MaybeUninit};
use core::sync::atomic::AtomicBool;

use kernel::alloc::KBox;
use kernel::bindings;

use super::super::c_helpers::{
    bedrock_copy_from_user, bedrock_remap_page, bedrock_remap_pages, bedrock_remap_vmalloc_range,
    bedrock_vma_end, bedrock_vma_pgoff, bedrock_vma_start,
};
use super::super::page::{LogBuffer, PagePool, LOG_BUFFER_SIZE};
use super::super::vmx::traits::GuestMemory;
use super::super::vmx::ForkableVm;
use super::super::HANDLER;
use super::core::BedrockVmFile;
use super::handlers::{self, VmFileOps};
use super::structs::*;

/// Implement VmFileOps for BedrockVmFile.
impl VmFileOps for BedrockVmFile {
    type Vm = super::super::vmx::RootVm<
        super::super::vmcs::RealVmcs,
        super::super::page::KernelGuestMemory,
        super::super::instruction_counter::LinuxInstructionCounter,
    >;

    fn vm(&self) -> &Self::Vm {
        &self.vm
    }

    fn vm_mut(&mut self) -> &mut Self::Vm {
        &mut self.vm
    }

    fn vm_id(&self) -> u64 {
        self.vm_id
    }

    fn running(&self) -> &AtomicBool {
        &self.running
    }

    fn log_buffer(&self) -> Option<&LogBuffer> {
        self.log_buffer.as_ref()
    }

    fn log_buffer_mut(&mut self) -> &mut Option<LogBuffer> {
        &mut self.log_buffer
    }

    fn can_run(&self) -> bool {
        self.vm.can_run()
    }

    fn children_count(&self) -> usize {
        self.vm.children_count()
    }

    fn vm_and_pool(&mut self) -> (&mut Self::Vm, &mut PagePool) {
        (&mut self.vm, &mut self.page_pool)
    }
}

/// File operations for bedrock-vm anonymous inodes.
pub(crate) static BEDROCK_VM_FOPS: SyncFileOps = {
    // SAFETY: SyncFileOps::zeroed() produces an all-zeros file_operations, which is valid.
    // We immediately set the required function pointers below.
    let mut fops: bindings::file_operations = unsafe { SyncFileOps::zeroed() };
    fops.owner = core::ptr::null_mut();
    fops.release = Some(bedrock_vm_release);
    fops.unlocked_ioctl = Some(bedrock_vm_ioctl);
    fops.mmap = Some(bedrock_vm_mmap);
    SyncFileOps(fops)
};

/// Release callback for bedrock-vm files.
///
/// # Safety
///
/// - `file` must be a valid pointer to a file struct
/// - `file->private_data` must be a valid pointer to a `KBox<BedrockVmFile>`
unsafe extern "C" fn bedrock_vm_release(
    _inode: *mut bindings::inode,
    file: *mut bindings::file,
) -> c_int {
    // SAFETY: `file` is a valid pointer to a file struct, guaranteed by the kernel
    // VFS layer which calls this release callback.
    let private_data = unsafe { (*file).private_data };

    if private_data.is_null() {
        log_err!("bedrock_vm_release: null private_data\n");
        return 0;
    }

    let vm_ptr = private_data.cast::<BedrockVmFile>();
    // SAFETY: We verified private_data is non-null above, and it was set to a valid
    // KBox<BedrockVmFile> pointer when the fd was created in create_vm_fd.
    let vm_id = unsafe { (*vm_ptr).vm_id };
    log_info!("Releasing VM {} (fd closed)\n", vm_id);

    // Remove from global vm_list
    {
        let mut guard = HANDLER.lock();
        if let Some(handler) = guard.as_mut() {
            handler.remove_vm(vm_ptr);
        }
    }

    // Drop the VM, freeing all resources
    // SAFETY: vm_ptr was created by KBox::into_raw in create_vm_fd. This is the
    // release callback, so this is the unique point where ownership is reclaimed.
    let _ = unsafe { KBox::from_raw(vm_ptr) };

    log_info!("VM {} released successfully\n", vm_id);
    0
}

/// Mmap callback for bedrock-vm files.
///
/// # Safety
///
/// - `file` must be a valid pointer to a file struct
/// - `file->private_data` must be a valid pointer to a `BedrockVmFile`
/// - `vma` must be a valid pointer to a vm_area_struct
unsafe extern "C" fn bedrock_vm_mmap(
    file: *mut bindings::file,
    vma: *mut bindings::vm_area_struct,
) -> c_int {
    // SAFETY: `file` is a valid pointer guaranteed by the kernel VFS layer.
    let private_data = unsafe { (*file).private_data };

    if private_data.is_null() {
        return -(bindings::EBADF as i32);
    }

    // SAFETY: private_data is non-null (checked above) and was set to a valid
    // BedrockVmFile pointer when the fd was created. We hold exclusive access
    // because the kernel serializes mmap calls per file.
    let vm_file = unsafe { &mut *(private_data.cast::<BedrockVmFile>()) };
    let memory = &mut vm_file.vm.memory;

    // Get VMA parameters
    // SAFETY: `vma` is a valid pointer to a vm_area_struct, guaranteed by the kernel
    // VFS/mmap layer. These helpers read standard VMA fields.
    let vma_start = unsafe { bedrock_vma_start(vma) };
    // SAFETY: Same as above — `vma` is a valid VMA pointer from the kernel mmap layer.
    let vma_end = unsafe { bedrock_vma_end(vma) };
    // SAFETY: Same as above — `vma` is a valid VMA pointer from the kernel mmap layer.
    let vma_pgoff = unsafe { bedrock_vma_pgoff(vma) };

    let requested_size = vma_end - vma_start;
    let offset_bytes = vma_pgoff * 4096;

    // Memory layout for mmap:
    // - Offset 0 to memory.size(): guest memory
    // - Offset memory.size(): serial buffer (1 page = 4096 bytes)
    // - Offset memory.size() + 4096: log buffer (1MB, if enabled)
    // - Offset memory.size() + 4096 + LOG_BUFFER_SIZE: serial TSC page (4KB)
    // - Offset memory.size() + 4096 + LOG_BUFFER_SIZE + 4096: feedback buffer 0 (up to 1MB)
    // - Offset memory.size() + 4096 + LOG_BUFFER_SIZE + 4096 + 1MB: feedback buffer 1 (up to 1MB)
    // - ... (each buffer slot reserves 1MB)
    let guest_mem_size = memory.size();
    let serial_buffer_offset = guest_mem_size;
    let log_buffer_offset = guest_mem_size + 4096;
    let serial_tsc_offset = log_buffer_offset + LOG_BUFFER_SIZE;
    let feedback_buffer_base_offset = serial_tsc_offset + 4096;
    const FEEDBACK_BUFFER_SLOT_SIZE: usize = 1024 * 1024; // 1MB per slot

    // Check if this is a feedback buffer mapping
    if offset_bytes as usize >= feedback_buffer_base_offset {
        let relative_offset = offset_bytes as usize - feedback_buffer_base_offset;
        let buffer_index = relative_offset / FEEDBACK_BUFFER_SLOT_SIZE;

        // Validate buffer index
        if buffer_index >= super::super::vmx::MAX_FEEDBACK_BUFFERS {
            log_err!("mmap: invalid feedback buffer index {}\n", buffer_index);
            return -(bindings::EINVAL as i32);
        }

        // Check alignment within slot
        if !relative_offset.is_multiple_of(FEEDBACK_BUFFER_SLOT_SIZE) {
            log_err!("mmap: feedback buffer offset not aligned to slot boundary\n");
            return -(bindings::EINVAL as i32);
        }

        // Feedback buffer mapping
        let feedback_buffer = match &vm_file.vm.state.feedback_buffers[buffer_index] {
            Some(fb) => fb,
            None => {
                log_err!("mmap: feedback buffer {} not registered\n", buffer_index);
                return -(bindings::EINVAL as i32);
            }
        };

        let expected_size = feedback_buffer.num_pages * 4096;
        if requested_size as usize != expected_size {
            log_err!(
                "mmap: feedback buffer {} size mismatch: expected {}, got {}\n",
                buffer_index,
                expected_size,
                requested_size
            );
            return -(bindings::EINVAL as i32);
        }

        // For root VM, translate each GPA to HPA using the GuestMemory trait.
        // Since guest memory is vmalloc'd, each page may have a different physical address.
        let mut hpas = [0u64; 256]; // FEEDBACK_BUFFER_MAX_PAGES = 256

        for (i, hpa) in hpas.iter_mut().enumerate().take(feedback_buffer.num_pages) {
            let gpa = feedback_buffer.gpas[i];
            // GPA is the guest physical address, which for root VM equals the offset
            // into the vmalloc'd memory region.
            *hpa = match memory.page_phys_addr(gpa as usize) {
                Some(addr) => addr.as_u64(),
                None => {
                    log_err!(
                        "mmap: failed to resolve GPA {:#x} for feedback buffer {}\n",
                        gpa,
                        buffer_index
                    );
                    return -(bindings::EINVAL as i32);
                }
            };
        }

        // SAFETY: `vma` is a valid VMA pointer from the kernel. `hpas` contains valid
        // physical addresses resolved from guest memory. num_pages does not exceed the
        // array size (256).
        let ret =
            unsafe { bedrock_remap_pages(vma, hpas.as_ptr(), feedback_buffer.num_pages as i32) };

        if ret != 0 {
            log_err!(
                "mmap: feedback buffer {} remap failed with {}\n",
                buffer_index,
                ret
            );
        } else {
            log_info!(
                "mmap: mapped feedback buffer {} for VM {} ({} pages)\n",
                buffer_index,
                vm_file.vm_id,
                feedback_buffer.num_pages
            );
        }

        ret
    } else if offset_bytes as usize == serial_tsc_offset {
        // Serial TSC metadata page mapping
        if requested_size as usize != 4096 {
            log_err!(
                "mmap: serial TSC page must be exactly 4096 bytes, got {}\n",
                requested_size
            );
            return -(bindings::EINVAL as i32);
        }

        let page = vm_file.vm.state.serial_tsc_page.as_raw_page();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `page` is a valid
        // kernel page obtained from serial_tsc_page.
        let ret = unsafe { bedrock_remap_page(vma, page) };

        if ret != 0 {
            log_err!("mmap: serial TSC page remap failed with {}\n", ret);
        }

        ret
    } else if offset_bytes as usize == log_buffer_offset {
        // Log buffer mapping
        if requested_size as usize != LOG_BUFFER_SIZE {
            log_err!(
                "mmap: log buffer must be exactly {} bytes, got {}\n",
                LOG_BUFFER_SIZE,
                requested_size
            );
            return -(bindings::EINVAL as i32);
        }

        let log_buffer = match &vm_file.log_buffer {
            Some(buf) => buf,
            None => {
                log_err!("mmap: log buffer not allocated (logging not enabled)\n");
                return -(bindings::EINVAL as i32);
            }
        };

        let addr = log_buffer.as_ptr().cast::<core::ffi::c_void>();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `addr` is a valid
        // vmalloc'd pointer to the log buffer. Offset 0 maps from the start.
        let ret = unsafe { bedrock_remap_vmalloc_range(vma, addr, 0) };

        if ret != 0 {
            log_err!("mmap: log buffer remap failed with {}\n", ret);
        } else {
            log_info!("mmap: mapped log buffer for VM {}\n", vm_file.vm_id);
        }

        ret
    } else if offset_bytes as usize == serial_buffer_offset {
        // Serial buffer mapping
        if requested_size as usize != 4096 {
            log_err!(
                "mmap: serial buffer must be exactly 4096 bytes, got {}\n",
                requested_size
            );
            return -(bindings::EINVAL as i32);
        }

        let page = vm_file.vm.state.serial_buffer_page.as_raw_page();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `page` is a valid
        // kernel page obtained from serial_buffer_page.
        let ret = unsafe { bedrock_remap_page(vma, page) };

        if ret != 0 {
            log_err!("mmap: serial buffer remap failed with {}\n", ret);
        } else {
            log_info!("mmap: mapped serial buffer for VM {}\n", vm_file.vm_id);
        }

        ret
    } else if (offset_bytes as usize) < guest_mem_size {
        // Guest memory mapping
        if (offset_bytes as usize) + (requested_size as usize) > guest_mem_size {
            log_err!(
                "mmap: offset {} + size {} exceeds memory size {}\n",
                offset_bytes,
                requested_size,
                guest_mem_size
            );
            return -(bindings::EINVAL as i32);
        }

        let addr = memory.as_mut_ptr().cast::<core::ffi::c_void>();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `addr` is a valid
        // vmalloc'd pointer to guest memory. `vma_pgoff` is the page offset within
        // the mapping, and we verified the range fits within guest_mem_size above.
        let ret = unsafe { bedrock_remap_vmalloc_range(vma, addr, vma_pgoff) };

        if ret != 0 {
            log_err!("mmap: remap_vmalloc_range failed with {}\n", ret);
        } else {
            log_info!(
                "mmap: mapped {} bytes at offset {} for VM {}\n",
                requested_size,
                offset_bytes,
                vm_file.vm_id
            );
        }

        ret
    } else {
        log_err!("mmap: invalid offset {}\n", offset_bytes);
        -(bindings::EINVAL as i32)
    }
}

/// Ioctl callback for bedrock-vm files.
///
/// # Safety
///
/// - `file` must be a valid pointer to a file struct
/// - `file->private_data` must be a valid pointer to a `BedrockVmFile`
unsafe extern "C" fn bedrock_vm_ioctl(
    file: *mut bindings::file,
    cmd: core::ffi::c_uint,
    arg: usize,
) -> isize {
    // SAFETY: `file` is a valid pointer guaranteed by the kernel VFS layer.
    let private_data = unsafe { (*file).private_data };

    if private_data.is_null() {
        return -(bindings::EBADF as isize);
    }

    // SAFETY: private_data is non-null (checked above) and was set to a valid
    // BedrockVmFile pointer when the fd was created. The kernel serializes ioctls
    // per file descriptor.
    let vm_file = unsafe { &mut *(private_data.cast::<BedrockVmFile>()) };

    match cmd {
        BEDROCK_VM_GET_REGS => handlers::handle_get_regs(vm_file, arg),
        BEDROCK_VM_SET_REGS => handlers::handle_set_regs(vm_file, arg),
        BEDROCK_VM_RUN => handlers::handle_run(vm_file, arg),
        BEDROCK_VM_SET_INPUT => handle_set_input(vm_file, arg),
        BEDROCK_VM_SET_RDRAND_CONFIG => handlers::handle_set_rdrand_config(vm_file, arg),
        BEDROCK_VM_SET_RDRAND_VALUE => handlers::handle_set_rdrand_value(vm_file, arg),
        BEDROCK_VM_SET_LOG_CONFIG => handlers::handle_set_log_config(vm_file, arg),
        BEDROCK_VM_SET_SINGLE_STEP => handlers::handle_set_single_step(vm_file, arg),
        BEDROCK_VM_GET_EXIT_STATS => handlers::handle_get_exit_stats(vm_file, arg),
        BEDROCK_VM_SET_STOP_TSC => handlers::handle_set_stop_tsc(vm_file, arg),
        BEDROCK_VM_GET_VM_ID => handlers::handle_get_vm_id(vm_file, arg),
        BEDROCK_VM_GET_FEEDBACK_BUFFER_INFO => {
            handlers::handle_get_feedback_buffer_info(vm_file, arg)
        }
        BEDROCK_VM_QUEUE_IO_ACTION => handlers::handle_queue_io_action(vm_file, arg),
        BEDROCK_VM_DRAIN_IO_RESPONSE => handlers::handle_drain_io_response(vm_file, arg),
        _ => -(bindings::ENOTTY as isize),
    }
}

// ============================================================================
// Root-VM-specific handlers (not shared with forked VMs)
// ============================================================================

/// Handle SET_INPUT ioctl - set serial input buffer from userspace.
fn handle_set_input(vm_file: &mut BedrockVmFile, arg: usize) -> isize {
    let mut input = MaybeUninit::<BedrockSerialInput>::uninit();

    // SAFETY: `input.as_mut_ptr()` points to valid, aligned, writable memory for a
    // BedrockSerialInput. `arg` is a user-provided pointer from the ioctl syscall.
    // bedrock_copy_from_user performs a bounded copy.
    let not_copied = unsafe {
        bedrock_copy_from_user(
            input.as_mut_ptr().cast::<core::ffi::c_void>(),
            arg as *const core::ffi::c_void,
            size_of::<BedrockSerialInput>() as core::ffi::c_ulong,
        )
    };

    if not_copied != 0 {
        return -(bindings::EFAULT as isize);
    }

    // SAFETY: bedrock_copy_from_user succeeded (returned 0), so all bytes of `input`
    // have been written and it is now fully initialized.
    let input = unsafe { input.assume_init() };

    // Validate length
    let len = input.len as usize;
    if len > SERIAL_INPUT_MAX_SIZE {
        return -(bindings::EINVAL as isize);
    }

    // Set the input buffer on the serial state
    vm_file.vm.state.devices.serial.set_input(&input.buf[..len]);

    log_info!("SET_INPUT: set {} bytes of serial input\n", len);
    0
}
