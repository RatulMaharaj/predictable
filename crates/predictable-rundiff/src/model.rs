//! The result of a run diff: the types `diff.json` is a rendering of (`04-verify.md` §5.4).
//!
//! These types are designed for the machine first. The terminal render in [`crate::render`] is a
//! projection of them; nothing is printed that is not also a field, because the feedback loop this
//! whole design rests on is an agent reading `diff.json`, not a human reading a table.

use serde::{Deserialize, Serialize};

use crate::hypothesis::Hypothesis;
use crate::run_side::Value;

/// `summary.verdict`, and the command's exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every compared cell is within tolerance. Exit 0.
    Matched,
    /// At least one cell is beyond tolerance. Exit 1.
    Diverged,
    /// The runs cannot be compared at all — different `T`, disjoint modelpoints, or
    /// `--require-same-emit` with different emit settings. Reported, not diffed. Exit 2.
    Incomparable,
}

impl Verdict {
    /// The exit code §5.1 assigns.
    pub fn exit_code(self) -> i32 {
        match self {
            Verdict::Matched => 0,
            Verdict::Diverged => 1,
            Verdict::Incomparable => 2,
        }
    }

    /// The word the report prints.
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Matched => "MATCHED",
            Verdict::Diverged => "DIVERGED",
            Verdict::Incomparable => "INCOMPARABLE",
        }
    }
}

/// `findings[].class` (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Every input of this component agrees at the relevant `t`: the divergence starts here.
    Root,
    /// An input of this component also diverges: this is downstream of somebody else's problem.
    Inherited,
    /// A component or modelpoint present on one side only.
    Structural,
    /// Cells that a per-component or per-unit override absorbed. Reported because a loosened
    /// tolerance must never be invisible; not a divergence, and never changes the verdict.
    ToleranceOnly,
}

impl Class {
    /// The word the report prints.
    pub fn word(self) -> &'static str {
        match self {
            Class::Root => "root",
            Class::Inherited => "inherited",
            Class::Structural => "structural",
            Class::ToleranceOnly => "tolerance-only",
        }
    }
}

/// `findings[].category`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Both sides have the cell; the numbers differ.
    Value,
    /// `a` has the cell and `b` does not.
    Missing,
    /// `b` has the cell and `a` does not.
    Extra,
    /// The two sides' `t` ranges differ for this component.
    Shape,
    /// The two sides store the component in different value lanes.
    Dtype,
    /// A `NaN` or an infinity on one side. Always severity-max, never a tolerance question.
    Nan,
}

impl Category {
    /// The word the report prints.
    pub fn word(self) -> &'static str {
        match self {
            Category::Value => "value",
            Category::Missing => "missing",
            Category::Extra => "extra",
            Category::Shape => "shape",
            Category::Dtype => "dtype",
            Category::Nan => "nan",
        }
    }
}

/// One diverging cell, kept so the report can name an exemplar and a worst case.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The modelpoint key.
    pub mp_key: String,
    /// The modelpoint's row index, the deterministic tiebreak.
    pub mp_row: u32,
    /// The timestep; `-1` for `Scalar`/`PerMP`.
    pub t: i32,
    /// The `a` value, absent when the cell is only in `b`.
    pub a: Option<Value>,
    /// The `b` value, absent when the cell is only in `a`.
    pub b: Option<Value>,
    /// `|a - b|` for numeric cells; `f64::NAN` when one side is absent or non-numeric.
    pub abs: f64,
    /// `|a - b| / max(|a|,|b|)`.
    pub rel: f64,
    /// Why this cell is a divergence.
    pub category: Category,
}

impl Cell {
    /// The cell as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "mp_key": self.mp_key,
            "mp_row": self.mp_row,
            "t": self.t,
            "a": self.a.as_ref().map(Value::to_json),
            "b": self.b.as_ref().map(Value::to_json),
            "abs": finite(self.abs),
            "rel": finite(self.rel),
            "category": self.category,
        })
    }
}

fn finite(x: f64) -> serde_json::Value {
    if x.is_finite() {
        serde_json::json!(x)
    } else {
        serde_json::Value::Null
    }
}

/// `findings[].contribution` — how much of the headline output's movement this component explains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    /// The output whose movement is being apportioned: the one with the largest `|Δ total|`.
    pub output: String,
    /// The component's signed delta sum over that output's total delta.
    pub share_of_total_delta: f64,
    /// How the share was computed. `delta_sum_ratio` is exact for a component the output sums
    /// linearly and an approximation otherwise; stating the method is what keeps it honest.
    pub method: String,
}

/// `findings[].explained_by_model_change` — step 5 of §5.3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Explained {
    /// True when the model diff marks this component as changed.
    pub changed: bool,
    /// The change components the model diff reported, e.g. `["formula"]`.
    pub what: Vec<String>,
}

