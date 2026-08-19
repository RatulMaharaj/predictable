//! Seeded-divergence fixtures built from the committed reference run.
#![allow(dead_code)] // each test binary uses a different subset of these helpers.

//!
//! Every test in this crate starts from `models/term_annual/runs/base/` — a real run of a real
//! reference model, with a golden committed beside it — and writes a *mutated copy* of it as a
//! second run directory. That is what makes the expected findings exact: the mutation is the only
//! difference, so the diff has exactly one right answer and the test can name it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_io::outbound::{
    Cell, Manifest, ModelpointRows, ResultsChunk, ResultsSchemaDoc, ResultsWriter, RunDir,
    WriterOptions,
};
use predictable_rundiff::{DiffOptions, RunSide, Value};

/// The repository root.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repo root")
        .to_path_buf()
}

/// The committed reference run.
pub fn base_dir() -> PathBuf {
    repo().join("models/term_annual/runs/base")
}

/// The reference model's `.pir` files, for the dependency graph and model attribution.
///
/// The assumption set travels with the modules: a hypothesis that recognises a ratio as
/// `(1 + expense_inflation)` can only do so if it knows what `expense_inflation` is.
pub fn model_files() -> Vec<PathBuf> {
    let model = repo().join("models/term_annual");
    [
        "build/model.pir",
        "build/product.pir",
        "build/schema.pir",
        "base.pir",
    ]
    .iter()
    .map(|f| model.join(f))
    .collect()
}

/// Options with the reference model on both sides, so `class` is decided by the IR.
pub fn options() -> DiffOptions {
    let files = model_files();
    DiffOptions::default().with_model_paths(Some(&files), Some(&files))
}

/// Options with a model on the `b` side only — the shape of a Prophet reconciliation, where
/// there is no second model to diff against.
pub fn options_b_only() -> DiffOptions {
    let files = model_files();
    DiffOptions::default().with_model_paths(None, Some(&files))
}

/// A scratch directory, unique per call: the tests run in parallel and two of them writing the
/// same path would make one test's failure depend on another test's timing.
pub fn scratch(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "predictable-rundiff-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The committed run, loaded.
pub fn base(label: &str) -> RunSide {
    RunSide::load(label, base_dir()).expect("the committed reference run loads")
}

/// A run directory identical to the base run except for what `mutate` changes.
///
/// `mutate` sees `(component_id, mp_row, t, value)` and returns the value to write. Returning
/// `None` drops the cell, which is how the "only in one side" cases are seeded.
pub fn mutated(
    tag: &str,
    label: &str,
    mutate: impl Fn(&str, u32, i32, f64) -> Option<f64>,
) -> RunSide {
    let source = base("source");
    let dir = scratch(tag).join("run");
    write_side(&dir, &source, &source.schema, &source.manifest, &mutate);
    RunSide::load(label, &dir).expect("the mutated run loads")
}

/// A run directory whose component set is a subset of the base run's — the `emit` case (Q6).
pub fn subset(tag: &str, label: &str, keep: &[&str]) -> RunSide {
    let source = base("source");
    let components: Vec<_> = source
        .schema
        .components
        .iter()
        .filter(|c| keep.contains(&c.id.as_str()))
        .cloned()
        .collect();
    let schema = ResultsSchemaDoc::new("list", components).expect("a subset schema");
    let dir = scratch(tag).join("run");
    write_side(&dir, &source, &schema, &source.manifest, &|_, _, _, v| {
        Some(v)
    });
    RunSide::load(label, &dir).expect("the subset run loads")
}

/// A run directory containing only the modelpoints whose key satisfies `keep`.
pub fn restricted(tag: &str, label: &str, keep: impl Fn(&str) -> bool) -> RunSide {
    let source = base("source");
    let dir = scratch(tag).join("run");
    let rows = rows(&source, &source.schema, &|_, _, _, v| Some(v));
    let rows = rows
        .into_iter()
        .filter(|(mp_row, _)| keep(source.mp_keys.get(mp_row).map(String::as_str).unwrap_or("")))
        .collect();
    write_rows(&dir, &source, &source.schema, &source.manifest, rows);
    RunSide::load(label, &dir).expect("the restricted run loads")
}

type Rows = BTreeMap<u32, Vec<Cell>>;

fn rows(
    source: &RunSide,
    schema: &ResultsSchemaDoc,
    mutate: &impl Fn(&str, u32, i32, f64) -> Option<f64>,
) -> Rows {
    let mut out: Rows = BTreeMap::new();
    for (id, cells) in &source.components {
        if schema.get(id).is_none() {
            continue;
        }
        for ((mp_row, t), value) in cells {
            let cell = match value {
                Value::F64(v) => match mutate(id, *mp_row, *t, *v) {
                    Some(v) => Cell::f64(id.clone(), *t, v),
                    None => continue,
                },
                Value::I64(v) => Cell::i64(id.clone(), *t, *v),
                Value::Bool(v) => Cell::bool(id.clone(), *t, *v),
                Value::Str(v) => Cell::str(id.clone(), *t, v.clone()),
            };
            out.entry(*mp_row).or_default().push(cell);
        }
    }
    out
}

fn write_side(
    dir: &Path,
    source: &RunSide,
    schema: &ResultsSchemaDoc,
    manifest: &Manifest,
    mutate: &impl Fn(&str, u32, i32, f64) -> Option<f64>,
) {
    let rows = rows(source, schema, mutate);
    write_rows(dir, source, schema, manifest, rows);
}

fn write_rows(
    dir: &Path,
    source: &RunSide,
    schema: &ResultsSchemaDoc,
    manifest: &Manifest,
    rows: Rows,
) {
    let run = RunDir::create(dir).expect("a scratch run directory");
    let mut writer: ResultsWriter<std::fs::File> = run
        .results_writer(schema.clone(), WriterOptions::default())
        .expect("a results writer");
    let mut chunk = ResultsChunk::new(0);
    for (offset, (mp_row, cells)) in rows.into_iter().enumerate() {
        chunk.push(ModelpointRows {
            offset: offset as u32,
            mp_key: source.mp_keys[&mp_row].clone(),
            mp_row,
            cells,
        });
    }
    writer.write_chunk(&chunk).expect("the chunk writes");
    let summary = writer.finish().expect("the writer closes");

    let mut manifest = manifest.clone();
    manifest.results.rows = summary.rows;
    manifest.results.components = summary.components;
    manifest.results.component_set_digest = summary.component_set_digest.clone();
    manifest.results.digest = summary.digest;
    manifest.run_config.emit = match schema.emit.as_str() {
        "list" => predictable_ir::run::Emit::List,
        "all" => predictable_ir::run::Emit::All,
        _ => predictable_ir::run::Emit::Outputs,
    };
    run.write_manifest(manifest).expect("the manifest writes");
}
