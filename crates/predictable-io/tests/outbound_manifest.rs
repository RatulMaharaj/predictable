//! Manifest behaviour: what the digest covers, what it deliberately does not, how the run
//! directory assembles, and what lineage and table copies record.

use std::collections::BTreeMap;
use std::fs;

use predictable_io::outbound::manifest::*;
use predictable_io::outbound::*;
use predictable_ir::run::{Emit, OnTrap, Retain, StoragePrecision};
use predictable_ir::{Component, DType, Expr, Kind, Shape};

fn schema() -> ResultsSchemaDoc {
    let mut bel = Component::derived("bel", DType::F64, Shape::Series, Expr::f64(0.0));
    bel.kind = Kind::Output;
    ResultsSchemaDoc::new("outputs", vec![ComponentDescriptor::from_ir("term", &bel)]).unwrap()
}

fn manifest() -> Manifest {
    Manifest::draft(
        "2026-08-17T09:14:22Z-3f0a",
        Versions {
            ir_version: "1.0".into(),
            engine_version: "0.4.1".into(),
            engine_git_sha: Some("a91c4de".into()),
            engine_build_profile: "release".into(),
            cli_version: Some("0.4.1".into()),
            dsl_version: None,
        },
        Inputs {
            model: ModelInputs {
                module: "term_assurance".into(),
                product: Some("TERM_UK".into()),
                digest: "sha256:9c11".into(),
                files: vec![
                    FileRef {
                        path: "models/term_assurance/model.pir".into(),
                        digest: "sha256:2e04".into(),
                    },
                    FileRef {
                        path: "models/term_assurance/schema.pir".into(),
                        digest: "sha256:71ab".into(),
                    },
                ],
            },
            assumptions: Some(AssumptionInputs {
                set: "base".into(),
                path: "assumptions/base.pir".into(),
                digest: "sha256:5d72".into(),
                values_digest: Some("sha256:8ab0".into()),
            }),
            modelpoints: ModelpointInputs {
                path: "data/term.mpf.parquet".into(),
                digest: "sha256:c4f9".into(),
                rows: 10_000,
                key_field: "policy_number".into(),
                source: None,
            },
            tables: vec![
                TableInput::new("sa8990", "tables/sa8990.csv", "sha256:9f2c"),
                TableInput::new("lapse_rates", "tables/lapses.csv", "sha256:31aa"),
            ],
        },
        TimelineInfo {
            basis: "annual".into(),
            periods: 40,
            origin: "policy".into(),
            valuation_date: Some("2026-06-30".into()),
            year_convention: Some("act/365".into()),
        },
        RunConfigInfo {
            path: "runs/base.pir".into(),
            digest: "sha256:4be7".into(),
            emit: Emit::All,
            outputs: vec!["bel".into()],
            retain: Retain::Ring,
            storage_precision: StoragePrecision::F64,
            on_trap: OnTrap::Abort,
            max_errors: 100,
            aggregations: vec!["by_product".into()],
            solves: vec![],
            exec: ExecInfo {
                threads: 8,
                chunk_size: 1024,
            },
            seed: None,
            flags: BTreeMap::from([("allow_table_drift".to_string(), false)]),
        },
        ResultsRef {
            path: "results.parquet".into(),
            digest: "sha256:e011".into(),
            rows: 19_680_000,
            components: 41,
            component_set_digest: "sha256:aa47".into(),
            aggregates: None,
        },
        Execution {
            outcome: Outcome::Completed,
            started_at: "2026-08-17T09:14:22Z".into(),
            finished_at: "2026-08-17T09:14:29Z".into(),
            wall_ms: 6981,
            modelpoints_projected: 10_000,
            modelpoints_trapped: 0,
            traps: vec![],
            warnings: vec![WarningCount {
                code: "W0102".into(),
                count: 3,
            }],
        },
        Provenance {
            invocation: "predictable run models/term_assurance -a base".into(),
            cwd_git: Some(GitInfo {
                repo: Some("predictable-models".into()),
                sha: Some("e4c1907".into()),
                dirty: false,
            }),
            user: None,
        },
    )
}

#[test]
fn build_stamps_both_digests_and_they_verify() {
    let m = manifest().build().unwrap();
    assert!(m.manifest_digest.starts_with("sha256:"));
    assert!(m.environment_hash.starts_with("sha256:"));
    assert!(m.verify_digest().unwrap());
    assert_eq!(m.format, "pvf/1");
    assert_eq!(m.kind, "manifest");
}

#[test]
fn the_digest_ignores_run_id_execution_results_digest_and_user() {
    // §6.3 rule 1: two runs with the same manifest_digest must produce byte-identical results,
    // so nothing that is an *outcome* may enter it.
    let base = manifest().build().unwrap();

    let mut other = manifest();
    other.run_id = "2099-01-01T00:00:00Z-ffff".into();
    other.execution.wall_ms = 999_999;
    other.execution.outcome = Outcome::Cancelled;
    other.results.digest = "sha256:different".into();
    other.provenance.user = Some("ratul".into());
    let other = other.build().unwrap();

    assert_eq!(base.manifest_digest, other.manifest_digest);
}

