// SPDX-License-Identifier: GPL-2.0

//! VPID (Virtual Processor Identifier) allocation with recycling.
//!
//! VPIDs are 16-bit identifiers used to tag TLB entries, allowing the CPU to
//! cache translations for multiple VMs without flushing on VM entry/exit.
//!
//! VPID 0 is reserved for VMX root operation, so we allocate starting from 1.
//! Intel SDM Vol 3C, Section 30.1.
//!
//! VPIDs are recycled when VMs are dropped using a bitmap to track in-use VPIDs.

use core::sync::atomic::{AtomicU64, AtomicU16, Ordering};

/// Bitmap tracking which VPIDs are in use.
/// 65536 VPIDs / 64 bits per word = 1024 words = 8KB
/// Bit N is set if VPID N is in use.
const BITMAP_WORDS: usize = 1024;

/// Bitmap of in-use VPIDs. Bit i in word w represents VPID (w * 64 + i).
static VPID_BITMAP: VpidBitmap = VpidBitmap::new();

/// Hint for where to start searching for free VPIDs.
/// This speeds up allocation when VPIDs are frequently allocated/deallocated.
static SEARCH_HINT: AtomicU16 = AtomicU16::new(1);

/// Bitmap for tracking VPID allocation.
struct VpidBitmap {
    words: [AtomicU64; BITMAP_WORDS],
}

impl VpidBitmap {
    const fn new() -> Self {
        // Use a const block to initialize the array
        // VPID 0 is reserved, so we set bit 0 in word 0 to mark it as "in use"
        #[allow(clippy::declare_interior_mutable_const)]
        const INIT_WORD: AtomicU64 = AtomicU64::new(0);
        Self {
            words: [INIT_WORD; BITMAP_WORDS],
        }
    }

    /// Try to allocate the specified VPID. Returns true if successful.
    fn try_allocate(&self, vpid: u16) -> bool {
        let word_idx = (vpid / 64) as usize;
        let bit_idx = vpid % 64;
        let mask = 1u64 << bit_idx;

        // Atomically set the bit if it's not already set
        let old = self.words[word_idx].fetch_or(mask, Ordering::AcqRel);
        (old & mask) == 0 // Return true if bit was previously clear
    }

    /// Deallocate the specified VPID.
    fn deallocate(&self, vpid: u16) {
        if vpid == 0 {
            return; // Never deallocate VPID 0
        }
        let word_idx = (vpid / 64) as usize;
        let bit_idx = vpid % 64;
        let mask = 1u64 << bit_idx;

        self.words[word_idx].fetch_and(!mask, Ordering::Release);
    }

    /// Find and allocate a free VPID starting from the hint.
    /// Returns None if all VPIDs are exhausted.
    fn allocate_any(&self, hint: u16) -> Option<u16> {
        // Start searching from hint, wrap around if needed
        let start_word = (hint / 64) as usize;

        // Search from hint to end
        for word_idx in start_word..BITMAP_WORDS {
            if let Some(vpid) = self.try_allocate_in_word(word_idx) {
                return Some(vpid);
            }
        }

        // Wrap around: search from beginning to hint
        for word_idx in 0..start_word {
            if let Some(vpid) = self.try_allocate_in_word(word_idx) {
                return Some(vpid);
            }
        }

        None // All VPIDs exhausted
    }

    /// Try to allocate a free VPID from the specified word.
    fn try_allocate_in_word(&self, word_idx: usize) -> Option<u16> {
        loop {
            let word = self.words[word_idx].load(Ordering::Acquire);
            if word == u64::MAX {
                return None; // All bits set, no free VPIDs in this word
            }

            // Find first clear bit
            let bit_idx = (!word).trailing_zeros() as u16;
            let vpid = (word_idx as u16) * 64 + bit_idx;

            // Skip VPID 0 (reserved)
            if vpid == 0 {
                // Try to allocate VPID 0 to mark it as used, then continue
                let mask = 1u64;
                self.words[0].fetch_or(mask, Ordering::AcqRel);
                continue;
            }

            if self.try_allocate(vpid) {
                return Some(vpid);
            }
            // CAS failed, another thread got this VPID, retry
        }
    }

    /// Reset the bitmap (for testing/module reload).
    fn reset(&self) {
        for word in &self.words {
            word.store(0, Ordering::Release);
        }
        // Mark VPID 0 as in use (reserved)
        self.words[0].store(1, Ordering::Release);
    }
}

/// Allocate a unique VPID for a new VM.
///
/// Uses a bitmap to track in-use VPIDs and recycles them when deallocated.
/// VPID 0 is reserved for VMX root operation and is never returned.
///
/// # Panics
///
/// Panics if all 65535 VPIDs are in use.
///
/// # Thread Safety
///
/// This function is thread-safe and can be called concurrently.
pub fn allocate_vpid() -> u16 {
    let hint = SEARCH_HINT.load(Ordering::Relaxed);

    match VPID_BITMAP.allocate_any(hint) {
        Some(vpid) => {
            // Update hint to search after this VPID next time
            SEARCH_HINT.store(vpid.wrapping_add(1), Ordering::Relaxed);
            vpid
        }
        None => panic!("VPID allocation failed: all 65535 VPIDs are in use"),
    }
}

/// Return a VPID to the pool for reuse.
///
/// Call this when a VM is dropped to allow its VPID to be reused.
///
/// # Arguments
///
/// * `vpid` - The VPID to return. Must not be 0.
pub fn deallocate_vpid(vpid: u16) {
    VPID_BITMAP.deallocate(vpid);

    // Update hint to potentially find this VPID faster next time
    let current_hint = SEARCH_HINT.load(Ordering::Relaxed);
    if vpid < current_hint {
        SEARCH_HINT.store(vpid, Ordering::Relaxed);
    }
}

/// Reset the VPID allocator to initial state.
///
/// This should only be called during module unload/reload when all VMs
/// have been destroyed.
///
/// # Safety
///
/// Caller must ensure no VMs are using allocated VPIDs.
pub fn reset_vpid_counter() {
    VPID_BITMAP.reset();
    SEARCH_HINT.store(1, Ordering::Relaxed);
}

/// Get the next VPID that would likely be allocated (for debugging/testing).
/// This is just a hint and may not be accurate under concurrent allocation.
pub fn peek_next_vpid() -> u16 {
    SEARCH_HINT.load(Ordering::Relaxed)
}

/// Count the number of allocated VPIDs (for debugging/testing).
/// This is O(n) and should only be used for debugging.
pub fn count_allocated_vpids() -> usize {
    let mut count = 0;
    for word in &VPID_BITMAP.words {
        count += word.load(Ordering::Relaxed).count_ones() as usize;
    }
    count
}

#[cfg(test)]
#[path = "vpid_tests.rs"]
mod tests;
