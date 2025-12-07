// SPDX-License-Identifier: GPL-2.0

extern crate alloc;
use alloc::vec::Vec;
use super::*;

#[test]
fn test_vpid_allocation() {
    // Reset to known state
    reset_vpid_counter();

    // First allocation should be 1 (VPID 0 is reserved)
    let vpid1 = allocate_vpid();
    assert_ne!(vpid1, 0, "VPID should never be 0");

    let vpid2 = allocate_vpid();
    assert_ne!(vpid2, 0);
    assert_ne!(vpid2, vpid1, "Each allocation should return a unique VPID");

    let vpid3 = allocate_vpid();
    assert_ne!(vpid3, 0);
    assert_ne!(vpid3, vpid1);
    assert_ne!(vpid3, vpid2);

    // Clean up
    deallocate_vpid(vpid1);
    deallocate_vpid(vpid2);
    deallocate_vpid(vpid3);
}

#[test]
fn test_vpid_never_zero() {
    reset_vpid_counter();

    // Allocate many VPIDs and verify none are 0
    let mut vpids = Vec::new();
    for _ in 0..1000 {
        let vpid = allocate_vpid();
        assert_ne!(vpid, 0, "VPID should never be 0");
        vpids.push(vpid);
    }

    // Clean up
    for vpid in vpids {
        deallocate_vpid(vpid);
    }
}

#[test]
fn test_vpid_recycling() {
    reset_vpid_counter();

    // Allocate a VPID
    let vpid1 = allocate_vpid();
    assert_ne!(vpid1, 0);

    // Deallocate it
    deallocate_vpid(vpid1);

    // Allocate again - should get the same VPID back (or another free one)
    let vpid2 = allocate_vpid();
    assert_ne!(vpid2, 0);

    // The recycled VPID should be reusable
    // (we can't guarantee it's the same one due to hint optimization)

    deallocate_vpid(vpid2);
}

#[test]
fn test_vpid_high_churn() {
    reset_vpid_counter();

    // Simulate high churn: allocate and deallocate many times
    // This should never exhaust VPIDs because we recycle
    for _ in 0..10000 {
        let vpid = allocate_vpid();
        assert_ne!(vpid, 0);
        deallocate_vpid(vpid);
    }

    // We should still be able to allocate after high churn
    let vpid = allocate_vpid();
    assert_ne!(vpid, 0);
    deallocate_vpid(vpid);
}

#[test]
fn test_vpid_count() {
    // Note: tests run in parallel and share global VPID state.
    // This test just verifies count_allocated_vpids doesn't panic
    // and returns reasonable values. Exact counts are unreliable
    // with concurrent tests.

    let vpid = allocate_vpid();
    let count = count_allocated_vpids();

    // Count should be at least 1 (our allocation, plus VPID 0 is reserved)
    assert!(count >= 1, "Should have at least 1 VPID marked in use");

    deallocate_vpid(vpid);
}
