// SPDX-License-Identifier: GPL-2.0

//! The corpus — which doubles as the VM checkpoint tree — and the scheduler
//! that decides which entry to fuzz next.
//!
//! Each [`Node`] is both a fuzzing corpus entry (an [`Input`] plus the serial
//! it produced) and a tree node (a [`Checkpoint`] with a parent link). The
//! scheduler ([`node_weight`] + [`weighted_pick`]) is deliberately a small,
//! self-contained function so its strategy can be tuned in one place.

use bedrock_lab::Checkpoint;

use crate::input::Input;
use crate::rng::Rng;

/// One node in the shared tree/corpus.
pub struct Node {
    pub input: Input,
    pub checkpoint: Checkpoint,
    pub parent: Option<usize>,
    /// Number of serial lines the discovering branch produced. Just a count —
    /// the lines themselves are never retained in memory (they're large on deep
    /// branches). With `--serial-dir` the full serial is written to disk keyed
    /// by checkpoint id; otherwise it's dropped after objective detection.
    pub serial_lines: usize,
    /// Times a worker has picked this entry. Human-facing only; the weight
    /// decays on `effort`, not this raw count.
    pub scheduled: u64,
    /// Accumulated exploration effort: each branch run adds the worker's run
    /// window normalized so the longest-window worker's run costs 1.0. The
    /// weight's denominator — so a swarm of cheap short-horizon looks doesn't
    /// write off an entry as fast as full-horizon ones would.
    pub effort: f64,
    /// Corpus entries discovered by mutating this one (its fertility). The
    /// weight's numerator — concentrates effort on entries that keep paying.
    pub novelty: u64,
    /// Virtual seconds the branch that produced this checkpoint ran for. 0 for
    /// the seed. The dashboard draws each node's incoming edge this long.
    pub runtime_secs: f64,
    /// Virtual seconds this checkpoint sits forward of the root (root = 0) —
    /// real forward progress, not corpus hop count. A scheduling multiplier so
    /// the time-frontier outcompetes shallow, perpetually-fertile nodes.
    /// Virtual time rather than hops: an in-place rewind lands a child early in
    /// time, so crediting hops would let it masquerade as deep.
    pub depth_secs: f64,
    /// Workers currently running a branch off this entry. Transient; divides
    /// the weight so workers spread across distinct checkpoints.
    pub in_flight: u32,
    /// Swarm-testing action subset (indices into the vocabulary) this entry's
    /// branches draw inserted actions from. Empty = unrestricted (the seed).
    /// A child inherits its parent's subset with a small drift.
    pub swarm: Vec<usize>,
    /// Rarity score at discovery: the sum over this node's covered edges of
    /// `1/(corpus entries covering that edge)`. High when the node broke into
    /// territory few others cover, ~0 when it re-treads common edges. A
    /// scheduling multiplier that steers effort toward seldom-explored regions
    /// (a generic FairFuzz-style rare-edge bias — purely coverage statistics,
    /// no workload knowledge). Computed once at add; the existing effort decay
    /// keeps it from pinning the fleet to one entry forever.
    pub rarity: f64,
}

impl Node {
    /// The seed node: an empty input bound to the discovery checkpoint.
    pub fn seed(checkpoint: Checkpoint) -> Self {
        Self {
            input: Input::new(checkpoint.time().instructions()),
            checkpoint,
            parent: None,
            serial_lines: 0,
            scheduled: 0,
            effort: 0.0,
            novelty: 0,
            runtime_secs: 0.0,
            depth_secs: 0.0,
            in_flight: 0,
            swarm: Vec::new(),
            rarity: 0.0,
        }
    }
}

/// An in-flight branch being pumped by a worker, for the live execution view.
pub struct ActiveBranch {
    pub parent: usize,
    pub start_secs: f64,
    pub current_secs: f64,
}

/// One reported bug. Unlike a [`Node`] it carries the objective that flagged it
/// and isn't scheduled for further mutation.
pub struct Solution {
    pub parent: Option<usize>,
    pub checkpoint: Checkpoint,
    pub serial: Vec<String>,
    pub reason: String,
}

