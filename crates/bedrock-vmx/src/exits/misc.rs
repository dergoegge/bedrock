// SPDX-License-Identifier: GPL-2.0

//! Miscellaneous exit handlers: exceptions, XSETBV, triple fault debugging.

use super::helpers::{advance_rip, inject_exception, ExitError, ExitHandlerResult};
use super::qualifications::{InterruptionInfo, InterruptionType};
use super::reasons::ExitReason;

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Handle exception/NMI exit.
pub fn handle_exception_nmi<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    // Read interruption info to get exception details
    let info_raw = match ctx.state().vmcs.read32(VmcsField32::VmExitInterruptionInfo) {
        Ok(v) => v,
        Err(e) => return ExitHandlerResult::Error(ExitError::VmcsReadError(e)),
    };

    let info = InterruptionInfo::from(info_raw);

    // Check if this is an NMI. NMIs can be either:
    // 1. Our PMI (performance counter overflow) — deterministic, handled internally
    // 2. A host NMI (watchdog, etc.) — must be forwarded to the host's NMI handler
    //
    // The `pmi_exit` flag is set by the exit classification in handle_exit()
    // via check_and_clear_pmi() BEFORE this handler runs. If it's true, this
    // was our PMI and the overflow status has already been cleared.
    if matches!(info.interruption_type, InterruptionType::Nmi) {
        if ctx.state().pmi_exit {
            // PMI-caused NMI — deterministic exit triggered by the instruction
            // counter reaching its overflow target. Nothing to do here; the
            // run loop will read the counter and handle timer injection.
            return ExitHandlerResult::Continue;
        }

        // Host NMI — invoke the host's NMI handler via software interrupt.
        // This is critical: KVM and bhyve both do this immediately after VM exit.
        // Failure to handle host NMIs can cause watchdog timeouts and system instability.
        // SAFETY: INT 2 invokes the host NMI handler to service the NMI that
        // triggered a VM exit. This is necessary for host watchdog and system stability.
        #[cfg(all(target_arch = "x86_64", not(feature = "cargo")))]
        unsafe {
            core::arch::asm!("int $2", options(nomem, nostack));
        }

        // After handling the host NMI, resume the guest.
        return ExitHandlerResult::Continue;
    }

    // Intercept guest #PF: reinject so the guest handles it normally.
    // #PF is a fault — RIP already points to the faulting instruction (no advance needed).
    if info.vector == 14 && ctx.state().intercept_pf {
        let error_code = ctx
            .state()
            .vmcs
            .read32(VmcsField32::VmExitInterruptionErrorCode)
            .unwrap_or(0);

        // The faulting linear address is in exit qualification (SDM Vol 3C, Section 28.2.2).
        // The CPU may not update the physical CR2 register before a VM exit caused by the
        // exception bitmap — the SDM only guarantees exit qualification. Explicitly copy it
        // to guest_cr2 so vmx_support.S restores the correct value on VM entry.
        // (KVM does the same: vmx_get_exit_qual → vcpu->arch.cr2.)
        let fault_addr = ctx
            .state()
            .vmcs
            .read_natural(VmcsFieldNatural::ExitQualification)
            .unwrap_or(0);
        ctx.state_mut().vmx_ctx.guest_cr2 = fault_addr;

        let reinject_info = InterruptionInfo {
            vector: 14,
            interruption_type: InterruptionType::HardwareException,
            error_code_valid: true,
            nmi_unblocking: false,
            valid: true,
        };
        if let Err(e) = inject_exception(ctx, reinject_info, Some(error_code)) {
            return ExitHandlerResult::Error(e);
        }

        return ExitHandlerResult::Continue;
    }

    // Read RIP for debugging
    let rip = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestRip)
        .unwrap_or(0);

    // For page faults, read CR2 (faulting address)
    let cr2 = if info.vector == 14 {
        ctx.state()
            .vmcs
            .read_natural(VmcsFieldNatural::ExitQualification)
            .unwrap_or(0)
    } else {
        0
    };

    // Read error code if present
    let error_code = if info.error_code_valid {
        ctx.state()
            .vmcs
            .read32(VmcsField32::VmExitInterruptionErrorCode)
            .unwrap_or(0)
    } else {
        0
    };

    // Log the exception for debugging
    log_err!(
        "Exception #{} ({}) at RIP={:#x}, error_code={:#x}, CR2={:#x}",
        info.vector,
        exception_name(info.vector),
        rip,
        error_code,
        cr2
    );

    // For now, exit to userspace for all exceptions
    // A more complete implementation would handle some internally
    ExitHandlerResult::ExitToUserspace(ExitReason::ExceptionNmi)
}

