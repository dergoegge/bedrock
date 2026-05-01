// SPDX-License-Identifier: GPL-2.0

//! VM Exit handling for Intel VMX.
//!
//! This module provides abstractions for handling VM exits in a testable manner.
//! The key abstraction is the `VmContext` trait which allows mocking the VMCS
//! and guest state for unit testing.
//!
//! # Module Organization
//!
//! - `reasons`: Exit reason enum and parsing
//! - `qualifications`: Exit qualification types (CR access, I/O, EPT, interrupts)
//! - `helpers`: Error types and shared helper functions
//! - `cpuid`: CPUID exit handler
//! - `msr`: MSR read/write handlers
//! - `cr`: Control register access handler
//! - `io`: I/O port instruction handler
//! - `ept`: EPT violation handler and GVA translation
//! - `apic`: Local APIC and I/O APIC MMIO emulation
//! - `interrupts`: Interrupt injection and APIC timer handling
//! - `misc`: Exception handlers, XSETBV, triple fault debugging

mod apic;
mod cpuid;
mod cr;
mod ept;
mod helpers;
mod interrupts;
mod io;
mod misc;
mod msr;
mod qualifications;
mod rdrand;
mod reasons;
mod time;
mod vmcall;

// Re-export public types
pub use apic::{APIC_BASE, APIC_SIZE, IOAPIC_BASE, IOAPIC_SIZE};
pub use helpers::{ExitError, ExitHandlerResult};
pub use interrupts::{inject_pending_interrupt, reinject_vectored_event};
pub use qualifications::{
    CrAccessQualification, EptViolationQualification, IoQualification, RdrandInstructionInfo,
    RdrandOperandSize,
};
pub use reasons::ExitReason;

// Internal imports for handle_exit
use cpuid::handle_cpuid;
use cr::handle_cr_access;
use ept::handle_ept_violation;
use helpers::{advance_rip, read_exit_qualification, read_exit_reason, ExitError as EE};
use interrupts::{disable_interrupt_window_exiting, handle_external_interrupt};
use io::handle_io;
use misc::{dump_triple_fault_state, handle_exception_nmi, handle_xsetbv};
use msr::{handle_msr_read, handle_msr_write};
use rdrand::{handle_rdrand, handle_rdseed};
use time::{handle_idle, handle_rdpmc, handle_rdtsc, handle_rdtscp};
use vmcall::handle_vmcall;

#[cfg(not(feature = "cargo"))]
use super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Compute the instruction count of the next pending APIC timer expiry.
///
/// Returns `None` if the timer is disarmed, the APIC is disabled, or the
/// LVT timer is masked. Returns the absolute retired-instruction count at
/// which the timer should fire (= `timer_deadline - tsc_offset`) so the
/// hypervisor can land MTF exactly there for precise interrupt injection.
fn next_timer_exit_count<C: VmContext>(ctx: &C) -> Option<u64> {
    let state = ctx.state();
    let apic = &state.devices.apic;
    if apic.timer_deadline == 0 {
        return None;
    }
    if (apic.svr & (1 << 8)) == 0 {
        return None; // APIC software disable
    }
    if (apic.lvt_timer & (1 << 16)) != 0 {
        return None; // Timer masked
    }
    Some(apic.timer_deadline.saturating_sub(state.tsc_offset))
}

/// Compute the instruction count at which the configured `stop_at_tsc`
/// threshold is reached (= `stop_at_tsc - tsc_offset`).
///
/// Returns `None` when no stop is configured. Used so MTF can land
/// precisely on the stop point — the existing `stop_at_tsc` check in
/// `handle_exit` then fires `StopTscReached` at exactly the configured TSC
/// rather than at whatever natural exit happens past it.
fn next_stop_exit_count<C: VmContext>(ctx: &C) -> Option<u64> {
    let state = ctx.state();
    let stop_tsc = state.stop_at_tsc?;
    Some(stop_tsc.saturating_sub(state.tsc_offset))
}

