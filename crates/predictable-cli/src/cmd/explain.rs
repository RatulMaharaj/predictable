//! `predictable explain <run/> --component c --mp KEY [--t 3]` — `04-verify.md` §3.
//!
//! One modelpoint, replayed. The command reads the run directory's manifest to
//! find the `[run]` file that produced it, rebuilds the model exactly as the run
//! did — same sources, same assumptions, same tables, same `[[solve]]` outcome —
//! and then projects **one** modelpoint through a fresh single-lane engine.
//!
//! Two deliberate differences from `run`, neither of which can change a number:
//!
//! * the plan is built with full retention, because a ring buffer has already
//!   thrown away the history a trace at `t` asks about (§4.3);
//! * `on_trap` is `continue`, because a trapping modelpoint is precisely the one
//!   worth explaining and aborting would print nothing.
//!
//! The JSON is the normative artefact; the terminal rendering is a projection of
//! it (§3.5), so `--json` and the default output can never disagree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_engine::explain::{ExplainError, ExplainOptions, Explainer, Trace, TraceContext};
use predictable_io::outbound::Manifest;
use predictable_io::MpSchema;
use predictable_ir::run::RunFile;
use predictable_plan::{OptLevel, PlanOptions};
use predictable_runner::executor::SerialExecutor;
use predictable_runner::pipeline::{RunInputs, Runner};
use predictable_runner::{load_chunks, CancelFlag, Chunk};
use predictable_tables::{FsResolver, LoadOptions};

use crate::args::Args;
use crate::emit::{Emitter, DOMAIN, OK, USAGE};
use crate::load::{compile, Sources};
use crate::paths;

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &[
    "json",
    "out-json",
    "component",
    "mp",
    "t",
    "depth",
    "expand",
    "values-only",
    "trace-max-terms",
    "width",
    "model",
    "assumptions",
    "modelpoints",
];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &[
    "out-json",
    "component",
    "mp",
    "t",
    "depth",
    "expand",
    "trace-max-terms",
    "width",
    "model",
    "assumptions",
    "modelpoints",
];

