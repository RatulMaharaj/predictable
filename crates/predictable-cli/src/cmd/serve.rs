//! `predictable serve <path>... [--port N] [--no-block]` — the local viz server
//! of `05-viz.md` §1.2, pointed at a model rather than at a run.
//!
//! This is the command-line face of `model.show()`: it plans the model, builds
//! the `GraphDoc` from the planner's own ordering and serves it on loopback with
//! a token. It holds no results, so it reports `explain: false` and the UI greys
//! the trace button out rather than offering a dead one.

use predictable_plan::PlanOptions;
use predictable_viz::{InMemoryDataSource, ServerOptions, VizServer};
use serde_json::json;
use std::sync::Arc;

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["json", "out-json", "port", "no-block", "O0", "retain-all"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out-json", "port"];

use crate::args::Args;
use crate::emit::{Emitter, OK, USAGE};
use crate::load::{compile, Sources};

/// Run `serve`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    if args.positional.is_empty() {
        return Err("serve needs at least one .pir path".into());
    }
    let sources = Sources::load(&args.positional)?;
    let check = sources.check();
    if !check.is_ok() {
        emitter.diagnostics(&check.diagnostics, &check.sources);
        emitter.document(
            "serve",
            json!({"status": "not_checked", "diagnostics": check.diagnostics}),
        )?;
        emitter.note("serve refused: the model does not check");
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
    let nodes = doc.nodes.len();

    let port = match args.opt("port") {
        Some(text) => text
            .parse::<u16>()
            .map_err(|_| format!("`--port {text}` is not a port number"))?,
        None => ServerOptions::default().port,
    };
    let source = InMemoryDataSource::new(doc).with_manifest(json!({
        "format": "pvf/1",
        "inputs": {"model": {"digest": compiled.model_digest, "files": sources.file_digests()}},
        "plan": {"digest": compiled.plan.digest, "order_digest": compiled.plan.order_digest},
    }));
    let server = VizServer::start(
        Arc::new(source),
        ServerOptions {
            port,
            ..ServerOptions::default()
        },
    )
    .map_err(|e| format!("cannot start the viz server: {e}"))?;

    emitter.document(
        "serve",
        json!({
            "url": server.url(),
            "port": server.addr().port(),
            "token": server.token(),
            "nodes": nodes,
            "capabilities": {"explain": false, "recompute": false, "sensitivity": false},
        }),
    )?;
    if !emitter.json {
        emitter.line(format!("serving {nodes} components at {}", server.url()));
        emitter.line("the token is in the fragment; every API call must carry it");
    }

    if args.flag("no-block") {
        // The process owns the server for the rest of its life: the caller asked
        // not to block, and shutting down on return would hand back a URL that
        // is already dead.
        std::mem::forget(server);
        return Ok(OK);
    }
    if !emitter.json {
        emitter.line("Ctrl-C to stop");
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
