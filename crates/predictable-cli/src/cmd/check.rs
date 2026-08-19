//! `predictable check <path>... [--json]` — the diagnostics pass of `01-ir.md` §7.

use serde_json::json;

use crate::args::Args;
use crate::emit::{diagnostic_summary, Emitter, OK, USAGE};
use crate::load::Sources;

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["json", "out-json", "quiet"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out-json"];

/// Run `check`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    if args.positional.is_empty() {
        return Err("check needs at least one .pir path".into());
    }
    let sources = Sources::load(&args.positional)?;
    let result = sources.check();

    emitter.diagnostics(&result.diagnostics, &result.sources);
    emitter.document(
        "check",
        json!({
            "files": sources.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
            "model_digest": sources.model_digest(),
            "summary": diagnostic_summary(&result.diagnostics),
            "diagnostics": result.diagnostics,
            "exit_code": result.exit_code(),
        }),
    )?;

    let code = result.exit_code();
    if !emitter.json && !args.flag("quiet") {
        let n = sources.files.len();
        match code {
            OK => emitter.note(format!("{n} file(s) checked, no diagnostics")),
            USAGE => emitter.note(format!(
                "{n} file(s) checked, {} error(s)",
                result.errors().count()
            )),
            _ => emitter.note(format!(
                "{n} file(s) checked, {} lint(s)",
                result.diagnostics.len()
            )),
        }
    }
    Ok(code)
}
