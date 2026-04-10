// SPDX-License-Identifier: GPL-2.0

//! Serial port (8250/16550 UART) emulation for COM1 at 0x3F8.
//!
//! The 8250 driver probes the UART by testing registers, particularly the
//! scratch register (SCR). We need to track writable registers so reads
//! return previously written values.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Maximum size of serial input buffer.
pub const SERIAL_INPUT_MAX_SIZE: usize = 256;

/// Serial port (8250/16550 UART) state for COM1 at 0x3F8.
///
/// The 8250 driver probes the UART by testing registers, particularly the
/// scratch register (SCR). We need to track writable registers so reads
/// return previously written values.
#[derive(Clone, Debug)]
pub struct SerialState {
    /// Interrupt Enable Register (0x3F9) - controls which interrupts are enabled
    pub ier: u8,
    /// Line Control Register (0x3FB) - controls data format
    pub lcr: u8,
    /// Modem Control Register (0x3FC) - controls modem signals
    pub mcr: u8,
    /// Scratch Register (0x3FF) - general purpose, used for UART detection
    pub scr: u8,
    /// Divisor Latch Low (when DLAB=1, 0x3F8)
    pub dll: u8,
    /// Divisor Latch High (when DLAB=1, 0x3F9)
    pub dlh: u8,
    /// Input buffer for data to be read by guest.
    pub input_buf: [u8; SERIAL_INPUT_MAX_SIZE],
    /// Current read position in input buffer.
    pub input_pos: usize,
    /// Length of valid data in input buffer.
    pub input_len: usize,
}

impl Default for SerialState {
    fn default() -> Self {
        Self {
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            dll: 0,
            dlh: 0,
            input_buf: [0u8; SERIAL_INPUT_MAX_SIZE],
            input_pos: 0,
            input_len: 0,
        }
    }
}

impl SerialState {
    /// Set the input buffer with data that will be returned when guest reads RBR.
    pub fn set_input(&mut self, data: &[u8]) {
        let len = data.len().min(SERIAL_INPUT_MAX_SIZE);
        self.input_buf[..len].copy_from_slice(&data[..len]);
        self.input_pos = 0;
        self.input_len = len;
    }

    /// Read a byte from the input buffer (used by RBR reads).
    /// Returns 0 if no data available.
    pub fn read_input(&mut self) -> u8 {
        if self.input_pos < self.input_len {
            let byte = self.input_buf[self.input_pos];
            self.input_pos += 1;
            byte
        } else {
            0
        }
    }

    /// Check if input data is available.
    pub fn has_input(&self) -> bool {
        self.input_pos < self.input_len
    }
}

impl StateHash for SerialState {
    fn state_hash(&self) -> u64 {
        let mut h = Xxh64Hasher::new();
        h.write_u8(self.ier);
        h.write_u8(self.lcr);
        h.write_u8(self.mcr);
        h.write_u8(self.scr);
        h.write_u8(self.dll);
        h.write_u8(self.dlh);
        // Hash input buffer state
        h.write_bytes(&self.input_buf[..self.input_len]);
        h.write_u64(self.input_pos as u64);
        h.write_u64(self.input_len as u64);
        h.finish()
    }
}
