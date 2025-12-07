// SPDX-License-Identifier: GPL-2.0

//! MP Table setup for APIC discovery.
//!
//! Creates Intel MultiProcessor Specification tables so Linux
//! can discover the Local APIC and I/O APIC.

use super::constants::mptable::{
    BASE_ADDR, IOAPIC_ID, IOAPIC_PADDR, IOAPIC_VERSION, LAPIC_PADDR, LAPIC_VERSION, SPEC_REV,
};

/// Compute MP table checksum (sum of all bytes must be 0).
fn mp_checksum(data: &[u8]) -> u8 {
    let sum: u8 = data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    0u8.wrapping_sub(sum)
}

/// Set up MP tables for APIC discovery.
///
/// Creates Intel MultiProcessor Specification tables at 0xF0000 so Linux
/// can discover the Local APIC and I/O APIC. Without these tables (or ACPI MADT),
/// Linux doesn't know the LAPIC timer exists.
///
/// Table structure:
/// - MP Floating Pointer Structure (16 bytes) at 0xF0000
/// - MP Configuration Table Header (44 bytes)
/// - Processor Entry (20 bytes) for BSP
/// - Bus Entry (8 bytes) for ISA bus
/// - I/O APIC Entry (8 bytes)
/// - Local Interrupt Entries (8 bytes each) for LINT0 and LINT1
pub fn setup_mptable(memory: &mut [u8]) {
    let mut offset = BASE_ADDR;
    let mpfp_addr = offset;

    // === MP Floating Pointer Structure (16 bytes) ===
    // Signature "_MP_"
    memory[offset..offset + 4].copy_from_slice(b"_MP_");
    offset += 4;

    // Physical Address Pointer (points to config table, right after this struct)
    let config_table_addr = (BASE_ADDR + 16) as u32;
    memory[offset..offset + 4].copy_from_slice(&config_table_addr.to_le_bytes());
    offset += 4;

    // Length (in 16-byte units) = 1
    memory[offset] = 1;
    offset += 1;

    // Specification revision
    memory[offset] = SPEC_REV;
    offset += 1;

    // Checksum (will fill in later)
    let checksum_offset = offset;
    memory[offset] = 0;
    offset += 1;

    // MP feature info bytes 1-5 (all 0 = use config table)
    memory[offset..offset + 5].copy_from_slice(&[0, 0, 0, 0, 0]);
    offset += 5;

    // Compute and write floating pointer checksum
    let fp_checksum = mp_checksum(&memory[mpfp_addr..mpfp_addr + 16]);
    memory[checksum_offset] = fp_checksum;

    // === MP Configuration Table Header (44 bytes) ===
    let config_table_start = offset;

    // Signature "PCMP"
    memory[offset..offset + 4].copy_from_slice(b"PCMP");
    offset += 4;

    // Base table length (will fill in later)
    let length_offset = offset;
    memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes());
    offset += 2;

    // Specification revision
    memory[offset] = SPEC_REV;
    offset += 1;

    // Checksum (will fill in later)
    let config_checksum_offset = offset;
    memory[offset] = 0;
    offset += 1;

    // OEM ID (8 bytes)
    memory[offset..offset + 8].copy_from_slice(b"BEDROCK ");
    offset += 8;

    // Product ID (12 bytes)
    memory[offset..offset + 12].copy_from_slice(b"Hypervisor  ");
    offset += 12;

    // OEM table pointer
    memory[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
    offset += 4;

    // OEM table size
    memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes());
    offset += 2;

    // Entry count (will fill in later)
    let entry_count_offset = offset;
    memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes());
    offset += 2;

    // Local APIC address
    memory[offset..offset + 4].copy_from_slice(&LAPIC_PADDR.to_le_bytes());
    offset += 4;

    // Extended table length
    memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes());
    offset += 2;

    // Extended table checksum
    memory[offset] = 0;
    offset += 1;

    // Reserved
    memory[offset] = 0;
    offset += 1;

    let mut entry_count: u16 = 0;

    // === Processor Entry (20 bytes) - Entry type 0 ===
    memory[offset] = 0; // Entry type: processor
    offset += 1;
    memory[offset] = 0; // Local APIC ID
    offset += 1;
    memory[offset] = LAPIC_VERSION; // Local APIC version
    offset += 1;
    memory[offset] = 0x03; // CPU flags: enabled (bit 0) + BSP (bit 1)
    offset += 1;
    // CPU signature: Family 6, Model 85, Stepping 7 (Skylake-SP like)
    let cpu_signature: u32 = (6 << 8) | (5 << 4) | 7;
    memory[offset..offset + 4].copy_from_slice(&cpu_signature.to_le_bytes());
    offset += 4;
    // Feature flags (basic x86-64 features)
    let feature_flags: u32 = 0xBFEBFBFF;
    memory[offset..offset + 4].copy_from_slice(&feature_flags.to_le_bytes());
    offset += 4;
    // Reserved (8 bytes)
    memory[offset..offset + 8].copy_from_slice(&[0u8; 8]);
    offset += 8;
    entry_count += 1;

    // === Bus Entry (8 bytes) - Entry type 1 ===
    memory[offset] = 1; // Entry type: bus
    offset += 1;
    memory[offset] = 0; // Bus ID
    offset += 1;
    memory[offset..offset + 6].copy_from_slice(b"ISA   "); // Bus type
    offset += 6;
    entry_count += 1;

    // === I/O APIC Entry (8 bytes) - Entry type 2 ===
    memory[offset] = 2; // Entry type: I/O APIC
    offset += 1;
    memory[offset] = IOAPIC_ID; // I/O APIC ID
    offset += 1;
    memory[offset] = IOAPIC_VERSION; // I/O APIC version
    offset += 1;
    memory[offset] = 0x01; // Flags: MPC_APIC_USABLE
    offset += 1;
    memory[offset..offset + 4].copy_from_slice(&IOAPIC_PADDR.to_le_bytes());
    offset += 4;
    entry_count += 1;

    // === I/O Interrupt Source Entries (8 bytes each) - Entry type 3 ===
    // Route ISA IRQs to I/O APIC - IRQ4 for COM1 serial port
    for irq in [4u8] {
        memory[offset] = 3; // Entry type: I/O interrupt source
        offset += 1;
        memory[offset] = 0; // Interrupt type: mp_INT (normal interrupt)
        offset += 1;
        memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes()); // Flags: default
        offset += 2;
        memory[offset] = 0; // Source bus ID (ISA bus)
        offset += 1;
        memory[offset] = irq; // Source bus IRQ
        offset += 1;
        memory[offset] = IOAPIC_ID; // Destination I/O APIC ID
        offset += 1;
        memory[offset] = irq; // Destination INTIN# (same as IRQ for ISA)
        offset += 1;
        entry_count += 1;
    }

    // === Local Interrupt Entries (8 bytes each) - Entry type 4 ===
    // LINT0: ExtINT (for 8259 PIC compatibility)
    memory[offset] = 4; // Entry type: local interrupt
    offset += 1;
    memory[offset] = 3; // Interrupt type: ExtINT
    offset += 1;
    memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes()); // Flags
    offset += 2;
    memory[offset] = 0; // Source bus ID
    offset += 1;
    memory[offset] = 0; // Source bus IRQ
    offset += 1;
    memory[offset] = 0xFF; // Destination APIC ID (all)
    offset += 1;
    memory[offset] = 0; // LINTIN# 0
    offset += 1;
    entry_count += 1;

    // LINT1: NMI
    memory[offset] = 4; // Entry type: local interrupt
    offset += 1;
    memory[offset] = 1; // Interrupt type: NMI
    offset += 1;
    memory[offset..offset + 2].copy_from_slice(&0u16.to_le_bytes()); // Flags
    offset += 2;
    memory[offset] = 0; // Source bus ID
    offset += 1;
    memory[offset] = 0; // Source bus IRQ
    offset += 1;
    memory[offset] = 0xFF; // Destination APIC ID (all)
    offset += 1;
    memory[offset] = 1; // LINTIN# 1
    offset += 1;
    entry_count += 1;

    // Fill in entry count
    memory[entry_count_offset..entry_count_offset + 2].copy_from_slice(&entry_count.to_le_bytes());

    // Fill in base table length
    let base_table_length = (offset - config_table_start) as u16;
    memory[length_offset..length_offset + 2].copy_from_slice(&base_table_length.to_le_bytes());

    // Compute and write config table checksum
    let config_checksum = mp_checksum(&memory[config_table_start..offset]);
    memory[config_checksum_offset] = config_checksum;
}
