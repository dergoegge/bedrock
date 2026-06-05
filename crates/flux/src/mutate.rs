// SPDX-License-Identifier: GPL-2.0

//! Mutators that rewrite an [`Input`].
//!
//! The set reflects two asymmetries between RNG and IO inputs:
//!
//! - **RNG values are pull-based.** The guest decides when it calls
//!   `RDRAND`/`RDSEED`; we can't synthesize new consumption events, only
//!   rewrite the bytes of values it already asked for ([`rng_byte_havoc`]).
//! - **IO actions are push-based.** The host decides when bash commands fire,
//!   so new actions can be inserted, retimed, retargeted, or dropped at any
//!   virtual time past the anchor.
//!
//! Each mutation records the earliest virtual time it touched into
//! `input.mutated_at` so the campaign knows where to rewind (or forward from)
//! before serving the mutated suffix. A [`Mutators`] applies a random stack of
//! `2^(1..=max_stack_pow)` of them per call.

use bedrock_lab::WorkloadDriver;

use crate::bytemut::havoc_bytes;
use crate::input::{Input, IoAction, Target};
use crate::rng::Rng;

/// Outcome of a mutation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationResult {
    Mutated,
    Skipped,
}

impl MutationResult {
    fn or(self, other: MutationResult) -> MutationResult {
        if self == MutationResult::Mutated || other == MutationResult::Mutated {
            MutationResult::Mutated
        } else {
            MutationResult::Skipped
        }
    }
}

/// How [`Mutators::io_insert`] selects the actions in a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwarmMode {
    /// Each corpus entry carries a fixed action subset its branches draw from
    /// and its children inherit-with-drift, so a lineage explores one coherent
    /// regime (e.g. never closing channels, so they mature). The default.
    Lineage,
    /// A fresh random subset per insert call.
    Burst,
    /// The full vocabulary every time.
    Off,
}

/// Auto-heal durations offered for partitions, passed straight through as the
/// fault-injector's `--duration` value. Every partition is timed — the server
/// heals it on its own — so the fuzzer never needs to schedule a `clear`. The
/// spread brackets the default run-window range (0.5–5 virtual seconds): the
/// short ones heal mid-branch (a self-contained split→heal → reorg-and-recovery
/// within one input), while the long one outlives a single branch yet still
/// self-clears, so an inherited split never wedges descendant branches forever.
/// Coverage feedback selects whichever land in a productive regime.
const FAULT_DURATIONS: &[&str] = &["500ms", "2s", "10s"];

/// One host- or container-driven action the IO mutators can schedule.
///
/// The discovered workload drivers plus workload-agnostic fault injection.
/// Fault actions are plain host-side bash commands handed to the in-guest
/// fault-injector server, which tracks the live faults. Partitions are timed and
/// heal themselves, so no ordering is required; `clear` stays a defined action
/// but the mutators never schedule it (see [`Action::insertable`]).
#[derive(Clone)]
pub enum Action {
    /// Invoke a discovered driver inside its container.
    Driver(WorkloadDriver),
    /// Network-partition the named container for `duration` of guest time, after
    /// which the server heals it automatically — a self-contained split→heal
    /// that drives a reorg without a paired `clear`.
    FaultPartition {
        container: String,
        duration: &'static str,
    },
    /// Clear every fault the server is tracking. Kept as a defined action (it is
    /// part of the fault-injector's surface), but [`Action::insertable`] is false
    /// for it, so the mutators never insert it — every partition self-heals.
    FaultClear,
}

impl Action {
    /// The `(target, command)` this action lowers to on the I/O channel.
    fn to_io(&self) -> (Target, String) {
        match self {
            Action::Driver(d) => (Target::Container(d.container.clone()), d.driver.clone()),
            Action::FaultPartition {
                container,
                duration,
            } => (
                Target::Host,
                format!("fault-injector partition {container} --duration {duration}"),
            ),
            Action::FaultClear => (Target::Host, "fault-injector clear".to_string()),
        }
    }

