// SPDX-License-Identifier: GPL-2.0

//! Machine abstraction traits for VMX operations.
//!
//! These traits abstract hardware access for testability.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::{Kernel, Page, VirtualMachineControlStructure, Vmx, VmxCpu};

/// Machine abstraction for hardware access.
///
/// This trait groups all hardware access traits together, allowing
/// the VM run loop to be tested without actual hardware.
pub trait Machine: Send + Sync {
    /// The page type for memory allocation.
    type P: Page;
    /// The kernel interface type.
    type K: Kernel<P = Self::P>;
    /// The MSR access type.
    type M: MsrAccess;
    /// The control register access type.
    type C: CrAccess;
    /// The descriptor table access type.
    type D: DescriptorTableAccess;
    /// The VMX implementation type.
    type V: Vmx<M = Self>;
    /// The per-CPU VMX state type.
    type Vcpu: VmxCpu<M = Self> + 'static;

    /// Get a reference to the kernel interface.
    fn kernel(&self) -> &Self::K;
    /// Get a reference to the MSR access interface.
    fn msr_access(&self) -> &Self::M;
    /// Get a reference to the control register access interface.
    fn cr_access(&self) -> &Self::C;
    /// Get a reference to the descriptor table access interface.
    fn descriptor_table_access(&self) -> &Self::D;
}

/// Error from VM entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmEntryError {
    /// VMLAUNCH or VMRESUME failed.
    VmEntryFailed,
}

/// Trait for executing VM entry/exit cycles.
///
/// This abstracts the low-level VM entry mechanism (assembly code)
/// from the high-level VM run loop, allowing the run loop to be
/// tested without actual hardware.
pub trait VmRunner {
    /// The VMCS type used by this runner.
    type Vmcs: VirtualMachineControlStructure;

    /// Execute a single VM entry/exit cycle.
    ///
    /// This function:
    /// 1. Loads guest GPRs from VmxContext into CPU registers
    /// 2. Executes VMLAUNCH (first time) or VMRESUME (subsequent)
    /// 3. On VM exit, saves guest GPRs back to VmxContext
    /// 4. Returns success or failure
    ///
    /// # Arguments
    ///
    /// * `ctx` - VMX context containing guest/host register state
    /// * `vmcs` - The VMCS for this VM
    ///
    /// # Returns
    ///
    /// * `Ok(())` - VM exit occurred normally
    /// * `Err(VmEntryError)` - VM entry failed
    ///
    /// # Safety
    ///
    /// Caller must ensure:
    /// - VMCS is loaded and properly configured
    /// - HOST_RSP points to `ctx`
    /// - Interrupts are in appropriate state
    /// - HOST_RIP is correctly set to the exit handler
    unsafe fn run(
        &mut self,
        ctx: &mut super::VmxContext,
        vmcs: &Self::Vmcs,
    ) -> Result<(), VmEntryError>;
}
