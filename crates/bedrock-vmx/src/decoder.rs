// SPDX-License-Identifier: GPL-2.0

//! Minimal x86-64 instruction decoder for MMIO emulation.
//!
//! This module decodes MOV instructions that access memory, which is needed
//! for software MMIO emulation (e.g., APIC register access). It only handles
//! the common instruction patterns used for MMIO operations.
//!
//! Intel SDM Vol 2A/2B contains the full instruction encoding reference.

/// Decoded instruction information for MMIO emulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedInstruction {
    /// Length of the instruction in bytes.
    pub length: u8,
    /// Type of memory operation.
    pub operation: MemoryOperation,
    /// Register operand (index into GPR array: 0=RAX, 1=RCX, 2=RDX, 3=RBX, etc.)
    pub register: u8,
    /// Operand size in bytes (1, 2, 4, or 8).
    pub operand_size: u8,
}

/// Type of memory operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryOperation {
    /// Load from memory to register.
    Load,
    /// Store from register to memory.
    Store,
}

/// Errors from instruction decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Instruction buffer too short.
    BufferTooShort,
    /// Unknown or unsupported opcode.
    UnsupportedOpcode,
    /// Unsupported addressing mode (not memory operand).
    UnsupportedAddressing,
}

/// REX prefix bits (Intel SDM Vol 2A, Section 2.2.1).
struct RexPrefix {
    /// REX.W - 64-bit operand size
    w: bool,
    /// REX.R - Extension of ModR/M reg field
    r: bool,
    /// REX.X - Extension of SIB index field
    #[allow(dead_code)]
    x: bool,
    /// REX.B - Extension of ModR/M r/m field or SIB base
    b: bool,
}

impl RexPrefix {
    fn from_byte(byte: u8) -> Option<Self> {
        if (byte & 0xF0) == 0x40 {
            Some(Self {
                w: byte & 0x08 != 0,
                r: byte & 0x04 != 0,
                x: byte & 0x02 != 0,
                b: byte & 0x01 != 0,
            })
        } else {
            None
        }
    }
}