    /// Build the default action vocabulary from a discovered workload: every
    /// driver, a timed `partition` per [`FAULT_DURATIONS`] for each non-excluded
    /// container, and one `clear`. `exclude_from_partition` names containers that
    /// legitimately exit on isolation (so partitioning them would look like a
    /// crash). `clear` is included for completeness, but the mutators never
    /// insert it ([`Action::insertable`]) — every partition self-heals.
    pub fn vocabulary(
        drivers: Vec<WorkloadDriver>,
        containers: &[String],
        exclude_from_partition: &[String],
    ) -> Vec<Action> {
        let mut actions: Vec<Action> = drivers.into_iter().map(Action::Driver).collect();
        for c in containers {
            if exclude_from_partition.iter().any(|e| e == c) {
                continue;
            }
            for &d in FAULT_DURATIONS {
                actions.push(Action::FaultPartition {
                    container: c.clone(),
                    duration: d,
                });
            }
        }
        actions.push(Action::FaultClear);
        actions
    }

    /// Whether this action takes a fuzzer-controlled byte argument. Drivers do:
    /// the argument is appended to the command as hex and the driver decodes it
    /// as its entropy source, so the fuzzer steers the driver's parameters
    /// directly (smooth, byte-level, coverage-hill-climbable) instead of via the
    /// guest CRNG, which a recorded-RDRAND mutation can't actually move. Fault
    /// actions take no argument.
    fn wants_arg(&self) -> bool {
        matches!(self, Action::Driver(_))
    }

    /// Whether the mutators may schedule this action. Everything is insertable
    /// except [`Action::FaultClear`]: partitions self-heal after their duration,
    /// so an explicit `clear` is never needed and would only add noise.
    fn insertable(&self) -> bool {
        !matches!(self, Action::FaultClear)
    }
}

/// Bytes the fuzzer hands a driver as its entropy, generated fresh on insert and
/// mutated in place by [`Mutators::io_arg_havoc`]. Long enough for any driver's
/// parameter draws (they index small offsets); kept fixed-length so a byte
/// mutation maps to the same parameter across runs.
const ARG_BYTES: usize = 64;

/// Percent of Lineage-mode bursts that re-roll a fresh regime instead of using
/// the inherited subset — prevents inheritance from ossifying the corpus or
/// locking a lineage out of an action combination. The rest stay coherent.
const LINEAGE_REROLL_PCT: u64 = 25;

/// Of the re-rolls, the percent that use the *full* vocabulary (vs a fresh
/// random subset). Small, so the throughput cost of full-vocab bursts is
/// bounded while still guaranteeing rare full-action combinations get tried.
const FULL_VOCAB_REROLL_PCT: u64 = 25;

/// Lowercase-hex encode.
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Decode an even-length all-lowercase-hex string to bytes, else `None`.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Split a command into `(base, arg_bytes)` if its last whitespace-separated
/// token is an even-length hex string (a fuzzer argument). The command string
/// is the recorded/replayed source of truth, so the argument round-trips
/// through it with no separate field.
fn split_arg(command: &str) -> Option<(&str, Vec<u8>)> {
    let (base, last) = command.rsplit_once(' ')?;
    let bytes = hex_decode(last)?;
    Some((base, bytes))
}

/// Append a hex argument to a base command.
fn with_arg(base: &str, arg: &[u8]) -> String {
    format!("{base} {}", hex_encode(arg))
}

/// A random subset (1..=n distinct indices, uniform size) of an `n`-action
/// vocabulary. Seeds a fresh lineage regime / serves `burst` mode.
pub fn random_subset(n: usize, rng: &mut Rng) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let k = 1 + rng.below(n);
    let mut pool: Vec<usize> = (0..n).collect();
    for i in 0..k {
        let j = i + rng.below(n - i);
        pool.swap(i, j);
    }
    pool.truncate(k);
    pool.sort_unstable();
    pool
}

