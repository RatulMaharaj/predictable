//! Registry invariants, and the docs-anchor check that keeps every `doc_url` live.

use std::collections::BTreeSet;
use std::path::PathBuf;

use predictable_diagnostics::registry::{codes_in, lookup, DOC_PATH, REGISTRY};
use predictable_diagnostics::{Namespace, Severity};

fn docs_source() -> String {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate lives at <root>/crates/<name>")
        .to_path_buf();
    let path = repo_root.join(DOC_PATH);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read the diagnostics catalogue at {path:?}: {e}"))
}

/// The anchor form the catalogue uses: an explicit `<a id="CODE"></a>` before the
/// heading, rather than a slugified heading id, so the anchor survives any
/// rewording of the title.
fn anchor(code: &str) -> String {
    format!("<a id=\"{code}\"></a>")
}

#[test]
fn every_registered_code_has_a_docs_anchor() {
    let docs = docs_source();
    let missing: Vec<&str> = REGISTRY
        .iter()
        .filter(|c| !docs.contains(&anchor(c.code)))
        .map(|c| c.code)
        .collect();
    assert!(
        missing.is_empty(),
        "these registered codes have no anchor in {DOC_PATH}: {missing:?}"
    );
}

#[test]
fn every_docs_anchor_is_a_registered_code() {
    let docs = docs_source();
    let mut orphans = Vec::new();
    for (idx, _) in docs.match_indices("<a id=\"") {
        let rest = &docs[idx + 7..];
        let end = rest.find('"').expect("unterminated anchor id");
        let code = &rest[..end];
        if lookup(code).is_none() {
            orphans.push(code.to_string());
        }
    }
    assert!(
        orphans.is_empty(),
        "{DOC_PATH} documents codes that are not registered: {orphans:?}"
    );
}

#[test]
fn every_registered_code_has_its_title_in_the_docs() {
    // The catalogue is generated from the registry, so a stale title is a real
    // drift signal, not a formatting nit.
    let docs = docs_source();
    let missing: Vec<&str> = REGISTRY
        .iter()
        .filter(|c| !docs.contains(c.title))
        .map(|c| c.code)
        .collect();
    assert!(
        missing.is_empty(),
        "these codes' titles differ between the registry and {DOC_PATH}: {missing:?}"
    );
}

#[test]
fn registry_is_sorted_and_unique() {
    let mut previous: Option<&str> = None;
    for entry in REGISTRY {
        if let Some(prev) = previous {
            assert!(
                prev < entry.code,
                "registry must be sorted and unique: `{prev}` precedes `{}`",
                entry.code
            );
        }
        previous = Some(entry.code);
    }
}

#[test]
fn codes_are_well_formed_and_match_their_namespace() {
    for entry in REGISTRY {
        assert_eq!(
            entry.code.len(),
            5,
            "`{}` is not five characters",
            entry.code
        );
        let (prefix, digits) = entry.code.split_at(2);
        assert!(
            digits.chars().all(|c| c.is_ascii_digit()),
            "`{}` must end in three digits",
            entry.code
        );
        assert_eq!(
            prefix,
            entry.namespace.prefix(),
            "`{}` is filed under {:?}",
            entry.code,
            entry.namespace
        );
        assert_eq!(
            Namespace::of_code(entry.code),
            Some(entry.namespace),
            "namespace inference disagrees for `{}`",
            entry.code
        );
    }
}

#[test]
fn namespace_severity_conventions_hold() {
    for entry in REGISTRY {
        match entry.namespace {
            Namespace::IrError | Namespace::DslError => {
                assert_eq!(
                    entry.severity,
                    Severity::Error,
                    "{} must be an error",
                    entry.code
                )
            }
            Namespace::IrLint | Namespace::DslLint => assert_eq!(
                entry.severity,
                Severity::Warning,
                "{} must be a warning",
                entry.code
            ),
            Namespace::TraceNote | Namespace::Hypothesis => assert_eq!(
                entry.severity,
                Severity::Info,
                "{} must be informational",
                entry.code
            ),
            // Reader diagnostics span all three severities by design.
            Namespace::ReaderNote => assert_ne!(entry.severity, Severity::Help),
        }
    }
}

