//! Lowering: raw spanned TOML (`crate::raw`) → the typed document (`crate::ast`).
//!
//! This is where a `.pir` file stops being key/value pairs and becomes
//! components, tables and a timeline — and where the formula strings are handed
//! to the expression parser. Lowering never stops at the first problem: an
//! unusable block is reported and skipped, and the rest of the file is lowered,
//! so one `predictable check` shows every syntax error in the file.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::expr::{parse_expr, ExprArena, ExprId};
use crate::raw::{RawDocument, Section, Table, Value};
use crate::source::{Span, Spanned};

/// Lower a raw document. Always returns a document; broken blocks are omitted.
pub fn lower(raw: &RawDocument, diags: &mut Diagnostics) -> PirDocument {
    let mut doc = PirDocument::default();
    let root = &raw.root;

    doc.format = str_field(root, "format", diags);
    doc.module = root.get("module").and_then(|v| match &v.value {
        Value::Str(s) => Some(Spanned::new(s.clone(), v.span)),
        _ => {
            type_error(v.span, "module", "a string", &v.value, diags);
            None
        }
    });
    doc.imports = str_array(root, "imports", diags);
    doc.assumption_set = str_field(root, "assumption_set", diags);
    doc.model_module = str_field(root, "model_module", diags);

    // Anything else at the root of an assumption-set file is an assumption value.
    const ROOT_META: &[&str] = &[
        "format",
        "module",
        "imports",
        "assumption_set",
        "model_module",
    ];
    for (k, v) in &root.entries {
        if !ROOT_META.contains(&k.value.as_str()) {
            doc.assumption_values.push((k.clone(), v.value.clone()));
        }
    }

    for section in &raw.sections {
        match (section.dotted().as_str(), section.array) {
            ("component", true) => {
                if let Some(c) = lower_component(section, &mut doc.arena, diags) {
                    doc.components.push(c);
                }
            }
            ("modelpoint_field", true) => {
                if let Some(f) = lower_modelpoint_field(section, diags) {
                    doc.modelpoint_fields.push(f);
                }
            }
            ("assumption", true) => {
                if let Some(a) = lower_assumption(section, diags) {
                    doc.assumptions.push(a);
                }
            }
            ("table", true) => {
                if let Some(t) = lower_table(section, diags) {
                    doc.tables.push(t);
                }
            }
            ("enum", true) => {
                if let Some(e) = lower_enum(section, diags) {
                    doc.enums.push(e);
                }
            }
            ("solve", true) => {
                if let Some(s) = lower_solve(section, diags) {
                    doc.solves.push(s);
                }
            }
            ("aggregation", true) => {
                if let Some(a) = lower_aggregation(section, diags) {
                    doc.aggregations.push(a);
                }
            }
            ("timeline", false) => {
                if doc.timeline.is_some() {
                    duplicate_section(section, diags);
                } else {
                    doc.timeline = lower_timeline(section, diags);
                }
            }
            ("product", false) => {
                if doc.product.is_some() {
                    duplicate_section(section, diags);
                } else {
                    doc.product = lower_product(section, diags);
                }
            }
            ("run", false) => {
                if doc.run.is_some() {
                    duplicate_section(section, diags);
                } else {
                    doc.run = lower_run(section, diags);
                }
            }
            ("run.exec", false) => {
                let exec = RunExec {
                    threads: int_field(&section.table, "threads", diags),
                    chunk_size: int_field(&section.table, "chunk_size", diags),
                    progress: bool_field(&section.table, "progress", diags),
                };
                unknown_keys(
                    &section.table,
                    &["threads", "chunk_size", "progress"],
                    diags,
                );
                match doc.run.as_mut() {
                    Some(run) => run.exec = exec,
                    None => diags.push(
                        Diagnostic::error("E0046", "`[run.exec]` without a `[run]` block")
                            .with_primary(section.header_span, "there is no run to configure"),
                    ),
                }
            }
            ("run.tables", false) => {
                let overrides: Vec<(String, String)> = section
                    .table
                    .entries
                    .iter()
                    .filter_map(|(k, v)| match &v.value {
                        Value::Str(s) => Some((k.value.clone(), s.clone())),
                        other => {
                            type_error(v.span, &k.value, "a string source or URI", other, diags);
                            None
                        }
                    })
                    .collect();
                match doc.run.as_mut() {
                    Some(run) => run.tables = overrides,
                    None => diags.push(
                        Diagnostic::error("E0046", "`[run.tables]` without a `[run]` block")
                            .with_primary(section.header_span, "there is no run to configure"),
                    ),
                }
            }
            _ if section.path.len() == 2
                && section.path[0] == "run"
                && section.path[1] == "timeline" =>
            {
                // §8.4.2: `[timeline].periods` is the sole source of T; a run
                // that redeclares the timeline is changing the model. One
                // diagnostic per key, so each has its own fix.
                for (key, _) in &section.table.entries {
                    diags.push(
                        Diagnostic::error(
                            "E0101",
                            "the timeline is a property of the model, not of the run",
                        )
                        .with_primary(
                            key.span,
                            format!("`{}` belongs to the model's `[timeline]` block", key.value),
                        )
                        .with_secondary(
                            section.header_span,
                            "changing the projection length is a model change and must appear in `model_digest`",
                        )
                        .with_suggestion(key.span, "", "move this key to the model's `[timeline]` block"),
                    );
                }
                if section.table.entries.is_empty() {
                    diags.push(
                        Diagnostic::error(
                            "E0101",
                            "the timeline is a property of the model, not of the run",
                        )
                        .with_primary(section.header_span, "a run may not declare a timeline")
                        .with_suggestion(
                            section.header_span,
                            "",
                            "remove this section",
                        ),
                    );
                }
            }
            _ => {
                let name = section.dotted();
                diags.push(
                    Diagnostic::error("E0045", format!("unknown section `{name}`"))
                        .with_primary(section.header_span, "not a `.pir` section")
                        .with_secondary(
                            section.header_span,
                            "expected one of: timeline, enum, modelpoint_field, assumption, table, component, product, run, solve, aggregation",
                        ),
                );
            }
        }
    }
    doc
}

