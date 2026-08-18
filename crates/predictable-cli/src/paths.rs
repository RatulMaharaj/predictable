//! Turning command-line paths into the sorted list of `.pir` files a command works on.
//!
//! Sorting matters beyond tidiness: `model_digest` is computed over the canonical
//! text of every module *in path order* (`01-ir.md` §9.6), so two invocations that
//! name the same files in different orders must still hash the same. Expansion
//! happens once, here.

use std::path::{Path, PathBuf};

/// Expand paths (files or directories) into a sorted, deduplicated `.pir` file list.
pub fn collect_pir(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for p in paths {
        let path = PathBuf::from(p);
        if path.is_dir() {
            walk(&path, &mut out)?;
        } else if path.exists() {
            out.push(path);
        } else {
            return Err(format!("{p}: no such file or directory"));
        }
    }
    out.sort();
    out.dedup();
    if out.is_empty() {
        return Err("no .pir files found".into());
    }
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "pir") {
            out.push(path);
        }
    }
    Ok(())
}

/// Read a file, naming it in the error.
pub fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Resolve `reference` — a path written inside a `.pir` file — against the
/// directory that file lives in. An absolute reference is taken as written.
pub fn resolve_relative(base: &Path, reference: &str) -> PathBuf {
    let r = Path::new(reference);
    if r.is_absolute() {
        r.to_path_buf()
    } else {
        base.join(r)
    }
}
