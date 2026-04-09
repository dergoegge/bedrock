// SPDX-License-Identifier: GPL-2.0

//! VMX assembly support for guest entry/exit.

// Re-export VmxContext from vmx crate (the struct definition lives there for testability)
pub(crate) use super::vmx::VmxContext;

use super::vmx::{VmEntryError, VmRunner};
use super::vmcs::RealVmcs;

extern "C" {
    /// Enter VMX non-root operation (guest mode).
    ///
    /// This function saves host GPRs, loads guest GPRs from the context,
    /// and executes VMLAUNCH (first time) or VMRESUME (subsequent).
    /// On VM exit, guest GPRs are saved and host GPRs are restored.
    ///
    /// # Arguments
    /// * `ctx` - Pointer to VmxContext containing guest/host register state
    ///
    /// # Returns
    /// * 0 on successful VM exit
    /// * -1 on VM entry failure (check VMCS error fields)
    ///
    /// # Safety
    /// - VMCS must be loaded and properly configured before calling
    /// - HOST_RSP must point to `ctx`
    /// - HOST_RIP must point to `vmx_exit_handler`
    fn vmx_run_guest(ctx: *mut VmxContext) -> i32;

    /// VM exit handler - landing point for all VM exits.
    ///
    /// This symbol is the entry point that HOST_RIP should point to.
    /// It saves guest state, restores host state, and returns to vmx_run_guest's caller.
    ///
    /// This function should not be called directly from Rust.
    fn vmx_exit_handler();
}

/// Extension trait for VmxContext that provides kernel-specific functionality.
///
/// This adds methods that depend on the assembly code in vmx_support.S.
pub(crate) trait VmxContextExt {
    /// Get the address of vmx_exit_handler for use as HOST_RIP.
    fn exit_handler_addr() -> u64;

    /// Run the guest until a VM exit occurs.
    ///
    /// # Safety
    /// - VMCS must be loaded and properly configured
    /// - HOST_RSP in VMCS must point to this VmxContext
    /// - HOST_RIP in VMCS must point to vmx_exit_handler
    /// - Must be called with interrupts disabled
    unsafe fn run(&mut self) -> i32;
}

impl VmxContextExt for VmxContext {
    fn exit_handler_addr() -> u64 {
        vmx_exit_handler as *const () as u64
    }

    unsafe fn run(&mut self) -> i32 {
        // SAFETY: Caller guarantees VMCS is properly configured.
        // HOST_RSP must point to this VmxContext, HOST_RIP must point to vmx_exit_handler.
        // The assembly keeps RSP on kernel stack during setup (no corruption window).
        unsafe { vmx_run_guest(self) }
    }
}

/// Real VM runner that uses the assembly code in vmx_support.S.
///
/// This is the kernel-mode implementation of `VmRunner` that executes
/// actual VMX instructions via the assembly routines.
pub(crate) struct RealVmRunner;

impl RealVmRunner {
    /// Create a new RealVmRunner.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl VmRunner for RealVmRunner {
    type Vmcs = RealVmcs;

    unsafe fn run(&mut self, ctx: &mut VmxContext, _vmcs: &Self::Vmcs) -> Result<(), VmEntryError> {
        // SAFETY: The caller guarantees that:
        // - The VMCS is loaded and properly configured
        // - HOST_RSP has been set to point to ctx
        // - HOST_RIP is set to vmx_exit_handler
        // - Interrupts are in appropriate state
        let result = unsafe { ctx.run() };

        if result == 0 {
            Ok(())
        } else {
            Err(VmEntryError::VmEntryFailed)
        }
    }
}
