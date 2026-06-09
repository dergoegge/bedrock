// SPDX-License-Identifier: GPL-2.0

//! `HYPERCALL_GET_RANDOM` emulation state.
//!
//! Backs the guest's `/dev/urandom` / `/dev/random` / `getrandom()` chokepoint
//! (`get_random_bytes_user`, patched to issue `HYPERCALL_GET_RANDOM` instead of
//! trapping RDRAND). Two modes mirror [`RdrandState`](super::RdrandState):
//!
//! 1. **SeededRng**: fill from an in-VM xorshift64 PRNG — fully deterministic,
//!    no userspace round-trip.
//! 2. **ExitToUserspace**: exit on each request so userspace (the fuzzer) can
//!    supply the exact bytes — every byte handed to guest userspace is then
//!    fuzzer-controlled and replayable, and the request's *size* and *PID* are
//!    surfaced to the fuzzer (which a bare RDRAND trap cannot communicate).

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::rdrand::RdrandMode;

/// Maximum bytes served by a single `HYPERCALL_GET_RANDOM`. Larger guest reads
/// are split into chunks of this size by the guest loop, so one request never
/// needs an unbounded reply buffer. 256 bytes covers the common cases (key
/// material, nonces, Go-runtime reads) in one shot while keeping the reply
/// buffer small enough to live inline in `VmState`.
pub const RANDOM_REPLY_MAX: usize = 256;

/// State for the `HYPERCALL_GET_RANDOM` random source.
#[derive(Clone)]
pub struct RandomState {
    /// Emulation mode. Shares [`RdrandMode`] with the RDRAND device; the two
    /// are configured together so guest randomness is either consistently
    /// seeded (deterministic standalone runs) or consistently fuzzer-served.
    pub mode: RdrandMode,
    /// xorshift64 state for `SeededRng` mode.
    pub seed: u64,
    /// Whether a request is awaiting userspace-supplied bytes (ExitToUserspace).
    pub awaiting: bool,
    /// GVA of the guest destination buffer for the in-flight request.
    pub buf_gva: u64,
    /// Bytes requested by the in-flight request (capped at `RANDOM_REPLY_MAX`).
    pub req_len: u32,
    /// PID (tgid) of the process that issued the in-flight request.
    pub pid: u32,
    /// Reply bytes staged by userspace; valid slice is `reply[..reply_len]`.
    pub reply: [u8; RANDOM_REPLY_MAX],
    /// Number of valid bytes in `reply`.
    pub reply_len: u32,
    /// Whether `reply` has been staged for the in-flight request.
    pub reply_valid: bool,
}

impl Default for RandomState {
    fn default() -> Self {
        Self {
            mode: RdrandMode::SeededRng,
            seed: 0x9e37_79b9_7f4a_7c15,
            awaiting: false,
            buf_gva: 0,
            req_len: 0,
            pid: 0,
            reply: [0u8; RANDOM_REPLY_MAX],
            reply_len: 0,
            reply_valid: false,
        }
    }
}

impl RandomState {
    /// Configure the mode and (for `SeededRng`) the PRNG seed. Forces a
    /// non-zero xorshift seed and clears any in-flight request.
    pub fn configure(&mut self, mode: RdrandMode, seed: u64) {
        self.mode = mode;
        self.seed = if seed == 0 { 1 } else { seed };
        self.clear_request();
    }

    /// Advance the xorshift64 PRNG and return the next value.
    pub fn next_seeded_u64(&mut self) -> u64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
    }

    /// Record the in-flight request before exiting to userspace.
    pub fn begin_request(&mut self, buf_gva: u64, req_len: u32, pid: u32) {
        self.buf_gva = buf_gva;
        self.req_len = req_len;
        self.pid = pid;
        self.awaiting = true;
        self.reply_valid = false;
        self.reply_len = 0;
    }

    /// Stage userspace-supplied reply bytes for the in-flight request. Bytes
    /// beyond `RANDOM_REPLY_MAX` are dropped (the request was capped anyway).
    pub fn stage_reply(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(RANDOM_REPLY_MAX);
        self.reply[..n].copy_from_slice(&bytes[..n]);
        self.reply_len = n as u32;
        self.reply_valid = true;
    }

    /// Clear in-flight request state after completion (or on (re)configure).
    pub fn clear_request(&mut self) {
        self.awaiting = false;
        self.buf_gva = 0;
        self.req_len = 0;
        self.pid = 0;
        self.reply_len = 0;
        self.reply_valid = false;
    }

    /// Whether the handler must exit to userspace to obtain bytes.
    pub fn needs_userspace_exit(&self) -> bool {
        self.mode == RdrandMode::ExitToUserspace && !self.reply_valid
    }
}

impl StateHash for RandomState {
    fn state_hash(&self) -> u64 {
        // Only the stable, determinism-relevant state: the mode and the PRNG
        // position. The in-flight request fields are transient (cleared at
        // quiescent checkpoints, and identical across replays at the same
        // exit), so they're left out to keep the hash a clean divergence
        // signal.
        let mut h = Xxh64Hasher::new();
        h.write_u8(self.mode as u8);
        h.write_u64(self.seed);
        h.finish()
    }
}

#[cfg(test)]
#[path = "random_tests.rs"]
mod tests;
