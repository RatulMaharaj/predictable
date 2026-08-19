//! The conformance corpus is the checker's contract.
//!
//! `conformance/` was written from `01-ir.md` **before** this crate existed, so
//! it is a specification test and not a snapshot: when the two disagree, the
//! default assumption is that the checker is wrong. Each case is run exactly as
//! `conformance/README.md` §3 describes — the diagnostic set must equal
//! `expected.diag`, code for code and span for span, with unexpected extras
//! failing the case, which is what stops a checker from being noisy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use predictable_check::{check, Input};

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate lives at <root>/crates/<name>")
        .join("conformance")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Expected {
    severity: String,
    code: String,
    file: String,
    line: Option<u32>,
    col: Option<u32>,
    message: String,
}

/// One line of `expected.diag`: `<severity> <code> <file>[:<line>[:<col>]] <message>`.
fn parse_expected(text: &str) -> Vec<Expected> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(4, ' ');
        let severity = parts.next().unwrap_or_default().to_string();
        let code = parts.next().unwrap_or_default().to_string();
        let location = parts.next().unwrap_or_default();
        let message = parts.next().unwrap_or_default().to_string();
        let mut bits = location.split(':');
        let file = bits.next().unwrap_or_default().to_string();
        let line_no = bits.next().and_then(|s| s.parse().ok());
        let col = bits.next().and_then(|s| s.parse().ok());
        out.push(Expected {
            severity,
            code,
            file,
            line: line_no,
            col,
            message,
        });
    }
    out.sort();
    out
}

/// A family wildcard (`E05xx`) matches any code in that family.
fn code_matches(expected: &str, actual: &str) -> bool {
    if let Some(prefix) = expected.strip_suffix("xx") {
        actual.starts_with(prefix)
    } else {
        expected == actual
    }
}

struct Case {
    name: String,
    dir: PathBuf,
    kind: String,
    entry: Vec<String>,
    product: Option<String>,
    run: Option<String>,
}

fn cases() -> Vec<Case> {
    let root = corpus_root();
    let index = std::fs::read_to_string(root.join("index.toml")).expect("index.toml");
    let index: toml::Value = index.parse().expect("index.toml parses");
    let mut out = Vec::new();
    for case in index["case"].as_array().expect("[[case]] array") {
        let name = case["name"].as_str().unwrap().to_string();
        let dir = root.join(case["path"].as_str().unwrap());
        let meta: toml::Value = std::fs::read_to_string(dir.join("case.toml"))
            .expect("case.toml")
            .parse()
            .expect("case.toml parses");
        out.push(Case {
            name,
            kind: meta["kind"].as_str().unwrap().to_string(),
            entry: meta["entry"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                .unwrap_or_default(),
            product: meta
                .get("product")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            run: meta.get("run").and_then(|v| v.as_str()).map(str::to_string),
            dir,
        });
    }
    out
}

fn inputs(case: &Case) -> Vec<Input> {
    let mut files: Vec<String> = case.entry.clone();
    files.extend(case.product.clone());
    files.extend(case.run.clone());
    files
        .iter()
        .map(|name| {
            let path: &Path = Path::new(name);
            Input::new(
                name.clone(),
                std::fs::read_to_string(case.dir.join(path))
                    .unwrap_or_else(|e| panic!("{}: {e}", case.dir.join(path).display())),
            )
        })
        .collect()
}

fn run_case(case: &Case) -> Result<(), String> {
    let result = check(&inputs(case));
    let expected_text = std::fs::read_to_string(case.dir.join("expected.diag")).unwrap_or_default();
    let expected = parse_expected(&expected_text);

    let mut actual: Vec<Expected> = result
        .diagnostics
        .iter()
        .map(|d| {
            let span = d.primary_span();
            Expected {
                severity: d.severity.as_str().to_string(),
                code: d.code.clone(),
                file: span.map(|s| s.file.clone()).unwrap_or_default(),
                line: span.and_then(|s| s.start_pos).map(|p| p.line as u32),
                col: span.and_then(|s| s.start_pos).map(|p| p.column as u32),
                message: d.message.clone(),
            }
        })
        .collect();
    actual.sort();

    let mut problems = Vec::new();
    let mut matched: BTreeSet<usize> = BTreeSet::new();
    for want in &expected {
        let found = actual.iter().enumerate().find(|(i, got)| {
            !matched.contains(i)
                && got.severity == want.severity
                && code_matches(&want.code, &got.code)
                && got.file == want.file
                && want.line.map_or(true, |l| got.line == Some(l))
                && want.col.map_or(true, |c| got.col == Some(c))
                && got.message.contains(&want.message)
        });
        match found {
            Some((i, _)) => {
                matched.insert(i);
            }
            None => problems.push(format!("  missing: {want:?}")),
        }
    }
    for (i, got) in actual.iter().enumerate() {
        if !matched.contains(&i) {
            problems.push(format!("  unexpected: {got:?}"));
        }
    }

    // kind = "valid" means the model checks; it may still carry lints.
    let has_error = result
        .diagnostics
        .iter()
        .any(|d| d.severity == predictable_diagnostics::Severity::Error);
    if case.kind == "valid" && has_error {
        problems.push("  a valid case produced an error".to_string());
    }
    // A case whose `expected.diag` names no error is a runtime case: the fault
    // is in the data, not the model (`conformance/README.md` §3.2).
    let expects_error = expected.iter().any(|e| e.severity == "error");
    if case.kind == "invalid" && expects_error && !has_error {
        problems.push("  an invalid case produced no error".to_string());
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{}:\n{}", case.name, problems.join("\n")))
    }
}

#[test]
fn every_conformance_case_matches_its_expected_diagnostics() {
    let mut failures = Vec::new();
    for case in cases() {
        if let Err(message) = run_case(&case) {
            failures.push(message);
        }
    }
    assert!(
        failures.is_empty(),
        "{} conformance case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_diagnostic_carries_a_suggested_edit() {
    // §7: "Every diagnostic carries a code, a primary span, secondary spans, and
    // at least one suggested edit." This is the property the migration loop
    // depends on, so it is asserted over the whole corpus at once.
    for case in cases() {
        let result = check(&inputs(&case));
        for d in &result.diagnostics {
            // Pass 1 diagnostics belong to `predictable-syntax`, which has its
            // own tests; everything the checker itself emits is covered here.
            if d.code.starts_with("E00") {
                continue;
            }
            assert!(
                d.primary_span().is_some(),
                "{}: {} has no primary span",
                case.name,
                d.code
            );
            assert!(
                d.suggestions.iter().any(|s| !s.edits.is_empty()),
                "{}: {} carries no suggested edit",
                case.name,
                d.code
            );
        }
    }
}

#[test]
fn suggested_edits_apply_cleanly() {
    // A suggestion is literal: replacement text plus a byte range. Applying one
    // must produce text, not an error — an edit that cannot be applied is not a
    // suggestion, it is prose.
    for case in cases() {
        let files = inputs(&case);
        let result = check(&files);
        for d in &result.diagnostics {
            for suggestion in &d.suggestions {
                for edit in &suggestion.edits {
                    let Some(source) = files.iter().find(|f| f.name == edit.file) else {
                        panic!("{}: edit names unknown file {}", case.name, edit.file);
                    };
                    predictable_diagnostics::apply_edits(&source.text, std::slice::from_ref(edit))
                        .unwrap_or_else(|e| {
                            panic!("{}: {} edit does not apply: {e}", case.name, d.code)
                        });
                }
            }
        }
    }
}
