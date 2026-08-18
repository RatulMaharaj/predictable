//! Slot allocation by shape (`03-engine.md` §3.1).
//!
//! Every value the runtime can name is a **slot**: a dense `u32`, assigned once
//! at plan time, partitioned by shape into three vectors. Inputs get slots too —
//! a modelpoint field is a `PerMP` slot with no expression, an assumption is a
//! `Scalar` (or `PerMP`) slot with no expression, and the timeline fields of
//! `01-ir.md` §5 are `Series` slots that are loop-invariant by construction.
//!
//! `SlotId` is *globally* dense rather than per-shape dense. The spec's
//! partition is preserved (the three vectors exist and are what the runtime
//! indexes), but an id identifies a slot on its own, which is what lets
//! `order_digest` hash a single sequence of ids across all four tapes without
//! ambiguity.

use predictable_ir::{DType, Expr, Kind, Stage, Timing, Unit};
use serde::{Deserialize, Serialize};

/// A dense, globally unique slot id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SlotId(pub u32);

impl SlotId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for SlotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s{}", self.0)
    }
}

/// Which of the three shape-partitioned vectors a [`SlotId`] lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    Scalar,
    PerMp,
    Series,
}

/// Where a [`SlotId`] lives: its space and its index within that space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotRef {
    pub space: Space,
    pub index: u32,
}

/// How much of a `Series` slot the runtime keeps (`03-engine.md` §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    /// Keep `len` periods in a power-of-two ring: `t & (len - 1)`, no modulo.
    Ring { len: u32 },
    /// Keep all `T + 1` periods.
    Full,
}

impl Retention {
    /// Periods retained per modelpoint, given the projection length `T`.
    pub fn periods(self, t_max: u32) -> u32 {
        match self {
            Retention::Ring { len } => len,
            Retention::Full => t_max + 1,
        }
    }

    pub fn is_full(self) -> bool {
        matches!(self, Retention::Full)
    }
}

/// Why a slot was forced to [`Retention::Full`]. Carried so the CLI can explain
/// a memory projection instead of just quoting a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullReason {
    Output,
    Reduced,
    AtTarget,
    RetainAll,
}

/// The fields every slot carries, whatever its shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub id: SlotId,
    /// Unqualified name — globally unique within a product (`01-ir.md` §2.2).
    pub name: String,
    /// Dot-separated module path; `<timeline>` for the built-in timeline fields.
    pub module_path: String,
    /// Index of the declaration within its module: the `01-ir.md` §3.2 tiebreak.
    pub decl_index: u32,
    pub kind: Kind,
    pub dtype: DType,
    pub unit: Unit,
    pub stage: Stage,
}

impl SlotInfo {
    /// `<module_path>.<name>` — the join key of `01-ir.md` §2.2.
    pub fn qualified_id(&self) -> String {
        format!("{}.{}", self.module_path, self.name)
    }

    /// The `01-ir.md` §3.2 ordering key.
    pub fn order_key(&self) -> (&str, u32) {
        (self.module_path.as_str(), self.decl_index)
    }
}

/// `Shape::Scalar` — one value for the whole run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScalarSlot {
    pub info: SlotInfo,
    pub expr: Option<Expr>,
}

/// `Shape::PerMP` — one value per modelpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerMpSlot {
    pub info: SlotInfo,
    pub expr: Option<Expr>,
}

/// `Shape::Series` — one value per `(modelpoint, t)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesSlot {
    pub info: SlotInfo,
    pub timing: Timing,
    /// Largest `k` of any `Lag(self, k)` anywhere in the model; 0 if never lagged.
    pub max_lag: u32,
    pub retention: Retention,
    /// Set when `retention` is `Full`, and why.
    pub full_reason: Option<FullReason>,
    /// The `t = 0` seed program (`01-ir.md` §2.7); may read stage-2 slots (§8.2).
    pub init: Option<Expr>,
    pub expr: Option<Expr>,
    /// True when the transitive dependency closure touches no `PerMP` slot and
    /// no modelpoint field — the slot is computed once per run (§3.4).
    pub hoistable: bool,
}

/// The timeline fields of `01-ir.md` §5, in their normative declaration order.
/// They are `Series` (they vary with `t`, not with the modelpoint), which is
/// exactly the case §3.4's hoisting exists for.
pub const TIMELINE_FIELDS: &[(&str, DType, Unit)] = &[
    ("t", DType::I64, Unit::None),
    ("period_start_date", DType::Date, Unit::None),
    ("period_end_date", DType::Date, Unit::None),
    ("year_frac", DType::F64, Unit::Years),
    ("month_of_year", DType::I64, Unit::None),
    ("policy_year", DType::I64, Unit::None),
    ("policy_month", DType::I64, Unit::None),
    ("is_anniversary", DType::Bool, Unit::None),
];

/// The module path the timeline fields are attributed to. `<` sorts below every
/// ASCII letter, so the timeline is always first in the §3.2 min-heap — which
/// is what makes `t` a stable head of the order across every model.
pub const TIMELINE_MODULE: &str = "<timeline>";