fn duplicate_section(section: &Section, diags: &mut Diagnostics) {
    let name = section.dotted();
    diags.push(
        Diagnostic::error("E0047", format!("duplicate `[{name}]` block"))
            .with_primary(section.header_span, "a file may declare this at most once"),
    );
}

// ---------------------------------------------------------------------------
// field accessors
// ---------------------------------------------------------------------------

fn type_error(span: Span, key: &str, expected: &str, found: &Value, diags: &mut Diagnostics) {
    if matches!(found, Value::Error) {
        return; // already reported by the raw parser
    }
    let found = found.type_name();
    diags.push(
        Diagnostic::error(
            "E0042",
            format!("`{key}` must be {expected}, found {found}"),
        )
        .with_primary(span, format!("expected {expected}")),
    );
}

fn missing(span: Span, key: &str, block: &str, diags: &mut Diagnostics) {
    diags.push(
        Diagnostic::error(
            "E0041",
            format!("`{block}` is missing the required key `{key}`"),
        )
        .with_primary(span, format!("`{key}` is required here")),
    );
}

fn str_field(t: &Table, key: &str, diags: &mut Diagnostics) -> Option<String> {
    let v = t.get(key)?;
    match &v.value {
        Value::Str(s) => Some(s.clone()),
        other => {
            type_error(v.span, key, "a string", other, diags);
            None
        }
    }
}

fn spanned_str_field(t: &Table, key: &str, diags: &mut Diagnostics) -> Option<Spanned<String>> {
    let v = t.get(key)?;
    match &v.value {
        Value::Str(s) => Some(Spanned::new(s.clone(), v.span)),
        other => {
            type_error(v.span, key, "a string", other, diags);
            None
        }
    }
}

fn int_field(t: &Table, key: &str, diags: &mut Diagnostics) -> Option<i64> {
    let v = t.get(key)?;
    match &v.value {
        Value::Int(i) => Some(*i),
        other => {
            type_error(v.span, key, "an integer", other, diags);
            None
        }
    }
}

fn float_field(t: &Table, key: &str, diags: &mut Diagnostics) -> Option<f64> {
    let v = t.get(key)?;
    match &v.value {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        other => {
            type_error(v.span, key, "a number", other, diags);
            None
        }
    }
}

