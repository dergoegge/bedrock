// SPDX-License-Identifier: GPL-2.0

//! Copy-on-write page allocator trait.
//!
//! This trait extends the basic FrameAllocator with the ability to allocate
//! Page objects, which is needed for COW handling in forked VMs.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Allocator trait for copy-on-write pages.
///
/// This trait extends `FrameAllocator` with the ability to allocate
/// `Page` objects that own their memory and handle deallocation on drop.
///
/// # Type Parameters
///
/// * `P` - The page type to allocate
pub trait CowAllocator<P: Page>: FrameAllocator {
    /// Allocate a zeroed page for copy-on-write.
    ///
    /// Returns a new page that owns its memory. The page will be freed
    /// when dropped.
    fn allocate_cow_page(&mut self) -> Result<P, Self::Error>;
}
