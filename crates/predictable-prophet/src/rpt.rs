//! The `.rpt` results reader (`04-verify.md` §4.3).
//!
//! The `.rpt` is the reconciliation target, so the reader's job is as much about
//! what it *refuses* as what it parses. It will not guess the period base: an
//! off-by-one in `t` produces a diff at every timestep and is the single most
//! expensive false alarm in the migration loop, so a 1-based file without an
//! explicit `--period-base` is `P0302` and nothing else happens.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use predictable_diagnostics::{Diagnostic, Span, Suggestion};
use predictable_ir::Basis;

use crate::common::{header_entry, is_comment, Header, Read};
use crate::text::{
    detect_encoding, is_identifier, lines, snake_case, split_fields, Encoding, Field,
};

/// Header keys the `.rpt` reader recognises (§4.3.1).
pub const RECOGNISED_KEYS: &[&str] = &[
    "RUN",
    "PRODUCT",
    "RUN_DATE",
    "TIME_UNITS",
    "NUM_PERIODS",
    "VALUATION_DATE",
];

/// Column names accepted as the period axis (§4.3.2).
pub const PERIOD_NAMES: &[&str] = &["PERIOD", "T", "TIME", "MONTH", "YEAR", "DURATION"];

/// Caller-supplied decisions the file itself cannot settle (§4.3.3, §4.4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RptOptions {
    /// `--period-base {0|1}`, or `period_base` from `migration/mapping.toml`.
    pub period_base: Option<i64>,
    /// The IR timeline's basis, checked against `TIME_UNITS`.
    pub timeline_basis: Option<Basis>,
    /// Which column holds the model point key, when more than one could.
    pub mp_key_column: Option<String>,
    /// `prophet name -> predictable component id`, from the mapping file.
    pub component_names: BTreeMap<String, String>,
}

/// Which of the two `.rpt` shapes the file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RptLevel {
    /// One row per (model point, period).
    #[default]
    PerModelPoint,
    /// One row per (group, period): `mp_key` is the group and model-point-level
    /// diffing is unavailable (`P0304`).
    Grouped,
}

/// One imported component.
#[derive(Debug, Clone, PartialEq)]
pub struct RptComponent {
    /// The component id used in `results.parquet`, mapped or `prophet.<name>`.
    pub id: String,
    /// The raw Prophet column name.
    pub source_name: String,
    /// True when the name came from the mapping file rather than the `prophet.` prefix.
    pub mapped: bool,
}

/// One result row, already rebased onto the IR's `t`.
#[derive(Debug, Clone, PartialEq)]
pub struct RptRow {
    /// Model point key, or the group name for a grouped file.
    pub mp_key: String,
    /// 0-based, in first-seen order of `mp_key`.
    pub mp_row: u32,
    /// `period - period_base`.
    pub t: i32,
    /// One entry per component, `None` for an empty cell.
    pub values: Vec<Option<f64>>,
}

/// A parsed `.rpt`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RptResults {
    /// The file name the spans point into.
    pub file: String,
    /// How the bytes were decoded.
    pub encoding: Option<Encoding>,
    /// Recognised keys, comments and retained unknown keys.
    pub header: Header,
    /// `RUN`.
    pub run: Option<String>,
    /// `PRODUCT`.
    pub product: Option<String>,
    /// `RUN_DATE`, verbatim.
    pub run_date: Option<String>,
    /// `TIME_UNITS`, upper-cased.
    pub time_units: Option<String>,
    /// `NUM_PERIODS`.
    pub num_periods: Option<u32>,
    /// `VALUATION_DATE`, verbatim.
    pub valuation_date: Option<String>,
    /// Grouped or per-model-point.
    pub level: RptLevel,
    /// The raw name of the period column.
    pub period_column: Option<String>,
    /// The raw name of the model point key column, if there is one.
    pub mp_key_column: Option<String>,
    /// Observed period range, before rebasing.
    pub period_min: Option<i64>,
    /// Observed period range, before rebasing.
    pub period_max: Option<i64>,
    /// The base actually applied; `None` when it could not be settled (`P0302`).
    pub period_base: Option<i64>,
    /// Components, in column order.
    pub components: Vec<RptComponent>,
    /// Rows, in file order.
    pub rows: Vec<RptRow>,
}

