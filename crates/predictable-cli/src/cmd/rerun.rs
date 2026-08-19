//! `predictable rerun <run/manifest.json>` — re-execute from the manifest alone.
//!
//! `04-verify.md` §6.3 rule 7: *"re-executes from the manifest alone, verifying
//! every digest before starting and refusing on any mismatch (`--allow-drift` to
//! proceed, which stamps `"drift": true` into the new manifest permanently)."*
//!
//! The verification is the point. A rerun that silently picked up an edited model
//! would produce a run directory that *looks* like a reproduction and is not, and
//! there is no downstream check that could tell the difference afterwards.

use std::path::{Path, PathBuf};

use predictable_io::outbound::Manifest;
use serde_json::json;

use crate::args::Args;
use crate::emit::{Emitter, DOMAIN, OK, USAGE};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["json", "out-json", "out", "allow-drift", "verify-only"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out-json", "out"];

/// One input the manifest pins, and what it hashes to now.
#[derive(Debug)]
struct Verified {
    what: String,
    path: String,
    expected: String,
    actual: Option<String>,
}

impl Verified {
    fn drifted(&self) -> bool {
        self.actual.as_deref() != Some(self.expected.as_str())
    }
}

/// Run `rerun`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let manifest_path = PathBuf::from(args.one_positional("path to a run/manifest.json")?);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest = Manifest::from_json(&text).map_err(|e| e.to_string())?;
    let run_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let self_consistent = manifest.verify_digest().map_err(|e| e.to_string())?;
    let checks = verify_inputs(&manifest);
    let drifted: Vec<&Verified> = checks.iter().filter(|c| c.drifted()).collect();
    let allow = args.flag("allow-drift");

    emitter.document(
        "rerun",
        json!({
            "manifest": manifest_path.display().to_string(),
            "run_id": manifest.run_id,
            "manifest_digest_verified": self_consistent,
            "inputs": checks.iter().map(|c| json!({
                "what": c.what,
                "path": c.path,
                "expected": c.expected,
                "actual": c.actual,
                "drift": c.drifted(),
            })).collect::<Vec<_>>(),
            "drift": !drifted.is_empty(),
            "allow_drift": allow,
            "verify_only": args.flag("verify-only"),
        }),
    )?;

    if !emitter.json {
        emitter.line(format!(
            "manifest_digest {}",
            if self_consistent {
                "verified"
            } else {
                "MISMATCH"
            }
        ));
        for c in &checks {
            let state = match (&c.actual, c.drifted()) {
                (None, _) => "missing",
                (Some(_), true) => "DRIFT",
                _ => "ok",
            };
            emitter.line(format!("{state:>8}  {:<12} {}", c.what, c.path));
        }
    }

    if !self_consistent {
        emitter.note("the manifest does not re-derive its own digest; it has been edited");
        return Ok(USAGE);
    }
    if !drifted.is_empty() && !allow {
        emitter.note(format!(
            "{} input(s) drifted since the run; refusing. Pass --allow-drift to proceed anyway.",
            drifted.len()
        ));
        return Ok(DOMAIN);
    }
    if args.flag("verify-only") {
        return Ok(OK);
    }

    // Re-execute through exactly the same path a fresh `run` takes: the manifest
    // names the `[run]` file, and that file is still the source of truth for what
    // a run *is*. Nothing about the run is reconstructed from the manifest.
    let out = match args.opt("out") {
        Some(o) => o.to_string(),
        None => format!("{}-rerun", run_dir.display()),
    };
    let mut argv = vec![
        manifest.run_config.path.clone(),
        "--out".to_string(),
        out,
        "--run-id".to_string(),
        format!("{}-rerun", manifest.run_id),
    ];
    if allow {
        argv.push("--allow-table-drift".to_string());
    }
    let run_args = Args::parse(&argv, crate::cmd::run::VALUE_FLAGS)?;
    crate::cmd::run::run(&run_args, emitter)
}

fn verify_inputs(manifest: &Manifest) -> Vec<Verified> {
    let mut out = Vec::new();
    for file in &manifest.inputs.model.files {
        out.push(Verified {
            what: "model".to_string(),
            path: file.path.clone(),
            expected: file.digest.clone(),
            // A model file's pinned digest is over its *canonical* text (§9.6),
            // so re-hashing the raw bytes of a re-formatted file would report a
            // drift that changes no number. Canonicalise first, exactly as the
            // run did.
            actual: canonical_digest(&file.path),
        });
    }
    if let Some(a) = &manifest.inputs.assumptions {
        out.push(Verified {
            what: "assumptions".to_string(),
            path: a.path.clone(),
            expected: a.digest.clone(),
            actual: std::fs::read_to_string(&a.path)
                .ok()
                .and_then(|t| predictable_fmt::digest::assumption_digest(&a.path, &t).ok()),
        });
    }
    let mp = &manifest.inputs.modelpoints;
    out.push(Verified {
        what: "modelpoints".to_string(),
        path: mp.path.clone(),
        expected: mp.digest.clone(),
        actual: std::fs::read(&mp.path)
            .ok()
            .map(|b| predictable_fmt::digest::digest_bytes(&b)),
    });
    for table in &manifest.inputs.tables {
        out.push(Verified {
            what: "table".to_string(),
            path: table.path.clone(),
            expected: table.digest.clone(),
            actual: std::fs::read(&table.path)
                .ok()
                .map(|b| predictable_fmt::digest::digest_bytes(&b)),
        });
    }
    out
}

fn canonical_digest(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let canonical = predictable_fmt::canonical::format_source(path, &text).ok()?;
    Some(predictable_fmt::digest::digest_bytes(canonical.as_bytes()))
}
