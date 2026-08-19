//! The wasm build, natively: does it produce the numbers the CLI produced?
//!
//! `03-engine.md` §9's whole claim is that the browser runs the *same* engine.
//! The cheapest way for that claim to rot is for this crate's own halves — the
//! CSV reader that replaces `predictable-io`, the resolver that replaces
//! `FsResolver` — to drift from theirs while the kernel stays honest. So the
//! oracle here is not a snapshot of this crate's output: it is
//! `models/term_annual/expected/`, the files T22 generated through the native
//! pipeline and T16's goldens pin. Bit equality, not a tolerance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_wasm::session::{Inputs, Session, SessionError, SourceFile, TableBytes};

fn model_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/term_annual")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `term_annual`, loaded the way a governance pack loads: text only, no paths.
fn term_annual() -> Inputs {
    let dir = model_dir();
    let mut sources = Vec::new();
    for name in ["build/model.pir", "build/product.pir", "build/schema.pir"] {
        sources.push(SourceFile {
            name: name.to_string(),
            text: read(&dir.join(name)),
        });
    }
    let mut tables = Vec::new();
    for name in [
        "tables/expenses.csv",
        "tables/lapses.csv",
        "tables/mortality.csv",
    ] {
        tables.push(TableBytes {
            name: name.to_string(),
            text: read(&dir.join(name)),
        });
    }
    Inputs {
        sources,
        assumptions: assumptions(&read(&dir.join("base.pir"))),
        tables,
        modelpoints: read(&dir.join("data/modelpoints.csv")),
        allow_table_drift: false,
    }
}

/// The assumption file is `.pir`; the pack embeds its values, so the test reads
/// them the same trivial way the exporter does.
fn assumptions(text: &str) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if let Ok(value) = value.trim().parse::<f64>() {
            out.insert(name.trim().to_string(), value);
        }
    }
    out
}

#[test]
fn a_pack_session_reproduces_the_native_run_bit_for_bit() {
    let session = Session::new(&term_annual()).expect("term_annual checks and plans");
    let projection = session.project(&[], &BTreeMap::new()).expect("it projects");

    let expected = read(&model_dir().join("expected/series_TA00001.csv"));
    let mut lines = expected.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let lane = projection
        .keys
        .iter()
        .position(|k| k == "TA00001")
        .expect("TA00001 survived");

    let mut compared = 0usize;
    for line in lines {
        let cells: Vec<&str> = line.split(',').collect();
        let t: usize = cells[0].parse().unwrap();
        for (column, cell) in header.iter().zip(&cells).skip(1) {
            let want: f64 = cell.parse().unwrap();
            let stride = projection.strides[*column];
            let got = projection.columns[*column][lane * stride + t];
            assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "{column} at t={t}: wasm build got {got}, the native run wrote {want}"
            );
            compared += 1;
        }
    }
    assert!(compared > 200, "the oracle covered only {compared} cells");
}

#[test]
fn per_mp_outputs_match_the_native_run_too() {
    let session = Session::new(&term_annual()).unwrap();
    let projection = session.project(&[], &BTreeMap::new()).unwrap();
    let expected = read(&model_dir().join("expected/per_mp.csv"));

    let mut checked = 0usize;
    for line in expected.lines().skip(1) {
        let cells: Vec<&str> = line.split(',').collect();
        let (key, component, want) = (cells[0], cells[1], cells[2].parse::<f64>().unwrap());
        let Some(values) = projection.columns.get(component) else {
            continue;
        };
        let stride = projection.strides[component];
        // A `PerMP` output has one value per lane; a series is compared above.
        if stride != 1 {
            continue;
        }
        let lane = projection.keys.iter().position(|k| k == key).unwrap();
        assert_eq!(
            values[lane].to_bits(),
            want.to_bits(),
            "{component} for {key}"
        );
        checked += 1;
    }
    assert!(
        checked > 20,
        "only {checked} per-modelpoint values compared"
    );
}

