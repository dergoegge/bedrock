// SPDX-License-Identifier: GPL-2.0

//! VMCS field encodings organized by width.
//!
//! Field encodings from Intel SDM Volume 3A, Appendix B.
//!
//! Encoding structure (bits):
//! - 14:13 = Width: 0=16-bit, 1=64-bit, 2=32-bit, 3=natural-width
//! - 11:10 = Type: 0=control, 1=VM-exit info (read-only), 2=guest-state, 3=host-state
//! - 9:1 = Index
//! - 0 = Access type (0=full, 1=high for 64-bit fields)

/// 16-bit VMCS fields (encoding pattern: 0000_xxxx_xxxx_xxx0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmcsField16 {
    // 16-bit control fields (type 0)
    /// Virtual-processor identifier (VPID).
    VirtualProcessorId = 0x0000,

    /// Page-modification log index.
    PmlIndex = 0x0812,

    // 16-bit guest-state fields (type 2)
    /// Guest ES selector.
    GuestEsSelector = 0x0800,
    /// Guest CS selector.
    GuestCsSelector = 0x0802,
    /// Guest SS selector.
    GuestSsSelector = 0x0804,
    /// Guest DS selector.
    GuestDsSelector = 0x0806,
    /// Guest FS selector.
    GuestFsSelector = 0x0808,
    /// Guest GS selector.
    GuestGsSelector = 0x080A,
    /// Guest LDTR selector.
    GuestLdtrSelector = 0x080C,
    /// Guest TR selector.
    GuestTrSelector = 0x080E,

    // 16-bit host-state fields (type 3)
    /// Host ES selector.
    HostEsSelector = 0x0C00,
    /// Host CS selector.
    HostCsSelector = 0x0C02,
    /// Host SS selector.
    HostSsSelector = 0x0C04,
    /// Host DS selector.
    HostDsSelector = 0x0C06,
    /// Host FS selector.
    HostFsSelector = 0x0C08,
    /// Host GS selector.
    HostGsSelector = 0x0C0A,
    /// Host TR selector.
    HostTrSelector = 0x0C0C,
}

/// 64-bit VMCS fields (encoding pattern: 0010_xxxx_xxxx_xxxA).
///
/// Note: For 64-bit fields, bit 0 indicates full (0) or high (1) access.
/// These encodings are for full access. Add 1 for high 32-bits only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmcsField64 {
    // 64-bit control fields (type 0)
    /// Address of MSR bitmaps.
    MsrBitmapAddr = 0x2004,
    /// VM-exit MSR-store list address (MSRs saved into memory on VM exit).
    VmExitMsrStoreAddr = 0x2006,
    /// VM-entry MSR-load list address (MSRs loaded from memory on VM entry).
    VmEntryMsrLoadAddr = 0x200A,
    /// Page-modification log address.
    PmlAddress = 0x200E,
    /// EPT pointer (EPTP).
    EptPointer = 0x201A,

    // 64-bit read-only data fields (type 1)
    /// Guest-physical address.
    GuestPhysicalAddr = 0x2400,

    // 64-bit guest-state fields (type 2)
    /// VMCS link pointer.
    VmcsLinkPointer = 0x2800,
    /// Guest IA32_DEBUGCTL.
    GuestIa32Debugctl = 0x2802,
    /// Guest IA32_PAT.
    GuestIa32Pat = 0x2804,
    /// Guest IA32_EFER.
    GuestIa32Efer = 0x2806,
    /// Guest IA32_PERF_GLOBAL_CTRL (for hardware perf counter switching).
    GuestIa32PerfGlobalCtrl = 0x2808,

    // 64-bit host-state fields (type 3)
    /// Host IA32_PAT.
    HostIa32Pat = 0x2C00,
    /// Host IA32_EFER.
    HostIa32Efer = 0x2C02,
    /// Host IA32_PERF_GLOBAL_CTRL (for hardware perf counter switching).
    HostIa32PerfGlobalCtrl = 0x2C04,
}