/// Derive a child lineage's subset from its parent's by flipping one action's
/// membership with probability `drift_pct` — so a lineage keeps a coherent
/// regime that drifts slowly as it deepens. Always keeps ≥1 action.
pub fn drift_subset(parent: &[usize], n: usize, drift_pct: u64, rng: &mut Rng) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let mut present = vec![false; n];
    for &i in parent {
        if i < n {
            present[i] = true;
        }
    }
    if rng.chance(drift_pct) {
        let flip = rng.below(n);
        present[flip] = !present[flip];
    }
    let out: Vec<usize> = (0..n).filter(|&i| present[i]).collect();
    if out.is_empty() {
        return random_subset(n, rng);
    }
    out
}

/// The structured-mutator set, parameterized by the discovered action
/// vocabulary and the worker's run window.
pub struct Mutators {
    actions: Vec<Action>,
    /// Run-window length in retired instructions; inserted/shifted actions are
    /// scattered across `[anchor, anchor+window)`.
    window: u64,
    /// Upper bound on actions inserted per `io_insert` (drawn in `1..=burst`).
    burst: usize,
    swarm: SwarmMode,
    /// `2^(1..=max_stack_pow)` mutations applied per [`Self::havoc`] call.
    max_stack_pow: u32,
}

impl Mutators {
    pub fn new(actions: Vec<Action>, window: u64, burst: usize, swarm: SwarmMode) -> Self {
        Self {
            actions,
            window,
            burst: burst.max(1),
            swarm,
            max_stack_pow: 2,
        }
    }

    /// Apply a random stack of `2^(1 + below(max_stack_pow))` structured
    /// mutations. `lineage` is the picked entry's swarm subset (used by
    /// `io_insert` in [`SwarmMode::Lineage`]); `None`/empty means unrestricted.
    /// Returns `Mutated` if any sub-mutation applied.
    pub fn havoc(
        &self,
        rng: &mut Rng,
        input: &mut Input,
        lineage: Option<&[usize]>,
    ) -> MutationResult {
        let iters = 1u32 << (1 + rng.below(self.max_stack_pow as usize));
        let mut result = MutationResult::Skipped;
        for _ in 0..iters {
            // io_arg_havoc (the direct parameter knob) gets two of six slots —
            // it's the highest-signal mutation now that drivers read their
            // entropy from the fuzzer-controlled argument.
            let pick = rng.below(6);
            let r = match pick {
                0 => self.rng_byte_havoc(rng, input),
                1 => self.io_insert(rng, input, lineage),
                2 => self.io_time_shift(rng, input),
                3 => self.io_driver_swap(rng, input),
                _ => self.io_arg_havoc(rng, input),
            };
            result = result.or(r);
        }
        result
    }

    /// Forward-only extension: append a burst of actions strictly at or past
    /// the input's anchor, never touching earlier RNG. Because `mutated_at`
    /// lands at the anchor, the campaign takes the no-rewind forward path and
    /// the branch runs *onward* from the picked checkpoint, building a longer
    /// action sequence. The counterpart to [`Self::havoc`], whose
    /// `rng_byte_havoc` component instead pulls `mutated_at` back to an early
    /// time and rewinds; the worker mixes the two so the corpus both extends
    /// sequences and re-explores from earlier points.
    pub fn extend_forward(
        &self,
        rng: &mut Rng,
        input: &mut Input,
        lineage: Option<&[usize]>,
    ) -> MutationResult {
        self.io_insert(rng, input, lineage)
    }