/// Run `explain`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let target =
        PathBuf::from(args.one_positional("path to a run directory or a [run] .pir file")?);
    let component = args
        .opt("component")
        .ok_or("`--component <name>` says which cell to explain")?
        .to_string();
    let mp = args
        .opt("mp")
        .ok_or("`--mp <key>` says which modelpoint to replay")?
        .to_string();
    let t = args.opt_u32("t")?;

    let (run_path, run_digest) = resolve_run(&target)?;
    let base = run_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // ---- the [run] file --------------------------------------------------
    let run_source = paths::read(&run_path)?;
    let run_display = run_path.display().to_string();
    let mut syn = predictable_syntax::SourceMap::new();
    let parsed = predictable_syntax::parse(&mut syn, run_display.clone(), run_source);
    if parsed.has_errors() {
        return Err(format!("{run_display}: the [run] file does not parse"));
    }
    let run_file: RunFile =
        predictable_runner::config::run_file(&parsed.document).map_err(|e| e.to_string())?;

    // ---- the model, planned for replay -----------------------------------
    let model_paths = crate::cmd::run::model_paths(args, &base, &run_file)?;
    let sources = Sources::load(&model_paths)?;
    let check = sources.check();
    emitter.diagnostics(&check.diagnostics, &check.sources);
    if !check.is_ok() {
        emitter.note("explain refused: the model does not check");
        return Ok(USAGE);
    }
    let options = PlanOptions {
        opt: OptLevel::O1,
        // A trace at `t` reads history a ring buffer has already overwritten.
        retain_all: true,
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
    let mut assumption_set = None;
    let mut assumption_file = None;
    if let Some(path) = &assumption_path {
        let text = paths::read(path)?;
        let name = path.display().to_string();
        let mut s = predictable_syntax::SourceMap::new();
        let doc = predictable_syntax::parse(&mut s, name.clone(), text);
        assumptions = predictable_runner::config::assumption_values(&doc.document);
        assumption_set = Some(
            doc.document
                .assumption_set
                .clone()
                .unwrap_or_else(|| "base".to_string()),
        );
        assumption_file = Some(name);
    }

    // ---- tables ----------------------------------------------------------
    let load_options = LoadOptions {
        allow_table_drift: run_file.run.allow_table_drift,
    };
    let resolver = FsResolver::new();
    let mut tables = Vec::new();
    for module in &compiled.modules {
        for decl in &module.tables {
            tables.push(
                predictable_tables::load(decl, &base, &resolver, &load_options)
                    .map_err(|e| format!("table `{}`: {e}", decl.name))?,
            );
        }
    }

    // ---- modelpoints -----------------------------------------------------
    let mp_path = match args.opt("modelpoints") {
        Some(p) => PathBuf::from(p),
        None => paths::resolve_relative(&base, &run_file.run.modelpoints),
    };
    let merged = crate::cmd::run::merge_modules(&compiled.modules);
    let schema = MpSchema::from_module(&merged).map_err(|e| e.to_string())?;
    let key_field = schema.key_field().name.clone();
    let mut source = crate::cmd::run::open_modelpoints(schema, &mp_path)?;
    let chunk_size = run_file.run.exec.chunk_size.max(1) as usize;
    let mut chunks =
        load_chunks(source.as_mut(), &key_field, chunk_size).map_err(|e| e.to_string())?;

    // A `[[solve]]` model's modelpoints are not the ones on disk: the solver
    // wrote the solved `vary` value back into the columns. Replaying the raw
    // file would explain a projection nobody ran, so the solves run first.
    if !run_file.solves.is_empty() {
        let mut runner = Runner::new(RunInputs {
            modules: &compiled.modules,
            plan: &compiled.plan,
            tapes: &compiled.tapes,
            tables: tables.clone(),
            assumptions: assumptions.clone(),
            run: &run_file,
        })
        .map_err(|e| e.to_string())?;
        let cancel = CancelFlag::new();
        for solve in runner.solves().to_vec() {
            predictable_runner::solver::solve(
                &mut runner,
                &solve,
                &mut chunks,
                &SerialExecutor,
                &cancel,
            )
            .map_err(|e| e.to_string())?;
        }
    }

    let Some(single) = single_lane(&chunks, &mp) else {
        return Err(format!(
            "no modelpoint `{mp}` in {} ({} row(s) loaded)",
            mp_path.display(),
            chunks.iter().map(Chunk::len).sum::<usize>()
        ));
    };

    // ---- replay ----------------------------------------------------------
    let timeline = compiled
        .modules
        .iter()
        .find_map(|m| m.timeline.clone())
        .ok_or("the model declares no [timeline]")?;
    let mut ctx = TraceContext::from_modules(&compiled.modules);
    ctx.run = run_digest;
    ctx.modelpoint_file = Some(mp_path.display().to_string());
    ctx.assumption_set = assumption_set;
    ctx.assumption_file = assumption_file;

    let mut explainer = Explainer::new(&compiled.plan, &compiled.tapes, tables, &timeline, ctx)
        .map_err(|e| e.to_string())?;
    explainer.prepare(&assumptions).map_err(|e| e.to_string())?;
    let input = single.bind(explainer.engine_mut());
    explainer.load(&input).map_err(|e| e.to_string())?;

    let trace_options = ExplainOptions {
        depth: args.opt("depth").map(parse_depth).transpose()?.unwrap_or(2),
        expand: args
            .opt("expand")
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
            .unwrap_or_default(),
        values_only: args.flag("values-only"),
        max_terms: args
            .opt_u32("trace-max-terms")?
            .map(|n| n as usize)
            .unwrap_or(predictable_engine::DEFAULT_MAX_TERMS),
        ..ExplainOptions::default()
    };

    // A `[[solve]]`'s solved value is emitted as a `PerMP` component under the
    // solve's own name (`01-ir.md` §8.4.4), but the model declares no such
    // component: it *is* the `vary` field, after solving. Explaining it by that
    // name is what a reader will try, so it resolves rather than 404s.
    let component = match run_file.solves.iter().find(|s| s.name == component) {
        Some(solve) => solve.vary.clone(),
        None => component,
    };

    match explainer.explain(&component, t, &trace_options) {
        Ok(trace) => {
            report(emitter, &trace, args)?;
            Ok(OK)
        }
        // `E0901` is an engine bug, and it is reported as one rather than
        // silently rendered: the trace is still emitted so the divergence can be
        // read, but the exit code says the run and the replay disagree.
        Err(ExplainError::ReplayDiverged(d)) => {
            report(emitter, &d.trace, args)?;
            emitter.diagnostics(std::slice::from_ref(&d.diagnostic), &Default::default());
            emitter.note(format!(
                "E0901: `{}` replayed to {} but the run stored {}",
                d.component, d.replayed, d.recorded
            ));
            Ok(DOMAIN)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn report(emitter: &mut Emitter, trace: &Trace, args: &Args) -> Result<(), String> {
    emitter.document(
        "explain",
        serde_json::to_value(trace).map_err(|e| e.to_string())?,
    )?;
    if !emitter.json {
        let width = args
            .opt_u32("width")?
            .map(|w| w as usize)
            .unwrap_or(predictable_engine::explain_text::DEFAULT_WIDTH);
        emitter.line(predictable_engine::explain_text::render_with_width(trace, width).trim_end());
    }
    Ok(())
}

/// `--depth -1` means "all the way down"; anything else is a level count.
fn parse_depth(text: &str) -> Result<i32, String> {
    text.parse::<i32>()
        .map_err(|_| format!("--depth expects an integer (or -1 for the whole tree), got `{text}`"))
}

/// A run directory (with a manifest) or a `[run]` file, resolved to the `[run]`
/// file plus the manifest digest of the run being explained.
fn resolve_run(target: &Path) -> Result<(PathBuf, String), String> {
    let manifest_path = if target.is_dir() {
        target.join("manifest.json")
    } else if target.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
        target.to_path_buf()
    } else {
        // A bare `[run]` file: there is no manifest, so the trace's `run` field
        // is empty rather than invented.
        return Ok((target.to_path_buf(), String::new()));
    };
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest = Manifest::from_json(&text).map_err(|e| e.to_string())?;
    // The manifest records the `[run]` file as it was named on the command
    // line, which is usually relative to the directory the run was launched
    // from. Look for it there, then beside the run directory, then one level up
    // — and say where we looked rather than reporting "no such file".
    let recorded = PathBuf::from(&manifest.run_config.path);
    let run_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut candidates = vec![recorded.clone()];
    for base in [
        Some(run_dir.clone()),
        run_dir.parent().map(Path::to_path_buf),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(base.join(&recorded));
        candidates.push(base.join(recorded.file_name().unwrap_or_default()));
    }
    let found = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "the run's `{}` was not found; looked in {}",
                recorded.display(),
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?
        .clone();
    Ok((found, manifest.manifest_digest.clone()))
}

/// The one-lane chunk holding `key`, keeping the modelpoint's real file row so
/// the trace's `Input` leaves point at the row a reviewer can open.
fn single_lane(chunks: &[Chunk], key: &str) -> Option<Chunk> {
    for chunk in chunks {
        let Some(lane) = chunk.keys.iter().position(|k| k == key) else {
            continue;
        };
        let mut columns = BTreeMap::new();
        for (name, column) in &chunk.columns {
            let one = match column {
                predictable_runner::ChunkColumn::Num(v) => {
                    predictable_runner::ChunkColumn::Num(vec![v[lane]])
                }
                predictable_runner::ChunkColumn::Text(v) => {
                    predictable_runner::ChunkColumn::Text(vec![v[lane].clone()])
                }
            };
            columns.insert(name.clone(), one);
        }
        return Some(Chunk {
            index: chunk.index,
            first_row: chunk.first_row + lane as u64,
            keys: vec![key.to_string()],
            columns,
        });
    }
    None
}
