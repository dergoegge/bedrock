// SPDX-License-Identifier: GPL-2.0

//! Register types for userspace-kernel communication.
//!
//! This module re-exports register types from `bedrock-vmx` to ensure
//! ABI compatibility between userspace and kernel.

// Re-export all register types from bedrock-vmx
pub use bedrock_vmx::registers::{
    ControlRegisters, Cr0, Cr2, Cr3, Cr4, Cr8, DebugRegisters, DescriptorTableRegisters, Efer,
    ExtendedControlRegisters, Gdtr, GeneralPurposeRegisters, GuestRegisters, Idtr,
    SegmentAccessRights, SegmentRegister, SegmentRegisters, SegmentSelector,
};

// Local constants for control registers used by userspace tools.
// These are not needed in the kernel module.

/// CR0 bit constants for userspace.
pub mod cr0 {
    /// Protection Enable.
    pub const PE: u64 = 1 << 0;
    /// Extension Type.
    pub const ET: u64 = 1 << 4;
    /// Numeric Error.
    pub const NE: u64 = 1 << 5;
    /// Paging.
    pub const PG: u64 = 1 << 31;
}

/// CR4 bit constants for userspace.
pub mod cr4 {
    /// Physical Address Extension.
    pub const PAE: u64 = 1 << 5;
}

/// EFER bit constants for userspace.
pub mod efer {
    /// Long Mode Enable.
    pub const LME: u64 = 1 << 8;
    /// Long Mode Active.
    pub const LMA: u64 = 1 << 10;
}

/// Segment access rights constants for userspace.
pub mod seg_ar {
    /// Data: Accessed.
    pub const DATA_ACCESSED: u32 = 1 << 0;
    /// Data: Writable.
    pub const DATA_WRITABLE: u32 = 1 << 1;
    /// Code: Accessed.
    pub const CODE_ACCESSED: u32 = 1 << 0;
    /// Code: Readable.
    pub const CODE_READABLE: u32 = 1 << 1;
    /// Code segment indicator.
    pub const CODE_SEGMENT: u32 = 1 << 3;
    /// S - Descriptor type (code or data).
    pub const S: u32 = 1 << 4;
    /// P - Segment present.
    pub const PRESENT: u32 = 1 << 7;
    /// L - 64-bit mode active.
    pub const LONG_MODE: u32 = 1 << 13;
    /// Segment unusable.
    pub const UNUSABLE: u32 = 1 << 16;
}

/// Complete VM register state for GET_REGS/SET_REGS ioctls.
///
/// This struct must have the exact same layout as the kernel's BedrockRegs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Regs {
    /// General-purpose registers.
    pub gprs: GeneralPurposeRegisters,
    /// Control registers.
    pub control_regs: ControlRegisters,
    /// Debug registers.
    pub debug_regs: DebugRegisters,
    /// Segment registers.
    pub segment_regs: SegmentRegisters,
    /// Descriptor table registers.
    pub descriptor_tables: DescriptorTableRegisters,
    /// Extended control registers.
    pub extended_control: ExtendedControlRegisters,
    /// Instruction pointer.
    pub rip: u64,
    /// Flags register.
    pub rflags: u64,
}

impl Default for Regs {
    fn default() -> Self {
        Self {
            gprs: GeneralPurposeRegisters::default(),
            control_regs: ControlRegisters {
                cr0: Cr0::new(0),
                cr2: Cr2::new(0),
                cr3: Cr3::new(0),
                cr4: Cr4::new(0),
                cr8: Cr8::new(0),
            },
            debug_regs: DebugRegisters {
                dr0: 0,
                dr1: 0,
                dr2: 0,
                dr3: 0,
                dr6: 0,
                dr7: 0,
            },
            segment_regs: SegmentRegisters {
                cs: SegmentRegister::new(0, 0, 0, 0),
                ds: SegmentRegister::new(0, 0, 0, 0),
                es: SegmentRegister::new(0, 0, 0, 0),
                fs: SegmentRegister::new(0, 0, 0, 0),
                gs: SegmentRegister::new(0, 0, 0, 0),
                ss: SegmentRegister::new(0, 0, 0, 0),
                tr: SegmentRegister::new(0, 0, 0, 0),
                ldtr: SegmentRegister::new(0, 0, 0, 0),
            },
            descriptor_tables: DescriptorTableRegisters {
                gdtr: Gdtr::new(0, 0),
                idtr: Idtr::new(0, 0),
            },
            extended_control: ExtendedControlRegisters { efer: Efer::new(0) },
            rip: 0,
            rflags: 0,
        }
    }
}

