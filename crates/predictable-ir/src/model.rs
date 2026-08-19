//! Model-module contents: components and their metadata, tables, the modelpoint schema,
//! assumptions, enums and the timeline (§2, §5, §11.1).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::error::IrError;
use crate::expr::{Expr, ExprPath, LitValue, PathRoot};
use crate::types::{DType, Kind, Shape, Timing, Unit};

/// The evaluation stage of a component (§8.2). Computed, never authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum Stage {
    /// Evaluated inside the `t` loop.
    One,
    /// A reduction over a completed series, evaluated after the loop.
    Two,
}

impl From<Stage> for u8 {
    fn from(s: Stage) -> u8 {
        match s {
            Stage::One => 1,
            Stage::Two => 2,
        }
    }
}

impl TryFrom<u8> for Stage {
    type Error = String;

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(Stage::One),
            2 => Ok(Stage::Two),
            other => Err(format!("stage must be 1 or 2, found {other}")),
        }
    }
}

/// A span into a `.pir` file (§11.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub file: String,
    pub byte_start: u32,
    pub byte_end: u32,
    pub line: u32,
    pub col: u32,
}

/// A span into the *authoring* language — Python, or a Prophet source line (§11.1).
///
/// Every diagnostic raised against a component must be renderable against this when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginSpan {
    pub file: String,
    pub line: u32,
    pub col_start: u32,
    pub col_end: u32,
}

/// Who wrote this component (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthoredBy {
    Dsl,
    Hand,
    Migration,
}

/// Provenance of a migrated component — the backbone of the Prophet migration report (§11.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// The library component this one overrides (§11.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideRef {
    pub library: String,
    pub version: String,
    pub component: String,
}

/// Where a generated component came from (§11.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedFrom {
    pub file: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<String>,
}

/// Rendering hints. Explicitly non-semantic and excluded from `model_digest` (§11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dp: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
}

/// `[component.meta]` (§11.1). Everything here is optional and none of it participates in
/// evaluation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Meta {
    /// An opaque stable identity that survives renames. Excluded from `model_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authored_by: Option<AuthoredBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_span: Option<OriginSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<OverrideRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_from: Option<GeneratedFrom>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Display>,
    /// The `.pir` span of the component's declaration, assigned at parse time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

impl Meta {
    pub fn is_empty(&self) -> bool {
        *self == Meta::default()
    }
}

/// A named, typed node of the computation graph (§2.1).
///
/// Field order matches the canonical key order of §4.1, so `pir.json` reads like the `.pir`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub kind: Kind,
    pub dtype: DType,
    pub shape: Shape,
    #[serde(default)]
    pub unit: Unit,
    /// Present only for `Series`. Serialised as `null` on `Scalar`/`PerMP` — never omitted,
    /// because "this value has no timing" is information (Q14).
    #[serde(default)]
    pub timing: Option<Timing>,
    /// The value at `t = 0` for a recursive series. May reference stage-2 values (§8.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init: Option<Expr>,
    /// Absent exactly for inputs; every non-input component in a well-formed module has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Meta::is_empty")]
    pub meta: Meta,
}

impl Component {
    /// A minimal `Derived` `Series` component; fields are then set directly.
    pub fn derived(name: impl Into<String>, dtype: DType, shape: Shape, expr: Expr) -> Component {
        Component {
            name: name.into(),
            kind: Kind::Derived,
            dtype,
            shape,
            unit: Unit::None,
            timing: None,
            init: None,
            expr: Some(expr),
            doc: None,
            tags: Vec::new(),
            meta: Meta::default(),
        }
    }

    /// `2` iff the component's `expr` contains an `Agg`, else `1` (§2.2). Computed, so that no
    /// consumer ever re-derives it and no author can disagree with it.
    pub fn stage(&self) -> Stage {
        match &self.expr {
            Some(e) if e.contains_agg() => Stage::Two,
            _ => Stage::One,
        }
    }

    /// The component's canonical fully-qualified id, `<module_path>.<name>` (§2.2). This is the
    /// join key in results, traces, run diffs and mapping files; no other separator is legal.
    pub fn qualified_id(&self, module_path: &str) -> String {
        format!("{module_path}.{}", self.name)
    }

    /// Every `Ref`/`Lag`/`At`/`Lookup` node in `expr` and `init`, each with its `ExprPath`.
    pub fn reference_paths(&self) -> Vec<(ExprPath, &Expr)> {
        let mut out = Vec::new();
        if let Some(e) = &self.expr {
            out.extend(e.reference_paths(PathRoot::Expr));
        }
        if let Some(i) = &self.init {
            out.extend(i.reference_paths(PathRoot::Init));
        }
        out
    }