/// Selection weight for the scheduler.
///
/// Three "attractiveness" signals, each **log-damped to the same order of
/// magnitude** so none can dominate and tunnel the fleet onto one dimension:
/// - `(1+ln(1+novelty))` — fertility: entries that keep spawning novel children.
/// - `(1+ln(1+depth_secs))` — a modest pull to the time-frontier so multi-step
///   sequences can build and deep states stay reachable.
/// - `(1+ln(1+rarity))` — a generic FairFuzz-style rare-edge bias.
///
/// Earlier these were linear, and whichever was largest won outright: linear
/// depth (~1800× at a 30-min-deep tip) made the whole fleet pile onto a handful
/// of barren deepest nodes; linear novelty re-traps it on the super-fertile
/// root. Log-damping keeps all three ~single-digit so the *combination* ranks
/// entries, while the linear `effort` denominator decays over-mined ones and
/// `/(1+in_flight)` fans workers across distinct checkpoints.
pub fn node_weight(node: &Node) -> f64 {
    (1.0 + (node.novelty as f64).ln_1p()) / (1.0 + node.effort) * (1.0 + node.depth_secs.ln_1p())
        / (1.0 + node.in_flight as f64)
        * (1.0 + node.rarity.ln_1p())
}

/// Selection weight for depth-seeking workers: depth dominates (linear) and
/// `in_flight` spreads the deep fleet across distinct frontier tips instead of
/// piling onto one. Coverage fertility is ignored on purpose — these workers
/// exist to extend the virtual-time frontier, not to mine shallow novelty.
/// Linear rather than squared: `depth²` collapsed the whole deep fleet onto the
/// single deepest node, which then spun barren at the depth cap; linear still
/// favors the frontier but keeps the deep workers spread across deep nodes.
pub fn deep_weight(node: &Node) -> f64 {
    (1.0 + node.depth_secs) / (1.0 + node.in_flight as f64)
}

/// Weighted random index biased hard toward the time-frontier, for the
/// depth-seeking workers. See [`deep_weight`].
pub fn deep_pick(corpus: &[Node], rng: &mut Rng) -> usize {
    let total: f64 = corpus.iter().map(deep_weight).sum();
    if total <= 0.0 {
        return corpus.len().saturating_sub(1);
    }
    let mut r = rng.next_f64() * total;
    for (i, node) in corpus.iter().enumerate() {
        r -= deep_weight(node);
        if r < 0.0 {
            return i;
        }
    }
    corpus.len() - 1
}

/// Weighted random index into `corpus` using [`node_weight`]. O(n) per draw
/// (weights change every pick, so there's nothing to precompute); fine at
/// VM-bound exec rates.
pub fn weighted_pick(corpus: &[Node], rng: &mut Rng) -> usize {
    let total: f64 = corpus.iter().map(node_weight).sum();
    if total <= 0.0 {
        return 0;
    }
    let mut r = rng.next_f64() * total;
    for (i, node) in corpus.iter().enumerate() {
        r -= node_weight(node);
        if r < 0.0 {
            return i;
        }
    }
    corpus.len() - 1
}

#[cfg(test)]
mod tests {
    // A weight-only stand-in so we can test the scheduler without real
    // checkpoints (which require a live VM). Mirrors `node_weight`.
    fn w(novelty: u64, effort: f64, depth: f64, in_flight: u32, rarity: f64) -> f64 {
        (1.0 + (novelty as f64).ln_1p()) / (1.0 + effort) * (1.0 + depth.ln_1p())
            / (1.0 + in_flight as f64)
            * (1.0 + rarity.ln_1p())
    }

    #[test]
    fn fertile_outweighs_barren() {
        assert!(w(5, 1.0, 1.0, 0, 0.0) > w(0, 1.0, 1.0, 0, 0.0));
    }

    #[test]
    fn effort_decays_weight() {
        assert!(w(1, 0.0, 1.0, 0, 0.0) > w(1, 10.0, 1.0, 0, 0.0));
    }

    #[test]
    fn depth_boosts_frontier() {
        assert!(w(1, 1.0, 5.0, 0, 0.0) > w(1, 1.0, 0.0, 0, 0.0));
    }

    #[test]
    fn in_flight_deprioritizes() {
        assert!(w(1, 1.0, 1.0, 0, 0.0) > w(1, 1.0, 1.0, 3, 0.0));
    }

    #[test]
    fn rarity_boosts_weight() {
        // An entry that broke into rare territory outweighs an identical one
        // that re-trod common edges, and the boost is log-damped (a 100×
        // rarity gap is well under a 100× weight gap).
        assert!(w(1, 1.0, 1.0, 0, 50.0) > w(1, 1.0, 1.0, 0, 0.0));
        assert!(w(1, 1.0, 1.0, 0, 50.0) < 10.0 * w(1, 1.0, 1.0, 0, 0.5));
    }
}
