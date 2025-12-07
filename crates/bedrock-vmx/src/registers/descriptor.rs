use super::segment::SegmentSelector;

/// Global Descriptor Table Register (GDTR).
/// Holds base address and limit for the GDT.
/// Layout matches SGDT/LGDT instruction format: 2-byte limit followed by 8-byte base.
/// See Intel SDM Vol 3A, Section 2.4.1.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Gdtr {
    /// Number of bytes in the table (limit = size - 1).
    pub limit: u16,
    /// Linear address of byte 0 of the GDT.
    pub base: u64,
}

impl Gdtr {
    pub fn new(base: u64, limit: u16) -> Self {
        Self { base, limit }
    }
}

/// Interrupt Descriptor Table Register (IDTR).
/// Holds base address and limit for the IDT.
/// Layout matches SIDT/LIDT instruction format: 2-byte limit followed by 8-byte base.
/// See Intel SDM Vol 3A, Section 2.4.3.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Idtr {
    /// Number of bytes in the table (limit = size - 1).
    pub limit: u16,
    /// Linear address of byte 0 of the IDT.
    pub base: u64,
}

impl Idtr {
    pub fn new(base: u64, limit: u16) -> Self {
        Self { base, limit }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DescriptorTableRegisters {
    pub gdtr: Gdtr,
    pub idtr: Idtr,
}

/// Trait for reading segment selectors and descriptor table registers.
///
/// This trait abstracts access to:
/// - Segment selectors (CS, SS, DS, ES, FS, GS, TR, LDTR)
/// - Descriptor table registers (GDTR, IDTR)
/// - TR base address (for host state, must point to valid TSS)
///
/// Segment selectors are read via MOV instructions:
/// - `MOV r16, Sreg` (opcode 8C): Read segment register to r16
///
/// Descriptor tables are read via SGDT/SIDT/SLDT/STR instructions.
///
/// See Intel SDM Vol 2B for instruction details and
/// Intel SDM Vol 3A, Section 3.4 for segment register format.
///
/// # Example Implementation
///
/// For direct hardware access (unsafe, requires ring 0 for some operations):
/// ```ignore
/// fn read_cs() -> u16 {
///     let sel: u16;
///     unsafe {
///         core::arch::asm!("mov {:x}, cs", out(reg) sel, options(nomem, nostack));
///     }
///     sel
/// }
///
/// fn read_gdtr() -> Gdtr {
///     let mut gdtr = Gdtr { limit: 0, base: 0 };
///     unsafe {
///         core::arch::asm!("sgdt [{}]", in(reg) &mut gdtr, options(nostack));
///     }
///     gdtr
/// }
/// ```
pub trait DescriptorTableAccess {
    /// Read the CS (Code Segment) selector.
    fn read_cs(&self) -> SegmentSelector;

    /// Read the SS (Stack Segment) selector.
    fn read_ss(&self) -> SegmentSelector;

    /// Read the DS (Data Segment) selector.
    fn read_ds(&self) -> SegmentSelector;

    /// Read the ES (Extra Segment) selector.
    fn read_es(&self) -> SegmentSelector;

    /// Read the FS segment selector.
    fn read_fs(&self) -> SegmentSelector;

    /// Read the GS segment selector.
    fn read_gs(&self) -> SegmentSelector;

    /// Read the TR (Task Register) selector.
    fn read_tr(&self) -> SegmentSelector;

    /// Read the TR (Task Register) base address.
    ///
    /// This must point to a valid TSS. On Linux, this is typically
    /// obtained via `this_cpu_ptr(&cpu_tss_rw)` rather than parsing
    /// the GDT descriptor.
    fn read_tr_base(&self) -> u64;

    /// Read the GDTR (Global Descriptor Table Register).
    ///
    /// Returns the limit and base address of the GDT.
    fn read_gdtr(&self) -> Gdtr;

    /// Read the IDTR (Interrupt Descriptor Table Register).
    ///
    /// Returns the limit and base address of the IDT.
    fn read_idtr(&self) -> Idtr;
}
