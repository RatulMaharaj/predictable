//! Products and run configuration — the two file kinds added by the decision log (§8.4).
//!
//! Both are `.pir`: the same restricted TOML, the same parser, the same digests.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The `[product]` block (§8.4.1): a named entry point — the closed module set plus the
/// manifest of outputs it promises.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Product {
    pub name: String,
    /// Exactly the modules loaded, in this order. That order is the `module_path` order used by
    /// §3.2's tiebreak and by `model_digest`. A module reached by `imports` but absent here is
    /// `E0105`.
    pub modules: Vec<String>,
    /// A **validated manifest**, not a selector: it must equal the set of `kind = "Output"`
    /// components across `modules`, or `E0107` (Q2).
    pub outputs: Vec<String>,
    /// The `modelpoint_field` with `key = true`, restated so the results join key is knowable
    /// from the product alone.
    pub key_field: String,
    /// Default assumption set; a run may override it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumptions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// A product file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductFile {
    pub format: String,
    pub product: Product,
}

/// Which components a run emits (§8.4.3). `emit` never *removes* an `Output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Emit {
    /// `kind = "Output"` components. The default.
    #[default]
    Outputs,
    /// Every `Derived` and `Output` component — the migration mode.
    All,
    /// `emit_list` ∪ outputs; each id must resolve, or `E0106`.
    List,
}

/// Series retention policy (§8.4.2). `ring` makes non-emitted series unavailable to
/// `explain()` replay, which is why it participates in `run_digest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Retain {
    #[default]
    Ring,
    Full,
}

/// Reserved in IR 1.0 (Q15). Only `f64` is accepted; `f32` is `E0108`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoragePrecision {
    #[default]
    F64,
    /// Parsed so the key exists in the grammar; rejected by the checker with `E0108`.
    F32,
}

impl StoragePrecision {
    /// True when this value is accepted by IR 1.0.
    pub fn is_supported_in_1_0(self) -> bool {
        matches!(self, StoragePrecision::F64)
    }
}

/// Trap policy (§9.3.1, Q7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnTrap {
    /// Report, stop, exit 2, write a manifest but no results.
    #[default]
    Abort,
    /// Drop the trapping modelpoint entirely — no null rows — and exit 1.
    Continue,
}

/// Non-semantic execution settings. Excluded from `run_digest` because
/// `run(threads = 1) ≡ run(threads = 64)` bit for bit (§9.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: u32,
    #[serde(default = "default_true")]
    pub progress: bool,
}

fn default_chunk_size() -> u32 {
    1024
}

fn default_true() -> bool {
    true
}

impl Default for ExecConfig {
    fn default() -> Self {
        ExecConfig {
            threads: None,
            chunk_size: default_chunk_size(),
            progress: true,
        }
    }
}

fn default_max_errors() -> u32 {
    100
}

/// The `[run]` block (§8.4.2, Q1).
///
/// A run may not set `periods`, `basis`, `origin`, `valuation_date` or `year_convention`:
/// the timeline is a property of the model (`E0101`), and there is no field here to set them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunConfig {
    pub product: String,
    /// Overrides `product.assumptions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumptions: Option<String>,
    pub modelpoints: String,
    /// Where results are written. *Not* part of `run_digest` — where results are written is not
    /// what they are.
    pub out: String,

    #[serde(default)]
    pub emit: Emit,
    /// Required iff `emit = "list"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emit_list: Vec<String>,
    #[serde(default)]
    pub retain: Retain,
    #[serde(default)]
    pub storage_precision: StoragePrecision,

    #[serde(default)]
    pub on_trap: OnTrap,
    #[serde(default = "default_max_errors")]
    pub max_errors: u32,
    #[serde(default)]
    pub allow_table_drift: bool,
    #[serde(default)]
    pub sum_kahan: bool,

    /// Per-table `source` override. The `digest` may never be overridden (§2.9.1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tables: BTreeMap<String, String>,

    #[serde(default)]
    pub exec: ExecConfig,
}

/// Root-find wrapped around the projection (§8.4.4, Q8). The projection itself stays pure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solve {
    pub name: String,
    /// A `PerMP` component (per-mp scope) or an `[[aggregation]]` name (portfolio scope).
    pub target: String,
    pub to: f64,
    /// An `Input.Modelpoint` field or a `Scalar` `Input.Assumption`.
    pub vary: String,
    #[serde(default)]
    pub scope: SolveScope,
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    #[serde(default = "default_max_iter")]
    pub max_iter: u32,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bracket: Option<[f64; 2]>,
    /// Defaults to `error` for `portfolio` scope, `warn` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_not_converged: Option<OnNotConverged>,
}

