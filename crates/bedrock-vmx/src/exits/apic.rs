// SPDX-License-Identifier: GPL-2.0

//! Local APIC and I/O APIC MMIO emulation.

use super::cr::{get_gpr_value, set_gpr_value};
use super::ept::translate_gva_to_gpa;
use super::helpers::{ExitError, ExitHandlerResult};
use super::qualifications::EptViolationQualification;
use super::reasons::ExitReason;

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// APIC base address (Local APIC MMIO region).
pub const APIC_BASE: u64 = 0xFEE0_0000;
/// Size of APIC MMIO region.
pub const APIC_SIZE: u64 = 0x1000;

/// I/O APIC base address.
pub const IOAPIC_BASE: u64 = 0xFEC0_0000;
/// Size of I/O APIC MMIO region.
pub const IOAPIC_SIZE: u64 = 0x1000;

/// I/O APIC register indices
const IOAPIC_REG_ID: u32 = 0x00;
const IOAPIC_REG_VER: u32 = 0x01;
const IOAPIC_REG_ARB: u32 = 0x02;
const IOAPIC_REG_REDTBL_BASE: u32 = 0x10;

/// Handle APIC MMIO access.
///
/// This emulates reads/writes to the Local APIC registers by:
/// 1. Fetching and decoding the instruction at guest RIP
/// 2. For reads: reading from ApicState and writing to guest register
/// 3. For writes: reading from guest register and writing to ApicState
/// 4. Advancing RIP past the instruction
pub fn handle_apic_access<C: VmContext>(
    ctx: &mut C,
    gpa: u64,
    qual: EptViolationQualification,
) -> ExitHandlerResult {
    let offset = (gpa - APIC_BASE) as u32;

    // Get guest RIP to fetch the instruction
    let rip = match ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestRip) {
        Ok(v) => v,
        Err(e) => return ExitHandlerResult::Error(ExitError::VmcsReadError(e)),
    };

    // Translate guest RIP (virtual address) to guest physical address
    let instr_gpa = match translate_gva_to_gpa(ctx, rip) {
        Ok(gpa) => gpa,
        Err(()) => {
            log_err!("APIC emulation: failed to translate RIP={:#x} to GPA", rip);
            return ExitHandlerResult::ExitToUserspace(ExitReason::EptViolation);
        }
    };

    // Fetch instruction bytes
    let mut instr_bytes = [0u8; 15];
    if ctx.read_guest_memory(instr_gpa, &mut instr_bytes).is_err() {
        log_err!(
            "APIC emulation: failed to read instruction at RIP={:#x} (GPA={:#x})",
            rip,
            instr_gpa.as_u64()
        );
        return ExitHandlerResult::ExitToUserspace(ExitReason::EptViolation);
    }

    // Decode the instruction
    let decoded = match decode_instruction(&instr_bytes) {
        Ok(d) => d,
        Err(e) => {
            log_err!(
                "APIC emulation: failed to decode instruction at RIP={:#x}: {:?}",
                rip,
                e
            );
            return ExitHandlerResult::ExitToUserspace(ExitReason::EptViolation);
        }
    };

    // Handle the access based on direction
    if qual.read {
        // APIC read - get value from emulated APIC and write to destination register
        let value = read_apic_register(&ctx.state().devices.apic, offset);

        // Write to the destination register (zero-extended, APIC registers are 32-bit)
        let reg_value = value as u64;

        set_gpr_value(&mut ctx.state_mut().gprs, decoded.register, reg_value);

        log_debug!(
            "APIC read: offset={:#x} -> value={:#x} -> reg{}",
            offset,
            value,
            decoded.register
        );
    } else if qual.write {
        // APIC write - get value from source register and write to emulated APIC
        let reg_value = get_gpr_value(&ctx.state().gprs, decoded.register) as u32;
        let current_tsc = ctx.state().emulated_tsc;

        write_apic_register(
            &mut ctx.state_mut().devices.apic,
            offset,
            reg_value,
            current_tsc,
        );
    }

    // Advance RIP past the instruction
    let new_rip = rip + decoded.length as u64;
    if ctx
        .state()
        .vmcs
        .write_natural(VmcsFieldNatural::GuestRip, new_rip)
        .is_err()
    {
        return ExitHandlerResult::Error(ExitError::Fatal("Failed to advance RIP for APIC access"));
    }

    ExitHandlerResult::Continue
}

