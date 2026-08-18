//! The `.fac` factor-table reader (`04-verify.md` §4.2).
//!
//! Two rules do all the work:
//!
//! * **Last dimension varies fastest.** Getting this wrong transposes a mortality
//!   table silently, so the reader restates the ordering and the first four
//!   resolved keys as `P0202` — a corner of the table a human can eyeball against
//!   the source.
//! * **The value count must be exactly `∏(HIₖ − LOₖ + 1)`.** No truncation, no
//!   padding: a mismatch is `P0203` and the table is not usable.

use std::ops::Range;

use predictable_diagnostics::{Diagnostic, Span};

use crate::common::{header_entry, is_comment, Read};
use crate::text::{detect_encoding, lines, snake_case, split_fields, Encoding};

/// Header keys the `.fac` reader recognises.
pub const RECOGNISED_KEYS: &[&str] = &["TABLE_NAME", "DIMENSIONS", "VALUES", "DATA"];

/// The lookup policy proposed for a dimension (§4.2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposedPolicy {
    /// Contiguous integer dimension, typically age.
    Clamp,
    /// Everything else.
    Exact,
}

impl ProposedPolicy {
    /// The spelling used in the `[[table]]` fragment.
    pub fn as_str(self) -> &'static str {
        match self {
            ProposedPolicy::Clamp => "clamp",
            ProposedPolicy::Exact => "exact",
        }
    }
}

/// One declared dimension, with its inclusive extent.
#[derive(Debug, Clone, PartialEq)]
pub struct FacDim {
    /// Snake-cased name (`AGE` → `age`).
    pub name: String,
    /// The original Prophet name.
    pub source_name: String,
    /// Inclusive lower bound.
    pub lo: i64,
    /// Inclusive upper bound.
    pub hi: i64,
    /// The policy the reader proposes; every one is reported as `P0204`.
    pub policy: ProposedPolicy,
    /// Byte range of the `DIMk` line.
    pub span: Range<usize>,
}

impl FacDim {
    /// Number of positions along this dimension.
    ///
    /// Saturating, because a `.fac` may declare `LO`/`HI` at the extremes of
    /// `i64` and a reader that overflows on a malformed file is a reader that
    /// panics on a malformed file.
    pub fn extent(&self) -> usize {
        self.hi
            .checked_sub(self.lo)
            .and_then(|d| d.checked_add(1))
            .unwrap_or(i64::MAX)
            .max(0)
            .try_into()
            .unwrap_or(usize::MAX)
    }
}

/// A parsed `.fac`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FacTable {
    /// The file name the spans point into.
    pub file: String,
    /// How the bytes were decoded.
    pub encoding: Option<Encoding>,
    /// `TABLE_NAME`, snake-cased, or the file stem when absent.
    pub name: String,
    /// Comment lines, retained.
    pub comments: Vec<String>,
    /// Dimensions in declaration order; the last varies fastest.
    pub dims: Vec<FacDim>,
    /// Value column names (`VALUES, qx, ix`), or `["value"]`.
    pub values: Vec<String>,
    /// Row-major data: `data[cell * values.len() + v]`.
    pub data: Vec<f64>,
}

impl FacTable {
    /// Number of cells the declared extents imply, saturating rather than
    /// wrapping on an absurd declaration.
    pub fn cell_count(&self) -> usize {
        self.dims
            .iter()
            .map(FacDim::extent)
            .fold(1usize, |a, b| a.saturating_mul(b))
    }

    /// The key tuple of cell `index`, last dimension varying fastest.
    pub fn key_at(&self, index: usize) -> Vec<i64> {
        let mut out = vec![0i64; self.dims.len()];
        let mut rem = index;
        for (i, dim) in self.dims.iter().enumerate().rev() {
            let extent = dim.extent().max(1);
            out[i] = dim.lo + (rem % extent) as i64;
            rem /= extent;
        }
        out
    }

