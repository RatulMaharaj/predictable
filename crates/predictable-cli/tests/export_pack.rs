//! `predictable export` — the governance pack, checked the way an auditor
//! would: open the file, with nothing else installed.
//!
//! `05-viz.md` §1.4 is a set of promises about a *file*, so these are assertions
//! about bytes rather than about code paths: one file, no network, the manifest
//! visible, the numbers present and equal to the run's, the engine real.
//!
//! The last test is the one the task's acceptance criterion names: the exported
//! pack is opened outside any Rust process — `node` loading the page's own
//! embedded engine through the page's own loader protocol — and `explain()` is
//! answered client-side. It is skipped only when `node` is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn cli() -> PathBuf {
    // The test binary lives in `target/<profile>/deps/`; the CLI is two up.
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.join("predictable")
}

fn wasm_artefact() -> Option<PathBuf> {
    let path = repo_root().join("target/wasm32-unknown-unknown/wasm-release/predictable_wasm.wasm");
    path.exists().then_some(path)
}

/// Export `models/term_annual` into `out`, returning the page text.
fn export(out: &Path, extra: &[&str]) -> String {
    let mut command = Command::new(cli());
    command
        .current_dir(repo_root().join("models/term_annual"))
        .args(["export", "runs/base", "--format", "html", "--out"])
        .arg(out)
        .args(["--modelpoints", "sample:5"])
        .args(extra);
    let output = command.output().expect("the CLI runs");
    assert!(
        output.status.success(),
        "export failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(out).unwrap()
}

/// The JSON island the page embeds, parsed.
fn payload(page: &str) -> serde_json::Value {
    let start = page
        .find(r#"<script type="application/json" id="predictable-payload">"#)
        .expect("the pack embeds a payload");
    let start = page[start..].find('>').unwrap() + start + 1;
    let end = page[start..].find("</script>").unwrap() + start;
    serde_json::from_str(&page[start..end].replace("\\u003c", "<")).expect("the payload is JSON")
}

#[test]
fn the_pack_is_one_file_with_no_network_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let page = export(&dir.path().join("pack.html"), &[]);
    for forbidden in ["<script src", "<link ", "http://", "https://", "@import"] {
        assert!(
            !page.contains(forbidden),
            "the pack reaches outside itself: found `{forbidden}`"
        );
    }
    // The print rule of §1.4 is part of the file, not of a stylesheet someone
    // has to remember to add.
    assert!(page.contains("@media print"));
}

#[test]
fn the_manifest_is_a_visible_header_not_a_tooltip() {
    let dir = tempfile::tempdir().unwrap();
    let page = export(&dir.path().join("pack.html"), &[]);
    for digest in [
        "manifest_digest",
        "model_digest",
        "assumption_digest",
        "modelpoint_digest",
        "run_digest",
        "results_digest",
        "engine_version",
    ] {
        assert!(page.contains(digest), "`{digest}` is not in the header");
    }
    assert!(
        page.contains("verified"),
        "the digest is re-derived, not echoed"
    );
}

#[test]
fn the_results_are_embedded_as_arrow_and_match_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let page = export(&dir.path().join("pack.html"), &[]);
    let payload = payload(&page);

    let ipc = payload["runs"][0]["series_ipc"]
        .as_str()
        .expect("Arrow IPC");
    let bytes = base64_decode(ipc);
    let batches = predictable_viz::from_ipc(&bytes).expect("a readable Arrow stream");
    let batch = &batches[0];
    // The period axis is the union of every `t` the run wrote, which includes
    // the `t = -1` row a `PerMP` output occupies in the results schema (§8.4.1).
    // The grid is dense over that union: `NaN` where a component has no cell.
    assert_eq!(batch.num_rows() % 5, 0, "the grid is dense");
    assert!(
        batch.num_rows() >= 5 * 41,
        "41 periods per modelpoint at least"
    );

    // The oracle is the file T22 generated through the native pipeline.
    let expected =
        std::fs::read_to_string(repo_root().join("models/term_annual/expected/series_TA00001.csv"))
            .unwrap();
    let mut lines = expected.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let mp = arrow_array::cast::AsArray::as_string::<i32>(batch.column_by_name("mp").unwrap());
    let t = arrow_array::cast::AsArray::as_primitive::<arrow_array::types::Int32Type>(
        batch.column_by_name("t").unwrap(),
    );

    let mut compared = 0;
    for line in lines {
        let cells: Vec<&str> = line.split(',').collect();
        let period: i32 = cells[0].parse().unwrap();
        let row = (0..batch.num_rows())
            .find(|i| mp.value(*i) == "TA00001" && t.value(*i) == period)
            .expect("the sampled pack holds TA00001");
        for (name, cell) in header.iter().zip(&cells).skip(1) {
            let column = arrow_array::cast::AsArray::as_primitive::<arrow_array::types::Float64Type>(
                batch.column_by_name(name).unwrap(),
            );
            let want: f64 = cell.parse().unwrap();
            assert_eq!(
                column.value(row).to_bits(),
                want.to_bits(),
                "{name} at t={period}"
            );
            compared += 1;
        }
    }
    assert!(compared > 200, "only {compared} cells compared");

    // "Show the numbers" (§1.4): the same values, as text an auditor can copy.
    assert!(page.contains("show the numbers"));
    assert!(page.contains("-223.20202973973744"));
}

