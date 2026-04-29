// SPDX-License-Identifier: GPL-2.0

//! Interrupt injection and APIC timer handling.

use core::arch::asm;

use super::helpers::{inject_exception, ExitError};
use super::pebs::arm_for_next_iteration;
use super::qualifications::InterruptionInfo;

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Check if APIC timer has expired and set IRR bit if so.
/// Uses emulated TSC for determinism.
pub fn check_apic_timer<C: VmContext>(ctx: &mut C) {
    let current_tsc = ctx.state().emulated_tsc;
    let timer_deadline = ctx.state().devices.apic.timer_deadline;
    let svr = ctx.state().devices.apic.svr;
    let lvt_timer_init = ctx.state().devices.apic.lvt_timer;

    if timer_deadline == 0 {
        return;
    }
    if current_tsc < timer_deadline {
        return;
    }
    if (svr & (1 << 8)) == 0 {
        return;
    }
    if (lvt_timer_init & (1 << 16)) != 0 {
        return;
    }

    // Diagnostic: count timer firings that arrive past the deadline. The
    // precise PEBS+MTF boundary lands `current_tsc == timer_deadline`;
    // anything strictly greater means PEBS didn't fire at the
    // `target - PEBS_MARGIN` point and the timer is being delivered late
    // on whatever deterministic exit happened past the deadline.
    if current_tsc > timer_deadline {
        ctx.state_mut().exit_stats.apic_timer_late_inject += 1;
    }

    let apic = &mut ctx.state_mut().devices.apic;

    // Get vector from LVT timer (bits 7:0)
    let vector = (apic.lvt_timer & 0xFF) as u8;

    // Set bit in IRR
    let irr_index = (vector / 32) as usize;
    let irr_bit = 1u32 << (vector % 32);
    apic.irr[irr_index] |= irr_bit;

    // Handle periodic vs one-shot mode (bit 17 of lvt_timer)
    if (apic.lvt_timer & (1 << 17)) != 0 {
        // Periodic: reset deadline for next period
        let divisor = apic_timer_divisor(apic.timer_divide);
        let ticks = u64::from(apic.timer_initial) * u64::from(divisor);
        apic.timer_deadline = current_tsc.wrapping_add(ticks);
    } else {
        // One-shot: stop timer
        apic.timer_deadline = 0;
    }
}

/// Calculate APIC timer divisor from the Divide Configuration Register (DCR).
fn apic_timer_divisor(dcr: u32) -> u32 {
    let encoded = ((dcr >> 1) & 0b100) | (dcr & 0b11);
    match encoded {
        0b000 => 2,
        0b001 => 4,
        0b010 => 8,
        0b011 => 16,
        0b100 => 32,
        0b101 => 64,
        0b110 => 128,
        0b111 => 1,
        _ => 1,
    }
}

/// Find the highest priority pending interrupt in the APIC IRR.
/// Returns the vector number if an interrupt is pending, None otherwise.
fn apic_pending_vector<C: VmContext>(ctx: &C) -> Option<u8> {
    let apic = &ctx.state().devices.apic;

    // Check if APIC is enabled (SVR bit 8)
    if (apic.svr & (1 << 8)) == 0 {
        return None;
    }

    // Find highest priority pending interrupt (highest vector number)
    for i in (0..8).rev() {
        if apic.irr[i] != 0 {
            // Find highest bit set in this word
            let bit = 31 - apic.irr[i].leading_zeros();
            return Some((i * 32 + bit as usize) as u8);
        }
    }
    None
}

/// Enable interrupt-window exiting so we get a VM exit when the guest becomes interruptible.
pub fn enable_interrupt_window_exiting<C: VmContext>(ctx: &mut C) -> Result<(), ExitError> {
    let controls = ctx
        .state()
        .vmcs
        .read32(VmcsField32::PrimaryProcBasedVmExecControls)
        .map_err(ExitError::VmcsReadError)?;
    if controls & cpu_based::INTR_WINDOW_EXITING == 0 {
        ctx.state()
            .vmcs
            .write32(
                VmcsField32::PrimaryProcBasedVmExecControls,
                controls | cpu_based::INTR_WINDOW_EXITING,
            )
            .map_err(ExitError::VmcsWriteError)?;
    }
    Ok(())
}