/// Get a human-readable name for an exception vector.
pub fn exception_name(vector: u8) -> &'static str {
    match vector {
        0 => "DE (Divide Error)",
        1 => "DB (Debug)",
        2 => "NMI",
        3 => "BP (Breakpoint)",
        4 => "OF (Overflow)",
        5 => "BR (Bound Range)",
        6 => "UD (Invalid Opcode)",
        7 => "NM (Device Not Available)",
        8 => "DF (Double Fault)",
        10 => "TS (Invalid TSS)",
        11 => "NP (Segment Not Present)",
        12 => "SS (Stack Fault)",
        13 => "GP (General Protection)",
        14 => "PF (Page Fault)",
        16 => "MF (x87 FPU Error)",
        17 => "AC (Alignment Check)",
        18 => "MC (Machine Check)",
        19 => "XM (SIMD Exception)",
        20 => "VE (Virtualization)",
        21 => "CP (Control Protection)",
        _ => "Unknown",
    }
}

// =============================================================================
// Triple Fault Debugging
// =============================================================================

/// Dump detailed VMCS state when a triple fault occurs.
///
/// This helps diagnose what exception is causing the triple fault.
pub fn dump_triple_fault_state<C: VmContext>(ctx: &C) {
    log_err!("=== TRIPLE FAULT DEBUG INFO ===");

    // Guest RIP and RSP
    if let Ok(rip) = ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestRip) {
        log_err!("Guest RIP: {:#018x}", rip);
    }
    if let Ok(rsp) = ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestRsp) {
        log_err!("Guest RSP: {:#018x}", rsp);
    }
    if let Ok(rflags) = ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestRflags) {
        log_err!("Guest RFLAGS: {:#018x}", rflags);
    }

    // Control registers
    if let Ok(cr0) = ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestCr0) {
        log_err!("Guest CR0: {:#018x}", cr0);
    }
    if let Ok(cr3) = ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestCr3) {
        log_err!("Guest CR3: {:#018x}", cr3);
    }
    if let Ok(cr4) = ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestCr4) {
        log_err!("Guest CR4: {:#018x}", cr4);
    }

    // Segment selectors
    if let Ok(cs) = ctx.state().vmcs.read16(VmcsField16::GuestCsSelector) {
        log_err!("Guest CS: {:#06x}", cs);
    }
    if let Ok(ss) = ctx.state().vmcs.read16(VmcsField16::GuestSsSelector) {
        log_err!("Guest SS: {:#06x}", ss);
    }

    // IDT vectoring info - shows what exception was being delivered when triple fault occurred
    if let Ok(idt_info) = ctx.state().vmcs.read32(VmcsField32::IdtVectoringInfo) {
        if idt_info & (1 << 31) != 0 {
            // Valid bit is set - an exception was being delivered
            let vector = idt_info & 0xFF;
            let int_type = (idt_info >> 8) & 0x7;
            let has_error = (idt_info >> 11) & 1 != 0;
            log_err!(
                "IDT Vectoring: vector={} type={} error_valid={}",
                vector,
                int_type,
                has_error
            );
            log_err!(
                "  Exception that caused triple fault: {} ({})",
                vector,
                exception_name(vector as u8)
            );

            if has_error {
                if let Ok(error_code) = ctx.state().vmcs.read32(VmcsField32::IdtVectoringErrorCode)
                {
                    log_err!("  Error code: {:#x}", error_code);
                }
            }
        } else {
            log_err!("IDT Vectoring: no exception was being delivered");
        }
    }

    // Exit qualification
    if let Ok(qual) = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::ExitQualification)
    {
        log_err!("Exit Qualification: {:#018x}", qual);
    }

    // Guest linear address (for memory-related faults)
    if let Ok(linear) = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestLinearAddr)
    {
        log_err!("Guest Linear Address: {:#018x}", linear);
    }

    // Guest physical address (for EPT violations)
    if let Ok(phys) = ctx.state().vmcs.read64(VmcsField64::GuestPhysicalAddr) {
        log_err!("Guest Physical Address: {:#018x}", phys);
    }

    // GDTR and IDTR
    if let Ok(gdtr_base) = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestGdtrBase)
    {
        if let Ok(gdtr_limit) = ctx.state().vmcs.read32(VmcsField32::GuestGdtrLimit) {
            log_err!(
                "Guest GDTR: base={:#018x} limit={:#06x}",
                gdtr_base,
                gdtr_limit
            );
        }
    }
    if let Ok(idtr_base) = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestIdtrBase)
    {
        if let Ok(idtr_limit) = ctx.state().vmcs.read32(VmcsField32::GuestIdtrLimit) {
            log_err!(
                "Guest IDTR: base={:#018x} limit={:#06x}",
                idtr_base,
                idtr_limit
            );
        }
    }

    // Guest activity and interruptibility state
    if let Ok(activity) = ctx.state().vmcs.read32(VmcsField32::GuestActivityState) {
        log_err!("Guest Activity State: {}", activity);
    }
    if let Ok(interruptibility) = ctx
        .state()
        .vmcs
        .read32(VmcsField32::GuestInterruptibilityState)
    {
        log_err!("Guest Interruptibility State: {:#x}", interruptibility);
    }

    log_err!("=== END TRIPLE FAULT DEBUG ===");
}

