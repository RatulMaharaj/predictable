//! [`CsvSource`] — modelpoints from a delimited text file.
//!
//! CSV has no types, so the IR schema does all the work: the header is matched against the load
//! plan before row 1, and each cell is parsed as the declared dtype. An empty (or `NA`/`null`)
//! cell is a null, and §2.11 eliminates it at load — default for an optional field, error for a
//! required one.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{IoError, Lint};
use crate::inbound::{check_layout, ChunkColumns, ModelpointSource, MpColumn};
use crate::schema::{parse_date, MpField, MpSchema};

/// Cell texts treated as a null. Case-insensitive.
const NULL_TEXTS: &[&str] = &["", "na", "n/a", "null", "nan", "none"];

/// Modelpoints from CSV.
pub struct CsvSource {
    schema: MpSchema,
    file: String,
    reader: csv::Reader<Box<dyn Read + Send>>,
    /// Header position for each schema field; `None` for an optional field the header omits.
    columns: Vec<Option<usize>>,
    lints: Vec<Lint>,
    record: csv::StringRecord,
    next_chunk_idx: u64,
    next_row: u64,
    done: bool,
}

impl std::fmt::Debug for CsvSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CsvSource")
            .field("file", &self.file)
            .field("fields", &self.schema.len())
            .field("next_chunk_idx", &self.next_chunk_idx)
            .finish()
    }
}

impl CsvSource {
    /// Open `path` against `schema`.
    pub fn open(schema: MpSchema, path: impl AsRef<Path>) -> Result<CsvSource, IoError> {
        let path = path.as_ref();
        let file = path.display().to_string();
        let handle = File::open(path)?;
        Self::from_reader(schema, Box::new(handle), file)
    }

    /// Open any reader against `schema`. The header is validated before any data row is decoded.
    pub fn from_reader(
        schema: MpSchema,
        reader: Box<dyn Read + Send>,
        file: impl Into<String>,
    ) -> Result<CsvSource, IoError> {
        let file = file.into();
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(reader);
        let header = reader.headers().map_err(|e| IoError::Backend {
            file: file.clone(),
            message: e.to_string(),
        })?;
        let header: Vec<String> = header.iter().map(|h| h.trim().to_string()).collect();

        let mut columns = Vec::with_capacity(schema.len());
        for field in schema.fields() {
            let hits: Vec<usize> = header
                .iter()
                .enumerate()
                .filter(|(_, h)| *h == &field.name)
                .map(|(i, _)| i)
                .collect();
            match hits.len() {
                0 => {
                    if field.required {
                        return Err(IoError::MissingColumn {
                            file: file.clone(),
                            column: field.name.clone(),
                            referenced_by: field.referenced_by.clone(),
                        });
                    }
                    columns.push(None);
                }
                1 => columns.push(Some(hits[0])),
                _ => {
                    return Err(IoError::DuplicateColumn {
                        file: file.clone(),
                        column: field.name.clone(),
                    })
                }
            }
        }

        let mut lints: Vec<Lint> = header
            .iter()
            .filter(|h| schema.field(h).is_none() && !schema.is_pruned(h))
            .map(|h| Lint::UnknownColumn {
                file: file.clone(),
                column: h.clone(),
            })
            .collect();
        lints.extend(schema.pruned_lints(&file));

        Ok(CsvSource {
            schema,
            file,
            reader,
            columns,
            lints,
            record: csv::StringRecord::new(),
            next_chunk_idx: 0,
            next_row: 0,
            done: false,
        })
    }
}

impl ModelpointSource for CsvSource {
    fn schema(&self) -> &MpSchema {
        &self.schema
    }

    fn next_chunk(&mut self, n: usize, out: &mut ChunkColumns) -> Result<usize, IoError> {
        check_layout(&self.schema, out, &self.file)?;
        out.reset(self.next_chunk_idx, self.next_row);
        let mut written = 0usize;
        while written < n && !self.done {
            let has_row =
                self.reader
                    .read_record(&mut self.record)
                    .map_err(|e| IoError::Backend {
                        file: self.file.clone(),
                        message: e.to_string(),
                    })?;
            if !has_row {
                self.done = true;
                break;
            }
            let file_row = self.next_row + written as u64 + 1;
            for (i, field) in self.schema.fields().iter().enumerate() {
                let cell = self.columns[i]
                    .and_then(|c| self.record.get(c))
                    .map(str::trim);
                match cell {
                    Some(text) if !is_null_text(text) => {
                        push_text(out, i, field, text, &self.file, file_row)?;
                        out.note_presence(i, true);
                    }
                    _ => {
                        if field.required {
                            return Err(IoError::NullInRequired {
                                file: self.file.clone(),
                                column: field.name.clone(),
                                row: file_row,
                            });
                        }
                        out.push_absent(i, field)?;
                    }
                }
            }
            written += 1;
        }
        out.finish(written);
        if written > 0 {
            self.next_chunk_idx += 1;
            self.next_row += written as u64;
        }
        Ok(written)
    }

    fn lints(&self) -> &[Lint] {
        &self.lints
    }
}

fn is_null_text(text: &str) -> bool {
    NULL_TEXTS.iter().any(|n| text.eq_ignore_ascii_case(n))
}

fn push_text(
    out: &mut ChunkColumns,
    i: usize,
    field: &MpField,
    text: &str,
    file: &str,
    row: u64,
) -> Result<(), IoError> {
    let bad = || IoError::BadValue {
        file: file.to_string(),
        column: field.name.clone(),
        row,
        text: text.to_string(),
        expected: field.dtype.clone(),
    };
    match out.column_mut(i) {
        MpColumn::F64(v) => v.push(text.parse::<f64>().map_err(|_| bad())?),
        MpColumn::I64(v) => v.push(text.parse::<i64>().map_err(|_| bad())?),
        MpColumn::Bool(v) => v.push(parse_bool(text).ok_or_else(bad)?),
        MpColumn::Date(v) => v.push(parse_date(text).ok_or_else(bad)?),
        MpColumn::Str(v) => v.push(text.to_string()),
        MpColumn::Enum(v) => {
            let code = field
                .variants
                .iter()
                .position(|variant| variant == text)
                .ok_or_else(|| IoError::UnknownVariant {
                    file: file.to_string(),
                    column: field.name.clone(),
                    row,
                    value: text.to_string(),
                    enum_name: field.enum_name().unwrap_or_default().to_string(),
                    expected: field.variants.clone(),
                })?;
            v.push(code as u32);
        }
    }
    Ok(())
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "y" => Some(true),
        "false" | "f" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}
