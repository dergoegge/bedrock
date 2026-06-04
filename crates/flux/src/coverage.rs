// SPDX-License-Identifier: GPL-2.0

//! Coverage: the novelty signal the campaign merges after each run.
//!
//! Instrumented workloads register one or more feedback buffers (keyed by an
//! id, e.g. a build-id). Each branch exposes a per-id view; we OR the slots
//! together and treat any byte going from 0 to non-zero (against the cumulative
//! map) as new coverage. The cumulative map lives behind a single lock so
//! merges and the prints they trigger don't interleave across workers.

use std::collections::HashMap;

use bedrock_lab::Branch;

/// Global cumulative edge coverage.
#[derive(Default)]
pub struct Coverage {
    /// Per feedback-buffer-id cumulative bitmap (bytewise OR of every matching
    /// buffer ever seen).
    bitmap: HashMap<Vec<u8>, Vec<u8>>,
    /// Per feedback-buffer-id, how many corpus nodes cover each edge (byte).
    /// Drives the scheduler's rare-edge bias: an edge owned by few nodes is
    /// rare. Updated only when a node is added (see [`Self::register_node`]),
    /// so it counts corpus coverage, not raw branch hits.
    owners: HashMap<Vec<u8>, Vec<u32>>,
}

/// A single run's feedback-buffer readout: per-id, the OR of that id's slots.
pub type RunBitmap = HashMap<Vec<u8>, Vec<u8>>;

impl Coverage {
    /// OR `current` into the cumulative bitmap per id; return the number of
    /// bytes that went from 0 to non-zero (newly-covered edges).
    pub fn merge_bitmap(&mut self, current: &RunBitmap) -> usize {
        let mut new_edges = 0;
        for (id, cur) in current {
            let cum = self.bitmap.entry(id.clone()).or_default();
            if cum.len() < cur.len() {
                cum.resize(cur.len(), 0);
            }
            for (i, &b) in cur.iter().enumerate() {
                if b != 0 && cum[i] == 0 {
                    new_edges += 1;
                }
                cum[i] |= b;
            }
        }
        new_edges
    }

    /// Record that one corpus node covers `current`'s edges (incrementing the
    /// per-edge owner counts) and return its **rarity score**: the sum over the
    /// node's covered edges of `1/owners`. A node breaking into territory few
    /// others cover scores high; one re-treading common edges scores ~0. Call
    /// exactly once per added node. Generic — purely a coverage statistic.
    pub fn register_node(&mut self, current: &RunBitmap) -> f64 {
        let mut score = 0.0;
        for (id, cur) in current {
            let own = self.owners.entry(id.clone()).or_default();
            if own.len() < cur.len() {
                own.resize(cur.len(), 0);
            }
            for (i, &b) in cur.iter().enumerate() {
                if b != 0 {
                    own[i] += 1;
                    score += 1.0 / own[i] as f64;
                }
            }
        }
        score
    }

    /// `(seen_edges, total_edges)` per id, sorted by id — for the dashboard.
    pub fn summary(&self) -> Vec<(String, usize, usize)> {
        let mut v: Vec<(String, usize, usize)> = self
            .bitmap
            .iter()
            .map(|(id, bm)| {
                (
                    String::from_utf8_lossy(id).into_owned(),
                    bm.iter().filter(|&&b| b != 0).count(),
                    bm.len(),
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// The cumulative bitmaps, for offline coverage dumps.
    pub fn bitmaps(&self) -> &HashMap<Vec<u8>, Vec<u8>> {
        &self.bitmap
    }
}

/// Read every registered feedback buffer off a branch, OR'ing all slots per id.
/// Errors (e.g. a branch with no buffers) yield an empty map rather than abort.
pub fn read_coverage(branch: &mut Branch) -> RunBitmap {
    let mut out: RunBitmap = HashMap::new();
    let ids = branch.feedback_buffer_ids().unwrap_or_default();
    for id in ids {
        let buffers = branch.feedback_buffers(&id).unwrap_or_default();
        let max_len = buffers.iter().map(|b| b.len()).max().unwrap_or(0);
        let entry = out.entry(id).or_default();
        if entry.len() < max_len {
            entry.resize(max_len, 0);
        }
        for src in &buffers {
            for (d, &s) in entry.iter_mut().zip(src.iter()) {
                *d |= s;
            }
        }
    }
    out
}
