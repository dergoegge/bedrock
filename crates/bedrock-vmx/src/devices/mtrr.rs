// SPDX-License-Identifier: GPL-2.0

//! MTRR (Memory Type Range Registers) emulation.
//!
//! Stores the guest's MTRR register values. Bedrock currently exposes these
//! values back to the guest but does not use them to compute EPT memory types;
//! EPT mappings are created as write-back elsewhere. Following bhyve's approach.

#[cfg(not(feature = "cargo"))]
use super::super::logging::{StateHash, Xxh64Hasher};
#[cfg(feature = "cargo")]
use crate::logging::{StateHash, Xxh64Hasher};

/// Number of variable range MTRRs (like bhyve's VMM_MTRR_VAR_MAX).
pub const MTRR_VAR_MAX: usize = 10;

/// MTRR (Memory Type Range Registers) state for guest emulation.
///
/// Stores the guest's MTRR register values. Bedrock currently exposes these
/// values back to the guest but does not use them to compute EPT memory types;
/// EPT mappings are created as write-back elsewhere. Following bhyve's approach.
#[derive(Clone, Debug)]
pub struct MtrrState {
    /// MTRRdefType (0x2FF) - default memory type and enable bits.
    pub def_type: u64,
    /// Fixed-range MTRRs for 4K regions (0x268-0x26F).
    pub fixed_4k: [u64; 8],
    /// Fixed-range MTRRs for 16K regions (0x258-0x259).
    pub fixed_16k: [u64; 2],
    /// Fixed-range MTRR for 64K region (0x250).
    pub fixed_64k: u64,
    /// Variable-range MTRRs (base and mask pairs).
    pub var: [(u64, u64); MTRR_VAR_MAX],
}

impl Default for MtrrState {
    fn default() -> Self {
        Self {
            // Default type: writeback (0x6), MTRRs disabled (bit 11 = 0)
            def_type: 0x6,
            fixed_4k: [0; 8],
            fixed_16k: [0; 2],
            fixed_64k: 0,
            var: [(0, 0); MTRR_VAR_MAX],
        }
    }
}

impl StateHash for MtrrState {
    fn state_hash(&self) -> u64 {
        let mut h = Xxh64Hasher::new();
        h.write_u64(self.def_type);
        for &val in &self.fixed_4k {
            h.write_u64(val);
        }
        for &val in &self.fixed_16k {
            h.write_u64(val);
        }
        h.write_u64(self.fixed_64k);
        for &(base, mask) in &self.var {
            h.write_u64(base);
            h.write_u64(mask);
        }
        h.finish()
    }
}
