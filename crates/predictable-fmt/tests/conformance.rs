//! `fmt` against the conformance corpus (`conformance/README.md` §4).
//!
//! Three assertions, and the corpus is the specification for all three:
//!
//! 1. every `valid/**` input formats to its `*.pir.expected`, or to itself when
//!    no `.expected` exists;
//! 2. `fmt` is idempotent — `fmt(fmt(x)) == fmt(x)` — for every file in the
//!    corpus that parses, and for a mangled variant of each of them;
//! 3. mangling a file's layout does not change its `model_digest`.

use std::path::{Path, PathBuf};

use predictable_fmt::digest::{model_digest, CanonicalFile};
use predictable_fmt::{format_source, FmtError};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

fn pir_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    assert!(!out.is_empty(), "no .pir files under {}", dir.display());
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "pir") {
            out.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("utf-8")
}

/// Cases whose input is a deliberate parse error: `case.toml` says `fmt = false`.
fn fmt_disabled(path: &Path) -> bool {
    let case = path.parent().unwrap().join("case.toml");
    std::fs::read_to_string(case)
        .map(|t| {
            t.lines()
                .any(|l| l.starts_with("fmt") && l.contains("false"))
        })
        .unwrap_or(false)
}

fn fmt(path: &Path) -> Result<String, FmtError> {
    format_source(&path.display().to_string(), &read(path))
}

#[test]
fn every_valid_case_matches_its_expected_canonical_form() {
    let mut checked = 0;
    for path in pir_files(&corpus().join("valid")) {
        let expected_path = path.with_extension("pir.expected");
        let expected = if expected_path.exists() {
            read(&expected_path)
        } else {
            read(&path)
        };
        let got = fmt(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(got, expected, "canonical form of {}", path.display());
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} valid files seen");
}

#[test]
fn every_invalid_case_that_parses_is_already_canonical() {
    // `kind = "invalid"` means the *checker* rejects it, not the parser: unless
    // the case says `fmt = false`, its input must still be canonical text.
    for path in pir_files(&corpus().join("invalid")) {
        if fmt_disabled(&path) {
            assert!(
                fmt(&path).is_err(),
                "{} was supposed to be unparseable",
                path.display()
            );
            continue;
        }
        let got = fmt(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(got, read(&path), "{} is not canonical", path.display());
    }
}

#[test]
fn fmt_is_a_fixed_point_over_the_whole_corpus() {
    let mut checked = 0;
    for path in pir_files(&corpus()) {
        let Ok(once) = fmt(&path) else { continue };
        let twice = format_source(&path.display().to_string(), &once).expect("canonical reparses");
        assert_eq!(once, twice, "fmt is not idempotent on {}", path.display());
        checked += 1;
    }
    assert!(checked >= 25, "only {checked} files reached");
}

/// A deterministic layout mangler: it changes only what §4.1 says is
/// insignificant — whitespace, line endings, blank lines, float spelling and
/// (inside a `[[component]]`) key order.
fn mangle(source: &str, seed: u64) -> String {
    let mut rng = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let mut next = move || {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (rng >> 33) as u32
    };
    let mut in_component = false;
    let mut block: Vec<String> = Vec::new();
    let mut out = String::new();
    let flush = |block: &mut Vec<String>, out: &mut String, n: &mut dyn FnMut() -> u32| {
        // Reverse or rotate the key lines of the component we just left.
        if n() % 2 == 0 {
            block.reverse();
        } else if !block.is_empty() {
            block.rotate_left(1);
        }
        for l in block.drain(..) {
            out.push_str(&l);
            out.push('\n');
        }
    };
    for line in source.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with('[') {
            flush(&mut block, &mut out, &mut next);
            in_component = trimmed == "[[component]]";
            out.push_str(trimmed);
            out.push('\n');
            if next() % 3 == 0 {
                out.push('\n');
            }
            continue;
        }
        // Never touch a continuation line of a multi-line array.
        let is_entry = trimmed.contains(" = ") && !trimmed.starts_with(' ');
        let mut jittered = trimmed.to_string();
        if is_entry && next() % 2 == 0 {
            jittered = jittered.replacen(" = ", "  =   ", 1);
        }
        if is_entry && next() % 4 == 0 {
            jittered.push_str("   ");
        }
        if in_component && is_entry {
            block.push(jittered);
        } else {
            if !block.is_empty() {
                flush(&mut block, &mut out, &mut next);
            }
            out.push_str(&jittered);
            out.push('\n');
            if next() % 5 == 0 {
                out.push('\n');
            }
        }
    }
    flush(&mut block, &mut out, &mut next);
    out
}

#[test]
fn mangled_layout_formats_back_to_the_same_canonical_text() {
    let mut checked = 0;
    for path in pir_files(&corpus()) {
        let Ok(canonical) = fmt(&path) else { continue };
        let name = path.display().to_string();
        for seed in 0..8u64 {
            let mangled = mangle(&read(&path), seed);
            let got = format_source(&name, &mangled)
                .unwrap_or_else(|e| panic!("mangled {name} (seed {seed}): {e}\n{mangled}"));
            // Key order inside a component is authorially irrelevant (rule 1
            // fixes it), so the canonical text must come back identical.
            assert_eq!(got, canonical, "{name}, seed {seed}");
            checked += 1;
        }
    }
    assert!(checked >= 200, "only {checked} mangles ran");
}

#[test]
fn model_digest_survives_reformatting_of_every_corpus_model() {
    for case in std::fs::read_dir(corpus().join("valid")).unwrap() {
        let dir = case.unwrap().path();
        let files = pir_files(&dir);
        let canonical: Vec<CanonicalFile> = files
            .iter()
            .filter_map(|p| {
                fmt(p).ok().map(|text| CanonicalFile {
                    path: name_of(p),
                    text,
                })
            })
            .collect();
        if canonical.is_empty() {
            continue;
        }
        let baseline = model_digest(&canonical);

        let mangled: Vec<CanonicalFile> = files
            .iter()
            .filter_map(|p| {
                let source = mangle(&read(p), 7);
                format_source(&name_of(p), &source)
                    .ok()
                    .map(|text| CanonicalFile {
                        path: name_of(p),
                        text,
                    })
            })
            .collect();
        assert_eq!(mangled.len(), canonical.len(), "{}", dir.display());
        assert_eq!(model_digest(&mangled), baseline, "{}", dir.display());
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().to_string()
}