/// One finding: an output, a component, and the earliest `t` at which they part company.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// `F001`, assigned after sorting so the ids match the printed order.
    pub id: String,
    /// Root, inherited, structural or tolerance-only.
    pub class: Class,
    /// Why the cells differ.
    pub category: Category,
    /// How `class` was decided — `ir_graph`, `partial_graph` or `no_graph_earliest_t`. A
    /// classification without its basis is not falsifiable.
    pub class_basis: String,
    /// The qualified component id, in `b`'s vocabulary.
    pub component: String,
    /// The `a`-side name when a mapping renamed it.
    pub source_component: Option<String>,
    /// The outputs this component reaches, from the IR impact set.
    pub affects_outputs: Vec<String>,
    /// The earliest diverging `t`.
    pub t_first: i32,
    /// `[first, last]` diverging `t`.
    pub t_range: [i32; 2],
    /// How many modelpoints exhibit the divergence.
    pub n_modelpoints: usize,
    /// How many cells of this component diverge.
    pub n_cells: usize,
    /// The lowest `mp_row` exhibiting the divergence at `t_first`.
    pub exemplar: Cell,
    /// The largest `|Δ|`.
    pub worst: Cell,
    /// The share of the headline output's movement.
    pub contribution: Option<Contribution>,
    /// Whether a model change explains this. `None` when no model was available on both sides —
    /// which is a different statement from "nothing changed".
    pub explained_by_model_change: Option<Explained>,
    /// The §5.2 one-line signal.
    pub message: String,
    /// Hypotheses, from the detectors of §5.5 — what the diff *proposes*, with the evidence that
    /// fired each one and, where the model source anchors it, the edit that would test it.
    pub hypotheses: Vec<Hypothesis>,
    /// The command that drills into the exemplar.
    pub explain_command: String,
    /// Every diverging cell, ascending by `(t, mp_row)`. Not serialised — this is the evidence a
    /// hypothesis detector reads, and it can be millions of cells.
    pub cells: Vec<Cell>,
}

impl Finding {
    /// The finding as JSON, in §5.4's field order.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "class": self.class,
            "category": self.category,
            "class_basis": self.class_basis,
            "component": self.component,
            "source_component": self.source_component,
            "affects_outputs": self.affects_outputs,
            "t_first": self.t_first,
            "t_range": self.t_range,
            "n_modelpoints": self.n_modelpoints,
            "n_cells": self.n_cells,
            "exemplar": self.exemplar.to_json(),
            "worst": self.worst.to_json(),
            "contribution": self.contribution,
            "explained_by_model_change": self.explained_by_model_change,
            "message": self.message,
            "hypotheses": self.hypotheses.iter().map(Hypothesis::to_json).collect::<Vec<_>>(),
            "explain_command": self.explain_command,
        })
    }

    /// The per-`t` divergence vectors a hypothesis detector needs (§5.5): for one modelpoint,
    /// ascending `t` with both sides' values.
    ///
    /// This is the hook T27's detectors hang off. It is deliberately a *view* over the finding's
    /// own cells rather than a re-read of the run: a detector that could see data the finding did
    /// not would be proposing hypotheses the evidence does not support.
    pub fn series_for(&self, mp_key: &str) -> Vec<(i32, f64, f64)> {
        let mut out: Vec<(i32, f64, f64)> = self
            .cells
            .iter()
            .filter(|c| c.mp_key == mp_key)
            .filter_map(|c| Some((c.t, c.a.as_ref()?.as_f64()?, c.b.as_ref()?.as_f64()?)))
            .collect();
        out.sort_by_key(|(t, _, _)| *t);
        out
    }

    /// The distinct modelpoint keys exhibiting this divergence, ascending by `mp_row`.
    pub fn modelpoints(&self) -> Vec<&str> {
        let mut seen: Vec<(u32, &str)> = Vec::new();
        for c in &self.cells {
            if !seen.iter().any(|(_, k)| *k == c.mp_key) {
                seen.push((c.mp_row, c.mp_key.as_str()));
            }
        }
        seen.sort();
        seen.into_iter().map(|(_, k)| k).collect()
    }
}

/// One entry of `summary.outputs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputTotal {
    /// The component id.
    pub component: String,
    /// The `a` total over the common cells.
    pub a_total: f64,
    /// The `b` total over the common cells.
    pub b_total: f64,
    /// `|b - a|`.
    pub abs: f64,
    /// `|b - a| / max(|a|,|b|)`.
    pub rel: f64,
    /// Whether the totals themselves are within tolerance.
    pub within_tolerance: bool,
}

/// `summary.emit_mismatch` (Q6): stated, banner-worthy, and not by itself `exit 2`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitMismatch {
    /// `a`'s emit setting.
    pub a: String,
    /// `b`'s emit setting.
    pub b: String,
    /// `a`'s component-set digest.
    pub a_component_set_digest: String,
    /// `b`'s component-set digest.
    pub b_component_set_digest: String,
}

/// A component present on one side only, and why the diff thinks so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OneSided {
    /// The component id, in its own side's vocabulary.
    pub component: String,
    /// `unmapped` | `no counterpart` | `not emitted by the other side`.
    pub reason: String,
}

/// `structural`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Structural {
    /// Components only in `a`.
    pub only_in_a: Vec<OneSided>,
    /// Components only in `b`.
    pub only_in_b: Vec<OneSided>,
    /// Modelpoint keys only in `a`.
    pub modelpoints_only_in_a: Vec<String>,
    /// Modelpoint keys only in `b`.
    pub modelpoints_only_in_b: Vec<String>,
}

/// `summary.cells`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CellCounts {
    /// Cells the diff actually compared.
    pub compared: u64,
    /// Cells beyond tolerance.
    pub diverged: u64,
    /// Cells a loosened override absorbed — beyond the profile, inside the override.
    pub absorbed_by_override: u64,
    /// The largest `|Δ|` seen.
    pub max_abs: f64,
    /// The largest relative difference seen.
    pub max_rel: f64,
}

/// `summary.modelpoints` / `summary.components` counts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetCounts {
    /// Distinct in `a`.
    pub a: usize,
    /// Distinct in `b`.
    pub b: usize,
    /// In both, after mapping.
    pub common: usize,
    /// Only in `a`.
    pub only_a: usize,
    /// Only in `b`.
    pub only_b: usize,
}

/// Where the diff first parts company, over the whole run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirstDivergence {
    /// The component.
    pub component: String,
    /// The earliest `t`.
    pub t: i32,
    /// The exemplar modelpoint.
    pub mp_key: String,
}
