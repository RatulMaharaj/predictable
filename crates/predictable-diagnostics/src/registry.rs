//! The diagnostic code registry.
//!
//! Every code that predictable can emit is declared here, exactly once, with its
//! namespace, default severity and one-line title. Nothing else in the codebase
//! may invent a code: [`crate::Diagnostic::new`] refuses an unregistered one.
//!
//! The registry is the source of truth for the diagnostics catalogue
//! (`docs/llm/diagnostics.md`); a test in `tests/registry.rs` asserts the two
//! agree in both directions, which is what makes every `doc_url` resolvable.

use serde::{Deserialize, Serialize};

use crate::model::Severity;

/// Base URL the catalogue is published at. `doc_url` is this plus `#<code>`.
pub const DOC_BASE: &str = "https://predictable.dev/llm/diagnostics/";

/// Path of the catalogue inside the repository, relative to the repo root.
pub const DOC_PATH: &str = "docs/llm/diagnostics.md";

/// The code namespaces. The prefix is the first two characters of every code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Namespace {
    /// `E0xxx` — IR / checker errors.
    IrError,
    /// `W0xxx` — IR / checker lints.
    IrLint,
    /// `E1xxx` — Python DSL errors, raised at build time.
    DslError,
    /// `W1xxx` — Python DSL lints.
    DslLint,
    /// `P0xxx` — Prophet file reader diagnostics (`.MPF`, `.fac`, `.rpt`).
    ReaderNote,
    /// `N0xxx` — provenance notes emitted by `explain()`.
    TraceNote,
    /// `H0xxx` — run-diff hypotheses.
    Hypothesis,
}

impl Namespace {
    /// The two-character code prefix.
    pub fn prefix(self) -> &'static str {
        match self {
            Namespace::IrError => "E0",
            Namespace::IrLint => "W0",
            Namespace::DslError => "E1",
            Namespace::DslLint => "W1",
            Namespace::ReaderNote => "P0",
            Namespace::TraceNote => "N0",
            Namespace::Hypothesis => "H0",
        }
    }

    /// Human title for the catalogue section.
    pub fn title(self) -> &'static str {
        match self {
            Namespace::IrError => "IR and checker errors (E0xxx)",
            Namespace::IrLint => "IR lints (W0xxx)",
            Namespace::DslError => "Python DSL errors (E1xxx)",
            Namespace::DslLint => "Python DSL lints (W1xxx)",
            Namespace::ReaderNote => "Prophet reader diagnostics (P0xxx)",
            Namespace::TraceNote => "Trace notes (N0xxx)",
            Namespace::Hypothesis => "Run-diff hypotheses (H0xxx)",
        }
    }

    /// The namespace a code belongs to, from its first two characters.
    pub fn of_code(code: &str) -> Option<Namespace> {
        Some(match code.get(..2)? {
            "E0" => Namespace::IrError,
            "W0" => Namespace::IrLint,
            "E1" => Namespace::DslError,
            "W1" => Namespace::DslLint,
            "P0" => Namespace::ReaderNote,
            "N0" => Namespace::TraceNote,
            "H0" => Namespace::Hypothesis,
            _ => return None,
        })
    }

    /// All namespaces, in catalogue order.
    pub fn all() -> &'static [Namespace] {
        &[
            Namespace::IrError,
            Namespace::IrLint,
            Namespace::DslError,
            Namespace::DslLint,
            Namespace::ReaderNote,
            Namespace::TraceNote,
            Namespace::Hypothesis,
        ]
    }
}

/// A registered code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeInfo {
    /// The code itself, e.g. `E0201`.
    pub code: &'static str,
    /// Which namespace it belongs to (redundant with the prefix; asserted in tests).
    pub namespace: Namespace,
    /// Default severity for diagnostics carrying this code.
    pub severity: Severity,
    /// One-line title, used as the catalogue heading.
    pub title: &'static str,
    /// Where the rule is defined, e.g. `01-ir.md §3.1`.
    pub spec: &'static str,
}

impl CodeInfo {
    /// The documentation URL for this code.
    pub fn doc_url(&self) -> String {
        doc_url_for(self.code)
    }
}

const fn e(code: &'static str, title: &'static str, spec: &'static str) -> CodeInfo {
    CodeInfo {
        code,
        namespace: Namespace::IrError,
        severity: Severity::Error,
        title,
        spec,
    }
}

