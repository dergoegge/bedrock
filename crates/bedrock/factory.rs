// SPDX-License-Identifier: GPL-2.0

//! Frame allocator and VM factory implementation.

use kernel::prelude::*;

use super::adaptive_instruction_counter::AdaptiveInstructionCounter;
use super::ept::FrameAllocator;
use super::instruction_counter::LinuxInstructionCounter;
use super::machine::{LinuxKernel, LinuxMachine};
use super::memory::HostPhysAddr;
use super::page::{alloc_zeroed_page, KernelGuestMemory, KernelPage, PagePool};
use super::pebs_instruction_counter::PebsInstructionCounter;
use super::vmcs::RealVmcs;
use super::vmx::traits::{
    CowAllocator, GuestMemory, Kernel, Machine, Page, VirtualMachineControlStructure,
};
use super::vmx::RootVm;
use super::vmx_asm::VmxContextExt;

/// Frame allocator for EPT page tables that uses the Kernel trait.
///
/// When `pool` is `Some`, pages are taken from the pre-filled pool (for use
/// during the VM run loop with preemption disabled). When `pool` is `None`,
/// pages are allocated directly with `GFP_KERNEL` (for VM creation/fork paths
/// that run in sleepable context).
pub(crate) struct KernelFrameAllocator<'a> {
    kernel: &'a LinuxKernel,
    pool: Option<&'a mut PagePool>,
}

impl<'a> KernelFrameAllocator<'a> {
    /// No pool - direct GFP_KERNEL allocation (fork/creation path).
    pub(crate) fn new(kernel: &'a LinuxKernel) -> Self {
        Self { kernel, pool: None }
    }

    /// With pool - takes from pool during run loop (preemption disabled).
    pub(crate) fn new_with_pool(kernel: &'a LinuxKernel, pool: &'a mut PagePool) -> Self {
        Self {
            kernel,
            pool: Some(pool),
        }
    }
}

/// Error type for kernel frame allocation.
#[derive(Debug)]
pub(crate) struct KernelAllocError;

impl FrameAllocator for KernelFrameAllocator<'_> {
    type Error = KernelAllocError;
    type Frame = KernelPage;

    fn allocate_frame(&mut self) -> Result<KernelPage, Self::Error> {
        if let Some(ref mut pool) = self.pool {
            pool.take().ok_or(KernelAllocError)
        } else {
            alloc_zeroed_page().ok_or(KernelAllocError)
        }
    }

    fn frame_phys_addr(frame: &KernelPage) -> HostPhysAddr {
        frame.physical_address()
    }

    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8 {
        self.kernel.phys_to_virt(phys)
    }
}

impl CowAllocator<KernelPage> for KernelFrameAllocator<'_> {
    fn allocate_cow_page(&mut self) -> Result<KernelPage, Self::Error> {
        if let Some(ref mut pool) = self.pool {
            pool.take().ok_or(KernelAllocError)
        } else {
            alloc_zeroed_page().ok_or(KernelAllocError)
        }
    }
}

/// Create a new VM with the specified guest memory size.
///
/// This allocates all VM resources: VMCS, guest memory, EPT tables.
/// Uses `AdaptiveInstructionCounter` which selects PEBS+PDist (zero-skid)
/// on capable hardware, falling back to Linux perf_event polling otherwise.
///
/// # Arguments
///
/// * `machine` - The machine abstraction for hardware access
/// * `memory_size` - Size of guest memory to allocate in bytes
///
/// Returns `None` if allocation fails.
#[inline(never)]
pub(crate) fn create_vm(
    machine: &LinuxMachine,
    memory_size: usize,
) -> Option<RootVm<RealVmcs, KernelGuestMemory, AdaptiveInstructionCounter>> {
    log_info!("create_vm: starting with memory_size={}\n", memory_size);

    // Allocate a VMCS for this VM.
    log_info!("create_vm: allocating VMCS\n");
    let vmcs = RealVmcs::new(machine).ok()?;
    log_info!("create_vm: VMCS allocated\n");

    // Allocate guest memory.
    log_info!(
        "create_vm: allocating guest memory ({} bytes)\n",
        memory_size
    );
    let memory = machine.kernel().alloc_guest_memory(memory_size)?;
    log_info!("create_vm: guest memory allocated\n");
    log_info!(
        "Allocated {} bytes of guest memory at virtual address {:p}\n",
        memory.size(),
        memory.virt_addr().as_u64() as *const u8
    );

    // Create frame allocator for EPT
    let mut allocator = KernelFrameAllocator::new(machine.kernel());

    // Get exit handler address for HOST_RIP
    let exit_handler_rip = super::vmx::VmxContext::exit_handler_addr();
    log_info!("Exit handler RIP: {:#x}\n", exit_handler_rip);

    // Create instruction counter for deterministic execution.
    // Try PEBS+PDist first (zero-skid overflow), fall back to perf_event polling.
    //
    // Userspace reserves the last page of guest memory as the PEBS DS area
    // (see `setup_pebs_ds_area_reservation` in bedrock-vm): the page is
    // listed as E820 RAM so Linux direct-maps it, and a setup_data entry at
    // that page causes Linux to memblock_reserve it. We compute the host
    // pointer (within the vmalloc'd guest memory) and the guest linear
    // address (Linux's direct-map VA) for the PEBS counter.
    const LINUX_DIRECT_MAP_BASE: u64 = 0xffff_8880_0000_0000;
    const PAGE_SIZE: usize = 4096;
    let ds_area_gpa = (memory_size - PAGE_SIZE) as u64;
    // SAFETY: memory.virt_addr() + memory_size - PAGE_SIZE points to the last
    // page of the VM's vmalloc'd guest memory (reserved for PEBS).
    let ds_area_host_ptr =
        unsafe { (memory.virt_addr().as_u64() as *mut u8).add(memory_size - PAGE_SIZE) };
    let ds_area_guest_virt = LINUX_DIRECT_MAP_BASE + ds_area_gpa;

    let instruction_counter =
        if let Some(pebs) = PebsInstructionCounter::new(ds_area_host_ptr, ds_area_guest_virt) {
            log_info!(
                "Using PEBS+PDist zero-skid instruction counter (DS area GPA={:#x})\n",
                ds_area_gpa
            );
            AdaptiveInstructionCounter::Pebs(pebs)
        } else {
            match LinuxInstructionCounter::new() {
                Some(counter) => {
                    log_info!("Using Linux perf_event instruction counter (fallback)\n");
                    AdaptiveInstructionCounter::PerfEvent(counter)
                }
                None => {
                    log_err!("Failed to create instruction counter, using null counter\n");
                    AdaptiveInstructionCounter::PerfEvent(LinuxInstructionCounter::null())
                }
            }
        };

    // Create RootVm with EPT mapping, MSR bitmap, and instruction counter
    match RootVm::new(
        vmcs,
        memory,
        machine,
        &mut allocator,
        exit_handler_rip,
        instruction_counter,
    ) {
        Ok(vm) => {
            log_info!("RootVm created successfully\n");

            // RTC uses a fixed base time (2024-01-01 00:00:00 UTC) for deterministic
            // execution. Time advances based on emulated TSC, not host time.
            log_info!(
                "RTC initialized with fixed base time: {}\n",
                vm.state.devices.rtc.base_time
            );

            Some(vm)
        }
        Err(e) => {
            log_err!("RootVm::new failed: {:?}\n", e);
            None
        }
    }
}