    /// Splice another corpus entry's action sequence (`donor`) onto this input:
    /// graft the donor — a real, coherent sequence some branch actually ran —
    /// starting near the input's anchor, preserving the donor's internal
    /// ordering and relative timing, and run forward from the picked checkpoint.
    /// This combines *building blocks* (e.g. a checkpoint where a force-close
    /// sweep is pending + a sequence that triggers a reorg) that random
    /// insertion almost never assembles together — the classic crossover remedy
    /// for bugs that need a conjunction of independent rare events. Forward
    /// (`mutated_at` at the anchor); no deep rewind.
    pub fn splice(&self, rng: &mut Rng, input: &mut Input, donor: &[IoAction]) -> MutationResult {
        if donor.is_empty() {
            return MutationResult::Skipped;
        }
        let donor_min = donor.iter().map(|a| a.at).min().unwrap_or(0);
        // Land the grafted sequence at a random offset within the window so it
        // falls at varied points after the checkpoint (e.g. while a sweep is
        // still pending).
        let jitter = if self.window == 0 {
            0
        } else {
            rng.below((self.window / 2).max(1) as usize) as u64
        };
        let start = input.anchor_at.saturating_add(jitter);
        for a in donor {
            input.io.push(IoAction {
                at: start.saturating_add(a.at - donor_min),
                target: a.target.clone(),
                command: a.command.clone(),
            });
        }
        input.io.sort_by_key(|e| e.at);
        input.mutated_at = Some(merge_earliest(input.mutated_at, start));
        MutationResult::Mutated
    }

    /// Byte-level havoc over the recorded `RDRAND`/`RDSEED` stream. The number
    /// of values and each value's `at` stay fixed (RNG is pull-based); only the
    /// 8-byte little-endian payloads change. `mutated_at` becomes the earliest
    /// `at` whose bytes actually moved.
    fn rng_byte_havoc(&self, rng: &mut Rng, input: &mut Input) -> MutationResult {
        if input.rng.is_empty() {
            return MutationResult::Skipped;
        }
        let mut buf = Vec::with_capacity(input.rng.len() * 8);
        for r in &input.rng {
            buf.extend_from_slice(&r.value.to_le_bytes());
        }
        if !havoc_bytes(rng, &mut buf) {
            return MutationResult::Skipped;
        }
        let mut earliest: Option<u64> = None;
        for (k, r) in input.rng.iter_mut().enumerate() {
            let v = u64::from_le_bytes(buf[k * 8..k * 8 + 8].try_into().unwrap());
            if v != r.value {
                r.value = v;
                earliest = Some(earliest.map_or(r.at, |e| e.min(r.at)));
            }
        }
        match earliest {
            Some(at) => {
                input.mutated_at = Some(merge_earliest(input.mutated_at, at));
                MutationResult::Mutated
            }
            None => MutationResult::Skipped,
        }
    }

