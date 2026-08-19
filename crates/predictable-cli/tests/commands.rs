//! End-to-end behaviour of the command surface: `check`, `build`, `run`, `graph`,
//! `export`, `rerun` and the phase-2 stubs.
//!
//! Every test drives [`predictable_cli::dispatch`] directly rather than spawning
//! the binary, so a failure points at a line of Rust instead of at a process.
//! What is asserted is the *contract* of `04-verify.md` §7 — the exit code, the
//! `pvf/1` document, and the artefacts on disk — never the prose.

use std::path::{Path, PathBuf};

use predictable_cli::dispatch;
use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// A model with an unresolvable name: `pols` is not declared anywhere.
const BROKEN: &str = r#"format = "pir/1"
module = "broken"

[[component]]
name = "claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "pols * 0.01"
"#;

fn cli(args: &[&str]) -> (i32, String, String) {
    let argv: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    dispatch(&argv)
}

fn json_of(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON: {e}\n{stdout}"))
}

/// A scratch copy of the fixture directory, so tests may mutate it.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("predictable-cli-cmd-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["term.pir", "mp.csv", "run.pir"] {
        std::fs::copy(Path::new(FIXTURES).join(name), dir.join(name)).unwrap();
    }
    dir
}

fn p(dir: &Path, name: &str) -> String {
    dir.join(name).display().to_string()
}

// ---------------------------------------------------------------------------
// the two global contracts: exit codes and --json
// ---------------------------------------------------------------------------

#[test]
fn every_command_accepts_json_and_writes_one_pvf_document() {
    let dir = scratch("json-everywhere");
    let out = p(&dir, "out");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    let cases: Vec<Vec<String>> = vec![
        vec!["check".into(), p(&dir, "term.pir")],
        vec!["fmt".into(), "--check".into(), p(&dir, "term.pir")],
        vec!["digest".into(), p(&dir, "term.pir")],
        vec!["build".into(), p(&dir, "term.pir")],
        vec!["graph".into(), p(&dir, "term.pir")],
        vec![
            "export".into(),
            out.clone(),
            "--out".into(),
            p(&dir, "pack.html"),
        ],
        vec![
            "rerun".into(),
            format!("{out}/manifest.json"),
            "--verify-only".into(),
        ],
        vec![
            "explain".into(),
            out.clone(),
            "--component".into(),
            "bel".into(),
            "--mp".into(),
            "POL1".into(),
        ],
        vec!["diff".into()],
        vec!["migrate".into()],
    ];

    for mut argv in cases {
        let command = argv[0].clone();
        argv.push("--json".into());
        let (_, stdout, _) = dispatch(&argv);
        let doc = json_of(&stdout);
        assert_eq!(
            doc["format"], "pvf/1",
            "{command} has no pvf/1 format field"
        );
        // `explain` writes the trace document of `04-verify.md` §3.2, whose kind
        // is `trace`: the artefact is the trace, not a report about one.
        let expected = if command == "explain" {
            "trace"
        } else {
            command.as_str()
        };
        assert_eq!(doc["kind"], expected, "{command} mislabels its kind");
    }
}

#[test]
fn out_json_writes_the_same_document_to_a_file() {
    let dir = scratch("out-json");
    let target = p(&dir, "check.json");
    let (code, _, _) = cli(&["check", &p(&dir, "term.pir"), "--out-json", &target]);
    assert_eq!(code, 0);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(doc["kind"], "check");
    assert_eq!(doc["summary"]["errors"], 0);
}

