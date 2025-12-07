// SPDX-License-Identifier: GPL-2.0

//! CPUID exit handler.

use super::helpers::{advance_rip, ExitHandlerResult};

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Handle CPUID exit.
///
/// Emulates CPUID by executing on the host and filtering results.
pub fn handle_cpuid<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let gprs = ctx.state().gprs;
    let leaf = gprs.rax as u32;
    let subleaf = gprs.rcx as u32;

    // Execute CPUID on host
    let (mut eax, mut ebx, mut ecx, mut edx) = cpuid(leaf, subleaf);

    // Filter/modify results based on leaf
    match leaf {
        0x0 => {
            // Vendor ID - pass through "GenuineIntel" from host
            // EAX = max supported basic leaf, limit to what we handle
            // This is deterministic as long as we run on same CPU vendor
            if eax > 0x16 {
                eax = 0x16; // Cap at highest leaf we handle
            }
            // EBX:EDX:ECX contains "GenuineIntel" - pass through
        }
        0x1 => {
            // Basic CPUID information
            // Set fixed processor signature: Family 6, Model 85 (Skylake-SP), Stepping 7
            // EAX format: [31:28]=ext_family, [27:20]=ext_model, [19:16]=type,
            //             [15:14]=reserved, [13:12]=type, [11:8]=family, [7:4]=model, [3:0]=stepping
            // For Family 6, Model 85: ext_model=5, model=5, family=6, stepping=7
            eax = 0x00050657;

            // Clear VMX (bit 5 of ECX) - guest shouldn't see VMX capability
            ecx &= !(1 << 5);

            // Clear TSC_DEADLINE_TIMER (bit 24 of ECX) - not supported
            // Guest will fall back to regular one-shot APIC timer (like bhyve)
            ecx &= !(1 << 24);

            // Set hypervisor present bit (bit 31 of ECX) - indicate we're running in a VM
            ecx |= 1 << 31;

            // Keep XSAVE/OSXSAVE/AVX if host supports (don't clear bits 26, 27, 28)
            // These will be passed through from host CPUID result

            // Keep APIC bit (don't clear bit 9 of EDX) - we handle APIC

            // Clear HTT (bit 28 of EDX) - no hyper-threading, single logical processor
            edx &= !(1 << 28);

            // Set virtual APIC ID in EBX[31:24] to 0 and logical processor count in EBX[23:16] to 1
            ebx = (ebx & 0x0000FFFF) | 0x00010000;
        }
        0x4 => {
            // Deterministic cache parameters
            if subleaf == 0 {
                // Report single core
                eax &= !0x3FFC000;
            }
        }
        0x6 => {
            // Thermal and Power Management (CPUID.06H)
            // Return 0 - no thermal monitoring or power management features
            // This prevents non-determinism from host thermal state changing between runs
            eax = 0;
            ebx = 0;
            ecx = 0;
            edx = 0;
        }
        0x7 => {
            // Structured extended feature flags
            if subleaf == 0 {
                // EBX: Keep features if host supports: fsgsbase(0), bmi1(3), avx2(5),
                // bmi2(8), erms(9)
                // Clear features not in our supported set:
                ebx &= !(1 << 2); // SGX - not supported
                ebx &= !(1 << 4); // HLE (TSX) - not supported
                ebx &= !(1 << 7); // SMEP - requires CR4.SMEP handling
                                  // Bit 10 (INVPCID) passed through - enabled via ENABLE_INVPCID control
                ebx &= !(1 << 11); // RTM (TSX) - not supported
                ebx &= !(1 << 16); // AVX512F - not supported (requires extended XSAVE)
                ebx &= !(1 << 17); // AVX512DQ - not supported
                ebx &= !(1 << 18); // RDSEED - not supported
                ebx &= !(1 << 19); // ADX - not supported
                ebx &= !(1 << 20); // SMAP - requires CR4.SMAP handling
                ebx &= !(1 << 21); // AVX512_IFMA - not supported
                ebx &= !(1 << 26); // AVX512PF - not supported
                ebx &= !(1 << 27); // AVX512ER - not supported
                ebx &= !(1 << 28); // AVX512CD - not supported
                ebx &= !(1 << 30); // AVX512BW - not supported
                ebx &= !(1 << 31); // AVX512VL - not supported

                // ECX: Clear all - no supported features in this register
                ecx = 0;

                // EDX: Clear all - mostly speculation control features
                edx = 0;
            } else {
                // Subleaves 1+: Clear all - newer extensions not supported
                eax = 0;
                ebx = 0;
                ecx = 0;
                edx = 0;
            }
        }
        0xD => {
            // XSAVE state enumeration
            // We only support x87 + SSE + AVX (XCR0 = 0x7)
            // Must filter to match what hypervisor actually virtualizes
            const SUPPORTED_XCR0: u32 = 0x7; // x87 | SSE | AVX
            const XSAVE_SIZE: u32 = 832; // 512 (legacy) + 64 (header) + 256 (AVX)

            match subleaf {
                0 => {
                    // Subleaf 0: XCR0 supported bits and sizes
                    eax = SUPPORTED_XCR0;
                    ebx = XSAVE_SIZE; // Size for current XCR0
                    ecx = XSAVE_SIZE; // Max size for all supported
                    edx = 0; // High 32 bits of XCR0
                }
                1 => {
                    // Subleaf 1: XSAVES features - we don't support XSAVES
                    eax = 0;
                    ebx = 0;
                    ecx = 0;
                    edx = 0;
                }
                2 => {
                    // Subleaf 2: AVX state (YMM_Hi128)
                    eax = 256; // Size
                    ebx = 576; // Offset (after legacy + header)
                    ecx = 0; // Flags: in XCR0, not XSS
                    edx = 0;
                }
                _ => {
                    // Subleafs >= 3: unsupported features
                    eax = 0;
                    ebx = 0;
                    ecx = 0;
                    edx = 0;
                }
            }
        }
        0xB | 0x1F => {
            // Extended topology enumeration (0xB) and V2 Extended Topology (0x1F)
            // EDX returns the x2APIC ID (must match CPUID.01H EBX[31:24] per SDM 12.12.8.1)
            // These leaves enumerate CPU topology which must be deterministic
            edx = 0; // Virtual APIC ID = 0
            if subleaf == 0 || subleaf == 1 {
                ebx = 0; // No logical processors at this level
                ecx = (ecx & 0xFFFFFF00) | subleaf;
            } else {
                // Higher subleaves: report no more levels
                eax = 0;
                ebx = 0;
                ecx = subleaf;
                edx = 0;
            }
        }
        0x80000000 => {
            // Extended function info
            if eax < 0x80000004 {
                eax = 0x80000004; // Support brand string
            }
        }
        0x80000002..=0x80000004 => {
            // Processor brand string
            let brand = b"Bedrock VM CPU  ";
            let brand_dwords: &[u32; 4] = unsafe { core::mem::transmute(brand) };
            if leaf == 0x80000002 {
                eax = brand_dwords[0];
                ebx = brand_dwords[1];
                ecx = brand_dwords[2];
                edx = brand_dwords[3];
            } else {
                eax = 0x20202020; // Spaces
                ebx = 0x20202020;
                ecx = 0x20202020;
                edx = 0x00000000;
            }
        }
        0xA => {
            // Architectural Performance Monitoring (CPUID.0AH)
            // Report no PMU support - prevents guest from using RDPMC
            // SDM Vol 1, Table 21-30: EAX[7:0]=0 means no perf monitoring
            eax = 0;
            ebx = 0;
            ecx = 0;
            edx = 0;
        }
        0x15 => {
            // Time Stamp Counter and Nominal Core Crystal Clock (CPUID.15H)
            // Return values consistent with configured TSC frequency
            // Simple approach: report TSC frequency directly in ECX (Hz)
            // EAX = denominator, EBX = numerator, ECX = crystal clock frequency
            // TSC frequency = ECX * EBX / EAX
            // For simplicity: EAX=1, EBX=1, ECX=tsc_frequency
            let tsc_freq = ctx.state().tsc_frequency;
            eax = 1;
            ebx = 1;
            ecx = (tsc_freq & 0xFFFFFFFF) as u32;
            edx = 0;
        }
        0x80000008 => {
            // Virtual/physical address sizes
            ecx &= 0xFFFFFF00; // Report single core
        }
        0x16 => {
            // Processor Frequency Information (CPUID.16H)
            // Return fixed frequencies for determinism
            // These values can change based on CPU power state on real hardware
            // EAX = Base Frequency (MHz), EBX = Max Frequency (MHz)
            // ECX = Bus Frequency (MHz), EDX = Reserved
            eax = 3000; // 3.0 GHz base frequency
            ebx = 5800; // 5.8 GHz max frequency
            ecx = 100; // 100 MHz bus frequency
            edx = 0;
        }
        _ => {
            // Unhandled leaves return 0 for determinism
            // This ensures guest doesn't see host-specific values that could vary
            eax = 0;
            ebx = 0;
            ecx = 0;
            edx = 0;
        }
    }

    // Write results back
    {
        let gprs = &mut ctx.state_mut().gprs;
        gprs.rax = eax as u64;
        gprs.rbx = ebx as u64;
        gprs.rcx = ecx as u64;
        gprs.rdx = edx as u64;
    }

    // Advance RIP
    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }

    ExitHandlerResult::Continue
}

/// Execute CPUID instruction.
#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            ebx_out = out(reg) ebx,
            lateout("edx") edx,
            options(nostack),
        );
    }
    (eax, ebx, ecx, edx)
}

/// Mock CPUID for non-x86_64 targets (for testing).
#[cfg(not(target_arch = "x86_64"))]
fn cpuid(_leaf: u32, _subleaf: u32) -> (u32, u32, u32, u32) {
    (0, 0, 0, 0)
}
