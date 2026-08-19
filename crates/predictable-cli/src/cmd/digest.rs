//! `predictable digest [--kind <kind>] <path>...` — the digests of `01-ir.md` §9.6.

use std::path::PathBuf;

use predictable_fmt::digest;
use serde_json::json;

use crate::args::Args;
use crate::emit::{Emitter, OK};
use crate::paths::{collect_pir, read};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["kind", "each", "json", "out-json"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["kind", "out-json"];

/// Run `digest`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let kind = args.opt("kind").unwrap_or("model").to_string();
    let each = args.flag("each");
    if args.positional.is_empty() {
        return Err("digest needs at least one path".into());
    }
    let files: Vec<PathBuf> = if kind == "file" {
        args.positional.iter().map(PathBuf::from).collect()
    } else {
        collect_pir(&args.positional)?
    };

    let mut entries: Vec<serde_json::Value> = Vec::new();
    // The text form keeps the suffix a human needs to tell one kind of digest
    // from another at a glance; the JSON form carries it as a field instead.
    let mut push = |emitter: &mut Emitter, digest: String, path: String, label: &str| {
        if !emitter.json {
            let suffix = match label {
                "run" => " (run_digest)".to_string(),
                "assumptions" => " (assumption_digest)".to_string(),
                "model" => "".to_string(),
                l if l.starts_with("table ") => format!(" ({l})"),
                _ => String::new(),
            };
            emitter.line(format!("{digest}  {path}{suffix}"));
        }
        entries.push(json!({"digest": digest, "path": path, "kind": label}));
    };

    match kind.as_str() {
        "file" => {
            for f in &files {
                let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
                push(
                    emitter,
                    digest::digest_bytes(&bytes),
                    f.display().to_string(),
                    "file",
                );
            }
        }
        "run" => {
            for f in &files {
                let source = read(f)?;
                let path = f.display().to_string();
                let d = digest::run_digest(&path, &source).map_err(|e| e.to_string())?;
                push(emitter, d, path, "run");
            }
        }
        "assumptions" => {
            for f in &files {
                let source = read(f)?;
                let path = f.display().to_string();
                let d = digest::assumption_digest(&path, &source).map_err(|e| e.to_string())?;
                push(emitter, d, path, "assumptions");
            }
        }
        "model" => {
            let mut canonical = Vec::new();
            for f in &files {
                let source = read(f)?;
                let path = f.display().to_string();
                let text = predictable_fmt::canonical::format_source(&path, &source)
                    .map_err(|e| e.to_string())?;
                if each {
                    push(
                        emitter,
                        digest::digest_bytes(text.as_bytes()),
                        path.clone(),
                        "file",
                    );
                }
                canonical.push(digest::CanonicalFile { path, text });
            }
            push(
                emitter,
                digest::model_digest(&canonical),
                "model_digest".to_string(),
                "model",
            );
        }
        k if k.starts_with("table:") => {
            let name = &k["table:".len()..];
            for f in &files {
                let source = read(f)?;
                let path = f.display().to_string();
                match digest::inline_rows_digest(&path, &source, name).map_err(|e| e.to_string())? {
                    Some(d) => push(emitter, d, path, &format!("table {name}")),
                    None => return Err(format!("{path}: no inlined table `{name}` with `rows`")),
                }
            }
        }
        other => return Err(format!("unknown digest kind `{other}`")),
    }

    emitter.document("digest", json!({ "digest_kind": kind, "digests": entries }))?;
    Ok(OK)
}
