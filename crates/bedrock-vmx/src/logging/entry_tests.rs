// SPDX-License-Identifier: GPL-2.0

use super::*;

#[test]
fn test_log_entry_size() {
    assert_eq!(core::mem::size_of::<LogEntry>(), 512);
}

#[test]
fn test_max_entries() {
    assert_eq!(MAX_LOG_ENTRIES, 2048);
}

#[test]
fn test_buffer_pages() {
    assert_eq!(LOG_BUFFER_PAGES, 256);
}
