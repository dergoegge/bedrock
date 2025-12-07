// SPDX-License-Identifier: GPL-2.0

//! EPT violation exit handler and GVA-to-GPA translation.

use super::apic::{
    handle_apic_access, handle_ioapic_access, APIC_BASE, APIC_SIZE, IOAPIC_BASE, IOAPIC_SIZE,
};
use super::helpers::ExitHandlerResult;
use super::qualifications::EptViolationQualification;
use super::reasons::ExitReason;

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Handle EPT violation exit.
pub fn handle_ept_violation<C: VmContext, A: CowAllocator<C::CowPage>>(
    ctx: &mut C,
    qual: EptViolationQualification,
    allocator: &mut A,
) -> ExitHandlerResult {
    // Read guest physical address that caused the violation
    let guest_phys = ctx
        .state()
        .vmcs
        .read64(VmcsField64::GuestPhysicalAddr)
        .unwrap_or(0);

    // Check if this is a local APIC access
    if guest_phys >= APIC_BASE && guest_phys < APIC_BASE + APIC_SIZE {
        return handle_apic_access(ctx, guest_phys, qual);
    }

    // Check if this is an I/O APIC access
    if guest_phys >= IOAPIC_BASE && guest_phys < IOAPIC_BASE + IOAPIC_SIZE {
        return handle_ioapic_access(ctx, guest_phys, qual);
    }

    // Check for copy-on-write fault: write to non-writable page
    // This is the common case for forked VMs where pages start as R+X
    if qual.write && !qual.writable {
        if let Some(result) = ctx.handle_cow_fault(GuestPhysAddr::new(guest_phys), allocator) {
            return result;
        }
        // Pool exhausted - exit to refill in sleepable context
        return ExitHandlerResult::ExitToUserspace(ExitReason::PoolExhausted);
    }

    // Read guest linear address (if valid) for logging
    let _guest_linear = if qual.guest_linear_valid {
        ctx.state()
            .vmcs
            .read_natural(VmcsFieldNatural::GuestLinearAddr)
            .unwrap_or(0)
    } else {
        0
    };

    // Read RIP for context
    let _rip = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestRip)
        .unwrap_or(0);

    // Log detailed EPT violation info
    log_err!(
        "EPT violation: GPA={:#x}, GLA={:#x}, RIP={:#x}\n",
        guest_phys,
        _guest_linear,
        _rip
    );
    log_err!(
        "  Access: read={}, write={}, execute={}\n",
        qual.read,
        qual.write,
        qual.execute
    );
    log_err!(
        "  EPT permissions: readable={}, writable={}, executable={}\n",
        qual.readable,
        qual.writable,
        qual.executable
    );

    // EPT violations need to be handled by userspace (for memory mapping)
    // or by the EPT subsystem. For now, exit to userspace.
    ExitHandlerResult::ExitToUserspace(ExitReason::EptViolation)
}

/// Page size constant for GVA translation.
const PAGE_SIZE: u64 = 4096;

/// Translate a guest virtual address range to an array of guest physical addresses.
///
/// This translates each page of a GVA range to its corresponding GPA, handling
/// ranges that span multiple pages. The resulting GPAs are page-aligned.
///
/// # Arguments
///
/// * `ctx` - VM context for reading guest memory and VMCS
/// * `gva` - Guest virtual address (start of buffer)
/// * `size` - Size in bytes of the buffer
/// * `gpas` - Output array to store page-aligned GPAs
///
/// # Returns
///
/// * `Ok(num_pages)` - Number of pages translated (gpas[0..num_pages] are valid)
/// * `Err(())` - Translation failed for one of the pages
pub fn translate_gva_range_to_gpas<C: VmContext>(
    ctx: &C,
    gva: u64,
    size: u64,
    gpas: &mut [u64],
) -> Result<usize, ()> {
    if size == 0 {
        return Ok(0);
    }

    // Calculate the number of pages needed
    let start_page = gva & !0xFFF; // Page-align start
    let end_addr = gva.checked_add(size.saturating_sub(1)).ok_or(())?;
    let end_page = end_addr & !0xFFF;
    let num_pages = ((end_page - start_page) / PAGE_SIZE + 1) as usize;

    if num_pages > gpas.len() {
        return Err(());
    }

    // Translate each page
    for i in 0..num_pages {
        let page_gva = start_page + (i as u64 * PAGE_SIZE);
        let gpa = translate_gva_to_gpa(ctx, page_gva)?;
        gpas[i] = gpa.as_u64() & !0xFFF; // Store page-aligned GPA
    }

    Ok(num_pages)
}

