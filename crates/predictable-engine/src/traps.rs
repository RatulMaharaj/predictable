//! Traps: per-lane flags in the hot loop, scalar replay for the report
//! (`03-engine.md` §5.5, `01-ir.md` §9.3.1 — decision Q7).
//!
//! The hot loop never returns a `Result` and never branches on data. A trapping
//! op writes a poison value and sets the lane's bit in a [`TrapFlags`] bitset;
//! the loop ORs the whole bitset **once per period** — 16 `u64` words for
//! `C = 1024` — instead of testing per op. Only when that OR is non-zero does
//! the engine pay for a diagnostic, and it pays for it by *replaying the same
//! period for the offending lanes alone*, scalar-wise, with recording on. Because
//! the kernel is deterministic the replay reproduces the trap exactly, so the
//! operand values in the report are the real ones rather than a re-derivation.

use predictable_diagnostics::{Diagnostic, Severity};
use serde::{Deserialize, Serialize};

/// The trap kinds of `01-ir.md` §9.3.1. The set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrapKind {
    DivByZero,
    LogNonPositive,
    PowNan,
    LookupMiss,
    IndexOutOfRange,
    OverflowToInf,
    NotFinite,
}

impl TrapKind {
    /// The wire name, which is also the `trap` field of the `E0902` envelope.
    pub fn name(self) -> &'static str {
        match self {
            TrapKind::DivByZero => "div_by_zero",
            TrapKind::LogNonPositive => "log_non_positive",
            TrapKind::PowNan => "pow_nan",
            TrapKind::LookupMiss => "lookup_miss",
            TrapKind::IndexOutOfRange => "index_out_of_range",
            TrapKind::OverflowToInf => "overflow_to_inf",
            TrapKind::NotFinite => "not_finite",
        }
    }
}

/// What a run does when a modelpoint traps (`01-ir.md` §9.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrapPolicy {
    /// The default: report, stop, exit 2, write no results.
    #[default]
    Abort,
    /// `--continue-on-trap`: the modelpoint contributes no rows at all and is
    /// recorded in `execution.traps[]`; the run exits 1.
    Continue,
}

/// One lane's trap: everything the report needs that the scalar replay cannot
/// re-derive later.
#[derive(Debug, Clone, PartialEq)]
pub struct TrapReport {
    pub kind: TrapKind,
    /// Qualified component name, e.g. `term.qx`.
    pub component: String,
    /// `expr.lhs` — the `ExprPath` of `01-ir.md` §3.0.1.
    pub expr_path: String,
    /// Modelpoint key as supplied by the source.
    pub mp_key: String,
    /// Row index of the modelpoint in the file.
    pub mp_row: u64,
    pub t: u32,
    pub message: String,
    /// The real operand values, captured during replay.
    pub operands: Vec<(String, f64)>,
}

impl TrapReport {
    /// The `E0902` envelope of `01-ir.md` §9.3.1, as a JSON value.
    pub fn envelope(&self) -> serde_json::Value {
        serde_json::json!({
            "code": "E0902",
            "severity": "error",
            "kind": "trap",
            "trap": self.kind.name(),
            "component": self.component,
            "expr_path": self.expr_path,
            "mp_key": self.mp_key,
            "mp_row": self.mp_row,
            "t": self.t,
            "message": self.message,
            "operands": self
                .operands
                .iter()
                .map(|(n, v)| (n.clone(), serde_json::json!(v)))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
            "doc_url": "https://predictable.dev/diagnostics/E0902",
        })
    }

    /// The same trap as a renderable diagnostic, for the terminal path.
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::new("E0902", self.message.clone())
            .severity(Severity::Error)
            .note(format!(
                "modelpoint `{}` (row {}), component `{}`, t = {}",
                self.mp_key, self.mp_row, self.component, self.t
            ))
    }
}

impl From<TrapReport> for serde_json::Value {
    fn from(r: TrapReport) -> serde_json::Value {
        r.envelope()
    }
}

/// A `C`-wide bitset of lanes. One `u64` per 64 modelpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrapFlags {
    words: Vec<u64>,
    lanes: usize,
}

impl TrapFlags {
    pub fn new(lanes: usize) -> TrapFlags {
        TrapFlags {
            words: vec![0; lanes.div_ceil(64)],
            lanes,
        }
    }

    #[inline]
    pub fn set(&mut self, lane: usize) {
        self.words[lane >> 6] |= 1u64 << (lane & 63);
    }

    #[inline]
    pub fn get(&self, lane: usize) -> bool {
        self.words[lane >> 6] >> (lane & 63) & 1 == 1
    }

    /// The once-per-period check: OR every word (`§5.5`).
    #[inline]
    pub fn any(&self) -> bool {
        self.words.iter().fold(0, |a, w| a | w) != 0
    }

    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Set lanes in ascending order — the order reports are emitted in, which is
    /// what makes a trap report set deterministic.
    pub fn lanes(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.lanes).filter(move |&c| self.get(c))
    }

    pub fn clear(&mut self) {
        for w in self.words.iter_mut() {
            *w = 0;
        }
    }

    /// `self |= other`.
    pub fn union(&mut self, other: &TrapFlags) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= b;
        }
    }

    pub fn capacity(&self) -> usize {
        self.lanes
    }
}

/// Retained trap reports, capped by `--max-errors` while the counts stay exact
/// (`01-ir.md` §9.3.1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrapLog {
    reports: Vec<TrapReport>,
    max: usize,
    total: usize,
    modelpoints: usize,
}

impl TrapLog {
    /// `max` is `--max-errors`; `0` retains nothing but still counts.
    pub fn new(max: usize) -> TrapLog {
        TrapLog {
            reports: Vec::new(),
            max,
            total: 0,
            modelpoints: 0,
        }
    }

    pub fn push(&mut self, report: TrapReport) {
        self.total += 1;
        self.modelpoints += 1;
        if self.reports.len() < self.max {
            self.reports.push(report);
        }
    }

    /// Retained reports, in `(chunk, lane, t)` order.
    pub fn reports(&self) -> &[TrapReport] {
        &self.reports
    }

    /// Exact count of traps, whether retained or not.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Exact count of modelpoints invalidated.
    pub fn modelpoints(&self) -> usize {
        self.modelpoints
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// True once retention is capped — the CLI says "showing N of M".
    pub fn truncated(&self) -> bool {
        self.total > self.reports.len()
    }
}
