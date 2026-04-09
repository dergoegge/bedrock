// SPDX-License-Identifier: GPL-2.0

//! Copy-on-write page tracking for forked VMs.
//!
//! This module provides the `CowPageMap` structure for tracking pages that have
//! been copied during copy-on-write handling in forked VMs.
//!
//! In cargo builds, uses `alloc::collections::BTreeMap`.
//! In kernel builds, uses `kernel::rbtree::RBTree`.

// ============================================================================
// Cargo build: Use alloc::collections::BTreeMap
// ============================================================================

#[cfg(feature = "cargo")]
mod cargo_impl {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use memory::GuestPhysAddr;

    use crate::traits::Page;

    /// Tracks copy-on-write pages for a forked VM.
    ///
    /// Only stores pages that THIS VM has modified - ancestor pages are
    /// accessed via EPT lookup (the EPT already points to the correct
    /// host physical addresses from parent/grandparent/etc).
    pub struct CowPageMap<P: Page> {
        /// Maps page-aligned GPAs to owned pages.
        pages: BTreeMap<u64, P>,
        /// Number of pages in the map.
        count: usize,
    }

    impl<P: Page> CowPageMap<P> {
        /// Create a new empty COW page map.
        pub fn new() -> Self {
            Self {
                pages: BTreeMap::new(),
                count: 0,
            }
        }

        /// Get a reference to the COW page at the given GPA, if it exists.
        ///
        /// Returns None if the page has not been copied for this VM.
        pub fn get(&self, gpa: GuestPhysAddr) -> Option<&P> {
            let page_aligned = gpa.as_u64() & !0xFFF;
            self.pages.get(&page_aligned)
        }

        /// Get a mutable reference to the COW page at the given GPA, if it exists.
        pub fn get_mut(&mut self, gpa: GuestPhysAddr) -> Option<&mut P> {
            let page_aligned = gpa.as_u64() & !0xFFF;
            self.pages.get_mut(&page_aligned)
        }

        /// Insert a new COW page for the given GPA.
        ///
        /// The GPA will be page-aligned before insertion.
        /// Returns Ok(()) on success.
        #[allow(clippy::result_unit_err)]
        pub fn insert(&mut self, gpa: GuestPhysAddr, page: P) -> Result<(), ()> {
            let page_aligned = gpa.as_u64() & !0xFFF;
            if self.pages.insert(page_aligned, page).is_none() {
                self.count += 1;
            }
            Ok(())
        }

        /// Check if a COW page exists for the given GPA.
        pub fn contains(&self, gpa: GuestPhysAddr) -> bool {
            let page_aligned = gpa.as_u64() & !0xFFF;
            self.pages.contains_key(&page_aligned)
        }

        /// Get the number of COW pages.
        pub fn len(&self) -> usize {
            self.count
        }

        /// Check if the map is empty.
        pub fn is_empty(&self) -> bool {
            self.count == 0
        }

        /// Iterate over all COW pages.
        ///
        /// Yields (GPA, Page) pairs where GPA is page-aligned.
        pub fn iter(&self) -> impl Iterator<Item = (GuestPhysAddr, &P)> {
            self.pages
                .iter()
                .map(|(&gpa, page)| (GuestPhysAddr::new(gpa), page))
        }
    }

    impl<P: Page> Default for CowPageMap<P> {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(feature = "cargo")]
pub use cargo_impl::CowPageMap;

// ============================================================================
// Kernel build: Use kernel::rbtree::RBTree
// ============================================================================

#[cfg(not(feature = "cargo"))]
mod kernel_impl {
    use kernel::alloc::flags::GFP_ATOMIC;
    use kernel::rbtree::RBTree;

    use crate::memory::GuestPhysAddr;
    use crate::vmx::traits::Page;

    /// Tracks copy-on-write pages for a forked VM.
    ///
    /// Only stores pages that THIS VM has modified - ancestor pages are
    /// accessed via EPT lookup (the EPT already points to the correct
    /// host physical addresses from parent/grandparent/etc).
    pub struct CowPageMap<P: Page> {
        /// Maps page-aligned GPAs to owned pages.
        pages: RBTree<u64, P>,
        /// Number of pages in the map.
        count: usize,
    }

    impl<P: Page> CowPageMap<P> {
        /// Create a new empty COW page map.
        pub fn new() -> Self {
            Self {
                pages: RBTree::new(),
                count: 0,
            }
        }

        /// Get a reference to the COW page at the given GPA, if it exists.
        ///
        /// Returns None if the page has not been copied for this VM.
        pub fn get(&self, gpa: GuestPhysAddr) -> Option<&P> {
            let page_aligned = gpa.as_u64() & !0xFFF;
            self.pages.get(&page_aligned)
        }

        /// Get a mutable reference to the COW page at the given GPA, if it exists.
        pub fn get_mut(&mut self, gpa: GuestPhysAddr) -> Option<&mut P> {
            let page_aligned = gpa.as_u64() & !0xFFF;
            self.pages.get_mut(&page_aligned)
        }

        /// Insert a new COW page for the given GPA.
        ///
        /// The GPA will be page-aligned before insertion.
        /// Returns Ok(()) on success, Err(()) if allocation fails.
        #[allow(clippy::result_unit_err)]
        pub fn insert(&mut self, gpa: GuestPhysAddr, page: P) -> Result<(), ()> {
            let page_aligned = gpa.as_u64() & !0xFFF;
            // try_create_and_insert allocates a node and inserts it
            match self
                .pages
                .try_create_and_insert(page_aligned, page, GFP_ATOMIC)
            {
                Ok(_) => {
                    self.count += 1;
                    Ok(())
                }
                Err(_) => Err(()),
            }
        }

        /// Check if a COW page exists for the given GPA.
        pub fn contains(&self, gpa: GuestPhysAddr) -> bool {
            let page_aligned = gpa.as_u64() & !0xFFF;
            self.pages.get(&page_aligned).is_some()
        }

        /// Get the number of COW pages.
        pub fn len(&self) -> usize {
            self.count
        }

        /// Check if the map is empty.
        pub fn is_empty(&self) -> bool {
            self.count == 0
        }

        /// Iterate over all COW pages.
        ///
        /// Yields (GPA, Page) pairs where GPA is page-aligned.
        pub fn iter(&self) -> impl Iterator<Item = (GuestPhysAddr, &P)> {
            self.pages
                .iter()
                .map(|(gpa, page)| (GuestPhysAddr::new(*gpa), page))
        }
    }

    impl<P: Page> Default for CowPageMap<P> {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(not(feature = "cargo"))]
pub use kernel_impl::CowPageMap;
