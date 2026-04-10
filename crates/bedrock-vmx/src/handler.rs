// SPDX-License-Identifier: GPL-2.0

//! Bedrock handler implementation.
//!
//! The handler manages VMX state and tracks active VMs. In the KVM-style
//! architecture, VMs are owned by file descriptors (via anon_inodes), but
//! the handler maintains a list of all VMs for administration purposes.

#[cfg(not(feature = "cargo"))]
use super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use core::ptr::NonNull;

/// Opaque VM reference for tracking in the handler.
///
/// This is a type-erased pointer to a VM. In kernel mode, this points to
/// a `BedrockVmFile`. The handler doesn't own these pointers - ownership
/// is with the file descriptors.
///
/// We wrap `NonNull<()>` to implement `Send`, which is safe because:
/// - These are weak references only used for tracking (pointer comparison)
/// - The actual VM data is protected by proper synchronization when accessed
/// - We never dereference these pointers across thread boundaries
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VmRef(NonNull<()>);

impl VmRef {
    /// Create a new VmRef from a NonNull pointer.
    pub fn new<T>(ptr: NonNull<T>) -> Self {
        VmRef(ptr.cast())
    }

    /// Get the raw pointer.
    pub fn as_ptr(&self) -> *mut () {
        self.0.as_ptr()
    }
}

// SAFETY: VmRef is only used for pointer comparison and tracking.
// The actual VM data access is properly synchronized elsewhere.
unsafe impl Send for VmRef {}

/// VM entry in the tracking list, containing both the ID and reference.
#[derive(Clone, Copy)]
pub struct VmEntry {
    /// Unique VM identifier.
    pub vm_id: u64,
    /// Opaque reference to the VM.
    pub vm_ref: VmRef,
}

impl VmEntry {
    /// Create a new VmEntry.
    pub fn new(vm_id: u64, vm_ref: VmRef) -> Self {
        Self { vm_id, vm_ref }
    }
}

/// The bedrock hypervisor handler.
///
/// This handler manages:
/// - VMX state (VMXON/VMXOFF on all CPUs)
/// - A list of all active VMs (weak references for tracking)
/// - VM ID allocation
///
/// VMs are NOT owned by the handler. They are owned by file descriptors
/// and the handler only keeps weak references for administrative purposes.
///
/// # Type Parameters
///
/// * `X` - The VMX implementation for hardware virtualization
/// * `MAX_VMS` - Maximum number of VMs that can be tracked
pub struct BedrockHandler<'a, X: Vmx, const MAX_VMS: usize = 64> {
    /// Weak references to all active VMs (not owned).
    /// These are raw pointers to VM file structures with their IDs.
    vm_list: HeapVec<VmEntry>,
    /// Next VM ID to assign (monotonically increasing).
    next_vm_id: u64,
    /// Marker for VMX type and lifetime.
    _marker: core::marker::PhantomData<&'a X>,
}

impl<'a, X: Vmx, const MAX_VMS: usize> BedrockHandler<'a, X, MAX_VMS> {
    /// Create a new handler.
    ///
    /// This initializes VMX on all processors before creating the handler.
    ///
    /// # Errors
    ///
    /// Returns `VmxInitError` if VMX initialization fails.
    pub fn new(machine: &'a X::M) -> Result<Self, VmxInitError> {
        X::initialize(machine)?;

        let vm_list =
            heap_vec_with_capacity(MAX_VMS).map_err(|_| VmxInitError::MemoryAllocationFailed)?;

        Ok(Self {
            vm_list,
            next_vm_id: 1,
            _marker: core::marker::PhantomData,
        })
    }

    /// Check if we can create more VMs.
    pub fn can_create_vm(&self) -> bool {
        self.vm_list.len() < MAX_VMS
    }

    /// Allocate a unique VM ID.
    ///
    /// Returns `None` if we've reached the maximum number of VMs.
    pub fn alloc_vm_id(&mut self) -> Option<u64> {
        if !self.can_create_vm() {
            return None;
        }
        let id = self.next_vm_id;
        self.next_vm_id += 1;
        Some(id)
    }

    /// Register a VM in the tracking list.
    ///
    /// This adds a weak reference to the VM for administrative tracking.
    /// The VM is NOT owned by the handler - ownership remains with the
    /// file descriptor.
    ///
    /// # Arguments
    ///
    /// * `vm` - Pointer to the VM file structure
    /// * `vm_id` - Unique identifier for this VM
    ///
    /// # Safety
    ///
    /// The caller must ensure that `vm` points to a valid VM that will
    /// remain valid until `remove_vm` is called.
    pub fn add_vm<T>(&mut self, vm: NonNull<T>, vm_id: u64) {
        let vm_ref = VmRef::new(vm);
        let entry = VmEntry::new(vm_id, vm_ref);
        heap_vec_push(&mut self.vm_list, entry);
    }

    /// Remove a VM from the tracking list.
    ///
    /// This removes the weak reference to the VM. Should be called when
    /// the VM's file descriptor is being closed.
    pub fn remove_vm<T>(&mut self, vm: *mut T) {
        let vm_ptr = vm.cast::<()>();
        self.vm_list.retain(|e| e.vm_ref.as_ptr() != vm_ptr);
    }

    /// Find a VM by its ID.
    ///
    /// Returns the VmRef if found, None otherwise.
    pub fn find_vm_by_id(&self, vm_id: u64) -> Option<VmRef> {
        self.vm_list
            .iter()
            .find(|e| e.vm_id == vm_id)
            .map(|e| e.vm_ref)
    }
}
