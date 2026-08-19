//! The wasm gate: the size budget, and byte equality against native under
//! `wasmtime`.
//!
//! `03-engine.md` §9 makes two shipped promises about the browser build, and
//! `05-viz.md` §1.5 turns the second into a release rule — "if that ever fails,
//! the WASM path is disabled rather than shipped with a footnote". Both are
//! asserted here rather than assumed:
//!
//! 1. **Size.** engine + parser + checker ≤ 1.5 MB gzipped, built with the
//!    `wasm-release` profile (`opt-level = "z"`, `panic = "abort"`, LTO,
//!    stripped) and `+simd128,+bulk-memory`. No `wasm-opt` is involved: the
//!    budget must hold for an artefact this repository can rebuild with cargo
//!    alone, and `wasm-opt` only ever makes it smaller.
//!
//! 2. **Determinism.** Every case in T16's corpus is rendered to IEEE-754 bit
//!    patterns by `predictable_wasm::render::render_case`, natively and again
//!    inside `wasm32-wasip1` under `wasmtime`, and the two byte strings must be
//!    equal. This is T16's harness extended to a third target: the corpus, the
//!    loader and the "bits, not decimals" rule are all its
//!    (`predictable_determinism::corpus`), and what is new is the guest.
//!
//! The gate builds its own guest, because a stale `.wasm` on disk would turn a
//! real divergence into a passing test. `PREDICTABLE_SKIP_WASM_GATE=1` skips it
//! for a machine without the `wasm32-*` targets installed; nothing else does,
//! and a missing target is otherwise a failure with the `rustup` line to fix it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::write::GzEncoder;
use flate2::Compression;
use predictable_wasm::session::{Inputs, SourceFile, TableBytes};
use wasmtime::{Config, Engine, Linker, Module, Store};
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

/// §9's budget, in bytes of gzip.
const SIZE_BUDGET_GZIP: usize = 1_536 * 1024;

fn skip() -> bool {
    std::env::var_os("PREDICTABLE_SKIP_WASM_GATE").is_some()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

/// Build one wasm artefact with the profile §9 specifies, and return its path.
fn build(target: &str, what: &[&str], artefact: &str) -> PathBuf {
    let root = workspace_root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = Command::new(cargo);
    command
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "predictable-wasm",
            "--profile",
            "wasm-release",
        ])
        .args(what)
        .args(["--target", target])
        // §9: SIMD128 and bulk memory are on. SIMD only ever accelerates
        // lane-parallel ops, never reductions, which is why enabling it cannot
        // move a number — a claim this test then proves.
        .env("RUSTFLAGS", "-C target-feature=+simd128,+bulk-memory")
        // Nested cargo: without this, the outer test's jobserver and the inner
        // build fight over the same lock file for no reason.
        .env_remove("CARGO_MAKEFLAGS");
    let output = command.output().expect("cargo runs");
    assert!(
        output.status.success(),
        "building for {target} failed. If the target is missing:\n  \
         rustup target add {target}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = root
        .join("target")
        .join(target)
        .join("wasm-release")
        .join(artefact);
    assert!(path.exists(), "{} was not produced", path.display());
    path
}

#[test]
fn the_browser_artefact_fits_the_size_budget() {
    if skip() {
        return;
    }
    let path = build(
        "wasm32-unknown-unknown",
        &["--lib"],
        "predictable_wasm.wasm",
    );
    let bytes = std::fs::read(&path).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&bytes).unwrap();
    let gzipped = encoder.finish().unwrap().len();
    println!(
        "wasm32-unknown-unknown: {} bytes raw, {gzipped} gzipped (budget {SIZE_BUDGET_GZIP})",
        bytes.len()
    );
    assert!(
        gzipped <= SIZE_BUDGET_GZIP,
        "the engine is {gzipped} bytes gzipped, over §9's {SIZE_BUDGET_GZIP} byte budget. \
         Adding a dependency to `predictable-wasm` is the usual cause."
    );
}

#[test]
fn the_browser_artefact_exports_the_whole_abi() {
    if skip() {
        return;
    }
    let path = build(
        "wasm32-unknown-unknown",
        &["--lib"],
        "predictable_wasm.wasm",
    );
    let engine = Engine::new(&Config::new()).unwrap();
    let module = Module::from_file(&engine, &path).expect("the artefact is a valid module");
    for name in ["pv_alloc", "pv_free", "pv_call", "memory"] {
        assert!(
            module.get_export(name).is_some(),
            "`{name}` is missing: the page's loader cannot talk to this build"
        );
    }
}