#[test]
fn usage_failures_are_exit_2_and_never_silently_defaulted() {
    assert_eq!(cli(&["frobnicate"]).0, 2);
    assert_eq!(cli(&["check", "--nonsense", "x.pir"]).0, 2);
    assert_eq!(cli(&["check"]).0, 2);
    assert_eq!(cli(&["run", "no-such-file.pir"]).0, 2);
    assert_eq!(cli(&["export", "no-such-run"]).0, 2);
    assert_eq!(cli(&["--help"]).0, 0);
    assert_eq!(cli(&["--version"]).0, 0);
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

#[test]
fn check_separates_clean_from_lints_from_errors() {
    let dir = scratch("check-codes");
    // Clean.
    let (code, _, _) = cli(&["check", &p(&dir, "term.pir")]);
    assert_eq!(code, 0);

    // Errors: exit 2, and the diagnostics travel in the document.
    let broken = p(&dir, "broken.pir");
    std::fs::write(&broken, BROKEN).unwrap();
    let (code, stdout, _) = cli(&["check", &broken, "--json"]);
    assert_eq!(code, 2);
    let doc = json_of(&stdout);
    assert!(doc["summary"]["errors"].as_u64().unwrap() >= 1);
    let codes: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.iter().any(|c| c.starts_with('E')), "{codes:?}");

    // A lint alone is exit 1: the model still runs.
    let lint = p(&dir, "lint.pir");
    let text = std::fs::read_to_string(p(&dir, "term.pir"))
        .unwrap()
        .replace(
            "unit = \"money\"\ntiming = \"end\"\nexpr = \"survivors * premium\"",
            "unit = \"money\"\ntiming = \"start\"\nexpr = \"survivors * premium\"",
        );
    std::fs::write(&lint, text).unwrap();
    let (code, stdout, _) = cli(&["check", &lint, "--json"]);
    assert_eq!(code, 1, "{stdout}");
    let doc = json_of(&stdout);
    assert_eq!(doc["summary"]["errors"], 0);
    assert!(doc["summary"]["warnings"].as_u64().unwrap() >= 1);
}

#[test]
fn a_human_reading_check_sees_the_rendered_snippet_and_an_agent_does_not() {
    let dir = scratch("check-render");
    let broken = p(&dir, "broken.pir");
    std::fs::write(&broken, BROKEN).unwrap();

    let (_, _, stderr) = cli(&["check", &broken]);
    assert!(stderr.contains("broken.pir"), "{stderr}");
    assert!(stderr.contains("expr = \"pols * 0.01\""), "{stderr}");

    // Under --json the snippet is not duplicated as prose; the document is the output.
    let (_, stdout, stderr) = cli(&["check", &broken, "--json"]);
    assert!(!stderr.contains("^^^"), "{stderr}");
    assert!(stdout.contains("\"diagnostics\""));
}

// ---------------------------------------------------------------------------
// build
// ---------------------------------------------------------------------------

#[test]
fn build_reports_the_three_digests_and_writes_a_plan() {
    let dir = scratch("build");
    let plan = p(&dir, "plan.json");
    let (code, stdout, _) = cli(&["build", &p(&dir, "term.pir"), "--json", "--out", &plan]);
    assert_eq!(code, 0);
    let doc = json_of(&stdout);
    for key in ["model_digest", "plan_digest", "order_digest", "tape_digest"] {
        assert!(
            doc[key].as_str().unwrap().len() > 8,
            "{key} missing: {stdout}"
        );
    }
    assert_eq!(doc["periods"], 3);
    assert_eq!(doc["slots"]["outputs"], 3);
    // The order is the planner's, and it is a total order over every slot.
    let order = doc["order"].as_array().unwrap();
    assert!(order.iter().any(|n| n == "survivors"));
    let survivors = order.iter().position(|n| n == "survivors").unwrap();
    let claims = order.iter().position(|n| n == "claims").unwrap();
    assert!(survivors < claims, "claims reads survivors: {order:?}");

    let written: Value = serde_json::from_str(&std::fs::read_to_string(&plan).unwrap()).unwrap();
    assert_eq!(written["kind"], "plan");
    assert_eq!(written["plan"]["periods"], 3);
}

#[test]
fn build_refuses_an_unchecked_model_rather_than_half_planning_it() {
    let dir = scratch("build-refuse");
    let broken = p(&dir, "broken.pir");
    std::fs::write(&broken, BROKEN).unwrap();
    let (code, stdout, _) = cli(&["build", &broken, "--json"]);
    assert_eq!(code, 2);
    assert_eq!(json_of(&stdout)["status"], "not_checked");
}

