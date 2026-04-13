// =============================================================================
// Model-Specific Registers (MSRs)
// See Intel SDM Vol 4, Chapter 2 - Model-Specific Registers
// =============================================================================

/// MSR addresses for architectural MSRs.
/// See Intel SDM Vol 4, Table 2-2.
pub mod msr {
    /// APIC base address and flags.
    pub const IA32_APIC_BASE: u32 = 0x1B;
    /// Control features in Intel 64 processor.
    pub const IA32_FEATURE_CONTROL: u32 = 0x3A;
    /// Per logical processor TSC adjust.
    pub const IA32_TSC_ADJUST: u32 = 0x3B;
    /// Speculation control (IBRS, STIBP, SSBD).
    pub const IA32_SPEC_CTRL: u32 = 0x48;
    /// Prediction command (IBPB).
    pub const IA32_PRED_CMD: u32 = 0x49;
    /// Protected Processor Inventory Number control.
    pub const IA32_PPIN_CTL: u32 = 0x4E;
    /// MKTME Key ID Partitioning (Multi-Key Total Memory Encryption).
    /// Read-only. Returns number of activated KeyIDs for TME-MK and TDX.
    pub const IA32_MKTME_KEYID_PARTITIONING: u32 = 0x87;
    /// BIOS update signature.
    pub const IA32_BIOS_SIGN_ID: u32 = 0x8B;
    /// General purpose performance counter 0.
    pub const IA32_PMC0: u32 = 0xC1;
    /// General purpose performance counter 1.
    pub const IA32_PMC1: u32 = 0xC2;
    /// General purpose performance counter 2.
    pub const IA32_PMC2: u32 = 0xC3;
    /// General purpose performance counter 3.
    pub const IA32_PMC3: u32 = 0xC4;
    /// General purpose performance counter 4.
    pub const IA32_PMC4: u32 = 0xC5;
    /// General purpose performance counter 5.
    pub const IA32_PMC5: u32 = 0xC6;
    /// General purpose performance counter 6.
    pub const IA32_PMC6: u32 = 0xC7;
    /// General purpose performance counter 7.
    pub const IA32_PMC7: u32 = 0xC8;
    /// Maximum performance frequency clock count.
    pub const IA32_MPERF: u32 = 0xE7;
    /// Actual performance frequency clock count.
    pub const IA32_APERF: u32 = 0xE8;
    /// Platform information (max non-turbo ratio, etc.).
    pub const IA32_PLATFORM_INFO: u32 = 0xCE;
    /// MTRR capabilities.
    pub const IA32_MTRRCAP: u32 = 0xFE;
    /// Miscellaneous feature enables (CPUID faulting control).
    pub const IA32_MISC_FEATURES_ENABLES: u32 = 0x140;
    /// Machine check global capabilities.
    pub const IA32_MCG_CAP: u32 = 0x179;
    /// Machine check global status.
    pub const IA32_MCG_STATUS: u32 = 0x17A;
    /// Performance event select 0.
    pub const IA32_PERFEVTSEL0: u32 = 0x186;
    /// Performance event select 1.
    pub const IA32_PERFEVTSEL1: u32 = 0x187;
    /// Performance event select 2.
    pub const IA32_PERFEVTSEL2: u32 = 0x188;
    /// Performance event select 3.
    pub const IA32_PERFEVTSEL3: u32 = 0x189;
    /// Performance event select 4.
    pub const IA32_PERFEVTSEL4: u32 = 0x18A;
    /// Performance event select 5.
    pub const IA32_PERFEVTSEL5: u32 = 0x18B;
    /// Performance event select 6.
    pub const IA32_PERFEVTSEL6: u32 = 0x18C;
    /// Performance event select 7.
    pub const IA32_PERFEVTSEL7: u32 = 0x18D;
    /// Performance status.
    pub const IA32_PERF_STATUS: u32 = 0x198;
    /// Performance control.
    pub const IA32_PERF_CTL: u32 = 0x199;
    /// Thermal interrupt control.
    pub const IA32_THERM_INTERRUPT: u32 = 0x19B;
    /// Thermal status.
    pub const IA32_THERM_STATUS: u32 = 0x19C;
    /// Miscellaneous enable bits.
    pub const IA32_MISC_ENABLE: u32 = 0x1A0;
    /// Off-core response event select 0.
    pub const IA32_OFFCORE_RSP_0: u32 = 0x1A6;
    /// Off-core response event select 1.
    pub const IA32_OFFCORE_RSP_1: u32 = 0x1A7;
    /// Performance energy bias hint.
    pub const IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;
    /// Package thermal status.
    pub const IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
    /// Package thermal interrupt.
    pub const IA32_PACKAGE_THERM_INTERRUPT: u32 = 0x1B2;
    /// Last Branch Record Top of Stack.
    pub const IA32_LBR_TOS: u32 = 0x1C9;
    /// SYSENTER CS.
    pub const IA32_SYSENTER_CS: u32 = 0x174;
    /// SYSENTER ESP.
    pub const IA32_SYSENTER_ESP: u32 = 0x175;
    /// SYSENTER EIP.
    pub const IA32_SYSENTER_EIP: u32 = 0x176;
    /// PAT (Page Attribute Table).
    pub const IA32_PAT: u32 = 0x277;
    /// MTRR default memory type.
    pub const IA32_MTRR_DEF_TYPE: u32 = 0x2FF;

