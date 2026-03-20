// SPDX-License-Identifier: GPL-2.0

//! Time-related VM exit handlers (RDTSC, RDTSCP, RDPMC, MWAIT, HLT).
//!
//! These handlers provide deterministic time emulation by intercepting
//! time-related instructions and returning controlled values.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::helpers::{advance_rip, ExitHandlerResult};

/// Handle RDTSC VM exit.
///
/// Returns the emulated TSC value in EDX:EAX and advances RIP.
/// The TSC value is derived from instruction count for determinism.
pub fn handle_rdtsc<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let tsc = ctx.state().emulated_tsc;

    // RDTSC returns TSC in EDX:EAX
    let gprs = &mut ctx.state_mut().gprs;
    gprs.rax = tsc & 0xFFFF_FFFF;
    gprs.rdx = tsc >> 32;

    // Advance past RDTSC instruction (2 bytes: 0x0F 0x31)
    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }

    ExitHandlerResult::Continue
}

/// Handle RDTSCP VM exit.
///
/// Returns the emulated TSC value in EDX:EAX, and TSC_AUX in ECX.
/// Then advances RIP.
pub fn handle_rdtscp<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let tsc = ctx.state().emulated_tsc;
    let tsc_aux = ctx.state().msr_state.tsc_aux;

    // RDTSCP returns TSC in EDX:EAX and TSC_AUX in ECX
    let gprs = &mut ctx.state_mut().gprs;
    gprs.rax = tsc & 0xFFFF_FFFF;
    gprs.rdx = tsc >> 32;
    gprs.rcx = tsc_aux & 0xFFFF_FFFF; // TSC_AUX is 32-bit

    // Advance past RDTSCP instruction (3 bytes: 0x0F 0x01 0xF9)
    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }

    ExitHandlerResult::Continue
}

/// Handle RDPMC VM exit.
///
/// Since we report no PMU support in CPUID.0AH, RDPMC should inject #GP(0).
/// However, for simplicity we just return 0 and continue.
pub fn handle_rdpmc<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    // Return 0 for all performance counters
    let gprs = &mut ctx.state_mut().gprs;
    gprs.rax = 0;
    gprs.rdx = 0;

    // Advance past RDPMC instruction (2 bytes: 0x0F 0x33)
    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }

    ExitHandlerResult::Continue
}

/// Handle HLT/MWAIT VM exit.
///
/// Both are idle instructions that wait for an interrupt. For deterministic
/// execution, we advance the TSC offset so emulated_tsc reaches the APIC timer
/// deadline, causing the timer to fire on the next VM entry.
pub fn handle_idle<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let current_tsc = ctx.state().emulated_tsc;
    let timer_deadline = ctx.state().devices.apic.timer_deadline;

    // If timer is armed and deadline is in the future, advance TSC offset
    if timer_deadline > 0 && timer_deadline > current_tsc {
        let delta = timer_deadline - current_tsc;
        ctx.state_mut().tsc_offset += delta;
        ctx.state_mut().emulated_tsc = timer_deadline;
    }

    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }

    // Continue execution - timer interrupt will be injected on next VM entry
    ExitHandlerResult::Continue
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
