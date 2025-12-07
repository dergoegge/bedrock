// SPDX-License-Identifier: GPL-2.0

//! Deterministic VM exit logging for non-determinism diagnosis.
//!
//! This module provides infrastructure for logging VM exits with device
//! state hashes to help identify sources of non-determinism in guest execution.
//!
//! # Overview
//!
//! The logging system captures:
//! - TSC value at each deterministic VM exit
//! - Exit reason and qualification
//! - Hashes of all device states (APIC, serial, IOAPIC, RTC, MTRR, RDRAND)
//! - Full hash of guest memory
//!
//! Log entries are written to a 1MB mmap'd buffer shared with userspace.
//! When the buffer is full, the VM exits to userspace to drain it.

mod entry;
mod hash;

pub use entry::{
    LogEntry, LOG_BUFFER_PAGES, LOG_BUFFER_SIZE, LOG_ENTRY_FLAG_DETERMINISTIC, LOG_ENTRY_SIZE,
    MAX_LOG_ENTRIES,
};
pub use hash::{hash_guest_memory, StateHash, Xxh64Hasher};