    // Variable range MTRRs (10 pairs of base/mask)
    /// First variable range MTRR base.
    pub const IA32_MTRR_PHYSBASE0: u32 = 0x200;
    /// Last variable range MTRR mask (for 10 variable MTRRs).
    pub const IA32_MTRR_PHYSMASK9: u32 = 0x213;

    // Fixed range MTRRs
    /// Fixed range MTRR for 64K at 0x00000.
    pub const IA32_MTRR_FIX64K_00000: u32 = 0x250;
    /// Fixed range MTRR for 16K at 0x80000.
    pub const IA32_MTRR_FIX16K_80000: u32 = 0x258;
    /// Fixed range MTRR for 16K at 0xA0000.
    pub const IA32_MTRR_FIX16K_A0000: u32 = 0x259;
    /// First fixed range MTRR for 4K (at 0xC0000).
    pub const IA32_MTRR_FIX4K_C0000: u32 = 0x268;
    /// Last fixed range MTRR for 4K (at 0xF8000).
    pub const IA32_MTRR_FIX4K_F8000: u32 = 0x26F;

    /// PEBS load latency threshold.
    pub const IA32_PEBS_LD_LAT_THRESHOLD: u32 = 0x3F6;
    /// PEBS frontend.
    pub const IA32_PEBS_FRONTEND: u32 = 0x3F7;
    /// Atom core frequency ratios.
    pub const MSR_ATOM_CORE_RATIOS: u32 = 0x66A;
    /// Atom core voltage ID ratios.
    pub const MSR_ATOM_CORE_VIDS: u32 = 0x66B;
    /// Atom core turbo ratios.
    pub const MSR_ATOM_CORE_TURBO_RATIOS: u32 = 0x66C;
    /// Debug control (LBR, BTF, FREEZE_PERFMON_ON_PMI, etc.).
    pub const IA32_DEBUGCTL: u32 = 0x1D9;

    /// Fixed-function performance counter 0 (INST_RETIRED.ANY).
    pub const IA32_FIXED_CTR0: u32 = 0x309;
    /// Performance capabilities.
    pub const IA32_PERF_CAPABILITIES: u32 = 0x345;
    /// Fixed counter control.
    pub const IA32_FIXED_CTR_CTRL: u32 = 0x38D;
    /// Performance counter global status (overflow flags, freeze indicators).
    pub const IA32_PERF_GLOBAL_STATUS: u32 = 0x38E;
    /// Performance counter global control (enable bits for all counters).
    pub const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
    /// Performance counter global status reset (clear overflow/freeze bits).
    pub const IA32_PERF_GLOBAL_STATUS_RESET: u32 = 0x390;
    /// PEBS enable (controls which counters generate PEBS records).
    pub const IA32_PEBS_ENABLE: u32 = 0x3F1;
    /// DS save area address (points to DS buffer management area for BTS/PEBS).
    pub const IA32_DS_AREA: u32 = 0x600;
    /// TSC deadline.
    pub const IA32_TSC_DEADLINE: u32 = 0x6E0;

    // Extended Feature Enable Register (EFER) and related MSRs
    // See Intel SDM Vol 4, page 2-86

