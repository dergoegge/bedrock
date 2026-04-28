// SPDX-License-Identifier: GPL-2.0

//! Frame allocator and VM factory implementation.

use kernel::prelude::*;

use super::ept::FrameAllocator;
use super::instruction_counter::LinuxInstructionCounter;
use super::machine::{LinuxKernel, LinuxMachine};
use super::memory::HostPhysAddr;
use super::page::{alloc_zeroed_page, KernelGuestMemory, KernelPage, PagePool};
use super::vmcs::RealVmcs;
use super::vmx::traits::{
    CowAllocator, GuestMemory, Kernel, Machine, Page, VirtualMachineControlStructure,
};
use super::vmx::RootVm;
use super::vmx_asm::VmxContextExt;

/// Seed `sample_period` for the sampling perf_event at VM creation.
///
/// The actual interval between PMIs is set by `update_mtf_state` via
/// `realign_sampling()` whenever a forced-exit target is installed, so this
/// value is only used until the first realign. Picked large enough that PMIs
/// fire roughly once a millisecond on a 3 GHz host (~0.1% overhead) for VMs
/// that never install a target — small enough to fit in the PMU's 48-bit
/// counter and avoid silent truncation.
pub(crate) const PMI_SEED_PERIOD: u64 = 1_000_000;

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
/// Uses `LinuxInstructionCounter` for deterministic instruction counting.
///
/// # Arguments
///
/// * `machine` - The machine abstraction for hardware access
/// * `memory_size` - Size of guest memory to allocate in bytes
/// * `tsc_frequency` - Configured TSC frequency in Hz
///
/// Returns `None` if allocation fails.
#[inline(never)]
pub(crate) fn create_vm(
    machine: &LinuxMachine,
    memory_size: usize,
    tsc_frequency: u64,
) -> Option<RootVm<RealVmcs, KernelGuestMemory, LinuxInstructionCounter>> {
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

    // Create the instruction counter. The free-running event provides the
    // deterministic cumulative count read at every exit; the sampling event
    // is seeded with `PMI_SEED_PERIOD` so it exists when the hypervisor first
    // installs a forced-exit target (APIC timer / stop_at_tsc), at which
    // point `update_mtf_state` realigns it to fire at the right point.
    let instruction_counter = match LinuxInstructionCounter::new(PMI_SEED_PERIOD) {
        Some(counter) => {
            log_info!("Instruction counter created successfully\n");
            counter
        }
        None => {
            log_err!("Failed to create instruction counter, using null counter\n");
            LinuxInstructionCounter::null()
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
        tsc_frequency,
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