const fn info(
    code: &'static str,
    namespace: Namespace,
    severity: Severity,
    title: &'static str,
    spec: &'static str,
) -> CodeInfo {
    CodeInfo {
        code,
        namespace,
        severity,
        title,
        spec,
    }
}

const fn w(code: &'static str, title: &'static str, spec: &'static str) -> CodeInfo {
    info(code, Namespace::IrLint, Severity::Warning, title, spec)
}
const fn de(code: &'static str, title: &'static str, spec: &'static str) -> CodeInfo {
    info(code, Namespace::DslError, Severity::Error, title, spec)
}
const fn dw(code: &'static str, title: &'static str, spec: &'static str) -> CodeInfo {
    info(code, Namespace::DslLint, Severity::Warning, title, spec)
}
const fn p(code: &'static str, severity: Severity, title: &'static str) -> CodeInfo {
    info(
        code,
        Namespace::ReaderNote,
        severity,
        title,
        "04-verify.md §4",
    )
}
const fn n(code: &'static str, title: &'static str) -> CodeInfo {
    info(
        code,
        Namespace::TraceNote,
        Severity::Info,
        title,
        "04-verify.md §3.4",
    )
}
const fn h(code: &'static str, title: &'static str) -> CodeInfo {
    info(
        code,
        Namespace::Hypothesis,
        Severity::Info,
        title,
        "04-verify.md §5.5",
    )
}