    /// Insert a burst of actions, each at a virtual time drawn uniformly across
    /// the run window so they scatter through the branch rather than bunching
    /// at the start (which would leave the guest idle, HLT-fast-forwarding the
    /// TSC). All inserted times are `>= anchor_at`, so the campaign takes the
    /// forward path; `mutated_at` is the earliest inserted time.
    fn io_insert(
        &self,
        rng: &mut Rng,
        input: &mut Input,
        lineage: Option<&[usize]>,
    ) -> MutationResult {
        if self.actions.is_empty() {
            return MutationResult::Skipped;
        }
        let n = self.actions.len();
        let burst = 1 + rng.below(self.burst);
        // Swarm testing (Groce et al., ISSTA'12): draw the burst from a subset
        // of the vocabulary, not the whole, so the fleet explores distinct
        // regimes (e.g. one subset excludes channel-close so channels mature).
        //
        // In Lineage mode a branch usually draws from its inherited regime, but
        // some branches *re-roll* a fresh subset so pure inheritance can't
        // ossify the corpus or permanently lock a lineage out of an action
        // combination (a fixed subset missing, say, `clear` can never produce a
        // reorg). Most re-rolls are a fresh random subset (keeps coherence +
        // throughput; uniform sizes mean the occasional large one carries rare
        // combos); a small slice is the full vocabulary so even a 4-action
        // combination gets exercised outright — without the throughput collapse
        // of `--swarm off` (full vocab on *every* branch).
        let subset: Vec<usize> = match self.swarm {
            SwarmMode::Lineage => match lineage {
                Some(l) if !l.is_empty() && !rng.chance(LINEAGE_REROLL_PCT) => l.to_vec(),
                _ if rng.chance(FULL_VOCAB_REROLL_PCT) => (0..n).collect(),
                _ => random_subset(n, rng),
            },
            SwarmMode::Burst => random_subset(n, rng),
            SwarmMode::Off => (0..n).collect(),
        };
        // `clear` lives in the vocabulary but is never inserted (partitions
        // self-heal), so drop any non-insertable index the subset picked up.
        let subset: Vec<usize> = subset
            .into_iter()
            .filter(|&i| self.actions[i].insertable())
            .collect();
        if subset.is_empty() {
            return MutationResult::Skipped;
        }
        let mut earliest: Option<u64> = None;
        for _ in 0..burst {
            let idx = pick_action(rng, &subset);
            let (target, mut command) = self.actions[idx].to_io();
            if self.actions[idx].wants_arg() {
                command = with_arg(&command, &rand_arg(rng));
            }
            let offset = if self.window == 0 {
                0
            } else {
                rng.below(self.window as usize) as u64
            };
            let at = input.anchor_at.saturating_add(offset);
            input.io.push(IoAction {
                at,
                target,
                command,
            });
            earliest = Some(earliest.map_or(at, |e| e.min(at)));
        }
        input.io.sort_by_key(|e| e.at);
        if let Some(at) = earliest {
            input.mutated_at = Some(merge_earliest(input.mutated_at, at));
        }
        MutationResult::Mutated
    }

    /// Shift one existing action's firing time by a random signed jitter (up to
    /// the window). Divergence is at the earlier of the old and new times.
    fn io_time_shift(&self, rng: &mut Rng, input: &mut Input) -> MutationResult {
        if input.io.is_empty() {
            return MutationResult::Skipped;
        }
        let max_shift = self.window.max(1);
        let idx = rng.below(input.io.len());
        let old_at = input.io[idx].at;
        let span = max_shift.saturating_mul(2).saturating_add(1);
        let delta = rng.below(span as usize) as u64;
        let new_at = if delta >= max_shift {
            old_at.saturating_add(delta - max_shift)
        } else {
            old_at.saturating_sub(max_shift - delta)
        }
        .max(1);
        input.io[idx].at = new_at;
        input.io.sort_by_key(|e| e.at);
        input.mutated_at = Some(merge_earliest(input.mutated_at, old_at.min(new_at)));
        MutationResult::Mutated
    }

    /// Re-target one existing action to a different vocabulary entry, keeping
    /// its firing time. Divergence is at that action's time.
    fn io_driver_swap(&self, rng: &mut Rng, input: &mut Input) -> MutationResult {
        // Re-target only to insertable actions, so a swap never conjures a
        // `clear` the fuzzer wouldn't otherwise insert.
        let candidates: Vec<usize> = (0..self.actions.len())
            .filter(|&i| self.actions[i].insertable())
            .collect();
        if input.io.is_empty() || candidates.is_empty() {
            return MutationResult::Skipped;
        }
        let idx = rng.below(input.io.len());
        let a = candidates[rng.below(candidates.len())];
        let (target, mut command) = self.actions[a].to_io();
        if self.actions[a].wants_arg() {
            command = with_arg(&command, &rand_arg(rng));
        }
        input.io[idx].target = target;
        input.io[idx].command = command;
        let at = input.io[idx].at;
        input.mutated_at = Some(merge_earliest(input.mutated_at, at));
        MutationResult::Mutated
    }

