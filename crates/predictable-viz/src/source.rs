//! The `DataSource` contract — `05-viz.md` §1.1.
//!
//! The SPA is written against one interface; the three delivery modes (local
//! server, inline payload, wasm) differ only in which implementation is injected.
//! This module is the Rust half of that interface: the server in
//! [`crate::server`] is a thin, mode-agnostic adapter over a `dyn DataSource`,
//! and [`crate::conformance`] is the suite every implementation must pass.
//!
//! Two rules are load-bearing and are checked by the conformance suite rather
//! than trusted:
//!
//! 1. **The viz layer never computes an actuarial number.** `series` and
//!    `aggregate` return what the engine produced; `explain` is the only call
//!    that may compute, and only by replaying the engine.
//! 2. **[`Capabilities`] tells the truth.** A source that reports
//!    `explain: false` must fail `explain` with [`DataSourceError::Unsupported`]
//!    so the UI can grey the button out instead of offering a dead one.

use arrow_array::RecordBatch;
use serde::{Deserialize, Serialize};

use crate::graphdoc::{GraphDoc, GraphNode};

/// What went wrong. The HTTP layer maps each variant to a status code, so
/// implementations never think about HTTP.
#[derive(Debug, thiserror::Error)]
pub enum DataSourceError {
    /// No component with that name in the graph. → 404.
    #[error("unknown component `{0}`")]
    UnknownComponent(String),
    /// No such run. → 404.
    #[error("unknown run `{0}`")]
    UnknownRun(String),
    /// No such modelpoint in the run. → 404.
    #[error("unknown modelpoint `{0}`")]
    UnknownModelpoint(String),
    /// The query is malformed — empty component list, `t` range inverted. → 400.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// The capability is off for this source. → 501.
    #[error("`{0}` is not available from this data source")]
    Unsupported(&'static str),
    /// Anything else. → 500.
    #[error("{0}")]
    Internal(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, DataSourceError>;

/// What this source can honestly do (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// `explain()` can be answered on demand.
    pub explain: bool,
    /// The engine is present and a projection can be re-run.
    pub recompute: bool,
    /// Sensitivity fans can be produced.
    pub sensitivity: bool,
}

impl Default for Capabilities {
    /// The static-export default: read-only, no engine.
    fn default() -> Capabilities {
        Capabilities {
            explain: false,
            recompute: false,
            sensitivity: false,
        }
    }
}

/// `GET /api/series` — which numbers, for which modelpoints, over which periods.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesQuery {
    /// Run id; `None` means the source's only/current run.
    #[serde(default)]
    pub run: Option<String>,
    /// Component names, in the order the columns must come back in.
    pub components: Vec<String>,
    /// Modelpoint keys; empty means every modelpoint the source holds.
    #[serde(default)]
    pub modelpoints: Vec<String>,
    /// First period, inclusive. `None` means 0.
    #[serde(default)]
    pub t_from: Option<u32>,
    /// Last period, inclusive. `None` means the last period held.
    #[serde(default)]
    pub t_to: Option<u32>,
}

/// `GET /api/aggregate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateQuery {
    /// Run id; `None` means the source's only/current run.
    #[serde(default)]
    pub run: Option<String>,
    /// The declared `[[aggregation]]` measure (IR §8.3).
    pub measure: String,
    /// Ordered grouping key tuple.
    #[serde(default)]
    pub group_by: Vec<String>,
}

/// `POST /api/explain` — the only call that may compute (IR §11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplainQuery {
    /// Run id; `None` means the source's only/current run.
    #[serde(default)]
    pub run: Option<String>,
    /// Component to trace.
    pub component: String,
    /// Modelpoint to trace it on.
    pub modelpoint: String,
    /// Period.
    pub t: u32,
}

/// `GET /api/component/{name}` — one node plus its immediate neighbourhood.
///
/// Both neighbour lists are read out of the [`GraphDoc`]'s edges, so they are
/// the planner's dependencies and not a second opinion about them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentDoc {
    /// The node itself.
    pub node: GraphNode,
    /// Qualified ids this component reads, in graph order, de-duplicated.
    pub upstream: Vec<String>,
    /// Qualified ids that read this component, in graph order, de-duplicated.
    pub downstream: Vec<String>,
}

impl ComponentDoc {
    /// Derive the document for `name` (qualified id or bare name) from a graph.
    pub fn from_graph(graph: &GraphDoc, name: &str) -> Result<ComponentDoc> {
        let node = graph
            .nodes
            .iter()
            .find(|n| n.id == name || n.name == name)
            .ok_or_else(|| DataSourceError::UnknownComponent(name.to_string()))?;
        let mut upstream = Vec::new();
        let mut downstream = Vec::new();
        for edge in &graph.edges {
            if edge.to == node.id && !upstream.contains(&edge.from) {
                upstream.push(edge.from.clone());
            }
            if edge.from == node.id && !downstream.contains(&edge.to) {
                downstream.push(edge.to.clone());
            }
        }
        Ok(ComponentDoc {
            node: node.clone(),
            upstream,
            downstream,
        })
    }
}

/// The contract every delivery mode implements (§1.1).
///
/// Numbers travel as an Arrow [`RecordBatch`]; the transport encodes it as an
/// Arrow IPC stream (§1.2). Everything else is JSON. Schemas the suite enforces:
///
/// | call | schema |
/// |---|---|
/// | `series` | `modelpoint: Utf8`, `t: UInt32`, then one `Float64` per requested component, in request order |
/// | `aggregate` | one `Utf8` per `group_by` key, in order, then `value: Float64` |
///
/// `series` rows are ordered by modelpoint (in the order requested, or the
/// source's own stable order when none was requested) then by `t` ascending.
pub trait DataSource: Send + Sync + 'static {
    /// What this source can do. Must agree with what the calls actually do.
    fn capabilities(&self) -> Capabilities;

    /// The run manifest (`04-verify.md` §6), verbatim.
    fn manifest(&self) -> Result<serde_json::Value>;

    /// The graph document (§2.1), from the planner's own ordering.
    fn graph(&self) -> Result<GraphDoc>;

    /// One component and its neighbours. Defaults to a view over [`Self::graph`].
    fn component(&self, name: &str) -> Result<ComponentDoc> {
        ComponentDoc::from_graph(&self.graph()?, name)
    }

    /// Numbers the engine produced. Never recomputed here.
    fn series(&self, query: &SeriesQuery) -> Result<RecordBatch>;

    /// Declared aggregations (IR §8.3).
    fn aggregate(&self, _query: &AggregateQuery) -> Result<RecordBatch> {
        Err(DataSourceError::Unsupported("aggregate"))
    }

    /// A trace tree (`04-verify.md` §3.2). Only legal when `capabilities().explain`.
    fn explain(&self, _query: &ExplainQuery) -> Result<serde_json::Value> {
        Err(DataSourceError::Unsupported("explain"))
    }

    /// A `RunDiffDoc` (`04-verify.md` §5.4).
    fn diff(&self, _a: &str, _b: &str) -> Result<serde_json::Value> {
        Err(DataSourceError::Unsupported("diff"))
    }
}
