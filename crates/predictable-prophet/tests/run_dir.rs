//! A Prophet run is a first-class run directory (`04-verify.md` §4.3.7, §6.3 rule 5).

#![cfg(feature = "run-dir")]

use predictable_prophet::{read_rpt, write_run_dir, ImportError, ImportOptions, RptOptions};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn writes_results_parquet_and_a_prophet_manifest() {
    let bytes = fixture("term_run.rpt");
    let read = read_rpt(
        &bytes,
        "prophet/term_run.rpt",
        &RptOptions {
            period_base: Some(1),
            ..RptOptions::default()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_run_dir(
        &read.value,
        dir.path(),
        &bytes,
        &ImportOptions {
            module: Some("term_assurance".into()),
            ..ImportOptions::default()
        },
    )
    .unwrap();

    // The three files `predictable diff` expects of any run.
    for f in ["manifest.json", "results.parquet", "results.schema.json"] {
        assert!(dir.path().join(f).exists(), "missing {f}");
    }

    assert_eq!(manifest.system, "prophet");
    assert_eq!(manifest.kind, "manifest");
    assert_eq!(manifest.run_id, "TERM_BASE_2026Q2");
    assert_eq!(manifest.results.rows, 6 * 4);
    assert_eq!(manifest.results.components, 4);
    assert!(manifest.results.digest.starts_with("sha256:"));
    assert!(!manifest.manifest_digest.is_empty());
    assert!(manifest.verify_digest().unwrap());

    // Provenance: the `.rpt` and its digest, not an invented model input.
    let source = manifest.inputs.modelpoints.source.as_ref().unwrap();
    assert_eq!(source.system, "prophet");
    assert_eq!(
        source.digest,
        format!("sha256:{}", predictable_prophet::sha256_hex(&bytes))
    );
    assert_eq!(manifest.inputs.modelpoints.key_field, "POL_NUM");
    assert_eq!(manifest.inputs.modelpoints.rows, 2);
    assert_eq!(manifest.inputs.model.module, "term_assurance");
    assert_eq!(manifest.inputs.model.product.as_deref(), Some("TERM_UK"));

    // Absent provenance is stated, never implied.
    assert!(manifest.versions.engine_git_sha.is_none());
    assert!(manifest.versions.cli_version.is_none());
    assert!(manifest.provenance.cwd_git.is_none());
    assert_eq!(manifest.environment.target_triple, "unknown");
    assert_eq!(manifest.timeline.basis, "months");

    // The manifest on disk is the canonical JSON of what we were handed back.
    let text = std::fs::read_to_string(dir.path().join("manifest.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["system"], "prophet");
    assert_eq!(parsed["manifest_digest"], manifest.manifest_digest);
    assert!(text.ends_with('\n'));
}

#[test]
fn the_component_descriptors_state_what_prophet_did_not() {
    let bytes = fixture("term_run.rpt");
    let read = read_rpt(
        &bytes,
        "prophet/term_run.rpt",
        &RptOptions {
            period_base: Some(1),
            ..RptOptions::default()
        },
    );
    let descriptors = predictable_prophet::run_dir::descriptors(&read.value);
    assert_eq!(descriptors.len(), 4);
    for d in &descriptors {
        // A `.rpt` never says whether a column is start-, mid- or end-of-period,
        // and inventing a timing would silently justify a timing shift.
        assert_eq!(d.timing, None);
        assert_eq!(d.unit, predictable_ir::Unit::None);
        assert_eq!(d.stage, 1);
        assert!(d.output);
    }
}

#[test]
fn an_unresolved_period_base_refuses_to_become_a_run() {
    let bytes = fixture("term_run.rpt");
    let read = read_rpt(&bytes, "prophet/term_run.rpt", &RptOptions::default());
    let dir = tempfile::tempdir().unwrap();
    let err =
        write_run_dir(&read.value, dir.path(), &bytes, &ImportOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::UnresolvedPeriodBase { .. }));
    assert!(err.to_string().contains("P0302"));
}
