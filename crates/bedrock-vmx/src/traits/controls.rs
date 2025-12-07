// =============================================================================
// VMX Control Bit Definitions
// See Intel SDM Vol 3C, Appendix A
// =============================================================================

/// Pin-based VM-execution controls.
/// See Intel SDM Vol 3C, Section 25.6.1.
pub mod pin_based {
    /// External-interrupt exiting.
    pub const EXT_INTR_EXITING: u32 = 1 << 0;
    /// NMI exiting.
    pub const NMI_EXITING: u32 = 1 << 3;
    /// Activate VMX-preemption timer.
    pub const PREEMPTION_TIMER: u32 = 1 << 6;
}

/// Primary processor-based VM-execution controls.
/// See Intel SDM Vol 3C, Section 25.6.2.
pub mod cpu_based {
    /// Interrupt-window exiting.
    pub const INTR_WINDOW_EXITING: u32 = 1 << 2;
    /// HLT exiting.
    pub const HLT_EXITING: u32 = 1 << 7;
    /// MWAIT exiting.
    pub const MWAIT_EXITING: u32 = 1 << 10;
    /// RDPMC exiting.
    pub const RDPMC_EXITING: u32 = 1 << 11;
    /// RDTSC exiting - causes VM exits on RDTSC and RDTSCP instructions.
    pub const RDTSC_EXITING: u32 = 1 << 12;
    /// CR3-load exiting.
    pub const CR3_LOAD_EXITING: u32 = 1 << 15;
    /// CR3-store exiting.
    pub const CR3_STORE_EXITING: u32 = 1 << 16;
    /// CR8-load exiting.
    pub const CR8_LOAD_EXITING: u32 = 1 << 19;
    /// CR8-store exiting.
    pub const CR8_STORE_EXITING: u32 = 1 << 20;
    /// Unconditional I/O exiting.
    pub const UNCOND_IO_EXITING: u32 = 1 << 24;
    /// Monitor trap flag - causes VM exit after each guest instruction.
    pub const MONITOR_TRAP_FLAG: u32 = 1 << 27;
    /// Use MSR bitmaps.
    pub const USE_MSR_BITMAPS: u32 = 1 << 28;
    /// MONITOR exiting - causes VM exit on MONITOR instruction for deterministic behavior.
    pub const MONITOR_EXITING: u32 = 1 << 29;
    /// Activate secondary controls.
    pub const ACTIVATE_SECONDARY_CONTROLS: u32 = 1 << 31;
}

/// Secondary processor-based VM-execution controls.
/// See Intel SDM Vol 3C, Section 25.6.2.
pub mod secondary_exec {
    /// Enable EPT.
    pub const ENABLE_EPT: u32 = 1 << 1;
    /// Enable RDTSCP.
    pub const ENABLE_RDTSCP: u32 = 1 << 3;
    /// Enable VPID.
    pub const ENABLE_VPID: u32 = 1 << 5;
    /// Unrestricted guest.
    pub const UNRESTRICTED_GUEST: u32 = 1 << 7;
    /// RDRAND exiting - causes VM exit on RDRAND instruction.
    pub const RDRAND_EXITING: u32 = 1 << 11;
    /// Enable INVPCID.
    pub const ENABLE_INVPCID: u32 = 1 << 12;
    /// RDSEED exiting - causes VM exit on RDSEED instruction.
    pub const RDSEED_EXITING: u32 = 1 << 16;
}

/// VM-exit controls.
/// See Intel SDM Vol 3C, Section 25.7.1.
pub mod vm_exit {
    /// Host address-space size (64-bit host).
    pub const HOST_ADDR_SPACE_SIZE: u32 = 1 << 9;
    /// Load IA32_PERF_GLOBAL_CTRL on VM exit (for hardware perf counter switching).
    pub const LOAD_IA32_PERF_GLOBAL_CTRL: u32 = 1 << 12;
    /// Save IA32_EFER.
    pub const SAVE_IA32_EFER: u32 = 1 << 20;
    /// Load IA32_EFER.
    pub const LOAD_IA32_EFER: u32 = 1 << 21;
}

/// VM-entry controls.
/// See Intel SDM Vol 3C, Section 25.8.1.
pub mod vm_entry {
    /// IA-32e mode guest.
    pub const IA32E_MODE: u32 = 1 << 9;
    /// Load IA32_PERF_GLOBAL_CTRL on VM entry (for hardware perf counter switching).
    pub const LOAD_IA32_PERF_GLOBAL_CTRL: u32 = 1 << 13;
    /// Load IA32_EFER.
    pub const LOAD_IA32_EFER: u32 = 1 << 15;
}
