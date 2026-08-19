//! The impact set: the transitive downstream closure of every change (§11.3).
//!
//! "`bel` changed" is not an answer a reviewer can act on; "these 3 changes
//! affect 14 components and 4 outputs, of which `bel` and `reserve` are
//! Outputs" is. The closure is taken over the *union* of both sides' dependency
//! edges, because a removed component's dependents exist only on side A and an
//! added one's only on side B — taking either side alone silently drops half
//! the blast radius.
//!
//! Edges come from the IR itself: every `Ref`/`Lag`/`At` names a component and
//! every `Lookup` names a table, so a table change propagates through exactly
//! the components that read it. Self-edges (`x[t-1]` inside `x`) are dropped —
//! a recursive component is not downstream of itself.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use predictable_ir::Expr;
use serde::{Deserialize, Serialize};

use crate::{
    AssumptionChange, ComponentChange, ComponentSummary, EntryStatus, ModelSide, TableChange,
};

/// What the changes reach.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImpactSet {
    /// The changed things themselves: components, tables and assumptions.
    pub seeds: Vec<String>,
    /// Components strictly downstream of a seed, sorted.
    pub impacted: Vec<String>,
    /// Those of `seeds ∪ impacted` that are `kind = "Output"` on either side.
    pub outputs_affected: Vec<String>,
}

/// The names whose change starts the closure.
///
/// A *pure* rename is deliberately not a seed: nothing downstream computes a
/// different number because a component acquired a new spelling.
pub fn seeds(
    added: &[ComponentSummary],
    removed: &[ComponentSummary],
    changed: &[ComponentChange],
    tables: &[TableChange],
    assumptions: &[AssumptionChange],
) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for c in added.iter().chain(removed) {
        out.insert(c.name.clone());
    }
    for c in changed {
        // A doc-only change cannot move a number, so it moves nothing.
        if c.is_doc_only() {
            continue;
        }
        out.insert(c.name.clone());
        if let Some(old) = &c.renamed_from {
            out.insert(old.clone());
        }
    }
    for t in tables {
        out.insert(t.name.clone());
        if t.status == EntryStatus::Changed && t.fields.is_empty() && t.rows.is_none() {
            out.remove(&t.name);
        }
    }
    for a in assumptions {
        out.insert(a.name.clone());
    }
    out.into_iter().collect()
}

/// `name → the components that read it`, over both sides.
fn dependents(a: &ModelSide, b: &ModelSide) -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in a.components().chain(b.components()) {
        let mut reads: BTreeSet<String> = BTreeSet::new();
        for tree in [c.expr.as_ref(), c.init.as_ref()].into_iter().flatten() {
            collect_reads(tree, &mut reads);
        }
        for r in reads {
            if r == c.name {
                continue; // `x[t-1]` inside `x` is recursion, not a dependency
            }
            map.entry(r).or_default().insert(c.name.clone());
        }
    }
    map
}

fn collect_reads(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Ref { name } | Expr::Lag { name, .. } | Expr::At { name, .. } => {
            out.insert(name.clone());
        }
        Expr::Lookup { table, .. } => {
            out.insert(table.clone());
        }
        _ => {}
    }
    for (_, child) in e.children() {
        collect_reads(child, out);
    }
}

/// Breadth-first closure from `seeds` over the dependency edges of both sides.
pub fn impact_set(a: &ModelSide, b: &ModelSide, seeds: &[String]) -> ImpactSet {
    let edges = dependents(a, b);
    let seed_set: BTreeSet<&str> = seeds.iter().map(String::as_str).collect();

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
    while let Some(name) = queue.pop_front() {
        let Some(next) = edges.get(&name) else {
            continue;
        };
        for d in next {
            if seen.insert(d.clone()) {
                queue.push_back(d.clone());
            }
        }
    }

    let impacted: Vec<String> = seen
        .iter()
        .filter(|n| !seed_set.contains(n.as_str()))
        .cloned()
        .collect();

    let is_output = |name: &str| {
        [a, b]
            .iter()
            .any(|side| side.component(name).map(crate::is_output).unwrap_or(false))
    };
    let outputs_affected: Vec<String> = seeds
        .iter()
        .chain(impacted.iter())
        .filter(|n| is_output(n))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    ImpactSet {
        seeds: seeds.to_vec(),
        impacted,
        outputs_affected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use predictable_ir::{BinaryOp, Component, DType, Kind, Module, Shape};

    fn chain_side(label: &str, first_expr: Expr) -> ModelSide {
        // a → b → c(Output), plus an unrelated d.
        let mut m = Module::new("m");
        m.components.push(Component::derived(
            "a",
            DType::F64,
            Shape::Series,
            first_expr,
        ));
        m.components.push(Component::derived(
            "b",
            DType::F64,
            Shape::Series,
            Expr::binary(BinaryOp::Mul, Expr::r#ref("a"), Expr::f64(2.0)),
        ));
        let mut c = Component::derived("c", DType::F64, Shape::Series, Expr::r#ref("b"));
        c.kind = Kind::Output;
        m.components.push(c);
        m.components.push(Component::derived(
            "d",
            DType::F64,
            Shape::Series,
            Expr::f64(7.0),
        ));
        let mut side = ModelSide::new(label);
        side.modules.push(m);
        side
    }

    #[test]
    fn a_change_reaches_its_whole_downstream_closure_and_no_further() {
        let a = chain_side("a", Expr::f64(1.0));
        let b = chain_side("b", Expr::f64(2.0));
        let d = crate::diff(&a, &b);
        assert_eq!(d.impact.seeds, vec!["a".to_string()]);
        assert_eq!(d.impact.impacted, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(d.impact.outputs_affected, vec!["c".to_string()]);
        assert_eq!(d.summary.components_impacted, 2);
        assert!(!d.impact.impacted.contains(&"d".to_string()));
    }
}
