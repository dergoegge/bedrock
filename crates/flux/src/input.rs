// SPDX-License-Identifier: GPL-2.0

//! The fuzzer's input type.
//!
//! An [`Input`] is a serializable mirror of [`bedrock_lab::InputRecording`]:
//! the `RDRAND`/`RDSEED` values and host-driven bash actions a branch consumed
//! along the path from the root checkpoint to its current position. Mutators
//! rewrite this sequence; the campaign feeds the mutated suffix back through an
//! [`InputSource`] on a freshly-rewound branch.
//!
//! Each entry carries its consumption time as a raw retired-instruction count.
//! Frequencies are tree-wide (every checkpoint shares one), so storing the
//! frequency per entry would be redundant — it's attached when converting to
//! lab types.

use bedrock_lab::{BashTarget, InputSource, IoInput, VirtTime};
use serde::{Deserialize, Serialize};

use crate::rng::SplitMix64;

/// One `RDRAND`/`RDSEED` value the guest consumed at a given virtual time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RngVal {
    /// Retired-instruction count at consumption time.
    pub at: u64,
    pub value: u64,
}

/// One bash action injected at a given virtual time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IoAction {
    /// Retired-instruction count at injection time.
    pub at: u64,
    pub target: Target,
    pub command: String,
}

/// Serializable mirror of [`BashTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    Host,
    Container(String),
}

impl From<&Target> for BashTarget {
    fn from(t: &Target) -> Self {
        match t {
            Target::Host => BashTarget::Host,
            Target::Container(name) => BashTarget::Container(name.clone()),
        }
    }
}

impl From<&BashTarget> for Target {
    fn from(t: &BashTarget) -> Self {
        match t {
            BashTarget::Host => Target::Host,
            BashTarget::Container(name) => Target::Container(name.clone()),
        }
    }
}

/// A complete path of consumed RDRAND values and bash actions for one branch.
/// The two vectors stay sorted by `at` so the suffix at or after a given time
/// can be sliced cheaply.
///
/// `anchor_at` is the virtual time (in instructions) of the checkpoint this
/// input describes — the floor for any newly-inserted entries, since there's
/// no point inserting earlier than the earliest checkpoint we could rewind to.
///
/// `mutated_at` is a one-shot hint written by mutators and consumed by the
/// campaign: the earliest virtual time touched by the most recent mutation,
/// which becomes the rewind target (or, when `mutated_at >= anchor_at`, the
/// "start serving from here" point on a no-rewind forward branch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub rng: Vec<RngVal>,
    pub io: Vec<IoAction>,
    pub anchor_at: u64,
    pub mutated_at: Option<u64>,
}

/// A self-contained, on-disk reproduction of one branch: the full sequence of
/// `RDRAND` values and bash actions the guest consumed from the fuzzing root
/// (the discovery checkpoint) up to the bug, plus the timing metadata needed to
/// replay it deterministically. Written on a solution and consumed by
/// `flux --reproduce`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reproduction {
    /// TSC frequency the times are expressed against.
    pub frequency: u64,
    /// Virtual time (instructions) of the replay origin — the discovery
    /// checkpoint a fresh boot reaches. Input before this is the deterministic
    /// boot/discovery prefix and is recreated by booting, not replayed.
    pub root_instr: u64,
    /// Virtual time (instructions) of the bug checkpoint — replay runs to here.
    pub bug_instr: u64,
    /// Why this was flagged a solution (the objective that fired).
    pub reason: String,
    /// The consumed-input recording (cumulative from the absolute root; the
    /// boot prefix is skipped at replay via `source_from(root_instr)`).
    pub input: Input,
}

impl Input {
    /// An empty input anchored at the given virtual time (in retired
    /// instructions).
    pub fn new(anchor_at: u64) -> Self {
        Self {
            rng: Vec::new(),
            io: Vec::new(),
            anchor_at,
            mutated_at: None,
        }
    }

    /// Build an [`InputSource`] feeding the suffix of this input at or after
    /// `at_instr`. Earlier entries are skipped — they've already been consumed
    /// in the rewound checkpoint's COW state. `fresh_seed` seeds a
    /// deterministic per-branch RNG that serves `RDRAND`/`RDSEED` traps firing
    /// *past* the recorded values, so the guest never hits `RngExhausted`;
    /// those fresh values land in the new branch's recording for future
    /// mutation.
    pub fn source_from(&self, at_instr: u64, frequency: u64, fresh_seed: u64) -> InputCursor {
        let rng_values: Vec<u64> = self
            .rng
            .iter()
            .skip_while(|r| r.at < at_instr)
            .map(|r| r.value)
            .collect();
        let io: Vec<IoInput> = self
            .io
            .iter()
            .skip_while(|i| i.at < at_instr)
            .map(|i| IoInput {
                at: VirtTime::from_instructions(i.at, frequency),
                target: BashTarget::from(&i.target),
                command: i.command.clone(),
            })
            .collect();
        InputCursor {
            rng_values,
            io,
            rng_pos: 0,
            io_pos: 0,
            fresh: SplitMix64::new(fresh_seed),
        }
    }
}

/// [`InputSource`] backed by an [`Input`] suffix.
///
/// `next_rng_u64` serves recorded values in order, then falls back to a
/// deterministic per-source RNG so the guest never exhausts. `next_io_input`
/// serves io entries in slot order, then `None`. Cloning produces an
/// independent cursor over the same content (with identically-seeded fresh
/// RNG) — the per-branch independence the lab requires.
#[derive(Clone)]
pub struct InputCursor {
    rng_values: Vec<u64>,
    io: Vec<IoInput>,
    rng_pos: usize,
    io_pos: usize,
    fresh: SplitMix64,
}

impl InputSource for InputCursor {
    fn next_rng_u64(&mut self) -> Option<u64> {
        let v = self
            .rng_values
            .get(self.rng_pos)
            .copied()
            .unwrap_or_else(|| self.fresh.next_u64());
        self.rng_pos += 1;
        Some(v)
    }

    fn next_io_input(&mut self) -> Option<IoInput> {
        let v = self.io.get(self.io_pos).cloned()?;
        self.io_pos += 1;
        Some(v)
    }

    fn clone_box(&self) -> Box<dyn InputSource> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproduction_json_roundtrips() {
        let input = Input {
            rng: vec![
                RngVal { at: 100, value: 0xdead_beef },
                RngVal { at: 250, value: 42 },
            ],
            io: vec![
                IoAction {
                    at: 120,
                    target: Target::Container("lnd1".into()),
                    command: "/opt/bedrock/drivers/lnd-force-close deadbeef".into(),
                },
                IoAction {
                    at: 300,
                    target: Target::Host,
                    command: "fault-injector partition btcd2".into(),
                },
            ],
            anchor_at: 90,
            mutated_at: None,
        };
        let repro = Reproduction {
            frequency: 2_995_200_000,
            root_instr: 50,
            bug_instr: 400,
            reason: "serial reported: container died".into(),
            input,
        };
        let json = serde_json::to_string(&repro).unwrap();
        let back: Reproduction = serde_json::from_str(&json).unwrap();
        assert_eq!(back.frequency, repro.frequency);
        assert_eq!(back.root_instr, repro.root_instr);
        assert_eq!(back.bug_instr, repro.bug_instr);
        assert_eq!(back.reason, repro.reason);
        assert_eq!(back.input, repro.input);
    }
}