/// Disable interrupt-window exiting.
pub fn disable_interrupt_window_exiting<C: VmContext>(ctx: &mut C) -> Result<(), ExitError> {
    let controls = ctx
        .state()
        .vmcs
        .read32(VmcsField32::PrimaryProcBasedVmExecControls)
        .map_err(ExitError::VmcsReadError)?;
    if controls & cpu_based::INTR_WINDOW_EXITING != 0 {
        ctx.state()
            .vmcs
            .write32(
                VmcsField32::PrimaryProcBasedVmExecControls,
                controls & !cpu_based::INTR_WINDOW_EXITING,
            )
            .map_err(ExitError::VmcsWriteError)?;
    }
    Ok(())
}

/// Check IDT-vectoring information and re-inject if an event was interrupted during delivery.
///
/// Per Intel SDM Vol 3C Section 29.2.4: When a VM exit occurs during delivery of an event
/// through the IDT (e.g., EPT violation while pushing interrupt frame to stack), the event
/// info is saved in IdtVectoringInfo. The hypervisor must re-inject this event by copying
/// it to VmEntryInterruptionInfo.
///
/// Returns Ok(true) if an event was re-injected, Ok(false) if no event needs re-injection.
pub fn reinject_vectored_event<C: VmContext>(ctx: &mut C) -> Result<bool, ExitError> {
    let idt_info = ctx
        .state()
        .vmcs
        .read32(VmcsField32::IdtVectoringInfo)
        .map_err(ExitError::VmcsReadError)?;

    // Check if valid (bit 31) - if not set, no event was interrupted
    if idt_info & (1 << 31) == 0 {
        return Ok(false);
    }

    let vector = (idt_info & 0xFF) as u8;
    let int_type = (idt_info >> 8) & 0x7;

    log_info!(
        "IDT-vectoring: re-injecting interrupted event vector={} type={}\n",
        vector,
        int_type
    );

    // Copy IDT-vectoring info to VM-entry interruption-info for re-injection.
    // The formats are identical per Intel SDM (Table 26-18 and Table 26-21).
    ctx.state()
        .vmcs
        .write32(VmcsField32::VmEntryInterruptionInfo, idt_info)
        .map_err(ExitError::VmcsWriteError)?;

    // If error code is valid (bit 11), copy that too
    if idt_info & (1 << 11) != 0 {
        let error_code = ctx
            .state()
            .vmcs
            .read32(VmcsField32::IdtVectoringErrorCode)
            .map_err(ExitError::VmcsReadError)?;
        ctx.state()
            .vmcs
            .write32(VmcsField32::VmEntryExceptionErrorCode, error_code)
            .map_err(ExitError::VmcsWriteError)?;
    }

    Ok(true)
}

