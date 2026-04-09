// SPDX-License-Identifier: GPL-2.0

//! Register access methods for VmContext.
//!
//! This module provides the set_registers, get_registers, and helper methods
//! for reading/writing guest registers via VMCS.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::{
    InstructionCounter, VirtualMachineControlStructure, VmGetRegistersError, VmSetRegistersError,
    Vmx,
};

/// Set guest registers from the provided register structs.
///
/// This writes all guest registers to the VMCS and updates the GPR state.
/// The VMCS must be loaded before calling this method.
pub fn set_registers<V, I>(
    state: &mut VmState<V, I>,
    gprs: &GeneralPurposeRegisters,
    control_regs: &ControlRegisters,
    debug_regs: &DebugRegisters,
    segment_regs: &SegmentRegisters,
    descriptor_tables: &DescriptorTableRegisters,
    extended_control_regs: &ExtendedControlRegisters,
    rip: u64,
    rflags: u64,
) -> Result<(), VmSetRegistersError>
where
    V: VirtualMachineControlStructure,
    I: InstructionCounter,
{
    // Update general-purpose registers
    state.gprs = *gprs;

    // Write guest state to control registers
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestRsp, gprs.rsp)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestRip, rip)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestRflags, rflags)
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Fix and write CR0 and CR4 if needed
    {
        let vcpu = <V::M as super::Machine>::V::current_vcpu();
        let fixed_cr0 =
            <V::M as super::Machine>::V::fix_cr0(&control_regs.cr0, &vcpu.capabilities());
        let fixed_cr4 =
            <V::M as super::Machine>::V::fix_cr4(&control_regs.cr4, &vcpu.capabilities());
        state
            .vmcs
            .write_natural(VmcsFieldNatural::GuestCr0, fixed_cr0.bits())
            .map_err(VmSetRegistersError::VmcsWrite)?;
        state
            .vmcs
            .write_natural(VmcsFieldNatural::GuestCr4, fixed_cr4.bits())
            .map_err(VmSetRegistersError::VmcsWrite)?;

        if fixed_cr0.bits() != control_regs.cr0.bits() {
            log_info!(
                "VmContext: CR0 fixed: 0x{:016x} -> 0x{:016x}",
                control_regs.cr0.bits(),
                fixed_cr0.bits()
            );
        }

        if fixed_cr4.bits() != control_regs.cr4.bits() {
            log_info!(
                "VmContext: CR4 fixed: 0x{:016x} -> 0x{:016x}",
                control_regs.cr4.bits(),
                fixed_cr4.bits()
            );
        }
    }

    // CR3 - page table base address
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestCr3, control_regs.cr3.bits())
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Debug registers
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestDr7, debug_regs.dr7)
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Segment registers - selectors
    state
        .vmcs
        .write16(
            VmcsField16::GuestCsSelector,
            segment_regs.cs.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestDsSelector,
            segment_regs.ds.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestEsSelector,
            segment_regs.es.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestFsSelector,
            segment_regs.fs.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestGsSelector,
            segment_regs.gs.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestSsSelector,
            segment_regs.ss.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestLdtrSelector,
            segment_regs.ldtr.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write16(
            VmcsField16::GuestTrSelector,
            segment_regs.tr.selector.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Segment registers - bases
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestCsBase, segment_regs.cs.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestDsBase, segment_regs.ds.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestEsBase, segment_regs.es.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestFsBase, segment_regs.fs.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestGsBase, segment_regs.gs.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestSsBase, segment_regs.ss.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestLdtrBase, segment_regs.ldtr.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestTrBase, segment_regs.tr.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Segment registers - limits
    state
        .vmcs
        .write32(VmcsField32::GuestCsLimit, segment_regs.cs.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestDsLimit, segment_regs.ds.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestEsLimit, segment_regs.es.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestFsLimit, segment_regs.fs.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestGsLimit, segment_regs.gs.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestSsLimit, segment_regs.ss.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestLdtrLimit, segment_regs.ldtr.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestTrLimit, segment_regs.tr.limit)
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Segment registers - access rights
    state
        .vmcs
        .write32(
            VmcsField32::GuestCsAccessRights,
            segment_regs.cs.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestDsAccessRights,
            segment_regs.ds.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestEsAccessRights,
            segment_regs.es.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestFsAccessRights,
            segment_regs.fs.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestGsAccessRights,
            segment_regs.gs.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestSsAccessRights,
            segment_regs.ss.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestLdtrAccessRights,
            segment_regs.ldtr.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestTrAccessRights,
            segment_regs.tr.access_rights.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Descriptor tables
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestGdtrBase, descriptor_tables.gdtr.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestIdtrBase, descriptor_tables.idtr.base)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestGdtrLimit,
            descriptor_tables.gdtr.limit as u32,
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(
            VmcsField32::GuestIdtrLimit,
            descriptor_tables.idtr.limit as u32,
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Extended control registers
    state
        .vmcs
        .write64(
            VmcsField64::GuestIa32Efer,
            extended_control_regs.efer.bits(),
        )
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // SYSENTER MSRs (required for Intel 64 - must be canonical)
    state
        .vmcs
        .write32(VmcsField32::GuestIa32SysenterCs, 0)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestIa32SysenterEsp, 0)
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestIa32SysenterEip, 0)
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Additional MSRs required by Intel SDM Vol 3C Section 28.3.1.1:
    // Even if "load" controls aren't enabled, these fields must be
    // initialized to avoid VM-entry failures.
    state
        .vmcs
        .write64(VmcsField64::GuestIa32Debugctl, 0) // No debug features enabled
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write64(VmcsField64::GuestIa32Pat, 0x0007040600070406) // Default PAT value
        .map_err(VmSetRegistersError::VmcsWrite)?;

    // Guest interruptibility and activity state (required by Intel SDM)
    state
        .vmcs
        .write32(VmcsField32::GuestInterruptibilityState, 0) // Fully interruptible
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write32(VmcsField32::GuestActivityState, 0) // Active state
        .map_err(VmSetRegistersError::VmcsWrite)?;
    state
        .vmcs
        .write_natural(VmcsFieldNatural::GuestPendingDebugExceptions, 0) // No pending debug exceptions
        .map_err(VmSetRegistersError::VmcsWrite)?;

    Ok(())
}

