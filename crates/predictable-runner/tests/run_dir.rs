//! A whole run on disk: `[[solve]]` (Q8), the run directory, and the manifest contract —
//! `run_digest` (Q1), lineage (Q12), table copies (Q13) and the trap policy (Q7).

mod common;

use std::collections::BTreeMap;

use common::{model, runner, term_chunks, TERM};
use predictable_io::outbound::manifest::{ModelInputs, ModelpointInputs};
use predictable_io::outbound::{
    ColumnData, Inputs, Lineage, Manifest, Outcome, Provenance, TableCopy, TableInput, Versions,
};
use predictable_ir::run::{OnTrap, Solve, SolveScope};
use predictable_runner::run::{execute, RunSpec};
use predictable_runner::{minimal_run_file, CancelFlag, SerialExecutor};

const RUN_PIR: &str = r#"format = "pir/1"

[run]
product = "products/term_uk"
modelpoints = "data/term.csv"
out = "runs/2026-06-30-base"

[run.exec]
threads = 4
chunk_size = 2
"#;

fn spec(table_copies: Vec<TableCopy>, no_table_copy: bool) -> RunSpec {
    RunSpec {
        run_id: "2026-08-17T09:14:22Z-3f0a".to_string(),
        run_path: "runs/base.pir".to_string(),
        run_source: RUN_PIR.to_string(),
        inputs: Inputs {
            model: ModelInputs {
                module: "term".to_string(),
                product: Some("products/term_uk".to_string()),
                digest: "sha256:aa".to_string(),
                files: vec![],
            },
            assumptions: None,
            modelpoints: ModelpointInputs {
                path: "data/term.csv".to_string(),
                digest: "sha256:bb".to_string(),
                rows: 4,
                key_field: "policy_number".to_string(),
                source: None,
            },
            tables: vec![TableInput::new("sa8990", "tables/sa8990.csv", "sha256:cc")],
        },
        versions: Versions {
            ir_version: "1.0".to_string(),
            engine_version: "0.0.1".to_string(),
            engine_git_sha: None,
            engine_build_profile: "debug".to_string(),
            cli_version: None,
            dsl_version: None,
        },
        provenance: Provenance {
            invocation: "predictable run runs/base.pir".to_string(),
            cwd_git: None,
            user: None,
        },
        lineage: Lineage::base(),
        table_copies,
        no_table_copy,
        started_at: "2026-08-17T09:14:22Z".to_string(),
        finished_at: "2026-08-17T09:14:23Z".to_string(),
        wall_ms: 1000,
    }
}

fn premium_solve() -> Solve {
    Solve {
        name: "premium_solve".to_string(),
        target: "bel".to_string(),
        to: 0.0,
        vary: "premium".to_string(),
        scope: SolveScope::PerMp,
        tolerance: 1e-9,
        max_iter: 60,
        method: "brent".to_string(),
        bracket: Some([0.0, 1.0e6]),
        on_not_converged: None,
    }
}

#[test]
fn a_plain_run_writes_a_verifiable_run_directory() {
    let dir = tempfile::tempdir().unwrap();
    let m = model(TERM);
    let run = minimal_run_file();
    let mut r = runner(&m, &run);
    let report = execute(
        &mut r,
        term_chunks(2),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir.path(),
    )
    .unwrap();

    assert_eq!(report.outcome, Outcome::Completed);
    assert_eq!(report.exit_code, 0);
    for file in ["manifest.json", "results.parquet", "results.schema.json"] {
        assert!(dir.path().join(file).exists(), "missing {file}");
    }

    // The manifest re-derives its own digest — what `predictable rerun` checks before trusting it.
    assert!(report.manifest.verify_digest().unwrap());
    let text = std::fs::read_to_string(dir.path().join("manifest.json")).unwrap();
    let reread = Manifest::from_json(&text).unwrap();
    assert_eq!(reread.manifest_digest, report.manifest.manifest_digest);

    // Q1: `run_digest` is over the run file's canonical text, excluding `[run.exec]` and `out`.
    let expected = predictable_fmt::digest::run_digest("runs/base.pir", RUN_PIR).unwrap();
    assert_eq!(reread.run_config.digest, expected);

    // 4 modelpoints × (3 outputs: claims + net_cashflow over T+1, bel once).
    let results = report.results.unwrap();
    assert_eq!(results.modelpoints, 4);
    assert_eq!(results.rows, 4 * (4 + 4 + 1));
    assert_eq!(
        reread.results.component_set_digest,
        results.component_set_digest
    );
    assert_eq!(reread.execution.modelpoints_projected, 4);
    assert_eq!(reread.execution.outcome, Outcome::Completed);
}

