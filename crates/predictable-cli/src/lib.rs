//! `predictable` — the command-line front end (`04-verify.md` §7, `05-viz.md` §1.4).
//!
//! ```text
//! predictable check   <path>...            # IR §7 diagnostics
//! predictable fmt     [--check] <path>...  # canonical form, IR §4.1
//! predictable digest  [--kind k] <path>... # model/run/assumption/table digests
//! predictable build   <path>...            # check + plan + lower; digests, no data
//! predictable run     <run.pir>            # → run/ with manifest + results
//! predictable graph   <path>...            # the GraphDoc of 05-viz §2.1
//! predictable export  <run/> --out pack.html
//! predictable rerun   <run/manifest.json>  # verify every digest, then re-execute
//! predictable diff model <a> <b>          # structural model diff, IR §11.3
//! predictable diff run   <a> <b>          # run diff, 04-verify.md §5
//! predictable explain <run/> --component c --mp k --t 3   # provenance trace, 04 §3
//! predictable migrate                      # stub; phase 3 owns it
//! ```
//!
//! ## The two contracts
//!
//! **`--json` everywhere.** Every subcommand accepts `--json` and writes one
//! `pvf/1` document to stdout; `--out-json <path>` writes the same document to a
//! file. An agent never has to parse prose, and prose is never the only place a
//! fact appears.
//!
//! **Exit codes mean one thing.** `0` success, `1` a *domain* failure (lints, a
//! trap, drift, a non-canonical file under `--check`), `2` a usage or structural
//! failure (bad flags, a missing file, errors that stop the toolchain before it
//! can have an opinion about numbers). The mapping from diagnostics to a code is
//! [`predictable_diagnostics::exit_code`] and is not re-derived here, so the CLI,
//! the Python surface and the engine cannot drift apart on what "failed" means.
//!
//! ```
//! let (code, out, _err) = predictable_cli::dispatch(&["--version".to_string()]);
//! assert_eq!(code, 0);
//! assert!(out.starts_with("predictable "));
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod args;
pub mod clock;
pub mod cmd;
pub mod emit;
pub mod load;
pub mod paths;

use args::Args;
use emit::{Emitter, OK, USAGE};

/// The `--help` text: the whole surface, in the order an agent uses it.
pub const USAGE_TEXT: &str = "\
predictable — actuarial modelling that diffs

USAGE:
    predictable <command> [options] [paths...]

COMMANDS
    check <path>...          Check .pir files (01-ir.md §7). Exit 2 on errors, 1 on lints.
    fmt [--check] <path>...  Rewrite in canonical form (§4.1). `-` reads stdin.
    digest [--kind k] <p>... model | run | assumptions | file | table:<name>
    build <path>...          Check, plan and lower. --out <plan.json>, --O0, --retain-all.
    run <run.pir>            Project a portfolio into a run directory.
                             --out <dir> --model <p> --assumptions <p> --modelpoints <p>
                             --threads N --chunk-size N --O0 --retain-all --allow-table-drift
    graph <path>...          The GraphDoc of 05-viz.md §2.1, in the planner's own order.
    serve <path>...          Serve the graph explorer on 127.0.0.1 with a token (05-viz.md §1.2).
                             --port N, --no-block.
    export <run/> --out f    Single-file governance pack. --format html|json.
    rerun <manifest.json>    Verify every input digest, then re-execute. --allow-drift,
                             --verify-only, --out <dir>.
    diff model <a> <b>       Structural diff of two models (01-ir.md §11.3).
                             --fail-on-change exits 1 when anything semantic moved.
    diff run <a> <b>         Run diff (04-verify.md §5). Each side is a run directory, or a
                             Prophet .rpt. Exit 0 within tolerance, 1 diverged, 2 incomparable.
                             --tolerance-profile exact|regression|reconcile|materiality
                             --abs N --rel N --mapping migration/mapping.toml --top N
                             --component <name> --mp <key> --fail-on any|root|never
                             --require-same-emit --explain-tolerance --no-source-precision
                             --model-a <p> --model-b <p> --period-base 0|1
    explain <run/>           Replay one modelpoint and print the provenance trace of a cell.
                             --component <name> --mp <key> [--t N] [--depth N|-1]
                             [--expand a,b] [--values-only] [--trace-max-terms N] [--width N]
    migrate                  Not implemented here.

