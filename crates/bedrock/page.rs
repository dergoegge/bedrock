// SPDX-License-Identifier: GPL-2.0

//! Kernel page and guest memory allocation.

use kernel::alloc::flags::{GFP_KERNEL, __GFP_ZERO};
use kernel::alloc::{allocator::KVmalloc, Vec as KVec};
use kernel::page::Page;

use super::c_helpers;
use super::memory::{HostPhysAddr, VirtAddr};
use super::vmx::traits::{GuestMemory, Page as PageTrait};

/// A kernel-allocated page wrapping the kernel crate's Page type.
pub(crate) struct KernelPage {
    pub(crate) page: Page,
    pub(crate) phys: HostPhysAddr,
    pub(crate) virt: VirtAddr,
}

impl PageTrait for KernelPage {
    fn physical_address(&self) -> HostPhysAddr {
        self.phys
    }

    fn virtual_address(&self) -> VirtAddr {
        self.virt
    }
}

impl KernelPage {
    /// Returns the raw kernel page pointer for use with remap_pfn_range.
    pub(crate) fn as_raw_page(&self) -> *mut kernel::bindings::page {
        self.page.as_ptr()
    }
}

/// Guest memory allocated via vmalloc_user.
///
/// This memory is virtually contiguous and can be mapped to userspace.
/// It is automatically freed when dropped.
pub(crate) struct KernelGuestMemory {
    ptr: *mut u8,
    size: usize,
}

// SAFETY: KernelGuestMemory contains a raw pointer but the memory it points to
// is owned exclusively by this struct and can be safely sent between threads.
unsafe impl Send for KernelGuestMemory {}
// SAFETY: KernelGuestMemory only provides shared access through &self methods
// that don't allow mutation of the underlying memory without &mut self.
unsafe impl Sync for KernelGuestMemory {}

impl KernelGuestMemory {
    /// Allocate guest memory of the given size.
    pub(crate) fn new(size: usize) -> Option<Self> {
        log_info!(
            "KernelGuestMemory::new: calling vmalloc_user({} bytes)\n",
            size
        );
        // SAFETY: bedrock_vmalloc_user allocates zeroed memory that can be mapped to userspace.
        let ptr = unsafe { c_helpers::bedrock_vmalloc_user(size as core::ffi::c_ulong) };
        log_info!("KernelGuestMemory::new: vmalloc_user returned {:p}\n", ptr);
        if ptr.is_null() {
            return None;
        }

        Some(Self {
            ptr: ptr as *mut u8,
            size,
        })
    }
}

impl GuestMemory for KernelGuestMemory {
    fn size(&self) -> usize {
        self.size
    }

    fn virt_addr(&self) -> VirtAddr {
        VirtAddr::new(self.ptr as u64)
    }

    fn page_phys_addr(&self, page_offset: usize) -> Option<HostPhysAddr> {
        if page_offset >= self.size {
            return None;
        }
        // SAFETY: ptr + page_offset is within the allocated vmalloc region.
        let page_ptr = unsafe { self.ptr.add(page_offset) };
        let phys =
            unsafe { c_helpers::bedrock_vmalloc_to_phys(page_ptr as *mut core::ffi::c_void) };
        if phys == 0 {
            return None;
        }
        Some(HostPhysAddr::new(phys))
    }
}

impl Drop for KernelGuestMemory {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: ptr was allocated by bedrock_vmalloc_user and has not been freed yet.
            unsafe {
                c_helpers::bedrock_vfree(self.ptr as *mut core::ffi::c_void);
            }
        }
    }
}

/// Log buffer for deterministic VM exit logging.
///
/// This is a 1MB vmalloc'd buffer that can be mapped to userspace.
pub(crate) struct LogBuffer {
    ptr: *mut u8,
}

/// Log buffer size: 1MB (256 pages).
pub(crate) const LOG_BUFFER_SIZE: usize = 1024 * 1024;

// SAFETY: LogBuffer contains a raw pointer but the memory it points to
// is owned exclusively by this struct and can be safely sent between threads.
unsafe impl Send for LogBuffer {}
// SAFETY: LogBuffer only provides shared access through &self methods.
unsafe impl Sync for LogBuffer {}

impl LogBuffer {
    /// Allocate a new log buffer.
    pub(crate) fn new() -> Option<Self> {
        log_info!("LogBuffer::new: allocating {} bytes\n", LOG_BUFFER_SIZE);
        // SAFETY: bedrock_vmalloc_user allocates zeroed memory that can be mapped to userspace.
        let ptr = unsafe { c_helpers::bedrock_vmalloc_user(LOG_BUFFER_SIZE as core::ffi::c_ulong) };
        if ptr.is_null() {
            log_err!("LogBuffer::new: vmalloc_user failed\n");
            return None;
        }
        log_info!("LogBuffer::new: allocated at {:p}\n", ptr);
        Some(Self {
            ptr: ptr as *mut u8,
        })
    }

    /// Get the pointer to the buffer.
    pub(crate) fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }
}

impl Drop for LogBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            log_info!("LogBuffer::drop: freeing {:p}\n", self.ptr);
            // SAFETY: ptr was allocated by bedrock_vmalloc_user and has not been freed yet.
            unsafe {
                c_helpers::bedrock_vfree(self.ptr as *mut core::ffi::c_void);
            }
        }
    }
}

/// Allocate a zeroed kernel page.
pub(crate) fn alloc_zeroed_page() -> Option<KernelPage> {
    // Allocate a zeroed page using the kernel crate's Page API.
    let page = Page::alloc_page(GFP_KERNEL | __GFP_ZERO).ok()?;

    // Get the physical address using our C helper.
    // SAFETY: page.as_ptr() returns a valid struct page pointer.
    let phys_addr = unsafe { c_helpers::bedrock_page_to_phys(page.as_ptr()) };

    // Get the virtual address (kernel linear mapping) using our C helper.
    // SAFETY: page.as_ptr() returns a valid struct page pointer.
    let virt_addr = unsafe { c_helpers::bedrock_page_address(page.as_ptr()) as u64 };

    Some(KernelPage {
        page,
        phys: HostPhysAddr::new(phys_addr),
        virt: VirtAddr::new(virt_addr),
    })
}

/// Pre-allocated pool of kernel pages for use in non-sleepable contexts.
///
/// Pages are allocated with `GFP_KERNEL` in sleepable context (ioctl handlers)
/// and dispensed from during the VM run loop (preemption disabled).
/// If the pool is exhausted mid-run, the run loop exits back to sleepable
/// context for refilling.
pub(crate) struct PagePool {
    pages: KVec<KernelPage, KVmalloc>,
    target: usize,
}

impl PagePool {
    pub(crate) fn new(target: usize) -> Self {
        Self {
            pages: KVec::new(),
            target,
        }
    }

    /// Refill pool to target count. Must be called in sleepable context.
    /// Only actually allocates when pool drops below 5% of target.
    pub(crate) fn refill(&mut self) -> bool {
        let threshold = self.target / 20; // 5%
        if self.pages.len() >= threshold {
            return true;
        }
        while self.pages.len() < self.target {
            match alloc_zeroed_page() {
                Some(page) => {
                    if self.pages.push(page, GFP_KERNEL).is_err() {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }

    /// Take a page from the pool. O(1), no allocation.
    pub(crate) fn take(&mut self) -> Option<KernelPage> {
        self.pages.pop()
    }
}
