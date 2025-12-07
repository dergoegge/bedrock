// SPDX-License-Identifier: GPL-2.0

//! GDT (Global Descriptor Table) setup for 64-bit Linux boot.

use super::constants::memory::{GDT_ADDR, TSS_ADDR};

/// Set up GDT for 64-bit mode.
///
/// Linux boot protocol requires:
/// - __BOOT_CS = 0x10 (GDT entry 2): 64-bit code segment
/// - __BOOT_DS = 0x18 (GDT entry 3): 64-bit data segment
///
/// Returns (gdt_base, gdt_limit).
pub fn setup_gdt(memory: &mut [u8]) -> (u64, u16) {
    // GDT entries (8 bytes each):
    // 0x00: Null descriptor
    // 0x08: Reserved (entry 1)
    // 0x10: __BOOT_CS - 64-bit code segment (entry 2)
    // 0x18: __BOOT_DS - 64-bit data segment (entry 3)
    // 0x20: TSS descriptor (16 bytes for 64-bit TSS, entries 4-5)

    let gdt_base = GDT_ADDR as usize;

    // Clear GDT area
    for i in 0..64 {
        memory[gdt_base + i] = 0;
    }

    // Entry 0 (0x00): Null descriptor (already zeroed)
    // Entry 1 (0x08): Reserved (zeroed)

    // Entry 2 (0x10): __BOOT_CS - 64-bit code segment
    // Base = 0, Limit = 0xFFFFF (4GB with G=1)
    // Access: Present(1) | DPL(0) | S(1) | Code(1) | Readable(1) = 0x9A
    // Flags: Granularity(1) | Long mode(1) = 0xA0
    let code_seg: u64 = 0x00AF9A000000FFFF; // 4G flat, 64-bit code
    memory[gdt_base + 0x10..gdt_base + 0x18].copy_from_slice(&code_seg.to_le_bytes());

    // Entry 3 (0x18): __BOOT_DS - 64-bit data segment
    // Access: Present(1) | DPL(0) | S(1) | Data(0) | Writable(1) = 0x92
    // Flags: Granularity(1) | Big(1) = 0xC0
    let data_seg: u64 = 0x00CF92000000FFFF; // 4G flat, 32/64-bit data
    memory[gdt_base + 0x18..gdt_base + 0x20].copy_from_slice(&data_seg.to_le_bytes());

    // Entry 4-5 (0x20): TSS descriptor (64-bit TSS is 16 bytes)
    let tss_base: u64 = TSS_ADDR;
    let tss_limit: u16 = 0x67; // Minimum TSS size

    // TSS descriptor format for 64-bit:
    // Low 8 bytes: Limit[15:0] | Base[23:0] | Access | Limit[19:16] | Flags | Base[31:24]
    // High 8 bytes: Base[63:32] | Reserved
    let tss_low: u64 = (tss_limit as u64)
        | ((tss_base & 0xFFFFFF) << 16)
        | (0x89u64 << 40) // Present, Type=9 (64-bit TSS available)
        | ((tss_base >> 24) & 0xFF) << 56;
    let tss_high: u64 = tss_base >> 32;

    memory[gdt_base + 0x20..gdt_base + 0x28].copy_from_slice(&tss_low.to_le_bytes());
    memory[gdt_base + 0x28..gdt_base + 0x30].copy_from_slice(&tss_high.to_le_bytes());

    // Set up a minimal TSS at TSS_ADDR
    let tss_addr = TSS_ADDR as usize;
    for i in 0..104 {
        // TSS is 104 bytes minimum
        memory[tss_addr + i] = 0;
    }

    // GDT limit is size - 1
    let gdt_limit = 0x30 - 1; // 6 entries (including 16-byte TSS descriptor)

    (GDT_ADDR, gdt_limit)
}
