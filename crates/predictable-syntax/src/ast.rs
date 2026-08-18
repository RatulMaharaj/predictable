//! The typed shape of a `.pir` file: the data model of `01-ir.md` §2, §5 and
//! §8.4, as produced by the parser and consumed by the checker.
//!
//! Everything here is *syntactically* valid and nothing here is *semantically*
//! checked: names are not resolved, shapes are not unified, cycles are not
//! detected. That is `T06`'s work, and it runs over this.

use std::fmt;

use crate::expr::{ExprArena, ExprId};
use crate::raw::Value;
use crate::source::{Span, Spanned};

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
            pub fn parse(s: &str) -> Option<$name> {
                match s { $($text => Some($name::$variant),)+ _ => None }
            }
            /// Every legal spelling, for "did you mean" messages.
            pub const ALL: &'static [&'static str] = &[$($text),+];
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

str_enum! {
    /// `01-ir.md` §2.2. There is deliberately no `Abstract`.
    Kind {
        InputModelpoint => "Input.Modelpoint",
        InputAssumption => "Input.Assumption",
        InputTable => "Input.Table",
        InputTimeline => "Input.Timeline",
        Derived => "Derived",
        Output => "Output",
    }
}

str_enum! {
    /// `01-ir.md` §2.3.
    Shape { Scalar => "Scalar", PerMP => "PerMP", Series => "Series" }
}

str_enum! {
    /// `01-ir.md` §2.5.
    Timing { Start => "start", End => "end", Mid => "mid", Point => "point" }
}

str_enum! {
    /// Basis of a `rate` unit (`01-ir.md` §2.4).
    RateBasis { Annual => "annual", Monthly => "monthly", Period => "period" }
}

str_enum! {
    /// `01-ir.md` §5.
    TimelineBasis { Monthly => "monthly", Annual => "annual", Quarterly => "quarterly" }
}

str_enum! {
    /// `01-ir.md` §5.
    Origin { Policy => "policy", Valuation => "valuation" }
}

str_enum! {
    /// Per-key lookup policy (`01-ir.md` §2.9).
    KeyPolicy { Exact => "exact", Clamp => "clamp", Step => "step", Interpolate => "interpolate" }
}

/// `01-ir.md` §2.4. `enum(Name)` carries its enum's name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DType {
    F64,
    I64,
    Bool,
    Date,
    Str,
    Enum(String),
}

impl DType {
    pub fn parse(s: &str) -> Option<DType> {
        match s {
            "f64" => Some(DType::F64),
            "i64" => Some(DType::I64),
            "bool" => Some(DType::Bool),
            "date" => Some(DType::Date),
            "str" => Some(DType::Str),
            _ => s
                .strip_prefix("enum(")
                .and_then(|r| r.strip_suffix(')'))
                .filter(|n| !n.is_empty())
                .map(|n| DType::Enum(n.to_string())),
        }
    }
}

impl fmt::Display for DType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DType::F64 => f.write_str("f64"),
            DType::I64 => f.write_str("i64"),
            DType::Bool => f.write_str("bool"),
            DType::Date => f.write_str("date"),
            DType::Str => f.write_str("str"),
            DType::Enum(n) => write!(f, "enum({n})"),
        }
    }
}

/// `01-ir.md` §2.4. Checked, never converted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Unit {
    None,
    Money,
    Rate(RateBasis),
    Prob,
    Count,
    Years,
    Months,
    Factor,
}

impl Unit {
    pub fn parse(s: &str) -> Option<Unit> {
        match s {
            "none" => Some(Unit::None),
            "money" => Some(Unit::Money),
            "prob" => Some(Unit::Prob),
            "count" => Some(Unit::Count),
            "years" => Some(Unit::Years),
            "months" => Some(Unit::Months),
            "factor" => Some(Unit::Factor),
            _ => s
                .strip_prefix("rate(")
                .and_then(|r| r.strip_suffix(')'))
                .and_then(RateBasis::parse)
                .map(Unit::Rate),
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unit::None => f.write_str("none"),
            Unit::Money => f.write_str("money"),
            Unit::Rate(b) => write!(f, "rate({b})"),
            Unit::Prob => f.write_str("prob"),
            Unit::Count => f.write_str("count"),
            Unit::Years => f.write_str("years"),
            Unit::Months => f.write_str("months"),
            Unit::Factor => f.write_str("factor"),
        }
    }
}

