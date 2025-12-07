// SPDX-License-Identifier: GPL-2.0

//! ELF kernel loading.

use std::io;

use goblin::elf::Elf;

use log::debug;

/// Load an ELF kernel into guest memory.
///
/// Returns (entry_point, kernel_end_address).
pub fn load_kernel(memory: &mut [u8], kernel_data: &[u8]) -> io::Result<(u64, usize)> {
    let elf = Elf::parse(kernel_data).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ELF parse error: {}", e),
        )
    })?;

    if elf.header.e_machine != goblin::elf::header::EM_X86_64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Not an x86_64 ELF file",
        ));
    }

    debug!("  ELF type: {:?}", elf.header.e_type);
    debug!("  Entry point: {:#x}", elf.entry);
    debug!("  Program headers: {}", elf.program_headers.len());

    let mut kernel_end: usize = 0;

    // Load each PT_LOAD segment
    for (i, phdr) in elf.program_headers.iter().enumerate() {
        if phdr.p_type != goblin::elf::program_header::PT_LOAD {
            continue;
        }

        let file_offset = phdr.p_offset as usize;
        let file_size = phdr.p_filesz as usize;
        let mem_size = phdr.p_memsz as usize;
        let load_addr = phdr.p_paddr as usize;

        debug!(
            "  Segment {}: load {:#x}-{:#x} (file offset {:#x}, {} bytes, mem {} bytes)",
            i,
            load_addr,
            load_addr + mem_size,
            file_offset,
            file_size,
            mem_size
        );

        // Track highest address used by kernel
        kernel_end = kernel_end.max(load_addr + mem_size);

        // Validate addresses
        if load_addr + mem_size > memory.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Segment {} extends beyond guest memory ({:#x} > {:#x})",
                    i,
                    load_addr + mem_size,
                    memory.len()
                ),
            ));
        }

        // Copy file content to memory
        if file_size > 0 {
            let src = &kernel_data[file_offset..file_offset + file_size];
            let dst = &mut memory[load_addr..load_addr + file_size];
            dst.copy_from_slice(src);
        }

        // Zero-fill the remainder (BSS section)
        if mem_size > file_size {
            let bss = &mut memory[load_addr + file_size..load_addr + mem_size];
            bss.fill(0);
        }
    }

    Ok((elf.entry, kernel_end))
}
