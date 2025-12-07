// SPDX-License-Identifier: GPL-2.0

//! Tests for the bedrock-ept crate.

extern crate std;

use crate::entry::{EptEntry, EptMemoryType, EptPermissions};
use crate::table::{EptPageTable, EptRemapError};
use crate::traits::{FrameAllocator, HostPhysAddr, PhysAddr};
use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::collections::HashMap;

/// A frame that tracks its physical address.
struct TestFrame {
    phys: HostPhysAddr,
}

/// A simple test allocator that uses std heap allocations.
struct TestAllocator {
    /// Maps physical addresses to virtual addresses (heap pointers).
    frames: HashMap<u64, *mut u8>,
    /// Next "physical" address to allocate.
    next_phys: u64,
}

impl TestAllocator {
    fn new() -> Self {
        Self {
            frames: HashMap::new(),
            // Start at 0x1000 to avoid zero addresses
            next_phys: 0x1000,
        }
    }
}

impl FrameAllocator for TestAllocator {
    type Error = ();
    type Frame = TestFrame;

    fn allocate_frame(&mut self) -> Result<TestFrame, Self::Error> {
        let layout = Layout::from_size_align(4096, 4096).unwrap();
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(());
        }

        let phys = self.next_phys;
        self.next_phys += 4096;
        self.frames.insert(phys, ptr);
        Ok(TestFrame {
            phys: HostPhysAddr::new(phys),
        })
    }

    fn frame_phys_addr(frame: &TestFrame) -> HostPhysAddr {
        frame.phys
    }

    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8 {
        // Mask off page offset bits to get page-aligned address
        *self.frames.get(&(phys.as_u64() & !0xFFF)).unwrap()
    }
}

impl Drop for TestAllocator {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(4096, 4096).unwrap();
        for (_phys, ptr) in self.frames.drain() {
            unsafe { dealloc(ptr, layout) };
        }
    }
}

// =============================================================================
// EptEntry tests
// =============================================================================

#[test]
fn ept_entry_table_entry() {
    let addr = HostPhysAddr::new(0x1000);
    let entry = EptEntry::table_entry(addr, EptPermissions::READ_WRITE_EXECUTE);

    assert!(entry.is_present());
    assert_eq!(entry.addr().as_u64(), 0x1000);
}

#[test]
fn ept_entry_page_4k() {
    let addr = HostPhysAddr::new(0x5000);
    let entry = EptEntry::page_entry_4k(
        addr,
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    );

    assert!(entry.is_present());
    assert_eq!(entry.addr().as_u64(), 0x5000);
}

#[test]
fn ept_entry_addr_mask() {
    // Address mask is bits 51:12
    let addr = HostPhysAddr::new(0x000F_FFFF_FFFF_F000);
    let entry = EptEntry::table_entry(addr, EptPermissions::READ_WRITE_EXECUTE);

    assert_eq!(entry.addr().as_u64(), 0x000F_FFFF_FFFF_F000);

    // Bits outside the mask should not affect the address
    let addr_with_low_bits = HostPhysAddr::new(0x000F_FFFF_FFFF_FFFF);
    let entry2 = EptEntry::table_entry(addr_with_low_bits, EptPermissions::READ_WRITE_EXECUTE);
    assert_eq!(entry2.addr().as_u64(), 0x000F_FFFF_FFFF_F000);
}

// =============================================================================
// EptPageTable tests
// =============================================================================

#[test]
fn ept_page_table_new() {
    let mut allocator = TestAllocator::new();
    let _ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();
    // Verify allocation succeeded
}

#[test]
fn ept_page_table_eptp() {
    let mut allocator = TestAllocator::new();
    let ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    let eptp = ept.eptp();
    // EPTP should have:
    // - PML4 address in bits 51:12
    // - Memory type WB (6) in bits 2:0
    // - Page walk length 3 in bits 5:3
    let expected = 0x1000 | (3 << 3) | 6;
    assert_eq!(eptp, expected);
}

#[test]
fn ept_page_table_map_4k() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    let guest_phys = PhysAddr::new(0x2000);
    let host_phys = PhysAddr::new(0xA000);

    ept.map_4k(
        &mut allocator,
        guest_phys,
        host_phys,
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();
}

#[test]
fn ept_page_table_multiple_mappings() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    // Map multiple 4KB pages
    for i in 0..10 {
        let guest = PhysAddr::new(i * 0x1000);
        let host = PhysAddr::new(0x10_0000 + i * 0x1000);
        ept.map_4k(
            &mut allocator,
            guest,
            host,
            EptPermissions::READ_WRITE_EXECUTE,
            EptMemoryType::WriteBack,
        )
        .unwrap();
    }
}

