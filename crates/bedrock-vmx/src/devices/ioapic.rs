// SPDX-License-Identifier: GPL-2.0

//! I/O APIC (Advanced Programmable Interrupt Controller) emulation.
//!
//! The I/O APIC routes external interrupts (like IRQ4 from serial) to the
//! local APIC. It's accessed via MMIO at 0xFEC00000.

#[cfg(not(feature = "cargo"))]
use super::super::logging::{StateHash, Xxh64Hasher};
#[cfg(feature = "cargo")]
use crate::logging::{StateHash, Xxh64Hasher};

/// Number of I/O APIC redirection table entries.
pub const IOAPIC_NUM_PINS: usize = 24;

/// I/O APIC state for interrupt routing.
///
/// The I/O APIC routes external interrupts (like IRQ4 from serial) to the
/// local APIC. It's accessed via MMIO at 0xFEC00000.
#[derive(Clone, Debug)]
pub struct IoApicState {
    /// I/O Register Select - selects which register to access via IOWIN
    pub ioregsel: u32,
    /// I/O APIC ID register (index 0x00)
    pub id: u32,
    /// Redirection table entries (indices 0x10-0x3F, 24 entries x 64 bits)
    /// Each entry specifies how an IRQ pin is routed to the local APIC.
    pub redtbl: [u64; IOAPIC_NUM_PINS],
}

impl Default for IoApicState {
    fn default() -> Self {
        // Initialize all redirection entries as masked (bit 16 = 1)
        let mut redtbl = [0u64; IOAPIC_NUM_PINS];
        for entry in &mut redtbl {
            *entry = 1 << 16; // Masked
        }
        Self {
            ioregsel: 0,
            id: 0,
            redtbl,
        }
    }
}

impl StateHash for IoApicState {
    fn state_hash(&self) -> u64 {
        let mut h = Xxh64Hasher::new();
        h.write_u32(self.ioregsel);
        h.write_u32(self.id);
        for &entry in &self.redtbl {
            h.write_u64(entry);
        }
        h.finish()
    }
}
