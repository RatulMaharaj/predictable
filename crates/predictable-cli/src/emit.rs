//! The uniform output contract: one `pvf/1` document per command under `--json`,
//! prose otherwise, and the same exit code either way.
//!
//! `04-verify.md` §7: *"Every one of these accepts `--json` and writes a `pvf/1`
//! document. Every one exits `0` on success, `1` on a domain failure
//! (diagnostics/divergence), `2` on a usage or structural failure."* The mapping
//! from diagnostics to a code is not re-derived here — it is
//! [`predictable_diagnostics::exit_code`], so the CLI, the Python surface and the
//! engine cannot drift apart on what "failed" means.

use predictable_diagnostics::{render_all, Diagnostic, RenderOptions, SourceMap};
use serde_json::{json, Value};

/// The `format` field every JSON document the CLI writes carries.
pub const FORMAT: &str = "pvf/1";

/// Exit code 0: the command did what was asked.
pub const OK: i32 = 0;
/// Exit code 1: a *domain* failure — diagnostics, divergence, a trap.
pub const DOMAIN: i32 = 1;
/// Exit code 2: a usage or structural failure — bad flags, missing file, errors
/// that stop the toolchain before it can have an opinion about numbers.
pub const USAGE: i32 = 2;

/// Where a command writes. Held rather than called globally so tests can capture.
#[derive(Debug)]
pub struct Emitter {
    /// `--json` was given.
    pub json: bool,
    /// `--json <path>`: write the document to a file as well as reporting.
    pub json_path: Option<String>,
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Emitter {
    /// A new emitter buffering both streams.
    pub fn new(json: bool, json_path: Option<String>) -> Emitter {
        Emitter {
            json,
            json_path,
            out: Vec::new(),
            err: Vec::new(),
        }
    }

    /// A line on stdout — the command's result.
    pub fn line(&mut self, text: impl AsRef<str>) {
        self.out.extend_from_slice(text.as_ref().as_bytes());
        self.out.push(b'\n');
    }

    /// A line on stderr — progress and commentary, never the result.
    pub fn note(&mut self, text: impl AsRef<str>) {
        self.err.extend_from_slice(text.as_ref().as_bytes());
        self.err.push(b'\n');
    }

    /// The `pvf/1` document for `kind`, printed under `--json` and written to
    /// `--out-json` when one was given.
    pub fn document(&mut self, kind: &str, mut body: Value) -> Result<(), String> {
        let obj = body.as_object_mut().expect("a document is an object");
        let mut doc = serde_json::Map::new();
        doc.insert("format".into(), json!(FORMAT));
        doc.insert("kind".into(), json!(kind));
        doc.append(obj);
        let text = serde_json::to_string_pretty(&Value::Object(doc))
            .map_err(|e| format!("serialising the {kind} document: {e}"))?;
        if let Some(path) = self.json_path.clone() {
            std::fs::write(&path, format!("{text}\n")).map_err(|e| format!("{path}: {e}"))?;
        }
        if self.json {
            self.line(text);
        }
        Ok(())
    }

    /// Render diagnostics for a human. Suppressed under `--json`, where the
    /// diagnostics travel inside the document instead.
    pub fn diagnostics(&mut self, diagnostics: &[Diagnostic], sources: &SourceMap) {
        if self.json || diagnostics.is_empty() {
            return;
        }
        let text = render_all(diagnostics, sources, &RenderOptions::default());
        self.note(text);
    }

    /// Everything written to stdout so far.
    pub fn stdout(&self) -> &str {
        std::str::from_utf8(&self.out).unwrap_or_default()
    }

    /// Everything written to stderr so far.
    pub fn stderr(&self) -> &str {
        std::str::from_utf8(&self.err).unwrap_or_default()
    }

    /// Flush both buffers to the real process streams.
    pub fn flush(&self) {
        use std::io::Write;
        let _ = std::io::stdout().write_all(&self.out);
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().write_all(&self.err);
    }
}

/// One diagnostic summarised for a JSON document: the counts an agent branches on.
pub fn diagnostic_summary(diagnostics: &[Diagnostic]) -> Value {
    use predictable_diagnostics::Severity;
    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    json!({
        "errors": count(Severity::Error),
        "warnings": count(Severity::Warning),
        "info": count(Severity::Info),
        "help": count(Severity::Help),
    })
}
