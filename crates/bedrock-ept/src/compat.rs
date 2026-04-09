// SPDX-License-Identifier: GPL-2.0

//! Platform compatibility layer for EPT allocation.
//!
//! EPT page tables use vmalloc-backed vectors (`KVec<T, KVmalloc>` in kernel)
//! because the frame list can grow large enough that physically contiguous
//! allocation may fail. All cfg gates for vector operations are isolated here.

#[cfg(feature = "cargo")]
mod cargo_impl {
    extern crate alloc;

    /// Growable vector for EPT frames (vmalloc-backed in kernel).
    pub type EptVec<T> = alloc::vec::Vec<T>;

    /// Create a new EptVec containing a single initial element.
    pub fn ept_vec_init<T>(val: T) -> EptVec<T> {
        alloc::vec![val]
    }

    /// Create a new EptVec with reserved capacity.
    pub fn ept_vec_with_capacity<T>(cap: usize) -> EptVec<T> {
        alloc::vec::Vec::with_capacity(cap)
    }

    /// Push a value onto an EptVec.
    pub fn ept_vec_push<T>(v: &mut EptVec<T>, val: T) {
        v.push(val);
    }
}

#[cfg(not(feature = "cargo"))]
mod kernel_impl {
    use kernel::alloc::{allocator::KVmalloc, flags::GFP_KERNEL, Vec};

    /// Growable vector for EPT frames (vmalloc-backed in kernel).
    pub type EptVec<T> = Vec<T, KVmalloc>;

    /// Create a new EptVec containing a single initial element.
    pub fn ept_vec_init<T>(val: T) -> EptVec<T> {
        let mut v = Vec::new();
        let _ = v.push(val, GFP_KERNEL);
        v
    }

    /// Create a new EptVec with reserved capacity.
    pub fn ept_vec_with_capacity<T>(cap: usize) -> EptVec<T> {
        let mut v = Vec::new();
        let _ = v.reserve(cap, GFP_KERNEL);
        v
    }

    /// Push a value onto an EptVec.
    pub fn ept_vec_push<T>(v: &mut EptVec<T>, val: T) {
        let _ = v.push(val, GFP_KERNEL);
    }
}

#[cfg(feature = "cargo")]
pub use cargo_impl::*;
#[cfg(not(feature = "cargo"))]
pub use kernel_impl::*;
