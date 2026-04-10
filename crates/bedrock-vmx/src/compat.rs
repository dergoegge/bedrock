// SPDX-License-Identifier: GPL-2.0

//! Platform compatibility layer for allocation.
//!
//! Provides unified type aliases and helpers that abstract over the
//! different allocation APIs between cargo (userspace) and kernel builds.
//! All cfg gates for allocation are isolated here.

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
    #[allow(clippy::result_unit_err)]
    pub fn heap_vec_with_capacity<T>(cap: usize) -> Result<HeapVec<T>, ()> {
        Ok(alloc::vec::Vec::with_capacity(cap))
    }

    /// Push a value onto a vector.
    pub fn heap_vec_push<T>(v: &mut HeapVec<T>, val: T) {
        v.push(val);
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
    #[allow(clippy::result_unit_err)]
    pub fn heap_vec_with_capacity<T>(cap: usize) -> Result<HeapVec<T>, ()> {
        kernel::alloc::KVec::with_capacity(cap, kernel::alloc::flags::GFP_KERNEL).map_err(|_| ())
    }

    /// Push a value onto a vector.
    pub fn heap_vec_push<T>(v: &mut HeapVec<T>, val: T) {
        let _ = v.push(val, kernel::alloc::flags::GFP_KERNEL);
    }
}

#[cfg(feature = "cargo")]
pub use cargo_impl::*;
#[cfg(not(feature = "cargo"))]
pub use kernel_impl::*;
