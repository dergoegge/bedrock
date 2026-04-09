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
pub use helpers::{ExitError, ExitHandlerResult};
pub use interrupts::{inject_pending_interrupt, reinject_vectored_event};
pub use qualifications::{
    CrAccessQualification, EptViolationQualification, IoQualification, RdrandInstructionInfo,
    RdrandOperandSize,
};
pub use reasons::ExitReason;

// Internal imports for handle_exit
use apic::{APIC_BASE, APIC_SIZE, IOAPIC_BASE, IOAPIC_SIZE};
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

/// Update MTF (Monitor Trap Flag) state based on TSC range configuration.
///
/// If single-stepping is configured and the current TSC is within the range,
/// enables MTF to cause a VM exit after each guest instruction.
/// Disables MTF when outside the range to avoid performance overhead.
pub fn update_mtf_state<C: VmContext>(ctx: &mut C) -> Result<(), ExitError> {
    let tsc = ctx.state().emulated_tsc;
    let range = ctx.state().single_step_tsc_range;
    let currently_enabled = ctx.state().mtf_enabled;

    let should_enable = match range {
        Some((start, end)) => tsc >= start && tsc < end,
        None => false,
    };

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
        ExitReason::ExternalInterrupt | ExitReason::VmxPreemptionTimer | ExitReason::ExceptionNmi => true,
        // Non-APIC EPT violations (COW faults, stale TLB hits) are treated as
        // non-deterministic — they don't advance the emulated TSC or get logged.
        // APIC/IOAPIC MMIO EPT violations are deterministic (device emulation).
        ExitReason::EptViolation => {
            let gpa = ctx.state().vmcs.read64(VmcsField64::GuestPhysicalAddr).unwrap_or(0);
            !((APIC_BASE..APIC_BASE + APIC_SIZE).contains(&gpa)
                || (IOAPIC_BASE..IOAPIC_BASE + IOAPIC_SIZE).contains(&gpa))
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

        // Update MTF (single-step) state based on TSC range
        if let Err(e) = update_mtf_state(ctx) {
            return ExitHandlerResult::Error(e);
        }
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
                .write32(VmcsField32::VmxPreemptionTimerValue, 0x100000).is_err()
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

    // Record exit handler timing statistics
    let end_tsc = rdtsc();
    let cycles = end_tsc.saturating_sub(start_tsc);
    ctx.state_mut().exit_stats.record(reason, cycles);

    // Now that the exit is handled, do logging and threshold checks.
    // These happen AFTER exit handling so device state is clean.

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