fn bool_field(t: &Table, key: &str, diags: &mut Diagnostics) -> Option<bool> {
    let v = t.get(key)?;
    match &v.value {
        Value::Bool(b) => Some(*b),
        other => {
            type_error(v.span, key, "a boolean", other, diags);
            None
        }
    }
}

fn str_array(t: &Table, key: &str, diags: &mut Diagnostics) -> Vec<String> {
    let Some(v) = t.get(key) else {
        return Vec::new();
    };
    match &v.value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match &item.value {
                Value::Str(s) => Some(s.clone()),
                other => {
                    type_error(item.span, key, "an array of strings", other, diags);
                    None
                }
            })
            .collect(),
        other => {
            type_error(v.span, key, "an array of strings", other, diags);
            Vec::new()
        }
    }
}

/// Report keys that are not part of a block's schema. This is what turns a typo
/// in `timming = "start"` into an error instead of a silently ignored line.
fn unknown_keys(t: &Table, allowed: &[&str], diags: &mut Diagnostics) {
    for (k, _) in &t.entries {
        if allowed.contains(&k.value.as_str()) {
            continue;
        }
        let mut d = Diagnostic::error("E0044", format!("unknown key `{}`", k.value))
            .with_primary(k.span, "not a key of this block");
        if let Some(near) = nearest(&k.value, allowed) {
            d = d.with_suggestion(k.span, near.to_string(), format!("did you mean `{near}`?"));
        }
        diags.push(d);
    }
}

