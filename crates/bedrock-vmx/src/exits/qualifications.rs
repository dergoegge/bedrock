// SPDX-License-Identifier: GPL-2.0

//! Exit qualification types for VM exits.
//!
//! These structures parse the exit qualification field from the VMCS
//! for various exit reasons.

/// CR access type (bits 5:4 of exit qualification).
/// Intel SDM Vol 3C, Table 29-3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrAccessType {
    /// MOV to CR.
    MovToCr = 0,
    /// MOV from CR.
    MovFromCr = 1,
    /// CLTS instruction.
    Clts = 2,
    /// LMSW instruction.
    Lmsw = 3,
}

impl TryFrom<u8> for CrAccessType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::MovToCr),
            1 => Ok(Self::MovFromCr),
            2 => Ok(Self::Clts),
            3 => Ok(Self::Lmsw),
            _ => Err(()),
        }
    }
}

/// CR access exit qualification.
/// Intel SDM Vol 3C, Table 29-3.
#[derive(Debug, Clone, Copy)]
pub struct CrAccessQualification {
    /// Control register number (0, 3, 4, or 8).
    pub cr_number: u8,
    /// Type of access.
    pub access_type: CrAccessType,
    /// General-purpose register (for MOV CR or register LMSW).
    pub register: u8,
    /// Source data for LMSW (bits 31:16).
    pub lmsw_source_data: u16,
}

impl From<u64> for CrAccessQualification {
    fn from(qual: u64) -> Self {
        Self {
            cr_number: (qual & 0xF) as u8,
            access_type: CrAccessType::try_from(((qual >> 4) & 0x3) as u8)
                .unwrap_or(CrAccessType::MovToCr),
            register: ((qual >> 8) & 0xF) as u8,
            lmsw_source_data: ((qual >> 16) & 0xFFFF) as u16,
        }
    }
}

/// I/O access direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoDirection {
    /// OUT instruction (write to port).
    Out = 0,
    /// IN instruction (read from port).
    In = 1,
}

/// I/O access size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoAccessSize {
    /// 1-byte access.
    Byte = 1,
    /// 2-byte access.
    Word = 2,
    /// 4-byte access.
    Dword = 4,
}

impl TryFrom<u8> for IoAccessSize {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Byte),
            1 => Ok(Self::Word),
            3 => Ok(Self::Dword),
            _ => Err(()),
        }
    }
}

/// I/O instruction exit qualification.
/// Intel SDM Vol 3C, Table 29-5.
#[derive(Debug, Clone, Copy)]
pub struct IoQualification {
    /// Size of access (1, 2, or 4 bytes).
    pub size: IoAccessSize,
    /// Direction: IN or OUT.
    pub direction: IoDirection,
    /// String instruction (INS/OUTS).
    pub string: bool,
    /// Port number (bits 31:16).
    pub port: u16,
}

impl From<u64> for IoQualification {
    fn from(qual: u64) -> Self {
        let size_bits = (qual & 0x7) as u8;
        Self {
            size: IoAccessSize::try_from(size_bits).unwrap_or(IoAccessSize::Byte),
            direction: if (qual >> 3) & 1 != 0 {
                IoDirection::In
            } else {
                IoDirection::Out
            },
            string: (qual >> 4) & 1 != 0,
            port: ((qual >> 16) & 0xFFFF) as u16,
        }
    }
}

/// EPT violation exit qualification.
/// Intel SDM Vol 3C, Table 29-7.
#[derive(Debug, Clone, Copy)]
pub struct EptViolationQualification {
    /// Violation was caused by a data read.
    pub read: bool,
    /// Violation was caused by a data write.
    pub write: bool,
    /// Violation was caused by an instruction fetch.
    pub execute: bool,
    /// Guest-physical address was readable.
    pub readable: bool,
    /// Guest-physical address was writable.
    pub writable: bool,
    /// Guest-physical address was executable (for supervisor-mode linear addresses).
    pub executable: bool,
    /// Guest linear-address field is valid.
    pub guest_linear_valid: bool,
    /// The access was asynchronous to instruction execution and not part of
    /// event delivery — set for accesses caused by Intel PT trace output, by
    /// PEBS on processors with the EPT-friendly enhancement, or by user-
    /// interrupt delivery.
    pub asynchronous: bool,
}

impl From<u64> for EptViolationQualification {
    fn from(qual: u64) -> Self {
        Self {
            read: qual & (1 << 0) != 0,
            write: qual & (1 << 1) != 0,
            execute: qual & (1 << 2) != 0,
            readable: qual & (1 << 3) != 0,
            writable: qual & (1 << 4) != 0,
            executable: qual & (1 << 5) != 0,
            guest_linear_valid: qual & (1 << 7) != 0,
            asynchronous: qual & (1 << 16) != 0,
        }
    }
}