#[test]
fn all_seven_namespaces_are_populated() {
    for ns in Namespace::all() {
        assert!(
            codes_in(*ns).next().is_some(),
            "namespace {ns:?} ({}) has no registered codes",
            ns.prefix()
        );
    }
}

#[test]
fn decision_log_codes_are_registered() {
    // 01-ir.md §13 lists the codes its rulings introduce; T03 owns registering them.
    for code in [
        "E0101", "E0105", "E0106", "E0107", "E0108", "E0402", "E0403", "E0404", "E0602", "E0801",
        "E0902", "E0903", "W0105",
    ] {
        assert!(
            lookup(code).is_some(),
            "decision-log code {code} is missing"
        );
    }
}

#[test]
fn lookup_rejects_unknown_and_malformed_codes() {
    for code in ["E9999", "", "e0201", "E020", "X0101", "E0201 "] {
        assert!(lookup(code).is_none(), "`{code}` should not resolve");
    }
}

#[test]
fn doc_urls_are_unique_and_anchored_on_the_code() {
    let urls: BTreeSet<String> = REGISTRY.iter().map(|c| c.doc_url()).collect();
    assert_eq!(urls.len(), REGISTRY.len());
    let e0201 = lookup("E0201").unwrap();
    assert_eq!(
        e0201.doc_url(),
        "https://predictable.dev/llm/diagnostics/#E0201"
    );
}

// ---------------------------------------------------------------------------
// The CI link check of 04-verify.md §8.3: *every code emitted anywhere in the
// codebase* — not merely every code someone remembered to register — must
// resolve through its `doc_url` to a section of the catalogue. Without this the
// registry can be complete and the product still ship a code an agent cannot
// look up, which is the failure mode the skill's loop depends on not happening.
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate lives at <root>/crates/<name>")
        .to_path_buf()
}

/// Every `.rs` / `.py` file under a `src/` directory of the workspace.
fn source_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut roots = vec![root.join("packages/predictable/src")];
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ is readable");
    for entry in crates.flatten() {
        roots.push(entry.path().join("src"));
    }
    let mut files = Vec::new();
    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rs") | Some("py")
            ) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// String literals in `text` that have the exact shape of a diagnostic code.
fn code_literals(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    for (idx, _) in text.match_indices('"') {
        // `"Xnnnn"` is exactly seven bytes including both quotes.
        let Some(window) = text.get(idx..idx + 7) else {
            continue;
        };
        if bytes.get(idx + 6) != Some(&b'"') {
            continue;
        }
        let body = &window[1..6];
        let mut chars = body.chars();
        let head = chars.next().unwrap();
        if !matches!(head, 'E' | 'W' | 'P' | 'N' | 'H') {
            continue;
        }
        if chars.all(|c| c.is_ascii_digit()) {
            found.push(body.to_string());
        }
    }
    found
}

#[test]
fn code_literals_only_matches_whole_codes() {
    assert_eq!(code_literals(r#"Diagnostic::error("E0201", x)"#), ["E0201"]);
    assert!(code_literals(r#""E020" "E02011" "e0201" "X0201""#).is_empty());
}

#[test]
fn every_code_emitted_in_the_codebase_is_registered_and_anchored() {
    let docs = docs_source();
    let mut unregistered: BTreeSet<String> = BTreeSet::new();
    let mut unanchored: BTreeSet<String> = BTreeSet::new();
    let mut seen = 0usize;
    for path in source_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for code in code_literals(&text) {
            seen += 1;
            match lookup(&code) {
                None => {
                    unregistered.insert(format!("{code} ({})", path.display()));
                }
                Some(info) => {
                    if !docs.contains(&anchor(info.code)) {
                        unanchored.insert(code);
                    }
                }
            }
        }
    }
    assert!(
        seen > 100,
        "the scan found only {seen} code literals; the walk is probably broken"
    );
    assert!(
        unregistered.is_empty(),
        "these codes are emitted but not in the registry, so their `doc_url` \
         would not resolve: {unregistered:?}"
    );
    assert!(
        unanchored.is_empty(),
        "these emitted codes have no anchor in {DOC_PATH}: {unanchored:?}"
    );
}
