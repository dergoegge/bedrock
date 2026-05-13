// SPDX-License-Identifier: GPL-2.0

//! Platform compatibility layer for allocation.
//!
//! Provides unified type aliases and helpers that abstract over the
//! different allocation APIs between cargo (userspace) and kernel builds.
//! All cfg gates for allocation are isolated here.

/// Error returned when heap allocation fails.
#[derive(Debug, Clone, Copy)]
pub struct AllocError;

#[cfg(feature = "cargo")]
mod cargo_impl {
    extern crate alloc;

    /// Heap-allocated box (standard allocator).
    pub type HeapBox<T> = alloc::boxed::Box<T>;

    /// Heap-allocated box using vmalloc (for large allocations).
    /// In cargo builds, this is the same as HeapBox.
    pub type VmallocBox<T> = alloc::boxed::Box<T>;

    /// Growable vector (standard allocator).
    pub type HeapVec<T> = alloc::vec::Vec<T>;

    /// Box a value on the heap.
    pub fn heap_box<T>(val: T) -> HeapBox<T> {
        alloc::boxed::Box::new(val)
    }

    /// Create a vector with pre-allocated capacity.
    pub fn heap_vec_with_capacity<T>(cap: usize) -> Result<HeapVec<T>, super::AllocError> {
        Ok(alloc::vec::Vec::with_capacity(cap))
    }

    /// Push a value onto a vector. Returns `Err(AllocError)` if growth
    /// fails — in cargo builds the standard allocator aborts on OOM, so
    /// the `Result` is just for API parity with kernel builds.
    pub fn heap_vec_push<T>(v: &mut HeapVec<T>, val: T) -> Result<(), super::AllocError> {
        v.push(val);
        Ok(())
    }

    /// Remove and return the front element of a vector, or `None` if
    /// empty. O(n) (shifts the rest down); used for FIFO queues whose
    /// depth is small enough that the shift cost is negligible relative
    /// to the per-element work.
    pub fn heap_vec_remove_front<T>(v: &mut HeapVec<T>) -> Option<T> {
        if v.is_empty() {
            None
        } else {
            Some(v.remove(0))
        }
    }
}

#[cfg(not(feature = "cargo"))]
mod kernel_impl {
    /// Heap-allocated box (kmalloc, GFP_KERNEL).
    pub type HeapBox<T> = kernel::alloc::KBox<T>;

    /// Heap-allocated box using kvmalloc (for large allocations).
    /// kvmalloc falls back to vmalloc when kmalloc fails for large contiguous
    /// allocations.
    pub type VmallocBox<T> = kernel::alloc::KVBox<T>;

    /// Growable vector (kmalloc, GFP_KERNEL).
    pub type HeapVec<T> = kernel::alloc::KVec<T>;

    /// Box a value on the heap.
    pub fn heap_box<T>(val: T) -> HeapBox<T> {
        kernel::alloc::KBox::new(val, kernel::alloc::flags::GFP_KERNEL)
            .expect("Failed to allocate HeapBox")
    }

    /// Create a vector with pre-allocated capacity.
    pub fn heap_vec_with_capacity<T>(cap: usize) -> Result<HeapVec<T>, super::AllocError> {
        kernel::alloc::KVec::with_capacity(cap, kernel::alloc::flags::GFP_KERNEL)
            .map_err(|_| super::AllocError)
    }

    /// Push a value onto a vector. Returns `Err(AllocError)` on
    /// allocation failure; callers must propagate ENOMEM rather than
    /// silently dropping the value.
    pub fn heap_vec_push<T>(v: &mut HeapVec<T>, val: T) -> Result<(), super::AllocError> {
        v.push(val, kernel::alloc::flags::GFP_KERNEL)
            .map_err(|_| super::AllocError)
    }

    /// Remove and return the front element of a vector, or `None` if
    /// empty. Kernel `KVec::remove` returns `Result`; we collapse the
    /// `Err` (OOB) and empty cases together into `None`.
    pub fn heap_vec_remove_front<T>(v: &mut HeapVec<T>) -> Option<T> {
        v.remove(0).ok()
    }
}

#[cfg(feature = "cargo")]
pub use cargo_impl::*;
#[cfg(not(feature = "cargo"))]
pub use kernel_impl::*;