impl RptResults {
    /// The distinct model point keys, in first-seen order.
    pub fn mp_keys(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for row in &self.rows {
            if seen.insert(row.mp_key.as_str()) {
                out.push(row.mp_key.as_str());
            }
        }
        out
    }

    /// Number of `(mp, component, t)` cells this file contributes to
    /// `results.parquet` — empty cells contribute nothing.
    pub fn cell_count(&self) -> usize {
        self.rows
            .iter()
            .map(|r| r.values.iter().filter(|v| v.is_some()).count())
            .sum()
    }

    /// The value of one component for one row.
    pub fn value(&self, row: usize, component_id: &str) -> Option<f64> {
        let idx = self.components.iter().position(|c| c.id == component_id)?;
        self.rows.get(row)?.values.get(idx).copied().flatten()
    }
}

/// Read a `.rpt`. Pure: bytes in, value and diagnostics out, no IO, no panics.
pub fn read_rpt(bytes: &[u8], file: &str, options: &RptOptions) -> Read<RptResults> {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut out = RptResults {
        file: file.to_string(),
        ..RptResults::default()
    };

    let (encoding, bad_at) = detect_encoding(bytes);
    out.encoding = Some(encoding);
    if let Some(at) = bad_at {
        diags.push(
            Diagnostic::new("P0101", "file is not valid utf-8; decoded as windows-1252").span(
                Span::primary(file, at..(at + 1).min(bytes.len())).label("first non-utf-8 byte"),
            ),
        );
    }

    let all = lines(bytes);
    let mut candidates: Vec<(Vec<Field>, Range<usize>)> = Vec::new();
    let mut data: Vec<(usize, Vec<Field>, Range<usize>)> = Vec::new();
    let mut in_data = false;

    for line in &all {
        let line = line.trimmed();
        if line.is_blank() {
            continue;
        }
        if is_comment(&line) {
            out.header.comments.push(line.text());
            continue;
        }
        let entry = header_entry(&line);
        if !in_data && RECOGNISED_KEYS.contains(&entry.key.as_str()) {
            out.header.entries.insert(entry.key.clone(), entry);
            continue;
        }
        let fields = split_fields(&line);
        let all_names = fields.len() >= 2 && fields.iter().all(|f| is_identifier(&f.text));
        if !in_data && all_names {
            candidates.push((fields, line.range()));
        } else {
            in_data = true;
            data.push((line.number, fields, line.range()));
        }
    }

    out.run = out.header.value("RUN").map(str::to_string);
    out.product = out.header.value("PRODUCT").map(str::to_string);
    out.run_date = out.header.value("RUN_DATE").map(str::to_string);
    out.time_units = out
        .header
        .value("TIME_UNITS")
        .map(|v| v.to_ascii_uppercase());
    out.valuation_date = out.header.value("VALUATION_DATE").map(str::to_string);
    out.num_periods = out
        .header
        .value("NUM_PERIODS")
        .and_then(|v| v.trim().parse().ok());

    // The name line is the last all-identifier line before the first data row;
    // everything before it that is not a recognised key is retained verbatim.
    let name_line = candidates.pop();
    for (fields, span) in &candidates {
        let key = fields[0].key();
        let rest: Vec<String> = fields[1..].iter().map(|f| f.text.clone()).collect();
        out.header.extra.insert(key.clone(), rest.join(","));
        diags.push(
            Diagnostic::new(
                "P0102",
                format!("unrecognised header key `{key}` retained verbatim"),
            )
            .span(Span::primary(file, span.clone()).label("kept in `header.extra`")),
        );
    }

    let Some((names, names_span)) = name_line else {
        diags.push(
            Diagnostic::new(
                "P0110",
                "no variable-name line found before the first data row",
            )
            .span(Span::primary(file, 0..bytes.len().min(1)).label("in this file"))
            .note("a `.rpt` needs a line of column names between the header block and the data"),
        );
        return Read {
            value: out,
            diagnostics: diags,
        };
    };

    if data.is_empty() {
        diags.push(
            Diagnostic::new(
                "P0110",
                "the file has a variable-name line but no data rows",
            )
            .span(Span::primary(file, names_span.clone()).label("columns declared here")),
        );
        return Read {
            value: out,
            diagnostics: diags,
        };
    }

    // §4.3.2: the period column, by name.
    let period_idx = names
        .iter()
        .position(|f| PERIOD_NAMES.contains(&f.key().as_str()));
    let Some(period_idx) = period_idx else {
        diags.push(
            Diagnostic::new(
                "P0301",
                format!(
                    "no period column: none of {} appears in the variable-name line",
                    PERIOD_NAMES.join(", ")
                ),
            )
            .span(
                Span::primary(file, names_span.clone()).label(
                    names
                        .iter()
                        .map(|f| f.text.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            )
            .note("a results file without a period axis cannot be diffed by timestep"),
        );
        return Read {
            value: out,
            diagnostics: diags,
        };
    };
    out.period_column = Some(names[period_idx].text.clone());

    // Classify the remaining columns: numeric ones become components, non-numeric
    // ones are model point key candidates.
    let width = names.len();
    let mut numeric = vec![true; width];
    let mut non_empty = vec![false; width];
    for (_, fields, _) in &data {
        for (i, f) in fields.iter().enumerate().take(width) {
            if f.text.is_empty() {
                continue;
            }
            non_empty[i] = true;
            if f.text.trim().parse::<f64>().is_err() {
                numeric[i] = false;
            }
        }
    }

    let key_candidates: Vec<usize> = (0..width)
        .filter(|i| *i != period_idx && non_empty[*i] && !numeric[*i])
        .collect();
    let mut key_idx: Option<usize> = None;
    if let Some(requested) = &options.mp_key_column {
        key_idx = names
            .iter()
            .position(|f| f.key() == requested.to_ascii_uppercase());
        if key_idx.is_none() {
            diags.push(
                Diagnostic::new(
                    "P0305",
                    format!("requested model point key column `{requested}` is not in this file"),
                )
                .span(Span::primary(file, names_span.clone()).label("variable-name line")),
            );
        }
    }
    if key_idx.is_none() {
        match key_candidates.len() {
            0 => {
                out.level = RptLevel::Grouped;
                diags.push(
                    Diagnostic::new(
                        "P0304",
                        "no model point key column; the file is aggregate-level and diffs at group level",
                    )
                    .span(Span::primary(file, names_span.clone()).label("no non-numeric key column"))
                    .note("re-export the Prophet run with the model point number to diff per policy"),
                );
            }
            1 => key_idx = Some(key_candidates[0]),
            _ => {
                let listed: Vec<&str> = key_candidates
                    .iter()
                    .map(|i| names[*i].text.as_str())
                    .collect();
                diags.push(
                    Diagnostic::new(
                        "P0305",
                        format!(
                            "several columns could be the model point key: {}",
                            listed.join(", ")
                        ),
                    )
                    .span(Span::primary(file, names_span.clone()).label("variable-name line"))
                    .suggestion(
                        Suggestion::new(format!(
                        "pass `--mp-key {}`, or set `mp_key.prophet` in `migration/mapping.toml`",
                        listed[0]
                    ))
                        .applicability(predictable_diagnostics::Applicability::HasPlaceholders),
                    ),
                );
                key_idx = Some(key_candidates[0]);
            }
        }
    }
    out.mp_key_column = key_idx.map(|i| names[i].text.clone());

    // §4.3.5: the numeric columns become components.
    for (i, name) in names.iter().enumerate() {
        if i == period_idx || Some(i) == key_idx || !numeric[i] {
            continue;
        }
        let mapped = options.component_names.get(&name.text);
        out.components.push(RptComponent {
            id: mapped
                .cloned()
                .unwrap_or_else(|| format!("prophet.{}", snake_case(&name.text))),
            source_name: name.text.clone(),
            mapped: mapped.is_some(),
        });
    }

    // §4.3.4: `TIME_UNITS` must agree with the timeline.
    if let (Some(units), Some(basis)) = (&out.time_units, options.timeline_basis) {
        let observed = basis_of(units);
        if observed != Some(basis) {
            diags.push(
                Diagnostic::new(
                    "P0303",
                    format!(
                        "`TIME_UNITS, {units}` does not match the timeline basis `{}`",
                        basis_name(basis)
                    ),
                )
                .span(
                    Span::primary(
                        file,
                        out.header.get("TIME_UNITS").map(|e| e.span.clone()).unwrap_or(0..0),
                    )
                    .label("Prophet time units"),
                )
                .note("either change the model's timeline basis or aggregate the Prophet output before diffing"),
            );
        }
    }

    // §4.3.3: the period base is a recorded decision, never a guess.
    let mut periods: Vec<Option<i64>> = Vec::with_capacity(data.len());
    for (line_no, fields, span) in &data {
        if fields.len() != width {
            let code = if fields.len() < width {
                "P0105"
            } else {
                "P0106"
            };
            diags.push(
                Diagnostic::new(
                    code,
                    format!(
                        "data row on line {line_no} has {} field(s); the variable-name line declares {width}",
                        fields.len()
                    ),
                )
                .span(Span::primary(file, span.clone()).label("row skipped")),
            );
            periods.push(None);
            continue;
        }
        periods.push(fields[period_idx].text.trim().parse::<i64>().ok());
    }
    out.period_min = periods.iter().flatten().copied().min();
    out.period_max = periods.iter().flatten().copied().max();

    let base = match (options.period_base, out.period_min) {
        (Some(b), _) => Some(b),
        (None, Some(0)) => Some(0),
        (None, Some(min)) => {
            diags.push(
                Diagnostic::new(
                    "P0302",
                    format!(
                        "period column `{}` runs {min}..{}; the period base is not stated",
                        out.period_column.clone().unwrap_or_default(),
                        out.period_max.unwrap_or(min)
                    ),
                )
                .span(Span::primary(file, names[period_idx].range()).label("period axis"))
                .note(
                    "an off-by-one in `t` produces a diff at every timestep, so the base is an \
                     explicit, one-time, recorded decision",
                )
                .suggestion(
                    Suggestion::new(format!(
                        "pass `--period-base {min}` (or set `period_base` in `migration/mapping.toml`) \
                         if period {min} is the model's `t = 0`"
                    ))
                    .applicability(predictable_diagnostics::Applicability::MaybeIncorrect),
                ),
            );
            None
        }
        (None, None) => None,
    };
    out.period_base = base;

    if let Some(base) = base {
        let mut mp_rows: Vec<String> = Vec::new();
        for ((_, fields, _), period) in data.iter().zip(periods.iter()) {
            let (Some(period), true) = (period, fields.len() == width) else {
                continue;
            };
            let mp_key = match key_idx {
                Some(i) => fields[i].text.clone(),
                None => out
                    .product
                    .clone()
                    .or_else(|| out.run.clone())
                    .unwrap_or_else(|| "group".to_string()),
            };
            let mp_row = match mp_rows.iter().position(|k| *k == mp_key) {
                Some(r) => r,
                None => {
                    mp_rows.push(mp_key.clone());
                    mp_rows.len() - 1
                }
            } as u32;
            let values = out
                .components
                .iter()
                .map(|c| {
                    names
                        .iter()
                        .position(|n| n.text == c.source_name)
                        .and_then(|i| fields.get(i))
                        .filter(|f| !f.text.is_empty())
                        .and_then(|f| f.text.trim().parse::<f64>().ok())
                })
                .collect();
            out.rows.push(RptRow {
                mp_key,
                mp_row,
                t: (*period - base) as i32,
                values,
            });
        }
    }

    Read {
        value: out,
        diagnostics: diags,
    }
}

fn basis_of(units: &str) -> Option<Basis> {
    Some(match units.trim().to_ascii_uppercase().as_str() {
        "MONTHS" | "MONTHLY" | "MONTH" => Basis::Monthly,
        "QUARTERS" | "QUARTERLY" | "QUARTER" => Basis::Quarterly,
        "YEARS" | "YEARLY" | "ANNUAL" | "ANNUALLY" | "YEAR" => Basis::Annual,
        _ => return None,
    })
}

/// The IR spelling of a basis, for messages.
pub fn basis_name(basis: Basis) -> &'static str {
    match basis {
        Basis::Monthly => "monthly",
        Basis::Quarterly => "quarterly",
        Basis::Annual => "annual",
    }
}