    /// Mutate the fuzzer-controlled byte argument of one existing driver action
    /// in place — the smooth, targeted parameter knob. Picks an io entry whose
    /// command carries a hex arg, byte-havocs the decoded bytes, and rewrites
    /// the command. Because the argument is replayed verbatim from the command
    /// string, this gives the fuzzer direct, deterministic control over the
    /// driver's parameter draws (block counts, amounts, channel sizes, raw
    /// parser inputs, …) — unlike `rng_byte_havoc`, whose effect is laundered
    /// through the guest CRNG. Divergence is at that action's time, so a deep
    /// action's re-parameterization re-runs the branch forward from there.
    fn io_arg_havoc(&self, rng: &mut Rng, input: &mut Input) -> MutationResult {
        let candidates: Vec<usize> = input
            .io
            .iter()
            .enumerate()
            .filter(|(_, e)| split_arg(&e.command).is_some())
            .map(|(i, _)| i)
            .collect();
        if candidates.is_empty() {
            return MutationResult::Skipped;
        }
        let idx = candidates[rng.below(candidates.len())];
        let (base, mut bytes) = match split_arg(&input.io[idx].command) {
            Some((b, by)) => (b.to_string(), by),
            None => return MutationResult::Skipped,
        };
        if !havoc_bytes(rng, &mut bytes) {
            return MutationResult::Skipped;
        }
        input.io[idx].command = with_arg(&base, &bytes);
        let at = input.io[idx].at;
        input.mutated_at = Some(merge_earliest(input.mutated_at, at));
        MutationResult::Mutated
    }
}

/// A fresh random fuzzer argument for a driver action.
fn rand_arg(rng: &mut Rng) -> Vec<u8> {
    (0..ARG_BYTES).map(|_| rng.below(256) as u8).collect()
}

/// Pick a uniformly-random index from `subset`.
fn pick_action(rng: &mut Rng, subset: &[usize]) -> usize {
    debug_assert!(!subset.is_empty());
    subset[rng.below(subset.len())]
}

