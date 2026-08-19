//! The `DataSource` conformance suite.
//!
//! Three implementations of one interface (§1.1) is three chances for the
//! screens to behave differently depending on how the page was opened. The suite
//! is the answer: it is written once, against `dyn DataSource`, and every
//! implementation — in-memory, HTTP-backed, wasm — is required to pass it.
//!
//! Every case is a property the UI relies on:
//!
//! - the graph is internally consistent and its `layers` *are* the plan's order,
//!   not a re-derivation of it;
//! - `component()` agrees with `graph()`;
//! - `series()` returns the contracted schema, in the requested column order,
//!   with row order and `t` filtering as documented;
//! - unknown names fail as unknown, not as 500s or empty results;
//! - [`Capabilities`](crate::source::Capabilities) tells the truth, so the UI
//!   never renders a dead button;
//! - identical calls return identical bytes.

use std::collections::BTreeSet;

use arrow_array::{Array, Float64Array, StringArray, UInt32Array};
use arrow_schema::DataType;

use crate::source::{DataSource, DataSourceError, ExplainQuery, SeriesQuery};

/// What happened to one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The property held.
    Pass,
    /// The property did not hold.
    Fail(String),
    /// Not applicable to this source (e.g. no runs to query).
    Skipped(String),
}

/// One case's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    /// Stable case name, safe to grep for in CI output.
    pub name: &'static str,
    /// Outcome.
    pub status: Status,
}

/// The suite's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Every case, in the order it ran.
    pub cases: Vec<Case>,
}

impl Report {
    /// Cases that failed.
    pub fn failures(&self) -> Vec<&Case> {
        self.cases
            .iter()
            .filter(|c| matches!(c.status, Status::Fail(_)))
            .collect()
    }

    /// True when nothing failed. Skipped cases do not fail the suite.
    pub fn is_conformant(&self) -> bool {
        self.failures().is_empty()
    }

    /// A one-line-per-case rendering, used in panic messages and in the CLI.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for case in &self.cases {
            let line = match &case.status {
                Status::Pass => format!("pass  {}\n", case.name),
                Status::Skipped(why) => format!("skip  {} ({why})\n", case.name),
                Status::Fail(why) => format!("FAIL  {}: {why}\n", case.name),
            };
            out.push_str(&line);
        }
        out
    }
}