#[test]
fn a_per_mp_solve_hits_its_target_and_records_how() {
    let dir = tempfile::tempdir().unwrap();
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.solves = vec![premium_solve()];
    let mut r = runner(&m, &run);

    let report = execute(
        &mut r,
        term_chunks(2),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir.path(),
    )
    .unwrap();

    // Every modelpoint's `bel` is zero to tolerance in the final, solved projection.
    for (key, bel) in report.projection.per_mp("term.bel") {
        assert!(bel.abs() < 1e-6, "{key}: bel = {bel}");
    }

    // Q8's manifest contract, in full.
    let block = &report.manifest.run_config.solves[0];
    assert_eq!(block.name, "premium_solve");
    assert_eq!(block.scope, "per_mp");
    assert_eq!(block.method, "brent");
    assert_eq!(block.converged, 4);
    assert_eq!(block.not_converged, 0);
    assert!(block.iterations.total >= 4);
    assert!(block.iterations.min >= 1 && block.iterations.max <= 60);
    assert!(block.residual.max_abs <= 1e-6);
    assert!(block.residual.argmax_mp.is_some());
    let per_mp = block.per_mp.as_ref().expect("solves/<name>.parquet");
    assert_eq!(per_mp.path, "solves/premium_solve.parquet");
    assert!(dir.path().join(&per_mp.path).exists());

    // The solved premium is a real premium: bigger sums assured need bigger premiums.
    let solved = &report.solves[0].solved;
    assert!(solved["POL2"] > solved["POL1"]);
    assert!(solved.values().all(|v| *v > 0.0 && *v < 1.0e6));
}

#[test]
fn a_solve_that_cannot_bracket_a_root_says_so_rather_than_guessing() {
    let dir = tempfile::tempdir().unwrap();
    let m = model(TERM);
    let mut run = minimal_run_file();
    let mut solve = premium_solve();
    // A bracket well above the root: no sign change, so no root to find.
    solve.bracket = Some([1.0e5, 1.0e6]);
    run.solves = vec![solve];
    let mut r = runner(&m, &run);

    let report = execute(
        &mut r,
        term_chunks(4),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir.path(),
    )
    .unwrap();
    let block = &report.manifest.run_config.solves[0];
    assert_eq!(block.converged, 0);
    assert_eq!(block.not_converged, 4);
    // `on_not_converged` defaults to `warn` for a per-mp solve, so the run still completes.
    assert_eq!(block.on_not_converged, "warn");
    assert_eq!(report.exit_code, 0);
}

#[test]
fn an_aborted_run_writes_a_manifest_and_no_results() {
    let dir = tempfile::tempdir().unwrap();
    let m = model(common::TRAPPING);
    let run = minimal_run_file();
    let mut r = runner(&m, &run);
    let chunks = vec![common::chunk(
        0,
        0,
        &["POL1", "POL2"],
        &[
            ("numerator", common::nums(&[1.0, 2.0])),
            ("exposure", common::nums(&[1.0, 0.0])),
        ],
    )];

    let report = execute(
        &mut r,
        chunks,
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir.path(),
    )
    .unwrap();

    assert_eq!(report.outcome, Outcome::Aborted);
    assert_eq!(report.exit_code, 2);
    assert!(report.results.is_none());
    assert!(dir.path().join("manifest.json").exists());
    assert!(!dir.path().join("results.parquet").exists());
    // The failure is as documented as a success would have been.
    assert_eq!(report.manifest.execution.traps.len(), 1);
    assert_eq!(report.manifest.execution.traps[0]["code"], "E0902");
    assert_eq!(report.manifest.execution.outcome, Outcome::Aborted);
}

