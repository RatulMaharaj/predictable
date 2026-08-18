//! Bytes → typed columns.
//!
//! The declaration is the schema (`01-ir.md` §2.9): every key and value column
//! is typed before a byte is read, so this is a coercion pass with good errors,
//! never an inference pass. Extra columns in the source are ignored; a declared
//! column the source lacks is an error naming what *was* present.

use predictable_ir::{DType, LitValue, TableDecl};

use crate::error::{CellAt, TableError};
use crate::resolver::{TableBytes, TableFormat};

/// One key cell, normalised to the three orderable carriers.
///
/// `bool` becomes `Int(0|1)`; `date`, `str` and `enum(..)` become `Text`, which
/// is compared by equality only — enums are unordered (§2.9).
#[derive(Debug, Clone, PartialEq)]
pub enum KeyCell {
    Int(i64),
    Float(f64),
    Text(String),
}

impl KeyCell {
    pub(crate) fn cmp_key(&self, other: &KeyCell) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (KeyCell::Int(a), KeyCell::Int(b)) => a.cmp(b),
            (KeyCell::Float(a), KeyCell::Float(b)) => a.total_cmp(b),
            (KeyCell::Text(a), KeyCell::Text(b)) => a.cmp(b),
            // Mixed carriers cannot occur within a column (the dtype fixes the
            // carrier); order them by discriminant so `sort` stays total.
            _ => self.rank().cmp(&other.rank()),
        }
        .then(Ordering::Equal)
    }

    fn rank(&self) -> u8 {
        match self {
            KeyCell::Int(_) => 0,
            KeyCell::Float(_) => 1,
            KeyCell::Text(_) => 2,
        }
    }

    /// Rendering used in duplicate-key and miss messages.
    pub fn render(&self) -> String {
        match self {
            KeyCell::Int(i) => i.to_string(),
            KeyCell::Float(x) => predictable_fmt::format_f64(*x),
            KeyCell::Text(s) => s.clone(),
        }
    }
}

/// One value column, in the dtype the declaration gave it.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueColumn {
    F64(Vec<f64>),
    I64(Vec<i64>),
    Bool(Vec<bool>),
    /// `date`, `str` and `enum(..)` values.
    Text(Vec<String>),
}

