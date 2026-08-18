//! Deterministic topological order (`01-ir.md` §3.2, `03-engine.md` §3.2).
//!
//! Kahn's algorithm with the ready set kept as a `BinaryHeap<Reverse<..>>` on
//! `(module_path, declaration_index)`. Two structurally identical models produce
//! byte-identical orders regardless of hash-map iteration, thread count or
//! platform, and the planner asserts the result is a *total* order (every node
//! emitted exactly once) before storing it.
//!
//! No `std::collections::HashMap` appears here or anywhere else in the crate:
//! the maps are `BTreeMap`, so even a debug print is deterministic.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::slots::SlotId;

/// The §3.2 ready-set key. `module_path` is interned to a `u32` whose numeric
/// order equals the string order of the paths, so the heap compares integers
/// while ordering by path exactly as the spec words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrderKey {
    pub module: u32,
    pub decl_index: u32,
    /// Final tiebreak. Unreachable for a well-formed model (a module path and a
    /// declaration index identify a declaration), present so the comparison is
    /// total by construction rather than by assumption.
    pub slot: u32,
}

/// Interns module paths so that `id(a) < id(b)` iff `a < b` as strings.
#[derive(Debug, Default)]
pub struct ModulePaths {
    ids: BTreeMap<String, u32>,
}

impl ModulePaths {
    /// Build the interner from every module path in the model. Sorting happens
    /// once, here; nothing downstream compares strings.
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a str>) -> ModulePaths {
        let sorted: std::collections::BTreeSet<&str> = paths.into_iter().collect();
        ModulePaths {
            ids: sorted
                .into_iter()
                .enumerate()
                .map(|(i, p)| (p.to_string(), i as u32))
                .collect(),
        }
    }

    pub fn id(&self, path: &str) -> u32 {
        // A path absent from the interner cannot participate in an order, so it
        // sorts last rather than panicking a planner mid-run.
        self.ids.get(path).copied().unwrap_or(u32::MAX)
    }
}

/// A node offered to [`kahn`]: the slot, its ordering key, and the slots it must
/// follow. Predecessors outside the node set are ignored — they are already
/// computed by an earlier tape (a `Scalar` read from a `Series` body, say).
#[derive(Debug, Clone)]
pub struct Node {
    pub slot: SlotId,
    pub key: OrderKey,
    pub preds: Vec<SlotId>,
}

/// What went wrong ordering a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    /// A cycle in `G₀`. The checker reports this as `E0201` before the planner
    /// ever runs; reaching it here means an unchecked model was planned, so it
    /// is an error value rather than a diagnostic.
    Cycle { remaining: Vec<SlotId> },
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderError::Cycle { remaining } => write!(
                f,
                "instantaneous cycle among {} slots ({}) — the model was not checked (E0201)",
                remaining.len(),
                remaining
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Kahn's algorithm with a `(module_path, declaration_index)` min-heap.
///
/// Returns the total order, or the set of slots that could not be emitted.
pub fn kahn(nodes: &[Node]) -> Result<Vec<SlotId>, OrderError> {
    let members: BTreeMap<SlotId, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.slot, i)).collect();

    let mut indegree = vec![0usize; nodes.len()];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        // Duplicate edges (`reserve[t-1] + reserve[0]` yields two) are distinct
        // edges in the IR and stay distinct here: the indegree counts edges, not
        // pairs, and the successor list mirrors it exactly.
        for pred in &node.preds {
            if let Some(&p) = members.get(pred) {
                if p == i {
                    // A lag-0 self edge is a cycle the checker rejects; a lagged
                    // self edge never reaches this list.
                    continue;
                }
                indegree[i] += 1;
                successors[p].push(i);
            }
        }
    }

    let mut ready: BinaryHeap<Reverse<(OrderKey, usize)>> = nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| indegree[*i] == 0)
        .map(|(i, n)| Reverse((n.key, i)))
        .collect();

    let mut out = Vec::with_capacity(nodes.len());
    while let Some(Reverse((_, i))) = ready.pop() {
        out.push(nodes[i].slot);
        for &s in &successors[i] {
            indegree[s] -= 1;
            if indegree[s] == 0 {
                ready.push(Reverse((nodes[s].key, s)));
            }
        }
    }

    if out.len() != nodes.len() {
        let emitted: std::collections::BTreeSet<SlotId> = out.iter().copied().collect();
        return Err(OrderError::Cycle {
            remaining: nodes
                .iter()
                .map(|n| n.slot)
                .filter(|s| !emitted.contains(s))
                .collect(),
        });
    }
    Ok(out)
}
