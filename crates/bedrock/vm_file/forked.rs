// SPDX-License-Identifier: GPL-2.0

//! Forked VM file operations and handlers.
//!
//! This module provides the file_operations callbacks for bedrock forked-vm
//! anonymous inodes. Forked VMs share most handlers with root VMs via the
//! `VmFileOps` trait.

use core::ffi::c_int;
use core::sync::atomic::AtomicBool;

use kernel::alloc::KBox;
use kernel::bindings;

use super::super::c_helpers::{
    bedrock_kva_to_phys, bedrock_remap_page, bedrock_remap_pages, bedrock_remap_vmalloc_range,
    bedrock_vma_end, bedrock_vma_pgoff, bedrock_vma_start,
};
use super::super::page::{LogBuffer, PagePool, LOG_BUFFER_SIZE};
use super::super::vmx::traits::Page;
use super::super::vmx::ForkableVm;
use super::super::vmx::{CowPageMap, ParentVm};
use super::super::HANDLER;
use super::core::BedrockForkedVmFile;
use super::handlers::{self, VmFileOps};
use super::structs::*;
use crate::memory::GuestPhysAddr;

/// Implement VmFileOps for BedrockForkedVmFile.
impl VmFileOps for BedrockForkedVmFile {
    type Vm = super::super::vmx::ForkedVm<
        super::super::vmcs::RealVmcs,
        super::super::page::KernelPage,
        super::super::instruction_counter::LinuxInstructionCounter,
    >;

    fn vm(&self) -> &Self::Vm {
        &self.vm
    }

    fn vm_mut(&mut self) -> &mut Self::Vm {
        &mut self.vm
    }

    fn vm_id(&self) -> u64 {
        self.vm_id
    }

    fn running(&self) -> &AtomicBool {
        &self.running
    }

    fn log_buffer(&self) -> Option<&LogBuffer> {
        self.log_buffer.as_ref()
    }

    fn log_buffer_mut(&mut self) -> &mut Option<LogBuffer> {
        &mut self.log_buffer
    }

    fn can_run(&self) -> bool {
        self.vm.can_run()
    }

    fn children_count(&self) -> usize {
        self.vm.children_count()
    }

    fn vm_and_pool(&mut self) -> (&mut Self::Vm, &mut PagePool) {
        (&mut self.vm, &mut self.page_pool)
    }
}

/// File operations for bedrock forked-vm anonymous inodes.
pub(crate) static BEDROCK_FORKED_VM_FOPS: SyncFileOps = {
    // SAFETY: SyncFileOps::zeroed() produces an all-zeros file_operations, which is valid.
    // We immediately set the required function pointers below.
    let mut fops: bindings::file_operations = unsafe { SyncFileOps::zeroed() };
    fops.owner = core::ptr::null_mut();
    fops.release = Some(bedrock_forked_vm_release);
    fops.unlocked_ioctl = Some(bedrock_forked_vm_ioctl);
    fops.mmap = Some(bedrock_forked_vm_mmap);
    SyncFileOps(fops)
};

/// Release callback for bedrock forked-vm files.
///
/// # Safety
///
/// - `file` must be a valid pointer to a file struct
/// - `file->private_data` must be a valid pointer to a `KBox<BedrockForkedVmFile>`
unsafe extern "C" fn bedrock_forked_vm_release(
    _inode: *mut bindings::inode,
    file: *mut bindings::file,
) -> c_int {
    // SAFETY: `file` is a valid pointer to a file struct, guaranteed by the kernel
    // VFS layer which calls this release callback.
    let private_data = unsafe { (*file).private_data };

    if private_data.is_null() {
        log_err!("bedrock_forked_vm_release: null private_data\n");
        return 0;
    }

    let vm_ptr = private_data.cast::<BedrockForkedVmFile>();
    // SAFETY: We verified private_data is non-null above, and it was set to a valid
    // KBox<BedrockForkedVmFile> pointer when the fd was created in create_forked_vm_fd.
    let vm_id = unsafe { (*vm_ptr).vm_id };
    log_info!("Releasing forked VM {} (fd closed)\n", vm_id);

    // Remove from global vm_list
    {
        let mut guard = HANDLER.lock();
        if let Some(handler) = guard.as_mut() {
            handler.remove_vm(vm_ptr);
        }
    }

    // Drop the ForkedVm - this decrements the parent's children count automatically
    // SAFETY: vm_ptr was created by KBox::into_raw in create_forked_vm_fd. This is the
    // release callback, so this is the unique point where ownership is reclaimed.
    let _ = unsafe { KBox::from_raw(vm_ptr) };

    log_info!("Forked VM {} released successfully\n", vm_id);
    0
}

