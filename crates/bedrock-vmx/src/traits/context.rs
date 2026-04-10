// SPDX-License-Identifier: GPL-2.0

//! VmContext trait definition.
//!
//! This trait abstracts over VM state for testability, allowing the exit
//! handler logic to be tested in userland by providing mock implementations.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::{
    CowAllocator, InstructionCounter, Kernel, Machine, Page, VirtualMachineControlStructure,
    VmGetRegistersError, VmRunError, VmRunner, VmSetRegistersError, Vmx,
};

// Import implementation helpers from submodules
use super::registers::{get_registers, set_registers};
use super::vm_run::{run, sync_gprs_from_vmx_ctx, sync_gprs_to_vmx_ctx};

/// Abstraction over VM state for testability.
///
/// This trait allows the exit handler logic to be tested in userland
/// by providing mock implementations of VMCS and guest state access.
///
/// Most VM state is accessed via `state()` and `state_mut()` which return
/// references to the `VmState` struct. Memory operations are separate since
/// they differ between root and forked VMs.
pub trait VmContext {
    /// The VMCS type used by this context.
    type Vmcs: VirtualMachineControlStructure;
    /// The VMX implementation type.
    type V: Vmx;
    /// The instruction counter type.
    type I: InstructionCounter;
    /// The page type used for copy-on-write allocations (ForkedVm only).
    type CowPage: Page;

    /// Returns a reference to the VM state.
    fn state(&self) -> &VmState<Self::Vmcs, Self::I>;

    /// Returns a mutable reference to the VM state.
    fn state_mut(&mut self) -> &mut VmState<Self::Vmcs, Self::I>;

