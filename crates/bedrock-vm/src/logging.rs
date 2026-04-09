// SPDX-License-Identifier: GPL-2.0

//! Deterministic exit logging support.
//!
//! This module provides the LogEntry struct for parsing log buffer data and
//! utilities for writing log entries to JSONL files.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

/// Size of each log entry in bytes.
pub const LOG_ENTRY_SIZE: usize = 512;

/// Maximum number of log entries that fit in the 1MB buffer.
pub const MAX_LOG_ENTRIES: usize = super::LOG_BUFFER_SIZE / LOG_ENTRY_SIZE;

/// Flag bit: entry represents a deterministic exit.
pub const LOG_ENTRY_FLAG_DETERMINISTIC: u32 = 1;

/// A log entry written by the hypervisor for each VM exit.
///
/// This struct must match the kernel's LogEntry layout exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    // Exit info (24 bytes)
    /// Emulated TSC value at the time of exit.
    pub tsc: u64,
    /// Exit reason (ExitReason as u32).
    pub exit_reason: u32,
    /// Flags bitfield. Bit 0 = deterministic exit.
    pub flags: u32,
    /// Exit qualification (interpretation depends on exit reason).
    pub exit_qualification: u64,

    // Guest registers (144 bytes)
    /// RAX register.
    pub rax: u64,
    /// RCX register.
    pub rcx: u64,
    /// RDX register.
    pub rdx: u64,
    /// RBX register.
    pub rbx: u64,
    /// RSP register.
    pub rsp: u64,
    /// RBP register.
    pub rbp: u64,
    /// RSI register.
    pub rsi: u64,
    /// RDI register.
    pub rdi: u64,
    /// R8 register.
    pub r8: u64,
    /// R9 register.
    pub r9: u64,
    /// R10 register.
    pub r10: u64,
    /// R11 register.
    pub r11: u64,
    /// R12 register.
    pub r12: u64,
    /// R13 register.
    pub r13: u64,
    /// R14 register.
    pub r14: u64,
    /// R15 register.
    pub r15: u64,
    /// RIP (instruction pointer).
    pub rip: u64,
    /// RFLAGS register.
    pub rflags: u64,

    // Device state hashes (56 bytes)
    /// Hash of APIC state.
    pub apic_hash: u64,
    /// Hash of serial port state.
    pub serial_hash: u64,
    /// Hash of I/O APIC state.
    pub ioapic_hash: u64,
    /// Hash of RTC state.
    pub rtc_hash: u64,
    /// Hash of MTRR state.
    pub mtrr_hash: u64,
    /// Hash of RDRAND state.
    pub rdrand_hash: u64,
    /// Hash of guest memory.
    pub memory_hash: u64,

    // Additional guest state (80 bytes)
    /// FS base address from VMCS.
    pub fs_base: u64,
    /// GS base address from VMCS.
    pub gs_base: u64,
    /// Kernel GS base (IA32_KERNEL_GS_BASE MSR).
    pub kernel_gs_base: u64,
    /// CR3 (page table root) from VMCS.
    pub cr3: u64,
    /// CS base address from VMCS.
    pub cs_base: u64,
    /// DS base address from VMCS.
    pub ds_base: u64,
    /// ES base address from VMCS.
    pub es_base: u64,
    /// SS base address from VMCS.
    pub ss_base: u64,
    /// Pending debug exceptions from VMCS.
    pub pending_dbg_exceptions: u64,
    /// Guest interruptibility state from VMCS.
    pub interruptibility_state: u32,
    /// Number of COW pages at time of exit.
    pub cow_page_count: u32,

    // Padding (208 bytes)
    /// Padding to reach 512 bytes.
    #[serde(skip)]
    pub _padding: [u64; 26],
}

impl LogEntry {
    /// Returns true if this entry represents a deterministic exit.
    pub fn is_deterministic(&self) -> bool {
        self.flags & LOG_ENTRY_FLAG_DETERMINISTIC != 0
    }

    /// Parse log entries from a raw buffer.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Raw log buffer from the kernel
    /// * `count` - Number of entries to parse (from VmExit.log_entry_count)
    ///
    /// # Returns
    ///
    /// A slice of LogEntry structs. Returns an empty slice if count is 0 or buffer is too small.
    pub fn from_buffer(buffer: &[u8], count: usize) -> &[LogEntry] {
        if count == 0 {
            return &[];
        }

        let count = count.min(MAX_LOG_ENTRIES);
        let required_size = count * LOG_ENTRY_SIZE;

        if buffer.len() < required_size {
            return &[];
        }

        // SAFETY: LogEntry is repr(C) with size 128 bytes, and we've verified
        // the buffer is large enough and count > 0. The buffer comes from mmap
        // which guarantees proper alignment.
        unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const LogEntry, count) }
    }

    /// Get the exit reason as a string.
    pub fn exit_reason_str(&self) -> &'static str {
        match self.exit_reason {
            0 => "EXCEPTION_NMI",
            10 => "CPUID",
            12 => "HLT",
            16 => "RDTSC",
            28 => "CR_ACCESS",
            30 => "IO_INSTRUCTION",
            31 => "MSR_READ",
            32 => "MSR_WRITE",
            36 => "MWAIT",
            39 => "MONITOR",
            48 => "EPT_VIOLATION",
            51 => "RDTSCP",
            55 => "XSETBV",
            57 => "RDRAND",
            61 => "RDSEED",
            258 => "VMCALL_SHUTDOWN",
            260 => "VMCALL_SNAPSHOT",
            _ => "OTHER",
        }
    }
}

/// Write log entries to a JSONL (JSON Lines) writer.
///
/// Each entry is written as a single JSON object on its own line.
///
/// # Arguments
///
/// * `writer` - Any type implementing Write
/// * `entries` - Slice of log entries to write
///
/// # Returns
///
/// The number of entries written.
pub fn write_jsonl<W: Write>(writer: &mut W, entries: &[LogEntry]) -> io::Result<usize> {
    for entry in entries {
        serde_json::to_writer(&mut *writer, entry)
            .map_err(io::Error::other)?;
        writeln!(writer)?;
    }
    Ok(entries.len())
}

/// Write log entries to a JSONL file.
///
/// # Arguments
///
/// * `path` - Path to the output JSONL file
/// * `entries` - Slice of log entries to write
///
/// # Returns
///
/// The number of entries written.
pub fn write_jsonl_file(path: &str, entries: &[LogEntry]) -> io::Result<usize> {
    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);
    write_jsonl(&mut writer, entries)
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