    /// Resolve an `ExprPath` against whichever of `expr`/`init` its root names.
    pub fn resolve(&self, path: &ExprPath) -> Option<&Expr> {
        let root = match path.root_kind() {
            PathRoot::Expr => self.expr.as_ref()?,
            PathRoot::Init => self.init.as_ref()?,
        };
        root.resolve(path)
    }
}

/// Per-key lookup policy (§2.9). Enum keys support `exact` only (`E0305`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyPolicy {
    Exact,
    Clamp,
    Step,
    Interpolate,
}

/// One ordered, typed key column of a table (§2.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableKey {
    pub name: String,
    pub dtype: DType,
    pub policy: KeyPolicy,
}

/// One value column of a table (§2.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableValue {
    pub name: String,
    pub dtype: DType,
    #[serde(default)]
    pub unit: Unit,
}

/// What a lookup miss does (§2.9). Silent NaN propagation is not available at any setting.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OnMissing {
    /// The default. Traps (§9.3).
    #[default]
    Error,
    /// Substitute a literal, decided at compile time of the lookup structure.
    Default(LitValue),
    /// Linear interpolation on the named key.
    Interpolate(String),
}

impl fmt::Display for OnMissing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OnMissing::Error => f.write_str("error"),
            OnMissing::Default(v) => write!(f, "default({v})"),
            OnMissing::Interpolate(k) => write!(f, "interpolate({k})"),
        }
    }
}

impl FromStr for OnMissing {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "error" {
            return Ok(OnMissing::Error);
        }
        if let Some(inner) = s.strip_prefix("default(").and_then(|r| r.strip_suffix(')')) {
            let inner = inner.trim();
            let value = if inner == "true" || inner == "false" {
                LitValue::Bool(inner == "true")
            } else if let Ok(i) = inner.parse::<i64>() {
                LitValue::Int(i)
            } else if let Ok(x) = inner.parse::<f64>() {
                LitValue::Float(x)
            } else {
                LitValue::Text(inner.trim_matches('"').to_string())
            };
            return Ok(OnMissing::Default(value));
        }
        if let Some(inner) = s
            .strip_prefix("interpolate(")
            .and_then(|r| r.strip_suffix(')'))
        {
            let inner = inner.trim();
            if inner.is_empty() {
                return Err(IrError::OnMissing(s.to_string()));
            }
            return Ok(OnMissing::Interpolate(inner.to_string()));
        }
        Err(IrError::OnMissing(s.to_string()))
    }
}

impl Serialize for OnMissing {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for OnMissing {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// How a table's bytes are obtained (§2.9.1). The engine never opens a file itself; the scheme
/// here selects the `TableResolver` that does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableSource {
    /// A path relative to the directory of the declaring `.pir`, sandboxed to the project root.
    File(String),
    /// `resource:<name>` — supplied by the host. WASM and hosted runs use this.
    Resource(String),
    /// `inline` — rows carried in the declaration itself.
    Inline,
}

impl fmt::Display for TableSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableSource::File(p) => f.write_str(p),
            TableSource::Resource(n) => write!(f, "resource:{n}"),
            TableSource::Inline => f.write_str("inline"),
        }
    }
}

impl FromStr for TableSource {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "inline" => TableSource::Inline,
            other => match other.strip_prefix("resource:") {
                Some(name) => TableSource::Resource(name.to_string()),
                None => TableSource::File(other.strip_prefix("file:").unwrap_or(other).to_string()),
            },
        })
    }
}

impl Serialize for TableSource {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TableSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// A declared external keyed lookup (§2.9). Tables are loaded once per run and are immutable
/// during projection; the `digest`, not the path, is the table's identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableDecl {
    pub name: String,
    pub keys: Vec<TableKey>,
    pub values: Vec<TableValue>,
    #[serde(default)]
    pub on_missing: OnMissing,
    pub source: TableSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// `source = "inline"` only: rows in `keys ++ values` order, sorted ascending by key tuple.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<LitValue>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

impl TableDecl {
    /// The arity a `Lookup` on this table must supply.
    pub fn arity(&self) -> usize {
        self.keys.len()
    }

