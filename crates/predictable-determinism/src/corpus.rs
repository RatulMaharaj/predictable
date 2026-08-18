//! Loading a conformance case the way a real run loads it.
//!
//! The corpus (`conformance/valid/**`) was written as a *checker* fixture: every case has
//! `.pir` sources, and only some have modelpoint data. A determinism corpus needs numbers, so
//! this module supplies the missing halves in the least surprising way it can:
//!
//! * modelpoints come from the case's own `data/` file when it has one, and otherwise from
//!   [`synthetic_csv`] — a deterministic CSV derived from the declared schema and nothing else,
//!   so it is a pure function of the corpus and regenerates identically anywhere;
//! * tables come from the case directory through the ordinary [`predictable_tables::FsResolver`],
//!   digest check included. A case whose table needs a host-supplied resource (`resource:`)
//!   cannot be loaded here and is reported as a skip rather than silently dropped.
//!
//! Everything after loading is the real pipeline: [`predictable_plan::plan_sources`],
//! [`predictable_tape::lower_plan`], [`predictable_runner::pipeline::Runner`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_check::Input;
use predictable_ir::run::RunFile;
use predictable_ir::{DType, Module, Unit};
use predictable_plan::{plan_sources, Plan, PlanOptions};
use predictable_runner::chunk::{load_chunks, Chunk};
use predictable_runner::executor::{ChunkExecutor, LocalExecutor, SerialExecutor};
use predictable_runner::pipeline::{Projection, RunInputs, Runner};
use predictable_runner::CancelFlag;
use predictable_tape::{lower_plan, TapeProgram};

/// Rows of synthetic modelpoint data. Small enough that `C = 1` is cheap, large enough that a
/// chunk of 2 straddles a boundary and a chunk of 1024 does not.
pub const SYNTHETIC_ROWS: usize = 4;

/// One conformance case, as files on disk.
#[derive(Debug, Clone)]
pub struct Case {
    /// Directory name, e.g. `v02-term-assurance`.
    pub name: String,
    /// Absolute path of the case directory.
    pub dir: PathBuf,
    /// Every `.pir` in the directory, in path order.
    pub inputs: Vec<Input>,
}

/// A case, planned, lowered and bound — everything a projection needs.
#[derive(Debug)]
pub struct Loaded {
    /// The case this came from.
    pub name: String,
    /// The planner's output, including `order_digest` and `plan_digest`.
    pub plan: Plan,
    /// The lowered tapes.
    pub tapes: TapeProgram,
    /// The checked modules.
    pub modules: Vec<Module>,
    /// Resolved, digest-checked, compiled tables.
    pub tables: Vec<predictable_tables::CompiledTable>,
    /// The `[run]` file, or the minimal one when the case has none.
    pub run: RunFile,
    /// Assumption values by name.
    pub assumptions: BTreeMap<String, f64>,
    /// `model_digest` over the canonical text of every module.
    pub model_digest: String,
    /// `tape_digest` over the lowered program.
    pub tape_digest: String,
    /// The modelpoint CSV actually used, and where it came from.
    pub modelpoints: (String, String),
    /// The key field.
    pub key_field: String,
}

/// A finished projection, with the chunking that produced it.
#[derive(Debug)]
pub struct RunOutput {
    /// Chunk size used.
    pub chunk_size: usize,
    /// The chunks fed in.
    pub chunks: Vec<Chunk>,
    /// What came out.
    pub projection: Projection,
}

/// The three executors §7 clause 1 says must agree.
#[derive(Debug)]
pub struct Executors;

impl Executors {
    /// Serial, one-thread rayon, eight-thread rayon — named for failure messages.
    pub fn all() -> Vec<(&'static str, Box<dyn ChunkExecutor>)> {
        vec![
            ("serial", Box::new(SerialExecutor)),
            ("local(1)", Box::new(LocalExecutor::new(1))),
            ("local(8)", Box::new(LocalExecutor::new(8))),
        ]
    }
}

impl Case {
    /// A case from `.pir` source held in memory — the generated-program path.
    ///
    /// `dir` is the corpus root so relative resolution still has a real, sandboxed base; a
    /// generated program declares no tables and no modelpoint file, so nothing is read from it.
    pub fn from_source(name: impl Into<String>, source: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            dir: corpus_root(),
            inputs: vec![Input::new("gen.pir", source.into())],
        }
    }
}

