// SPDX-License-Identifier: GPL-2.0

//! Tree — a read-only view of the execution genealogy.

use std::collections::HashMap;
use std::sync::Arc;

use crate::branch::BranchId;
use crate::checkpoint::{Checkpoint, CheckpointId};
use crate::inner::{BranchMeta, LabInner};
use crate::time::VirtTime;

/// Lightweight view of a live branch in the tree. Cheap, by-value, doesn't
/// pin the underlying [`Branch`](crate::Branch) — it's a snapshot of its
/// metadata at the moment [`Tree`] was constructed.
#[derive(Debug, Clone)]
pub struct BranchView {
    pub id: BranchId,
    pub origin: CheckpointId,
    pub current_time: VirtTime,
}

/// A snapshot of every live [`Checkpoint`] and live [`Branch`](crate::Branch)
/// in the tree at the moment of construction.
///
/// `Tree` is purely a read-only view; it doesn't extend the lifetime of its
/// nodes beyond the handles already held by the user.
pub struct Tree {
    pub(crate) checkpoints: Vec<Checkpoint>,
    pub(crate) branches: Vec<BranchView>,
}

impl Tree {
    pub(crate) fn from_lab(lab: &Arc<LabInner>) -> Self {
        let checkpoints = lab.graph.lock().unwrap().checkpoints();
        let mut branches: Vec<_> = lab
            .live_branches
            .lock()
            .unwrap()
            .values()
            .map(|m: &BranchMeta| BranchView {
                id: m.id,
                origin: m.origin,
                current_time: m.current_time,
            })
            .collect();
        branches.sort_by_key(|b| b.id);
        Self {
            checkpoints,
            branches,
        }
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    pub fn branches(&self) -> &[BranchView] {
        &self.branches
    }

    /// Render the tree as an ASCII drawing using box-drawing characters.
    /// Roots flush left, children indented under their parent with
    /// `├──`/`└──` connectors, live branches dangling as leaves under
    /// their origin checkpoint.
    ///
    /// Only live checkpoints are drawn — the same set [`Tree::dot`] uses.
    /// Callers that want short-lived rewind intermediates to appear must
    /// hold strong [`Checkpoint`] handles to them; otherwise the
    /// underlying `Arc<CheckpointInner>` is reclaimed as soon as the
    /// rewinding code returns.
    ///
    /// Edges follow [`Checkpoint::closest_live_ancestor`] rather than the
    /// immediate parent, so a checkpoint that was reparented under a
    /// since-dropped rewind intermediate stays attached to its nearest
    /// live ancestor instead of floating off as a stray root.
    ///
    /// Example output:
    ///
    /// ```text
    /// cp0 @ 56.613s
    /// ├── cp1 @ 61.234s
    /// │   ├── cp3 @ 66.456s
    /// │   └── br7 @ 65.001s
    /// └── cp2 @ 62.345s
    /// ```
    pub fn ascii(&self) -> String {
        let mut nodes: HashMap<CheckpointId, &Checkpoint> = HashMap::new();
        let mut children: HashMap<CheckpointId, Vec<CheckpointId>> = HashMap::new();
        let mut roots: Vec<CheckpointId> = Vec::new();
        for cp in &self.checkpoints {
            nodes.insert(cp.id(), cp);
            match cp.closest_live_ancestor() {
                Some(parent) => children.entry(parent.id()).or_default().push(cp.id()),
                None => roots.push(cp.id()),
            }
        }
        for v in children.values_mut() {
            v.sort_by_key(|c| c.0);
        }
        roots.sort_by_key(|c| c.0);

        let mut branches: HashMap<CheckpointId, Vec<&BranchView>> = HashMap::new();
        for b in &self.branches {
            branches.entry(b.origin).or_default().push(b);
        }
        for v in branches.values_mut() {
            v.sort_by_key(|b| b.id.0);
        }

        let mut out = String::new();
        for &root_id in &roots {
            let cp = nodes[&root_id];
            out.push_str(&format!(
                "cp{} @ {:.3}s\n",
                root_id.0,
                cp.time().as_secs_f64()
            ));
            write_subtree(&mut out, root_id, &nodes, &children, &branches, "");
        }
        out
    }

    /// Render the tree as a Graphviz DOT graph. Useful for visualizing
    /// branching exploration runs.
    ///
    /// Edges follow [`Checkpoint::closest_live_ancestor`], not the immediate
    /// parent — matching [`Tree::ascii`] and the JSON tree view. A checkpoint
    /// whose direct parent has been dropped (e.g. a transient rewind
    /// intermediate, or an eventually-pass fork point that wasn't retained)
    /// attaches to its nearest live ancestor instead of floating off as a stray
    /// node with no edge.
    pub fn dot(&self) -> String {
        let mut s = String::new();
        s.push_str("digraph tree {\n");
        s.push_str("  rankdir=LR;\n");
        s.push_str("  node [shape=box];\n");
        for cp in &self.checkpoints {
            s.push_str(&format!(
                "  cp{} [label=\"cp{} @ {:.3}s\"];\n",
                cp.id().0,
                cp.id().0,
                cp.time().as_secs_f64()
            ));
        }
        for cp in &self.checkpoints {
            if let Some(parent) = cp.closest_live_ancestor() {
                s.push_str(&format!("  cp{} -> cp{};\n", parent.id().0, cp.id().0));
            }
        }
        s.push_str("  node [shape=oval, style=dashed];\n");
        for b in &self.branches {
            s.push_str(&format!(
                "  br{} [label=\"br{} @ {:.3}s\"];\n",
                b.id.0,
                b.id.0,
                b.current_time.as_secs_f64()
            ));
            s.push_str(&format!("  cp{} -> br{};\n", b.origin.0, b.id.0));
        }
        s.push_str("}\n");
        s
    }
}

/// Recursively writes the children of `parent_id` to `out`, indented under
/// `prefix`. Caller is responsible for writing the line for `parent_id`
/// itself before calling.
fn write_subtree(
    out: &mut String,
    parent_id: CheckpointId,
    nodes: &HashMap<CheckpointId, &Checkpoint>,
    children: &HashMap<CheckpointId, Vec<CheckpointId>>,
    branches: &HashMap<CheckpointId, Vec<&BranchView>>,
    prefix: &str,
) {
    let kids = children.get(&parent_id).cloned().unwrap_or_default();
    let brs = branches.get(&parent_id).cloned().unwrap_or_default();
    let total = kids.len() + brs.len();
    for (i, child_id) in kids.iter().enumerate() {
        let is_last = i + 1 == total;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };
        let cp = nodes[child_id];
        out.push_str(&format!(
            "{prefix}{connector}cp{} @ {:.3}s\n",
            child_id.0,
            cp.time().as_secs_f64()
        ));
        let next_prefix = format!("{prefix}{child_prefix}");
        write_subtree(out, *child_id, nodes, children, branches, &next_prefix);
    }
    for (j, b) in brs.iter().enumerate() {
        let i = kids.len() + j;
        let is_last = i + 1 == total;
        let connector = if is_last { "└── " } else { "├── " };
        out.push_str(&format!(
            "{prefix}{connector}br{} @ {:.3}s\n",
            b.id.0,
            b.current_time.as_secs_f64()
        ));
    }
}
