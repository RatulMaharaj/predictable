//! `predictable fmt [--check] [--stdout] <path>...` — canonical form, `01-ir.md` §4.1.

use std::io::Read;

use serde_json::json;

use crate::args::Args;
use crate::emit::{Emitter, DOMAIN, OK};
use crate::paths::{collect_pir, read};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["check", "stdout", "json", "out-json"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out-json"];

/// Run `fmt`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let check = args.flag("check");
    let to_stdout = args.flag("stdout");
    if args.positional.is_empty() {
        return Err("fmt needs at least one path (or `-` for stdin)".into());
    }

    if args.positional.len() == 1 && args.positional[0] == "-" {
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|e| e.to_string())?;
        let canonical = format("<stdin>", &source)?;
        emitter.line(canonical.trim_end_matches('\n'));
        return Ok(if check && canonical != source {
            DOMAIN
        } else {
            OK
        });
    }

    let files = collect_pir(&args.positional)?;
    let mut unformatted: Vec<String> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    for file in &files {
        let source = read(file)?;
        let name = file.display().to_string();
        let canonical = format(&name, &source)?;
        if to_stdout {
            emitter.line(canonical.trim_end_matches('\n'));
            continue;
        }
        if canonical == source {
            continue;
        }
        if check {
            unformatted.push(name.clone());
            if !emitter.json {
                emitter.line(format!("{name}: not canonical"));
            }
        } else {
            std::fs::write(file, &canonical).map_err(|e| format!("{name}: {e}"))?;
            changed.push(name.clone());
            if !emitter.json {
                emitter.line(format!("{name}: formatted"));
            }
        }
    }

    emitter.document(
        "fmt",
        json!({
            "mode": if check { "check" } else if to_stdout { "stdout" } else { "write" },
            "files_checked": files.len(),
            "not_canonical": unformatted,
            "formatted": changed,
        }),
    )?;

    if check {
        emitter.note(format!(
            "{} file(s) checked, {} not canonical",
            files.len(),
            unformatted.len()
        ));
        return Ok(if unformatted.is_empty() { OK } else { DOMAIN });
    }
    if !to_stdout {
        emitter.note(format!(
            "{} file(s) checked, {} formatted",
            files.len(),
            changed.len()
        ));
    }
    Ok(OK)
}

fn format(name: &str, source: &str) -> Result<String, String> {
    predictable_fmt::canonical::format_source(name, source).map_err(|e| e.to_string())
}