/// The corpus root, `conformance/valid`.
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/valid")
        .canonicalize()
        .expect("the conformance corpus is checked in")
}

/// Every valid case, in directory order.
pub fn cases() -> Vec<Case> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(corpus_root())
        .expect("read corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .expect("read case")
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "pir"))
                .collect();
            files.sort();
            let inputs = files
                .into_iter()
                .map(|p| {
                    let text = std::fs::read_to_string(&p).expect("read .pir");
                    Input::new(p.file_name().unwrap().to_string_lossy().to_string(), text)
                })
                .collect();
            Case {
                name: dir.file_name().unwrap().to_string_lossy().to_string(),
                dir,
                inputs,
            }
        })
        .collect()
}

fn parse_doc(name: &str, text: &str) -> predictable_syntax::ast::PirDocument {
    let mut sources = predictable_syntax::SourceMap::new();
    predictable_syntax::parse(&mut sources, name.to_string(), text.to_string()).document
}

/// The union of every module's schema: fields, enums, tables and timeline in one place.
///
/// The corpus splits a model across files (`schema.pir` declares the modelpoint fields,
/// `model.pir` the components), and [`predictable_io::MpSchema::from_module`] takes one module.
/// Merging is a loading concern, not a semantic one — the checker has already agreed the parts
/// belong together.
fn merged_module(modules: &[Module]) -> Module {
    let mut out = Module::new("merged");
    for m in modules {
        if out.timeline.is_none() {
            out.timeline = m.timeline.clone();
        }
        out.enums.extend(m.enums.iter().cloned());
        out.modelpoint_fields
            .extend(m.modelpoint_fields.iter().cloned());
        out.tables.extend(m.tables.iter().cloned());
        out.components.extend(m.components.iter().cloned());
    }
    out
}