    /// The values of cell `index`.
    pub fn values_at(&self, index: usize) -> &[f64] {
        let n = self.values.len().max(1);
        let lo = index * n;
        self.data.get(lo..lo + n).unwrap_or(&[])
    }

    /// Long-format CSV: one column per key, one per value — the shape
    /// `predictable-tables` reads.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        let mut header: Vec<String> = self.dims.iter().map(|d| d.name.clone()).collect();
        header.extend(self.values.iter().cloned());
        out.push_str(&header.join(","));
        out.push('\n');
        let n = self.values.len().max(1);
        for cell in 0..(self.data.len() / n) {
            let mut fields: Vec<String> = self
                .key_at(cell)
                .into_iter()
                .map(|k| k.to_string())
                .collect();
            fields.extend(self.values_at(cell).iter().map(|v| crate::format_f64(*v)));
            out.push_str(&fields.join(","));
            out.push('\n');
        }
        out
    }

    /// SHA-256 of the emitted CSV — the table's identity in the IR (§2.9.1).
    pub fn digest(&self) -> String {
        crate::sha256_hex(self.to_csv().as_bytes())
    }

    /// The `[[table]]` fragment, with the proposed key policies and the digest.
    pub fn to_fragment_toml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# generated by `predictable prophet fac` from {}\n[[table]]\nname = \"{}\"\n",
            self.file, self.name
        ));
        out.push_str("keys = [\n");
        for dim in &self.dims {
            out.push_str(&format!(
                "  {{ name = \"{}\", dtype = \"i64\", policy = \"{}\" }},\n",
                dim.name,
                dim.policy.as_str()
            ));
        }
        out.push_str("]\nvalues = [\n");
        for v in &self.values {
            out.push_str(&format!(
                "  {{ name = \"{v}\", dtype = \"f64\", unit = \"none\" }},\n"
            ));
        }
        out.push_str("]\non_missing = \"error\"\n");
        out.push_str(&format!("source = \"tables/{}.csv\"\n", self.name));
        out.push_str(&format!("digest = \"sha256:{}\"\n", self.digest()));
        out.push_str(&format!("rows = {}\n", self.cell_count()));
        out
    }
}

