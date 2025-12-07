/// IA32_EFER - Extended Feature Enable Register.
/// See Intel SDM Vol 4, page 2-86.
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Efer(u64);

impl Efer {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn bits(&self) -> u64 {
        self.0
    }
}

/// IA32_STAR - System Call Target Address (legacy mode segments).
/// See Intel SDM Vol 4, page 2-86 and Vol 2B (SYSCALL/SYSRET instructions).
///
/// Bit layout:
/// - Bits 31:0: Reserved (must be 0)
/// - Bits 47:32: SYSCALL CS selector (SS = CS + 8)
/// - Bits 63:48: SYSRET CS selector base (actual CS/SS derived based on operand size)
///
/// SYSCALL loads:
/// - CS.Selector := IA32_STAR[47:32] AND 0xFFFC (RPL forced to 0)
/// - SS.Selector := IA32_STAR[47:32] + 8
///
/// SYSRET loads (64-bit operand size):
/// - CS.Selector := (IA32_STAR[63:48] + 16) OR 3 (RPL forced to 3)
/// - SS.Selector := (IA32_STAR[63:48] + 8) OR 3
///
/// SYSRET loads (32-bit operand size, compatibility mode return):
/// - CS.Selector := IA32_STAR[63:48] OR 3 (RPL forced to 3)
/// - SS.Selector := (IA32_STAR[63:48] + 8) OR 3
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Star(u64);

impl Star {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn bits(&self) -> u64 {
        self.0
    }
}

/// IA32_LSTAR - IA-32e Mode System Call Target Address.
/// Contains the target RIP for SYSCALL in 64-bit mode.
/// See Intel SDM Vol 4, page 2-86.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Lstar(u64);

impl Lstar {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn bits(&self) -> u64 {
        self.0
    }
}

/// IA32_CSTAR - Compatibility Mode System Call Target Address.
/// Not used as SYSCALL is not recognized in compatibility mode.
/// See Intel SDM Vol 4, page 2-87.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Cstar(u64);

impl Cstar {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn bits(&self) -> u64 {
        self.0
    }
}

/// IA32_FMASK - System Call Flag Mask.
/// On SYSCALL, RFLAGS is ANDed with the complement of this value:
/// `RFLAGS := RFLAGS AND NOT(IA32_FMASK)`
///
/// This allows the kernel to automatically clear specific RFLAGS bits on
/// system call entry. Common bits to mask include IF (interrupts), TF (tracing),
/// DF (direction), and AC (alignment checking).
///
/// See Intel SDM Vol 4, page 2-87 and Vol 2B (SYSCALL instruction).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Fmask(u64);

impl Fmask {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn bits(&self) -> u64 {
        self.0
    }
}

/// IA32_MISC_ENABLE - Miscellaneous Enable Bits.
/// Controls various processor features.
/// See Intel SDM Vol 4, Table 2-2.
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct MiscEnable(u64);

impl MiscEnable {
    /// Branch Trace Storage Unavailable (bit 11, RO).
    const BTS_UNAVAILABLE: u64 = 1 << 11;
    /// PEBS Unavailable (bit 12, RO).
    const PEBS_UNAVAILABLE: u64 = 1 << 12;
    /// MWAIT Enable (bit 18).
    const MWAIT_ENABLE: u64 = 1 << 18;

    pub const fn bits(&self) -> u64 {
        self.0
    }

    fn set(&mut self, flag: u64) {
        self.0 |= flag;
    }

    fn clear(&mut self, flag: u64) {
        self.0 &= !flag;
    }

    /// Create a guest-safe value from host value.
    /// Sets BTS/PEBS unavailable, clears MWAIT.
    pub fn for_guest(host_value: u64) -> Self {
        let mut val = Self(host_value);
        val.set(Self::BTS_UNAVAILABLE | Self::PEBS_UNAVAILABLE);
        val.clear(Self::MWAIT_ENABLE);
        val
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExtendedControlRegisters {
    pub efer: Efer,
}