/// Run the WASI guest with `request` on stdin and return its stdout.
fn run_guest(module_path: &Path, request: &str) -> String {
    let mut config = Config::new();
    config.wasm_simd(true);
    config.wasm_bulk_memory(true);
    let engine = Engine::new(&config).unwrap();
    let module = Module::from_file(&engine, module_path).expect("a valid wasip1 module");

    let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
    preview1::add_to_linker_sync(&mut linker, |ctx| ctx).unwrap();

    let stdout = wasmtime_wasi::pipe::MemoryOutputPipe::new(64 * 1024 * 1024);
    // No preopens and no environment: a guest that could read a file might read
    // the host's copy of the corpus and agree with native for the wrong reason.
    let wasi = WasiCtxBuilder::new()
        .stdin(wasmtime_wasi::pipe::MemoryInputPipe::new(
            request.as_bytes().to_vec(),
        ))
        .stdout(stdout.clone())
        .inherit_stderr()
        .build_p1();

    let mut store = Store::new(&engine, wasi);
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .unwrap();
    start.call(&mut store, ()).expect("the guest ran");
    drop(store);
    String::from_utf8(stdout.contents().to_vec()).expect("the guest wrote UTF-8")
}

/// The gate corpus: T16's conformance cases, plus the reference model whose
/// numbers the governance pack actually ships.
fn gate_cases() -> Vec<(String, Inputs)> {
    let root = workspace_root();
    let mut out = Vec::new();

    for case in predictable_determinism::cases() {
        // A case whose tables need a host resource cannot be loaded without a
        // filesystem; T16 reports those as skips and so does this.
        let Ok(loaded) = predictable_determinism::load(&case) else {
            continue;
        };
        let sources = case
            .inputs
            .iter()
            .map(|input| SourceFile {
                name: input.name.clone(),
                text: input.text.clone(),
            })
            .collect();
        let mut tables = Vec::new();
        let mut resolvable = true;
        for module in &loaded.modules {
            for decl in &module.tables {
                if let predictable_ir::model::TableSource::File(path) = &decl.source {
                    match std::fs::read_to_string(case.dir.join(path)) {
                        Ok(text) => tables.push(TableBytes {
                            name: path.clone(),
                            text,
                        }),
                        Err(_) => resolvable = false,
                    }
                }
            }
        }
        if !resolvable {
            continue;
        }
        out.push((
            case.name.clone(),
            Inputs {
                sources,
                assumptions: loaded.assumptions.clone(),
                tables,
                modelpoints: loaded.modelpoints.0.clone(),
                allow_table_drift: false,
            },
        ));
    }

    let dir = root.join("models/term_annual");
    let read = |p: &str| std::fs::read_to_string(dir.join(p)).unwrap();
    let mut assumptions = std::collections::BTreeMap::new();
    for line in read("base.pir").lines() {
        if let Some((name, value)) = line.split_once('=') {
            if let Ok(value) = value.trim().parse::<f64>() {
                assumptions.insert(name.trim().to_string(), value);
            }
        }
    }
    out.push((
        "models/term_annual".to_string(),
        Inputs {
            sources: ["build/model.pir", "build/product.pir", "build/schema.pir"]
                .iter()
                .map(|n| SourceFile {
                    name: n.to_string(),
                    text: read(n),
                })
                .collect(),
            assumptions,
            tables: [
                "tables/expenses.csv",
                "tables/lapses.csv",
                "tables/mortality.csv",
            ]
            .iter()
            .map(|n| TableBytes {
                name: n.to_string(),
                text: read(n),
            })
            .collect(),
            modelpoints: read("data/modelpoints.csv"),
            allow_table_drift: false,
        },
    ));
    out
}

#[test]
fn wasm32_wasip1_under_wasmtime_is_bit_identical_to_native() {
    if skip() {
        return;
    }
    let guest = build(
        "wasm32-wasip1",
        &["--bin", "predictable-wasm-cli"],
        "predictable-wasm-cli.wasm",
    );

    let cases = gate_cases();
    assert!(
        cases.len() >= 3,
        "the gate corpus collapsed to {} case(s)",
        cases.len()
    );

    let mut compared = 0usize;
    for (name, inputs) in &cases {
        let request = serde_json::json!({"op": "render", "name": name, "inputs": inputs});
        let native = predictable_wasm::call(&request.to_string());
        let guest_out = run_guest(&guest, &request.to_string());

        let native: serde_json::Value = serde_json::from_str(&native).unwrap();
        let wasm: serde_json::Value = serde_json::from_str(&guest_out).unwrap();
        if native["ok"] != serde_json::Value::Bool(true) {
            // A case the wasm build cannot load fails identically on both
            // targets or the gate has found something; either way, compare.
            assert_eq!(native, wasm, "`{name}`: the two targets failed differently");
            continue;
        }
        let native = native["render"].as_str().unwrap();
        let wasm = wasm["render"].as_str().unwrap();
        if native != wasm {
            let line = native
                .lines()
                .zip(wasm.lines())
                .find(|(a, b)| a != b)
                .map(|(a, b)| format!("native: {a}\n  wasm: {b}"))
                .unwrap_or_else(|| "one rendering is longer than the other".to_string());
            panic!(
                "`{name}` diverged between native and wasm32-wasip1.\n{line}\n\
                 §1.5: the wasm path is disabled, not shipped with a footnote."
            );
        }
        compared += native.lines().count();
    }
    println!("{} case(s), {compared} rendered line(s) equal", cases.len());
    assert!(compared > 1_000, "only {compared} lines were compared");
}
