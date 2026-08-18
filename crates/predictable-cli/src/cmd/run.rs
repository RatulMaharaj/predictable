//! `predictable run <run.pir> [--out dir]` — the whole projection, end to end.
//!
//! This is the only command that touches every crate at once, and its job is
//! almost entirely *assembly*: read the `[run]` file, resolve the model, the
//! assumption set, the tables and the modelpoints relative to it, check, plan,
//! lower, load, execute, and write a run directory whose manifest re-derives its
//! own digest.
//!
//! Two things it deliberately does not do. It never computes a number — every
//! value comes out of the kernel through `predictable-runner`. And it never
//! invents provenance: a field it cannot know (the git sha, the DSL version, the
//! table copies) is stamped absent rather than guessed, because `04-verify.md` §6
//! makes the manifest evidence and evidence may not contain filler.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use predictable_io::outbound::manifest::{
    AssumptionInputs, ModelInputs, ModelpointInputs, TableInput,
};
use predictable_io::outbound::{Inputs, Lineage, Outcome, Provenance, Versions};
use predictable_io::{CsvSource, ModelpointSource, MpSchema, ParquetSource};
use predictable_ir::run::RunFile;
use predictable_ir::Module;
use predictable_plan::{OptLevel, PlanOptions};
use predictable_runner::executor::{ChunkExecutor, LocalExecutor, SerialExecutor};
use predictable_runner::pipeline::{RunInputs, Runner};
use predictable_runner::run::RunSpec;
use predictable_runner::{load_chunks, CancelFlag};
use predictable_tables::{FsResolver, LoadOptions};
use serde_json::json;

use crate::args::Args;
use crate::clock;
use crate::emit::{diagnostic_summary, Emitter, DOMAIN, OK, USAGE};
use crate::load::{compile, Sources};
use crate::paths;

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &[
    "json",
    "out-json",
    "out",
    "model",
    "assumptions",
    "modelpoints",
    "threads",
    "chunk-size",
    "O0",
    "retain-all",
    "allow-table-drift",
    "run-id",
];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &[
    "out-json",
    "out",
    "model",
    "assumptions",
    "modelpoints",
    "threads",
    "chunk-size",
    "run-id",
];

