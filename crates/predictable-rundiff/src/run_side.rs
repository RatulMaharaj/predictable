//! One side of a run diff: a run directory read back into memory.
//!
//! A run directory is a run directory whoever wrote it (`04-verify.md` §4.3.7): the Prophet
//! importer writes the same `results.parquet` + `results.schema.json` + `manifest.json` triple the
//! engine does, so *this* module is the only reader either side needs. That is what makes
//! `predictable diff run_prophet/ run/` the same code path as diffing two predictable runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int32Type, Int64Type, Int8Type, UInt32Type};
use arrow_array::Array;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use predictable_io::outbound::{ComponentDescriptor, Manifest, ResultsSchemaDoc};

use crate::error::DiffError;

/// A single cell value, in the physical lane its component's dtype dictates.
///
/// Only [`Value::F64`] is ever compared under a tolerance; everything else compares exactly
/// (§5.6), because "close enough" is not a thing an enum, a date or a policy count can be.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `value : double`.
    F64(f64),
    /// `value_i : int64`.
    I64(i64),
    /// `value_b : bool`.
    Bool(bool),
    /// `value_s : dictionary<string>` — `str`, `enum` and `date` (ISO-8601).
    Str(String),
}

impl Value {
    /// The `f64` payload, when this is a float cell.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F64(v) => Some(*v),
            _ => None,
        }
    }

    /// A JSON rendering that survives a round trip (`float_roundtrip` is on for `serde_json`).
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::F64(v) if v.is_finite() => serde_json::json!(v),
            // JSON has no NaN/Inf. Stating the token is honest; omitting the cell would not be.
            Value::F64(v) => serde_json::json!(format!("{v}")),
            Value::I64(v) => serde_json::json!(v),
            Value::Bool(v) => serde_json::json!(v),
            Value::Str(v) => serde_json::json!(v),
        }
    }
}

/// The cells of one component, keyed by `(mp_row, t)` — the join key of §5.3 step 1.
///
/// `mp_row` rather than `mp_key` is the physical key because it is the deterministic tiebreak the
/// exemplar rule needs ("the lowest `mp_row` exhibiting the divergence at `t_first`").
pub type ComponentCells = BTreeMap<(u32, i32), Value>;

/// A run directory, read.
#[derive(Debug, Clone)]
pub struct RunSide {
    /// `a` or `b` — which side of the diff this is.
    pub label: String,
    /// The path as the user spelled it, echoed into `diff.json`.
    pub path: String,
    /// `manifest.json`. Absent only for a directory that carries results without one, which the
    /// loader refuses: a result set without a manifest is not evidence (§6.1).
    pub manifest: Manifest,
    /// `results.schema.json`.
    pub schema: ResultsSchemaDoc,
    /// `component id → cells`.
    pub components: BTreeMap<String, ComponentCells>,
    /// `mp_row → mp_key`, the identity map the report prints keys from.
    pub mp_keys: BTreeMap<u32, String>,
    /// Total rows read.
    pub rows: usize,
}

impl RunSide {
    /// Read `dir` as a run directory.
    pub fn load(label: impl Into<String>, dir: impl AsRef<Path>) -> Result<RunSide, DiffError> {
        let dir = dir.as_ref();
        let label = label.into();
        if !dir.is_dir() {
            return Err(DiffError::NotARunDir {
                path: display(dir),
                why: "not a directory".into(),
            });
        }
        let manifest = read_manifest(dir)?;
        let schema = read_schema(dir)?;

        let mut side = RunSide {
            label,
            path: display(dir),
            manifest,
            schema,
            components: BTreeMap::new(),
            mp_keys: BTreeMap::new(),
            rows: 0,
        };
        // An aborted run has a manifest and no results (§6.3 rule 6). Reporting that is the
        // diff's job; inventing an empty result set for it is not.
        if !side.manifest.execution.outcome.has_results() {
            return Err(DiffError::NoResults {
                path: side.path,
                outcome: format!("{:?}", side.manifest.execution.outcome).to_lowercase(),
            });
        }
        side.read_results(&dir.join(&side.manifest.results.path))?;
        Ok(side)
    }

    /// The descriptor of a component, when the schema knows it.
    pub fn descriptor(&self, id: &str) -> Option<&ComponentDescriptor> {
        self.schema.get(id)
    }

    /// `run_id`, for the report header.
    pub fn run_id(&self) -> &str {
        &self.manifest.run_id
    }

    /// `"predictable"` or `"prophet"`.
    pub fn system(&self) -> &str {
        &self.manifest.system
    }

    fn read_results(&mut self, path: &Path) -> Result<(), DiffError> {
        let file = std::fs::File::open(path).map_err(|e| DiffError::NotARunDir {
            path: display(path),
            why: e.to_string(),
        })?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| DiffError::Parquet {
                path: display(path),
                why: e.to_string(),
            })?
            .build()
            .map_err(|e| DiffError::Parquet {
                path: display(path),
                why: e.to_string(),
            })?;