/// Read from an emulated APIC register.
///
/// Intel SDM Vol 3A, Table 12-1 defines the register map.
fn read_apic_register(apic: &ApicState, offset: u32) -> u32 {
    match offset {
        // APIC ID (bits 31:24 contain the ID)
        0x020 => apic.id << 24,
        // APIC Version
        0x030 => apic.version,
        // Task Priority Register
        0x080 => apic.tpr,
        // Arbitration Priority Register (always 0 for single vCPU)
        0x090 => 0,
        // Processor Priority Register (derived from TPR for single vCPU)
        0x0A0 => apic.tpr & 0xF0,
        // EOI Register (write-only, reads return 0)
        0x0B0 => 0,
        // Remote Read Register (not implemented)
        0x0C0 => 0,
        // Logical Destination Register
        0x0D0 => apic.ldr,
        // Destination Format Register
        0x0E0 => apic.dfr,
        // Spurious Interrupt Vector Register
        0x0F0 => apic.svr,
        // In-Service Register (8 x 32-bit, offsets 0x100-0x170, stride 0x10)
        0x100..=0x170 => apic.isr[((offset - 0x100) >> 4) as usize],
        // Trigger Mode Register (8 x 32-bit, offsets 0x180-0x1F0, stride 0x10)
        0x180..=0x1F0 => apic.tmr[((offset - 0x180) >> 4) as usize],
        // Interrupt Request Register (8 x 32-bit, offsets 0x200-0x270, stride 0x10)
        0x200..=0x270 => apic.irr[((offset - 0x200) >> 4) as usize],
        // Error Status Register
        0x280 => apic.esr,
        // Interrupt Command Register (low)
        0x300 => apic.icr_lo,
        // Interrupt Command Register (high)
        0x310 => apic.icr_hi,
        // LVT Timer
        0x320 => apic.lvt_timer,
        // LVT Thermal Sensor
        0x330 => apic.lvt_thermal,
        // LVT Performance Monitoring
        0x340 => apic.lvt_perf,
        // LVT LINT0
        0x350 => apic.lvt_lint0,
        // LVT LINT1
        0x360 => apic.lvt_lint1,
        // LVT Error
        0x370 => apic.lvt_error,
        // Timer Initial Count
        0x380 => apic.timer_initial,
        // Timer Current Count (timer not running, always 0)
        0x390 => 0,
        // Timer Divide Configuration
        0x3E0 => apic.timer_divide,
        // Reserved/unknown registers return 0
        _ => {
            log_debug!("APIC read: unknown offset {:#x}", offset);
            0
        }
    }
}