/// Run `run`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let started = Instant::now();
    let run_path = PathBuf::from(args.one_positional("path to a [run] .pir file")?);
    let base = run_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let run_source = paths::read(&run_path)?;
    let run_display = run_path.display().to_string();

    // ---- the [run] file --------------------------------------------------
    let mut syn = predictable_syntax::SourceMap::new();
    let parsed = predictable_syntax::parse(&mut syn, run_display.clone(), run_source.clone());
    if parsed.has_errors() {
        return Err(format!("{run_display}: the [run] file does not parse"));
    }
    let mut run_file: RunFile =
        predictable_runner::config::run_file(&parsed.document).map_err(|e| e.to_string())?;
    // `--chunk-size` / `--threads` override the run file's `[run.exec]`. They are
    // scheduling knobs, outside `run_digest` (IR §8.4.2), so overriding them
    // cannot change a number — but the *runner* sizes its arenas from the same
    // field the loader chunks by, so both must see one value, not two.
    if let Some(chunk) = args.opt_u32("chunk-size")? {
        run_file.run.exec.chunk_size = chunk.max(1);
    }
    if let Some(threads) = args.opt_u32("threads")? {
        run_file.run.exec.threads = Some(threads.max(1));
    }

    // ---- the model -------------------------------------------------------
    let model_paths = model_paths(args, &base, &run_file)?;
    let sources = Sources::load(&model_paths)?;
    let check = sources.check();
    emitter.diagnostics(&check.diagnostics, &check.sources);
    if !check.is_ok() {
        emitter.document(
            "run",
            json!({
                "status": "not_checked",
                "run_file": run_display,
                "summary": diagnostic_summary(&check.diagnostics),
                "diagnostics": check.diagnostics,
            }),
        )?;
        emitter.note("run refused: the model does not check");
        return Ok(USAGE);
    }

    let options = PlanOptions {
        opt: if args.flag("O0") {
            OptLevel::O0
        } else {
            OptLevel::O1
        },
        retain_all: args.flag("retain-all"),
        program_digest: String::new(),
        periods: None,
    };
    let compiled = compile(&sources, &options)?;

    // ---- assumptions -----------------------------------------------------
    let assumption_path = args.opt("assumptions").map(PathBuf::from).or_else(|| {
        run_file
            .run
            .assumptions
            .as_ref()
            .map(|a| paths::resolve_relative(&base, a))
    });
    let mut assumptions = BTreeMap::new();
    let mut assumption_inputs = None;
    if let Some(path) = &assumption_path {
        let text = paths::read(path)?;
        let name = path.display().to_string();
        let mut s = predictable_syntax::SourceMap::new();
        let doc = predictable_syntax::parse(&mut s, name.clone(), text.clone());
        assumptions = predictable_runner::config::assumption_values(&doc.document);
        assumption_inputs = Some(AssumptionInputs {
            set: doc
                .document
                .assumption_set
                .clone()
                .unwrap_or_else(|| "base".to_string()),
            path: name.clone(),
            digest: predictable_fmt::digest::assumption_digest(&name, &text)
                .map_err(|e| e.to_string())?,
            values_digest: None,
        });
    }

    // ---- tables ----------------------------------------------------------
    let mut table_decls = Vec::new();
    for module in &compiled.modules {
        table_decls.extend(module.tables.iter().cloned());
    }
    let load_options = LoadOptions {
        allow_table_drift: args.flag("allow-table-drift") || run_file.run.allow_table_drift,
    };
    let resolver = FsResolver::new();
    let mut tables = Vec::new();
    let mut table_inputs: Vec<TableInput> = Vec::new();
    for decl in &table_decls {
        let compiled_table = predictable_tables::load(decl, &base, &resolver, &load_options)
            .map_err(|e| format!("table `{}`: {e}", decl.name))?;
        let mut input = TableInput::new(
            decl.name.clone(),
            decl.source.to_string(),
            compiled_table.digest.clone(),
        );
        input.declared_digest = decl.digest.clone();
        input.recompute_drift();
        table_inputs.push(input);
        tables.push(compiled_table);
    }
    table_inputs.sort_by(|a, b| a.name.cmp(&b.name));

    // ---- modelpoints -----------------------------------------------------
    let mp_path = match args.opt("modelpoints") {
        Some(p) => PathBuf::from(p),
        None => {
            if run_file.run.modelpoints.is_empty() {
                return Err(
                    "the [run] file names no `modelpoints`, and none was given with --modelpoints"
                        .into(),
                );
            }
            paths::resolve_relative(&base, &run_file.run.modelpoints)
        }
    };
    let merged = merge_modules(&compiled.modules);
    let schema = MpSchema::from_module(&merged).map_err(|e| e.to_string())?;
    let key_field = schema.key_field().name.clone();
    let mut source = open_modelpoints(schema, &mp_path)?;

    let chunk_size = run_file.run.exec.chunk_size as usize;
    let chunks = load_chunks(source.as_mut(), &key_field, chunk_size).map_err(|e| e.to_string())?;
    let rows: u64 = chunks.iter().map(|c| c.len() as u64).sum();
    let mp_bytes = std::fs::read(&mp_path).map_err(|e| format!("{}: {e}", mp_path.display()))?;

    // ---- execute ---------------------------------------------------------
    let mut runner = Runner::new(RunInputs {
        modules: &compiled.modules,
        plan: &compiled.plan,
        tapes: &compiled.tapes,
        tables,
        assumptions,
        run: &run_file,
    })
    .map_err(|e| e.to_string())?;

    let threads = run_file.run.exec.threads.unwrap_or(1).max(1);
    let local;
    let executor: &dyn ChunkExecutor = if threads > 1 {
        local = LocalExecutor::new(threads as usize);
        &local
    } else {
        &SerialExecutor
    };

    let out_dir = match args.opt("out") {
        Some(o) => PathBuf::from(o),
        None => {
            if run_file.run.out.is_empty() {
                return Err(
                    "the [run] file names no `out` directory, and none was given with --out".into(),
                );
            }
            paths::resolve_relative(&base, &run_file.run.out)
        }
    };

    let started_at = clock::now_iso8601();
    let spec = RunSpec {
        run_id: args
            .opt("run-id")
            .map(str::to_string)
            .unwrap_or_else(|| format!("{started_at}-{}", &compiled.plan.digest[..8])),
        run_path: run_display.clone(),
        run_source: run_source.clone(),
        inputs: Inputs {
            model: ModelInputs {
                module: compiled
                    .modules
                    .first()
                    .map(|m| m.module.clone())
                    .unwrap_or_default(),
                product: Some(run_file.run.product.clone()).filter(|p| !p.is_empty()),
                digest: compiled.model_digest.clone(),
                files: sources
                    .file_digests()
                    .into_iter()
                    .map(
                        |(path, digest)| predictable_io::outbound::manifest::FileRef {
                            path,
                            digest,
                        },
                    )
                    .collect(),
            },
            assumptions: assumption_inputs,
            modelpoints: ModelpointInputs {
                path: mp_path.display().to_string(),
                digest: predictable_fmt::digest::digest_bytes(&mp_bytes),
                rows,
                key_field: key_field.clone(),
                source: None,
            },
            tables: table_inputs,
        },
        versions: Versions {
            ir_version: predictable_ir::FORMAT.to_string(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            engine_git_sha: None,
            engine_build_profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
            cli_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            dsl_version: None,
        },
        provenance: Provenance {
            invocation: invocation(),
            cwd_git: None,
            user: None,
        },
        lineage: Lineage::base(),
        // Table *content* copies (Q13) are not taken by this build: the digests
        // and the drift flag travel, and `copy` is stamped absent so a consumer
        // says "table content unavailable in this run" instead of rendering a
        // partial row.
        table_copies: Vec::new(),
        no_table_copy: true,
        started_at: started_at.clone(),
        finished_at: started_at.clone(),
        wall_ms: 0,
    };

    // Solves, then the final (solved) projection, then the run directory —
    // `predictable_runner::run::execute` in three steps rather than one, so that
    // `finished_at` and `wall_ms` are stamped *after* the numbers exist rather
    // than before them.
    let mut spec = spec;
    let mut chunks = chunks;
    let cancel = CancelFlag::new();
    let mut solves = Vec::new();
    for solve in runner.solves().to_vec() {
        let result =
            predictable_runner::solver::solve(&mut runner, &solve, &mut chunks, executor, &cancel)
                .map_err(|e| e.to_string())?;
        result.check(&solve).map_err(|e| e.to_string())?;
        solves.push(result);
    }
    let projection = runner
        .project(&chunks, executor, &cancel)
        .map_err(|e| e.to_string())?;
    spec.finished_at = clock::now_iso8601();
    spec.wall_ms = clock::elapsed_ms(started);
    let report =
        predictable_runner::run::write_run(&runner, &chunks, projection, solves, &spec, &out_dir)
            .map_err(|e| e.to_string())?;

    // ---- report ----------------------------------------------------------
    let results = report.results.as_ref();
    emitter.document(
        "run",
        json!({
            "status": "ran",
            "run_id": report.manifest.run_id,
            "run_file": run_display,
            "out": out_dir.display().to_string(),
            "outcome": report.outcome,
            "model_digest": compiled.model_digest,
            "manifest_digest": report.manifest.manifest_digest,
            "run_digest": report.manifest.run_config.digest,
            "modelpoints": {
                "path": mp_path.display().to_string(),
                "rows": rows,
                "projected": report.manifest.execution.modelpoints_projected,
                "trapped": report.manifest.execution.modelpoints_trapped,
            },
            "results": results.map(|r| json!({
                "rows": r.rows,
                "components": r.components,
                "component_set_digest": r.component_set_digest,
                "digest": r.digest,
            })),
            "traps": report.manifest.execution.traps,
            "solves": report.manifest.run_config.solves.iter().map(|s| json!({
                "name": s.name,
                "converged": s.converged,
                "not_converged": s.not_converged,
                "residual_max_abs": s.residual.max_abs,
            })).collect::<Vec<_>>(),
            "wall_ms": report.manifest.execution.wall_ms,
            "exit_code": report.exit_code,
        }),
    )?;

    if !emitter.json {
        emitter.line(format!("run_id        {}", report.manifest.run_id));
        emitter.line(format!("out           {}", out_dir.display()));
        emitter.line(format!("outcome       {:?}", report.outcome));
        emitter.line(format!(
            "modelpoints   {} projected, {} trapped, of {rows}",
            report.manifest.execution.modelpoints_projected,
            report.manifest.execution.modelpoints_trapped,
        ));
        match results {
            Some(r) => emitter.line(format!(
                "results       {} row(s), {} component(s)",
                r.rows, r.components
            )),
            None => emitter.line("results       none written (the run aborted)"),
        }
        emitter.line(format!("manifest      {}", report.manifest.manifest_digest));
    }

    Ok(match report.outcome {
        Outcome::Completed => OK,
        Outcome::CompletedWithTraps | Outcome::Cancelled => DOMAIN,
        Outcome::Aborted => USAGE,
    })
}