impl Regs {
    // RFLAGS bits
    /// Carry Flag
    pub const RFLAGS_CF: u64 = 1 << 0;
    /// Reserved, always 1
    pub const RFLAGS_RESERVED: u64 = 1 << 1;
    /// Parity Flag
    pub const RFLAGS_PF: u64 = 1 << 2;
    /// Auxiliary Carry Flag
    pub const RFLAGS_AF: u64 = 1 << 4;
    /// Zero Flag
    pub const RFLAGS_ZF: u64 = 1 << 6;
    /// Sign Flag
    pub const RFLAGS_SF: u64 = 1 << 7;
    /// Trap Flag
    pub const RFLAGS_TF: u64 = 1 << 8;
    /// Interrupt Enable Flag
    pub const RFLAGS_IF: u64 = 1 << 9;
    /// Direction Flag
    pub const RFLAGS_DF: u64 = 1 << 10;
    /// Overflow Flag
    pub const RFLAGS_OF: u64 = 1 << 11;
    /// I/O Privilege Level mask
    pub const RFLAGS_IOPL_MASK: u64 = 0b11 << 12;
    /// Nested Task
    pub const RFLAGS_NT: u64 = 1 << 14;
    /// Resume Flag
    pub const RFLAGS_RF: u64 = 1 << 16;
    /// Virtual 8086 Mode
    pub const RFLAGS_VM: u64 = 1 << 17;
    /// Alignment Check
    pub const RFLAGS_AC: u64 = 1 << 18;
    /// Virtual Interrupt Flag
    pub const RFLAGS_VIF: u64 = 1 << 19;
    /// Virtual Interrupt Pending
    pub const RFLAGS_VIP: u64 = 1 << 20;
    /// ID Flag
    pub const RFLAGS_ID: u64 = 1 << 21;

    /// Create a new Regs with reasonable defaults for real mode.
    pub fn real_mode() -> Self {
        let mut regs = Self {
            rflags: Self::RFLAGS_RESERVED,
            ..Default::default()
        };

        // CR0: just protection and cache settings
        regs.control_regs.cr0 = Cr0::new(cr0::NE | cr0::ET);

        // Real mode segments: base = selector << 4, limit = 0xFFFF
        let real_seg = SegmentRegister::new(0, 0x93, 0xFFFF, 0);
        regs.segment_regs.cs = SegmentRegister::new(0xF000, 0x9B, 0xFFFF, 0xF0000);
        regs.segment_regs.ds = real_seg;
        regs.segment_regs.es = real_seg;
        regs.segment_regs.fs = real_seg;
        regs.segment_regs.gs = real_seg;
        regs.segment_regs.ss = real_seg;

        // TR and LDTR must be set up
        regs.segment_regs.tr = SegmentRegister::new(0, 0x8B, 0xFFFF, 0);
        regs.segment_regs.ldtr = SegmentRegister::new(0, seg_ar::UNUSABLE, 0, 0);

        // Descriptor tables
        regs.descriptor_tables.gdtr = Gdtr::new(0, 0xFFFF);
        regs.descriptor_tables.idtr = Idtr::new(0, 0xFFFF);

        // Start at reset vector
        regs.rip = 0xFFF0;

        regs
    }

    /// Create a new Regs for 64-bit long mode.
    pub fn long_mode() -> Self {
        let mut regs = Self {
            rflags: Self::RFLAGS_RESERVED,
            ..Default::default()
        };

        // CR0: PE + PG + NE + ET
        regs.control_regs.cr0 = Cr0::new(cr0::PE | cr0::PG | cr0::NE | cr0::ET);

        // CR4: PAE required for long mode
        regs.control_regs.cr4 = Cr4::new(cr4::PAE);

        // EFER: LME + LMA for long mode
        regs.extended_control.efer = Efer::new(efer::LME | efer::LMA);

        // 64-bit code segment: present, DPL=0, code, long mode
        // Type must be 9, 11, 13, or 15 (accessed code segment) per SDM 28.3.1.2
        let code_ar = seg_ar::PRESENT
            | seg_ar::S
            | seg_ar::CODE_SEGMENT
            | seg_ar::CODE_READABLE
            | seg_ar::CODE_ACCESSED // Required: type must be accessed
            | seg_ar::LONG_MODE;
        regs.segment_regs.cs = SegmentRegister::new(0x08, code_ar, 0, 0);

        // 64-bit data segments: present, DPL=0, data, writable
        // Type must be 3 or 7 (read/write accessed) for SS per SDM 28.3.1.2
        let data_ar = seg_ar::PRESENT | seg_ar::S | seg_ar::DATA_WRITABLE | seg_ar::DATA_ACCESSED; // Required: type must be accessed
        let data_seg = SegmentRegister::new(0x10, data_ar, 0, 0);
        regs.segment_regs.ds = data_seg;
        regs.segment_regs.es = data_seg;
        regs.segment_regs.fs = data_seg;
        regs.segment_regs.gs = data_seg;
        regs.segment_regs.ss = data_seg;

        // TR must be present and point to valid TSS
        let tss_ar = seg_ar::PRESENT | 0x0B; // Type = 64-bit TSS (busy)
        regs.segment_regs.tr = SegmentRegister::new(0x18, tss_ar, 0x67, 0);
        regs.segment_regs.ldtr = SegmentRegister::new(0, seg_ar::UNUSABLE, 0, 0);

        regs
    }
}
