//! Digests — `01-ir.md` §9.3 rule 6, §8.4.2 and §8.4.3.
//!
//! Every model-level digest is taken over **canonical text** ([`crate::canonical`]),
//! never over the bytes the author happened to save, so a formatting-only change
//! does not change a digest and a semantic change always does.
//!
//! Every digest is SHA-256 and is rendered `sha256:<64 lowercase hex digits>`.
//!
//! ## Framing
//!
//! A digest over several files must not depend on where one file ends and the
//! next begins, so the files are fed to the hash length-prefixed. For each file,
//! in ascending byte order of its path:
//!
//! ```text
//! <path>\n<byte length of the canonical text in decimal>\n<canonical text>
//! ```
//!
//! Concatenating two different file sets can therefore never produce the same
//! byte stream.

use sha2::{Digest, Sha256};

use crate::canonical::{format_source_with, Filter, FmtError};

/// Render a raw SHA-256 digest the way the IR spells it.
fn render(hash: [u8; 32]) -> String {
    let mut out = String::with_capacity(71);
    out.push_str("sha256:");
    for b in hash {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// `sha256:…` over arbitrary bytes. This is the *table file* digest of §2.9: the
/// identity of a table is the bytes the resolver returned, not its path.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    render(h.finalize().into())
}

/// One file entering a multi-file digest: its path and its **canonical** text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalFile {
    pub path: String,
    pub text: String,
}

/// Canonicalise `(path, source)` pairs, ready for [`model_digest`].
pub fn canonicalise_all<'a, I>(files: I) -> Result<Vec<CanonicalFile>, FmtError>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    files
        .into_iter()
        .map(|(path, source)| {
            Ok(CanonicalFile {
                path: path.to_string(),
                text: format_source_with(path, source, Filter::default())?,
            })
        })
        .collect()
}

/// `model_digest` — SHA-256 over the canonical text of every module in path
/// order, including the product file (§9.3 rule 6). The run file is **not**
/// included; it has its own [`run_digest`].
pub fn model_digest(files: &[CanonicalFile]) -> String {
    let mut sorted: Vec<&CanonicalFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut h = Sha256::new();
    for f in sorted {
        h.update(f.path.as_bytes());
        h.update(b"\n");
        h.update(f.text.len().to_string().as_bytes());
        h.update(b"\n");
        h.update(f.text.as_bytes());
    }
    render(h.finalize().into())
}

/// `model_digest` straight from unformatted sources.
pub fn model_digest_of_sources<'a, I>(files: I) -> Result<String, FmtError>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    Ok(model_digest(&canonicalise_all(files)?))
}

/// `run_digest` (§8.4.2) — SHA-256 over the canonical text of the `[run]` file
/// with `[run.exec]`, `out` and `progress` removed, because a field participates
/// iff changing it can change a number in `results.parquet`.
pub fn run_digest(path: &str, source: &str) -> Result<String, FmtError> {
    let text = format_source_with(
        path,
        source,
        Filter {
            drop_run_non_semantic: true,
        },
    )?;
    Ok(digest_bytes(text.as_bytes()))
}

/// The canonical text `run_digest` actually hashes — useful when a diff has to
/// explain *why* two runs differ.
pub fn run_digest_text(path: &str, source: &str) -> Result<String, FmtError> {
    format_source_with(
        path,
        source,
        Filter {
            drop_run_non_semantic: true,
        },
    )
}

/// `assumption_digest` / `modelpoint_digest` for a `.pir` assumption set: the
/// same rule as a module — canonical text, so reformatting is not a rebasis.
pub fn assumption_digest(path: &str, source: &str) -> Result<String, FmtError> {
    let text = format_source_with(path, source, Filter::default())?;
    Ok(digest_bytes(text.as_bytes()))
}

/// `results.component_set_digest` (§8.4.3) — SHA-256 over the sorted list of
/// emitted qualified component ids, one per line, each terminated by `\n`.
pub fn component_set_digest<S: AsRef<str>>(ids: &[S]) -> String {
    let mut sorted: Vec<&str> = ids.iter().map(|s| s.as_ref()).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut h = Sha256::new();
    for id in sorted {
        h.update(id.as_bytes());
        h.update(b"\n");
    }
    render(h.finalize().into())
}

/// The digest of an inlined table (§2.9.1): SHA-256 over the canonical text of
/// its `rows` array — the text `fmt` writes for `rows = …`, right-hand side
/// only, with one trailing newline. An inlined table and the CSV it came from
/// are therefore *not* required to share a digest, which is why a manifest
/// records both `digest` and `declared_digest`.
pub fn inline_rows_digest(
    module_path: &str,
    module_source: &str,
    table: &str,
) -> Result<Option<String>, FmtError> {
    let canonical = format_source_with(module_path, module_source, Filter::default())?;
    Ok(extract_rows(&canonical, table).map(|rows| digest_bytes(rows.as_bytes())))
}

/// Pull the canonical `rows = …` text of a named `[[table]]` out of canonical
/// module text. Operating on canonical text keeps this a substring scan rather
/// than a second parser.
fn extract_rows(canonical: &str, table: &str) -> Option<String> {
    let needle = format!("name = \"{table}\"");
    let mut in_table = false;
    let mut named = false;
    let mut rows: Option<String> = None;
    let mut collecting = false;
    for line in canonical.lines() {
        if line.starts_with('[') {
            if collecting {
                break;
            }
            in_table = line == "[[table]]";
            named = false;
            continue;
        }
        if !in_table {
            continue;
        }
        if line == needle {
            named = true;
        }
        if collecting {
            let done = line == "]";
            let buf = rows.get_or_insert_with(String::new);
            buf.push_str(line);
            buf.push('\n');
            if done {
                break;
            }
            continue;
        }
        if named {
            if let Some(rest) = line.strip_prefix("rows = ") {
                if rest == "[" {
                    collecting = true;
                    rows = Some("[\n".to_string());
                } else {
                    return Some(format!("{rest}\n"));
                }
            }
        }
    }
    rows
}