/// Where the model files are: `--model`, else `[run].product` resolved against
/// the run file's directory.
pub(crate) fn model_paths(
    args: &Args,
    base: &Path,
    run_file: &RunFile,
) -> Result<Vec<String>, String> {
    if let Some(model) = args.opt("model") {
        return Ok(vec![model.to_string()]);
    }
    if run_file.run.product.is_empty() {
        return Err("the [run] file names no `product`, and no --model was given".into());
    }
    let path = paths::resolve_relative(base, &run_file.run.product);
    let with_ext = path.with_extension("pir");
    if path.is_dir() || path.exists() {
        Ok(vec![path.display().to_string()])
    } else if with_ext.exists() {
        Ok(vec![with_ext.display().to_string()])
    } else {
        Err(format!(
            "{}: the model named by `product` does not exist",
            path.display()
        ))
    }
}

/// One module carrying every modelpoint field and every component, so the
/// modelpoint schema is built from the whole product rather than from whichever
/// file happened to declare the key.
pub(crate) fn merge_modules(modules: &[Module]) -> Module {
    let mut merged = Module::new(
        modules
            .first()
            .map(|m| m.module.clone())
            .unwrap_or_else(|| "model".to_string()),
    );
    for module in modules {
        merged
            .modelpoint_fields
            .extend(module.modelpoint_fields.iter().cloned());
        merged.components.extend(module.components.iter().cloned());
        merged.enums.extend(module.enums.iter().cloned());
        if merged.timeline.is_none() {
            merged.timeline = module.timeline.clone();
        }
    }
    merged
}

pub(crate) fn open_modelpoints(
    schema: MpSchema,
    path: &Path,
) -> Result<Box<dyn ModelpointSource>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "csv" => Ok(Box::new(
            CsvSource::open(schema, path).map_err(|e| e.to_string())?,
        )),
        "parquet" => Ok(Box::new(
            ParquetSource::open(schema, path).map_err(|e| e.to_string())?,
        )),
        other => Err(format!(
            "{}: `{other}` is not a modelpoint format this build reads (csv, parquet)",
            path.display()
        )),
    }
}

fn invocation() -> String {
    let args: Vec<String> = std::env::args().collect();
    args.join(" ")
}