#[test]
fn the_digest_notices_anything_that_can_change_a_number() {
    let base = manifest().build().unwrap();
    for mutate in [
        (|m: &mut Manifest| m.inputs.model.digest = "sha256:other".into()) as fn(&mut Manifest),
        |m: &mut Manifest| m.inputs.modelpoints.digest = "sha256:other".into(),
        |m: &mut Manifest| m.inputs.tables[0].digest = "sha256:other".into(),
        |m: &mut Manifest| m.run_config.digest = "sha256:other".into(),
        |m: &mut Manifest| m.run_config.emit = Emit::Outputs,
        |m: &mut Manifest| m.timeline.periods = 41,
        |m: &mut Manifest| m.versions.engine_version = "0.4.2".into(),
        |m: &mut Manifest| m.results.component_set_digest = "sha256:other".into(),
        |m: &mut Manifest| m.lineage.label = Some("mortality +10%".into()),
    ] {
        let mut m = manifest();
        mutate(&mut m);
        let m = m.build().unwrap();
        assert_ne!(base.manifest_digest, m.manifest_digest);
    }
}

#[test]
fn environment_is_hashed_separately_so_same_inputs_different_machine_is_visible() {
    // §6.3 rule 2: this is the case the diff tool must call out first.
    let base = manifest().build().unwrap();
    let mut other = manifest();
    other.environment.arch = "x86_64".into();
    other.environment.target_triple = "x86_64-unknown-linux-gnu".into();
    let other = other.build().unwrap();

    assert_eq!(base.manifest_digest, other.manifest_digest);
    assert_ne!(base.environment_hash, other.environment_hash);
}

#[test]
fn arrays_are_sorted_and_keys_are_in_spec_order() {
    let m = manifest().build().unwrap();
    assert_eq!(
        m.inputs
            .tables
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["lapse_rates", "sa8990"]
    );
    assert!(m.inputs.model.files[0].path < m.inputs.model.files[1].path);

    let text = m.to_canonical_json().unwrap();
    let keys: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.split('"').nth(1).unwrap())
        .collect();
    assert_eq!(
        keys,
        vec![
            "format",
            "kind",
            "manifest_digest",
            "run_id",
            "system",
            "versions",
            "inputs",
            "timeline",
            "run_config",
            "lineage",
            "environment_hash",
            "environment",
            "results",
            "execution",
            "provenance"
        ]
    );
    assert!(text.ends_with("}\n") && !text.contains('\r'));
}

#[test]
fn sorting_happens_before_hashing() {
    // Declaring the same tables in the other order must not change the digest.
    let a = manifest().build().unwrap();
    let mut b = manifest();
    b.inputs.tables.reverse();
    b.inputs.model.files.reverse();
    let b = b.build().unwrap();
    assert_eq!(a.manifest_digest, b.manifest_digest);
}

#[test]
fn json_round_trips_and_the_digest_survives() {
    let m = manifest().build().unwrap();
    let back = Manifest::from_json(&m.to_canonical_json().unwrap()).unwrap();
    assert_eq!(back, m);
    assert!(back.verify_digest().unwrap());
}

#[test]
fn f32_storage_is_refused() {
    // Q15 / E0108.
    let mut m = manifest();
    m.run_config.storage_precision = StoragePrecision::F32;
    assert!(matches!(
        m.build().unwrap_err(),
        IoOutError::UnsupportedStoragePrecision
    ));
}

#[test]
fn lineage_records_a_sensitivity_child_and_varied_is_derivable() {
    // Q12: a fan is grouped by group_id, never by filename, and `varied` is always re-derivable
    // by diffing the two manifests.
    let parent = manifest().build().unwrap();

    let mut child = manifest();
    child.inputs.assumptions.as_mut().unwrap().digest = "sha256:mort110".into();
    child.run_config.digest = "sha256:run2".into();
    child.lineage = Lineage::child(
        &parent.run_id,
        &parent.manifest_digest,
        "sens-2026-06-30-mort",
        "mortality +10%",
        vec![VariedInput {
            path: "assumptions.base".into(),
            from: serde_json::json!(1.0),
            to: serde_json::json!(1.1),
        }],
    );
    let child = child.build().unwrap();

    assert!(child.lineage.is_child());
    assert_eq!(
        child.lineage.group_id.as_deref(),
        Some("sens-2026-06-30-mort")
    );
    assert_eq!(
        child.lineage.parent_manifest_digest,
        Some(parent.manifest_digest.clone())
    );
    assert_ne!(child.manifest_digest, parent.manifest_digest);

    let derived = Manifest::derive_varied(&parent, &child);
    let paths: Vec<&str> = derived.iter().map(|v| v.path.as_str()).collect();
    assert_eq!(paths, vec!["assumptions.base", "run"]);

    // A base run says so explicitly rather than by omission.
    assert!(!parent.lineage.is_child());
    let text = parent.to_canonical_json().unwrap();
    assert!(text.contains("\"parent_run\": null"), "{text}");
}

