//! `predictable graph <path>... [--json]` — the `GraphDoc` of `05-viz.md` §2.1.
//!
//! One rule governs this command: **the viz layer does not compute the topological
//! order — it renders the engine's.** So `layers` here is bucketed out of
//! [`predictable_plan::Plan::order`], the same sequence `order_digest` hashes and
//! the same sequence the tapes execute. If the picture and the evaluation order
//! ever disagreed, the picture would be a lie about the run; deriving both from
//! one list is what makes that impossible rather than merely unlikely.
//!
//! The document itself is built by [`predictable_viz::graph_doc`], which is also
//! what `GET /api/graph` serves. The CLI and the browser therefore cannot show
//! two different graphs of the same model — there is one builder, not two.

use predictable_plan::PlanOptions;
use serde_json::{json, Value};

use crate::args::Args;
use crate::emit::{Emitter, OK, USAGE};
use crate::load::{compile, Sources};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["json", "out-json", "O0", "retain-all"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out-json"];

/// Run `graph`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    if args.positional.is_empty() {
        return Err("graph needs at least one .pir path".into());
    }
    let sources = Sources::load(&args.positional)?;
    let check = sources.check();
    if !check.is_ok() {
        emitter.diagnostics(&check.diagnostics, &check.sources);
        emitter.document(
            "graph",
            json!({"status": "not_checked", "diagnostics": check.diagnostics}),
        )?;
        emitter.note("graph refused: the model does not check");
        return Ok(USAGE);
    }

    let options = PlanOptions {
        opt: if args.flag("O0") {
            predictable_plan::OptLevel::O0
        } else {
            predictable_plan::OptLevel::O1
        },
        retain_all: args.flag("retain-all"),
        program_digest: String::new(),
        periods: None,
    };
    let compiled = compile(&sources, &options)?;
    let doc = predictable_viz::graph_doc(&sources.inputs, &compiled.plan, &compiled.model_digest);
    let doc: Value = serde_json::to_value(&doc).map_err(|e| e.to_string())?;

    emitter.document("graph", doc.clone())?;
    if !emitter.json {
        let nodes = doc["nodes"].as_array().map(Vec::len).unwrap_or(0);
        let edges = doc["edges"].as_array().map(Vec::len).unwrap_or(0);
        let layers = doc["layers"].as_array().map(Vec::len).unwrap_or(0);
        emitter.line(format!(
            "{nodes} node(s), {edges} edge(s), {layers} layer(s) in evaluation order"
        ));
        for (depth, layer) in doc["layers"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .enumerate()
        {
            let names: Vec<&str> = layer
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            emitter.line(format!("  {depth:>2}  {}", names.join(", ")));
        }
    }
    Ok(OK)
}