/// Write to an emulated APIC register.
///
/// Intel SDM Vol 3A, Table 12-1 defines the register map.
/// `current_tsc` is the emulated TSC value, used for timer deadline calculation.
fn write_apic_register(apic: &mut ApicState, offset: u32, value: u32, current_tsc: u64) {
    match offset {
        // APIC ID (bits 31:24 are writable in some modes)
        0x020 => apic.id = (value >> 24) & 0xFF,
        // Task Priority Register (bits 7:0 are writable)
        0x080 => apic.tpr = value & 0xFF,
        // EOI Register - clear highest priority ISR bit
        0x0B0 => {
            // Find and clear the highest priority bit in ISR
            for i in (0..8).rev() {
                if apic.isr[i] != 0 {
                    let bit = 31 - apic.isr[i].leading_zeros();
                    apic.isr[i] &= !(1 << bit);
                    break;
                }
            }
        }
        // Logical Destination Register
        0x0D0 => apic.ldr = value,
        // Destination Format Register (bits 31:28 are writable, rest reserved as 1)
        0x0E0 => apic.dfr = value | 0x0FFF_FFFF,
        // Spurious Interrupt Vector Register
        0x0F0 => apic.svr = value,
        // Error Status Register - write clears it
        0x280 => apic.esr = 0,
        // Interrupt Command Register (low) - could trigger IPI (ignored for now)
        0x300 => {
            apic.icr_lo = value;
            // In a real implementation, this would trigger an IPI
            // For single-vCPU, we can mostly ignore this
        }
        // Interrupt Command Register (high)
        0x310 => apic.icr_hi = value,
        // LVT Timer
        0x320 => apic.lvt_timer = value,
        // LVT Thermal Sensor
        0x330 => apic.lvt_thermal = value,
        // LVT Performance Monitoring
        0x340 => apic.lvt_perf = value,
        // LVT LINT0
        0x350 => apic.lvt_lint0 = value,
        // LVT LINT1
        0x360 => apic.lvt_lint1 = value,
        // LVT Error
        0x370 => apic.lvt_error = value,
        // Timer Initial Count - starts the timer
        0x380 => {
            apic.timer_initial = value;
            if value != 0 {
                // Calculate deadline based on divide configuration
                let divisor = apic_timer_divisor(apic.timer_divide);
                let ticks = (value as u64) * (divisor as u64);
                // Set deadline using emulated TSC for determinism
                apic.timer_deadline = current_tsc + ticks;
            } else {
                // Timer stopped
                apic.timer_deadline = 0;
            }
        }
        // Timer Divide Configuration
        0x3E0 => apic.timer_divide = value,
        // Read-only or reserved registers - ignore writes
        _ => {
            log_debug!(
                "APIC write: ignored offset {:#x} value {:#x}",
                offset,
                value
            );
        }
    }
}

/// Calculate APIC timer divisor from the Divide Configuration Register (DCR).
/// DCR bits [3,1:0] encode the divisor.
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

// =============================================================================
// I/O APIC Emulation
// =============================================================================

/// Handle I/O APIC MMIO access.
///
/// The I/O APIC uses indirect register access:
/// - IOREGSEL (offset 0x00): Selects which register to access
/// - IOWIN (offset 0x10): Read/write window for the selected register
pub fn handle_ioapic_access<C: VmContext>(
    ctx: &mut C,
    gpa: u64,
    qual: EptViolationQualification,
) -> ExitHandlerResult {
    let offset = (gpa - IOAPIC_BASE) as u32;

    // Read the instruction to decode the access
    let rip = match ctx.state().vmcs.read_natural(VmcsFieldNatural::GuestRip) {
        Ok(v) => v,
        Err(e) => return ExitHandlerResult::Error(ExitError::VmcsReadError(e)),
    };

    // Translate guest virtual address (RIP) to guest physical address
    let instr_gpa = match translate_gva_to_gpa(ctx, rip) {
        Ok(gpa) => gpa,
        Err(_) => {
            return ExitHandlerResult::Error(ExitError::Fatal(
                "Failed to translate I/O APIC instruction address",
            ));
        }
    };

    // Read instruction bytes from guest memory
    let mut instr_bytes = [0u8; 15];
    if ctx.read_guest_memory(instr_gpa, &mut instr_bytes).is_err() {
        return ExitHandlerResult::Error(ExitError::Fatal("Failed to read I/O APIC instruction"));
    }

    let decoded = match decode_instruction(&instr_bytes) {
        Ok(d) => d,
        Err(_) => {
            return ExitHandlerResult::Error(ExitError::Fatal(
                "Failed to decode I/O APIC instruction",
            ));
        }
    };

    if qual.read {
        // I/O APIC read
        let value = match offset {
            0x00 => {
                // IOREGSEL - return current register select value
                ctx.state().devices.ioapic.ioregsel
            }
            0x10 => {
                // IOWIN - read from selected register
                read_ioapic_register(&ctx.state().devices.ioapic)
            }
            _ => {
                log_warn!("I/O APIC read: unknown offset {:#x}", offset);
                0
            }
        };

        // Write value to destination register
        set_gpr_value(&mut ctx.state_mut().gprs, decoded.register, value as u64);
    } else if qual.write {
        // I/O APIC write - get value from source register
        let value = get_gpr_value(&ctx.state().gprs, decoded.register) as u32;

        match offset {
            0x00 => {
                // IOREGSEL - set register select
                ctx.state_mut().devices.ioapic.ioregsel = value;
            }
            0x10 => {
                // IOWIN - write to selected register
                write_ioapic_register(ctx, value);
            }
            _ => {
                log_warn!(
                    "I/O APIC write: unknown offset {:#x} value {:#x}",
                    offset,
                    value
                );
            }
        }
    }

    // Advance RIP past the instruction
    let new_rip = rip + decoded.length as u64;
    if ctx
        .state()
        .vmcs
        .write_natural(VmcsFieldNatural::GuestRip, new_rip)
        .is_err()
    {
        return ExitHandlerResult::Error(ExitError::Fatal(
            "Failed to advance RIP for I/O APIC access",
        ));
    }

    ExitHandlerResult::Continue
}