    /// Extended Feature Enable Register.
    pub const IA32_EFER: u32 = 0xC000_0080;
    /// System call target address (legacy mode).
    pub const IA32_STAR: u32 = 0xC000_0081;
    /// System call target address (64-bit mode).
    pub const IA32_LSTAR: u32 = 0xC000_0082;
    /// System call target address (compatibility mode, not used).
    pub const IA32_CSTAR: u32 = 0xC000_0083;
    /// System call flag mask.
    pub const IA32_FMASK: u32 = 0xC000_0084;
    /// FS base address.
    pub const IA32_FS_BASE: u32 = 0xC000_0100;
    /// GS base address.
    pub const IA32_GS_BASE: u32 = 0xC000_0101;
    /// Kernel GS base (swapped by SWAPGS).
    pub const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;
    /// Auxiliary TSC signature.
    pub const IA32_TSC_AUX: u32 = 0xC000_0103;

    // AMD MSRs (probed by Linux even on Intel)

    /// AMD decode configuration.
    pub const MSR_AMD64_DE_CFG: u32 = 0xC001_1029;

    // VMX Capability MSRs
    // See Intel SDM Vol 4, pages 2-44 to 2-46

    /// Basic VMX information.
    pub const IA32_VMX_BASIC: u32 = 0x480;
    /// Pin-based VM-execution controls.
    pub const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
    /// Primary processor-based VM-execution controls.
    pub const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
    /// VM-exit controls.
    pub const IA32_VMX_EXIT_CTLS: u32 = 0x483;
    /// VM-entry controls.
    pub const IA32_VMX_ENTRY_CTLS: u32 = 0x484;
    /// CR0 bits fixed to 0.
    pub const IA32_VMX_CR0_FIXED0: u32 = 0x486;
    /// CR0 bits fixed to 1.
    pub const IA32_VMX_CR0_FIXED1: u32 = 0x487;
    /// CR4 bits fixed to 0.
    pub const IA32_VMX_CR4_FIXED0: u32 = 0x488;
    /// CR4 bits fixed to 1.
    pub const IA32_VMX_CR4_FIXED1: u32 = 0x489;
    /// Secondary processor-based VM-execution controls.
    pub const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
    /// EPT and VPID capabilities.
    pub const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;

    // Power management MSRs

    /// Package C-State configuration control.
    pub const MSR_PKG_CST_CONFIG_CONTROL: u32 = 0xE2;
    /// Power control.
    pub const MSR_POWER_CTL: u32 = 0x1FC;
    /// Productive performance count.
    pub const MSR_PPERF: u32 = 0x64E;
    /// Overclocking mailbox (Turbo Max 3.0).
    pub const MSR_OC_MAILBOX: u32 = 0x150;
    /// SMI count.
    pub const MSR_SMI_COUNT: u32 = 0x34;
    /// Miscellaneous power management.
    pub const MSR_MISC_PWR_MGMT: u32 = 0x1AA;
    /// Power management enable (HWP).
    pub const IA32_PM_ENABLE: u32 = 0x770;
    /// HWP capabilities.
    pub const IA32_HWP_CAPABILITIES: u32 = 0x771;
    /// HWP interrupt control.
    pub const IA32_HWP_INTERRUPT: u32 = 0x773;
    /// HWP request.
    pub const IA32_HWP_REQUEST: u32 = 0x774;
    /// HWP status.
    pub const IA32_HWP_STATUS: u32 = 0x777;

    // Intel Processor Trace (Intel PT) MSRs
    // See Intel SDM Vol 4

    /// Intel PT control.
    pub const IA32_RTIT_CTL: u32 = 0x570;

    // RAPL (Running Average Power Limit) MSRs
    // See Intel SDM Vol 4, Section 16.10

