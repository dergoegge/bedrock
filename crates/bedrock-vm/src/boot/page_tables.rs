// SPDX-License-Identifier: GPL-2.0

//! Page table setup for identity-mapped guest memory.

use super::constants::memory::{PAGE_SIZE, PDPT_HIGH_ADDR, PDPT_LOW_ADDR, PD_ADDR, PML4_ADDR};
use super::constants::pte::{PAGE_SIZE_2MB, PRESENT, WRITABLE};

/// Set up identity-mapped page tables covering all guest memory.
///
/// Uses 2MB pages for efficiency. Maps both low addresses (identity)
/// and high kernel addresses (0xFFFFFFFF80000000+) to the same physical memory.
pub fn setup_page_tables(memory: &mut [u8], memory_size: usize) {
    let write_u64 = |mem: &mut [u8], offset: usize, value: u64| {
        mem[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    };

    let gb = 1024 * 1024 * 1024usize;
    let mb2 = 2 * 1024 * 1024usize;

    // Use 2MB pages - need one PD per GB
    let num_gb = (memory_size + gb - 1) / gb;
    let num_gb = num_gb.max(4); // Map at least 4GB for kernel

    // Clear page table area (PML4 through all PDs we'll use)
    let pt_end = PD_ADDR as usize + num_gb * PAGE_SIZE;
    for i in PML4_ADDR as usize..pt_end {
        if i < memory.len() {
            memory[i] = 0;
        }
    }

    // PML4[0] -> PDPT_LOW (for identity mapping 0-512GB)
    write_u64(
        memory,
        PML4_ADDR as usize,
        PDPT_LOW_ADDR | PRESENT | WRITABLE,
    );

    // PML4[511] -> PDPT_HIGH (for kernel high addresses 0xFFFFFFFF80000000+)
    write_u64(
        memory,
        PML4_ADDR as usize + 511 * 8,
        PDPT_HIGH_ADDR | PRESENT | WRITABLE,
    );

    // Set up PDPT_LOW entries - each points to a PD (1GB each)
    for i in 0..num_gb {
        let pd_addr = PD_ADDR + (i * PAGE_SIZE) as u64;
        write_u64(
            memory,
            PDPT_LOW_ADDR as usize + i * 8,
            pd_addr | PRESENT | WRITABLE,
        );
    }

    // Set up PDPT_HIGH entries for kernel virtual address space
    // 0xFFFFFFFF80000000 is in PDPT index 510 (the last 2GB before end)
    for i in 0..num_gb.min(2) {
        let pd_addr = PD_ADDR + (i * PAGE_SIZE) as u64;
        write_u64(
            memory,
            PDPT_HIGH_ADDR as usize + (510 + i) * 8,
            pd_addr | PRESENT | WRITABLE,
        );
    }

    // Set up PD entries - each PD covers 1GB with 512 x 2MB pages
    for gb_idx in 0..num_gb {
        let pd_base = PD_ADDR as usize + gb_idx * PAGE_SIZE;
        for i in 0..512 {
            let phys_addr = ((gb_idx * 512 + i) * mb2) as u64;
            if phys_addr < memory_size as u64 {
                write_u64(
                    memory,
                    pd_base + i * 8,
                    phys_addr | PRESENT | WRITABLE | PAGE_SIZE_2MB,
                );
            }
        }
    }
}