#[test]
fn continue_writes_results_without_the_trapping_modelpoint() {
    let dir = tempfile::tempdir().unwrap();
    let m = model(common::TRAPPING);
    let mut run = minimal_run_file();
    run.run.on_trap = OnTrap::Continue;
    let mut r = runner(&m, &run);
    let chunks = vec![common::chunk(
        0,
        0,
        &["POL1", "POL2", "POL3"],
        &[
            ("numerator", common::nums(&[1.0, 2.0, 3.0])),
            ("exposure", common::nums(&[1.0, 0.0, 4.0])),
        ],
    )];

    let report = execute(
        &mut r,
        chunks,
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir.path(),
    )
    .unwrap();

    assert_eq!(report.outcome, Outcome::CompletedWithTraps);
    assert_eq!(report.exit_code, 1);
    let results = report.results.unwrap();
    assert_eq!(results.modelpoints, 2, "no rows at all for the trapped one");
    assert_eq!(report.manifest.execution.modelpoints_trapped, 1);
}

#[test]
fn table_content_travels_with_the_result_set_unless_it_is_suppressed() {
    let m = model(TERM);
    let run = minimal_run_file();

    // Q13: the copy is written and its digest recorded.
    let dir = tempfile::tempdir().unwrap();
    let mut r = runner(&m, &run);
    let copy = TableCopy {
        name: "sa8990".to_string(),
        columns: vec![
            ("age".to_string(), ColumnData::I64(vec![30, 31, 32])),
            (
                "qx".to_string(),
                ColumnData::F64(vec![0.0011, 0.0012, 0.0013]),
            ),
        ],
    };
    let report = execute(
        &mut r,
        term_chunks(4),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![copy], false),
        dir.path(),
    )
    .unwrap();
    let table = &report.manifest.inputs.tables[0];
    assert_eq!(table.copy.as_deref(), Some("tables/sa8990.parquet"));
    assert!(table.copy_digest.as_deref().unwrap().starts_with("sha256:"));
    assert_eq!(table.rows, Some(3));
    assert!(dir.path().join("tables/sa8990.parquet").exists());

    // `--no-table-copy` stamps `copy: null`, so a consumer must say the content is unavailable.
    let dir2 = tempfile::tempdir().unwrap();
    let mut r2 = runner(&m, &run);
    let report2 = execute(
        &mut r2,
        term_chunks(4),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir2.path(),
    )
    .unwrap();
    assert!(report2.manifest.inputs.tables[0].copy.is_none());
    assert!(!dir2.path().join("tables").join("sa8990.parquet").exists());
}

#[test]
fn lineage_records_a_sensitivity_child_rather_than_leaving_it_to_the_filename() {
    let m = model(TERM);
    let run = minimal_run_file();

    let base_dir = tempfile::tempdir().unwrap();
    let mut r = runner(&m, &run);
    let base = execute(
        &mut r,
        term_chunks(4),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        base_dir.path(),
    )
    .unwrap();
    assert!(!base.manifest.lineage.is_child());

    let child_dir = tempfile::tempdir().unwrap();
    let mut child_spec = spec(vec![], true);
    child_spec.run_id = "2026-08-17T09:20:00Z-9c11".to_string();
    child_spec.inputs.modelpoints.digest = "sha256:dd".to_string();
    child_spec.lineage = Lineage::child(
        base.manifest.run_id.clone(),
        base.manifest.manifest_digest.clone(),
        "sens-2026-06-30-mort",
        "mortality +10%",
        vec![],
    );
    let mut r2 = runner(&m, &run);
    let child = execute(
        &mut r2,
        term_chunks(4),
        &SerialExecutor,
        &CancelFlag::new(),
        &child_spec,
        child_dir.path(),
    )
    .unwrap();

    assert!(child.manifest.lineage.is_child());
    assert_eq!(
        child.manifest.lineage.group_id.as_deref(),
        Some("sens-2026-06-30-mort")
    );
    // Q12: `varied` is always derivable from the two manifests, never only asserted.
    let varied = Manifest::derive_varied(&base.manifest, &child.manifest);
    assert_eq!(varied.len(), 1);
    assert_eq!(varied[0].path, "modelpoints");
}

#[test]
fn a_run_with_no_aggregations_writes_no_aggregates_file() {
    let dir = tempfile::tempdir().unwrap();
    let m = model(TERM);
    let run = minimal_run_file();
    let mut r = runner(&m, &run);
    let report = execute(
        &mut r,
        term_chunks(4),
        &SerialExecutor,
        &CancelFlag::new(),
        &spec(vec![], true),
        dir.path(),
    )
    .unwrap();
    assert!(!dir.path().join("aggregates.parquet").exists());
    assert!(report.manifest.results.aggregates.is_none());
    let _ = BTreeMap::<String, f64>::new();
}
