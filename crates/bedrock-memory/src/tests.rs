// SPDX-License-Identifier: GPL-2.0

//! Tests for the bedrock-memory crate.

use crate::{PhysAddr, VirtAddr};

// =============================================================================
// PhysAddr tests
// =============================================================================

#[test]
fn phys_addr_new_and_as_u64() {
    let addr = PhysAddr::new(0x12345678);
    assert_eq!(addr.as_u64(), 0x12345678);
}

#[test]
fn phys_addr_ordering() {
    let a = PhysAddr::new(100);
    let b = PhysAddr::new(200);
    let c = PhysAddr::new(100);

    assert!(a < b);
    assert!(b > a);
    assert_eq!(a, c);
}

// =============================================================================
// VirtAddr tests
// =============================================================================

#[test]
fn virt_addr_new_and_as_u64() {
    let addr = VirtAddr::new(0xDEADBEEF);
    assert_eq!(addr.as_u64(), 0xDEADBEEF);
}

#[test]
fn virt_addr_pml4_index() {
    // PML4 index is bits 47:39
    // 0x0000_7F80_0000_0000 has bit 39-46 set
    let addr = VirtAddr::new(0x0000_7F80_0000_0000);
    assert_eq!(addr.pml4_index(), 0xFF); // 0x1FF >> 1 = 0xFF at bits 39-46

    let addr2 = VirtAddr::new(0x0000_0080_0000_0000);
    assert_eq!(addr2.pml4_index(), 1);

    let addr3 = VirtAddr::new(0);
    assert_eq!(addr3.pml4_index(), 0);
}

#[test]
fn virt_addr_pdpt_index() {
    // PDPT index is bits 38:30
    // Each increment of (1 << 30) increases pdpt_index by 1
    let addr = VirtAddr::new(1 << 30);
    assert_eq!(addr.pdpt_index(), 1);

    let addr2 = VirtAddr::new(0x1FF << 30);
    assert_eq!(addr2.pdpt_index(), 0x1FF);
}

#[test]
fn virt_addr_pd_index() {
    // PD index is bits 29:21
    let addr = VirtAddr::new(1 << 21);
    assert_eq!(addr.pd_index(), 1);

    let addr2 = VirtAddr::new(0x1FF << 21);
    assert_eq!(addr2.pd_index(), 0x1FF);
}

#[test]
fn virt_addr_pt_index() {
    // PT index is bits 20:12
    let addr = VirtAddr::new(1 << 12);
    assert_eq!(addr.pt_index(), 1);

    let addr2 = VirtAddr::new(0x1FF << 12);
    assert_eq!(addr2.pt_index(), 0x1FF);
}

#[test]
fn virt_addr_full_decomposition() {
    // Create an address with known indices
    let pml4_idx = 0x10;
    let pdpt_idx = 0x20;
    let pd_idx = 0x30;
    let pt_idx = 0x40;

    let addr = (pml4_idx << 39) | (pdpt_idx << 30) | (pd_idx << 21) | (pt_idx << 12);
    let virt = VirtAddr::new(addr);

    assert_eq!(virt.pml4_index(), pml4_idx as usize);
    assert_eq!(virt.pdpt_index(), pdpt_idx as usize);
    assert_eq!(virt.pd_index(), pd_idx as usize);
    assert_eq!(virt.pt_index(), pt_idx as usize);
}
