// SPDX-License-Identifier: GPL-2.0

//! VM file descriptor creation functions.
//!
//! This module provides functions to create anonymous inode file descriptors
//! for VMs.

use core::ffi::c_int;
use kernel::bindings;
use kernel::sync::Arc;

use super::super::c_helpers::bedrock_anon_inode_getfd;
use super::super::instruction_counter::LinuxInstructionCounter;
use super::super::page::{KernelGuestMemory, KernelPage};
use super::super::vmcs::RealVmcs;
use super::super::vmx::{ForkedVm, RootVm};
use super::super::HANDLER;
use super::core::{BedrockForkedVmFile, BedrockVmFile, ParentVmArc};
use super::forked::BEDROCK_FORKED_VM_FOPS;
use super::root::BEDROCK_VM_FOPS;

/// Create an anonymous inode file descriptor for a VM.
///
/// This function:
/// 1. Wraps the VM in a `BedrockVmFile`
/// 2. Adds it to the global vm_list for tracking
/// 3. Creates an anonymous inode file descriptor
///
/// The returned file descriptor owns the VM. When the fd is closed, the VM
/// is automatically released.
///
/// # Returns
///
/// On success, returns the new file descriptor (positive integer).
/// On failure, returns a negative error code and the VM is freed.
#[inline(never)]
pub(crate) fn create_vm_fd(
    vm: RootVm<RealVmcs, KernelGuestMemory, LinuxInstructionCounter>,
    vm_id: u64,
) -> Result<i32, kernel::error::Error> {
    // Wrap VM in BedrockVmFile and allocate under an Arc. The file
    // descriptor owns this Arc reference via private_data.
    let vm_file = Arc::new(
        BedrockVmFile::new(vm, vm_id),
        kernel::alloc::flags::GFP_KERNEL,
    )?;
    let handler_ref = ParentVmArc::Root(vm_file.clone());
    let vm_ptr = Arc::into_raw(vm_file).cast_mut();

    // Register in global vm_list. The handler owns a strong reference while
    // the VM is visible by ID.
    {
        let mut guard = HANDLER.lock();
        if let Some(handler) = guard.as_mut() {
            handler.add_vm(handler_ref);
        }
    }

    // Create anonymous inode file descriptor
    // SAFETY: The name is a valid C string literal. BEDROCK_VM_FOPS is a valid,
    // static file_operations struct. vm_ptr is a valid heap-allocated BedrockVmFile.
    // The flags are standard open flags.
    let fd = unsafe {
        bedrock_anon_inode_getfd(
            c"bedrock-vm".as_ptr(),
            &BEDROCK_VM_FOPS.0,
            vm_ptr.cast::<core::ffi::c_void>(),
            bindings::O_RDWR as c_int | bindings::O_CLOEXEC as c_int,
        )
    };

    if fd < 0 {
        // Cleanup on failure: remove from list and free
        {
            let mut guard = HANDLER.lock();
            if let Some(handler) = guard.as_mut() {
                handler.remove_vm(vm_ptr);
            }
        }
        // SAFETY: vm_ptr was created by Arc::into_raw above and has not been
        // transferred to the kernel (fd creation failed), so we drop that Arc.
        let _ = unsafe { Arc::from_raw(vm_ptr) };
        return Err(kernel::error::Error::from_errno(fd));
    }

    Ok(fd)
}

/// Create an anonymous inode file descriptor for a forked VM.
///
/// This function:
/// 1. Wraps the ForkedVm in a `BedrockForkedVmFile`
/// 2. Adds it to the global vm_list for tracking
/// 3. Creates an anonymous inode file descriptor
///
/// The ForkedVm already has its parent's children count incremented.
/// When the fd is closed, the ForkedVm is dropped, which decrements
/// the parent's children count.
///
/// # Returns
///
/// On success, returns the new file descriptor (positive integer).
/// On failure, returns a negative error code and the ForkedVm is freed.
#[inline(never)]
pub(crate) fn create_forked_vm_fd(
    vm: ForkedVm<RealVmcs, KernelPage, LinuxInstructionCounter>,
    parent: ParentVmArc,
    vm_id: u64,
) -> Result<i32, kernel::error::Error> {
    let vm_file = Arc::new(
        BedrockForkedVmFile::new(vm, parent, vm_id),
        kernel::alloc::flags::GFP_KERNEL,
    )?;
    let handler_ref = ParentVmArc::Forked(vm_file.clone());
    let vm_ptr = Arc::into_raw(vm_file).cast_mut();

    // Register in global vm_list. The handler owns a strong reference while
    // the VM is visible by ID.
    {
        let mut guard = HANDLER.lock();
        if let Some(handler) = guard.as_mut() {
            handler.add_vm(handler_ref);
        }
    }

    // SAFETY: The name is a valid C string literal. BEDROCK_FORKED_VM_FOPS is a valid,
    // static file_operations struct. vm_ptr is a valid heap-allocated BedrockForkedVmFile.
    // The flags are standard open flags.
    let fd = unsafe {
        bedrock_anon_inode_getfd(
            c"bedrock-forked-vm".as_ptr(),
            &BEDROCK_FORKED_VM_FOPS.0,
            vm_ptr.cast::<core::ffi::c_void>(),
            bindings::O_RDWR as c_int | bindings::O_CLOEXEC as c_int,
        )
    };

    if fd < 0 {
        {
            let mut guard = HANDLER.lock();
            if let Some(handler) = guard.as_mut() {
                handler.remove_vm(vm_ptr);
            }
        }
        // SAFETY: vm_ptr was created by Arc::into_raw above and has not been
        // transferred to the kernel (fd creation failed), so we drop that Arc.
        let _ = unsafe { Arc::from_raw(vm_ptr) };
        return Err(kernel::error::Error::from_errno(fd));
    }

    Ok(fd)
}
