// SPDX-License-Identifier: GPL-2.0

//! MSR read/write exit handlers.

use super::helpers::{advance_rip, ExitHandlerResult};
use super::reasons::ExitReason;

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Fake microcode version to report to guest (matches KVM's default).
const FAKE_MICROCODE_VERSION: u64 = 0x1_0000_0000;

/// Handle MSR read (RDMSR) exit.
pub fn handle_msr_read<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let msr_num = ctx.state().gprs.rcx as u32;

    let value: u64 = match msr_num {
        msr::IA32_MISC_ENABLE => {
            // Return guest-safe value derived from host:
            // - Sets BTS/PEBS unavailable (bits 11, 12)
            // - Clears MWAIT (bit 18) to match CPUID
            let host_val = ctx.state().host_state.misc_enable;
            MiscEnable::for_guest(host_val).bits()
        }
        msr::IA32_BIOS_SIGN_ID => {
            // Return fake microcode version (upper 32 bits).
            // Guest writes 0 then executes CPUID then reads this.
            FAKE_MICROCODE_VERSION
        }
        msr::IA32_MCG_CAP | msr::IA32_MCG_STATUS => {
            // Return 0 - no machine check support (bhyve approach)
            0
        }
        msr::IA32_TSC_ADJUST => {
            // Return 0 - we don't support TSC adjustment
            // KVM stores per-vCPU value, bhyve exits to userspace
            0
        }
        msr::IA32_TSC_DEADLINE => {
            // Return 0 - TSC-deadline mode not supported
            // (CPUID bit 24 is cleared, guest uses regular APIC timer instead)
            0
        }
        msr::IA32_MTRRCAP => {
            // MTRR capabilities (like bhyve):
            // - VCNT = 10 (10 variable range MTRRs)
            // - FIX = 1 (fixed range MTRRs supported, bit 8)
            // - WC = 1 (write-combining supported, bit 10)
            (1 << 10) | (1 << 8) | (MTRR_VAR_MAX as u64)
        }
        msr::IA32_MTRR_DEF_TYPE => ctx.state().devices.mtrr.def_type,
        msr::IA32_MTRR_PHYSBASE0..=msr::IA32_MTRR_PHYSMASK9 => {
            let offset = (msr_num - msr::IA32_MTRR_PHYSBASE0) as usize;
            let index = offset / 2;
            if index < MTRR_VAR_MAX {
                let (base, mask) = ctx.state().devices.mtrr.var[index];
                if offset % 2 == 0 {
                    base
                } else {
                    mask
                }
            } else {
                0
            }
        }
        msr::IA32_MTRR_FIX64K_00000 => ctx.state().devices.mtrr.fixed_64k,
        msr::IA32_MTRR_FIX16K_80000 | msr::IA32_MTRR_FIX16K_A0000 => {
            let index = (msr_num - msr::IA32_MTRR_FIX16K_80000) as usize;
            ctx.state().devices.mtrr.fixed_16k[index]
        }
        msr::IA32_MTRR_FIX4K_C0000..=msr::IA32_MTRR_FIX4K_F8000 => {
            let index = (msr_num - msr::IA32_MTRR_FIX4K_C0000) as usize;
            if index < 8 {
                ctx.state().devices.mtrr.fixed_4k[index]
            } else {
                0
            }
        }
        msr::IA32_PAT => ctx.state().msr_state.pat,
        msr::IA32_APIC_BASE => ctx.state().devices.apic.base,
        msr::IA32_TSC_AUX => ctx.state().msr_state.tsc_aux,
        msr::IA32_FEATURE_CONTROL => {
            // Return "locked, VMX not enabled" - we don't support nested virt.
            // Bit 0 = Lock, Bit 2 = VMX outside SMX enable.
            0x1 // Lock bit only
        }
        msr::IA32_PLATFORM_INFO => {
            // Return host platform info (bhyve approach).
            // Contains max non-turbo ratio in bits 15:8.
            ctx.state().host_state.platform_info
        }
        msr::IA32_THERM_INTERRUPT
        | msr::IA32_THERM_STATUS
        | msr::IA32_PACKAGE_THERM_STATUS
        | msr::IA32_PACKAGE_THERM_INTERRUPT => {
            // Return 0 - no thermal monitoring support.
            0
        }
        msr::IA32_PPIN_CTL => {
            // Return 0 - PPIN feature not available.
            0
        }
        msr::IA32_PERF_CAPABILITIES => {
            // Return 0 - no performance monitoring capabilities.
            0
        }
        msr::IA32_LBR_TOS => {
            // Return 0 - no LBR support.
            0
        }
        msr::IA32_OFFCORE_RSP_0 | msr::IA32_OFFCORE_RSP_1 => {
            // Return 0 - no off-core response PMU support.
            0
        }
        msr::IA32_PEBS_LD_LAT_THRESHOLD | msr::IA32_PEBS_FRONTEND => {
            // Return 0 - no PEBS support.
            0
        }
        msr::IA32_PERFEVTSEL0
        | msr::IA32_PERFEVTSEL1
        | msr::IA32_PERFEVTSEL2
        | msr::IA32_PERFEVTSEL3
        | msr::IA32_PERFEVTSEL4
        | msr::IA32_PERFEVTSEL5
        | msr::IA32_PERFEVTSEL6
        | msr::IA32_PERFEVTSEL7
        | msr::IA32_FIXED_CTR_CTRL
        | msr::IA32_PMC0
        | msr::IA32_PMC1
        | msr::IA32_PMC2
        | msr::IA32_PMC3
        | msr::IA32_PMC4
        | msr::IA32_PMC5
        | msr::IA32_PMC6
        | msr::IA32_PMC7
        | msr::IA32_MPERF
        | msr::IA32_APERF => {
            // Return 0 - no PMU support.
            0
        }
        msr::MSR_ATOM_CORE_RATIOS | msr::MSR_ATOM_CORE_VIDS | msr::MSR_ATOM_CORE_TURBO_RATIOS => {
            // Return 0 - model-specific MSRs not supported.
            0
        }
        msr::IA32_MISC_FEATURES_ENABLES => {
            // Return 0 - CPUID faulting not enabled (bhyve approach).
            0
        }
        msr::IA32_RTIT_CTL => {
            // Return 0 - Intel PT not supported (bhyve approach).
            0
        }
        msr::MSR_PKG_CST_CONFIG_CONTROL
        | msr::MSR_POWER_CTL
        | msr::MSR_PPERF
        | msr::MSR_MISC_PWR_MGMT
        | msr::IA32_PM_ENABLE
        | msr::IA32_HWP_CAPABILITIES
        | msr::IA32_HWP_INTERRUPT
        | msr::IA32_HWP_REQUEST
        | msr::IA32_HWP_STATUS
        | msr::IA32_PERF_STATUS
        | msr::IA32_PERF_CTL
        | msr::IA32_ENERGY_PERF_BIAS
        | msr::MSR_OC_MAILBOX => {
            // Return 0 - power management not supported.
            0
        }
        msr::MSR_SMI_COUNT => {
            // Return 0 - no SMM support.
            0
        }
        msr::MSR_AMD64_DE_CFG => {
            // Return 0 - AMD MSR probed by Linux on Intel.
            0
        }
        msr::IA32_MKTME_KEYID_PARTITIONING => {
            // Return 0 - no multi-key memory encryption support.
            0
        }
        // SYSCALL MSRs (STAR, LSTAR, CSTAR, FMASK) use passthrough - no VM exit.
        _ => {
            // Unknown MSR - exit to userspace
            if let Err(e) = advance_rip(ctx) {
                return ExitHandlerResult::Error(e);
            }
            return ExitHandlerResult::ExitToUserspace(ExitReason::MsrRead);
        }
    };

    // Write value to EAX:EDX (RDMSR result format)
    let gprs = &mut ctx.state_mut().gprs;
    gprs.rax = value & 0xFFFF_FFFF;
    gprs.rdx = value >> 32;

    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }
    ExitHandlerResult::Continue
}