/// A closest legal spelling within edit distance 2, for a suggested edit.
pub(crate) fn nearest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|c| (levenshtein(word, c), *c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Parse a string-typed enumeration field (`kind`, `shape`, `timing`, ...).
fn enum_field<T>(
    t: &Table,
    key: &str,
    parse: impl Fn(&str) -> Option<T>,
    all: &[&str],
    diags: &mut Diagnostics,
) -> Option<T> {
    let v = t.get(key)?;
    let Value::Str(s) = &v.value else {
        type_error(v.span, key, "a string", &v.value, diags);
        return None;
    };
    match parse(s) {
        Some(x) => Some(x),
        None => {
            let mut d = Diagnostic::error("E0043", format!("`{s}` is not a legal `{key}`"))
                .with_primary(v.span, format!("expected one of: {}", all.join(", ")));
            if let Some(near) = nearest(s, all) {
                d = d.with_suggestion(
                    v.span,
                    format!("\"{near}\""),
                    format!("did you mean `{near}`?"),
                );
            }
            diags.push(d);
            None
        }
    }
}

/// Parse an expression held in a string field.
///
/// The span base is the value span plus one byte, so every node inside the
/// expression points at the right column of the original `.pir` line.
fn expr_field(
    t: &Table,
    key: &str,
    arena: &mut ExprArena,
    diags: &mut Diagnostics,
) -> Option<ExprId> {
    let v = t.get(key)?;
    let Value::Str(s) = &v.value else {
        type_error(v.span, key, "a string holding a formula", &v.value, diags);
        return None;
    };
    Some(parse_expr(s, v.span.file, v.span.start + 1, arena, diags))
}

// ---------------------------------------------------------------------------
// blocks
// ---------------------------------------------------------------------------

const COMPONENT_KEYS: &[&str] = &[
    "name", "kind", "dtype", "shape", "unit", "timing", "init", "expr", "doc", "tags", "output",
];

fn lower_component(
    section: &Section,
    arena: &mut ExprArena,
    diags: &mut Diagnostics,
) -> Option<Component> {
    let t = &section.table;
    unknown_keys(t, COMPONENT_KEYS, diags);
    let name = match spanned_str_field(t, "name", diags) {
        Some(n) => n,
        None => {
            if t.get("name").is_none() {
                missing(section.header_span, "name", "[[component]]", diags);
            }
            return None;
        }
    };
    let kind = enum_field(t, "kind", Kind::parse, Kind::ALL, diags).unwrap_or_else(|| {
        if t.get("kind").is_none() {
            missing(name.span, "kind", "[[component]]", diags);
        }
        Kind::Derived
    });
    let dtype = enum_field(
        t,
        "dtype",
        DType::parse,
        &["f64", "i64", "bool", "date", "str", "enum(<Name>)"],
        diags,
    )
    .unwrap_or_else(|| {
        if t.get("dtype").is_none() {
            missing(name.span, "dtype", "[[component]]", diags);
        }
        DType::F64
    });
    let shape = enum_field(t, "shape", Shape::parse, Shape::ALL, diags).unwrap_or_else(|| {
        if t.get("shape").is_none() {
            missing(name.span, "shape", "[[component]]", diags);
        }
        Shape::Series
    });
    let unit = enum_field(
        t,
        "unit",
        Unit::parse,
        &[
            "none",
            "money",
            "rate(annual|monthly|period)",
            "prob",
            "count",
            "years",
            "months",
            "factor",
        ],
        diags,
    )
    .unwrap_or(Unit::None);
    let timing = enum_field(t, "timing", Timing::parse, Timing::ALL, diags);

    // §2.5: timing belongs to `Series` and only to `Series`.
    if shape == Shape::Series && timing.is_none() && t.get("timing").is_none() {
        missing(name.span, "timing", "a Series [[component]]", diags);
    }
    if shape != Shape::Series {
        if let Some(span) = t.key_span("timing") {
            diags.push(
                Diagnostic::error("E0048", format!("a {shape} component has no timing"))
                    .with_primary(span, "timing is a property of Series values only")
                    .with_suggestion(span, "", "remove this key"),
            );
        }
    }

    let init = expr_field(t, "init", arena, diags);
    let expr = expr_field(t, "expr", arena, diags);
    if expr.is_none()
        && t.get("expr").is_none()
        && !matches!(
            kind,
            Kind::InputModelpoint | Kind::InputAssumption | Kind::InputTable | Kind::InputTimeline
        )
    {
        // §2.2: a well-formed `.pir` never contains a hole.
        diags.push(
            Diagnostic::error("E0049", format!("`{}` has no `expr`", name.value))
                .with_primary(
                    name.span,
                    format!("a `{kind}` component must define a formula"),
                )
                .with_secondary(
                    section.header_span,
                    "declared-but-undefined is a DSL concept; it never reaches a .pir file",
                ),
        );
    }

    Some(Component {
        name,
        kind,
        dtype,
        shape,
        unit,
        timing: if shape == Shape::Series { timing } else { None },
        init,
        expr,
        doc: str_field(t, "doc", diags),
        tags: str_array(t, "tags", diags),
        span: section.header_span,
    })
}

fn lower_modelpoint_field(section: &Section, diags: &mut Diagnostics) -> Option<ModelpointField> {
    let t = &section.table;
    unknown_keys(
        t,
        &["name", "dtype", "unit", "required", "key", "default", "doc"],
        diags,
    );
    let name = spanned_str_field(t, "name", diags).or_else(|| {
        missing(section.header_span, "name", "[[modelpoint_field]]", diags);
        None
    })?;
    let dtype = enum_field(
        t,
        "dtype",
        DType::parse,
        &["f64", "i64", "bool", "date", "str", "enum(<Name>)"],
        diags,
    )
    .unwrap_or_else(|| {
        missing(name.span, "dtype", "[[modelpoint_field]]", diags);
        DType::F64
    });
    let unit = enum_field(
        t,
        "unit",
        Unit::parse,
        &[
            "none", "money", "prob", "count", "years", "months", "factor",
        ],
        diags,
    )
    .unwrap_or(Unit::None);
    let required = bool_field(t, "required", diags).unwrap_or(false);
    let default = t.get("default").map(|v| v.value.clone());
    if !required && default.is_none() {
        // §2.11: missingness is eliminated at the boundary, so an optional field
        // without a default would be the one place a null could enter.
        diags.push(
            Diagnostic::error(
                "E0050",
                format!(
                    "optional modelpoint field `{}` has no `default`",
                    name.value
                ),
            )
            .with_primary(
                name.span,
                "an optional field must declare what a missing cell means",
            )
            .with_suggestion(
                name.span,
                format!("{}\"; default = ", name.value),
                "add a default",
            ),
        );
    }
    Some(ModelpointField {
        name,
        dtype,
        unit,
        required,
        key: bool_field(t, "key", diags).unwrap_or(false),
        default,
        doc: str_field(t, "doc", diags),
        span: section.header_span,
    })
}

fn lower_assumption(section: &Section, diags: &mut Diagnostics) -> Option<AssumptionDecl> {
    let t = &section.table;
    unknown_keys(t, &["name", "dtype", "unit", "shape", "doc"], diags);
    let name = spanned_str_field(t, "name", diags).or_else(|| {
        missing(section.header_span, "name", "[[assumption]]", diags);
        None
    })?;
    let dtype = enum_field(
        t,
        "dtype",
        DType::parse,
        &["f64", "i64", "bool", "date", "str", "enum(<Name>)"],
        diags,
    )
    .unwrap_or(DType::F64);
    let unit = enum_field(
        t,
        "unit",
        Unit::parse,
        &[
            "none",
            "money",
            "rate(annual|monthly|period)",
            "prob",
            "count",
            "years",
            "months",
            "factor",
        ],
        diags,
    )
    .unwrap_or(Unit::None);
    let shape = enum_field(t, "shape", Shape::parse, Shape::ALL, diags).unwrap_or(Shape::Scalar);
    Some(AssumptionDecl {
        name,
        dtype,
        unit,
        shape,
        doc: str_field(t, "doc", diags),
        span: section.header_span,
    })
}

fn lower_enum(section: &Section, diags: &mut Diagnostics) -> Option<EnumDecl> {
    let t = &section.table;
    unknown_keys(t, &["name", "values"], diags);
    let name = spanned_str_field(t, "name", diags).or_else(|| {
        missing(section.header_span, "name", "[[enum]]", diags);
        None
    })?;
    let values = str_array(t, "values", diags);
    if values.is_empty() {
        diags.push(
            Diagnostic::error("E0051", format!("enum `{}` has no values", name.value))
                .with_primary(name.span, "an enum must list at least one value"),
        );
    }
    Some(EnumDecl {
        name,
        values,
        span: section.header_span,
    })
}

fn lower_table(section: &Section, diags: &mut Diagnostics) -> Option<TableDecl> {
    let t = &section.table;
    unknown_keys(
        t,
        &[
            "name",
            "keys",
            "values",
            "on_missing",
            "source",
            "digest",
            "rows",
        ],
        diags,
    );
    let name = spanned_str_field(t, "name", diags).or_else(|| {
        missing(section.header_span, "name", "[[table]]", diags);
        None
    })?;

    let mut keys = Vec::new();
    match t.get("keys") {
        Some(Spanned {
            value: Value::Array(items),
            ..
        }) => {
            for item in items {
                let Value::Table(kt) = &item.value else {
                    type_error(
                        item.span,
                        "keys",
                        "an array of inline tables",
                        &item.value,
                        diags,
                    );
                    continue;
                };
                unknown_keys(kt, &["name", "dtype", "policy"], diags);
                let Some(kn) = str_field(kt, "name", diags) else {
                    missing(item.span, "name", "a table key", diags);
                    continue;
                };
                let dtype = enum_field(
                    kt,
                    "dtype",
                    DType::parse,
                    &["f64", "i64", "bool", "date", "str", "enum(<Name>)"],
                    diags,
                )
                .unwrap_or(DType::I64);
                let policy = enum_field(kt, "policy", KeyPolicy::parse, KeyPolicy::ALL, diags)
                    .unwrap_or(KeyPolicy::Exact);
                // §2.9: enums are unordered, so only `exact` is meaningful.
                if matches!(dtype, DType::Enum(_)) && policy != KeyPolicy::Exact {
                    diags.push(
                        Diagnostic::error(
                            "E0305",
                            format!("enum keys support `policy = \"exact\"` only, and `{kn}` uses `{policy}`"),
                        )
                        .with_primary(
                            kt.get("policy").map(|v| inner_span(v.span)).unwrap_or(item.span),
                            "enums are unordered; only `exact` compares them",
                        )
                        .with_suggestion(
                            kt.get("policy").map(|v| inner_span(v.span)).unwrap_or(item.span),
                            "exact",
                            "use `policy = \"exact\"`",
                        ),
                    );
                }
                keys.push(TableKey {
                    name: kn,
                    dtype,
                    policy,
                    span: item.span,
                });
            }
        }
        Some(v) => type_error(v.span, "keys", "an array of inline tables", &v.value, diags),
        None => missing(name.span, "keys", "[[table]]", diags),
    }

    let mut values = Vec::new();
    match t.get("values") {
        Some(Spanned {
            value: Value::Array(items),
            ..
        }) => {
            for item in items {
                let Value::Table(vt) = &item.value else {
                    type_error(
                        item.span,
                        "values",
                        "an array of inline tables",
                        &item.value,
                        diags,
                    );
                    continue;
                };
                unknown_keys(vt, &["name", "dtype", "unit"], diags);
                let Some(vn) = str_field(vt, "name", diags) else {
                    missing(item.span, "name", "a table value column", diags);
                    continue;
                };
                let dtype = enum_field(
                    vt,
                    "dtype",
                    DType::parse,
                    &["f64", "i64", "bool", "date", "str", "enum(<Name>)"],
                    diags,
                )
                .unwrap_or(DType::F64);
                let unit = enum_field(
                    vt,
                    "unit",
                    Unit::parse,
                    &[
                        "none", "money", "prob", "count", "years", "months", "factor",
                    ],
                    diags,
                )
                .unwrap_or(Unit::None);
                values.push(TableValue {
                    name: vn,
                    dtype,
                    unit,
                    span: item.span,
                });
            }
        }
        Some(v) => type_error(
            v.span,
            "values",
            "an array of inline tables",
            &v.value,
            diags,
        ),
        None => missing(name.span, "values", "[[table]]", diags),
    }

    let on_missing = match t.get("on_missing") {
        None => OnMissing::Error,
        Some(v) => {
            match &v.value {
                Value::Str(s) if s == "error" => OnMissing::Error,
                Value::Str(s) if s.starts_with("default(") && s.ends_with(')') => {
                    OnMissing::Default(s["default(".len()..s.len() - 1].to_string())
                }
                Value::Str(s) if s.starts_with("interpolate(") && s.ends_with(')') => {
                    OnMissing::Interpolate(s["interpolate(".len()..s.len() - 1].to_string())
                }
                other => {
                    diags.push(
                    Diagnostic::error("E0043", "`on_missing` must be `error`, `default(<lit>)` or `interpolate(<key>)`")
                        .with_primary(v.span, format!("found {}", other.type_name())),
                );
                    OnMissing::Error
                }
            }
        }
    };

    let source = str_field(t, "source", diags).unwrap_or_else(|| {
        missing(name.span, "source", "[[table]]", diags);
        String::new()
    });

    let mut rows = Vec::new();
    if let Some(v) = t.get("rows") {
        match &v.value {
            Value::Array(items) => {
                for item in items {
                    match &item.value {
                        Value::Array(cells) => {
                            rows.push(cells.iter().map(|c| c.value.clone()).collect())
                        }
                        other => type_error(item.span, "rows", "an array of arrays", other, diags),
                    }
                }
                if source != "inline" {
                    diags.push(
                        Diagnostic::error("E0052", "`rows` is only legal on an inline table")
                            .with_primary(v.span, "these rows have no `source = \"inline\"`")
                            .with_suggestion(
                                v.span.at_start(),
                                "",
                                "set `source = \"inline\"` or delete the rows",
                            ),
                    );
                }
            }
            other => type_error(v.span, "rows", "an array of arrays", other, diags),
        }
    } else if source == "inline" {
        missing(name.span, "rows", "an inline [[table]]", diags);
    }

    Some(TableDecl {
        name,
        keys,
        values,
        on_missing,
        source,
        digest: str_field(t, "digest", diags),
        rows,
        span: section.header_span,
    })
}

fn lower_timeline(section: &Section, diags: &mut Diagnostics) -> Option<Timeline> {
    let t = &section.table;
    unknown_keys(
        t,
        &[
            "basis",
            "periods",
            "origin",
            "valuation_date",
            "year_convention",
        ],
        diags,
    );
    let basis = enum_field(t, "basis", TimelineBasis::parse, TimelineBasis::ALL, diags)
        .unwrap_or_else(|| {
            missing(section.header_span, "basis", "[timeline]", diags);
            TimelineBasis::Annual
        });
    let periods = int_field(t, "periods", diags).unwrap_or_else(|| {
        missing(section.header_span, "periods", "[timeline]", diags);
        0
    });
    if periods < 0 {
        if let Some(v) = t.get("periods") {
            diags.push(
                Diagnostic::error("E0053", "`periods` must not be negative")
                    .with_primary(v.span, "the projection runs t = 0..=periods"),
            );
        }
    }
    let origin =
        enum_field(t, "origin", Origin::parse, Origin::ALL, diags).unwrap_or(Origin::Policy);
    let valuation_date = t.get("valuation_date").and_then(|v| match &v.value {
        Value::Date(d) => Some(d.clone()),
        Value::Str(s) => Some(s.clone()),
        other => {
            type_error(v.span, "valuation_date", "a date", other, diags);
            None
        }
    });
    Some(Timeline {
        basis,
        periods,
        origin,
        valuation_date,
        year_convention: str_field(t, "year_convention", diags),
        span: section.header_span,
    })
}

fn lower_product(section: &Section, diags: &mut Diagnostics) -> Option<Product> {
    let t = &section.table;
    unknown_keys(
        t,
        &[
            "name",
            "modules",
            "outputs",
            "key_field",
            "assumptions",
            "doc",
        ],
        diags,
    );
    let name = str_field(t, "name", diags).unwrap_or_else(|| {
        missing(section.header_span, "name", "[product]", diags);
        String::new()
    });
    let modules = str_array(t, "modules", diags);
    if modules.is_empty() {
        missing(section.header_span, "modules", "[product]", diags);
    }
    Some(Product {
        name,
        modules,
        outputs: str_array(t, "outputs", diags),
        key_field: str_field(t, "key_field", diags),
        assumptions: str_field(t, "assumptions", diags),
        doc: str_field(t, "doc", diags),
        span: section.header_span,
    })
}

const RUN_KEYS: &[&str] = &[
    "product",
    "assumptions",
    "modelpoints",
    "out",
    "emit",
    "emit_list",
    "retain",
    "storage_precision",
    "on_trap",
    "max_errors",
    "allow_table_drift",
    "sum_kahan",
];

fn lower_run(section: &Section, diags: &mut Diagnostics) -> Option<Run> {
    let t = &section.table;
    unknown_keys(t, RUN_KEYS, diags);
    // §8.4.2: the timeline is a property of the model, never of the run.
    for key in [
        "periods",
        "basis",
        "origin",
        "valuation_date",
        "year_convention",
    ] {
        if let Some(span) = t.key_span(key) {
            diags.push(
                Diagnostic::error(
                    "E0101",
                    format!("`{key}` is a timeline field and cannot be set by a run"),
                )
                .with_primary(
                    span,
                    "the timeline is a property of the model, not of the run",
                )
                .with_suggestion(
                    span,
                    "",
                    "move it to the model's `[timeline]` block",
                ),
            );
        }
    }
    let emit = str_field(t, "emit", diags);
    let emit_list = str_array(t, "emit_list", diags);
    if emit.as_deref() == Some("list") && emit_list.is_empty() {
        missing(section.header_span, "emit_list", "`emit = \"list\"`", diags);
    }
    if let Some(sp) = str_field(t, "storage_precision", diags) {
        if sp != "f64" {
            let span = t
                .get("storage_precision")
                .map(|v| inner_span(v.span))
                .unwrap_or(section.header_span);
            diags.push(
                Diagnostic::error(
                    "E0108",
                    format!("output-only f32 storage arrives in IR 1.1; `storage_precision = \"{sp}\"` is not available in IR 1.0"),
                )
                .with_primary(span, "only `\"f64\"` is accepted in IR 1.0")
                .with_suggestion(span, "f64", "use f64 storage"),
            );
        }
    }
    Some(Run {
        product: str_field(t, "product", diags),
        assumptions: str_field(t, "assumptions", diags),
        modelpoints: str_field(t, "modelpoints", diags),
        out: str_field(t, "out", diags),
        emit,
        emit_list,
        retain: str_field(t, "retain", diags),
        storage_precision: str_field(t, "storage_precision", diags),
        on_trap: str_field(t, "on_trap", diags),
        max_errors: int_field(t, "max_errors", diags),
        allow_table_drift: bool_field(t, "allow_table_drift", diags),
        sum_kahan: bool_field(t, "sum_kahan", diags),
        tables: Vec::new(),
        exec: RunExec::default(),
        span: section.header_span,
    })
}

fn lower_solve(section: &Section, diags: &mut Diagnostics) -> Option<Solve> {
    let t = &section.table;
    unknown_keys(
        t,
        &[
            "name",
            "target",
            "to",
            "vary",
            "scope",
            "tolerance",
            "max_iter",
            "method",
            "bracket",
        ],
        diags,
    );
    let name = str_field(t, "name", diags).or_else(|| {
        missing(section.header_span, "name", "[[solve]]", diags);
        None
    })?;
    let target = str_field(t, "target", diags).unwrap_or_else(|| {
        missing(section.header_span, "target", "[[solve]]", diags);
        String::new()
    });
    let vary = str_field(t, "vary", diags).unwrap_or_else(|| {
        missing(section.header_span, "vary", "[[solve]]", diags);
        String::new()
    });
    let bracket = t.get("bracket").and_then(|v| match &v.value {
        Value::Array(items) if items.len() == 2 => {
            let num = |x: &Value| match x {
                Value::Float(f) => Some(*f),
                Value::Int(i) => Some(*i as f64),
                _ => None,
            };
            match (num(&items[0].value), num(&items[1].value)) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => {
                    type_error(v.span, "bracket", "a pair of numbers", &v.value, diags);
                    None
                }
            }
        }
        other => {
            type_error(v.span, "bracket", "a pair of numbers", other, diags);
            None
        }
    });
    Some(Solve {
        name,
        target,
        to: float_field(t, "to", diags).unwrap_or(0.0),
        vary,
        scope: str_field(t, "scope", diags),
        tolerance: float_field(t, "tolerance", diags),
        max_iter: int_field(t, "max_iter", diags),
        method: str_field(t, "method", diags),
        bracket,
        span: section.header_span,
    })
}

