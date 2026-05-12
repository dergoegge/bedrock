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

    /// IA32_PERF_CAPABILITIES.PEBS_FMT (bits 11:8). Encoding of the PEBS record
    /// layout. Format >= 4 is required for adaptive / EPT-friendly PEBS.
    /// See Intel SDM Vol 3B Section 21.8.
    pub pebs_format: u8,
    /// IA32_PERF_CAPABILITIES.PEBS_BASELINE (bit 14). When set:
    /// IA32_PEBS_ENABLE exists, all counters support PEBS, adaptive PEBS via
    /// MSR_PEBS_DATA_CFG is supported. See Intel SDM Vol 3B Section 21.8.
    pub pebs_baseline: bool,
    /// IA32_PERF_CAPABILITIES.PEBS_TRAP (bit 6). 1 = trap-like (record points
    /// to instruction following overflow); 0 = fault-like.
    pub pebs_trap: bool,
}

impl VmxCapabilities {
    /// Whether the processor supports the architectural prerequisites for
    /// EPT-friendly PEBS as used by precise VM exits: PEBS_BASELINE = 1 and
    /// PEBS record format >= 4. See Intel SDM Vol 3B Section 21.9.5.
    ///
    /// Note: EPT-friendly PEBS itself has no separate CPUID/MSR enumeration
    /// bit; it is implicit on parts that satisfy these constraints (Ice Lake-SP
    /// / 12th-gen Core and later).
    pub fn supports_precise_pebs_exits(&self) -> bool {
        self.pebs_baseline && self.pebs_format >= 4
    }
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
