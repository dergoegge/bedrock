// SPDX-License-Identifier: GPL-2.0

//! Length-preserving byte havoc.
//!
//! The guest consumes randomness as raw *bytes* — mixed into the entropy pool,
//! sliced by callers, compared bit by bit — so a recorded `RDRAND` stream
//! deserves the same treatment as any fuzz buffer: incremental bit/byte edits
//! that hit the boundaries real code branches on (0, 1, MAX, sign flips),
//! preserving the progress coverage-guided fuzzing relies on, rather than
//! uniform replacement.
//!
//! Every mutator here is *length-preserving*, so when the RNG mutator flattens
//! N recorded values to `8*N` bytes, runs this havoc, and slices back, the
//! value↔`at` mapping stays intact. [`havoc_bytes`] applies a random stack of
//! `2^(1..=4)` of them.

use crate::rng::Rng;

/// Max magnitude of an arithmetic add/sub step (AFL's `ARITH_MAX`).
const ARITH_MAX: u64 = 35;

const INTERESTING_8: [i8; 9] = [-128, -1, 0, 1, 16, 32, 64, 100, 127];
const INTERESTING_16: [i16; 19] = [
    -128, -1, 0, 1, 16, 32, 64, 100, 127, // 8-bit
    -32768, -129, 128, 255, 256, 512, 1000, 1024, 4096, 32767,
];
const INTERESTING_32: [i32; 27] = [
    -128,
    -1,
    0,
    1,
    16,
    32,
    64,
    100,
    127, // 8-bit
    -32768,
    -129,
    128,
    255,
    256,
    512,
    1000,
    1024,
    4096,
    32767, // 16-bit
    -2147483648,
    -100663046,
    -32769,
    32768,
    65535,
    65536,
    100663045,
    2147483647,
];

/// A byte mutator: edits `buf` in place, returns whether it changed anything.
type ByteMut = fn(&mut Rng, &mut [u8]) -> bool;

const MUTATORS: &[ByteMut] = &[
    bit_flip,
    byte_flip,
    byte_inc,
    byte_dec,
    byte_neg,
    byte_rand,
    byte_add,
    word_add,
    dword_add,
    qword_add,
    interesting_8,
    interesting_16,
    interesting_32,
    rand_set,
];

/// Apply a random stack of `2^(1 + below(4))` ∈ {2,4,8,16} byte mutators to
/// `buf`. Returns whether any of them changed a byte.
pub fn havoc_bytes(rng: &mut Rng, buf: &mut [u8]) -> bool {
    if buf.is_empty() {
        return false;
    }
    let iters = 1usize << (1 + rng.below(4));
    let mut changed = false;
    for _ in 0..iters {
        let m = MUTATORS[rng.below(MUTATORS.len())];
        changed |= m(rng, buf);
    }
    changed
}

fn bit_flip(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] ^= 1 << rng.below(8);
    true
}

fn byte_flip(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] ^= 0xff;
    true
}

fn byte_inc(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] = buf[i].wrapping_add(1);
    true
}

fn byte_dec(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] = buf[i].wrapping_sub(1);
    true
}

fn byte_neg(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] = buf[i].wrapping_neg();
    true
}

/// XOR a random byte with a nonzero value — guaranteed to change it.
fn byte_rand(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] ^= 1 + rng.below(255) as u8;
    true
}

/// Add/subtract a small value to a byte.
fn byte_add(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    let delta = 1 + rng.below(ARITH_MAX as usize) as u8;
    buf[i] = if rng.coin() {
        buf[i].wrapping_add(delta)
    } else {
        buf[i].wrapping_sub(delta)
    };
    true
}

/// Add/subtract a small value to a little-endian word at a random offset.
fn word_add(rng: &mut Rng, buf: &mut [u8]) -> bool {
    if buf.len() < 2 {
        return false;
    }
    let i = rng.below(buf.len() - 1);
    let delta = (1 + rng.below(ARITH_MAX as usize)) as u16;
    let v = u16::from_le_bytes([buf[i], buf[i + 1]]);
    let v = if rng.coin() {
        v.wrapping_add(delta)
    } else {
        v.wrapping_sub(delta)
    };
    buf[i..i + 2].copy_from_slice(&v.to_le_bytes());
    true
}

fn dword_add(rng: &mut Rng, buf: &mut [u8]) -> bool {
    if buf.len() < 4 {
        return false;
    }
    let i = rng.below(buf.len() - 3);
    let delta = (1 + rng.below(ARITH_MAX as usize)) as u32;
    let v = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
    let v = if rng.coin() {
        v.wrapping_add(delta)
    } else {
        v.wrapping_sub(delta)
    };
    buf[i..i + 4].copy_from_slice(&v.to_le_bytes());
    true
}

fn qword_add(rng: &mut Rng, buf: &mut [u8]) -> bool {
    if buf.len() < 8 {
        return false;
    }
    let i = rng.below(buf.len() - 7);
    let delta = 1 + rng.below(ARITH_MAX as usize) as u64;
    let v = u64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
    let v = if rng.coin() {
        v.wrapping_add(delta)
    } else {
        v.wrapping_sub(delta)
    };
    buf[i..i + 8].copy_from_slice(&v.to_le_bytes());
    true
}

fn interesting_8(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let i = rng.below(buf.len());
    buf[i] = INTERESTING_8[rng.below(INTERESTING_8.len())] as u8;
    true
}

fn interesting_16(rng: &mut Rng, buf: &mut [u8]) -> bool {
    if buf.len() < 2 {
        return false;
    }
    let i = rng.below(buf.len() - 1);
    let v = INTERESTING_16[rng.below(INTERESTING_16.len())] as u16;
    buf[i..i + 2].copy_from_slice(&v.to_le_bytes());
    true
}

fn interesting_32(rng: &mut Rng, buf: &mut [u8]) -> bool {
    if buf.len() < 4 {
        return false;
    }
    let i = rng.below(buf.len() - 3);
    let v = INTERESTING_32[rng.below(INTERESTING_32.len())] as u32;
    buf[i..i + 4].copy_from_slice(&v.to_le_bytes());
    true
}

/// Set a random-length run to a single random byte value.
fn rand_set(rng: &mut Rng, buf: &mut [u8]) -> bool {
    let len = buf.len();
    let start = rng.below(len);
    let end = start + 1 + rng.below(len - start);
    let val = rng.below(256) as u8;
    for b in &mut buf[start..end] {
        *b = val;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn havoc_preserves_length_and_changes_bytes() {
        let mut rng = Rng::new(0xfeed);
        let original = vec![0u8; 64];
        let mut buf = original.clone();
        let mut ever_changed = false;
        for _ in 0..100 {
            buf.copy_from_slice(&original);
            let changed = havoc_bytes(&mut rng, &mut buf);
            assert_eq!(buf.len(), original.len(), "length must be preserved");
            if changed {
                ever_changed = true;
            }
        }
        assert!(ever_changed, "havoc should change bytes over 100 tries");
    }

    #[test]
    fn empty_buffer_is_a_noop() {
        let mut rng = Rng::new(1);
        let mut buf: Vec<u8> = Vec::new();
        assert!(!havoc_bytes(&mut rng, &mut buf));
    }
}