/// Mmap callback for bedrock forked-vm files.
///
/// Forked VMs support mapping auxiliary buffers (serial, log, serial TSC,
/// feedback). Guest memory cannot be mapped as one contiguous region because
/// it uses COW from the parent.
///
/// # Safety
///
/// - `file` must be a valid pointer to a file struct
/// - `file->private_data` must be a valid pointer to a `BedrockForkedVmFile`
/// - `vma` must be a valid pointer to a vm_area_struct
unsafe extern "C" fn bedrock_forked_vm_mmap(
    file: *mut bindings::file,
    vma: *mut bindings::vm_area_struct,
) -> c_int {
    // SAFETY: `file` is a valid pointer guaranteed by the kernel VFS layer.
    let private_data = unsafe { (*file).private_data };

    if private_data.is_null() {
        return -(bindings::EBADF as i32);
    }

    // SAFETY: private_data is non-null (checked above) and was set to a valid
    // BedrockForkedVmFile pointer when the fd was created. We hold exclusive access
    // because the kernel serializes mmap calls per file.
    let vm_file = unsafe { &mut *(private_data.cast::<BedrockForkedVmFile>()) };

    // SAFETY: `vma` is a valid pointer to a vm_area_struct, guaranteed by the kernel
    // VFS/mmap layer. These helpers read standard VMA fields.
    let vma_start = unsafe { bedrock_vma_start(vma) };
    // SAFETY: Same as above — `vma` is a valid VMA pointer from the kernel mmap layer.
    let vma_end = unsafe { bedrock_vma_end(vma) };
    // SAFETY: Same as above — `vma` is a valid VMA pointer from the kernel mmap layer.
    let vma_pgoff = unsafe { bedrock_vma_pgoff(vma) };

    let requested_size = vma_end - vma_start;
    let offset_bytes = vma_pgoff * 4096;

    // ForkedVm doesn't have contiguous guest memory - it uses COW from parent.
    // We only allow mapping:
    // - offset 0: serial buffer (4KB)
    // - offset 4096: log buffer (1MB)
    // - offset 4096 + LOG_BUFFER_SIZE: serial TSC page (4KB)
    // - offset 4096 + LOG_BUFFER_SIZE + 4096: feedback buffer 0 (up to 1MB)
    // - offset 4096 + LOG_BUFFER_SIZE + 4096 + 1MB: feedback buffer 1 (up to 1MB)
    // - ... (each buffer slot reserves 1MB)
    const SERIAL_BUFFER_OFFSET: u64 = 0;
    const LOG_BUFFER_OFFSET: u64 = 4096;
    let serial_tsc_offset: u64 = LOG_BUFFER_OFFSET + LOG_BUFFER_SIZE as u64;
    let feedback_buffer_base_offset: u64 = serial_tsc_offset + 4096;
    const FEEDBACK_BUFFER_SLOT_SIZE: u64 = 1024 * 1024; // 1MB per slot

    // Check if this is a feedback buffer mapping
    if offset_bytes >= feedback_buffer_base_offset {
        let relative_offset = offset_bytes - feedback_buffer_base_offset;
        let buffer_index = (relative_offset / FEEDBACK_BUFFER_SLOT_SIZE) as usize;

        // Validate buffer index
        if buffer_index >= super::super::vmx::MAX_FEEDBACK_BUFFERS {
            log_err!("mmap: invalid feedback buffer index {}\n", buffer_index);
            return -(bindings::EINVAL as i32);
        }

        // Check alignment within slot
        if !relative_offset.is_multiple_of(FEEDBACK_BUFFER_SLOT_SIZE) {
            log_err!("mmap: feedback buffer offset not aligned to slot boundary\n");
            return -(bindings::EINVAL as i32);
        }

        // Feedback buffer mapping
        let feedback_buffer = match &vm_file.vm.state.feedback_buffers[buffer_index] {
            Some(fb) => fb,
            None => {
                log_err!("mmap: feedback buffer {} not registered\n", buffer_index);
                return -(bindings::EINVAL as i32);
            }
        };

        let expected_size = feedback_buffer.num_pages * 4096;
        if requested_size as usize != expected_size {
            log_err!(
                "mmap: feedback buffer {} size mismatch: expected {}, got {}\n",
                buffer_index,
                expected_size,
                requested_size
            );
            return -(bindings::EINVAL as i32);
        }

        // For forked VM, resolve each GPA to HPA:
        // 1. Check COW pages first (if guest wrote to this page, it's in COW map)
        // 2. Otherwise, get from parent chain (walks through nested forks to root)
        let mut hpas = [0u64; 256]; // FEEDBACK_BUFFER_MAX_PAGES = 256

        for (i, hpa) in hpas.iter_mut().enumerate().take(feedback_buffer.num_pages) {
            let gpa = feedback_buffer.gpas[i];
            let page_gpa = GuestPhysAddr::new(gpa);

            // Check if this page is in our COW map
            if let Some(cow_page) =
                <CowPageMap<super::super::page::KernelPage>>::get(&vm_file.vm.cow_pages, page_gpa)
            {
                // Page is in COW map - use its physical address directly
                *hpa = Page::physical_address(cow_page).as_u64();
            } else {
                // Page is in parent chain - get virtual address and convert to physical
                let virt_ptr = match vm_file.vm.read_page(page_gpa) {
                    Some(ptr) => ptr,
                    None => {
                        log_err!(
                            "mmap: failed to resolve GPA {:#x} for feedback buffer {}\n",
                            gpa,
                            buffer_index
                        );
                        return -(bindings::EINVAL as i32);
                    }
                };
                // Convert kernel virtual address to physical
                // SAFETY: virt_ptr is a valid kernel virtual address obtained from
                // read_page, which returns a pointer into the parent's guest memory.
                // bedrock_kva_to_phys converts it to a physical address.
                let phys =
                    unsafe { bedrock_kva_to_phys(virt_ptr.cast::<core::ffi::c_void>().cast_mut()) };
                if phys == 0 {
                    log_err!(
                        "mmap: kva_to_phys failed for GPA {:#x} (virt {:#x})\n",
                        gpa,
                        virt_ptr as u64
                    );
                    return -(bindings::EINVAL as i32);
                }
                *hpa = phys;
            }
        }

        // SAFETY: `vma` is a valid VMA pointer from the kernel. `hpas` contains valid
        // physical addresses resolved from COW pages or parent memory. num_pages does
        // not exceed the array size (256).
        let ret =
            unsafe { bedrock_remap_pages(vma, hpas.as_ptr(), feedback_buffer.num_pages as i32) };

        if ret != 0 {
            log_err!(
                "mmap: feedback buffer {} remap failed with {}\n",
                buffer_index,
                ret
            );
        } else {
            log_info!(
                "mmap: mapped feedback buffer {} for forked VM {} ({} pages)\n",
                buffer_index,
                vm_file.vm_id,
                feedback_buffer.num_pages
            );
        }

        ret
    } else if offset_bytes == serial_tsc_offset {
        // Serial TSC metadata page mapping
        if requested_size as usize != 4096 {
            log_err!("mmap: serial TSC page must be exactly 4096 bytes\n");
            return -(bindings::EINVAL as i32);
        }

        let page = vm_file.vm.state.serial_tsc_page.as_raw_page();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `page` is a valid
        // kernel page obtained from serial_tsc_page.
        unsafe { bedrock_remap_page(vma, page) }
    } else if offset_bytes == LOG_BUFFER_OFFSET {
        // Log buffer mapping
        if requested_size as usize != LOG_BUFFER_SIZE {
            log_err!(
                "mmap: log buffer must be exactly {} bytes\n",
                LOG_BUFFER_SIZE
            );
            return -(bindings::EINVAL as i32);
        }

        let log_buffer = match &vm_file.log_buffer {
            Some(buf) => buf,
            None => {
                log_err!("mmap: log buffer not allocated\n");
                return -(bindings::EINVAL as i32);
            }
        };

        let addr = log_buffer.as_ptr().cast::<core::ffi::c_void>();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `addr` is a valid
        // vmalloc'd pointer to the log buffer. Offset 0 maps from the start.
        unsafe { bedrock_remap_vmalloc_range(vma, addr, 0) }
    } else if offset_bytes == SERIAL_BUFFER_OFFSET {
        // Serial buffer mapping
        if requested_size as usize != 4096 {
            log_err!("mmap: serial buffer must be exactly 4096 bytes\n");
            return -(bindings::EINVAL as i32);
        }

        let page = vm_file.vm.state.serial_buffer_page.as_raw_page();
        // SAFETY: `vma` is a valid VMA pointer from the kernel. `page` is a valid
        // kernel page obtained from serial_buffer_page.
        unsafe { bedrock_remap_page(vma, page) }
    } else {
        log_err!("mmap: forked VM only supports serial, log, and TSC buffers\n");
        -(bindings::EINVAL as i32)
    }
}