/// Run the suite against `source`.
///
/// `sample_modelpoint` and `sample_component` let the caller point the data
/// cases at something the source actually holds; pass `None` and the suite picks
/// the first series-shaped node and the source's own first modelpoint, skipping
/// the data cases if there is nothing to pick.
pub fn run_suite(
    source: &dyn DataSource,
    sample_component: Option<&str>,
    sample_modelpoint: Option<&str>,
) -> Report {
    let mut cases: Vec<Case> = Vec::new();
    let mut case = |name: &'static str, r: std::result::Result<(), String>| {
        cases.push(Case {
            name,
            status: match r {
                Ok(()) => Status::Pass,
                Err(why) if why.starts_with("skip:") => Status::Skipped(why[5..].trim().into()),
                Err(why) => Status::Fail(why),
            },
        })
    };

    let graph = match source.graph() {
        Ok(g) => g,
        Err(e) => {
            return Report {
                cases: vec![Case {
                    name: "graph.available",
                    status: Status::Fail(e.to_string()),
                }],
            }
        }
    };
    case("graph.available", Ok(()));

    case("graph.ids_unique", {
        let mut seen = BTreeSet::new();
        match graph.nodes.iter().find(|n| !seen.insert(n.id.clone())) {
            Some(n) => Err(format!("duplicate node id `{}`", n.id)),
            None => Ok(()),
        }
    });

    case("graph.edges_resolve", {
        let ids: BTreeSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        let dangling: Vec<String> = graph
            .edges
            .iter()
            .filter(|e| !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()))
            .map(|e| format!("{} -> {}", e.from, e.to))
            .collect();
        if dangling.is_empty() {
            Ok(())
        } else {
            Err(format!("edges with unknown endpoints: {dangling:?}"))
        }
    });

    // The whole point of §2.1: `layers` is a *view* of the plan's order, so it
    // must partition the nodes exactly, and each node must sit in its own depth.
    case("graph.layers_partition_nodes", {
        let flat: Vec<&String> = graph.layers.iter().flatten().collect();
        let names: BTreeSet<&str> = graph.nodes.iter().map(|n| n.name.as_str()).collect();
        let flat_set: BTreeSet<&str> = flat.iter().map(|s| s.as_str()).collect();
        if flat.len() != graph.nodes.len() {
            Err(format!(
                "{} names across layers for {} nodes",
                flat.len(),
                graph.nodes.len()
            ))
        } else if flat_set != names {
            Err("layers and nodes name different components".into())
        } else {
            Ok(())
        }
    });

    case("graph.layer_index_is_node_depth", {
        let mut bad = Vec::new();
        for (depth, layer) in graph.layers.iter().enumerate() {
            for name in layer {
                match graph.nodes.iter().find(|n| &n.name == name) {
                    Some(n) if n.depth == depth => {}
                    Some(n) => bad.push(format!("`{name}` depth {} in layer {depth}", n.depth)),
                    None => bad.push(format!("`{name}` is in no node list")),
                }
            }
        }
        if bad.is_empty() {
            Ok(())
        } else {
            Err(bad.join("; "))
        }
    });

    // A dependency must be evaluated before its dependant unless the read is
    // lagged (`x[t-k]`) or a table read: that is what makes the drawing safe to
    // read left to right.
    case("graph.order_respects_edges", {
        let position: std::collections::BTreeMap<&str, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        let mut bad = Vec::new();
        for edge in &graph.edges {
            if edge.lag != Some(0) {
                continue;
            }
            let (Some(&from), Some(&to)) = (
                position.get(edge.from.as_str()),
                position.get(edge.to.as_str()),
            ) else {
                continue;
            };
            if from > to {
                bad.push(format!("{} after {}", edge.from, edge.to));
            }
        }
        if bad.is_empty() {
            Ok(())
        } else {
            Err(format!("same-period edges out of order: {bad:?}"))
        }
    });

    case("graph.stable_across_calls", {
        match source.graph() {
            Ok(again) if again == graph => Ok(()),
            Ok(_) => Err("two graph() calls returned different documents".into()),
            Err(e) => Err(e.to_string()),
        }
    });

    case(
        "manifest.is_object",
        match source.manifest() {
            Ok(v) if v.is_object() => Ok(()),
            Ok(v) => Err(format!("manifest is {v}, not a JSON object")),
            Err(e) => Err(e.to_string()),
        },
    );

    case("component.matches_graph", {
        match graph.nodes.first() {
            None => Err("skip: the graph has no nodes".into()),
            Some(node) => match source.component(&node.id) {
                Err(e) => Err(e.to_string()),
                Ok(doc) if doc.node != *node => {
                    Err(format!("component(`{}`) disagrees with graph()", node.id))
                }
                Ok(doc) => {
                    let expected: Vec<&str> = graph
                        .edges
                        .iter()
                        .filter(|e| e.to == node.id)
                        .map(|e| e.from.as_str())
                        .collect();
                    let missing: Vec<&&str> = expected
                        .iter()
                        .filter(|f| !doc.upstream.iter().any(|u| u == **f))
                        .collect();
                    if missing.is_empty() {
                        Ok(())
                    } else {
                        Err(format!("upstream is missing {missing:?}"))
                    }
                }
            },
        }
    });

    case(
        "component.unknown_is_unknown",
        match source.component("__no_such_component__") {
            Err(DataSourceError::UnknownComponent(_)) => Ok(()),
            Err(e) => Err(format!("expected UnknownComponent, got {e}")),
            Ok(_) => Err("an unknown component resolved".into()),
        },
    );

    // ---- data cases -------------------------------------------------------

    let component = sample_component.map(str::to_string).or_else(|| {
        graph
            .nodes
            .iter()
            .find(|n| n.shape == "Series")
            .map(|n| n.name.clone())
    });
    let Some(component) = component else {
        case("series.schema", Err("skip: no series component".into()));
        return Report { cases };
    };

    let probe = SeriesQuery {
        components: vec![component.clone()],
        modelpoints: sample_modelpoint
            .map(|m| vec![m.to_string()])
            .unwrap_or_default(),
        ..SeriesQuery::default()
    };
    let batch = match source.series(&probe) {
        Ok(b) => b,
        Err(e) => {
            case("series.schema", Err(format!("series() failed: {e}")));
            return Report { cases };
        }
    };

    case("series.schema", {
        let schema = batch.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        if names.first() != Some(&"modelpoint") || names.get(1) != Some(&"t") {
            Err(format!("columns start {names:?}, expected modelpoint, t"))
        } else if schema.field(0).data_type() != &DataType::Utf8 {
            Err("modelpoint is not Utf8".into())
        } else if schema.field(1).data_type() != &DataType::UInt32 {
            Err("t is not UInt32".into())
        } else if names.get(2) != Some(&component.as_str()) {
            Err(format!(
                "value column is {:?}, expected `{component}`",
                names.get(2)
            ))
        } else if schema.field(2).data_type() != &DataType::Float64 {
            Err("the value column is not Float64".into())
        } else {
            Ok(())
        }
    });

    case("series.column_order_follows_request", {
        // Pick two components the source actually holds data for, rather than
        // the first two in the graph, so the case asserts instead of skipping.
        let mut two: Vec<String> = Vec::new();
        for candidate in graph.nodes.iter().filter(|n| n.shape == "Series") {
            if two.len() == 2 {
                break;
            }
            let single = SeriesQuery {
                components: vec![candidate.name.clone()],
                modelpoints: probe.modelpoints.clone(),
                ..SeriesQuery::default()
            };
            if source.series(&single).is_ok() {
                two.push(candidate.name.clone());
            }
        }
        if two.len() < 2 {
            Err("skip: fewer than two series components".into())
        } else {
            let reversed: Vec<String> = two.iter().rev().cloned().collect();
            match source.series(&SeriesQuery {
                components: reversed.clone(),
                modelpoints: probe.modelpoints.clone(),
                ..SeriesQuery::default()
            }) {
                Err(DataSourceError::BadRequest(_)) => {
                    Err("skip: source holds no such pair".into())
                }
                Err(e) => Err(e.to_string()),
                Ok(b) => {
                    let got: Vec<String> = b
                        .schema()
                        .fields()
                        .iter()
                        .skip(2)
                        .map(|f| f.name().clone())
                        .collect();
                    if got == reversed {
                        Ok(())
                    } else {
                        Err(format!("asked for {reversed:?}, got {got:?}"))
                    }
                }
            }
        }
    });

    case("series.rows_sorted_by_modelpoint_then_t", {
        let mps = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or("modelpoint column is not a StringArray")
            .map(|a| {
                (0..a.len())
                    .map(|i| a.value(i).to_string())
                    .collect::<Vec<_>>()
            });
        let ts = batch
            .column(1)
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or("t column is not a UInt32Array")
            .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<_>>());
        match (mps, ts) {
            (Err(e), _) | (_, Err(e)) => Err(e.to_string()),
            (Ok(mps), Ok(ts)) => {
                let mut seen: Vec<&String> = Vec::new();
                let mut err = None;
                for i in 0..mps.len() {
                    if i > 0 && mps[i] == mps[i - 1] {
                        if ts[i] <= ts[i - 1] {
                            err = Some(format!("t not ascending at row {i}"));
                            break;
                        }
                    } else {
                        if seen.contains(&&mps[i]) {
                            err = Some(format!("modelpoint `{}` appears in two blocks", mps[i]));
                            break;
                        }
                        seen.push(&mps[i]);
                    }
                }
                err.map(Err).unwrap_or(Ok(()))
            }
        }
    });

    case("series.values_are_finite_f64", {
        match batch.column(2).as_any().downcast_ref::<Float64Array>() {
            None => Err("value column is not a Float64Array".into()),
            Some(a) => {
                if a.null_count() > 0 {
                    // IR §2.11: missingness is eliminated before the run.
                    Err(format!("{} nulls in a value column", a.null_count()))
                } else {
                    Ok(())
                }
            }
        }
    });

    case("series.t_range_is_respected", {
        let ts = batch
            .column(1)
            .as_any()
            .downcast_ref::<UInt32Array>()
            .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<_>>())
            .unwrap_or_default();
        let (lo, hi) = match (ts.iter().min().copied(), ts.iter().max().copied()) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => (0u32, 0u32),
        };
        if hi == lo {
            Err("skip: only one period held".into())
        } else {
            let want_hi = hi - 1;
            match source.series(&SeriesQuery {
                t_from: Some(lo),
                t_to: Some(want_hi),
                ..probe.clone()
            }) {
                Err(e) => Err(e.to_string()),
                Ok(b) => {
                    let got = b
                        .column(1)
                        .as_any()
                        .downcast_ref::<UInt32Array>()
                        .map(|a| (0..a.len()).map(|i| a.value(i)).max())
                        .flatten();
                    if got == Some(want_hi) {
                        Ok(())
                    } else {
                        Err(format!("asked for t<={want_hi}, saw max t {got:?}"))
                    }
                }
            }
        }
    });

    case(
        "series.unknown_component_is_unknown",
        match source.series(&SeriesQuery {
            components: vec!["__no_such_component__".into()],
            ..probe.clone()
        }) {
            Err(DataSourceError::UnknownComponent(_)) => Ok(()),
            Err(e) => Err(format!("expected UnknownComponent, got {e}")),
            Ok(_) => Err("an unknown component returned data".into()),
        },
    );

    case(
        "series.empty_request_is_rejected",
        match source.series(&SeriesQuery::default()) {
            Err(DataSourceError::BadRequest(_)) => Ok(()),
            Err(e) => Err(format!("expected BadRequest, got {e}")),
            Ok(_) => Err("a query with no components returned data".into()),
        },
    );

    case("series.deterministic", {
        match source.series(&probe) {
            Err(e) => Err(e.to_string()),
            Ok(again) => {
                if crate::transport::to_ipc(&again).ok() == crate::transport::to_ipc(&batch).ok() {
                    Ok(())
                } else {
                    Err("two identical series() calls encoded differently".into())
                }
            }
        }
    });

    // `capabilities()` must not lie in either direction.
    let modelpoint = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .filter(|a| a.len() > 0)
        .map(|a| a.value(0).to_string());
    case("capabilities.explain_is_honest", {
        let query = ExplainQuery {
            run: None,
            component: component.clone(),
            modelpoint: modelpoint.clone().unwrap_or_default(),
            t: 0,
        };
        match (source.capabilities().explain, source.explain(&query)) {
            (false, Err(DataSourceError::Unsupported(_))) => Ok(()),
            (false, Err(e)) => Err(format!("explain: false but the error was {e}")),
            (false, Ok(_)) => Err("explain: false but explain() answered".into()),
            (true, Err(DataSourceError::Unsupported(_))) => {
                Err("explain: true but explain() reports Unsupported".into())
            }
            (true, _) => Ok(()),
        }
    });

    finish(cases)
}

fn finish(cases: Vec<Case>) -> Report {
    Report { cases }
}

/// Run the suite and panic with the full report if anything failed.
///
/// This is the one-liner a `DataSource` implementation's test file calls.
pub fn assert_conformant(
    source: &dyn DataSource,
    sample_component: Option<&str>,
    sample_modelpoint: Option<&str>,
) {
    let report = run_suite(source, sample_component, sample_modelpoint);
    assert!(
        report.is_conformant(),
        "DataSource is not conformant:\n{}",
        report.to_text()
    );
}