/// Decode an x86-64 instruction for MMIO emulation.
///
/// This decoder handles the common patterns used for MMIO register access:
/// - `MOV r32/r64, [mem]` - load from memory
/// - `MOV [mem], r32/r64` - store to memory
///
/// # Arguments
///
/// * `bytes` - Instruction bytes starting at the instruction to decode
///
/// # Returns
///
/// * `Ok(DecodedInstruction)` - Successfully decoded instruction
/// * `Err(DecodeError)` - Decoding failed
pub fn decode_instruction(bytes: &[u8]) -> Result<DecodedInstruction, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::BufferTooShort);
    }

    let mut pos = 0;

    // Skip legacy prefixes (operand-size, address-size, segment overrides)
    // These appear before REX in x86-64
    let mut has_operand_size_prefix = false;
    while pos < bytes.len() {
        match bytes[pos] {
            // Operand-size override (16-bit operand in 32/64-bit mode)
            0x66 => {
                has_operand_size_prefix = true;
                pos += 1;
            }
            // Address-size override
            0x67 => pos += 1,
            // Segment overrides (rarely used in long mode for MMIO)
            0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 => pos += 1,
            // LOCK, REP prefixes (unusual for MMIO)
            0xF0 | 0xF2 | 0xF3 => pos += 1,
            _ => break,
        }
    }

    if pos >= bytes.len() {
        return Err(DecodeError::BufferTooShort);
    }

    // Check for REX prefix (0x40-0x4F)
    let rex = RexPrefix::from_byte(bytes[pos]);
    if rex.is_some() {
        pos += 1;
    }
    let rex = rex.unwrap_or(RexPrefix {
        w: false,
        r: false,
        x: false,
        b: false,
    });

    if pos >= bytes.len() {
        return Err(DecodeError::BufferTooShort);
    }

    // Determine operand size
    let operand_size = if rex.w {
        8 // REX.W = 64-bit
    } else if has_operand_size_prefix {
        2 // 0x66 prefix = 16-bit
    } else {
        4 // Default = 32-bit
    };

    let opcode = bytes[pos];
    pos += 1;

    // Handle the opcode
    match opcode {
        // MOV r32/r64, r/m32/r/m64 (8B /r)
        // Load from memory to register
        0x8B => {
            if pos >= bytes.len() {
                return Err(DecodeError::BufferTooShort);
            }
            let modrm = bytes[pos];
            pos += 1;

            let reg = ((modrm >> 3) & 0x07) | (if rex.r { 0x08 } else { 0 });
            let mod_bits = modrm >> 6;
            let rm = (modrm & 0x07) | (if rex.b { 0x08 } else { 0 });

            // Calculate ModR/M length (displacement bytes)
            let modrm_len = modrm_displacement_length(mod_bits, rm & 0x07, &bytes[pos..])?;
            pos += modrm_len;

            // mod=11 means register-to-register, not memory
            if mod_bits == 0b11 {
                return Err(DecodeError::UnsupportedAddressing);
            }

            Ok(DecodedInstruction {
                length: pos as u8,
                operation: MemoryOperation::Load,
                register: reg,
                operand_size,
            })
        }

        // MOV r/m32/r/m64, r32/r64 (89 /r)
        // Store from register to memory
        0x89 => {
            if pos >= bytes.len() {
                return Err(DecodeError::BufferTooShort);
            }
            let modrm = bytes[pos];
            pos += 1;

            let reg = ((modrm >> 3) & 0x07) | (if rex.r { 0x08 } else { 0 });
            let mod_bits = modrm >> 6;
            let rm = (modrm & 0x07) | (if rex.b { 0x08 } else { 0 });

            let modrm_len = modrm_displacement_length(mod_bits, rm & 0x07, &bytes[pos..])?;
            pos += modrm_len;

            if mod_bits == 0b11 {
                return Err(DecodeError::UnsupportedAddressing);
            }

            Ok(DecodedInstruction {
                length: pos as u8,
                operation: MemoryOperation::Store,
                register: reg,
                operand_size,
            })
        }

        // MOV r8, r/m8 (8A /r)
        0x8A => {
            if pos >= bytes.len() {
                return Err(DecodeError::BufferTooShort);
            }
            let modrm = bytes[pos];
            pos += 1;

            let reg = ((modrm >> 3) & 0x07) | (if rex.r { 0x08 } else { 0 });
            let mod_bits = modrm >> 6;
            let rm = (modrm & 0x07) | (if rex.b { 0x08 } else { 0 });

            let modrm_len = modrm_displacement_length(mod_bits, rm & 0x07, &bytes[pos..])?;
            pos += modrm_len;

            if mod_bits == 0b11 {
                return Err(DecodeError::UnsupportedAddressing);
            }

            Ok(DecodedInstruction {
                length: pos as u8,
                operation: MemoryOperation::Load,
                register: reg,
                operand_size: 1,
            })
        }

        // MOV r/m8, r8 (88 /r)
        0x88 => {
            if pos >= bytes.len() {
                return Err(DecodeError::BufferTooShort);
            }
            let modrm = bytes[pos];
            pos += 1;

            let reg = ((modrm >> 3) & 0x07) | (if rex.r { 0x08 } else { 0 });
            let mod_bits = modrm >> 6;
            let rm = (modrm & 0x07) | (if rex.b { 0x08 } else { 0 });

            let modrm_len = modrm_displacement_length(mod_bits, rm & 0x07, &bytes[pos..])?;
            pos += modrm_len;

            if mod_bits == 0b11 {
                return Err(DecodeError::UnsupportedAddressing);
            }

            Ok(DecodedInstruction {
                length: pos as u8,
                operation: MemoryOperation::Store,
                register: reg,
                operand_size: 1,
            })
        }

        // MOVZX r32/r64, r/m8 (0F B6 /r)
        // MOVZX r32/r64, r/m16 (0F B7 /r)
        0x0F => {
            if pos >= bytes.len() {
                return Err(DecodeError::BufferTooShort);
            }
            let opcode2 = bytes[pos];
            pos += 1;

            match opcode2 {
                0xB6 | 0xB7 => {
                    if pos >= bytes.len() {
                        return Err(DecodeError::BufferTooShort);
                    }
                    let modrm = bytes[pos];
                    pos += 1;

                    let reg = ((modrm >> 3) & 0x07) | (if rex.r { 0x08 } else { 0 });
                    let mod_bits = modrm >> 6;
                    let rm = (modrm & 0x07) | (if rex.b { 0x08 } else { 0 });

                    let modrm_len =
                        modrm_displacement_length(mod_bits, rm & 0x07, &bytes[pos..])?;
                    pos += modrm_len;

                    if mod_bits == 0b11 {
                        return Err(DecodeError::UnsupportedAddressing);
                    }

                    // Source operand size (memory)
                    let mem_operand_size = if opcode2 == 0xB6 { 1 } else { 2 };

                    Ok(DecodedInstruction {
                        length: pos as u8,
                        operation: MemoryOperation::Load,
                        register: reg,
                        // For MOVZX, we return the source (memory) operand size
                        // The emulation needs to know to zero-extend
                        operand_size: mem_operand_size,
                    })
                }
                _ => Err(DecodeError::UnsupportedOpcode),
            }
        }

        _ => Err(DecodeError::UnsupportedOpcode),
    }
}

/// Calculate the displacement length for a ModR/M byte.
///
/// This handles SIB byte presence and displacement sizes.
fn modrm_displacement_length(mod_bits: u8, rm: u8, remaining: &[u8]) -> Result<usize, DecodeError> {
    let mut len = 0;

    // Check for SIB byte (rm == 4 with mod != 11)
    let has_sib = rm == 4 && mod_bits != 0b11;
    if has_sib {
        if remaining.is_empty() {
            return Err(DecodeError::BufferTooShort);
        }
        len += 1;

        // Check SIB base for additional displacement
        let sib = remaining[0];
        let base = sib & 0x07;

        // If mod=00 and base=5 (EBP/RBP), there's a 32-bit displacement
        if mod_bits == 0b00 && base == 5 {
            if remaining.len() < len + 4 {
                return Err(DecodeError::BufferTooShort);
            }
            len += 4;
        }
    }

    // Displacement based on mod bits
    match mod_bits {
        0b00 => {
            // mod=00: no displacement, except:
            // - rm=5 (RIP-relative in 64-bit mode) has 32-bit disp
            // - SIB with base=5 already handled above
            if rm == 5 && !has_sib {
                if remaining.len() < len + 4 {
                    return Err(DecodeError::BufferTooShort);
                }
                len += 4;
            }
        }
        0b01 => {
            // mod=01: 8-bit displacement
            if remaining.len() < len + 1 {
                return Err(DecodeError::BufferTooShort);
            }
            len += 1;
        }
        0b10 => {
            // mod=10: 32-bit displacement
            if remaining.len() < len + 4 {
                return Err(DecodeError::BufferTooShort);
            }
            len += 4;
        }
        _ => {} // mod=11: register (no memory operand)
    }

    Ok(len)
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
