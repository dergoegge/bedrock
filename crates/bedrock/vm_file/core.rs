// SPDX-License-Identifier: GPL-2.0

//! Core VM file structures.
//!
//! This module defines the per-VM state stored in file descriptors.

use core::sync::atomic::AtomicBool;

use super::super::instruction_counter::LinuxInstructionCounter;
use super::super::page::{KernelGuestMemory, KernelPage, LogBuffer, PagePool};
use super::super::vmcs::RealVmcs;
use super::super::vmx::{ForkedVm, RootVm};

/// Type discriminant for VM file structs.
///
/// This must be the first field of both BedrockVmFile and BedrockForkedVmFile,
/// allowing safe type identification through a raw pointer.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum VmFileType {
    /// Root VM that owns its memory.
    Root = 0,
    /// Forked VM using copy-on-write from a parent.
    Forked = 1,
}

/// Per-VM state owned by the file descriptor.
///
/// This struct is stored in `file->private_data` for bedrock-vm anonymous inodes.
/// When the file descriptor is closed, this struct is dropped, freeing all VM
/// resources.
#[repr(C)]
pub(crate) struct BedrockVmFile {
    /// Type discriminant - MUST be first field for safe type identification.
    pub vm_file_type: VmFileType,
    /// The actual VM with VMCS, guest memory, EPT, etc.
    pub vm: RootVm<RealVmcs, KernelGuestMemory, LinuxInstructionCounter>,
    /// Unique identifier for this VM.
    pub vm_id: u64,
    /// Flag to detect concurrent access to RUN ioctl.
    /// Set to true when RUN is in progress, false otherwise.
    pub running: AtomicBool,
    /// Optional log buffer for deterministic exit logging.
    /// Allocated on ENABLE_LOGGING, freed on DISABLE_LOGGING or file close.
    pub log_buffer: Option<LogBuffer>,
    /// Pre-allocated page pool for COW allocation during run loop.
    /// Root VMs don't do COW, so target=0.
    pub page_pool: PagePool,
}

impl BedrockVmFile {
    /// Create a new BedrockVmFile wrapping a RootVm.
    pub(crate) fn new(
        vm: RootVm<RealVmcs, KernelGuestMemory, LinuxInstructionCounter>,
        vm_id: u64,
    ) -> Self {
        Self {
            vm_file_type: VmFileType::Root,
            vm,
            vm_id,
            running: AtomicBool::new(false),
            log_buffer: None,
            page_pool: PagePool::new(0),
        }
    }
}

/// Per-forked-VM state owned by the file descriptor.
///
/// This struct is stored in `file->private_data` for bedrock forked-vm anonymous inodes.
/// When the file descriptor is closed, this struct is dropped, freeing all VM
/// resources and decrementing the parent's children count.
#[repr(C)]
pub(crate) struct BedrockForkedVmFile {
    /// Type discriminant - MUST be first field for safe type identification.
    pub vm_file_type: VmFileType,
    /// The forked VM with COW memory.
    pub vm: ForkedVm<RealVmcs, KernelPage, LinuxInstructionCounter>,
    /// Unique identifier for this VM.
    pub vm_id: u64,
    /// Flag to detect concurrent access to RUN ioctl.
    pub running: AtomicBool,
    /// Optional log buffer for deterministic exit logging.
    pub log_buffer: Option<LogBuffer>,
    /// Pre-allocated page pool for COW allocation during run loop.
    pub page_pool: PagePool,
}

/// Number of pages to pre-allocate in the COW page pool for forked VMs.
/// 512 pages = 2MB. The pool is refilled when it drops below 5% of target.
pub(crate) const COW_POOL_SIZE: usize = 512;

impl BedrockForkedVmFile {
    /// Create a new BedrockForkedVmFile wrapping a ForkedVm.
    pub(crate) fn new(
        vm: ForkedVm<RealVmcs, KernelPage, LinuxInstructionCounter>,
        vm_id: u64,
    ) -> Self {
        Self {
            vm_file_type: VmFileType::Forked,
            vm,
            vm_id,
            running: AtomicBool::new(false),
            log_buffer: None,
            page_pool: PagePool::new(COW_POOL_SIZE),
        }
    }
}

/// Read the VM file type from a raw pointer.
///
/// # Safety
///
/// The pointer must point to either a valid `BedrockVmFile` or `BedrockForkedVmFile`.
/// Both structs must have `vm_file_type` as their first field.
pub(crate) unsafe fn read_vm_file_type(ptr: *const ()) -> VmFileType {
    // SAFETY: Both BedrockVmFile and BedrockForkedVmFile have vm_file_type as their
    // first field (enforced by #[repr(C)] and struct layout), so we can safely read
    // the first byte to determine the type.
    unsafe { *(ptr.cast::<VmFileType>()) }
}