impl ValueColumn {
    pub fn len(&self) -> usize {
        match self {
            ValueColumn::F64(v) => v.len(),
            ValueColumn::I64(v) => v.len(),
            ValueColumn::Bool(v) => v.len(),
            ValueColumn::Text(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The dtype name, for messages.
    pub fn dtype_name(&self) -> &'static str {
        match self {
            ValueColumn::F64(_) => "f64",
            ValueColumn::I64(_) => "i64",
            ValueColumn::Bool(_) => "bool",
            ValueColumn::Text(_) => "str",
        }
    }

    fn push_lit(&mut self, v: LitValue) {
        match (self, v) {
            (ValueColumn::F64(c), LitValue::Float(x)) => c.push(x),
            (ValueColumn::F64(c), LitValue::Int(i)) => c.push(i as f64),
            (ValueColumn::I64(c), LitValue::Int(i)) => c.push(i),
            (ValueColumn::Bool(c), LitValue::Bool(b)) => c.push(b),
            (ValueColumn::Text(c), LitValue::Text(s)) => c.push(s),
            (col, other) => {
                unreachable!("value {other:?} does not fit column {}", col.dtype_name())
            }
        }
    }
}

fn empty_column(dtype: &DType) -> ValueColumn {
    match dtype {
        DType::F64 => ValueColumn::F64(Vec::new()),
        DType::I64 => ValueColumn::I64(Vec::new()),
        DType::Bool => ValueColumn::Bool(Vec::new()),
        DType::Date | DType::Str | DType::Enum(_) => ValueColumn::Text(Vec::new()),
    }
}

/// The typed content of one table, before any index is built.
#[derive(Debug, Clone, PartialEq)]
pub struct RawTable {
    /// One column per declared key, in declaration order.
    pub keys: Vec<Vec<KeyCell>>,
    /// One column per declared value, in declaration order.
    pub values: Vec<ValueColumn>,
    pub rows: usize,
}

/// Parse whatever the resolver returned into typed columns.
pub fn parse(decl: &TableDecl, bytes: &TableBytes) -> Result<RawTable, TableError> {
    match bytes.format {
        TableFormat::Csv => parse_csv(decl, &bytes.bytes),
        TableFormat::Inline => parse_inline(decl),
        other => Err(TableError::UnsupportedFormat {
            table: decl.name.clone(),
            format: other.label(),
            origin: bytes.origin.clone(),
        }),
    }
}

fn parse_inline(decl: &TableDecl) -> Result<RawTable, TableError> {
    let rows = decl.rows.as_deref().unwrap_or(&[]);
    let want = decl.keys.len() + decl.values.len();
    let mut keys: Vec<Vec<KeyCell>> = vec![Vec::with_capacity(rows.len()); decl.keys.len()];
    let mut values: Vec<ValueColumn> = decl.values.iter().map(|v| empty_column(&v.dtype)).collect();

    for (r, row) in rows.iter().enumerate() {
        if row.len() != want {
            return Err(TableError::RowArity {
                table: decl.name.clone(),
                at: r + 1,
                got: row.len(),
                want,
            });
        }
        for (k, key) in decl.keys.iter().enumerate() {
            let at = CellAt {
                row: r + 1,
                column: key.name.clone(),
            };
            keys[k].push(key_from_lit(&decl.name, &row[k], &key.dtype, at)?);
        }
        for (v, value) in decl.values.iter().enumerate() {
            let at = CellAt {
                row: r + 1,
                column: value.name.clone(),
            };
            let lit = coerce_lit(&decl.name, &row[decl.keys.len() + v], &value.dtype, at)?;
            values[v].push_lit(lit);
        }
    }

    Ok(RawTable {
        keys,
        values,
        rows: rows.len(),
    })
}

fn parse_csv(decl: &TableDecl, bytes: &[u8]) -> Result<RawTable, TableError> {
    let text = std::str::from_utf8(bytes).map_err(|_| TableError::NotUtf8 {
        table: decl.name.clone(),
    })?;
    let mut records = read_csv(text);
    let header = records.next().ok_or_else(|| TableError::NoHeader {
        table: decl.name.clone(),
    })?;
    let header: Vec<String> = header.into_iter().map(|h| h.trim().to_string()).collect();

    let column_of = |name: &str| -> Result<usize, TableError> {
        header
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| TableError::MissingColumn {
                table: decl.name.clone(),
                column: name.to_string(),
                present: header.join(", "),
            })
    };
    let key_at: Vec<usize> = decl
        .keys
        .iter()
        .map(|k| column_of(&k.name))
        .collect::<Result<_, _>>()?;
    let value_at: Vec<usize> = decl
        .values
        .iter()
        .map(|v| column_of(&v.name))
        .collect::<Result<_, _>>()?;

    let mut keys: Vec<Vec<KeyCell>> = vec![Vec::new(); decl.keys.len()];
    let mut values: Vec<ValueColumn> = decl.values.iter().map(|v| empty_column(&v.dtype)).collect();
    let mut rows = 0usize;

    for record in records {
        if record.len() < header.len() {
            return Err(TableError::RowArity {
                table: decl.name.clone(),
                at: rows + 1,
                got: record.len(),
                want: header.len(),
            });
        }
        rows += 1;
        for (k, key) in decl.keys.iter().enumerate() {
            let at = CellAt {
                row: rows,
                column: key.name.clone(),
            };
            let cell = key_from_text(&decl.name, record[key_at[k]].trim(), &key.dtype, at)?;
            keys[k].push(cell);
        }
        for (v, value) in decl.values.iter().enumerate() {
            let at = CellAt {
                row: rows,
                column: value.name.clone(),
            };
            let lit = lit_from_text(&decl.name, record[value_at[v]].trim(), &value.dtype, at)?;
            values[v].push_lit(lit);
        }
    }

    Ok(RawTable { keys, values, rows })
}

/// A minimal RFC-4180 reader: quoted fields, `""` escapes, CR/LF or LF line
/// ends, blank lines and `#` comment lines skipped. Enough for an assumption
/// table and nothing more — Parquet is the preferred path (`03-engine.md` §6).
fn read_csv(text: &str) -> impl Iterator<Item = Vec<String>> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || loop {
        chars.peek()?;
        let mut record: Vec<String> = Vec::new();
        let mut field = String::new();
        let mut quoted = false;
        loop {
            let Some((_, c)) = chars.next() else {
                record.push(field);
                break;
            };
            if quoted {
                if c == '"' {
                    if matches!(chars.peek(), Some((_, '"'))) {
                        chars.next();
                        field.push('"');
                    } else {
                        quoted = false;
                    }
                } else {
                    field.push(c);
                }
                continue;
            }
            match c {
                '"' if field.is_empty() => quoted = true,
                ',' => record.push(std::mem::take(&mut field)),
                '\r' => {}
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    break;
                }
                other => field.push(other),
            }
        }
        let blank = record.iter().all(|f| f.trim().is_empty());
        let comment = record
            .first()
            .is_some_and(|f| f.trim_start().starts_with('#'));
        if blank || comment {
            continue;
        }
        return Some(record);
    })
}