    /// RAPL power unit multipliers (R/O). Reports units for power, energy, time.
    pub const MSR_RAPL_POWER_UNIT: u32 = 0x606;
    /// Package power limit control (R/W).
    pub const MSR_PKG_POWER_LIMIT: u32 = 0x610;
    /// Package energy status (R/O). Cumulative energy consumed.
    pub const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
    /// Package power info (R/O). TDP, min/max power.
    pub const MSR_PKG_POWER_INFO: u32 = 0x614;
    /// DRAM energy status (R/O).
    pub const MSR_DRAM_ENERGY_STATUS: u32 = 0x619;
    /// DRAM power limit control (R/W).
    pub const MSR_DRAM_POWER_LIMIT: u32 = 0x618;
    /// DRAM power info (R/O).
    pub const MSR_DRAM_POWER_INFO: u32 = 0x61C;
    /// PP0 (cores) power limit control (R/W).
    pub const MSR_PP0_POWER_LIMIT: u32 = 0x638;
    /// PP0 (cores) energy status (R/O).
    pub const MSR_PP0_ENERGY_STATUS: u32 = 0x639;
    /// PP1 (uncore/GPU) power limit control (R/W).
    pub const MSR_PP1_POWER_LIMIT: u32 = 0x640;
    /// PP1 (uncore/GPU) energy status (R/O).
    pub const MSR_PP1_ENERGY_STATUS: u32 = 0x641;
}

/// Error returned by MSR read/write operations.
/// See Intel SDM Vol 2B (RDMSR) and Vol 2D (WRMSR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsrError {
    /// The MSR address in ECX is reserved or unimplemented.
    /// Results in #GP(0) on hardware.
    InvalidAddress,
}

/// Result type for MSR operations.
pub type MsrResult<T> = Result<T, MsrError>;

/// Trait for reading and writing Model-Specific Registers (MSRs).
///
/// MSRs are accessed via the RDMSR and WRMSR instructions:
/// - RDMSR (opcode 0F 32): Reads the MSR specified in ECX into EDX:EAX
/// - WRMSR (opcode 0F 30): Writes EDX:EAX to the MSR specified in ECX
///
/// Both instructions require CPL 0 (ring 0) or real-address mode.
/// Invalid MSR addresses or reserved bit violations cause #GP(0).
///
/// See Intel SDM Vol 2B Section 4-535 (RDMSR) and Vol 2D Section 6-8 (WRMSR).
///
/// Implementors must ensure:
/// - Operations are only performed at CPL 0
/// - Invalid MSR addresses are handled appropriately
/// - Reserved bits are not modified
///
/// # Example Implementation
///
/// For direct hardware access (unsafe, requires ring 0):
/// ```ignore
/// unsafe fn rdmsr(address: u32) -> u64 {
///     let (low, high): (u32, u32);
///     core::arch::asm!(
///         "rdmsr",
///         in("ecx") address,
///         out("eax") low,
///         out("edx") high,
///         options(nomem, nostack)
///     );
///     ((high as u64) << 32) | (low as u64)
/// }
///
/// unsafe fn wrmsr(address: u32, value: u64) {
///     let low = value as u32;
///     let high = (value >> 32) as u32;
///     core::arch::asm!(
///         "wrmsr",
///         in("ecx") address,
///         in("eax") low,
///         in("edx") high,
///         options(nomem, nostack)
///     );
/// }
/// ```
pub trait MsrAccess {
    /// Read a 64-bit value from the MSR at the given address.
    ///
    /// Corresponds to the RDMSR instruction (opcode 0F 32).
    /// The MSR address is placed in ECX, and the result is returned in EDX:EAX.
    ///
    /// # Arguments
    ///
    /// * `address` - The MSR address (placed in ECX)
    ///
    /// # Returns
    ///
    /// The 64-bit MSR value (EDX:EAX), or an error if the operation fails.
    ///
    /// # Errors
    ///
    /// * `MsrError::InvalidAddress` - The MSR address is reserved or unimplemented
    /// * `MsrError::PrivilegeViolation` - Not executing at CPL 0
    fn read_msr(&self, address: u32) -> MsrResult<u64>;

    /// Write a 64-bit value to the MSR at the given address.
    ///
    /// Corresponds to the WRMSR instruction (opcode 0F 30).
    /// The MSR address is placed in ECX, and the value is provided in EDX:EAX.
    ///
    /// WRMSR is a serializing instruction (except for IA32_TSC_DEADLINE and X2APIC MSRs).
    /// Writing to MTRRs invalidates TLBs including global entries.
    ///
    /// # Arguments
    ///
    /// * `address` - The MSR address (placed in ECX)
    /// * `value` - The 64-bit value to write (EDX:EAX)
    ///
    /// # Errors
    ///
    /// * `MsrError::InvalidAddress` - The MSR address is reserved or unimplemented
    fn write_msr(&self, address: u32, value: u64) -> MsrResult<()>;
}