/// Inject any pending interrupt into the guest before VM entry.
/// This should be called before each VMLAUNCH/VMRESUME.
pub fn inject_pending_interrupt<C: VmContext>(ctx: &mut C) -> Result<(), ExitError> {
    // First, check if there's an interrupted event that needs re-injection.
    // This handles the case where interrupt delivery was aborted by an EPT violation
    // (e.g., CoW fault when pushing interrupt frame to guest stack).
    // Per Intel SDM Vol 3C Section 29.2.4, we must re-inject before handling new events.
    // This path runs unconditionally — the event was already in flight and must complete.
    //
    // Re-arm PEBS even on this path. If the previous PEBS-EPT exit consumed
    // `armed_action` via `.take()` and the very next exit also sets
    // IdtVectoringInfo (e.g. an interrupted CoW-fault delivery), early-
    // returning without re-arming would enter the next iter with
    // `pebs_armed_this_iter = false` — no MSR-load of PEBS state, no
    // counter overflow for the upcoming deadline, and the timer fires
    // at whatever natural exit happens past it. That difference between
    // runs surfaces as a divergent deterministic log.
    if reinject_vectored_event(ctx)? {
        // Event will be re-injected on VM entry, don't inject anything else
        arm_for_next_iteration(ctx);
        return Ok(());
    }

    // Update IRR for any timer that expired since the last deterministic exit
    // and arm PEBS for the next deadline. Both run only when the last exit
    // was deterministic — otherwise we'd risk setting IRR / re-injecting at
    // a non-deterministic boundary (e.g., a host NMI landing at the same
    // instruction where hardware would have fired an interrupt-window VM
    // exit, silently absorbing the IWE exit and shortening the determ log
    // by one entry). On non-det exits we still re-arm PEBS, just below.
    if ctx.state().last_exit_deterministic {
        // If an exception reinjection (e.g., #PF) is already pending in
        // VmEntryInterruptionInfo, don't overwrite it with an interrupt. The
        // interrupt will be injected on the next exit.
        let pending = ctx
            .state()
            .vmcs
            .read32(VmcsField32::VmEntryInterruptionInfo)
            .unwrap_or(0);
        if pending & (1 << 31) != 0 {
            arm_for_next_iteration(ctx);
            return Ok(());
        }

        // Check timer expiry and update IRR. If the deadline was reached, this
        // sets the IRR bit and (for periodic timers) auto-reloads the deadline.
        check_apic_timer(ctx);
    }

    // Re-arm PEBS for the next APIC timer deadline regardless of whether
    // the last exit was deterministic. After a non-deterministic exit
    // (host NMI, external interrupt, VMX preemption timer) the previous
    // iteration's counter_reload no longer matches "instructions remaining
    // to deadline": the interrupted iter retired some instructions toward
    // the overflow target, but PMC0 resets to the same counter_reload on
    // the next VM-entry, so PEBS would fire delta-1 instructions into the
    // *new* iter — past the original deadline by exactly however many
    // instructions were burned in the interrupted iter. Re-arming
    // recomputes the remaining delta from the current INST_RETIRED count
    // and keeps the precise emulated_tsc landing point intact.
    // arm_for_next_iteration deliberately uses last_instruction_count +
    // tsc_offset (not emulated_tsc, which is stale on non-det exits) so
    // the math works in either case.
    arm_for_next_iteration(ctx);

    if !ctx.state().last_exit_deterministic {
        return Ok(());
    }

    // Find highest priority pending interrupt
    let vector = match apic_pending_vector(ctx) {
        Some(v) => v,
        None => return Ok(()), // No pending interrupt
    };

    // Check if guest is interruptible (RFLAGS.IF = 1)
    let rflags = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestRflags)
        .map_err(ExitError::VmcsReadError)?;
    if (rflags & (1 << 9)) == 0 {
        // IF=0, enable interrupt-window exiting to inject later
        enable_interrupt_window_exiting(ctx)?;
        return Ok(());
    }

    // Check interruptibility state (blocking by STI or MOV SS)
    let interruptibility = ctx
        .state()
        .vmcs
        .read32(VmcsField32::GuestInterruptibilityState)
        .map_err(ExitError::VmcsReadError)?;
    if (interruptibility & 0x3) != 0 {
        // Blocked by STI or MOV SS
        enable_interrupt_window_exiting(ctx)?;
        return Ok(());
    }

    // Guest is interruptible - inject the interrupt
    let info = InterruptionInfo::external_interrupt(vector);
    inject_exception(ctx, info, None)?;

    // Clear IRR bit, set ISR bit (interrupt now in service)
    {
        let apic = &mut ctx.state_mut().devices.apic;
        let irr_index = (vector / 32) as usize;
        let bit = 1u32 << (vector % 32);
        apic.irr[irr_index] &= !bit;
        apic.isr[irr_index] |= bit;
    }

    // Disable interrupt-window exiting if it was enabled
    disable_interrupt_window_exiting(ctx)?;

    Ok(())
}

/// Deliver an interrupt through the I/O APIC to the local APIC.
///
/// This looks up the redirection table entry for the given IRQ pin,
/// and if not masked, sets the corresponding bit in the local APIC's IRR.
pub fn ioapic_deliver_irq<C: VmContext>(ctx: &mut C, irq: u8) {
    if irq as usize >= IOAPIC_NUM_PINS {
        return;
    }

    let entry = ctx.state().devices.ioapic.redtbl[irq as usize];

    // Check if masked (bit 16)
    if (entry >> 16) & 1 != 0 {
        return;
    }

    let vector = (entry & 0xFF) as u8;
    if vector < 16 {
        // Vectors 0-15 are reserved
        return;
    }

    // Set the bit in local APIC's IRR
    let irr_idx = (vector / 32) as usize;
    let irr_bit = 1u32 << (vector % 32);

    if irr_idx < 8 {
        ctx.state_mut().devices.apic.irr[irr_idx] |= irr_bit;
    }
}

/// Handle external interrupt by briefly enabling interrupts.
///
/// This uses the SVM-style approach: enable interrupts to allow the pending
/// interrupt to be delivered through the IDT, then disable interrupts before
/// returning to re-enter the guest.
#[inline]
pub fn handle_external_interrupt<K: Kernel>(kernel: &K) {
    let _irq_window = ReverseIrqGuard::new(kernel);
    // SAFETY: NOP is a safe instruction; the IRQ window opened by ReverseIrqGuard
    // allows pending host interrupts to be delivered through the IDT.
    unsafe {
        asm!("nop", options(nomem, nostack, preserves_flags));
    }
}
