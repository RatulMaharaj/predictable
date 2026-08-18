//! `predictable build <path>... [--out plan.json]` — check, plan, lower.
//!
//! `build` is the compile-only half of `run`: everything that depends on the model
//! and the plan options but on no data at all. It is what CI runs to prove a model
//! still compiles, and what pins `model_digest`, `plan_digest` and `order_digest`
//! before a single modelpoint is read.

use predictable_plan::{OptLevel, PlanOptions};
use serde_json::json;

use crate::args::Args;
use crate::emit::{diagnostic_summary, Emitter, OK, USAGE};
use crate::load::{compile, Sources};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["json", "out", "out-json", "O0", "retain-all", "chunk-size"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out", "out-json", "chunk-size"];

/// Run `build`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    if args.positional.is_empty() {
        return Err("build needs at least one .pir path".into());
    }
    let sources = Sources::load(&args.positional)?;
    let check = sources.check();
    emitter.diagnostics(&check.diagnostics, &check.sources);
    if !check.is_ok() {
        emitter.document(
            "build",
            json!({
                "status": "not_checked",
                "summary": diagnostic_summary(&check.diagnostics),
                "diagnostics": check.diagnostics,
            }),
        )?;
        emitter.note(format!(
            "build refused: {} error(s); nothing was planned",
            check.errors().count()
        ));
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
    let plan = &compiled.plan;
    let chunk = args.opt_u32("chunk-size")?.unwrap_or(1024);

    let body = json!({
        "status": "ok",
        "files": sources.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
        "model_digest": compiled.model_digest,
        "plan_digest": plan.digest,
        "order_digest": plan.order_digest,
        "tape_digest": compiled.tapes.digest,
        "opt": plan.opt.as_str(),
        "periods": plan.periods,
        "slots": {
            "scalar": plan.scalars.len(),
            "per_mp": plan.permp.len(),
            "series": plan.series.len(),
            "hoisted": plan.hoisted.len(),
            "stage1": plan.stage1.len(),
            "stage2": plan.stage2.len(),
            "outputs": plan.outputs.len(),
        },
        "order": plan.order_names(),
        "series_bytes_per_chunk": plan.series_bytes_per_chunk(chunk),
        "chunk_size": chunk,
        "summary": diagnostic_summary(&check.diagnostics),
        "diagnostics": check.diagnostics,
    });

    if let Some(out) = args.opt("out") {
        let text = serde_json::to_string_pretty(&json!({
            "format": crate::emit::FORMAT,
            "kind": "plan",
            "model_digest": compiled.model_digest,
            "plan": plan,
        }))
        .map_err(|e| e.to_string())?;
        std::fs::write(out, format!("{text}\n")).map_err(|e| format!("{out}: {e}"))?;
        emitter.note(format!("plan written to {out}"));
    }

    emitter.document("build", body)?;
    if !emitter.json {
        emitter.line(format!("model_digest  {}", compiled.model_digest));
        emitter.line(format!("plan_digest   {}", plan.digest));
        emitter.line(format!("order_digest  {}", plan.order_digest));
        emitter.line(format!(
            "{} slot(s): {} scalar, {} per-mp, {} series; {} output(s) over T = {}",
            plan.slot_refs.len(),
            plan.scalars.len(),
            plan.permp.len(),
            plan.series.len(),
            plan.outputs.len(),
            plan.periods,
        ));
        emitter.line(format!(
            "series buffer {} MiB per chunk of {chunk}",
            plan.series_bytes_per_chunk(chunk) as f64 / (1024.0 * 1024.0)
        ));
    }
    Ok(if check.diagnostics.is_empty() {
        OK
    } else {
        check.exit_code()
    })
}