/// Update MTF (Monitor Trap Flag) state and the sampling-counter alignment.
///
/// Enables MTF (which causes a VM-exit after every guest instruction) when
/// either:
///
/// 1. Single-stepping is configured and the current TSC is within the range, or
/// 2. The retired-instruction count is within `PERIODIC_EXIT_MARGIN` of the
///    `next_periodic_exit_count` boundary. The PMU is configured to overflow
///    `MARGIN` instructions before that boundary, so once the PMI fires we
///    step the guest one instruction at a time until we land exactly on it.
///
/// `next_periodic_exit_count` is the closest of:
///
/// - The next pending APIC timer deadline (converted to instruction count).
///   This makes timer interrupts arrive at exactly the right instruction
///   instead of being delayed until the next natural exit.
/// - The configured `stop_at_tsc` threshold, so the VM stops on exactly the
///   requested TSC instead of at the next natural exit past it.
///
/// Whenever the chosen target changes (timer deadline configured/expired/
/// advanced by MWAIT, stop threshold set, etc.), the sampling counter is
/// re-armed so its next overflow fires `target - count - MARGIN` events
/// from now — putting the PMI at exactly `target - MARGIN`.
pub fn update_mtf_state<C: VmContext>(ctx: &mut C) -> Result<(), ExitError> {
    // Use last_instruction_count + tsc_offset directly so this is correct for
    // both deterministic exits (where emulated_tsc was already set to this)
    // and non-deterministic exits (where emulated_tsc is stale).
    let count = ctx.state().last_instruction_count;
    let tsc = count + ctx.state().tsc_offset;
    let range = ctx.state().single_step_tsc_range;
    let currently_enabled = ctx.state().mtf_enabled;

    let should_single_step = match range {
        Some((start, end)) => tsc >= start && tsc < end,
        None => false,
    };

    // Pick the next forced-exit instruction count: closest of the pending
    // APIC timer deadline and the configured stop_at_tsc threshold.
    // u64::MAX disables forced exits when neither source is active.
    let mut new_next = u64::MAX;
    if let Some(timer_count) = next_timer_exit_count(ctx) {
        if timer_count > count {
            new_next = new_next.min(timer_count);
        }
    }
    if let Some(stop_count) = next_stop_exit_count(ctx) {
        if stop_count > count {
            new_next = new_next.min(stop_count);
        }
    }

    // If the target moved, install it and re-arm the sampling counter so the
    // PMI fires `MARGIN` instructions before it — regardless of where the
    // sampling counter happens to be in its current period. Without this
    // realign, PMIs at a fixed sample_period drift relative to either source
    // (each periodic PMI drifts `MARGIN` earlier per cycle; an APIC timer
    // can be configured at any future deadline mid-period).
    let prev_next = ctx.state().next_periodic_exit_count;
    if new_next != prev_next {
        ctx.state_mut().next_periodic_exit_count = new_next;
        if new_next != u64::MAX && new_next > count + PERIODIC_EXIT_MARGIN {
            let period = new_next - count - PERIODIC_EXIT_MARGIN;
            ctx.state_mut().instruction_counter.realign_sampling(period);
        }
    }

    let in_margin = new_next != u64::MAX
        && count >= new_next.saturating_sub(PERIODIC_EXIT_MARGIN)
        && count < new_next;

    // Record PMU skid on the exit that first enters the margin window for
    // this target. The PMI was configured to fire at
    // `new_next - PERIODIC_EXIT_MARGIN`; the actual firing point is `count`,
    // so skid = count - (new_next - PERIODIC_EXIT_MARGIN). Stashed on
    // VmState for the next log_exit call to attach to this entry.
    if in_margin && !currently_enabled {
        let want = new_next - PERIODIC_EXIT_MARGIN;
        ctx.state_mut().pending_pmi_skid = count.saturating_sub(want);
    }

    let should_enable = should_single_step || in_margin;

    if should_enable != currently_enabled {
        // Toggle MTF in primary processor-based controls
        let mut controls = ctx
            .state()
            .vmcs
            .read32(VmcsField32::PrimaryProcBasedVmExecControls)
            .map_err(|_| EE::Fatal("Failed to read primary controls for MTF"))?;

        if should_enable {
            controls |= cpu_based::MONITOR_TRAP_FLAG;
        } else {
            controls &= !cpu_based::MONITOR_TRAP_FLAG;
        }

        ctx.state()
            .vmcs
            .write32(VmcsField32::PrimaryProcBasedVmExecControls, controls)
            .map_err(|_| EE::Fatal("Failed to write primary controls for MTF"))?;

        ctx.state_mut().mtf_enabled = should_enable;
    }

    Ok(())
}