/// Interruption type for VM-entry/exit interruption-info fields.
/// Intel SDM Vol 3C, Tables 26-18, 26-20, and 26-21.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InterruptionType {
    /// External interrupt.
    ExternalInterrupt = 0,
    /// Reserved.
    Reserved = 1,
    /// Non-maskable interrupt (NMI).
    Nmi = 2,
    /// Hardware exception (has error code if vector 8, 10-14, or 17).
    HardwareException = 3,
    /// Software interrupt (INT n).
    SoftwareInterrupt = 4,
    /// Privileged software exception (INT1).
    PrivilegedSoftwareException = 5,
    /// Software exception (INT3 or INTO).
    SoftwareException = 6,
    /// Other event.
    OtherEvent = 7,
}

/// VM-entry/exit interruption information field.
/// Intel SDM Vol 3C, Section 26.8.3 and Sections 26.9.2-26.9.3.
#[derive(Debug, Clone, Copy)]
pub struct InterruptionInfo {
    /// Interrupt/exception vector (bits 7:0).
    pub vector: u8,
    /// Interruption type (bits 10:8).
    pub interruption_type: InterruptionType,
    /// Error code valid (bit 11).
    pub error_code_valid: bool,
    /// NMI unblocking due to IRET (bit 12, VM-exit only).
    pub nmi_unblocking: bool,
    /// Valid (bit 31).
    pub valid: bool,
}

impl InterruptionInfo {
    /// Create an external interrupt for injection.
    pub fn external_interrupt(vector: u8) -> Self {
        Self {
            vector,
            interruption_type: InterruptionType::ExternalInterrupt,
            error_code_valid: false,
            nmi_unblocking: false,
            valid: true,
        }
    }

    /// Encode to 32-bit VMCS field format.
    pub fn encode(&self) -> u32 {
        let mut value = u32::from(self.vector);
        value |= (self.interruption_type as u32) << 8;
        if self.error_code_valid {
            value |= 1 << 11;
        }
        if self.nmi_unblocking {
            value |= 1 << 12;
        }
        if self.valid {
            value |= 1 << 31;
        }
        value
    }
}

impl From<u32> for InterruptionInfo {
    fn from(value: u32) -> Self {
        Self {
            vector: (value & 0xFF) as u8,
            interruption_type: match (value >> 8) & 0x7 {
                0 => InterruptionType::ExternalInterrupt,
                1 => InterruptionType::Reserved,
                2 => InterruptionType::Nmi,
                3 => InterruptionType::HardwareException,
                4 => InterruptionType::SoftwareInterrupt,
                5 => InterruptionType::PrivilegedSoftwareException,
                6 => InterruptionType::SoftwareException,
                _ => InterruptionType::OtherEvent,
            },
            error_code_valid: (value >> 11) & 1 != 0,
            nmi_unblocking: (value >> 12) & 1 != 0,
            valid: (value >> 31) & 1 != 0,
        }
    }
}

/// Operand size for RDRAND/RDSEED instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RdrandOperandSize {
    /// 16-bit operand.
    Size16 = 0,
    /// 32-bit operand.
    Size32 = 1,
    /// 64-bit operand.
    Size64 = 2,
}

impl RdrandOperandSize {
    /// Returns the size in bits.
    pub fn bits(&self) -> u8 {
        match self {
            Self::Size16 => 16,
            Self::Size32 => 32,
            Self::Size64 => 64,
        }
    }
}

impl TryFrom<u8> for RdrandOperandSize {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Size16),
            1 => Ok(Self::Size32),
            2 => Ok(Self::Size64),
            _ => Err(()),
        }
    }
}

/// RDRAND/RDSEED instruction information from VM-exit instruction-information field.
/// Intel SDM Vol 3C, Table 29-12.
#[derive(Debug, Clone, Copy)]
pub struct RdrandInstructionInfo {
    /// Destination register index (0=RAX, 1=RCX, 2=RDX, 3=RBX, 4=RSP, 5=RBP, 6=RSI, 7=RDI, 8-15=R8-R15).
    pub dest_reg: u8,
    /// Operand size.
    pub operand_size: RdrandOperandSize,
}

impl From<u32> for RdrandInstructionInfo {
    fn from(value: u32) -> Self {
        Self {
            // Bits 6:3 = destination register
            dest_reg: ((value >> 3) & 0xF) as u8,
            // Bits 12:11 = operand size
            operand_size: RdrandOperandSize::try_from(((value >> 11) & 0x3) as u8)
                .unwrap_or(RdrandOperandSize::Size64),
        }
    }
}
