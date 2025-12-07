/// Segment selector - visible part of a segment register.
/// See Intel SDM Vol 3A, Section 3.4.2.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct SegmentSelector(u16);

impl SegmentSelector {
    pub fn new(value: u16) -> Self {
        Self(value)
    }

    pub fn bits(&self) -> u16 {
        self.0
    }
}

/// Segment access rights - format used in VMCS guest-state area.
/// See Intel SDM Vol 3C, Table 26-2.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct SegmentAccessRights(u32);

impl SegmentAccessRights {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Complete segment register state including hidden descriptor cache.
/// See Intel SDM Vol 3A, Section 3.4.3 and Vol 3C, Section 26.4.1.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SegmentRegister {
    /// Visible part - the segment selector.
    pub selector: SegmentSelector,
    /// Padding to align access_rights.
    _pad: u16,
    /// Hidden part - access rights from descriptor.
    pub access_rights: SegmentAccessRights,
    /// Hidden part - segment limit (in bytes).
    pub limit: u32,
    /// Hidden part - segment base address.
    pub base: u64,
}

impl SegmentRegister {
    pub fn new(selector: u16, access_rights: u32, limit: u32, base: u64) -> Self {
        Self {
            selector: SegmentSelector::new(selector),
            _pad: 0,
            access_rights: SegmentAccessRights::new(access_rights),
            limit,
            base,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SegmentRegisters {
    /// Code segment - must always be valid.
    pub cs: SegmentRegister,
    /// Data segment - not used in 64-bit mode.
    pub ds: SegmentRegister,
    /// Extra segment - not used in 64-bit mode.
    pub es: SegmentRegister,
    /// Additional data segment - base used in 64-bit mode.
    pub fs: SegmentRegister,
    /// Additional data segment - base used in 64-bit mode.
    pub gs: SegmentRegister,
    /// Stack segment - DPL equals CPL.
    pub ss: SegmentRegister,
    /// Task register - points to TSS.
    pub tr: SegmentRegister,
    /// Local Descriptor Table Register.
    pub ldtr: SegmentRegister,
}
