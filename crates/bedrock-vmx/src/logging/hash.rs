// SPDX-License-Identifier: GPL-2.0

//! Hash utilities for deterministic state hashing.
//!
//! Provides XXH64 hasher for device state and guest memory hashing.
//! Uses the Linux kernel's xxhash implementation via C bindings.
//! In cargo/test builds, hashing is disabled (returns 0).

/// Trait for computing deterministic 64-bit state hashes.
///
/// Implemented by device state structs to enable logging of their state
/// as compact hash values for non-determinism diagnosis.
pub trait StateHash {
    /// Compute a 64-bit hash of the current state.
    fn state_hash(&self) -> u64;
}

/// XXH64 streaming hasher.
///
/// In kernel builds, uses the Linux kernel's xxhash implementation.
/// In cargo/test builds, hashing is disabled and always returns 0.
pub struct Xxh64Hasher {
    #[cfg(not(feature = "cargo"))]
    state: crate::c_helpers::Xxh64State,
    #[cfg(feature = "cargo")]
    _dummy: (),
}

impl Xxh64Hasher {
    /// Create a new hasher with seed 0.
    #[cfg(not(feature = "cargo"))]
    pub fn new() -> Self {
        let mut state = crate::c_helpers::Xxh64State {
            total_len: 0,
            v1: 0,
            v2: 0,
            v3: 0,
            v4: 0,
            mem64: [0; 4],
            memsize: 0,
        };
        unsafe {
            crate::c_helpers::bedrock_xxh64_reset(&mut state, 0);
        }
        Self { state }
    }

    #[cfg(feature = "cargo")]
    pub fn new() -> Self {
        Self { _dummy: () }
    }

    /// Hash a single byte.
    #[inline]
    pub fn write_u8(&mut self, byte: u8) {
        self.write_bytes(&[byte]);
    }

    /// Hash a u16 value (little-endian).
    #[inline]
    pub fn write_u16(&mut self, val: u16) {
        self.write_bytes(&val.to_le_bytes());
    }

    /// Hash a u32 value (little-endian).
    #[inline]
    pub fn write_u32(&mut self, val: u32) {
        self.write_bytes(&val.to_le_bytes());
    }

    /// Hash a u64 value (little-endian).
    #[inline]
    pub fn write_u64(&mut self, val: u64) {
        self.write_bytes(&val.to_le_bytes());
    }

    /// Hash a byte slice.
    #[inline]
    #[cfg(not(feature = "cargo"))]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        unsafe {
            crate::c_helpers::bedrock_xxh64_update(
                &mut self.state,
                bytes.as_ptr() as *const core::ffi::c_void,
                bytes.len(),
            );
        }
    }

    #[cfg(feature = "cargo")]
    #[inline]
    pub fn write_bytes(&mut self, _bytes: &[u8]) {
        // No-op in cargo builds
    }

    /// Finalize and return the hash value.
    #[inline]
    #[cfg(not(feature = "cargo"))]
    pub fn finish(&self) -> u64 {
        unsafe { crate::c_helpers::bedrock_xxh64_digest(&self.state) }
    }

    #[cfg(feature = "cargo")]
    #[inline]
    pub fn finish(&self) -> u64 {
        0 // Disabled in cargo builds
    }
}

impl Default for Xxh64Hasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash guest memory using XXH64.
#[cfg(not(feature = "cargo"))]
pub fn hash_guest_memory(memory: &[u8]) -> u64 {
    unsafe {
        crate::c_helpers::bedrock_xxh64(
            memory.as_ptr() as *const core::ffi::c_void,
            memory.len(),
            0,
        )
    }
}

#[cfg(feature = "cargo")]
pub fn hash_guest_memory(_memory: &[u8]) -> u64 {
    0 // Disabled in cargo builds
}

#[cfg(test)]
#[path = "hash_tests.rs"]
mod tests;
