// SPDX-License-Identifier: GPL-2.0

//! A tiny, fast, deterministic PRNG.
//!
//! We need exactly three things from a fuzzer RNG — a raw 64-bit draw, an
//! unbiased index into a range, and a float in `[0, 1)` — so this is
//! `xoshiro256**` (fast, well-distributed, 256-bit state) seeded through
//! `SplitMix64`. Both are public domain and stable across platforms, so a seed
//! reproduces a run exactly.

/// `xoshiro256**` generator. `Clone` is intentional: an [`InputCursor`]
/// clones its fresh-RNG so a re-forked branch serves the identical
/// out-of-recording stream.
///
/// [`InputCursor`]: crate::input::InputCursor
#[derive(Clone)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seed the generator. Any seed (including 0) is fine — `SplitMix64`
    /// expands it into a well-mixed 256-bit state.
    pub fn new(seed: u64) -> Self {
        let mut sm = SplitMix64::new(seed);
        Self {
            s: [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()],
        }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform index in `[0, n)`. Returns 0 for `n <= 1`. Uses Lemire's
    /// multiply-shift, which is unbiased enough for fuzzing and faster than
    /// rejection-sampled modulo.
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }

    /// Float in `[0, 1)` with 53 bits of entropy.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// True with probability `pct/100`.
    #[inline]
    pub fn chance(&mut self, pct: u64) -> bool {
        (self.below(100) as u64) < pct
    }

    /// True/false with equal probability.
    #[inline]
    pub fn coin(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

/// `SplitMix64` — used to seed [`Rng`] and to serve `RDRAND` traps that fire
/// past a recorded input's suffix (see [`InputCursor`]). Small, branch-free,
/// reproducible from the seed alone.
///
/// [`InputCursor`]: crate::input::InputCursor
#[derive(Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