#[test]
fn ept_page_table_different_pml4_entries() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    // Map addresses that use different PML4 entries
    // PML4 index changes every 512GB
    let addr1 = PhysAddr::new(0x0000_0000_0000); // PML4[0]
    let addr2 = PhysAddr::new(0x0080_0000_0000); // PML4[1]

    ept.map_4k(
        &mut allocator,
        addr1,
        PhysAddr::new(0x1_0000),
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();

    ept.map_4k(
        &mut allocator,
        addr2,
        PhysAddr::new(0x2_0000),
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();
}

#[test]
fn ept_page_table_lookup() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    let guest_phys = PhysAddr::new(0x5000);
    let host_phys = PhysAddr::new(0xA0000);

    // Before mapping, lookup should return None
    assert!(ept.lookup(&allocator, guest_phys).is_none());

    // Map the page
    ept.map_4k(
        &mut allocator,
        guest_phys,
        host_phys,
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();

    // After mapping, lookup should return the host address and permissions
    let (found_host, found_perms) = ept.lookup(&allocator, guest_phys).unwrap();
    assert_eq!(found_host.as_u64(), host_phys.as_u64());
    assert_eq!(
        found_perms.bits(),
        EptPermissions::READ_WRITE_EXECUTE.bits()
    );
}

#[test]
fn ept_page_table_remap_4k() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    let guest_phys = PhysAddr::new(0x3000);
    let host_phys_1 = PhysAddr::new(0xB0000);
    let host_phys_2 = PhysAddr::new(0xC0000);

    // Map initially with RWX
    ept.map_4k(
        &mut allocator,
        guest_phys,
        host_phys_1,
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();

    // Remap to different host address with different permissions
    ept.remap_4k(
        &allocator,
        guest_phys,
        host_phys_2,
        EptPermissions::READ_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();

    // Verify the remap
    let (found_host, found_perms) = ept.lookup(&allocator, guest_phys).unwrap();
    assert_eq!(found_host.as_u64(), host_phys_2.as_u64());
    assert_eq!(found_perms.bits(), EptPermissions::READ_EXECUTE.bits());
}

#[test]
fn ept_page_table_remap_not_mapped() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    let guest_phys = PhysAddr::new(0x4000);
    let host_phys = PhysAddr::new(0xD0000);

    // Remap without mapping first should fail
    let result = ept.remap_4k(
        &allocator,
        guest_phys,
        host_phys,
        EptPermissions::READ_EXECUTE,
        EptMemoryType::WriteBack,
    );
    assert_eq!(result, Err(EptRemapError::NotMapped));
}

#[test]
fn ept_page_table_clone_for_fork() {
    let mut allocator = TestAllocator::new();
    let mut ept: EptPageTable<TestFrame> = EptPageTable::new(&mut allocator).unwrap();

    // Map some pages with RWX
    let guest1 = PhysAddr::new(0x1000);
    let host1 = PhysAddr::new(0x10_0000);
    let guest2 = PhysAddr::new(0x2000);
    let host2 = PhysAddr::new(0x20_0000);

    ept.map_4k(
        &mut allocator,
        guest1,
        host1,
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();

    ept.map_4k(
        &mut allocator,
        guest2,
        host2,
        EptPermissions::READ_WRITE_EXECUTE,
        EptMemoryType::WriteBack,
    )
    .unwrap();

    // Clone for fork
    let forked_ept = ept.clone_for_fork(&mut allocator).unwrap();

    // Verify forked EPT has R+X (no W) for all pages
    let (found_host1, found_perms1) = forked_ept.lookup(&allocator, guest1).unwrap();
    assert_eq!(found_host1.as_u64(), host1.as_u64());
    assert_eq!(found_perms1.bits(), EptPermissions::READ_EXECUTE.bits());

    let (found_host2, found_perms2) = forked_ept.lookup(&allocator, guest2).unwrap();
    assert_eq!(found_host2.as_u64(), host2.as_u64());
    assert_eq!(found_perms2.bits(), EptPermissions::READ_EXECUTE.bits());

    // Original EPT should still have RWX
    let (_, orig_perms1) = ept.lookup(&allocator, guest1).unwrap();
    assert_eq!(
        orig_perms1.bits(),
        EptPermissions::READ_WRITE_EXECUTE.bits()
    );
}
