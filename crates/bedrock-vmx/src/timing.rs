// SPDX-License-Identifier: GPL-2.0

//! CPU timing utilities for performance measurement.
//!
//! This module provides low-level access to CPU timing features like RDTSC
//! (Read Time Stamp Counter). In kernel builds, this uses inline assembly
//! to read the TSC. In cargo builds (tests), it returns 0 to avoid depending
//! on x86-specific features.

/// Read the CPU timestamp counter (TSC).
///
/// Returns the current value of the processor's time stamp counter,
/// which increments at a constant rate (typically the base CPU frequency).
///
/// # Note
///
/// In cargo builds (tests), this always returns 0 to avoid platform-specific
/// assembly. Performance statistics will show 0 cycles in tests.
#[cfg(not(feature = "cargo"))]
#[inline]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC is safe to execute and only reads the timestamp counter.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Read the CPU timestamp counter (TSC) - stub version for tests.
///
/// In cargo builds, this returns 0 since we can't use inline assembly
/// and don't want to depend on platform-specific features in tests.
#[cfg(feature = "cargo")]
#[inline]
pub fn rdtsc() -> u64 {
    0
}