/// Read a `.fac`. Pure: bytes in, value and diagnostics out, no IO, no panics.
pub fn read_fac(bytes: &[u8], file: &str) -> Read<FacTable> {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut table = FacTable {
        file: file.to_string(),
        name: stem(file),
        values: vec!["value".to_string()],
        ..FacTable::default()
    };

    let (encoding, bad_at) = detect_encoding(bytes);
    table.encoding = Some(encoding);
    if let Some(at) = bad_at {
        diags.push(
            Diagnostic::new("P0101", "file is not valid utf-8; decoded as windows-1252").span(
                Span::primary(file, at..(at + 1).min(bytes.len())).label("first non-utf-8 byte"),
            ),
        );
    }

    let all = lines(bytes);
    let mut declared_dims: Option<(usize, Range<usize>)> = None;
    let mut dim_lines: Vec<(usize, Vec<crate::text::Field>, Range<usize>)> = Vec::new();
    let mut in_data = false;
    let mut data_fields: Vec<(String, Range<usize>)> = Vec::new();

    for line in &all {
        let line = line.trimmed();
        if line.is_blank() {
            continue;
        }
        if is_comment(&line) {
            table.comments.push(line.text());
            continue;
        }
        if in_data {
            for field in split_fields(&line) {
                for part in split_whitespace_fields(&field) {
                    data_fields.push(part);
                }
            }
            continue;
        }
        let entry = header_entry(&line);
        match entry.key.as_str() {
            "TABLE_NAME" => table.name = snake_case(entry.value()),
            "DIMENSIONS" => match entry.value().trim().parse::<usize>() {
                Ok(n) => declared_dims = Some((n, entry.span.clone())),
                Err(_) => diags.push(
                    Diagnostic::new(
                        "P0201",
                        format!(
                            "`DIMENSIONS` value `{}` is not a whole number",
                            entry.value()
                        ),
                    )
                    .span(
                        Span::primary(file, entry.span.clone()).label("expected `DIMENSIONS, <n>`"),
                    ),
                ),
            },
            "VALUES" => {
                let names: Vec<String> = entry
                    .payload
                    .iter()
                    .filter(|f| !f.is_empty())
                    .map(|f| snake_case(&f.text))
                    .collect();
                if !names.is_empty() {
                    table.values = names;
                }
            }
            "DATA" => in_data = true,
            key if key.starts_with("DIM") && key[3..].chars().all(|c| c.is_ascii_digit()) => {
                let index: usize = key[3..].parse().unwrap_or(0);
                dim_lines.push((index, entry.payload.clone(), entry.span.clone()));
            }
            _ => {
                diags.push(
                    Diagnostic::new(
                        "P0102",
                        format!("unrecognised header key `{}` retained verbatim", entry.key),
                    )
                    .span(
                        Span::primary(file, entry.span.clone())
                            .label("kept in the coverage report"),
                    ),
                );
            }
        }
    }

    let declared = match declared_dims {
        Some((n, span)) => {
            if n != dim_lines.len() {
                diags.push(
                    Diagnostic::new(
                        "P0201",
                        format!(
                            "`DIMENSIONS` says {n} but {} `DIMk` line(s) are present",
                            dim_lines.len()
                        ),
                    )
                    .span(Span::primary(file, span).label("declared dimension count")),
                );
            }
            n
        }
        None => {
            diags.push(
                Diagnostic::new("P0201", "no `DIMENSIONS` line; the table's shape is undeclared")
                    .span(Span::primary(file, 0..bytes.len().min(1)).label("in this file"))
                    .note("a `.fac` must declare `DIMENSIONS, <n>` followed by exactly n `DIMk, NAME, LO, HI` lines"),
            );
            dim_lines.len()
        }
    };

    dim_lines.sort_by_key(|(i, _, _)| *i);
    for (pos, (index, payload, span)) in dim_lines.iter().enumerate() {
        if *index != pos + 1 {
            diags.push(
                Diagnostic::new(
                    "P0201",
                    format!("expected `DIM{}` but found `DIM{index}`", pos + 1),
                )
                .span(Span::primary(file, span.clone()).label("dimensions must be numbered 1..n")),
            );
        }
        if payload.len() < 3 {
            diags.push(
                Diagnostic::new(
                    "P0201",
                    format!(
                        "`DIM{index}` needs `NAME, LO, HI`; {} field(s) given",
                        payload.len()
                    ),
                )
                .span(Span::primary(file, span.clone()).label("malformed dimension")),
            );
            continue;
        }
        let lo = payload[1].text.trim().parse::<i64>();
        let hi = payload[2].text.trim().parse::<i64>();
        match (lo, hi) {
            (Ok(lo), Ok(hi)) if lo <= hi => {
                table.dims.push(FacDim {
                    name: snake_case(&payload[0].text),
                    source_name: payload[0].text.clone(),
                    lo,
                    hi,
                    policy: if pos == 0 {
                        ProposedPolicy::Clamp
                    } else {
                        ProposedPolicy::Exact
                    },
                    span: span.clone(),
                });
            }
            _ => diags.push(
                Diagnostic::new(
                    "P0201",
                    format!(
                        "`DIM{index}` extent `{}, {}` is not a pair of whole numbers with LO <= HI",
                        payload[1].text, payload[2].text
                    ),
                )
                .span(Span::primary(file, span.clone()).label("inclusive extents, LO then HI")),
            ),
        }
    }

    if !in_data {
        diags.push(
            Diagnostic::new("P0201", "no `DATA` line; the value block is unmarked")
                .span(Span::primary(file, 0..bytes.len().min(1)).label("in this file")),
        );
    }

    // §4.2.4: the count must be exact.
    let per_cell = table.values.len().max(1);
    let expected = table.cell_count().saturating_mul(per_cell);
    let actual = data_fields.len();
    if table.dims.len() == declared && !table.dims.is_empty() && actual != expected {
        let (word, delta) = if actual < expected {
            ("missing", expected - actual)
        } else {
            ("extra", actual - expected)
        };
        let span = data_fields
            .first()
            .map(|(_, r)| r.clone())
            .unwrap_or(0..bytes.len().min(1));
        diags.push(
            Diagnostic::new(
                "P0203",
                format!(
                    "expected {expected} value(s) ({} cell(s) × {per_cell} value column(s)) but found {actual}: {delta} {word}",
                    table.cell_count()
                ),
            )
            .span(Span::primary(file, span).label("value block starts here"))
            .note("there is no truncation and no padding; fix the file or the dimension extents"),
        );
    }

    for (raw, span) in &data_fields {
        match raw.parse::<f64>() {
            Ok(v) => table.data.push(v),
            Err(_) => {
                diags.push(
                    Diagnostic::new("P0111", format!("`{raw}` is not a number"))
                        .span(Span::primary(file, span.clone()).label("in the `DATA` block")),
                );
                table.data.push(f64::NAN);
            }
        }
    }

    // §4.2.3: restate the ordering, with a corner of the table to eyeball.
    if !table.dims.is_empty() {
        let names: Vec<&str> = table.dims.iter().map(|d| d.source_name.as_str()).collect();
        let mut corner = Vec::new();
        for cell in 0..4.min(table.data.len() / per_cell) {
            let keys: Vec<String> = table
                .key_at(cell)
                .iter()
                .zip(&table.dims)
                .map(|(k, d)| format!("{}={k}", d.name))
                .collect();
            let vals: Vec<String> = table
                .values_at(cell)
                .iter()
                .map(|v| crate::format_f64(*v))
                .collect();
            corner.push(format!("({}) -> {}", keys.join(", "), vals.join(", ")));
        }
        diags.push(
            Diagnostic::new(
                "P0202",
                format!(
                    "values read row-major with `{}` varying fastest: {}",
                    names.last().copied().unwrap_or("<none>"),
                    corner.join("; ")
                ),
            )
            .span(Span::primary(file, table.dims[0].span.clone()).label("dimension order as declared"))
            .note("check these against the source table; a transposed factor file is silent otherwise"),
        );
    }

    // §4.2.6: every proposed policy is a reviewed decision.
    for dim in &table.dims {
        diags.push(
            Diagnostic::new(
                "P0204",
                format!(
                    "key `{}` proposed with `policy = \"{}\"`",
                    dim.name,
                    dim.policy.as_str()
                ),
            )
            .span(Span::primary(file, dim.span.clone()).label(match dim.policy {
                ProposedPolicy::Clamp => "first numeric dimension: contiguous integers",
                ProposedPolicy::Exact => "not the first dimension",
            }))
            .note("change it in the `[[table]]` fragment if reading off the end of this key should be an error"),
        );
    }

    Read {
        value: table,
        diagnostics: diags,
    }
}

/// Split a comma-field further on whitespace: `.fac` value blocks are sometimes
/// space-separated, sometimes comma-separated, and often both on one line.
fn split_whitespace_fields(field: &crate::text::Field) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let bytes = field.text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if b.is_ascii_whitespace() {
            if let Some(s) = start.take() {
                out.push((
                    field.text[s..i].to_string(),
                    field.start + s..field.start + i,
                ));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((
            field.text[s..].to_string(),
            field.start + s..field.start + bytes.len(),
        ));
    }
    out
}

fn stem(file: &str) -> String {
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let base = base.split('.').next().unwrap_or(base);
    snake_case(base)
}