GLOBAL
    --json                   Write a pvf/1 document to stdout instead of prose.
    --out-json <path>        Also write that document to <path>.
    --help, --version

EXIT CODES
    0  success
    1  a domain failure: lints, traps, drift, a non-canonical file under --check
    2  a usage or structural failure: bad flags, a missing file, checker errors
";

/// Run one invocation and return `(exit_code, stdout, stderr)`.
///
/// The whole CLI is a function of its arguments and the filesystem, with no
/// global state and no direct printing, so every subcommand is testable end to
/// end without spawning a process.
pub fn dispatch(argv: &[String]) -> (i32, String, String) {
    let command = argv.first().map(String::as_str);
    let rest = if argv.is_empty() { &[][..] } else { &argv[1..] };

    let value_flags: &[&str] = match command {
        Some("check") => cmd::check::VALUE_FLAGS,
        Some("fmt") => cmd::fmt::VALUE_FLAGS,
        Some("digest") => cmd::digest::VALUE_FLAGS,
        Some("build") => cmd::build::VALUE_FLAGS,
        Some("run") => cmd::run::VALUE_FLAGS,
        Some("graph") => cmd::graph::VALUE_FLAGS,
        Some("serve") => cmd::serve::VALUE_FLAGS,
        Some("export") => cmd::export::VALUE_FLAGS,
        Some("rerun") => cmd::rerun::VALUE_FLAGS,
        Some("diff") if rest.first().map(String::as_str) == Some("model") => cmd::diff::VALUE_FLAGS,
        Some("diff") if rest.first().map(String::as_str) == Some("run") => {
            cmd::rundiff::VALUE_FLAGS
        }
        Some("explain") => cmd::explain::VALUE_FLAGS,
        Some("diff") | Some("migrate") => cmd::stub::VALUE_FLAGS,
        _ => &[],
    };

    let parsed = match Args::parse(rest, value_flags) {
        Ok(args) => args,
        Err(message) => return (USAGE, String::new(), format!("error: {message}\n")),
    };
    let mut emitter = Emitter::new(
        parsed.flag("json"),
        parsed.opt("out-json").map(str::to_string),
    );

    let result = match command {
        None | Some("-h") | Some("--help") | Some("help") => {
            emitter.line(USAGE_TEXT.trim_end());
            Ok(OK)
        }
        Some("--version") | Some("-V") | Some("version") => {
            emitter.line(format!("predictable {}", env!("CARGO_PKG_VERSION")));
            Ok(OK)
        }
        Some("check") => cmd::check::run(&parsed, &mut emitter),
        Some("fmt") => cmd::fmt::run(&parsed, &mut emitter),
        Some("digest") => cmd::digest::run(&parsed, &mut emitter),
        Some("build") => cmd::build::run(&parsed, &mut emitter),
        Some("run") => cmd::run::run(&parsed, &mut emitter),
        Some("graph") => cmd::graph::run(&parsed, &mut emitter),
        Some("serve") => cmd::serve::run(&parsed, &mut emitter),
        Some("export") => cmd::export::run(&parsed, &mut emitter),
        Some("rerun") => cmd::rerun::run(&parsed, &mut emitter),
        Some("diff") if parsed.positional.first().map(String::as_str) == Some("model") => {
            cmd::diff::run(&parsed, &mut emitter)
        }
        Some("diff") if parsed.positional.first().map(String::as_str) == Some("run") => {
            cmd::rundiff::run(&parsed, &mut emitter)
        }
        Some("diff") => cmd::stub::run(&cmd::stub::DIFF, &parsed, &mut emitter),
        Some("explain") => cmd::explain::run(&parsed, &mut emitter),
        Some("migrate") => cmd::stub::run(&cmd::stub::MIGRATE, &parsed, &mut emitter),
        Some(other) => Err(format!(
            "unknown subcommand `{other}`. Try `predictable --help`."
        )),
    };

    match result {
        Ok(code) => (
            code,
            emitter.stdout().to_string(),
            emitter.stderr().to_string(),
        ),
        Err(message) => (
            USAGE,
            emitter.stdout().to_string(),
            format!("{}error: {message}\n", emitter.stderr()),
        ),
    }
}