fn default_tolerance() -> f64 {
    1e-8
}

fn default_max_iter() -> u32 {
    50
}

fn default_method() -> String {
    "brent".to_string()
}

/// Whether a solve runs per modelpoint or once for the portfolio (§8.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolveScope {
    #[default]
    PerMp,
    Portfolio,
}

/// What a non-converged solve does (§8.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnNotConverged {
    Warn,
    /// Fails the run with `E0903`.
    Error,
}

impl Solve {
    /// The effective `on_not_converged`, applying the scope-dependent default (§8.4.4).
    pub fn effective_on_not_converged(&self) -> OnNotConverged {
        self.on_not_converged.unwrap_or(match self.scope {
            SolveScope::Portfolio => OnNotConverged::Error,
            SolveScope::PerMp => OnNotConverged::Warn,
        })
    }

    /// A `scope = "per_mp"` solve writes `solves/<name>.parquet`; a portfolio solve does not.
    pub fn writes_per_mp_file(&self) -> bool {
        matches!(self.scope, SolveScope::PerMp)
    }
}

/// Aggregation monoid (§8.3, Q4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregationOp {
    Sum,
    Mean,
    Min,
    Max,
    Count,
    WeightedMean,
}

impl AggregationOp {
    /// True iff a `weight` is required (and legal) for this op.
    pub fn requires_weight(self) -> bool {
        matches!(self, AggregationOp::WeightedMean)
    }
}

/// How a `Series` measure is reduced over `t` (§8.3). Absent for a `PerMP` measure (`E0404`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverT {
    /// One row per `(group, t)`. The default for a `Series` measure.
    Each,
    /// One row per group, summed over `t`.
    Total,
}

/// A portfolio aggregation, declared in the run file — it is a property of what you are
/// reporting, not of the model (§8.3, Q4). Groupings do not nest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aggregation {
    pub name: String,
    /// One *ordered key tuple* producing one flat row per distinct tuple. A drill-down tree is
    /// built from successive prefixes; there is no subtotal row.
    pub group_by: Vec<String>,
    /// A single component id, `PerMP` or `Series`.
    pub measure: String,
    pub op: AggregationOp,
    /// Required iff `op = "weighted_mean"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<String>,
    /// A `bool` `PerMP` component id — a reference, never an expression, so every predicate
    /// that affects a reported number is a named, diffable, explainable thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_t: Option<OverT>,
}

impl Aggregation {
    /// Render a group tuple as the normative `group_key`: `k1=v1|k2=v2` in `group_by` order
    /// (§8.3). It is a join key; it is stable.
    pub fn group_key(&self, values: &[String]) -> String {
        self.group_by
            .iter()
            .zip(values)
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// A run file: the `[run]` block plus its `[[solve]]` and `[[aggregation]]` blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFile {
    pub format: String,
    pub run: RunConfig,
    #[serde(default, rename = "solve", skip_serializing_if = "Vec::is_empty")]
    pub solves: Vec<Solve>,
    #[serde(default, rename = "aggregation", skip_serializing_if = "Vec::is_empty")]
    pub aggregations: Vec<Aggregation>,
}

/// Fields of `[run]` that are excluded from `run_digest` (§8.4.2).
pub const RUN_DIGEST_EXCLUDED: &[&str] = &["out", "exec"];

impl RunFile {
    /// The digest-participating projection of this run file (§8.4.2).
    ///
    /// > A field participates in `run_digest` **iff changing it can change a number in
    /// > `results.parquet`**.
    ///
    /// So `[run.exec]` (threads, chunk size, progress) and `out` are dropped, and everything
    /// else — including `emit`, `retain`, `max_errors`, `on_trap`, `[run.tables]`, every
    /// `[[solve]]` and every `[[aggregation]]` — is kept. Hashing this value's canonical JSON
    /// is what `predictable run` does to produce `run_digest`; the exclusion rule lives here so
    /// there is exactly one implementation of it.
    pub fn digest_payload(&self) -> Value {
        let mut root = match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        if let Some(Value::Object(run)) = root.get_mut("run") {
            for key in RUN_DIGEST_EXCLUDED {
                run.remove(*key);
            }
        }
        Value::Object(root)
    }
}