/// Ioctl callback for bedrock forked-vm files.
///
/// # Safety
///
/// - `file` must be a valid pointer to a file struct
/// - `file->private_data` must be a valid pointer to a `BedrockForkedVmFile`
unsafe extern "C" fn bedrock_forked_vm_ioctl(
    file: *mut bindings::file,
    cmd: core::ffi::c_uint,
    arg: usize,
) -> isize {
    // SAFETY: `file` is a valid pointer guaranteed by the kernel VFS layer.
    let private_data = unsafe { (*file).private_data };

    if private_data.is_null() {
        return -(bindings::EBADF as isize);
    }

    // SAFETY: private_data is non-null (checked above) and was set to a valid
    // BedrockForkedVmFile pointer when the fd was created. The kernel serializes
    // ioctls per file descriptor.
    let vm_file = unsafe { &mut *(private_data.cast::<BedrockForkedVmFile>()) };

    match cmd {
        BEDROCK_VM_GET_REGS => handlers::handle_get_regs(vm_file, arg),
        BEDROCK_VM_SET_REGS => handlers::handle_set_regs(vm_file, arg),
        BEDROCK_VM_RUN => handlers::handle_run(vm_file, arg),
        BEDROCK_VM_SET_RDRAND_CONFIG => handlers::handle_set_rdrand_config(vm_file, arg),
        BEDROCK_VM_SET_RDRAND_VALUE => handlers::handle_set_rdrand_value(vm_file, arg),
        BEDROCK_VM_SET_LOG_CONFIG => handlers::handle_set_log_config(vm_file, arg),
        BEDROCK_VM_SET_SINGLE_STEP => handlers::handle_set_single_step(vm_file, arg),
        BEDROCK_VM_GET_EXIT_STATS => handlers::handle_get_exit_stats(vm_file, arg),
        BEDROCK_VM_SET_STOP_TSC => handlers::handle_set_stop_tsc(vm_file, arg),
        BEDROCK_VM_GET_VM_ID => handlers::handle_get_vm_id(vm_file, arg),
        BEDROCK_VM_GET_FEEDBACK_BUFFER_INFO => {
            handlers::handle_get_feedback_buffer_info(vm_file, arg)
        }
        _ => -(bindings::ENOTTY as isize),
    }
}