fn bad(table: &str, at: CellAt, text: &str, dtype: &DType) -> TableError {
    TableError::BadCell {
        table: table.to_string(),
        at,
        text: text.to_string(),
        dtype: dtype.to_string(),
    }
}

fn key_from_text(
    table: &str,
    text: &str,
    dtype: &DType,
    at: CellAt,
) -> Result<KeyCell, TableError> {
    Ok(match lit_from_text(table, text, dtype, at.clone())? {
        LitValue::Int(i) => KeyCell::Int(i),
        LitValue::Float(x) => KeyCell::Float(x),
        LitValue::Bool(b) => KeyCell::Int(b as i64),
        LitValue::Text(s) => KeyCell::Text(s),
    })
}

fn key_from_lit(
    table: &str,
    lit: &LitValue,
    dtype: &DType,
    at: CellAt,
) -> Result<KeyCell, TableError> {
    Ok(match coerce_lit(table, lit, dtype, at)? {
        LitValue::Int(i) => KeyCell::Int(i),
        LitValue::Float(x) => KeyCell::Float(x),
        LitValue::Bool(b) => KeyCell::Int(b as i64),
        LitValue::Text(s) => KeyCell::Text(s),
    })
}

/// Text → the literal carrier of `dtype`. Nothing is guessed: a `f64` column
/// accepts what `f64::from_str` accepts and nothing else.
fn lit_from_text(
    table: &str,
    text: &str,
    dtype: &DType,
    at: CellAt,
) -> Result<LitValue, TableError> {
    match dtype {
        DType::F64 => text
            .parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .map(LitValue::Float)
            .ok_or_else(|| bad(table, at, text, dtype)),
        DType::I64 => text
            .parse::<i64>()
            .map(LitValue::Int)
            .map_err(|_| bad(table, at, text, dtype)),
        DType::Bool => match text.to_ascii_lowercase().as_str() {
            "true" | "1" | "t" | "y" | "yes" => Ok(LitValue::Bool(true)),
            "false" | "0" | "f" | "n" | "no" => Ok(LitValue::Bool(false)),
            _ => Err(bad(table, at, text, dtype)),
        },
        DType::Date | DType::Str | DType::Enum(_) => {
            if text.is_empty() {
                Err(bad(table, at, text, dtype))
            } else {
                Ok(LitValue::Text(text.to_string()))
            }
        }
    }
}

/// An inline literal → the carrier of `dtype`, widening `Int` to `Float` for an
/// `f64` column (`rows = [[1, 0.12]]` writes `1`, not `1.0`, for a money value).
fn coerce_lit(
    table: &str,
    lit: &LitValue,
    dtype: &DType,
    at: CellAt,
) -> Result<LitValue, TableError> {
    match (lit, dtype) {
        (LitValue::Int(i), DType::F64) => Ok(LitValue::Float(*i as f64)),
        (LitValue::Float(x), DType::F64) if x.is_finite() => Ok(LitValue::Float(*x)),
        (LitValue::Int(i), DType::I64) => Ok(LitValue::Int(*i)),
        (LitValue::Bool(b), DType::Bool) => Ok(LitValue::Bool(*b)),
        (LitValue::Text(s), DType::Date | DType::Str | DType::Enum(_)) if !s.is_empty() => {
            Ok(LitValue::Text(s.clone()))
        }
        _ => Err(bad(table, at, &lit.to_string(), dtype)),
    }
}