#[test]
fn o0_and_o1_are_different_plans_and_say_so() {
    let dir = scratch("opt");
    let (_, a, _) = cli(&["build", &p(&dir, "term.pir"), "--json"]);
    let (_, b, _) = cli(&["build", &p(&dir, "term.pir"), "--json", "--O0"]);
    let (a, b) = (json_of(&a), json_of(&b));
    assert_eq!(a["opt"], "O1");
    assert_eq!(b["opt"], "O0");
    assert_ne!(a["plan_digest"], b["plan_digest"]);
    // The *model* is the same either way: only the plan changed.
    assert_eq!(a["model_digest"], b["model_digest"]);
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[test]
fn run_writes_a_verifiable_run_directory() {
    let dir = scratch("run");
    let out = p(&dir, "out");
    let (code, stdout, _) = cli(&["run", &p(&dir, "run.pir"), "--out", &out, "--json"]);
    assert_eq!(code, 0, "{stdout}");
    let doc = json_of(&stdout);
    assert_eq!(doc["outcome"], "completed");
    assert_eq!(doc["modelpoints"]["rows"], 4);
    assert_eq!(doc["modelpoints"]["projected"], 4);
    // 4 modelpoints × (claims and net_cashflow over t = 0..3, plus one bel).
    assert_eq!(doc["results"]["rows"], 4 * (4 + 4 + 1));
    assert_eq!(doc["results"]["components"], 3);

    for file in ["manifest.json", "results.parquet", "results.schema.json"] {
        assert!(Path::new(&out).join(file).exists(), "missing {file}");
    }

    // The manifest re-derives its own digest — the property `rerun` depends on.
    let (code, _, _) = cli(&["rerun", &format!("{out}/manifest.json"), "--verify-only"]);
    assert_eq!(code, 0);

    // Every digest the manifest promises is present and pinned.
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&out).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["inputs"]["model"]["digest"], doc["model_digest"]);
    assert_eq!(manifest["run_config"]["digest"], doc["run_digest"]);
    assert!(manifest["inputs"]["modelpoints"]["digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
}

#[test]
fn the_executor_is_a_scheduling_choice_never_a_numerical_one() {
    let dir = scratch("determinism-threads");
    // Same chunking, different thread counts: pure scheduling, so `results.parquet`
    // must be byte-identical — that is what `run(threads = 1) ≡ run(threads = 64)`
    // means, and a digest over the file is the only honest way to assert it.
    let mut digests = Vec::new();
    for threads in ["1", "4"] {
        let (code, stdout, _) = cli(&[
            "run",
            &p(&dir, "run.pir"),
            "--out",
            &p(&dir, &format!("t{threads}")),
            "--threads",
            threads,
            "--chunk-size",
            "2",
            "--json",
            "--run-id",
            "fixed",
        ]);
        assert_eq!(code, 0, "{stdout}");
        digests.push(json_of(&stdout));
    }
    assert_eq!(
        digests[0]["results"]["digest"],
        digests[1]["results"]["digest"]
    );
    assert_eq!(digests[0]["run_digest"], digests[1]["run_digest"]);
    // `manifest_digest` is deliberately *not* asserted equal: `run_config.exec`
    // participates in it, so recording "this ran on four threads" changes the
    // manifest even though it cannot change a number. That is the manifest's
    // contract (`04-verify.md` §6), not a determinism failure — `results.digest`
    // above is the byte-equality claim.
}

#[test]
fn the_chunk_size_changes_the_file_layout_and_nothing_else() {
    let dir = scratch("determinism-chunks");
    let mut docs = Vec::new();
    for chunk in ["1", "1024"] {
        let (code, stdout, _) = cli(&[
            "run",
            &p(&dir, "run.pir"),
            "--out",
            &p(&dir, &format!("c{chunk}")),
            "--chunk-size",
            chunk,
            "--json",
            "--run-id",
            "fixed",
        ]);
        assert_eq!(code, 0, "{stdout}");
        docs.push(json_of(&stdout));
    }
    // The result *set* is the same set: same rows, same components, same digest
    // over the component set, same run identity.
    assert_eq!(docs[0]["results"]["rows"], docs[1]["results"]["rows"]);
    assert_eq!(
        docs[0]["results"]["component_set_digest"],
        docs[1]["results"]["component_set_digest"]
    );
    assert_eq!(docs[0]["run_digest"], docs[1]["run_digest"]);
    assert_eq!(docs[0]["model_digest"], docs[1]["model_digest"]);
    assert_eq!(
        docs[0]["modelpoints"]["projected"],
        docs[1]["modelpoints"]["projected"]
    );
}

#[test]
fn run_refuses_a_model_that_does_not_check() {
    let dir = scratch("run-refuse");
    std::fs::write(p(&dir, "term.pir"), BROKEN).unwrap();
    let (code, stdout, _) = cli(&[
        "run",
        &p(&dir, "run.pir"),
        "--out",
        &p(&dir, "out"),
        "--json",
    ]);
    assert_eq!(code, 2);
    assert_eq!(json_of(&stdout)["status"], "not_checked");
    assert!(!Path::new(&p(&dir, "out")).exists(), "nothing was written");
}

#[test]
fn run_overrides_take_precedence_over_the_run_file() {
    let dir = scratch("run-overrides");
    // A second modelpoint file with two rows instead of four.
    let small = p(&dir, "small.csv");
    std::fs::write(
        &small,
        "policy_number,sum_assured,premium,q\nA,1000,10,0.01\nB,2000,20,0.02\n",
    )
    .unwrap();
    let (code, stdout, _) = cli(&[
        "run",
        &p(&dir, "run.pir"),
        "--out",
        &p(&dir, "out"),
        "--modelpoints",
        &small,
        "--json",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let doc = json_of(&stdout);
    assert_eq!(doc["modelpoints"]["rows"], 2);
    assert!(doc["modelpoints"]["path"]
        .as_str()
        .unwrap()
        .ends_with("small.csv"));
}

// ---------------------------------------------------------------------------
// graph
// ---------------------------------------------------------------------------

#[test]
fn graph_renders_the_planners_order_rather_than_one_of_its_own() {
    let dir = scratch("graph");
    let (code, stdout, _) = cli(&["graph", &p(&dir, "term.pir"), "--json"]);
    assert_eq!(code, 0);
    let doc = json_of(&stdout);

    let nodes = doc["nodes"].as_array().unwrap();
    let ids: Vec<&str> = nodes.iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert!(ids.contains(&"survivors") && ids.contains(&"bel"));

    // Every node appears in exactly one layer, and the layer count is the depth.
    let layers = doc["layers"].as_array().unwrap();
    let mut flat: Vec<&str> = layers
        .iter()
        .flat_map(|l| l.as_array().unwrap().iter().map(|n| n.as_str().unwrap()))
        .collect();
    flat.sort_unstable();
    let mut sorted_ids = ids.clone();
    sorted_ids.sort_unstable();
    assert_eq!(flat, sorted_ids);

    // A reader is never in a shallower layer than what it reads.
    let depth = |name: &str| {
        nodes.iter().find(|n| n["name"] == name).unwrap()["depth"]
            .as_u64()
            .unwrap()
    };
    assert!(depth("claims") > depth("survivors"));
    assert!(depth("net_cashflow") > depth("claims"));
    assert!(depth("bel") > depth("net_cashflow"));

    // Edges carry the lag the IR declared: `survivors[t-1]` is a lag-1 self edge.
    let edges = doc["edges"].as_array().unwrap();
    let self_edge = edges
        .iter()
        .find(|e| e["from"] == "term.survivors" && e["to"] == "term.survivors")
        .expect("the self-referential edge is in the graph");
    assert_eq!(self_edge["lag"], 1);
    assert_eq!(self_edge["via"], "expr.lhs");

    // The expression text and its span survive, because the inspector renders them.
    let survivors = nodes.iter().find(|n| n["name"] == "survivors").unwrap();
    assert_eq!(survivors["expr"], "survivors[t-1] * (1 - q)");
    assert_eq!(survivors["init"], "1.0");
    assert!(survivors["span"]["line"].as_u64().unwrap() > 1);
    assert_eq!(survivors["stage"], 1);
    assert_eq!(
        nodes.iter().find(|n| n["name"] == "bel").unwrap()["stage"],
        2
    );

    // The graph is pinned to the plan it was drawn from.
    let (_, build, _) = cli(&["build", &p(&dir, "term.pir"), "--json"]);
    assert_eq!(doc["order_digest"], json_of(&build)["order_digest"]);
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

#[test]
fn the_governance_pack_is_one_file_that_touches_no_network() {
    let dir = scratch("export");
    let out = p(&dir, "out");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    let pack = p(&dir, "pack.html");
    let (code, _, _) = cli(&["export", &out, "--out", &pack]);
    assert_eq!(code, 0);
    let html = std::fs::read_to_string(&pack).unwrap();

    // §1.4: no CDN, no fonts fetched, no network at all. Inline `<script>` is
    // how the payload and the engine loader travel (T32); a `<script src>` is
    // what would reach off the file, and that is what is forbidden.
    for forbidden in [
        "http://",
        "https://",
        "<script src",
        "<link ",
        "@import",
        "url(",
    ] {
        assert!(!html.contains(forbidden), "the pack fetches `{forbidden}`");
    }
    // The manifest is a visible header, not a tooltip.
    assert!(html.contains("model_digest"));
    assert!(html.contains("manifest_digest"));
    assert!(html.contains("run_digest"));
    // The model text travels with it.
    assert!(html.contains("survivors[t-1] * (1 - q)"));
    // And it says what it does not contain: the sample is stated, and the rest
    // is named as living in the run directory.
    assert!(html.contains("results.parquet"));
    // The results themselves travel now, as base64 Arrow IPC in the payload.
    assert!(html.contains(r#"id="predictable-payload""#));

    // The JSON pack carries the same facts in machine form.
    let json_pack = p(&dir, "pack.json");
    assert_eq!(
        cli(&["export", &out, "--out", &json_pack, "--format", "json"]).0,
        0
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&json_pack).unwrap()).unwrap();
    assert_eq!(doc["kind"], "pack");
    assert_eq!(doc["manifest_digest_verified"], true);
    assert_eq!(doc["modules"].as_array().unwrap().len(), 1);
    assert!(doc["runs"][0]["series_ipc"].as_str().unwrap().len() > 100);
    assert!(doc["engine"].is_null(), "no --engine, no engine");

    assert_eq!(
        cli(&["export", &out, "--out", &pack, "--format", "pdf"]).0,
        2
    );
}

// ---------------------------------------------------------------------------
// rerun
// ---------------------------------------------------------------------------

#[test]
fn rerun_verifies_every_digest_before_it_will_start() {
    let dir = scratch("rerun");
    let out = p(&dir, "out");
    let manifest = format!("{out}/manifest.json");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    // Clean: verification passes and the rerun reproduces the results byte for byte.
    let (code, stdout, _) = cli(&["rerun", &manifest, "--out", &p(&dir, "again"), "--json"]);
    assert_eq!(code, 0, "{stdout}");
    let original: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    let repeated: Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&p(&dir, "again")).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(original["results"]["digest"], repeated["results"]["digest"]);
    assert_eq!(original["manifest_digest"], repeated["manifest_digest"]);
}

#[test]
fn rerun_refuses_drifted_inputs_and_says_which() {
    let dir = scratch("rerun-drift");
    let out = p(&dir, "out");
    let manifest = format!("{out}/manifest.json");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    // Someone edits a modelpoint after the run.
    std::fs::write(
        p(&dir, "mp.csv"),
        "policy_number,sum_assured,premium,q\nPOL1,999999,900,0.01\n",
    )
    .unwrap();

    let (code, stdout, stderr) = cli(&["rerun", &manifest, "--verify-only", "--json"]);
    assert_eq!(code, 1, "a drifted input is a domain failure");
    let doc = json_of(&stdout);
    assert_eq!(doc["drift"], true);
    let drifted: Vec<&str> = doc["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["drift"] == true)
        .map(|i| i["what"].as_str().unwrap())
        .collect();
    assert_eq!(drifted, vec!["modelpoints"]);
    assert!(stderr.contains("--allow-drift"), "{stderr}");

    // With --allow-drift it proceeds, and the new run is a different run.
    let (code, stdout, _) = cli(&[
        "rerun",
        &manifest,
        "--allow-drift",
        "--out",
        &p(&dir, "drifted"),
        "--json",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let repeated: Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&p(&dir, "drifted")).join("manifest.json")).unwrap(),
    )
    .unwrap();
    let original: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    assert_ne!(original["manifest_digest"], repeated["manifest_digest"]);
    assert_eq!(repeated["inputs"]["modelpoints"]["rows"], 1);
}

#[test]
fn rerun_refuses_a_manifest_that_has_been_edited() {
    let dir = scratch("rerun-tamper");
    let out = p(&dir, "out");
    let manifest = format!("{out}/manifest.json");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    let text = std::fs::read_to_string(&manifest).unwrap();
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    doc["inputs"]["modelpoints"]["rows"] = serde_json::json!(9_999);
    std::fs::write(&manifest, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

    let (code, stdout, stderr) = cli(&["rerun", &manifest, "--verify-only", "--json"]);
    assert_eq!(code, 2);
    assert_eq!(json_of(&stdout)["manifest_digest_verified"], false);
    assert!(stderr.contains("edited"), "{stderr}");
}

// ---------------------------------------------------------------------------
// stubs
// ---------------------------------------------------------------------------

#[test]
fn the_phase_2_stubs_refuse_in_a_way_a_machine_can_tell_apart_from_a_typo() {
    for (name, owner) in [("diff", "T26"), ("migrate", "T24")] {
        let (code, stdout, stderr) = cli(&[name, "--json"]);
        assert_eq!(code, 2, "{name}");
        let doc = json_of(&stdout);
        assert_eq!(doc["status"], "unimplemented");
        assert_eq!(doc["command"], name);
        assert!(
            doc["implemented_by"].as_str().unwrap().contains(owner),
            "{name}: {doc}"
        );
        assert!(stderr.contains("not implemented"), "{stderr}");
    }
    // A typo, by contrast, has no document at all.
    let (code, stdout, _) = cli(&["diffff", "--json"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
}

// ---------------------------------------------------------------------------
// explain (04-verify.md §3)
// ---------------------------------------------------------------------------

/// The command's whole contract in one test: the trace reports the number the
/// run reported. If the replay had diverged the engine would have raised
/// `E0901` and this would exit 1, so a pass *is* the replay assertion.
#[test]
fn explain_replays_a_modelpoint_to_the_value_the_run_stored() {
    let dir = scratch("explain");
    let out = p(&dir, "out");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    let (code, stdout, stderr) = cli(&[
        "explain",
        &out,
        "--component",
        "bel",
        "--mp",
        "POL2",
        "--depth",
        "-1",
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let doc = json_of(&stdout);
    assert_eq!(doc["kind"], "trace");
    assert_eq!(doc["root"]["id"], "term.bel");
    assert_eq!(doc["root"]["modelpoint"]["key"], "POL2");
    assert!(doc["run"].as_str().unwrap().starts_with("sha256:"));

    // POL2: sum_assured 250000, premium 1500, q 0.02, over t = 0..=3.
    let (mut survivors, mut expected) = (1.0f64, 0.0f64);
    for _ in 0..=3 {
        expected += survivors * 0.02 * 250000.0 - survivors * 1500.0;
        survivors *= 1.0 - 0.02;
    }
    let value = doc["root"]["value"].as_f64().unwrap();
    assert!((value - expected).abs() < 1e-9, "{value} vs {expected}");
}

#[test]
fn explain_renders_the_json_and_never_a_fact_the_json_lacks() {
    let dir = scratch("explain-text");
    let out = p(&dir, "out");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    let argv = [
        "explain",
        &out,
        "--component",
        "claims",
        "--mp",
        "POL1",
        "--t",
        "2",
    ];
    let (code, text, _) = cli(&argv);
    assert_eq!(code, 0);
    let (_, again, _) = cli(&argv);
    assert_eq!(text, again, "the same tree must render identically");

    let mut json_argv = argv.to_vec();
    json_argv.push("--json");
    let doc = json_of(&cli(&json_argv).1);
    assert!(text.starts_with("term.claims[t=2]"));
    assert!(text.contains(doc["root"]["modelpoint"]["key"].as_str().unwrap()));
    assert!(text.contains(doc["root"]["expr"].as_str().unwrap()));
}

#[test]
fn explain_refuses_a_t_on_a_permp_and_demands_one_for_a_series() {
    let dir = scratch("explain-shape");
    let out = p(&dir, "out");
    assert_eq!(cli(&["run", &p(&dir, "run.pir"), "--out", &out]).0, 0);

    for (argv, expected) in [
        (
            vec!["--component", "bel", "--mp", "POL1", "--t", "1"],
            "not a Series",
        ),
        (vec!["--component", "claims", "--mp", "POL1"], "needs a `t`"),
        (
            vec!["--component", "claims", "--mp", "POL1", "--t", "99"],
            "outside the projection",
        ),
        (
            vec!["--component", "nope", "--mp", "POL1"],
            "no component named",
        ),
        (vec!["--component", "bel", "--mp", "NOPE"], "no modelpoint"),
    ] {
        let mut full = vec!["explain", &out];
        full.extend(argv.iter().copied());
        let (code, _, stderr) = cli(&full);
        assert_eq!(code, 2, "{argv:?}");
        assert!(stderr.contains(expected), "{argv:?}: {stderr}");
    }
}