/// Get all guest registers from VMCS and GPR state.
///
/// The VMCS must be loaded before calling this method.
#[allow(clippy::type_complexity)]
pub fn get_registers<V, I>(
    state: &VmState<V, I>,
) -> Result<
    (
        GeneralPurposeRegisters,
        ControlRegisters,
        DebugRegisters,
        SegmentRegisters,
        DescriptorTableRegisters,
        ExtendedControlRegisters,
        u64, // rip
        u64, // rflags
    ),
    VmGetRegistersError,
>
where
    V: VirtualMachineControlStructure,
    I: InstructionCounter,
{
    // Read GPRs from our cached state
    let gprs = state.gprs;

    // Read control registers from VMCS
    let vmcs = &state.vmcs;
    let cr0 = vmcs
        .read_natural(VmcsFieldNatural::GuestCr0)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let cr3 = vmcs
        .read_natural(VmcsFieldNatural::GuestCr3)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let cr4 = vmcs
        .read_natural(VmcsFieldNatural::GuestCr4)
        .map_err(VmGetRegistersError::VmcsRead)?;

    let control_regs = ControlRegisters {
        cr0: Cr0::new(cr0),
        cr2: Cr2::new(0), // CR2 not stored in VMCS
        cr3: Cr3::new(cr3),
        cr4: Cr4::new(cr4),
        cr8: Cr8::new(0), // CR8 (TPR) not commonly used
    };

    // Read debug registers
    let dr7 = vmcs
        .read_natural(VmcsFieldNatural::GuestDr7)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let debug_regs = DebugRegisters {
        dr0: 0,
        dr1: 0,
        dr2: 0,
        dr3: 0,
        dr6: 0,
        dr7,
    };

    // Read segment registers
    let cs = read_segment(
        vmcs,
        VmcsField16::GuestCsSelector,
        VmcsFieldNatural::GuestCsBase,
        VmcsField32::GuestCsLimit,
        VmcsField32::GuestCsAccessRights,
    )?;
    let ds = read_segment(
        vmcs,
        VmcsField16::GuestDsSelector,
        VmcsFieldNatural::GuestDsBase,
        VmcsField32::GuestDsLimit,
        VmcsField32::GuestDsAccessRights,
    )?;
    let es = read_segment(
        vmcs,
        VmcsField16::GuestEsSelector,
        VmcsFieldNatural::GuestEsBase,
        VmcsField32::GuestEsLimit,
        VmcsField32::GuestEsAccessRights,
    )?;
    let fs = read_segment(
        vmcs,
        VmcsField16::GuestFsSelector,
        VmcsFieldNatural::GuestFsBase,
        VmcsField32::GuestFsLimit,
        VmcsField32::GuestFsAccessRights,
    )?;
    let gs = read_segment(
        vmcs,
        VmcsField16::GuestGsSelector,
        VmcsFieldNatural::GuestGsBase,
        VmcsField32::GuestGsLimit,
        VmcsField32::GuestGsAccessRights,
    )?;
    let ss = read_segment(
        vmcs,
        VmcsField16::GuestSsSelector,
        VmcsFieldNatural::GuestSsBase,
        VmcsField32::GuestSsLimit,
        VmcsField32::GuestSsAccessRights,
    )?;
    let tr = read_segment(
        vmcs,
        VmcsField16::GuestTrSelector,
        VmcsFieldNatural::GuestTrBase,
        VmcsField32::GuestTrLimit,
        VmcsField32::GuestTrAccessRights,
    )?;
    let ldtr = read_segment(
        vmcs,
        VmcsField16::GuestLdtrSelector,
        VmcsFieldNatural::GuestLdtrBase,
        VmcsField32::GuestLdtrLimit,
        VmcsField32::GuestLdtrAccessRights,
    )?;

    let segment_regs = SegmentRegisters {
        cs,
        ds,
        es,
        fs,
        gs,
        ss,
        tr,
        ldtr,
    };

    // Read descriptor tables
    let gdtr_base = vmcs
        .read_natural(VmcsFieldNatural::GuestGdtrBase)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let gdtr_limit = vmcs
        .read32(VmcsField32::GuestGdtrLimit)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let idtr_base = vmcs
        .read_natural(VmcsFieldNatural::GuestIdtrBase)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let idtr_limit = vmcs
        .read32(VmcsField32::GuestIdtrLimit)
        .map_err(VmGetRegistersError::VmcsRead)?;

    let descriptor_tables = DescriptorTableRegisters {
        gdtr: Gdtr::new(gdtr_base, gdtr_limit as u16),
        idtr: Idtr::new(idtr_base, idtr_limit as u16),
    };

    // Read EFER
    let efer = vmcs
        .read64(VmcsField64::GuestIa32Efer)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let extended_control = ExtendedControlRegisters {
        efer: Efer::new(efer),
    };

    // Read RIP and RFLAGS
    let rip = vmcs
        .read_natural(VmcsFieldNatural::GuestRip)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let rflags = vmcs
        .read_natural(VmcsFieldNatural::GuestRflags)
        .map_err(VmGetRegistersError::VmcsRead)?;

    Ok((
        gprs,
        control_regs,
        debug_regs,
        segment_regs,
        descriptor_tables,
        extended_control,
        rip,
        rflags,
    ))
}

/// Helper to read a segment register from VMCS.
fn read_segment<V: VirtualMachineControlStructure>(
    vmcs: &V,
    sel_field: VmcsField16,
    base_field: VmcsFieldNatural,
    limit_field: VmcsField32,
    ar_field: VmcsField32,
) -> Result<SegmentRegister, VmGetRegistersError> {
    let selector = vmcs
        .read16(sel_field)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let base = vmcs
        .read_natural(base_field)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let limit = vmcs
        .read32(limit_field)
        .map_err(VmGetRegistersError::VmcsRead)?;
    let ar = vmcs
        .read32(ar_field)
        .map_err(VmGetRegistersError::VmcsRead)?;
    Ok(SegmentRegister::new(selector, ar, limit, base))
}