// =============================================================================
// XSETBV Handler
// =============================================================================

/// Handle XSETBV exit.
///
/// XSETBV sets the extended control register XCR0 which controls which
/// XSAVE-supported processor state components are enabled.
///
/// Intel SDM Vol 2B, XSETBV instruction.
pub fn handle_xsetbv<C: VmContext>(ctx: &mut C) -> ExitHandlerResult {
    let gprs = ctx.state().gprs;
    let xcr_num = gprs.rcx as u32;
    let value = (gprs.rdx << 32) | (gprs.rax & 0xFFFFFFFF);

    // Only XCR0 is currently defined
    if xcr_num != 0 {
        log_err!("XSETBV: invalid XCR number {}", xcr_num);
        // Should inject #GP(0)
        return ExitHandlerResult::ExitToUserspace(ExitReason::Xsetbv);
    }

    // Validate XCR0 value:
    // - Bit 0 (x87 FPU) must be 1
    // - If AVX (bit 2) is set, SSE (bit 1) must also be set
    if value & 1 == 0 {
        log_err!("XSETBV: XCR0 bit 0 (x87) must be 1, got {:#x}", value);
        return ExitHandlerResult::ExitToUserspace(ExitReason::Xsetbv);
    }

    if (value & 4) != 0 && (value & 2) == 0 {
        log_err!("XSETBV: AVX requires SSE, got {:#x}", value);
        return ExitHandlerResult::ExitToUserspace(ExitReason::Xsetbv);
    }

    // Update the xcr0_mask used for XSAVE/XRSTOR
    // This ensures the correct state components are saved/restored
    ctx.state_mut().xcr0_mask = value;

    log_debug!("XSETBV: setting XCR0 to {:#x}", value);

    // Advance RIP
    if let Err(e) = advance_rip(ctx) {
        return ExitHandlerResult::Error(e);
    }

    ExitHandlerResult::Continue
}
