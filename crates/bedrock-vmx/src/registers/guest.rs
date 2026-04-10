// SPDX-License-Identifier: GPL-2.0

//! Aggregate guest register state.

use super::{
    ControlRegisters, DebugRegisters, DescriptorTableRegisters, ExtendedControlRegisters,
    GeneralPurposeRegisters, SegmentRegisters,
};

/// Complete guest register state.
///
/// Bundles all register groups needed to fully describe guest CPU state.
/// Used as the parameter/return type for `VmContext` register methods.
///
/// This struct is `#[repr(C)]` with the same field layout as the userspace
/// `Regs` and kernel `BedrockRegs` ioctl structs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GuestRegisters {
    /// General-purpose registers (RAX, RCX, ..., R15).
    pub gprs: GeneralPurposeRegisters,
    /// Control registers (CR0, CR2, CR3, CR4, CR8).
    pub control_regs: ControlRegisters,
    /// Debug registers (DR0-DR3, DR6, DR7).
    pub debug_regs: DebugRegisters,
    /// Segment registers (CS, DS, ES, FS, GS, SS, TR, LDTR).
    pub segment_regs: SegmentRegisters,
    /// Descriptor table registers (GDTR, IDTR).
    pub descriptor_tables: DescriptorTableRegisters,
    /// Extended control registers (EFER).
    pub extended_control_regs: ExtendedControlRegisters,
    /// Instruction pointer.
    pub rip: u64,
    /// Flags register.
    pub rflags: u64,
}