/// Translate a guest virtual address to guest physical address.
///
/// This walks the guest's page tables (4-level paging) to translate
/// a guest virtual address to a guest physical address.
///
/// # Arguments
///
/// * `ctx` - VM context for reading guest memory and VMCS
/// * `gva` - Guest virtual address to translate
///
/// # Returns
///
/// * `Ok(GuestPhysAddr)` - The translated guest physical address
/// * `Err(())` - Translation failed (page not present, etc.)
pub fn translate_gva_to_gpa<C: VmContext>(ctx: &C, gva: u64) -> Result<GuestPhysAddr, ()> {
    // Read guest CR3 to get PML4 base address
    let cr3 = ctx
        .state()
        .vmcs
        .read_natural(VmcsFieldNatural::GuestCr3)
        .map_err(|_| ())?;

    // PML4 physical address (bits 51:12 of CR3, with PCID/etc masked out)
    let pml4_addr = cr3 & 0x000F_FFFF_FFFF_F000;

    // Extract page table indices from virtual address
    let pml4_index = ((gva >> 39) & 0x1FF) as usize;
    let pdpt_index = ((gva >> 30) & 0x1FF) as usize;
    let pd_index = ((gva >> 21) & 0x1FF) as usize;
    let pt_index = ((gva >> 12) & 0x1FF) as usize;
    let page_offset = gva & 0xFFF;

    // Read PML4 entry
    let pml4e_addr = GuestPhysAddr::new(pml4_addr + (pml4_index * 8) as u64);
    let mut buf = [0u8; 8];
    ctx.read_guest_memory(pml4e_addr, &mut buf)
        .map_err(|_| ())?;
    let pml4e = u64::from_le_bytes(buf);

    // Check present bit
    if pml4e & 1 == 0 {
        return Err(());
    }

    // Get PDPT address
    let pdpt_addr = pml4e & 0x000F_FFFF_FFFF_F000;

    // Read PDPT entry
    let pdpte_addr = GuestPhysAddr::new(pdpt_addr + (pdpt_index * 8) as u64);
    ctx.read_guest_memory(pdpte_addr, &mut buf)
        .map_err(|_| ())?;
    let pdpte = u64::from_le_bytes(buf);

    if pdpte & 1 == 0 {
        return Err(());
    }

    // Check for 1GB page (PS bit = bit 7)
    if pdpte & (1 << 7) != 0 {
        let page_base = pdpte & 0x000F_FFFF_C000_0000; // 1GB aligned
        let offset_1g = gva & 0x3FFF_FFFF; // 30-bit offset
        return Ok(GuestPhysAddr::new(page_base | offset_1g));
    }

    // Get PD address
    let pd_addr = pdpte & 0x000F_FFFF_FFFF_F000;

    // Read PD entry
    let pde_addr = GuestPhysAddr::new(pd_addr + (pd_index * 8) as u64);
    ctx.read_guest_memory(pde_addr, &mut buf).map_err(|_| ())?;
    let pde = u64::from_le_bytes(buf);

    if pde & 1 == 0 {
        return Err(());
    }

    // Check for 2MB page (PS bit = bit 7)
    if pde & (1 << 7) != 0 {
        let page_base = pde & 0x000F_FFFF_FFE0_0000; // 2MB aligned
        let offset_2m = gva & 0x1F_FFFF; // 21-bit offset
        return Ok(GuestPhysAddr::new(page_base | offset_2m));
    }

    // Get PT address
    let pt_addr = pde & 0x000F_FFFF_FFFF_F000;

    // Read PT entry
    let pte_addr = GuestPhysAddr::new(pt_addr + (pt_index * 8) as u64);
    ctx.read_guest_memory(pte_addr, &mut buf).map_err(|_| ())?;
    let pte = u64::from_le_bytes(buf);

    if pte & 1 == 0 {
        return Err(());
    }

    // 4KB page
    let page_base = pte & 0x000F_FFFF_FFFF_F000;
    Ok(GuestPhysAddr::new(page_base | page_offset))
}
