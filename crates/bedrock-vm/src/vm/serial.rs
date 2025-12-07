// SPDX-License-Identifier: GPL-2.0

//! Serial buffer types and parsing utilities.

/// Maximum size of serial output buffer (1 page).
pub const SERIAL_BUFFER_SIZE: usize = 4096;

/// Log buffer size (1MB).
pub const LOG_BUFFER_SIZE: usize = 1024 * 1024;

/// Size of the serial TSC metadata page.
pub const SERIAL_TSC_PAGE_SIZE: usize = 4096;

/// Magic value to identify line TSC metadata format.
pub const SERIAL_METADATA_MAGIC: u16 = 0xCAFE;

/// Offset where line TSC entries start in the TSC page (after 4-byte header).
pub const SERIAL_LINE_TSC_OFFSET: usize = 4;

/// Serial input buffer passed to the kernel via ioctl.
///
/// Maximum size is SERIAL_INPUT_MAX_SIZE bytes.
#[repr(C)]
pub struct SerialInput {
    /// Length of valid data in buf.
    pub len: u32,
    /// Reserved for alignment.
    pub _reserved: u32,
    /// Input data buffer.
    pub buf: [u8; SERIAL_INPUT_MAX_SIZE],
}

/// Maximum size of serial input buffer.
pub const SERIAL_INPUT_MAX_SIZE: usize = 256;

/// A line TSC entry containing the byte offset where a line starts and its TSC.
#[derive(Clone, Copy, Debug)]
pub struct LineTscEntry {
    /// Byte offset in the serial buffer where this line starts.
    pub offset: u16,
    /// Emulated TSC when this line started being written.
    pub tsc: u64,
}

/// Parse line TSC entries from the serial TSC metadata page.
///
/// The TSC page layout is:
/// - Bytes 0-1: line_count (u16)
/// - Bytes 2-3: magic (u16, 0xCAFE)
/// - Bytes 4+: line entries (10 bytes each: u16 offset + u64 tsc)
///
/// Returns a vector of entries if the metadata format is valid,
/// or None if the page doesn't contain valid metadata.
pub fn parse_line_tsc_entries(tsc_page: &[u8]) -> Option<Vec<LineTscEntry>> {
    if tsc_page.len() < SERIAL_TSC_PAGE_SIZE {
        return None;
    }

    // Read metadata header at offset 0
    let line_count = u16::from_le_bytes([tsc_page[0], tsc_page[1]]) as usize;
    let magic = u16::from_le_bytes([tsc_page[2], tsc_page[3]]);

    // Check magic value
    if magic != SERIAL_METADATA_MAGIC {
        return None;
    }

    // Parse line entries
    let mut entries = Vec::with_capacity(line_count);
    for i in 0..line_count {
        let entry_offset = SERIAL_LINE_TSC_OFFSET + i * 10;
        if entry_offset + 10 > SERIAL_TSC_PAGE_SIZE {
            break; // Entry would exceed page
        }

        let offset = u16::from_le_bytes([tsc_page[entry_offset], tsc_page[entry_offset + 1]]);

        let tsc = u64::from_le_bytes([
            tsc_page[entry_offset + 2],
            tsc_page[entry_offset + 3],
            tsc_page[entry_offset + 4],
            tsc_page[entry_offset + 5],
            tsc_page[entry_offset + 6],
            tsc_page[entry_offset + 7],
            tsc_page[entry_offset + 8],
            tsc_page[entry_offset + 9],
        ]);

        entries.push(LineTscEntry { offset, tsc });
    }

    Some(entries)
}
