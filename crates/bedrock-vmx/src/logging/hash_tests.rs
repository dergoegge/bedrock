// SPDX-License-Identifier: GPL-2.0

use super::*;

#[test]
fn test_xxh64_cargo_returns_zero() {
    let h = Xxh64Hasher::new();
    assert_eq!(h.finish(), 0);
}

#[test]
fn test_hash_guest_memory_cargo_returns_zero() {
    let memory = [0u8; 4096];
    assert_eq!(hash_guest_memory(&memory), 0);
}