/// Handle a VM exit.
///
/// This is the main entry point for VM exit handling. It reads the exit reason
/// and dispatches to the appropriate handler.
///
/// # Returns
///
/// - `ExitHandlerResult::Continue` if the exit was handled and guest execution should continue
/// - `ExitHandlerResult::ExitToUserspace(reason)` if control should return to userspace
/// - `ExitHandlerResult::Error(e)` if a fatal error occurred
pub fn handle_exit<C: VmContext, K: Kernel, A: CowAllocator<C::CowPage>>(
    ctx: &mut C,
    kernel: &K,
    allocator: &mut A,
) -> ExitHandlerResult {
    // Start timing the exit handler
    let start_tsc = rdtsc();

    let reason = match read_exit_reason(ctx) {
        Ok(r) => r,
        Err(e) => return ExitHandlerResult::Error(e),
    };

    let qual = match read_exit_qualification(ctx) {
        Ok(q) => q,
        Err(e) => return ExitHandlerResult::Error(e),
    };

    let non_deterministic_exit = match reason {
        ExitReason::ExternalInterrupt
        | ExitReason::VmxPreemptionTimer
        | ExitReason::ExceptionNmi => true,
        // Non-APIC EPT violations (COW faults, stale TLB hits) are treated as
        // non-deterministic — they don't advance the emulated TSC or get logged.
        // APIC/IOAPIC MMIO EPT violations are deterministic (device emulation).
        ExitReason::EptViolation => {
            let gpa = ctx
                .state()
                .vmcs
                .read64(VmcsField64::GuestPhysicalAddr)
                .unwrap_or(0);
            !((APIC_BASE..APIC_BASE + APIC_SIZE).contains(&gpa)
                || (IOAPIC_BASE..IOAPIC_BASE + IOAPIC_SIZE).contains(&gpa))
        }
        // MTF exits inside the margin happen at instruction counts that
        // depend on PMU skid, so the intermediate steps are non-deterministic.
        // The exit exactly at the next forced-exit boundary
        // (count == next_periodic_exit_count) lands at a deterministic count
        // — MTF stepping into the boundary is precise, and the sampling
        // counter is realigned at every target change so the next PMI
        // reliably re-enters the margin. MTF inside a configured single-step
        // TSC range is deterministic by construction.
        ExitReason::MonitorTrapFlag => {
            let count = ctx.state().last_instruction_count;
            let tsc = count + ctx.state().tsc_offset;
            let on_boundary = count == ctx.state().next_periodic_exit_count
                && ctx.state().next_periodic_exit_count != u64::MAX;
            let in_single_step_range = match ctx.state().single_step_tsc_range {
                Some((start, end)) => tsc >= start && tsc < end,
                None => false,
            };
            !(on_boundary || in_single_step_range)
        }
        _ => false,
    };

    ctx.state_mut().last_exit_deterministic = !non_deterministic_exit;

    // Update emulated TSC from instruction count + offset for deterministic exits.
    // This ensures RDTSC/RDTSCP return values that correlate with guest progress.
    // The offset is increased by time-advancing exits like MWAIT.
    if !non_deterministic_exit {
        let tsc = ctx.state().last_instruction_count + ctx.state().tsc_offset;
        ctx.state_mut().emulated_tsc = tsc;
    }

    // Handle the exit FIRST, before any logging or threshold checks.
    // This ensures device state is fully updated before we potentially
    // return to userspace (e.g., for forked VMs to get clean state).
    let result = match reason {
        ExitReason::Cpuid => handle_cpuid(ctx),
        ExitReason::MsrRead => handle_msr_read(ctx),
        ExitReason::MsrWrite => handle_msr_write(ctx),
        ExitReason::CrAccess => handle_cr_access(ctx, CrAccessQualification::from(qual)),
        ExitReason::IoInstruction => handle_io(ctx, IoQualification::from(qual)),
        ExitReason::EptViolation => {
            handle_ept_violation(ctx, EptViolationQualification::from(qual), allocator)
        }
        ExitReason::ExceptionNmi => handle_exception_nmi(ctx),
        ExitReason::Xsetbv => handle_xsetbv(ctx),

        // Time-related exits for deterministic emulation
        ExitReason::Rdtsc => handle_rdtsc(ctx),
        ExitReason::Rdtscp => handle_rdtscp(ctx),
        ExitReason::Rdpmc => handle_rdpmc(ctx),

        // RDRAND/RDSEED exits for random number emulation
        ExitReason::Rdrand => handle_rdrand(ctx),
        ExitReason::Rdseed => handle_rdseed(ctx),

        // Monitor Trap Flag - VM exit after each guest instruction (single-step mode)
        // The exit is already logged above; just continue executing.
        ExitReason::MonitorTrapFlag => ExitHandlerResult::Continue,

        ExitReason::Hlt => handle_idle(ctx),

        ExitReason::Mwait => handle_idle(ctx),

        ExitReason::Monitor => {
            // MONITOR sets up address-range monitoring hardware for use with MWAIT.
            // We intercept it (MONITOR_EXITING=1) to ensure deterministic behavior:
            // by not actually arming the monitor hardware, MWAIT exit qualification
            // will always be 0 (not armed), regardless of external interrupt timing.
            // This is safe because our MWAIT handler advances TSC to the timer deadline
            // anyway - we don't rely on memory store wakeups.
            if let Err(e) = advance_rip(ctx) {
                return ExitHandlerResult::Error(e);
            }
            ExitHandlerResult::Continue
        }

        ExitReason::TripleFault => {
            // Dump detailed VMCS state for debugging
            dump_triple_fault_state(ctx);
            ExitHandlerResult::Error(EE::TripleFault)
        }

        ExitReason::InvalidGuestState => ExitHandlerResult::Error(EE::InvalidGuestState),

        // VMCALL - hypercall interface
        ExitReason::Vmcall => handle_vmcall(ctx, allocator),

        // Other VMX instructions - exit to userspace (guest shouldn't use nested VMX)
        ExitReason::Vmclear
        | ExitReason::Vmlaunch
        | ExitReason::Vmptrld
        | ExitReason::Vmptrst
        | ExitReason::Vmread
        | ExitReason::Vmresume
        | ExitReason::Vmwrite
        | ExitReason::Vmxoff
        | ExitReason::Vmxon => {
            // Exit to userspace. Could inject #UD instead.
            ExitHandlerResult::ExitToUserspace(reason)
        }

        // VMX preemption timer - return to userspace to give it a heartbeat.
        // This allows userspace to receive serial output periodically and
        // check for signals. Userspace should just call RUN again.
        ExitReason::VmxPreemptionTimer => {
            // Reset the preemption timer for the next run (~10ms)
            if ctx
                .state()
                .vmcs
                .write32(VmcsField32::VmxPreemptionTimerValue, 0x100000)
                .is_err()
            {
                return ExitHandlerResult::Error(EE::Fatal("Failed to reset preemption timer"));
            }
            ExitHandlerResult::ExitToUserspace(reason)
        }

        // External interrupt - handled in-kernel by briefly enabling interrupts.
        // The pending interrupt is delivered through the IDT.
        ExitReason::ExternalInterrupt => {
            handle_external_interrupt(kernel);
            ExitHandlerResult::Continue
        }

        // Interrupt window opened - guest is now interruptible
        // Disable interrupt-window exiting; inject_pending_interrupt() will inject on next VM entry
        ExitReason::InterruptWindow => {
            if let Err(e) = disable_interrupt_window_exiting(ctx) {
                return ExitHandlerResult::Error(e);
            }
            ExitHandlerResult::Continue
        }

        // Other external events that should return to userspace
        ExitReason::Init
        | ExitReason::Sipi
        | ExitReason::NmiWindow
        | ExitReason::TprBelowThreshold
        | ExitReason::ApicAccess
        | ExitReason::ApicWrite => ExitHandlerResult::ExitToUserspace(reason),

        // Unhandled exits - return to userspace
        _ => ExitHandlerResult::ExitToUserspace(reason),
    };

    // Record exit handler timing statistics. Margin-window MTF steps are
    // non-deterministic (their count depends on PMU skid), so they go to a
    // separate bucket to keep `mtf.count` reproducible across runs.
    let end_tsc = rdtsc();
    let cycles = end_tsc.saturating_sub(start_tsc);
    if reason == ExitReason::MonitorTrapFlag && non_deterministic_exit {
        ctx.state_mut().exit_stats.periodic_margin_steps += 1;
    } else {
        ctx.state_mut().exit_stats.record(reason, cycles);
    }

    // Now that the exit is handled, do logging and threshold checks.
    // These happen AFTER exit handling so device state is clean.

    // Update MTF state after the handler. Runs for both deterministic and
    // non-deterministic exits because the periodic-exit margin needs the
    // PMI's external-interrupt exit (non-deterministic) to enable MTF on
    // margin entry. update_mtf_state derives its TSC from
    // last_instruction_count + tsc_offset, which stays current regardless of
    // whether emulated_tsc was updated above. Time-advancing exits (MWAIT/HLT
    // via handle_idle) modify emulated_tsc and tsc_offset to reach the APIC
    // timer deadline; this still needs to run after the handler so the post-
    // handler TSC is what's checked.
    if let Err(e) = update_mtf_state(ctx) {
        return ExitHandlerResult::Error(e);
    }

    // Check if stop-at-tsc threshold is reached (deterministic exits only)
    if !non_deterministic_exit {
        if let Some(stop_tsc) = ctx.state().stop_at_tsc {
            if ctx.state().emulated_tsc >= stop_tsc {
                // Log what exit triggered stop-at-tsc and full APIC state
                let apic = &ctx.state().devices.apic;
                log_err!(
                    "STOP-AT-TSC: exit={:?}, tsc={}, deadline={}, initial={}, lvt_timer={:#x}, irr[7]={:#x}, isr[7]={:#x}\n",
                    reason,
                    ctx.state().emulated_tsc,
                    apic.timer_deadline,
                    apic.timer_initial,
                    apic.lvt_timer,
                    apic.irr[7],
                    apic.isr[7]
                );
                // Log state if AtShutdown mode is enabled (treat stop-at-tsc like shutdown)
                ctx.state_mut().log_shutdown();
                return ExitHandlerResult::ExitToUserspace(ExitReason::StopTscReached);
            }
        }
    }

    // Log exits if logging is enabled (both deterministic and non-deterministic)
    //
    // We need to do this after the stop_at_tsc check, so that the guest is not re-entered after
    // returning from user space (to handle the LogBufferFull exit), which would cause the
    // StopTscReached exit to be non-deterministic
    //
    // TODO: log the exit above but only return to userspace here, otherwise we're missing one exit
    // from the log
    if ctx.state().log_enabled() {
        ctx.state_mut()
            .log_exit(reason, qual, !non_deterministic_exit);
        if ctx.state().log_buffer_full() {
            return ExitHandlerResult::ExitToUserspace(ExitReason::LogBufferFull);
        }
    }

    result
}
