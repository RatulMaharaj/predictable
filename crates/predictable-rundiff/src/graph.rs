//! The IR dependency graph, as the run diff needs it: *which inputs, at which `t`*.
//!
//! §5.3 step 4: *"A diverging component whose inputs **at the relevant `t`** all agree within
//! tolerance is a root divergence; one whose inputs also diverge is inherited."* The offset matters
//! — a component reading `x[t-1]` is inherited when `x` diverges at `t-1`, not at `t` — so the
//! edges here carry the lag, which the model diff's own impact set (a pure reachability question)
//! does not need.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use predictable_ir::Expr;
use predictable_modeldiff::ModelSide;

/// One dependency edge: `component` reads `input` at `t + offset`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    /// The qualified id of the component read.
    pub input: String,
    /// `0` for a plain reference, `-k` for `x[t-k]`, and `None` for `x@k`, an absolute
    /// reference whose `t` does not move with the reader's.
    pub offset: Option<i32>,
}

/// `component id → what it reads`, plus the reverse edges and the output set.
#[derive(Debug, Clone, Default)]
pub struct DepGraph {
    inputs: BTreeMap<String, BTreeSet<Edge>>,
    dependents: BTreeMap<String, BTreeSet<String>>,
    outputs: BTreeSet<String>,
    /// Bare name → qualified id, so a diff can talk about `bel` and mean `model.bel`.
    qualified: BTreeMap<String, String>,
}

impl DepGraph {
    /// Build the graph from a model.
    ///
    /// Ids are qualified exactly as `results.parquet` qualifies them (`<module_path>.<name>`), so
    /// the graph and the result set share one vocabulary and no lookup has to guess.
    pub fn from_model(side: &ModelSide) -> DepGraph {
        let mut g = DepGraph::default();
        for module in &side.modules {
            for c in &module.components {
                let id = c.qualified_id(&module.module);
                g.qualified.insert(c.name.clone(), id.clone());
                if c.kind.is_emitted() {
                    g.outputs.insert(id.clone());
                }
                g.inputs.entry(id).or_default();
            }
        }
        // A second pass, so every name is qualifiable before any edge is resolved.
        for module in &side.modules {
            for c in &module.components {
                let id = c.qualified_id(&module.module);
                let mut reads: BTreeSet<Edge> = BTreeSet::new();
                for tree in [c.expr.as_ref(), c.init.as_ref()].into_iter().flatten() {
                    collect(tree, &mut reads);
                }
                for edge in reads {
                    let input = g.qualified.get(&edge.input).cloned().unwrap_or(edge.input);
                    if input == id {
                        continue; // `x[t-1]` inside `x` is recursion, not a dependency
                    }
                    g.dependents
                        .entry(input.clone())
                        .or_default()
                        .insert(id.clone());
                    g.inputs.entry(id.clone()).or_default().insert(Edge {
                        input,
                        offset: edge.offset,
                    });
                }
            }
        }
        g
    }

    /// True when the graph knows this component.
    pub fn knows(&self, id: &str) -> bool {
        self.inputs.contains_key(id)
    }

    /// What `id` reads.
    pub fn inputs_of(&self, id: &str) -> impl Iterator<Item = &Edge> {
        self.inputs.get(id).into_iter().flatten()
    }

    /// Every emitted component reachable downstream of `id`, including `id` when it is one.
    pub fn outputs_affected(&self, id: &str) -> Vec<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(id);
        seen.insert(id.to_string());
        let mut out: BTreeSet<String> = BTreeSet::new();
        if self.outputs.contains(id) {
            out.insert(id.to_string());
        }
        while let Some(node) = queue.pop_front() {
            for down in self.dependents.get(node).into_iter().flatten() {
                if !seen.insert(down.clone()) {
                    continue;
                }
                if self.outputs.contains(down) {
                    out.insert(down.clone());
                }
                queue.push_back(down);
            }
        }
        out.into_iter().collect()
    }

    /// The qualified id of a bare component name, when the model declares one.
    pub fn qualify(&self, name: &str) -> Option<&str> {
        self.qualified.get(name).map(String::as_str)
    }

    /// Number of components. `0` means "no graph", which the classifier must say out loud.
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// True when the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }
}

fn collect(e: &Expr, out: &mut BTreeSet<Edge>) {
    match e {
        Expr::Ref { name } => {
            out.insert(Edge {
                input: name.clone(),
                offset: Some(0),
            });
        }
        Expr::Lag { name, k } => {
            out.insert(Edge {
                input: name.clone(),
                offset: Some(-(*k as i32)),
            });
        }
        Expr::At { name, .. } => {
            out.insert(Edge {
                input: name.clone(),
                // An absolute `x@k` does not move with the reader's `t`; the classifier treats
                // that as "unknown offset" and considers the whole of `x`.
                offset: None,
            });
        }
        _ => {}
    }
    for (_, child) in e.children() {
        collect(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelSide {
        let text = r#"format = "pir/1"
module = "m"

[[modelpoint_field]]
name = "pol"
dtype = "str"
required = true
key = true

[[component]]
name = "a"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
init = "1.0"
expr = "a[t-1] * 2.0"

[[component]]
name = "b"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "a[t-1] + 1.0"

[[component]]
name = "c"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "b * 3.0"
"#;
        ModelSide::from_inputs("m", [("m.pir", text)])
    }

    #[test]
    fn ids_are_qualified_the_way_results_qualify_them() {
        let g = DepGraph::from_model(&model());
        assert!(g.knows("m.a"));
        assert_eq!(g.qualify("c"), Some("m.c"));
    }

    #[test]
    fn a_lag_edge_carries_its_offset_and_self_recursion_is_not_an_edge() {
        let g = DepGraph::from_model(&model());
        let a_inputs: Vec<&Edge> = g.inputs_of("m.a").collect();
        assert!(a_inputs.is_empty(), "{a_inputs:?}");
        let b_inputs: Vec<&Edge> = g.inputs_of("m.b").collect();
        assert_eq!(
            b_inputs,
            vec![&Edge {
                input: "m.a".into(),
                offset: Some(-1)
            }]
        );
    }

    #[test]
    fn outputs_affected_is_the_transitive_downstream_output_set() {
        let g = DepGraph::from_model(&model());
        assert_eq!(g.outputs_affected("m.a"), vec!["m.c".to_string()]);
        assert_eq!(g.outputs_affected("m.c"), vec!["m.c".to_string()]);
    }
}
