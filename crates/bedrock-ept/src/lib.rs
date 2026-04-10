// SPDX-License-Identifier: GPL-2.0

//! Extended Page Tables (EPT) implementation for x86-64 virtualization.
//!
//! This crate provides a platform-agnostic EPT implementation using traits
//! for memory allocation and physical address translation.

#![no_std]

extern crate alloc;

mod compat;
mod entry;
mod table;
mod traits;

#[cfg(test)]
mod tests;

pub use entry::{EptEntry, EptMemoryType, EptPermissions};
pub use table::{EptPageTable, EptRemapError};
pub use traits::{FrameAllocator, PhysAddr, VirtAddr};