/// 32-bit VMCS fields (encoding pattern: 0100_xxxx_xxxx_xxx0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmcsField32 {
    // 32-bit control fields (type 0)
    /// Pin-based VM-execution controls.
    PinBasedVmExecControls = 0x4000,
    /// Primary processor-based VM-execution controls.
    PrimaryProcBasedVmExecControls = 0x4002,
    /// Exception bitmap.
    ExceptionBitmap = 0x4004,
    /// Page-fault error-code mask.
    PageFaultErrorCodeMask = 0x4006,
    /// Page-fault error-code match.
    PageFaultErrorCodeMatch = 0x4008,
    /// CR3-target count.
    Cr3TargetCount = 0x400A,
    /// Primary VM-exit controls.
    PrimaryVmExitControls = 0x400C,
    /// VM-exit MSR-store list count.
    VmExitMsrStoreCount = 0x400E,
    /// VM-entry controls.
    VmEntryControls = 0x4012,
    /// VM-entry MSR-load list count.
    VmEntryMsrLoadCount = 0x4014,
    /// VM-entry interruption-information field.
    VmEntryInterruptionInfo = 0x4016,
    /// VM-entry exception error code.
    VmEntryExceptionErrorCode = 0x4018,
    /// Secondary processor-based VM-execution controls.
    SecondaryProcBasedVmExecControls = 0x401E,

    // 32-bit read-only data fields (type 1)
    /// VM-instruction error.
    VmInstructionError = 0x4400,
    /// Exit reason.
    VmExitReason = 0x4402,
    /// VM-exit interruption information.
    VmExitInterruptionInfo = 0x4404,
    /// VM-exit interruption error code.
    VmExitInterruptionErrorCode = 0x4406,
    /// IDT-vectoring information field.
    IdtVectoringInfo = 0x4408,
    /// IDT-vectoring error code.
    IdtVectoringErrorCode = 0x440A,
    /// VM-exit instruction length.
    VmExitInstructionLen = 0x440C,
    /// VM-exit instruction information (for certain instructions like RDRAND).
    VmExitInstructionInfo = 0x440E,

    // 32-bit guest-state fields (type 2)
    /// Guest ES limit.
    GuestEsLimit = 0x4800,
    /// Guest CS limit.
    GuestCsLimit = 0x4802,
    /// Guest SS limit.
    GuestSsLimit = 0x4804,
    /// Guest DS limit.
    GuestDsLimit = 0x4806,
    /// Guest FS limit.
    GuestFsLimit = 0x4808,
    /// Guest GS limit.
    GuestGsLimit = 0x480A,
    /// Guest LDTR limit.
    GuestLdtrLimit = 0x480C,
    /// Guest TR limit.
    GuestTrLimit = 0x480E,
    /// Guest GDTR limit.
    GuestGdtrLimit = 0x4810,
    /// Guest IDTR limit.
    GuestIdtrLimit = 0x4812,
    /// Guest ES access rights.
    GuestEsAccessRights = 0x4814,
    /// Guest CS access rights.
    GuestCsAccessRights = 0x4816,
    /// Guest SS access rights.
    GuestSsAccessRights = 0x4818,
    /// Guest DS access rights.
    GuestDsAccessRights = 0x481A,
    /// Guest FS access rights.
    GuestFsAccessRights = 0x481C,
    /// Guest GS access rights.
    GuestGsAccessRights = 0x481E,
    /// Guest LDTR access rights.
    GuestLdtrAccessRights = 0x4820,
    /// Guest TR access rights.
    GuestTrAccessRights = 0x4822,
    /// Guest interruptibility state.
    GuestInterruptibilityState = 0x4824,
    /// Guest activity state.
    GuestActivityState = 0x4826,
    /// Guest IA32_SYSENTER_CS.
    GuestIa32SysenterCs = 0x482A,
    /// VMX-preemption timer value.
    VmxPreemptionTimerValue = 0x482E,

    // 32-bit host-state fields (type 3)
    /// Host IA32_SYSENTER_CS.
    HostIa32SysenterCs = 0x4C00,
}

/// Natural-width VMCS fields (encoding pattern: 0110_xxxx_xxxx_xxx0).
///
/// Natural-width fields are 64-bit on processors that support Intel 64
/// architecture and 32-bit on processors that do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmcsFieldNatural {
    // Natural-width control fields (type 0)
    /// CR0 guest/host mask.
    Cr0GuestHostMask = 0x6000,
    /// CR4 guest/host mask.
    Cr4GuestHostMask = 0x6002,
    /// CR0 read shadow.
    Cr0ReadShadow = 0x6004,
    /// CR4 read shadow.
    Cr4ReadShadow = 0x6006,

    // Natural-width read-only data fields (type 1)
    /// Exit qualification.
    ExitQualification = 0x6400,
    /// Guest-linear address.
    GuestLinearAddr = 0x640A,

    // Natural-width guest-state fields (type 2)
    /// Guest CR0.
    GuestCr0 = 0x6800,
    /// Guest CR3.
    GuestCr3 = 0x6802,
    /// Guest CR4.
    GuestCr4 = 0x6804,
    /// Guest ES base.
    GuestEsBase = 0x6806,
    /// Guest CS base.
    GuestCsBase = 0x6808,
    /// Guest SS base.
    GuestSsBase = 0x680A,
    /// Guest DS base.
    GuestDsBase = 0x680C,
    /// Guest FS base.
    GuestFsBase = 0x680E,
    /// Guest GS base.
    GuestGsBase = 0x6810,
    /// Guest LDTR base.
    GuestLdtrBase = 0x6812,
    /// Guest TR base.
    GuestTrBase = 0x6814,
    /// Guest GDTR base.
    GuestGdtrBase = 0x6816,
    /// Guest IDTR base.
    GuestIdtrBase = 0x6818,
    /// Guest DR7.
    GuestDr7 = 0x681A,
    /// Guest RSP.
    GuestRsp = 0x681C,
    /// Guest RIP.
    GuestRip = 0x681E,
    /// Guest RFLAGS.
    GuestRflags = 0x6820,
    /// Guest pending debug exceptions.
    GuestPendingDebugExceptions = 0x6822,
    /// Guest IA32_SYSENTER_ESP.
    GuestIa32SysenterEsp = 0x6824,
    /// Guest IA32_SYSENTER_EIP.
    GuestIa32SysenterEip = 0x6826,

    // Natural-width host-state fields (type 3)
    /// Host CR0.
    HostCr0 = 0x6C00,
    /// Host CR3.
    HostCr3 = 0x6C02,
    /// Host CR4.
    HostCr4 = 0x6C04,
    /// Host FS base.
    HostFsBase = 0x6C06,
    /// Host GS base.
    HostGsBase = 0x6C08,
    /// Host TR base.
    HostTrBase = 0x6C0A,
    /// Host GDTR base.
    HostGdtrBase = 0x6C0C,
    /// Host IDTR base.
    HostIdtrBase = 0x6C0E,
    /// Host IA32_SYSENTER_ESP.
    HostIa32SysenterEsp = 0x6C10,
    /// Host IA32_SYSENTER_EIP.
    HostIa32SysenterEip = 0x6C12,
    /// Host RSP.
    HostRsp = 0x6C14,
    /// Host RIP.
    HostRip = 0x6C16,
}