/// Read from an I/O APIC register via the IOWIN window.
fn read_ioapic_register(ioapic: &IoApicState) -> u32 {
    let reg = ioapic.ioregsel;
    match reg {
        IOAPIC_REG_ID => ioapic.id,
        IOAPIC_REG_VER => {
            // Version register: bits 7:0 = version (0x20), bits 23:16 = max redir entry (23)
            0x00170020
        }
        IOAPIC_REG_ARB => {
            // Arbitration ID (bits 27:24) - use same as ID
            ioapic.id
        }
        _ if (IOAPIC_REG_REDTBL_BASE..IOAPIC_REG_REDTBL_BASE + 48).contains(&reg) => {
            // Redirection table entry (24 entries, 2 registers each)
            let entry_idx = ((reg - IOAPIC_REG_REDTBL_BASE) / 2) as usize;
            let is_high = (reg - IOAPIC_REG_REDTBL_BASE) % 2 == 1;

            if entry_idx < IOAPIC_NUM_PINS {
                let entry = ioapic.redtbl[entry_idx];
                if is_high {
                    (entry >> 32) as u32
                } else {
                    entry as u32
                }
            } else {
                0
            }
        }
        _ => {
            log_warn!("I/O APIC read: unknown register {:#x}", reg);
            0
        }
    }
}

/// Write to an I/O APIC register via the IOWIN window.
fn write_ioapic_register<C: VmContext>(ctx: &mut C, value: u32) {
    let reg = ctx.state().devices.ioapic.ioregsel;
    match reg {
        IOAPIC_REG_ID => {
            // Only bits 27:24 are writable
            ctx.state_mut().devices.ioapic.id = value & 0x0F00_0000;
        }
        IOAPIC_REG_VER | IOAPIC_REG_ARB => {
            // Read-only registers
        }
        _ if (IOAPIC_REG_REDTBL_BASE..IOAPIC_REG_REDTBL_BASE + 48).contains(&reg) => {
            // Redirection table entry
            let entry_idx = ((reg - IOAPIC_REG_REDTBL_BASE) / 2) as usize;
            let is_high = (reg - IOAPIC_REG_REDTBL_BASE) % 2 == 1;

            if entry_idx < IOAPIC_NUM_PINS {
                let entry = &mut ctx.state_mut().devices.ioapic.redtbl[entry_idx];
                if is_high {
                    // High 32 bits: destination field
                    *entry = (*entry & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
                } else {
                    // Low 32 bits: vector, delivery mode, mask, etc.
                    *entry = (*entry & 0xFFFF_FFFF_0000_0000) | (value as u64);
                }
                log_info!(
                    "I/O APIC: redtbl[{}] = {:#018x} (vector={}, masked={})",
                    entry_idx,
                    ctx.state().devices.ioapic.redtbl[entry_idx],
                    ctx.state().devices.ioapic.redtbl[entry_idx] & 0xFF,
                    (ctx.state().devices.ioapic.redtbl[entry_idx] >> 16) & 1
                );
            }
        }
        _ => {
            log_warn!(
                "I/O APIC write: unknown register {:#x} value {:#x}",
                reg,
                value
            );
        }
    }
}
