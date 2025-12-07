// =============================================================================
// VMX Capabilities Structure
// =============================================================================

/// VMX capabilities read from processor MSRs.
///
/// Contains the adjusted control values that can be used directly
/// when writing to VMCS control fields.
///
/// See Intel SDM Vol 3C, Appendix A.
///
/// Note: This struct must be `#[repr(C)]` to match the layout of
/// `struct bedrock_vmx_caps` in helpers.c for per-CPU access.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct VmxCapabilities {
    /// Pin-based VM-execution controls (adjusted).
    pub pin_based_exec_ctrl: u32,
    /// Primary processor-based VM-execution controls (adjusted).
    pub cpu_based_exec_ctrl: u32,
    /// Secondary processor-based VM-execution controls (adjusted).
    pub cpu_based_exec_ctrl2: u32,
    /// VM-exit controls (adjusted).
    pub vmexit_ctrl: u32,
    /// VM-entry controls (adjusted).
    pub vmentry_ctrl: u32,

    /// CR0 bits that must be 1 in VMX operation.
    pub cr0_fixed0: u64,
    /// CR0 bits that can be 1 in VMX operation.
    pub cr0_fixed1: u64,
    /// CR4 bits that must be 1 in VMX operation.
    pub cr4_fixed0: u64,
    /// CR4 bits that can be 1 in VMX operation.
    pub cr4_fixed1: u64,

    /// EPT support available.
    pub has_ept: bool,
    /// VPID support available.
    pub has_vpid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmxBasic {
    /// VMCS revision identifier.
    pub vmcs_revision_id: u32,
    /// VMCS size in bytes.
    pub vmcs_size: u16,
    /// Memory type is write-back.
    pub mem_type_wb: bool,
    /// I/O exit information available.
    pub io_exit_info: bool,
    /// VMX flexible controls supported.
    pub vmx_flex_controls: bool,
}
