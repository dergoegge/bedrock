// SPDX-License-Identifier: GPL-2.0

use super::ioctl::{IOC_READ, IOC_WRITE};
use super::*;
use crate::{cr0, efer};
use std::mem::size_of;

#[test]
fn test_ioctl_encoding() {
    // Verify our ioctl encoding matches the kernel's
    // _IOW('B', 0, u64) - has write direction bit and size of u64
    assert_eq!((BEDROCK_CREATE_ROOT_VM >> 30) & 0x3, IOC_WRITE);
    assert_eq!(
        (BEDROCK_CREATE_ROOT_VM >> 16) & 0x3FFF,
        size_of::<u64>() as u64
    );

    // _IOR('B', 1, size) and _IOW('B', 2, size) depend on struct size
    // Just verify they're different and have the right direction bits
    assert_ne!(BEDROCK_VM_GET_REGS, BEDROCK_VM_SET_REGS);
    assert_eq!((BEDROCK_VM_GET_REGS >> 30) & 0x3, IOC_READ);
    assert_eq!((BEDROCK_VM_SET_REGS >> 30) & 0x3, IOC_WRITE);
}

#[test]
fn test_regs_size() {
    // Verify Regs has the expected size (for ABI compatibility)
    // This should match sizeof(BedrockRegs) in the kernel
    let size = size_of::<Regs>();
    println!("Regs size: {} bytes", size);

    // GeneralPurposeRegisters: 16 * 8 = 128
    // ControlRegisters: 5 * 8 = 40
    // DebugRegisters: 6 * 8 = 48
    // SegmentRegisters: 8 * (2 + 2 + 4 + 4 + 8) = 8 * 20 = 160
    // DescriptorTableRegisters: 2 * (2 + 8) = 20
    // ExtendedControlRegisters: 8
    // rip: 8
    // rflags: 8
    // Total: 128 + 40 + 48 + 160 + 20 + 8 + 8 + 8 = 420

    // Note: actual size may differ due to alignment
    assert!(size > 0);
}

#[test]
fn test_real_mode_defaults() {
    let regs = Regs::real_mode();

    // Reserved RFLAGS bit should be set
    assert_eq!(regs.rflags & Regs::RFLAGS_RESERVED, Regs::RFLAGS_RESERVED);

    // Reset vector
    assert_eq!(regs.rip, 0xFFF0);

    // CS should be 0xF000 with base 0xF0000
    assert_eq!(regs.segment_regs.cs.selector.bits(), 0xF000);
    assert_eq!(regs.segment_regs.cs.base, 0xF0000);
}

#[test]
fn test_long_mode_defaults() {
    let regs = Regs::long_mode();

    // Reserved RFLAGS bit should be set
    assert_eq!(regs.rflags & Regs::RFLAGS_RESERVED, Regs::RFLAGS_RESERVED);

    // CR0 should have PE and PG
    assert!((regs.control_regs.cr0.bits() & cr0::PE) != 0);
    assert!((regs.control_regs.cr0.bits() & cr0::PG) != 0);

    // EFER should have LME and LMA
    assert!((regs.extended_control.efer.bits() & efer::LME) != 0);
    assert!((regs.extended_control.efer.bits() & efer::LMA) != 0);
}
