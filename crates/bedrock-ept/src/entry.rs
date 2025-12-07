// SPDX-License-Identifier: GPL-2.0

//! EPT entry definitions and manipulation.

use super::traits::HostPhysAddr;

/// EPT entry permission flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct EptPermissions(u64);

impl EptPermissions {
    /// Read, write, and execute permissions.
    pub const READ_WRITE_EXECUTE: Self = Self(0b111);

    /// Read and execute permissions (no write) - used for COW pages in forked VMs.
    pub const READ_EXECUTE: Self = Self(0b101);

    /// Create permissions from raw bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits & 0b111)
    }

    /// Get the raw bits.
    pub const fn bits(self) -> u64 {
        self.0
    }
}

/// EPT memory types (encoded in bits 5:3 of leaf entries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EptMemoryType {
    /// Write Back (WB).
    WriteBack = 6,
}

impl EptMemoryType {
    fn to_bits(self) -> u64 {
        (self as u64) << 3
    }
}

/// An EPT entry (used at all levels: PML4, PDPT, PD, PT).
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct EptEntry(u64);

impl EptEntry {
    /// Mask for the physical address in the entry (bits 51:12).
    const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    /// Create an EPT entry pointing to the next level page table.
    pub fn table_entry(addr: HostPhysAddr, perms: EptPermissions) -> Self {
        Self((addr.as_u64() & Self::ADDR_MASK) | perms.bits())
    }

    /// Create an EPT entry for a 4KB page.
    pub fn page_entry_4k(
        addr: HostPhysAddr,
        perms: EptPermissions,
        mem_type: EptMemoryType,
    ) -> Self {
        Self((addr.as_u64() & Self::ADDR_MASK) | perms.bits() | mem_type.to_bits())
    }

    /// Returns true if this entry is present (has any permission bits set).
    pub const fn is_present(&self) -> bool {
        (self.0 & 0b111) != 0
    }

    /// Get the physical address from this entry.
    pub const fn addr(&self) -> HostPhysAddr {
        HostPhysAddr::new(self.0 & Self::ADDR_MASK)
    }

    /// Get the permission bits from this entry.
    pub const fn permissions(&self) -> EptPermissions {
        EptPermissions::from_bits(self.0)
    }

    /// Get the raw u64 value.
    pub const fn raw(&self) -> u64 {
        self.0
    }
}