fn lower_aggregation(section: &Section, diags: &mut Diagnostics) -> Option<Aggregation> {
    let t = &section.table;
    unknown_keys(
        t,
        &[
            "name", "group_by", "measure", "op", "weight", "filter", "over_t",
        ],
        diags,
    );
    let name = str_field(t, "name", diags).or_else(|| {
        missing(section.header_span, "name", "[[aggregation]]", diags);
        None
    })?;
    let measure = str_field(t, "measure", diags).unwrap_or_else(|| {
        missing(section.header_span, "measure", "[[aggregation]]", diags);
        String::new()
    });
    let op = str_field(t, "op", diags).unwrap_or_else(|| {
        missing(section.header_span, "op", "[[aggregation]]", diags);
        String::new()
    });
    const OPS: &[&str] = &["sum", "mean", "min", "max", "count", "weighted_mean"];
    if !op.is_empty() && !OPS.contains(&op.as_str()) {
        let span = t.get("op").map(|v| v.span).unwrap_or(section.header_span);
        let mut d = Diagnostic::error("E0043", format!("`{op}` is not an aggregation op"))
            .with_primary(span, format!("expected one of: {}", OPS.join(", ")));
        if let Some(near) = nearest(&op, OPS) {
            d = d.with_suggestion(
                span,
                format!("\"{near}\""),
                format!("did you mean `{near}`?"),
            );
        }
        diags.push(d);
    }
    let weight = str_field(t, "weight", diags);
    if op == "weighted_mean" && weight.is_none() {
        missing(
            section.header_span,
            "weight",
            "`op = \"weighted_mean\"`",
            diags,
        );
    }
    Some(Aggregation {
        name,
        group_by: str_array(t, "group_by", diags),
        measure,
        op,
        weight,
        filter: str_field(t, "filter", diags),
        over_t: str_field(t, "over_t", diags),
        span: section.header_span,
    })
}

/// The span of a quoted value's contents — a diagnostic points at what the
/// author wrote, not at the quotes around it.
fn inner_span(span: Span) -> Span {
    if span.len() >= 2 {
        Span::new(span.file, span.start as usize + 1, span.end as usize - 1)
    } else {
        span
    }
}
