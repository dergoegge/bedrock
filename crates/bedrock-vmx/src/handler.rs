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

#[cfg(feature = "cargo")]
use core::ptr::NonNull;
#[cfg(feature = "cargo")]
/// Opaque VM reference used by cargo tests and userland crates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VmRef(NonNull<()>);

#[cfg(feature = "cargo")]
impl VmRef {
    /// Create a new VmRef from a NonNull pointer.
    pub fn new<T>(ptr: NonNull<T>) -> Self {
        VmRef(ptr.cast())
    }

    /// Get the raw pointer.
    pub fn as_ptr(&self) -> *const () {
        self.0.as_ptr()
    }
}

#[cfg(feature = "cargo")]
// SAFETY: VmRef is only used for pointer comparison and tracking in cargo
// builds. The actual VM data access is synchronized elsewhere.
unsafe impl Send for VmRef {}

#[cfg(feature = "cargo")]
type VmHandle = VmRef;
#[cfg(not(feature = "cargo"))]
type VmHandle = ParentVmArc;

/// VM entry in the tracking list, containing both the ID and reference.
pub struct VmEntry {
    /// Unique VM identifier.
    pub vm_id: u64,
    /// Strong reference to the VM while it is open and visible by ID.
    pub vm_ref: VmHandle,
}

impl VmEntry {
    /// Create a new VmEntry.
    pub fn new(vm_id: u64, vm_ref: VmHandle) -> Self {
        Self { vm_id, vm_ref }
    }
}

/// The bedrock hypervisor handler.
///
/// This handler manages:
/// - VMX state (VMXON/VMXOFF on all CPUs)
/// - A list of all active VMs
/// - VM ID allocation
///
/// VMs are owned by file descriptors and by the handler while they are visible
/// for lookup by VM ID. Removing a VM from the handler drops the handler's
/// strong reference; forked children may hold additional parent references.
///
/// # Type Parameters
///
/// * `X` - The VMX implementation for hardware virtualization
/// * `MAX_VMS` - Maximum number of VMs that can be tracked
pub struct BedrockHandler<'a, X: Vmx, const MAX_VMS: usize = 64> {
    /// Strong references to all active VMs while they are visible by ID.
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

    /// Register a VM in the tracking list, taking a strong reference.
    ///
    /// # Arguments
    ///
    /// * `vm` - Strong VM file reference
    /// * `vm_id` - Unique identifier for this VM
    #[cfg(not(feature = "cargo"))]
    pub fn add_vm(&mut self, vm: ParentVmArc) {
        let entry = VmEntry::new(vm.vm_id(), vm);
        heap_vec_push(&mut self.vm_list, entry);
    }

    #[cfg(feature = "cargo")]
    pub fn add_vm<T>(&mut self, vm: NonNull<T>, vm_id: u64) {
        let entry = VmEntry::new(vm_id, VmRef::new(vm));
        heap_vec_push(&mut self.vm_list, entry);
    }

    /// Remove a VM from the tracking list.
    ///
    /// This drops the handler's strong reference. It should be called when the
    /// VM's file descriptor is being closed.
    pub fn remove_vm<T>(&mut self, vm: *const T) {
        let vm_ptr = vm.cast::<()>();
        self.vm_list.retain(|e| e.vm_ref.as_ptr() != vm_ptr);
    }

    /// Find a VM by its ID.
    ///
    /// Returns a cloned strong reference if found, None otherwise.
    #[cfg(not(feature = "cargo"))]
    pub fn find_vm_by_id(&self, vm_id: u64) -> Option<ParentVmArc> {
        self.vm_list
            .iter()
            .find(|e| e.vm_id == vm_id)
            .map(|e| e.vm_ref.clone())
    }

    #[cfg(feature = "cargo")]
    pub fn find_vm_by_id(&self, vm_id: u64) -> Option<VmRef> {
        self.vm_list
            .iter()
            .find(|e| e.vm_id == vm_id)
            .map(|e| e.vm_ref)
    }
}