    /// Read guest memory at the given guest physical address.
    fn read_guest_memory(&self, gpa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemoryError>;

    /// Write guest memory at the given guest physical address.
    fn write_guest_memory(&mut self, gpa: GuestPhysAddr, buf: &[u8]) -> Result<(), MemoryError>;

    /// Finalize the pending log entry by computing the memory hash.
    ///
    /// This walks the EPT to find dirty pages, hashes them, and updates the
    /// memory_hash field of the pending log entry. Each VM type implements
    /// this based on how it accesses guest memory.
    ///
    /// Does nothing if there is no pending log entry.
    fn finalize_log_entry<K: Kernel>(&mut self, kernel: &K);

    // ========== Copy-on-Write Methods ==========

    /// Handle a copy-on-write fault at the given guest physical address.
    ///
    /// This is called by the EPT violation handler when a write access is made
    /// to a page that is mapped as read-only (R+X) in the EPT.
    ///
    /// For root VMs, this returns `None` (COW not supported).
    /// For forked VMs, this should:
    /// 1. Allocate a new page using the provided allocator
    /// 2. Copy the content from the parent page
    /// 3. Remap the EPT entry to point to the new page with RWX
    /// 4. Return `Some(ExitHandlerResult::Continue)` to retry the instruction
    ///
    /// Returns `None` if COW is not supported or the fault cannot be handled.
    fn handle_cow_fault<A: CowAllocator<Self::CowPage>>(
        &mut self,
        _gpa: GuestPhysAddr,
        _allocator: &mut A,
    ) -> Option<ExitHandlerResult> {
        None // Default: no COW support (RootVm)
    }

    /// Check if this VM is a forked VM (supports copy-on-write).
    ///
    /// Root VMs return `false`, forked VMs return `true`.
    fn is_forked(&self) -> bool {
        false
    }

    /// Pre-COW all feedback buffer pages to ensure stable physical addresses.
    ///
    /// This should be called at fork time to pre-COW all registered feedback buffers.
    ///
    /// By pre-COWing these pages, userspace can mmap them without needing
    /// to remap when the guest later writes to them.
    ///
    /// For root VMs, this is a no-op (no COW needed).
    /// For forked VMs, this iterates through all registered feedback buffers and COWs them.
    fn pre_cow_feedback_buffers<A: CowAllocator<Self::CowPage>>(&mut self, _allocator: &mut A) {
        // Default: no-op for root VMs
    }

    /// Pre-COW feedback buffer pages at a specific index.
    ///
    /// This should be called at registration time (hypercall) when a buffer is
    /// registered after fork.
    ///
    /// For root VMs, this is a no-op (no COW needed).
    /// For forked VMs, this COWs the pages of the specified feedback buffer.
    fn pre_cow_feedback_buffer_at<A: CowAllocator<Self::CowPage>>(
        &mut self,
        _index: usize,
        _allocator: &mut A,
    ) {
        // Default: no-op for root VMs
    }

    // ========== Register Methods ==========

    /// Set guest registers from the provided register structs.
    ///
    /// The VMCS must be loaded before calling this method.
    #[allow(clippy::too_many_arguments)]
    fn set_registers(
        &mut self,
        gprs: &GeneralPurposeRegisters,
        control_regs: &ControlRegisters,
        debug_regs: &DebugRegisters,
        segment_regs: &SegmentRegisters,
        descriptor_tables: &DescriptorTableRegisters,
        extended_control_regs: &ExtendedControlRegisters,
        rip: u64,
        rflags: u64,
    ) -> Result<(), VmSetRegistersError> {
        set_registers(
            self.state_mut(),
            gprs,
            control_regs,
            debug_regs,
            segment_regs,
            descriptor_tables,
            extended_control_regs,
            rip,
            rflags,
        )
    }

    /// Set guest registers with VMCS guarded load/clear.
    #[allow(clippy::too_many_arguments)]
    fn set_registers_guarded(
        &mut self,
        gprs: &GeneralPurposeRegisters,
        control_regs: &ControlRegisters,
        debug_regs: &DebugRegisters,
        segment_regs: &SegmentRegisters,
        descriptor_tables: &DescriptorTableRegisters,
        extended_control_regs: &ExtendedControlRegisters,
        rip: u64,
        rflags: u64,
    ) -> Result<(), VmSetRegistersError> {
        self.state()
            .vmcs
            .load()
            .map_err(VmSetRegistersError::VmcsGuard)?;

        let result = self.set_registers(
            gprs,
            control_regs,
            debug_regs,
            segment_regs,
            descriptor_tables,
            extended_control_regs,
            rip,
            rflags,
        );

        self.state()
            .vmcs
            .clear()
            .map_err(VmSetRegistersError::VmcsGuard)?;

        result
    }

    /// Get all guest registers from VMCS and GPR state.
    ///
    /// The VMCS must be loaded before calling this method.
    #[allow(clippy::type_complexity)]
    fn get_registers(
        &self,
    ) -> Result<
        (
            GeneralPurposeRegisters,
            ControlRegisters,
            DebugRegisters,
            SegmentRegisters,
            DescriptorTableRegisters,
            ExtendedControlRegisters,
            u64, // rip
            u64, // rflags
        ),
        VmGetRegistersError,
    > {
        get_registers(self.state())
    }

    /// Get all guest registers with VMCS guarded load/clear.
    #[allow(clippy::type_complexity)]
    fn get_registers_guarded(
        &self,
    ) -> Result<
        (
            GeneralPurposeRegisters,
            ControlRegisters,
            DebugRegisters,
            SegmentRegisters,
            DescriptorTableRegisters,
            ExtendedControlRegisters,
            u64, // rip
            u64, // rflags
        ),
        VmGetRegistersError,
    > {
        self.state()
            .vmcs
            .load()
            .map_err(VmGetRegistersError::VmcsGuard)?;

        let result = self.get_registers();

        self.state()
            .vmcs
            .clear()
            .map_err(VmGetRegistersError::VmcsGuard)?;

        result
    }

    // ========== GPR Sync Methods ==========

    /// Copy GPRs from GeneralPurposeRegisters to VmxContext guest registers.
    /// Also sets up XSAVE area pointers for extended state management.
    fn sync_gprs_to_vmx_ctx(&mut self) {
        sync_gprs_to_vmx_ctx(self.state_mut())
    }

    /// Copy GPRs from VmxContext guest registers to GeneralPurposeRegisters.
    fn sync_gprs_from_vmx_ctx(&mut self) {
        sync_gprs_from_vmx_ctx(self.state_mut())
    }

    // ========== VM Run Methods ==========

    /// Run the VM until an exit requiring userspace handling.
    ///
    /// This is the main entry point for running the VM. It:
    /// 1. Saves host MSRs (KERNEL_GS_BASE, SYSCALL/SYSRET MSRs)
    /// 2. Loads guest MSRs that don't have VMCS fields
    /// 3. Loads the VMCS and runs the VM loop
    /// 4. Restores host MSRs on exit
    ///
    /// # Safety
    ///
    /// Caller must ensure:
    /// - VMCS is properly configured
    /// - Interrupts are in appropriate state
    /// - HOST_RIP is correctly set in VMCS
    /// - Preemption is disabled to prevent migration during VM entry/exit
    unsafe fn run<R: VmRunner<Vmcs = Self::Vmcs>, M: Machine, A: CowAllocator<Self::CowPage>>(
        &mut self,
        runner: &mut R,
        machine: &M,
        allocator: &mut A,
    ) -> Result<ExitReason, VmRunError>
    where
        Self: Sized,
    {
        // SAFETY: The caller upholds all safety requirements documented on this method:
        // VMCS is configured, interrupts are appropriate, and preemption is disabled.
        unsafe { run(self, runner, machine, allocator) }
    }
}
