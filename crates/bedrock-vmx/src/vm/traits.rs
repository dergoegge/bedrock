// SPDX-License-Identifier: GPL-2.0

//! VM traits for fork hierarchy support.
//!
//! This module defines the traits that allow VMs to participate in fork hierarchies
//! with copy-on-write memory sharing.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Object-safe trait for parent VM access in fork hierarchies.
///
/// This trait allows ForkedVm to interact with its parent without knowing
/// the concrete parent type. It provides:
/// - Memory access: reading pages through the COW chain
/// - Child tracking: decrementing children count on drop
///
/// When a ForkedVm needs to read memory that isn't in its own COW pages,
/// it calls `read_page` on its parent, which recursively walks the chain
/// until reaching the RootVm.
///
/// # Safety
///
/// The returned pointer from `read_page` is only valid while the parent VM exists.
/// ForkedVm's children counter mechanism ensures the parent outlives its children.
pub trait ParentVm {
    /// Read a page at the given guest physical address.
    ///
    /// Returns a pointer to the page data, or None if the GPA is out of range.
    /// For RootVm, this returns a pointer into contiguous guest memory.
    /// For ForkedVm, this checks COW pages first, then delegates to its parent.
    fn read_page(&self, gpa: GuestPhysAddr) -> Option<*const u8>;

    /// Get the total guest memory size.
    fn memory_size(&self) -> usize;

    /// Remove a child from this VM's children count.
    /// Called when a ForkedVm is dropped to update the parent's count.
    fn remove_child(&self);
}

/// Trait for VM types that can be forked (used as parents for ForkedVm).
///
/// This trait provides the interface needed to create copy-on-write child VMs.
/// Both `RootVm` and `ForkedVm` implement this trait, allowing nested forking.
///
/// Extends `ParentVm` to provide the memory reading interface needed
/// for COW chain traversal.
pub trait ForkableVm<V: VirtualMachineControlStructure, I: InstructionCounter>: ParentVm {
    /// The page type used by this VM.
    type Page: Page;

    /// Get the VmState for this VM.
    fn vm_state(&self) -> &VmState<V, I>;

    /// Get the mutable VmState for this VM.
    fn vm_state_mut(&mut self) -> &mut VmState<V, I>;

    /// Increment children count.
    ///
    /// Called when a child ForkedVm is created from this VM.
    /// Takes &self since children_count uses atomic operations.
    fn add_child(&self);

    /// Decrement children count.
    ///
    /// Called when a child ForkedVm is dropped.
    /// Takes &self since children_count uses atomic operations.
    fn remove_child(&self);

    /// Get the current number of child VMs.
    fn children_count(&self) -> usize;

    /// Check if this VM can be run (no children).
    fn can_run(&self) -> bool {
        self.children_count() == 0
    }
}