fn merge_earliest(prev: Option<u64>, new: u64) -> u64 {
    match prev {
        Some(p) => p.min(new),
        None => new,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drivers(n: usize) -> Vec<Action> {
        (0..n)
            .map(|i| {
                Action::Driver(WorkloadDriver {
                    container: format!("c{i}"),
                    driver: format!("d{i}"),
                })
            })
            .collect()
    }

    #[test]
    fn random_subset_is_sorted_distinct_nonempty() {
        let mut rng = Rng::new(7);
        for _ in 0..50 {
            let s = random_subset(6, &mut rng);
            assert!(!s.is_empty() && s.len() <= 6);
            assert!(s.windows(2).all(|w| w[0] < w[1]), "sorted+distinct: {s:?}");
        }
    }

    #[test]
    fn drift_keeps_at_least_one() {
        let mut rng = Rng::new(9);
        let parent = vec![0usize, 2];
        for _ in 0..50 {
            let c = drift_subset(&parent, 5, 30, &mut rng);
            assert!(!c.is_empty());
        }
    }

    #[test]
    fn io_insert_scatters_and_sets_mutated_at() {
        let mut rng = Rng::new(3);
        let m = Mutators::new(drivers(4), 1000, 40, SwarmMode::Off);
        let mut input = Input::new(500);
        assert_eq!(
            m.io_insert(&mut rng, &mut input, None),
            MutationResult::Mutated
        );
        assert!(!input.io.is_empty());
        assert!(
            input.io.windows(2).all(|w| w[0].at <= w[1].at),
            "sorted by at"
        );
        assert!(
            input.io.iter().all(|a| a.at >= 500),
            "respects anchor floor"
        );
        assert_eq!(input.mutated_at, input.io.iter().map(|a| a.at).min());
    }

    #[test]
    fn hex_arg_roundtrips_through_command() {
        let arg = vec![0x00, 0xde, 0xad, 0xbe, 0xef, 0xff];
        let cmd = with_arg("/opt/bedrock/drivers/mine-blocks", &arg);
        let (base, back) = split_arg(&cmd).expect("has arg");
        assert_eq!(base, "/opt/bedrock/drivers/mine-blocks");
        assert_eq!(back, arg);
    }

    #[test]
    fn split_arg_ignores_non_hex_tails() {
        // Fault commands and bare driver paths have no hex tail.
        assert!(split_arg("fault-injector clear").is_none());
        assert!(split_arg("fault-injector partition btcd1").is_none());
        // A timed partition ends in the `--duration` value, not a hex arg, so
        // io_arg_havoc leaves it alone.
        assert!(split_arg("fault-injector partition btcd1 --duration 500ms").is_none());
        assert!(split_arg("/opt/bedrock/drivers/mine-blocks").is_none());
    }

    #[test]
    fn fault_partition_lowers_duration_to_flag() {
        let timed = Action::FaultPartition {
            container: "btcd2".into(),
            duration: "2s",
        };
        assert_eq!(
            timed.to_io(),
            (
                Target::Host,
                "fault-injector partition btcd2 --duration 2s".to_string()
            )
        );
    }

    #[test]
    fn vocabulary_partitions_are_all_timed_and_clear_is_not_insertable() {
        let actions = Action::vocabulary(Vec::new(), &["btcd1".into()], &[]);
        let cmds: Vec<String> = actions.iter().map(|a| a.to_io().1).collect();
        // No persistent partition — every one carries a --duration.
        assert!(!cmds.iter().any(|c| c == "fault-injector partition btcd1"));
        for d in FAULT_DURATIONS {
            assert!(
                cmds.contains(&format!("fault-injector partition btcd1 --duration {d}")),
                "missing timed partition for {d}: {cmds:?}"
            );
        }
        // `clear` is still a defined action in the vocabulary, but not insertable.
        assert!(cmds.iter().any(|c| c == "fault-injector clear"));
        assert_eq!(actions.iter().filter(|a| !a.insertable()).count(), 1);
        // Excluded containers get no partition at all.
        let none = Action::vocabulary(Vec::new(), &["btcd1".into()], &["btcd1".into()]);
        assert!(none.iter().all(|a| !a.to_io().1.contains("partition")));
    }

    #[test]
    fn splice_grafts_donor_forward_preserving_gaps() {
        let mut rng = Rng::new(7);
        let m = Mutators::new(drivers(4), 1000, 40, SwarmMode::Off);
        let mut input = Input::new(500); // checkpoint anchor at 500
        let donor = vec![
            IoAction {
                at: 100,
                target: Target::Host,
                command: "a".into(),
            },
            IoAction {
                at: 160,
                target: Target::Host,
                command: "b".into(),
            },
        ];
        assert_eq!(
            m.splice(&mut rng, &mut input, &donor),
            MutationResult::Mutated
        );
        assert_eq!(input.io.len(), 2);
        // Grafted forward (at/after the anchor) and sorted.
        assert!(input.io.iter().all(|e| e.at >= 500), "{:?}", input.io);
        assert!(input.io.windows(2).all(|w| w[0].at <= w[1].at));
        // The donor's 60-tick internal gap is preserved.
        assert_eq!(input.io[1].at - input.io[0].at, 60);
        assert_eq!(input.mutated_at, Some(input.io[0].at));
        // Empty donor is a no-op.
        let mut empty = Input::new(0);
        assert_eq!(m.splice(&mut rng, &mut empty, &[]), MutationResult::Skipped);
    }

    #[test]
    fn lineage_reroll_escapes_a_locked_subset() {
        // A lineage locked to a single action must still, on some branches,
        // re-roll and draw actions outside its inherited subset — otherwise an
        // inherited regime could permanently lock a lineage out of a combo.
        let mut rng = Rng::new(123);
        let m = Mutators::new(drivers(6), 1000, 40, SwarmMode::Lineage);
        let lineage = vec![0usize]; // regime restricted to action 0 ("d0")
        let mut saw_outside = false;
        for _ in 0..100 {
            let mut input = Input::new(0);
            m.io_insert(&mut rng, &mut input, Some(&lineage));
            if input
                .io
                .iter()
                .any(|e| e.command.split(' ').next() != Some("d0"))
            {
                saw_outside = true;
                break;
            }
        }
        assert!(
            saw_outside,
            "lineage re-roll should escape the inherited subset"
        );
    }

    #[test]
    fn io_insert_attaches_arg_to_drivers_only() {
        let mut rng = Rng::new(11);
        let mut actions = drivers(2);
        actions.push(Action::FaultPartition {
            container: "btcd1".into(),
            duration: "2s",
        });
        let m = Mutators::new(actions, 1000, 40, SwarmMode::Off);
        let mut input = Input::new(0);
        // Insert many so we very likely hit both a driver and the fault action.
        for _ in 0..40 {
            m.io_insert(&mut rng, &mut input, None);
        }
        let (mut saw_driver_arg, mut saw_fault) = (false, false);
        for e in &input.io {
            if e.command.starts_with("fault-injector") {
                assert!(split_arg(&e.command).is_none(), "fault has no hex arg");
                saw_fault = true;
            } else {
                assert!(
                    split_arg(&e.command).is_some(),
                    "driver has hex arg: {}",
                    e.command
                );
                saw_driver_arg = true;
            }
        }
        assert!(saw_driver_arg && saw_fault);
    }

    #[test]
    fn mutations_never_insert_clear() {
        let mut rng = Rng::new(99);
        let mut actions = drivers(2);
        actions.push(Action::FaultPartition {
            container: "btcd1".into(),
            duration: "2s",
        });
        actions.push(Action::FaultClear);
        let m = Mutators::new(actions, 1000, 40, SwarmMode::Off);
        let mut input = Input::new(0);
        // io_insert draws from the full vocabulary (incl. clear) yet never emits it.
        for _ in 0..200 {
            m.io_insert(&mut rng, &mut input, None);
        }
        assert!(!input.io.is_empty());
        assert!(input.io.iter().all(|e| e.command != "fault-injector clear"));
        // io_driver_swap never re-targets an existing entry to clear either.
        for _ in 0..200 {
            m.io_driver_swap(&mut rng, &mut input);
        }
        assert!(input.io.iter().all(|e| e.command != "fault-injector clear"));
    }

    #[test]
    fn io_arg_havoc_changes_an_arg() {
        let mut rng = Rng::new(4);
        let m = Mutators::new(drivers(1), 1000, 8, SwarmMode::Off);
        let mut input = Input::new(0);
        m.io_insert(&mut rng, &mut input, None);
        let before = input.io[0].command.clone();
        let mut changed = false;
        for _ in 0..50 {
            if m.io_arg_havoc(&mut rng, &mut input) == MutationResult::Mutated
                && input.io[0].command != before
            {
                // Still a valid driver command with a decodable arg.
                assert!(split_arg(&input.io[0].command).is_some());
                changed = true;
                break;
            }
        }
        assert!(changed, "arg havoc should eventually change the arg");
    }

    #[test]
    fn rng_havoc_skips_when_empty_and_preserves_count() {
        let mut rng = Rng::new(5);
        let m = Mutators::new(drivers(2), 1000, 8, SwarmMode::Off);
        let mut empty = Input::new(0);
        assert_eq!(
            m.rng_byte_havoc(&mut rng, &mut empty),
            MutationResult::Skipped
        );

        let mut input = Input::new(0);
        input.rng = (0..8)
            .map(|i| crate::input::RngVal {
                at: i * 10,
                value: i,
            })
            .collect();
        // Try until havoc applies; count and positions must be preserved.
        let mut applied = false;
        for _ in 0..50 {
            let mut clone = input.clone();
            if m.rng_byte_havoc(&mut rng, &mut clone) == MutationResult::Mutated {
                assert_eq!(clone.rng.len(), 8);
                assert!(clone.rng.iter().zip(&input.rng).all(|(a, b)| a.at == b.at));
                assert!(clone.mutated_at.is_some());
                applied = true;
                break;
            }
        }
        assert!(applied);
    }
}