/// Every registered code, sorted by code (asserted in tests, relied on by [`lookup`]).
pub const REGISTRY: &[CodeInfo] = &[
    // ---- E0xxx: IR and checker errors ----
    // E0000: the fallback used when a message reaches the DSL without a code.
    e("E0000", "an error arrived without a registry code", "01-ir.md §7"),
    // E00 0x/1x: the `.pir` lexer and the raw key/value grammar (§4).
    e("E0001", "unexpected character", "01-ir.md §4"),
    e("E0002", "unterminated string", "01-ir.md §4"),
    e("E0003", "expected a key", "01-ir.md §4"),
    e("E0004", "expected `=` after a key", "01-ir.md §4"),
    e("E0005", "duplicate key", "01-ir.md §4"),
    e(
        "E0006",
        "unexpected trailing input on this line",
        "01-ir.md §4",
    ),
    e("E0007", "expected a `[section]` header", "01-ir.md §4"),
    e(
        "E0008",
        "expected a name in the section header",
        "01-ir.md §4",
    ),
    e("E0009", "unclosed section header", "01-ir.md §4"),
    e("E0010", "integer literal does not fit in i64", "01-ir.md §4"),
    e("E0011", "a bare word is not a value", "01-ir.md §4"),
    e("E0012", "expected a value", "01-ir.md §4"),
    e("E0013", "unclosed array", "01-ir.md §4"),
    e("E0014", "unclosed inline table", "01-ir.md §4"),
    // E002x/E003x: the expression grammar (§4.2, §2.6-§2.7).
    e("E0020", "expected a value in an expression", "01-ir.md §4.2"),
    e(
        "E0021",
        "unexpected character in an expression",
        "01-ir.md §4.2",
    ),
    e(
        "E0022",
        "trailing input after the expression",
        "01-ir.md §4.2",
    ),
    e(
        "E0023",
        "expected `then` after the `if` condition",
        "01-ir.md §4.2",
    ),
    e(
        "E0024",
        "expected `else`: both arms of an `if` are required",
        "01-ir.md §4.2",
    ),
    e("E0025", "comparisons do not chain", "01-ir.md §4.2"),
    e("E0026", "unclosed `(`", "01-ir.md §4.2"),
    e("E0027", "unclosed argument list", "01-ir.md §4.2"),
    e(
        "E0028",
        "expected `(` after the lookup sigil",
        "01-ir.md §2.9",
    ),
    e("E0029", "`x[t-0]` is not a lag", "01-ir.md §2.7"),
    e(
        "E0030",
        "forward references are not expressible",
        "01-ir.md §2.7",
    ),
    e(
        "E0031",
        "an index must be `t`, `t-k` or a non-negative integer",
        "01-ir.md §2.7",
    ),
    e("E0032", "unclosed index", "01-ir.md §2.7"),
    // E004x/E005x: lowering a well-formed document into the IR data model (§2).
    e("E0041", "a required key is missing", "01-ir.md §2"),
    e("E0042", "a key has the wrong type", "01-ir.md §2"),
    e(
        "E0043",
        "a key's value is not one of the legal choices",
        "01-ir.md §2",
    ),
    e("E0044", "unknown key for this block", "01-ir.md §2"),
    e("E0045", "unknown section", "01-ir.md §4"),
    e(
        "E0046",
        "`[run.exec]` without a `[run]` block",
        "01-ir.md §8.4",
    ),
    e("E0047", "duplicate block", "01-ir.md §4"),
    e("E0048", "a non-Series component has no timing", "01-ir.md §2.5"),
    e("E0049", "a component has no `expr`", "01-ir.md §2.2"),
    e(
        "E0050",
        "an optional modelpoint field has no `default`",
        "01-ir.md §2.10",
    ),
    e("E0051", "an enum has no values", "01-ir.md §2.10"),
    e(
        "E0052",
        "`rows` is only legal on an inline table",
        "01-ir.md §2.9",
    ),
    e("E0053", "`periods` must not be negative", "01-ir.md §5"),
    e(
        "E0101",
        "a run config may not override a timeline field",
        "01-ir.md §8.4.2",
    ),
    e(
        "E0105",
        "module imported but not listed in `product.modules`",
        "01-ir.md §8.4.1",
    ),
    e(
        "E0106",
        "`emit_list` names a component that does not resolve",
        "01-ir.md §8.4.3",
    ),
    e(
        "E0107",
        "`product.outputs` disagrees with the `Output` components",
        "01-ir.md §8.4.1",
    ),
    e(
        "E0108",
        "`storage_precision = \"f32\"` is not supported in IR 1.0",
        "01-ir.md §8.4.5",
    ),
    e(
        "E0201",
        "cyclic dependency in the same period",
        "01-ir.md §3.1",
    ),
    e("E0202", "cyclic dependency through `init`", "01-ir.md §3.1"),
    e("E0203", "cannot find a name in this model", "01-ir.md §7"),
    e(
        "E0204",
        "the same name is defined in two modules",
        "01-ir.md §2.2",
    ),
    e(
        "E0205",
        "absolute period index beyond the projection horizon",
        "01-ir.md §2.7",
    ),
    e(
        "E0301",
        "shape narrowing without an aggregate",
        "01-ir.md §7",
    ),
    e(
        "E0305",
        "`clamp` / `step` / `interpolate` on an enum lookup key",
        "01-ir.md §2.9",
    ),
    e(
        "E0402",
        "aggregation `group_by` key is a `Series`",
        "01-ir.md §8.3",
    ),
    e(
        "E0403",
        "aggregation `group_by` key is `f64`",
        "01-ir.md §8.3",
    ),
    e(
        "E0404",
        "`over_t` given for a `PerMP` measure",
        "01-ir.md §8.3",
    ),
    e(
        "E0501",
        "incompatible units in an addition",
        "01-ir.md §2.4",
    ),
    e(
        "E0502",
        "rates on different bases cannot be added",
        "01-ir.md §2.4",
    ),
    e("E0503", "`money * money` is not a unit", "01-ir.md §2.4"),
    e("E0504", "wrong number of lookup keys", "01-ir.md §2.9"),
    e(
        "E0505",
        "lookup key dtype does not match the table",
        "01-ir.md §2.9",
    ),
    e(
        "E0506",
        "`timing` on a component that is not a `Series`",
        "01-ir.md §2.5",
    ),
    e(
        "E0507",
        "a `Series` component with no `timing`",
        "01-ir.md §2.5",
    ),
    e("E0508", "unknown timing tag", "01-ir.md §2.5"),
    e(
        "E0509",
        "a `Lag` on a dtype with no zero and no `init`",
        "01-ir.md §2.7",
    ),
    e(
        "E0602",
        "`is_null` / `coalesce` on a value that cannot be missing",
        "01-ir.md §2.11",
    ),
    e(
        "E0801",
        "table `source` escapes the project root",
        "01-ir.md §2.9.1",
    ),
    e(
        "E0901",
        "trace replay diverged from the recorded run",
        "04-verify.md §3.4",
    ),
    e(
        "E0902",
        "arithmetic trap during projection",
        "01-ir.md §9.3.1",
    ),
    e("E0903", "solve did not converge", "01-ir.md §8.4.4"),
    // ---- E1xxx: Python DSL errors ----
    de(
        "E1101",
        "missing return annotation (no dtype or unit)",
        "02-dsl.md §10",
    ),
    de("E1102", "`@series` without `timing`", "02-dsl.md §10"),
    de("E1103", "unknown parameter name", "02-dsl.md §10"),
    de(
        "E1104",
        "parameter annotation contradicts the declaration",
        "02-dsl.md §10",
    ),
    de("E1105", "ambiguous name across namespaces", "02-dsl.md §10"),
    de(
        "E1106",
        "table used but not in the signature (or vice versa)",
        "02-dsl.md §10",
    ),
    de(
        "E1201",
        "Python `if` / `and` / `or` / `not` on a traced value",
        "02-dsl.md §10",
    ),
    de("E1202", "unlagged self-reference", "02-dsl.md §10"),
    de("E1203", "non-constant lag index", "02-dsl.md §10"),
    de("E1204", "forward time reference", "02-dsl.md §10"),
    de(
        "E1205",
        "loop or comprehension over a traced value",
        "02-dsl.md §10",
    ),
    de(
        "E1206",
        "call to a non-builtin function on a traced value",
        "02-dsl.md §10",
    ),
    de(
        "E1207",
        "stage-2 value read in `expr` rather than `init`",
        "02-dsl.md §10",
    ),
    de(
        "E1301",
        "modelpoint schema has zero or multiple `key()` fields",
        "02-dsl.md §10",
    ),
    de(
        "E1302",
        "modelpoint object accessed as a value",
        "02-dsl.md §10",
    ),
    de("E1401", "redefinition of a timeline input", "02-dsl.md §10"),
    de(
        "E1402",
        "arithmetic basis conversion on a `Rate`",
        "02-dsl.md §10",
    ),
    de(
        "E1501",
        "shadowing an inherited component without `@override`",
        "02-dsl.md §10",
    ),
    de(
        "E1502",
        "`@override` changes shape, dtype, unit or timing",
        "02-dsl.md §10",
    ),
    de(
        "E1503",
        "unimplemented `@abstract` component in a product",
        "02-dsl.md §10",
    ),
    de("E1504", "override of a `@final` component", "02-dsl.md §10"),
    de(
        "E1601",
        "`product(outputs=...)` disagrees with the `output=True` components",
        "02-dsl.md §10",
    ),
    // ---- H0xxx: run-diff hypotheses ----
    h("H0101", "constant ratio — a scale factor is missing"),
    h("H0102", "constant offset — an additive term is missing"),
    h("H0103", "sign flip — sign convention mismatch"),
    h("H0201", "off-by-one in `t` — timing or `shift` mismatch"),
    h("H0202", "timing basis mismatch in an `npv`"),
    h(
        "H0301",
        "divergence begins at a table key boundary — lookup policy mismatch",
    ),
    h(
        "H0302",
        "one side is zero from `t = k` onward — indicator or term expiry off by one",
    ),
    h(
        "H0303",
        "divergence confined to modelpoints sharing a field value",
    ),
    h("H0401", "rate conversion — the Prophet `/12` idiom"),
    h(
        "H0402",
        "rounding only — raise the tolerance rather than change the model",
    ),
    h(
        "H0501",
        "NaN or Inf on one side — a trap, never a tolerance question",
    ),
    // ---- N0xxx: trace notes ----
    n("N0101", "lookup key clamped"),
    n(
        "N0102",
        "lookup key stepped (banded table, exact key absent)",
    ),
    n("N0103", "lookup key interpolated between two rows"),
    n("N0104", "lookup hit `on_missing = default(...)`"),
    n("N0201", "`retime` applied — timing cast, value unchanged"),
    n("N0202", "timing-mismatched addition inside this expression"),
    n("N0301", "`pre_origin_default` used (no `init` declared)"),
    n("N0302", "`init` used at `t = 0`"),
    n(
        "N0401",
        "value is exactly zero because a factor in the product chain is zero",
    ),
    n(
        "N0402",
        "money value magnitude outside 1e-12 .. 1e12 (probable scaling error)",
    ),
    n("N0403", "denominator within 1e-12 of zero (near-trap)"),
    n("N0501", "branch of an `If` never taken for any `t`"),
    // ---- P0xxx: Prophet reader diagnostics ----
    p(
        "P0101",
        Severity::Warning,
        "file is not valid UTF-8; decoded as Windows-1252",
    ),
    p(
        "P0102",
        Severity::Info,
        "unrecognised header key retained verbatim",
    ),
    p(
        "P0103",
        Severity::Error,
        "`OUTPUT_FORMAT` disagrees with the name line",
    ),
    p(
        "P0104",
        Severity::Error,
        "`VARIABLE_TYPES` length differs from the name line",
    ),
    p("P0105", Severity::Error, "short data row"),
    p("P0106", Severity::Error, "long data row"),
    p(
        "P0107",
        Severity::Warning,
        "`NUMLINES` disagrees with the actual row count",
    ),
    p(
        "P0108",
        Severity::Info,
        "percentage column converted by dividing by 100",
    ),
    p(
        "P0109",
        Severity::Error,
        "ambiguous date format (`DD/MM` vs `MM/DD`)",
    ),
    p(
        "P0110",
        Severity::Error,
        "no column-name line, or no data rows, in the Prophet file",
    ),
    p(
        "P0111",
        Severity::Error,
        "value does not parse as its declared Prophet type",
    ),
    p(
        "P0201",
        Severity::Error,
        "`.fac` dimension declaration is malformed",
    ),
    p("P0202", Severity::Info, "`.fac` key ordering resolved"),
    p(
        "P0203",
        Severity::Error,
        "`.fac` value count does not match the dimension extents",
    ),
    p(
        "P0204",
        Severity::Info,
        "lookup policy proposed for a `.fac` dimension",
    ),
    p("P0301", Severity::Error, "`.rpt` has no period axis"),
    p(
        "P0302",
        Severity::Error,
        "`.rpt` period base could not be aligned to the IR timeline",
    ),
    p(
        "P0303",
        Severity::Error,
        "`TIME_UNITS` does not match the IR timeline basis",
    ),
    p(
        "P0304",
        Severity::Info,
        "`.rpt` is aggregate-level; modelpoint diffing unavailable",
    ),
    p(
        "P0305",
        Severity::Error,
        "`.rpt` model point key column is ambiguous",
    ),
    // ---- W0xxx: IR lints ----
    w("W0101", "component declared but never read", "01-ir.md §7"),
    w(
        "W0102",
        "`unit = \"none\"` in money arithmetic",
        "01-ir.md §7",
    ),
    w("W0103", "timing mismatch in a sum", "01-ir.md §7"),
    w(
        "W0104",
        "`Series` constant across modelpoints and time",
        "01-ir.md §7",
    ),
    w(
        "W0105",
        "timing operation applied to an untimed value",
        "01-ir.md §2.5",
    ),
    w(
        "W0110",
        "model exceeds three stage-2 substage levels",
        "03-engine.md §5.3",
    ),
    // ---- W1xxx: Python DSL lints ----
    dw(
        "W1101",
        "component declared but never read and not `output=True`",
        "02-dsl.md §10",
    ),
    dw(
        "W1102",
        "`Num` (unitless) in money arithmetic",
        "02-dsl.md §10",
    ),
    dw("W1103", "timing mismatch in a sum", "02-dsl.md §10"),
];

/// Look a code up in the registry.
pub fn lookup(code: &str) -> Option<&'static CodeInfo> {
    REGISTRY
        .binary_search_by(|probe| probe.code.cmp(code))
        .ok()
        .map(|i| &REGISTRY[i])
}

/// Whether `code` is registered.
pub fn is_registered(code: &str) -> bool {
    lookup(code).is_some()
}

/// Every registered code in a namespace, in code order.
pub fn codes_in(namespace: Namespace) -> impl Iterator<Item = &'static CodeInfo> {
    REGISTRY.iter().filter(move |c| c.namespace == namespace)
}

/// The documentation URL for a code, whether or not it is registered.
pub fn doc_url_for(code: &str) -> String {
    format!("{DOC_BASE}#{code}")
}