        for batch in reader {
            let batch = batch.map_err(|e| DiffError::Parquet {
                path: display(path),
                why: e.to_string(),
            })?;
            let col = |name: &str| -> Result<arrow_array::ArrayRef, DiffError> {
                batch
                    .column_by_name(name)
                    .cloned()
                    .ok_or_else(|| DiffError::Parquet {
                        path: display(path),
                        why: format!("column `{name}` is missing from results.parquet"),
                    })
            };
            let mp_key = col("mp_key")?;
            let mp_key = mp_key.as_string::<i32>();
            let mp_row = col("mp_row")?;
            let mp_row = mp_row.as_primitive::<UInt32Type>();
            let component = col("component")?;
            let component = component.as_dictionary::<Int32Type>();
            let component_values = component.values().as_string::<i32>();
            let _stage = col("stage")?;
            let _stage = _stage.as_primitive::<Int8Type>();
            let t = col("t")?;
            let t = t.as_primitive::<Int32Type>();
            let value = col("value")?;
            let value = value.as_primitive::<Float64Type>();
            let value_i = col("value_i")?;
            let value_i = value_i.as_primitive::<Int64Type>();
            let value_b = col("value_b")?;
            let value_b = value_b.as_boolean();
            let value_s = col("value_s")?;
            let value_s = value_s.as_dictionary::<Int32Type>();
            let value_s_values = value_s.values().as_string::<i32>();

            for row in 0..batch.num_rows() {
                let id = component_values.value(component.keys().value(row) as usize);
                let mp = mp_row.value(row);
                let at = t.value(row);
                let cell = if !value.is_null(row) {
                    Value::F64(value.value(row))
                } else if !value_i.is_null(row) {
                    Value::I64(value_i.value(row))
                } else if !value_b.is_null(row) {
                    Value::Bool(value_b.value(row))
                } else if !value_s.is_null(row) {
                    Value::Str(
                        value_s_values
                            .value(value_s.keys().value(row) as usize)
                            .to_string(),
                    )
                } else {
                    // Exactly one `value*` is non-null per row (§2). A row with none is a
                    // malformed result set, not a cell with an unknown value.
                    return Err(DiffError::Parquet {
                        path: display(path),
                        why: format!("row {row}: every value column is null for `{id}`"),
                    });
                };
                self.mp_keys
                    .entry(mp)
                    .or_insert_with(|| mp_key.value(row).to_string());
                self.components
                    .entry(id.to_string())
                    .or_default()
                    .insert((mp, at), cell);
                self.rows += 1;
            }
        }
        Ok(())
    }
}

fn read_manifest(dir: &Path) -> Result<Manifest, DiffError> {
    let path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|e| DiffError::NotARunDir {
        path: display(dir),
        why: format!("manifest.json: {e}"),
    })?;
    Manifest::from_json(&text).map_err(|e| DiffError::NotARunDir {
        path: display(dir),
        why: format!("manifest.json: {e}"),
    })
}

fn read_schema(dir: &Path) -> Result<ResultsSchemaDoc, DiffError> {
    let path = dir.join("results.schema.json");
    let text = std::fs::read_to_string(&path).map_err(|e| DiffError::NotARunDir {
        path: display(dir),
        why: format!("results.schema.json: {e}"),
    })?;
    let doc: ResultsSchemaDoc = serde_json::from_str(&text).map_err(|e| DiffError::NotARunDir {
        path: display(dir),
        why: format!("results.schema.json: {e}"),
    })?;
    doc.reindex().map_err(|e| DiffError::NotARunDir {
        path: display(dir),
        why: format!("results.schema.json: {e}"),
    })
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

/// The model files a run's manifest points at, when they still exist on disk.
///
/// Used for step 5 of §5.3 (attribute a divergence to a model change): a diff that can find both
/// models says *explained* or *unexplained*, and one that cannot says `null` rather than guessing.
/// A manifest records the path as the run saw it, which is usually relative to the project the run
/// was launched from — so the lookup tries the path as written and then walks up from the run
/// directory. A path that resolves nowhere is dropped: a diff with half a model would classify
/// from half a graph, which is worse than saying it had none.
pub fn model_files(side: &RunSide) -> Vec<PathBuf> {
    let run = PathBuf::from(&side.path);
    let mut roots: Vec<PathBuf> = vec![PathBuf::new()];
    let mut here = Some(run.as_path());
    for _ in 0..3 {
        match here {
            Some(dir) => {
                roots.push(dir.to_path_buf());
                here = dir.parent();
            }
            None => break,
        }
    }
    let assumptions = side
        .manifest
        .inputs
        .assumptions
        .as_ref()
        .map(|a| a.path.clone());
    let expected: BTreeMap<String, String> = side
        .manifest
        .inputs
        .model
        .files
        .iter()
        .map(|f| (f.path.clone(), f.digest.clone()))
        .chain(
            side.manifest
                .inputs
                .assumptions
                .as_ref()
                .map(|a| (a.path.clone(), a.digest.clone())),
        )
        .collect();
    side.manifest
        .inputs
        .model
        .files
        .iter()
        .map(|f| f.path.clone())
        // The assumption set travels with the model: two runs of the same modules under
        // different assumptions are a *model* difference, and a diff that read only the modules
        // would report every consequence of a changed rate as unexplained.
        .chain(assumptions)
        .map(|path| {
            let found = roots
                .iter()
                .map(|root| root.join(&path))
                .find(|p| p.is_file())?;
            // The file has to be the file that *ran*. A `.pir` that has been edited since — the
            // usual case when someone diffs a run against a rerun of the same working tree — is
            // not this run's model, and classifying from it would attribute one run's numbers to
            // another run's formulas.
            let digest = expected.get(&path)?;
            let bytes = std::fs::read(&found).ok()?;
            (*digest == format!("sha256:{}", crate::sha256_hex(&bytes))).then_some(found)
        })
        // All or nothing. Half a model is worse than none: it would classify some components from
        // the IR and silently guess at the rest, with nothing in the report to say which.
        .collect::<Option<Vec<PathBuf>>>()
        .unwrap_or_default()
}