/// Handle MSR write (WRMSR) exit.
pub fn handle_msr_write<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let msr_num = ctx.state().gprs.rcx as u32;
    // WRMSR value is in EDX:EAX
    let value = ((ctx.state().gprs.rdx & 0xFFFF_FFFF) << 32) | (ctx.state().gprs.rax & 0xFFFF_FFFF);

    match msr_num {
        msr::IA32_MISC_ENABLE => {
            // Ignore writes - guest can't change these features
        }
        msr::IA32_BIOS_SIGN_ID => {
            // Ignore writes - guest writes 0 before CPUID to trigger
            // microcode version update, but we just return a fake version
        }
        msr::IA32_MCG_CAP | msr::IA32_MCG_STATUS => {
            // Ignore writes - no MCE support
        }
        msr::IA32_TSC_ADJUST => {
            // Ignore writes - we don't support TSC adjustment
        }
        msr::IA32_TSC_DEADLINE => {
            // Ignore writes - TSC-deadline mode not supported
            // (CPUID bit 24 is cleared, guest uses regular APIC timer instead)
        }
        msr::IA32_MTRRCAP => {
            // MTRRcap is read-only, ignore writes (bhyve returns error, we just ignore)
        }
        msr::IA32_MTRR_DEF_TYPE => {
            // Store def_type (bhyve validates reserved bits, we just store)
            ctx.state_mut().devices.mtrr.def_type = value;
        }
        msr::IA32_MTRR_PHYSBASE0..=msr::IA32_MTRR_PHYSMASK9 => {
            let offset = (msr_num - msr::IA32_MTRR_PHYSBASE0) as usize;
            let index = offset / 2;
            if index < MTRR_VAR_MAX {
                if offset % 2 == 0 {
                    ctx.state_mut().devices.mtrr.var[index].0 = value; // base
                } else {
                    ctx.state_mut().devices.mtrr.var[index].1 = value; // mask
                }
            }
        }
        msr::IA32_MTRR_FIX64K_00000 => {
            ctx.state_mut().devices.mtrr.fixed_64k = value;
        }
        msr::IA32_MTRR_FIX16K_80000 | msr::IA32_MTRR_FIX16K_A0000 => {
            let index = (msr_num - msr::IA32_MTRR_FIX16K_80000) as usize;
            ctx.state_mut().devices.mtrr.fixed_16k[index] = value;
        }
        msr::IA32_MTRR_FIX4K_C0000..=msr::IA32_MTRR_FIX4K_F8000 => {
            let index = (msr_num - msr::IA32_MTRR_FIX4K_C0000) as usize;
            if index < 8 {
                ctx.state_mut().devices.mtrr.fixed_4k[index] = value;
            }
        }
        msr::IA32_PAT => {
            ctx.state_mut().msr_state.pat = value;
        }
        msr::IA32_APIC_BASE => {
            // Like bhyve: reject any attempt to change APIC_BASE.
            // Our APIC MMIO emulation is hardcoded to 0xFEE00000.
            if ctx.state().devices.apic.base != value {
                if let Err(e) = advance_rip(ctx) {
                    return ExitHandlerResult::Error(e);
                }
                return ExitHandlerResult::ExitToUserspace(ExitReason::MsrWrite);
            }
        }
        msr::IA32_TSC_AUX => {
            ctx.state_mut().msr_state.tsc_aux = value;
        }
        msr::IA32_FEATURE_CONTROL => {
            // Ignore writes - MSR is locked (bit 0 = 1).
            // Guest can't change VMX settings.
        }
        msr::IA32_PLATFORM_INFO => {
            // Ignore writes - MSR is read-only.
        }
        msr::IA32_THERM_INTERRUPT
        | msr::IA32_THERM_STATUS
        | msr::IA32_PACKAGE_THERM_STATUS
        | msr::IA32_PACKAGE_THERM_INTERRUPT => {
            // Ignore writes - no thermal monitoring support.
        }
        msr::IA32_PPIN_CTL
        | msr::IA32_PERF_CAPABILITIES
        | msr::IA32_LBR_TOS
        | msr::IA32_OFFCORE_RSP_0
        | msr::IA32_OFFCORE_RSP_1
        | msr::IA32_PEBS_LD_LAT_THRESHOLD
        | msr::IA32_PEBS_FRONTEND
        | msr::IA32_PERFEVTSEL0
        | msr::IA32_PERFEVTSEL1
        | msr::IA32_PERFEVTSEL2
        | msr::IA32_PERFEVTSEL3
        | msr::IA32_PERFEVTSEL4
        | msr::IA32_PERFEVTSEL5
        | msr::IA32_PERFEVTSEL6
        | msr::IA32_PERFEVTSEL7
        | msr::IA32_FIXED_CTR_CTRL
        | msr::IA32_PMC0
        | msr::IA32_PMC1
        | msr::IA32_PMC2
        | msr::IA32_PMC3
        | msr::IA32_PMC4
        | msr::IA32_PMC5
        | msr::IA32_PMC6
        | msr::IA32_PMC7
        | msr::IA32_MPERF
        | msr::IA32_APERF
        | msr::MSR_ATOM_CORE_RATIOS
        | msr::MSR_ATOM_CORE_VIDS
        | msr::MSR_ATOM_CORE_TURBO_RATIOS => {
            // Ignore writes - features not available.
        }
        msr::IA32_MISC_FEATURES_ENABLES => {
            // Ignore writes - we don't support CPUID faulting (bhyve approach).
        }
        msr::IA32_RTIT_CTL => {
            // Ignore writes - Intel PT not supported.
        }
        msr::MSR_PKG_CST_CONFIG_CONTROL
        | msr::MSR_POWER_CTL
        | msr::MSR_PPERF
        | msr::MSR_MISC_PWR_MGMT
        | msr::IA32_PM_ENABLE
        | msr::IA32_HWP_CAPABILITIES
        | msr::IA32_HWP_INTERRUPT
        | msr::IA32_HWP_REQUEST
        | msr::IA32_HWP_STATUS
        | msr::IA32_PERF_STATUS
        | msr::IA32_PERF_CTL
        | msr::IA32_ENERGY_PERF_BIAS
        | msr::MSR_OC_MAILBOX => {
            // Ignore writes - power management not supported.
        }
        // SYSCALL MSRs (STAR, LSTAR, CSTAR, FMASK) use passthrough - no VM exit.
        _ => {
            // Unknown MSR - exit to userspace
            if let Err(e) = advance_rip(ctx) {
                return ExitHandlerResult::Error(e);
            }
            return ExitHandlerResult::ExitToUserspace(ExitReason::MsrWrite);
        }
    }

    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }
    ExitHandlerResult::Continue
}
