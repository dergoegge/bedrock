// SPDX-License-Identifier: GPL-2.0

//! Initial register state for Linux 64-bit boot.

use super::constants::memory::{BOOT_PARAMS_ADDR, PML4_ADDR, TSS_ADDR};
use crate::registers::{seg_ar, Cr3, Gdtr, Regs, SegmentRegister};

/// Create register state for Linux 64-bit boot.
///
/// Per Linux boot protocol (Documentation/arch/x86/boot.rst):
/// - CS = __BOOT_CS (0x10)
/// - DS/ES/SS = __BOOT_DS (0x18)
/// - RSI = physical address of boot_params
/// - Interrupts disabled
pub fn linux_boot_regs(kernel_entry: u64, gdt_base: u64, gdt_limit: u16) -> Regs {
    let mut regs = Regs::long_mode();

    // RIP = kernel entry point
    regs.rip = kernel_entry;

    // RSI = pointer to boot_params (zero page) - REQUIRED by boot protocol
    regs.gprs.rsi = BOOT_PARAMS_ADDR;

    // RSP = give it a stack (Linux kernel sets up its own stack)
    regs.gprs.rsp = BOOT_PARAMS_ADDR;

    // CR3 = page table base
    regs.control_regs.cr3 = Cr3::new(PML4_ADDR);

    // RFLAGS = interrupts disabled, reserved bit set
    regs.rflags = Regs::RFLAGS_RESERVED;

    // Set up GDTR to point to our GDT
    regs.descriptor_tables.gdtr = Gdtr::new(gdt_base, gdt_limit);

    // Set up segment registers per Linux boot protocol:
    // CS = __BOOT_CS = 0x10 (GDT entry 2)
    let code_ar = seg_ar::PRESENT
        | seg_ar::S
        | seg_ar::CODE_SEGMENT
        | seg_ar::CODE_READABLE
        | seg_ar::CODE_ACCESSED
        | seg_ar::LONG_MODE;
    regs.segment_regs.cs = SegmentRegister::new(0x10, code_ar, 0, 0);

    // DS/ES/FS/GS/SS = __BOOT_DS = 0x18 (GDT entry 3)
    let data_ar = seg_ar::PRESENT | seg_ar::S | seg_ar::DATA_WRITABLE | seg_ar::DATA_ACCESSED;
    let data_seg = SegmentRegister::new(0x18, data_ar, 0, 0);
    regs.segment_regs.ds = data_seg;
    regs.segment_regs.es = data_seg;
    regs.segment_regs.fs = data_seg;
    regs.segment_regs.gs = data_seg;
    regs.segment_regs.ss = data_seg;

    // TR = 0x20 (TSS) - entry 4 in our GDT
    let tss_ar = seg_ar::PRESENT | 0x0B; // Type = 64-bit TSS (busy)
    regs.segment_regs.tr = SegmentRegister::new(0x20, tss_ar, 0x67, TSS_ADDR);

    // LDTR = unusable
    regs.segment_regs.ldtr = SegmentRegister::new(0, seg_ar::UNUSABLE, 0, 0);

    regs
}