#[test]
fn a_pack_without_an_engine_says_so_and_bakes_what_it_was_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let page = export(
        &dir.path().join("pack.html"),
        &["--with-traces", "model.reserve"],
    );
    let payload = payload(&page);
    assert!(payload["engine"].is_null());
    let traces = payload["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 3, "three sampled modelpoints, one component");
    assert_eq!(traces[0]["component"], "model.reserve");
    let value = traces[0]["trace"]["root"]["value"].as_f64().unwrap();
    assert_eq!(value.to_bits(), (-223.20202973973744_f64).to_bits());
    assert!(
        page.contains("Re-export with"),
        "the limit is stated, not hidden"
    );
}

#[test]
fn a_diff_can_be_embedded_and_is_the_rundiff_document_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let page = export(
        &dir.path().join("pack.html"),
        &["--include", "graph,diff:runs/base"],
    );
    let payload = payload(&page);
    let diffs = payload["diffs"].as_array().unwrap();
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0]["doc"]["kind"], "rundiff");
    // A run against itself: the verdict is the identity case, and the pack
    // carries the document rather than a summary of it.
    assert!(diffs[0]["doc"]["summary"]["verdict"].is_string());
}

#[test]
fn the_embedded_engine_is_a_real_module_and_the_page_can_reach_it() {
    let Some(wasm) = wasm_artefact() else {
        eprintln!("skipping: build the engine first (see tests/wasm_gate.rs)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let page = export(
        &dir.path().join("pack.html"),
        &["--engine", "wasm", "--engine-path", wasm.to_str().unwrap()],
    );
    let payload = payload(&page);
    let bytes = base64_decode(payload["engine"]["wasm_base64"].as_str().unwrap());
    assert_eq!(&bytes[..4], b"\0asm", "that is not a wasm module");
    assert_eq!(bytes, std::fs::read(&wasm).unwrap());
    // §1.5's caveat is stated in the export header, not left implicit.
    assert!(page.contains("bit-identical"));
    // Everything the engine needs to rebuild the run is in the payload.
    let inputs = &payload["engine"]["inputs"];
    assert_eq!(inputs["sources"].as_array().unwrap().len(), 3);
    assert_eq!(inputs["tables"].as_array().unwrap().len(), 3);
    assert!(inputs["modelpoints"].as_str().unwrap().contains("TA00001"));
    assert!(inputs["assumptions"]["mortality_loading"].as_f64().unwrap() > 0.0);
}

/// The acceptance criterion: the pack opens outside Rust and traces a cell.
///
/// `node` stands in for the browser deliberately — it has no DOM shim here, so
/// what is exercised is exactly the part that must work in a file:// page with
/// no network: `WebAssembly.instantiate` over the embedded bytes, and the
/// `pv_alloc`/`pv_call`/`pv_free` protocol the pack's own loader uses.
#[test]
fn explain_answers_client_side_from_the_exported_file() {
    let Some(wasm) = wasm_artefact() else {
        eprintln!("skipping: the engine has not been built");
        return;
    };
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping: node is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("pack.html");
    export(
        &pack,
        &["--engine", "wasm", "--engine-path", wasm.to_str().unwrap()],
    );

    let driver = dir.path().join("driver.mjs");
    std::fs::write(&driver, DRIVER_JS).unwrap();
    let output = Command::new("node")
        .arg(&driver)
        .arg(&pack)
        .output()
        .expect("node runs");
    assert!(
        output.status.success(),
        "the pack did not answer client-side:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the driver prints JSON");

    // The traced value is the run's value, to the bit.
    assert_eq!(result["trace"]["kind"], "trace");
    let value = result["trace"]["root"]["value"].as_f64().unwrap();
    assert_eq!(value.to_bits(), (-223.20202973973744_f64).to_bits());
    // And the fan is computed points only, each with Q12 lineage.
    assert_eq!(result["fan"]["interpolated"], false);
    assert_eq!(result["fan"]["scenarios"].as_array().unwrap().len(), 3);
    assert_eq!(
        result["fan"]["scenarios"][0]["lineage"]["varied"][0]["path"],
        "assumptions.mortality_loading"
    );
    assert_ne!(
        result["fan"]["scenarios"][0]["series"]["columns"]["model.bel"][0],
        result["fan"]["base"]["columns"]["model.bel"][0],
        "a scenario that changed nothing is not a scenario"
    );
}

/// The browser's half of the pack, in 40 lines: read the page, decode the
/// engine, speak the ABI. If this drifts from `LOADER_JS` in `cmd/export.rs`,
/// the pack is broken and this test says so.
const DRIVER_JS: &str = r#"
import { readFileSync } from "node:fs";
const page = readFileSync(process.argv[2], "utf8");
const open = page.indexOf('<script type="application/json" id="predictable-payload">');
const start = page.indexOf(">", open) + 1;
const payload = JSON.parse(page.slice(start, page.indexOf("</script>", start)).replaceAll("\\u003c", "<"));
const bytes = Uint8Array.from(Buffer.from(payload.engine.wasm_base64, "base64"));
const { instance } = await WebAssembly.instantiate(bytes, {});
const { pv_alloc, pv_free, pv_call, memory } = instance.exports;
const enc = new TextEncoder(), dec = new TextDecoder();
const call = (request) => {
  const body = enc.encode(JSON.stringify(request));
  const ptr = pv_alloc(body.length);
  new Uint8Array(memory.buffer, ptr, body.length).set(body);
  const res = pv_call(ptr, body.length);
  const len = new DataView(memory.buffer).getUint32(res, true);
  const text = dec.decode(new Uint8Array(memory.buffer, res + 4, len));
  pv_free(res, len + 4);
  return JSON.parse(text);
};
const inputs = payload.engine.inputs;
const trace = call({ op: "explain", inputs, component: "model.reserve", modelpoint: "TA00001", t: 0, depth: -1 });
if (!trace.ok) { console.error(trace); process.exit(1); }
const base = inputs.assumptions.mortality_loading;
const fan = call({
  op: "sensitivity", inputs, components: ["model.bel"], modelpoints: ["TA00001"],
  group_id: "driver", scenarios: [0.9, 1.1, 1.25].map((f) => ({
    label: "mortality × " + f, set: { mortality_loading: base * f },
  })),
});
if (!fan.ok) { console.error(fan); process.exit(1); }
process.stdout.write(JSON.stringify({ trace: trace.trace, fan }));
"#;

/// Standard base64 decode — the mirror of the encoder in `cmd/export.rs`.
fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut index = [255u8; 256];
    for (i, c) in ALPHABET.iter().enumerate() {
        index[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let value = index[byte as usize];
        assert_ne!(value, 255, "not base64: `{}`", byte as char);
        acc = (acc << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}
