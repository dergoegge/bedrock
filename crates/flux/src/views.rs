// SPDX-License-Identifier: GPL-2.0

//! Serializable DTOs for the HTTP/SSE state API. Pure data — the [`Campaign`]
//! fills these under the relevant lock and this module just defines their
//! JSON shape.
//!
//! [`Campaign`]: crate::campaign::Campaign

use serde::Serialize;

/// Serialize a view to JSON. Our DTOs always serialize; the fallback is
/// unreachable in practice.
pub fn json_string<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

/// Snapshot of campaign progress: `/stats` and the periodic `stats` event.
#[derive(Serialize)]
pub struct StatsView {
    pub wall_secs: f64,
    pub vt_secs: f64,
    pub vt_per_wall: f64,
    pub branches: u64,
    pub checkpoints: usize,
    pub corpus: usize,
    pub adds: u64,
    pub solutions: u64,
    /// Distinct findings (unique solution reasons); ≤ `solutions`.
    pub unique_solutions: u64,
    pub coverage: Vec<CoverageView>,
}

/// Per feedback-buffer edge coverage: `seen` hit edges of `total`.
#[derive(Serialize)]
pub struct CoverageView {
    pub id: String,
    pub seen: usize,
    pub total: usize,
}

/// One node in the `/tree` genealogy (ids namespaced `cp<n>` / `br<n>`).
#[derive(Serialize)]
pub struct TreeNodeView {
    pub id: String,
    pub parent: Option<String>,
    pub time_secs: f64,
    pub kind: &'static str,
}

#[derive(Serialize)]
pub struct TreeView {
    pub nodes: Vec<TreeNodeView>,
}

/// One in-flight branch in the `branches` SSE stream.
#[derive(Serialize)]
pub struct BranchLiveView {
    pub id: String,
    pub parent: usize,
    pub start_secs: f64,
    pub current_secs: f64,
}

/// Corpus entry summary (list view + the `corpus_add` event).
#[derive(Serialize)]
pub struct CorpusEntryView {
    pub id: usize,
    pub parent: Option<usize>,
    pub checkpoint: u64,
    pub time_secs: f64,
    pub runtime_secs: f64,
    pub scheduled: u64,
    pub novelty: u64,
    pub serial_lines: usize,
}

/// Corpus entry detail (`/corpus/{id}`): the summary plus captured serial.
#[derive(Serialize)]
pub struct CorpusDetail {
    pub id: usize,
    pub parent: Option<usize>,
    pub checkpoint: u64,
    pub time_secs: f64,
    pub runtime_secs: f64,
    pub scheduled: u64,
    pub novelty: u64,
    pub serial: Vec<String>,
}

/// Solution summary (`/solutions` list + the `solution` event).
#[derive(Serialize)]
pub struct SolutionView {
    pub id: usize,
    pub parent: Option<usize>,
    pub reason: String,
    pub serial_lines: usize,
}

/// Solution detail (`/solutions/{id}`): the summary plus captured serial.
#[derive(Serialize)]
pub struct SolutionDetail {
    pub id: usize,
    pub parent: Option<usize>,
    pub reason: String,
    pub checkpoint: u64,
    pub time_secs: f64,
    pub serial: Vec<String>,
}