/// A deterministic modelpoint CSV for a schema that has no data file.
///
/// The values are a pure function of the field's *declared* name, dtype and unit — never of the
/// host, the clock or an RNG — so the same corpus regenerates the same file on every machine.
/// Units carry the magnitude: a `prob` gets a probability, a `money` gets money. Nothing is
/// zero, because a zero in a denominator is a trap and a trap is not the thing under test here.
pub fn synthetic_csv(schema: &Module, rows: usize) -> String {
    let header: Vec<&str> = schema
        .modelpoint_fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let mut out = header.join(",");
    out.push('\n');
    for r in 0..rows {
        let cells: Vec<String> = schema
            .modelpoint_fields
            .iter()
            .map(|f| synthetic_cell(schema, &f.name, &f.dtype, &f.unit, f.key, r))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

/// A stand-in value for an assumption the case declares but never sets.
///
/// Same rule as [`synthetic_cell`]: the unit picks the magnitude, nothing is zero, and the
/// answer depends only on the declaration.
pub fn default_assumption(name: &str, unit: &Unit) -> f64 {
    let u = unit.to_string();
    if u.contains("prob") || u.contains("rate") {
        0.035
    } else if u.contains("factor") {
        1.0
    } else if u.contains("money") {
        50.0
    } else if name.contains("term") || name.contains("year") {
        20.0
    } else {
        1.25
    }
}

fn synthetic_cell(
    schema: &Module,
    name: &str,
    dtype: &DType,
    unit: &Unit,
    key: bool,
    row: usize,
) -> String {
    let u = unit.to_string();
    match dtype {
        DType::Str if key => format!("MP{:05}", row + 1),
        DType::Str => format!("GRP{}", row % 2 + 1),
        DType::Bool => if row % 2 == 0 { "true" } else { "false" }.to_string(),
        DType::Date => "2026-06-30".to_string(),
        DType::Enum(e) => schema
            .enums
            .iter()
            .find(|d| &d.name == e)
            .and_then(|d| d.values.get(row % d.values.len().max(1)))
            .cloned()
            .unwrap_or_else(|| "M".to_string()),
        DType::I64 => {
            // `years` reads as an age or a term; anything else is a small positive count.
            let base = if u.contains("year") { 40 } else { 10 };
            format!("{}", base + row as i64)
        }
        DType::F64 => {
            let v = if u.contains("prob") || u.contains("rate") {
                0.01 + 0.002 * row as f64
            } else if u.contains("factor") {
                1.0 + 0.05 * row as f64
            } else if u.contains("money") {
                100_000.0 + 12_500.0 * row as f64
            } else if u.contains("count") || u.contains("year") {
                1.0 + row as f64
            } else if name.contains("term") {
                20.0
            } else {
                1.5 + 0.25 * row as f64
            };
            format!("{v:?}")
        }
    }
}

/// Plan, lower, resolve tables and pick up modelpoints for one case.
///
/// `Err` is a *skip with a reason*, not a panic: a corpus case that cannot be executed (a table
/// behind a `resource:` scheme, a model with no modelpoint schema) is a fact about the corpus
/// worth recording in a golden, and the caller records it.
pub fn load(case: &Case) -> Result<Loaded, String> {
    let options = PlanOptions {
        retain_all: true,
        ..PlanOptions::default()
    };
    let plan = plan_sources(&case.inputs, &options).map_err(|e| format!("plan: {e}"))?;
    let tapes = lower_plan(&plan).map_err(|e| format!("lower: {e}"))?;
    let modules: Vec<Module> = predictable_plan::lower_modules(&case.inputs)
        .into_iter()
        .filter(|m| !m.module.is_empty())
        .collect();
    if modules.is_empty() {
        return Err("no modules".to_string());
    }
    let schema_module = merged_module(&modules);
    if schema_module.modelpoint_fields.is_empty() {
        return Err("no modelpoint_field declarations: nothing to project".to_string());
    }
    let key_field = schema_module
        .modelpoint_fields
        .iter()
        .find(|f| f.key)
        .map(|f| f.name.clone())
        .ok_or_else(|| "no key field".to_string())?;

    // The `[run]` file if the case has one; the minimal projection otherwise.
    let run_src = case
        .inputs
        .iter()
        .find(|i| i.name == "run.pir")
        .map(|i| (i.name.clone(), i.text.clone()));
    let mut run = match &run_src {
        Some((path, text)) => predictable_runner::config::run_file(&parse_doc(path, text))
            .map_err(|e| format!("run file: {e}"))?,
        None => predictable_runner::minimal_run_file(),
    };
    // §7 clause 1 is a claim about the *pipeline*, so the harness owns the chunk size and the
    // executor; the case owns everything else about the run.
    run.run.exec.threads = None;

    let mut assumptions = case
        .inputs
        .iter()
        .find(|i| i.text.contains("assumption_set"))
        .map(|i| predictable_runner::config::assumption_values(&parse_doc(&i.name, &i.text)))
        .unwrap_or_default();
    // A case with declared assumptions but no assumption set is a checker fixture, not a run.
    // Filling the gap from the declaration's unit — deterministically, as with modelpoints —
    // makes it executable without asking the corpus to grow a file it was never meant to have.
    for module in &modules {
        for decl in &module.assumptions {
            assumptions
                .entry(decl.name.clone())
                .or_insert_with(|| default_assumption(&decl.name, &decl.unit));
        }
    }

    // The dispatching resolver, rooted at the case directory: one model may mix `file:` and
    // `inline` tables, and both are ordinary corpus content.
    let resolver = predictable_tables::SchemeResolver {
        fs: predictable_tables::FsResolver::rooted_at(&case.dir),
        ..predictable_tables::SchemeResolver::new()
    };
    let tables = predictable_tables::load_all(
        &schema_module.tables,
        &case.dir,
        &resolver,
        // Drift is *allowed and then reported* rather than fatal: `v04-expressions` declares an
        // inline table whose digest the corpus and `predictable-tables` do not agree on, and
        // losing every builtin in the corpus to that disagreement would be the worse trade. The
        // golden prints `drifted=true`, so the disagreement stays review-visible.
        &predictable_tables::LoadOptions {
            allow_table_drift: true,
        },
    )
    .map_err(|e| format!("tables: {e}"))?;

    let model_digest = predictable_fmt::digest::model_digest_of_sources(
        case.inputs
            .iter()
            .map(|i| (i.name.as_str(), i.text.as_str())),
    )
    .map_err(|e| format!("model digest: {e}"))?;
    let tape_digest = predictable_tape::digest::tape_digest(&tapes);

    let declared = run.run.modelpoints.clone();
    let path = case.dir.join(&declared);
    let modelpoints = if !declared.is_empty() && path.is_file() {
        (
            declared,
            std::fs::read_to_string(&path).map_err(|e| format!("modelpoints: {e}"))?,
        )
    } else {
        (
            "<synthetic>".to_string(),
            synthetic_csv(&schema_module, SYNTHETIC_ROWS),
        )
    };

    Ok(Loaded {
        name: case.name.clone(),
        plan,
        tapes,
        modules,
        tables,
        run,
        assumptions,
        model_digest,
        tape_digest,
        modelpoints,
        key_field,
    })
}

/// Load the modelpoint CSV into chunks of `chunk_size`.
pub fn chunks(loaded: &Loaded, chunk_size: usize) -> Result<Vec<Chunk>, String> {
    let schema_module = merged_module(&loaded.modules);
    let schema = predictable_io::MpSchema::from_module(&schema_module)
        .map_err(|e| format!("schema: {e}"))?;
    let bytes = loaded.modelpoints.1.clone().into_bytes();
    let mut source = predictable_io::CsvSource::from_reader(
        schema,
        Box::new(std::io::Cursor::new(bytes)),
        loaded.modelpoints.0.clone(),
    )
    .map_err(|e| format!("csv: {e}"))?;
    let mut chunks = load_chunks(&mut source, &loaded.key_field, chunk_size)
        .map_err(|e| format!("chunks: {e}"))?;
    for chunk in &mut chunks {
        restore_enum_text(&schema_module, chunk);
    }
    Ok(chunks)
}

/// Put `enum` columns back as text before the engine sees them.
///
/// **This is a workaround for a live cross-crate defect, not a design choice.**
/// `predictable_io` decodes an `enum` column to its *ordinal* (`M` → 0, `F` → 1) and
/// `predictable_runner::chunk::lane_of` passes that ordinal through as a numeric lane, while
/// `predictable_tables` keeps an `enum` table key as text for the executing engine's dictionary
/// to intern. The two codings are unrelated, so an `enum`-keyed lookup misses every row: with
/// this workaround removed, `v02-term-assurance` — the corpus's only `runtime = true` case —
/// aborts at t = 0 with two `LookupMiss` traps and projects nothing.
///
/// Reverting the ordinal to the declared spelling puts the value back on the one path both sides
/// agree on: the engine's own dictionary. Delete this the moment the `enum` representation is
/// settled between io, runner and engine; the goldens will move when you do, which is the point.
fn restore_enum_text(schema: &Module, chunk: &mut Chunk) {
    for field in &schema.modelpoint_fields {
        let DType::Enum(enum_name) = &field.dtype else {
            continue;
        };
        let Some(decl) = schema.enums.iter().find(|d| &d.name == enum_name) else {
            continue;
        };
        if let Some(predictable_runner::chunk::ChunkColumn::Num(codes)) =
            chunk.columns.get(&field.name)
        {
            let text = codes
                .iter()
                .map(|c| {
                    decl.values
                        .get(*c as usize)
                        .cloned()
                        .unwrap_or_else(|| c.to_string())
                })
                .collect();
            chunk.columns.insert(
                field.name.clone(),
                predictable_runner::chunk::ChunkColumn::Text(text),
            );
        }
    }
}

/// Project a loaded case at a given chunk size on a given executor.
pub fn project(
    loaded: &Loaded,
    chunk_size: usize,
    executor: &dyn ChunkExecutor,
) -> Result<RunOutput, String> {
    let chunks = chunks(loaded, chunk_size)?;
    let mut run = loaded.run.clone();
    run.run.exec.chunk_size = chunk_size as u32;
    let runner = Runner::new(RunInputs {
        modules: &loaded.modules,
        plan: &loaded.plan,
        tapes: &loaded.tapes,
        tables: loaded.tables.clone(),
        assumptions: loaded.assumptions.clone(),
        run: &run,
    })
    .map_err(|e| format!("bind: {e}"))?;
    let projection = runner
        .project(&chunks, executor, &CancelFlag::new())
        .map_err(|e| format!("project: {e}"))?;
    Ok(RunOutput {
        chunk_size,
        chunks,
        projection,
    })
}