/// Behaviour of a lookup miss (`01-ir.md` §2.9).
#[derive(Debug, Clone, PartialEq)]
pub enum OnMissing {
    Error,
    /// `default(<lit>)`, the literal kept as raw text.
    Default(String),
    /// `interpolate(<key>)`.
    Interpolate(String),
}

/// A `[[component]]` block (`01-ir.md` §2.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: Spanned<String>,
    pub kind: Kind,
    pub dtype: DType,
    pub shape: Shape,
    pub unit: Unit,
    /// Present only for `Series` components (§2.5); `None` otherwise.
    pub timing: Option<Timing>,
    /// The `t = 0` seed of a recursive series, already parsed.
    pub init: Option<ExprId>,
    /// The formula. Absent for `Input.*` kinds.
    pub expr: Option<ExprId>,
    pub doc: Option<String>,
    pub tags: Vec<String>,
    /// Span of the whole `[[component]]` block header, for diagnostics.
    pub span: Span,
}

impl Component {
    /// `true` iff the component is `kind = "Output"` (§2.2: the single source of
    /// truth for emission).
    pub fn is_output(&self) -> bool {
        self.kind == Kind::Output
    }

    /// Computed stage (§2.2): `2` iff the expression contains an aggregate.
    pub fn stage(&self, arena: &ExprArena) -> u8 {
        match self.expr {
            Some(e) if arena.contains_agg(e) => 2,
            _ => 1,
        }
    }
}

/// A `[[modelpoint_field]]` block (§2.10).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelpointField {
    pub name: Spanned<String>,
    pub dtype: DType,
    pub unit: Unit,
    pub required: bool,
    /// The results join key (§8.4.1 `key_field`).
    pub key: bool,
    /// Required for optional fields (§2.11).
    pub default: Option<Value>,
    pub doc: Option<String>,
    pub span: Span,
}

/// An `[[assumption]]` declaration in a model module.
#[derive(Debug, Clone, PartialEq)]
pub struct AssumptionDecl {
    pub name: Spanned<String>,
    pub dtype: DType,
    pub unit: Unit,
    pub shape: Shape,
    pub doc: Option<String>,
    pub span: Span,
}

/// One key column of a table declaration (§2.9).
#[derive(Debug, Clone, PartialEq)]
pub struct TableKey {
    pub name: String,
    pub dtype: DType,
    pub policy: KeyPolicy,
    pub span: Span,
}

/// One value column of a table declaration (§2.9).
#[derive(Debug, Clone, PartialEq)]
pub struct TableValue {
    pub name: String,
    pub dtype: DType,
    pub unit: Unit,
    pub span: Span,
}

/// A `[[table]]` declaration (§2.9).
#[derive(Debug, Clone, PartialEq)]
pub struct TableDecl {
    pub name: Spanned<String>,
    pub keys: Vec<TableKey>,
    pub values: Vec<TableValue>,
    pub on_missing: OnMissing,
    /// Path or URI; resolution is `TableResolver`'s job (§2.9.1).
    pub source: String,
    pub digest: Option<String>,
    /// Present for `source = "inline"`; rows in `keys ++ values` order.
    pub rows: Vec<Vec<Value>>,
    pub span: Span,
}

/// An `[[enum]]` declaration. Order is presentational (§2.9).
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Spanned<String>,
    pub values: Vec<String>,
    pub span: Span,
}

/// The `[timeline]` block (§5).
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub basis: TimelineBasis,
    /// `T`; the projection runs `t = 0..=periods`.
    pub periods: i64,
    pub origin: Origin,
    /// Kept as written text.
    pub valuation_date: Option<String>,
    pub year_convention: Option<String>,
    pub span: Span,
}