    /// True when this table carries a presence bit that `is_null` may observe (§2.11).
    pub fn has_presence_bit(&self) -> bool {
        matches!(self.on_missing, OnMissing::Default(_))
    }
}

/// One typed column of the modelpoint file (§2.10). The schema is part of the IR, never
/// inferred from the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelpointField {
    pub name: String,
    pub dtype: DType,
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub required: bool,
    /// The results join key. Exactly one field per schema carries it.
    #[serde(default)]
    pub key: bool,
    /// Required for an optional field: missingness is eliminated at load (§2.11).
    #[serde(default, rename = "default", skip_serializing_if = "Option::is_none")]
    pub default_value: Option<LitValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

impl ModelpointField {
    /// True when `is_null(field)` is legal on this field — an optional field is the only
    /// modelpoint value that carries a presence bit (§2.11).
    pub fn has_presence_bit(&self) -> bool {
        !self.required
    }
}

/// A named run input, versioned separately from the model (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssumptionDecl {
    pub name: String,
    pub dtype: DType,
    #[serde(default)]
    pub unit: Unit,
    #[serde(default = "scalar_shape")]
    pub shape: Shape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

fn scalar_shape() -> Shape {
    Shape::Scalar
}

/// An `[[enum]]` declaration (§2.9). Declaration order is presentational: it fixes the
/// dictionary encoding for results, and enums compare by equality alone, never by ordinal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumDecl {
    pub name: String,
    pub values: Vec<String>,
}

impl EnumDecl {
    /// The dictionary code of a variant, or `None` when the variant is not declared.
    pub fn code_of(&self, variant: &str) -> Option<u32> {
        self.values
            .iter()
            .position(|v| v == variant)
            .map(|i| i as u32)
    }
}

/// Projection basis (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    Monthly,
    Quarterly,
    Annual,
}

impl Basis {
    /// Periods per year — the divisor a basis conversion must use.
    pub fn periods_per_year(self) -> u32 {
        match self {
            Basis::Monthly => 12,
            Basis::Quarterly => 4,
            Basis::Annual => 1,
        }
    }
}

/// Where `t = 0` sits (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Policy,
    Valuation,
}

/// The `[timeline]` block (§5). `periods` is the **only** normative source of `T`; a run may
/// not override any field here (`E0101`, Q1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub basis: Basis,
    /// `T`. The projection runs `t = 0..=periods`.
    pub periods: u32,
    pub origin: Origin,
    /// ISO-8601 date, canonical text form.
    pub valuation_date: String,
    #[serde(default = "default_year_convention")]
    pub year_convention: String,
}

fn default_year_convention() -> String {
    "act/365".to_string()
}

/// The `Input.Timeline` components a `[timeline]` block generates. Always available, never
/// redeclared, never overridable by a run (§5).
pub const TIMELINE_FIELDS: &[&str] = &[
    "t",
    "period_start_date",
    "period_end_date",
    "year_frac",
    "month_of_year",
    "policy_year",
    "policy_month",
    "is_anniversary",
];

/// True when `name` is a generated timeline input, which may not be redeclared.
pub fn is_timeline_field(name: &str) -> bool {
    TIMELINE_FIELDS.contains(&name)
}

/// A model module: one `.pir` file's worth of declarations (§4, §6).
///
/// Array field names match the TOML array-of-table names exactly, so `pir.json` is a
/// mechanical, lossless transcription of the `.pir` rather than a re-modelling of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    /// `pir/1`.
    pub format: String,
    /// Dot-separated module path; the prefix of every `qualified_id` in this file.
    pub module: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Timeline>,
    #[serde(default, rename = "enum", skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<EnumDecl>,
    #[serde(
        default,
        rename = "modelpoint_field",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub modelpoint_fields: Vec<ModelpointField>,
    #[serde(default, rename = "assumption", skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<AssumptionDecl>,
    #[serde(default, rename = "table", skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<TableDecl>,
    #[serde(default, rename = "component", skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
}

impl Module {
    /// An empty module carrying the current format version.
    pub fn new(module: impl Into<String>) -> Module {
        Module {
            format: crate::FORMAT.to_string(),
            module: module.into(),
            imports: Vec::new(),
            timeline: None,
            enums: Vec::new(),
            modelpoint_fields: Vec::new(),
            assumptions: Vec::new(),
            tables: Vec::new(),
            components: Vec::new(),
        }
    }

    /// Look a component up by its unqualified name. Names are globally unique within a product,
    /// so this is the resolution a checker performs per module (§7 pass 2).
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }

    pub fn table(&self, name: &str) -> Option<&TableDecl> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// The `kind = "Output"` components, in declaration order. This is the set a product's
    /// `outputs` manifest must equal (`E0107`, Q2).
    pub fn outputs(&self) -> Vec<&Component> {
        self.components
            .iter()
            .filter(|c| c.kind.is_emitted())
            .collect()
    }

    /// The modelpoint field flagged `key = true`, if any.
    pub fn key_field(&self) -> Option<&ModelpointField> {
        self.modelpoint_fields.iter().find(|f| f.key)
    }
}

/// An assumption set file (§6): values only, versioned separately from the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssumptionSet {
    pub format: String,
    pub assumption_set: String,
    pub model_module: String,
    /// Name → value. Ordered so the file's canonical text and its digest are stable.
    #[serde(flatten)]
    pub values: BTreeMap<String, LitValue>,
}