#[test]
fn chunking_cannot_change_a_number() {
    // §7 clause 1, restated for the browser: the page picks its own `C`.
    let session = Session::new(&term_annual()).unwrap();
    let all = session.project(&[], &BTreeMap::new()).unwrap();
    let one = session
        .project(&["TA00001".to_string()], &BTreeMap::new())
        .unwrap();
    let lane = all.keys.iter().position(|k| k == "TA00001").unwrap();
    for (name, values) in &one.columns {
        let stride = one.strides[name];
        assert_eq!(
            &values[..stride],
            &all.columns[name][lane * stride..(lane + 1) * stride],
            "`{name}` differs between a 25-lane chunk and a 1-lane chunk"
        );
    }
}

#[test]
fn explain_replays_the_cell_the_projection_stored() {
    // The E0901 assertion lives inside `Explainer::explain`: a trace whose
    // replay disagrees with the kernel is an error, not a rendering. So a
    // successful trace *is* the assertion — plus this test checks the value it
    // reports is the one `expected/` records.
    let session = Session::new(&term_annual()).unwrap();
    let trace = session
        .explain(
            "model.reserve",
            "TA00001",
            Some(0),
            &Default::default(),
            &BTreeMap::new(),
        )
        .expect("model.reserve at t=0 traces");
    let json = serde_json::to_value(&trace).unwrap();
    assert_eq!(json["kind"], "trace");
    let value = json["root"]["value"].as_f64().unwrap();
    assert_eq!(value.to_bits(), (-223.20202973973744_f64).to_bits());
}

#[test]
fn explain_names_the_component_it_cannot_find() {
    let session = Session::new(&term_annual()).unwrap();
    let err = session
        .explain(
            "model.nope",
            "TA00001",
            Some(0),
            &Default::default(),
            &BTreeMap::new(),
        )
        .unwrap_err();
    assert!(matches!(err, SessionError::UnknownComponent(_)), "{err}");
}

#[test]
fn an_unknown_modelpoint_is_named_not_swallowed() {
    let session = Session::new(&term_annual()).unwrap();
    let err = session
        .project(&["ZZ99999".to_string()], &BTreeMap::new())
        .unwrap_err();
    assert!(matches!(err, SessionError::UnknownModelpoint(_)), "{err}");
}

#[test]
fn a_drifted_table_is_refused_by_default() {
    let mut inputs = term_annual();
    let table = inputs
        .tables
        .iter_mut()
        .find(|t| t.name.ends_with("mortality.csv"))
        .unwrap();
    // One digit, one table, one refusal: a pack whose bytes no longer hash to
    // the declared digest cannot reproduce its own numbers.
    table.text = table.text.replacen("0.0", "0.9", 1);
    let err = Session::new(&inputs).unwrap_err();
    assert!(err.to_string().contains("mortality"), "{err}");
}

#[test]
fn a_model_that_does_not_check_carries_the_checkers_diagnostics() {
    let mut inputs = term_annual();
    inputs.sources[0].text = inputs.sources[0]
        .text
        .replace("expr = \"entry_age + t\"", "expr = \"entry_age + nope\"");
    match Session::new(&inputs) {
        Err(SessionError::Check(diags)) => {
            assert!(!diags.is_empty());
            assert!(
                diags.iter().any(|d| d.message.contains("nope")),
                "the page shows the CLI's message, not a paraphrase: {diags:?}"
            );
        }
        other => panic!("expected a check failure, got {other:?}"),
    }
}

#[test]
fn a_scenario_moves_the_answer_and_the_base_is_untouched() {
    // The fan's precondition (§3.3): each band is a real run of its own.
    let session = Session::new(&term_annual()).unwrap();
    let base = session
        .project(&["TA00001".to_string()], &BTreeMap::new())
        .unwrap();
    let mut set = BTreeMap::new();
    set.insert("mortality_loading".to_string(), 1.5);
    let bumped = session.project(&["TA00001".to_string()], &set).unwrap();
    assert_ne!(base.columns["model.bel"], bumped.columns["model.bel"]);

    let again = session
        .project(&["TA00001".to_string()], &BTreeMap::new())
        .unwrap();
    assert_eq!(base.columns["model.bel"], again.columns["model.bel"]);
}