/// The `[product]` block (§8.4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub name: String,
    pub modules: Vec<String>,
    /// The validated output manifest, not a selector (§2.2).
    pub outputs: Vec<String>,
    pub key_field: Option<String>,
    pub assumptions: Option<String>,
    pub doc: Option<String>,
    pub span: Span,
}

/// `[run.exec]` — non-semantic, excluded from `run_digest` (§8.4.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunExec {
    pub threads: Option<i64>,
    pub chunk_size: Option<i64>,
    pub progress: Option<bool>,
}

/// The `[run]` block (§8.4.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub product: Option<String>,
    pub assumptions: Option<String>,
    pub modelpoints: Option<String>,
    pub out: Option<String>,
    pub emit: Option<String>,
    pub emit_list: Vec<String>,
    pub retain: Option<String>,
    pub storage_precision: Option<String>,
    pub on_trap: Option<String>,
    pub max_errors: Option<i64>,
    pub allow_table_drift: Option<bool>,
    pub sum_kahan: Option<bool>,
    /// `[run.tables]` per-table source overrides; digests may never be overridden.
    pub tables: Vec<(String, String)>,
    pub exec: RunExec,
    pub span: Span,
}

/// A `[[solve]]` block (§8.4.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Solve {
    pub name: String,
    pub target: String,
    pub to: f64,
    pub vary: String,
    pub scope: Option<String>,
    pub tolerance: Option<f64>,
    pub max_iter: Option<i64>,
    pub method: Option<String>,
    pub bracket: Option<(f64, f64)>,
    pub span: Span,
}

/// An `[[aggregation]]` block (§8.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregation {
    pub name: String,
    pub group_by: Vec<String>,
    pub measure: String,
    pub op: String,
    pub weight: Option<String>,
    pub filter: Option<String>,
    pub over_t: Option<String>,
    pub span: Span,
}

/// What kind of `.pir` file this is. A file declares itself by its content;
/// nothing depends on the file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    /// A model module: components, schema, tables, timeline.
    Module,
    /// An assumption set (`assumption_set = "..."`).
    AssumptionSet,
    /// A product entry point (`[product]`).
    Product,
    /// A run configuration (`[run]`).
    Run,
    /// Nothing identifiable — an empty or badly broken file.
    Unknown,
}

/// A parsed `.pir` file.
#[derive(Debug, Clone, Default)]
pub struct PirDocument {
    pub format: Option<String>,
    pub module: Option<Spanned<String>>,
    pub imports: Vec<String>,
    /// `assumption_set = "base"` in an assumption-set file.
    pub assumption_set: Option<String>,
    pub model_module: Option<String>,
    pub timeline: Option<Timeline>,
    pub enums: Vec<EnumDecl>,
    pub modelpoint_fields: Vec<ModelpointField>,
    pub assumptions: Vec<AssumptionDecl>,
    pub tables: Vec<TableDecl>,
    pub components: Vec<Component>,
    pub product: Option<Product>,
    pub run: Option<Run>,
    pub solves: Vec<Solve>,
    pub aggregations: Vec<Aggregation>,
    /// Root-level values of an assumption set file, in declaration order.
    pub assumption_values: Vec<(Spanned<String>, Value)>,
    /// Every expression in the file, `expr` and `init` alike.
    pub arena: ExprArena,
}

impl PirDocument {
    pub fn kind(&self) -> DocumentKind {
        if self.run.is_some() {
            DocumentKind::Run
        } else if self.product.is_some() {
            DocumentKind::Product
        } else if self.assumption_set.is_some() {
            DocumentKind::AssumptionSet
        } else if self.module.is_some() {
            DocumentKind::Module
        } else {
            DocumentKind::Unknown
        }
    }

    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name.value == name)
    }

    /// Components with `kind = "Output"`, in declaration order — the emission
    /// set the product manifest must equal (§2.2).
    pub fn outputs(&self) -> impl Iterator<Item = &Component> {
        self.components.iter().filter(|c| c.is_output())
    }

    pub fn table(&self, name: &str) -> Option<&TableDecl> {
        self.tables.iter().find(|t| t.name.value == name)
    }
}