#[test]
fn outcome_is_load_bearing() {
    assert!(!Outcome::Completed.needs_banner());
    assert!(Outcome::CompletedWithTraps.needs_banner());
    assert!(Outcome::Cancelled.needs_banner());
    assert!(!Outcome::Aborted.has_results());
    assert!(Outcome::Cancelled.has_results());

    let mut m = manifest();
    m.execution.outcome = Outcome::CompletedWithTraps;
    let text = m.build().unwrap().to_canonical_json().unwrap();
    assert!(
        text.contains("\"outcome\": \"completed_with_traps\""),
        "{text}"
    );
}

#[test]
fn table_drift_is_computed_not_assumed() {
    let mut t = TableInput::new("sa8990", "tables/sa8990.csv", "sha256:9f2c");
    t.recompute_drift();
    assert!(!t.drift);
    t.digest = "sha256:changed".into();
    t.recompute_drift();
    assert!(t.drift);
}

/// The whole outbound path: results, aggregates, a solve, a table copy, then the manifest that
/// records all of their digests.
#[test]
fn a_full_run_directory_assembles() {
    let dir = tempfile::tempdir().unwrap();
    let run = RunDir::create(dir.path().join("run")).unwrap();

    let mut w = run
        .results_writer(schema(), WriterOptions::default())
        .unwrap();
    let mut chunk = ResultsChunk::new(0);
    chunk.push(ModelpointRows {
        offset: 0,
        mp_key: "POL0001".into(),
        mp_row: 0,
        cells: vec![
            Cell::f64("term.bel", 0, 1234.5),
            Cell::f64("term.bel", 1, 1300.0),
        ],
    });
    w.write_chunk(&chunk).unwrap();
    let results = w.finish().unwrap();
    assert_eq!(results.rows, 2);

    let aggregates = run
        .write_aggregates(&[AggregateRow {
            aggregation: "by_product".into(),
            group_key: "product_code=TERM_UK".into(),
            measure: "bel".into(),
            t: 0,
            value: 1234.5,
        }])
        .unwrap();

    let (solve_path, solve) = run
        .write_solve(
            "premium_solve",
            &[SolveRow {
                mp_key: "POL0001".into(),
                mp_row: 0,
                solved_value: 412.19,
                residual: 4.1e-9,
                iterations: 7,
                converged: true,
            }],
        )
        .unwrap();
    assert_eq!(solve_path, "solves/premium_solve.parquet");
    assert_eq!(solve.rows, 1);

    // Q13: the table travels with the result set, or the trace degrades to "some row".
    let (table_path, table_copy) = run
        .write_table_copy(&TableCopy {
            name: "sa8990".into(),
            columns: vec![
                ("age".into(), ColumnData::I64(vec![40, 41, 42])),
                ("qx".into(), ColumnData::F64(vec![0.0021, 0.0023, 0.0026])),
            ],
        })
        .unwrap();
    assert_eq!(table_path, "tables/sa8990.parquet");
    assert_eq!(table_copy.rows, 3);

    let mut m = manifest();
    m.results = ResultsRef {
        path: "results.parquet".into(),
        digest: results.digest.clone(),
        rows: results.rows,
        components: results.components,
        component_set_digest: results.component_set_digest.clone(),
        aggregates: Some(AggregatesRef {
            path: "aggregates.parquet".into(),
            digest: aggregates.digest.clone(),
            rows: aggregates.rows,
        }),
    };
    m.inputs.tables = vec![
        TableInput::new("sa8990", "tables/sa8990.csv", "sha256:9f2c").with_copy(
            &table_path,
            &table_copy.digest,
            table_copy.rows,
        ),
    ];
    let m = run.write_manifest(m).unwrap();

    for f in [
        "manifest.json",
        "results.parquet",
        "results.schema.json",
        "aggregates.parquet",
        "solves/premium_solve.parquet",
        "tables/sa8990.parquet",
    ] {
        assert!(run.path(f).exists(), "{f} missing");
    }

    // The manifest on disk is the manifest we built, and its digest still verifies.
    let on_disk =
        Manifest::from_json(&fs::read_to_string(run.path("manifest.json")).unwrap()).unwrap();
    assert_eq!(on_disk, m);
    assert!(on_disk.verify_digest().unwrap());
    assert_eq!(on_disk.results.digest, results.digest);
    assert_eq!(
        on_disk.inputs.tables[0].copy.as_deref(),
        Some("tables/sa8990.parquet")
    );
    assert_eq!(
        on_disk.inputs.tables[0].copy_digest,
        Some(table_copy.digest)
    );

    // results.schema.json is written even though nothing asked for it separately.
    let schema_text = fs::read_to_string(run.path("results.schema.json")).unwrap();
    assert!(schema_text.contains("\"term.bel\""));
}

#[test]
fn no_table_copy_says_so_rather_than_pretending() {
    // Q13: a consumer that finds `copy: null` must say "table content unavailable".
    let mut m = manifest();
    m.inputs.tables = vec![TableInput::new(
        "sa8990",
        "tables/sa8990.csv",
        "sha256:9f2c",
    )];
    let m = m.build().unwrap();
    assert_eq!(m.inputs.tables[0].copy, None);
    let text = m.to_canonical_json().unwrap();
    assert!(text.contains("\"copy\": null"), "{text}");
}
