// =============================================================================
// Control Registers
// See Intel SDM Vol 3A, Section 2.5 - Control Registers
// =============================================================================

/// CR0 - Contains system control flags that control operating mode and states.
/// See Intel SDM Vol 3A, Section 2.5.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Cr0(u64);

impl Cr0 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// CR2 - Contains the page-fault linear address.
/// All 64 bits are writable by software.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Cr2(pub u64);

impl Cr2 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
}

/// CR3 - Contains the physical address of the paging-structure hierarchy base and flags.
/// See Intel SDM Vol 3A, Section 2.5.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Cr3(u64);

impl Cr3 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// CR4 - Contains flags that enable architectural extensions.
/// See Intel SDM Vol 3A, Section 2.5.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Cr4(u64);

impl Cr4 {
    /// VMX Enable - enables VMX operation.
    pub const VMXE: u64 = 1 << 13;

    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }

    pub fn set(&mut self, flag: u64) {
        self.0 |= flag;
    }

    pub fn clear(&mut self, flag: u64) {
        self.0 &= !flag;
    }
}

/// CR8 - Provides read/write access to bits 7:4 of the local APIC's TPR.
/// Available in 64-bit mode only. Only bits 3:0 are used.
/// See Intel SDM Vol 3A, Section 2.5.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Cr8(u64);

impl Cr8 {
    /// Mask for valid TPR bits (only bits 3:0 are used).
    const TPR_MASK: u64 = 0xF;

    pub fn new(value: u64) -> Self {
        Self(value & Self::TPR_MASK)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ControlRegisters {
    pub cr0: Cr0,
    pub cr2: Cr2,
    pub cr3: Cr3,
    pub cr4: Cr4,
    pub cr8: Cr8,
}

// =============================================================================
// Control Register Access
// See Intel SDM Vol 3A, Section 2.5 - Control Registers
// See Intel SDM Vol 2B, MOV—Move to/from Control Registers
// =============================================================================

/// Error returned by control register read/write operations.
/// See Intel SDM Vol 2B (MOV—Move to/from Control Registers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrError {
    /// Attempted to access an invalid control register (CR1, CR5-CR7, CR9-CR15).
    /// Results in #UD on hardware.
    InvalidRegister,
    /// The operation requires CPL 0.
    /// Results in #GP(0) on hardware.
    PrivilegeViolation,
    /// Invalid bit combination in CR0 (e.g., PG=1 with PE=0, or CD=0 with NW=1).
    /// Results in #GP(0) on hardware.
    InvalidCr0Combination,
    /// Attempted to set reserved bits in CR0[63:32].
    /// Results in #GP(0) on hardware.
    Cr0ReservedBits,
    /// Attempted to set reserved bits in CR4.
    /// Results in #GP(0) on hardware.
    Cr4ReservedBits,
    /// Attempted to set reserved bits in CR8 (bits 63:4).
    /// Results in #GP(0) on hardware.
    Cr8ReservedBits,
    /// Attempted to set reserved bits in CR3[63:MAXPHYADDR].
    /// Results in #GP(0) on hardware.
    Cr3ReservedBits,
    /// CR8 access attempted outside of 64-bit mode.
    /// CR8 is only available in 64-bit mode.
    Cr8NotAvailable,
    /// Attempted to change CR4.PCIDE from 0 to 1 while CR3[11:0] != 0.
    /// Results in #GP(0) on hardware.
    PcidEnableWithNonZeroCr3,
    /// Attempted to clear CR0.PG in 64-bit mode.
    /// Results in #GP(0) on hardware.
    CannotDisablePaging64Bit,
    /// Attempted to clear CR4.PAE in IA-32e mode.
    /// Results in #GP(0) on hardware.
    CannotDisablePae,
    /// Platform-specific error.
    PlatformError(u32),
}

/// Result type for control register operations.
pub type CrResult<T> = Result<T, CrError>;

/// Trait for reading and writing Control Registers (CR0, CR2, CR3, CR4, CR8).
///
/// Control registers are accessed via the MOV instruction:
/// - MOV r64, CRn (opcode 0F 20): Reads the control register into a GPR
/// - MOV CRn, r64 (opcode 0F 22): Writes a GPR value to the control register
///
/// Both forms require CPL 0 (ring 0). Invalid register access causes #UD,
/// and invalid values or reserved bit violations cause #GP(0).
///
/// MOV CR instructions (except MOV CR8) are serializing instructions.
///
/// See Intel SDM Vol 3A, Section 2.5 (Control Registers) and
/// Intel SDM Vol 2B, MOV—Move to/from Control Registers.
///
/// # Safety
///
/// Implementors must ensure:
/// - Operations are only performed at CPL 0
/// - Invalid register numbers are rejected
/// - Reserved bits are properly handled
/// - Invalid bit combinations are detected for CR0
///
/// # Example Implementation
///
/// For direct hardware access (unsafe, requires ring 0):
/// ```ignore
/// unsafe fn read_cr0() -> u64 {
///     let value: u64;
///     core::arch::asm!(
///         "mov {}, cr0",
///         out(reg) value,
///         options(nomem, nostack)
///     );
///     value
/// }
///
/// unsafe fn write_cr0(value: u64) {
///     core::arch::asm!(
///         "mov cr0, {}",
///         in(reg) value,
///         options(nomem, nostack)
///     );
/// }
/// ```
pub trait CrAccess {
    /// Read CR0 - Contains system control flags that control operating mode and states.
    fn read_cr0(&self) -> CrResult<Cr0>;

    /// Read CR3 - Contains the physical address of the paging-structure hierarchy base.
    fn read_cr3(&self) -> CrResult<Cr3>;

    /// Read CR4 - Contains flags that enable architectural extensions.
    fn read_cr4(&self) -> CrResult<Cr4>;

    /// Write CR4 - Contains flags that enable architectural extensions.
    fn write_cr4(&self, value: &Cr4) -> CrResult<()>;

    /// Set CR4.VMXE (bit 13) to enable VMX operation.
    ///
    /// On real hardware, this must update the kernel's CR4 shadow (cpu_tlbstate.cr4)
    /// in addition to the actual CR4 register. A raw MOV to CR4 would desync the
    /// shadow, causing the kernel to #GP when it later writes CR4 without VMXE
    /// during context switches.
    fn set_vmxe(&self) -> CrResult<()>;

    /// Clear CR4.VMXE (bit 13) to disable VMX.
    ///
    /// Must only be called after VMXOFF (outside VMX operation).
    /// Like set_vmxe, must update the kernel's CR4 shadow.
    fn clear_vmxe(&self) -> CrResult<()>;
}
