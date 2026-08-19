//! `predictable export <run_dir> --format html|json --out <file>` — the governance pack.
//!
//! `05-viz.md` §1.4: one self-contained file, no CDN, no fonts fetched, no network
//! at all; the manifest is a **visible header**, not a tooltip; and `Ctrl-P`
//! produces a sane paginated print layout. That audience — an auditor opening an
//! emailed file in three years — is why the page states what it does *not* contain
//! as plainly as what it does: a pack that quietly omitted the results would look
//! exactly like one that included them.
//!
//! ```bash
//! predictable export runs/base --format html --out pack.html \
//!     --include graph,waterfall,diff:runs/sensitivity \
//!     --modelpoints sample:200 \
//!     --with-traces bel,reserve \
//!     --engine wasm
//! ```
//!
//! What the pack embeds, in the order §1.4 lists it:
//!
//! * the **manifest**, with every digest, re-derived and reported as verified or
//!   as a mismatch — a pack whose header says MISMATCH is a pack that was edited;
//! * the full **canonical IR text** of every module the run pinned, plus the
//!   assumption values and the table bytes, so the model is reconstructible from
//!   the file alone;
//! * the **results**, as base64 Arrow IPC in the embedded payload (§1.4's size
//!   discipline: Arrow, never JSON) and as an HTML table behind each chart's
//!   "show the numbers" toggle, because auditors copy numbers;
//! * the **graph document** the Model Explorer reads, so the pack opens in the
//!   same SPA the local server serves (`datasource.ts`: `InlineDataSource`);
//! * optionally **pre-baked traces** (`--with-traces`), and optionally the
//!   **engine itself** (`--engine wasm`), which turns the pre-baked few into
//!   *any cell* — §1.5's highest-value use of WASM.
//!
//! `--sign` is not implemented (§1.4 puts the mechanics out of scope for v1), but
//! the content hash it would sign is printed in the header from day one so the
//! format does not change when it arrives.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_io::outbound::Manifest;
use predictable_ir::model::TableSource;
use predictable_plan::{OptLevel, PlanOptions};
use predictable_rundiff::RunSide;
use predictable_wasm::session::{Inputs, Session, SourceFile, TableBytes};
use serde_json::{json, Value};

use crate::args::Args;
use crate::emit::{Emitter, OK};
use crate::load::{compile, Sources};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &[
    "json",
    "out-json",
    "out",
    "format",
    "include",
    "modelpoints",
    "with-traces",
    "engine",
    "engine-path",
];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &[
    "out-json",
    "out",
    "format",
    "include",
    "modelpoints",
    "with-traces",
    "engine",
    "engine-path",
];

/// Where the SPA looks for the embedded payload
/// (`frontend/src/datasource.ts`: `PAYLOAD_ELEMENT_ID`). The two must agree, and
/// `crates/predictable-viz/frontend/src/wasm/pack.test.ts` asserts it against a
/// pack this command wrote.
const PAYLOAD_ELEMENT_ID: &str = "predictable-payload";

/// Where `cargo build -p predictable-wasm --lib --profile wasm-release --target
/// wasm32-unknown-unknown` leaves the engine.
const DEFAULT_WASM: &str = "target/wasm32-unknown-unknown/wasm-release/predictable_wasm.wasm";

/// Run `export`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let run_dir = PathBuf::from(args.one_positional("path to a run directory")?);
    let format = args.opt("format").unwrap_or("html");
    if !matches!(format, "html" | "json") {
        return Err(format!(
            "`--format {format}` is not a pack format (html, json)"
        ));
    }
    let out = args
        .opt("out")
        .ok_or("export needs --out <file>")?
        .to_string();

    let manifest_path = run_dir.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest = Manifest::from_json(&manifest_text).map_err(|e| e.to_string())?;
    let verified = manifest.verify_digest().unwrap_or(false);

    let modules: Vec<(String, String)> = manifest
        .inputs
        .model
        .files
        .iter()
        .map(|f| {
            let text = std::fs::read_to_string(&f.path)
                .unwrap_or_else(|e| format!("<< {}: {e} >>", f.path));
            (f.path.clone(), text)
        })
        .collect();
    let results_schema = read_optional(&run_dir.join("results.schema.json"));

    let include: Vec<String> = args
        .opt("include")
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["graph".to_string()]);

    // ---- the results -----------------------------------------------------
    // A run directory is a run directory whoever wrote it (§4.3.7), so the pack
    // reads it with the same loader the run diff uses rather than a second,
    // divergent parquet reader.
    let side = RunSide::load("pack", &run_dir).map_err(|e| e.to_string())?;
    let selection = Selection::parse(args.opt("modelpoints").unwrap_or("sample:200"))?;
    let sampled = selection.apply(&side);
    let series = SeriesBlock::build(&side, &sampled)?;

    // ---- the graph -------------------------------------------------------
    let model_paths: Vec<String> = manifest
        .inputs
        .model
        .files
        .iter()
        .map(|f| f.path.clone())
        .collect();
    let sources = Sources::load(&model_paths)?;
    let graph = match sources.check().is_ok() {
        true => {
            let compiled = compile(
                &sources,
                &PlanOptions {
                    opt: OptLevel::O1,
                    retain_all: true,
                    program_digest: String::new(),
                    periods: None,
                },
            )?;
            Some(predictable_viz::graph_doc(
                &sources.inputs,
                &compiled.plan,
                &compiled.model_digest,
            ))
        }
        // A pack for a model that no longer checks is still worth having: the
        // manifest and the numbers are the evidence. The graph is what goes.
        false => None,
    };

    // ---- the model, as a wasm session's inputs ---------------------------
    let engine_inputs = engine_inputs(&manifest, &modules, &sampled)?;

    // ---- pre-baked traces ------------------------------------------------
    let trace_components: Vec<String> = args
        .opt("with-traces")
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let traces = bake_traces(&engine_inputs, &trace_components, &sampled)?;

    // ---- the engine ------------------------------------------------------
    let engine = match args.opt("engine") {
        None => None,
        Some("none") => None,
        Some("wasm") => {
            let path = args
                .opt("engine-path")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("PREDICTABLE_WASM").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WASM));
            let bytes = std::fs::read(&path).map_err(|e| {
                format!(
                    "--engine wasm needs the engine at {}: {e}\n  build it with: cargo build \
                     -p predictable-wasm --lib --profile wasm-release --target \
                     wasm32-unknown-unknown\n  or point --engine-path at one",
                    path.display()
                )
            })?;
            Some((path.display().to_string(), bytes))
        }
        Some(other) => return Err(format!("`--engine {other}` is not an engine (wasm, none)")),
    };

    // ---- embedded diffs (`--include diff:<run>`) -------------------------
    let mut diffs = Vec::new();
    for spec in &include {
        let Some(other) = spec.strip_prefix("diff:") else {
            continue;
        };
        let b = RunSide::load("b", other).map_err(|e| e.to_string())?;
        let doc = predictable_rundiff::diff_runs(&side, &b, &Default::default()).to_json();
        diffs.push(json!({"a": manifest.run_id, "b": b.manifest.run_id, "doc": doc}));
    }

    let payload = json!({
        "format": crate::emit::FORMAT,
        "kind": "pack",
        "manifest": serde_json::from_str::<Value>(&manifest_text).map_err(|e| e.to_string())?,
        "manifest_digest_verified": verified,
        "graph": graph,
        "runs": [{
            "run": manifest.run_id,
            "series_ipc": series.ipc_base64,
        }],
        "diffs": diffs,
        "traces": traces,
        "engine": engine.as_ref().map(|(path, bytes)| json!({
            "kind": "wasm32-unknown-unknown",
            "version": predictable_wasm::version(),
            "built_from": path,
            "bytes": bytes.len(),
            "wasm_base64": base64(bytes),
            "inputs": engine_inputs,
        })),
    });

    let bytes = match format {
        "json" => {
            let mut doc = payload.clone();
            doc["modules"] = json!(modules
                .iter()
                .map(|(p, t)| json!({"path": p, "text": t}))
                .collect::<Vec<_>>());
            doc["results_schema"] = results_schema
                .as_deref()
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
                .unwrap_or(Value::Null);
            format!(
                "{}\n",
                serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
            )
        }
        _ => html_pack(
            &manifest,
            &manifest_text,
            verified,
            &modules,
            results_schema.as_deref(),
            &series,
            &payload,
            engine.is_some(),
            &trace_components,
        ),
    };
    std::fs::write(&out, bytes.as_bytes()).map_err(|e| format!("{out}: {e}"))?;

    emitter.document(
        "export",
        json!({
            "run": run_dir.display().to_string(),
            "out": out,
            "pack_format": format,
            "bytes": bytes.len(),
            "manifest_digest": manifest.manifest_digest,
            "manifest_digest_verified": verified,
            "modules": modules.len(),
            "embeds_results": true,
            "modelpoints": sampled.len(),
            "components": series.components.len(),
            "traces": payload["traces"].as_array().map(Vec::len).unwrap_or(0),
            "diffs": diffs_count(&payload),
            "engine": engine.as_ref().map(|(_, b)| json!({
                "kind": "wasm32-unknown-unknown",
                "bytes": b.len(),
                "version": predictable_wasm::version(),
            })),
            "content_hash": predictable_rundiff::sha256_hex(bytes.as_bytes()),
        }),
    )?;
    if !emitter.json {
        emitter.line(format!(
            "{out}  {} byte(s), {} modelpoint(s) × {} component(s), manifest {}{}",
            bytes.len(),
            sampled.len(),
            series.components.len(),
            if verified { "verified" } else { "UNVERIFIED" },
            match &engine {
                Some((_, b)) => format!(", engine {} byte(s)", b.len()),
                None => String::new(),
            }
        ));
    }
    Ok(OK)
}

fn diffs_count(payload: &Value) -> usize {
    payload["diffs"].as_array().map(Vec::len).unwrap_or(0)
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

// ---------------------------------------------------------------------------
// Which modelpoints
// ---------------------------------------------------------------------------

/// `--modelpoints all | sample:N | K1,K2,…`.
///
/// A sample is the **first N in `mp_row` order**, never a random draw: a pack
/// that sampled randomly could not be re-exported to the same file, and an
/// auditor who re-runs the command must get the same bytes.
#[derive(Debug)]
enum Selection {
    All,
    Sample(usize),
    Keys(Vec<String>),
}

impl Selection {
    fn parse(spec: &str) -> Result<Selection, String> {
        if spec == "all" {
            return Ok(Selection::All);
        }
        if let Some(n) = spec.strip_prefix("sample:") {
            return n
                .parse::<usize>()
                .map(Selection::Sample)
                .map_err(|_| format!("`--modelpoints sample:{n}` needs a count"));
        }
        Ok(Selection::Keys(
            spec.split(',').map(|s| s.trim().to_string()).collect(),
        ))
    }

    fn apply(&self, side: &RunSide) -> Vec<(u32, String)> {
        let ordered: Vec<(u32, String)> = side
            .mp_keys
            .iter()
            .map(|(row, key)| (*row, key.clone()))
            .collect();
        match self {
            Selection::All => ordered,
            Selection::Sample(n) => ordered.into_iter().take(*n).collect(),
            Selection::Keys(keys) => ordered
                .into_iter()
                .filter(|(_, key)| keys.contains(key))
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// The results
// ---------------------------------------------------------------------------

/// The sampled results, in both forms the pack needs: Arrow IPC for the payload
/// (§1.4) and long-form rows for the "show the numbers" table.
struct SeriesBlock {
    components: Vec<String>,
    periods: Vec<i32>,
    keys: Vec<String>,
    /// `component → (mp index, period index) → value`, dense, NaN where absent.
    values: BTreeMap<String, Vec<f64>>,
    ipc_base64: String,
}

impl SeriesBlock {
    fn build(side: &RunSide, sampled: &[(u32, String)]) -> Result<SeriesBlock, String> {
        let mut periods: Vec<i32> = side
            .components
            .values()
            .flat_map(|cells| cells.keys().map(|(_, t)| *t))
            .collect();
        periods.sort_unstable();
        periods.dedup();
        if periods.is_empty() {
            periods.push(0);
        }
        // Float columns only: a fan, a waterfall and a grid all plot numbers,
        // and a dictionary column would need a second Arrow schema for no gain.
        let components: Vec<String> = side
            .components
            .iter()
            .filter(|(_, cells)| cells.values().any(|v| v.as_f64().is_some()))
            .map(|(name, _)| name.clone())
            .collect();

        let mut values = BTreeMap::new();
        for component in &components {
            let cells = &side.components[component];
            let mut column = Vec::with_capacity(sampled.len() * periods.len());
            for (row, _) in sampled {
                for t in &periods {
                    column.push(
                        cells
                            .get(&(*row, *t))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(f64::NAN),
                    );
                }
            }
            values.insert(component.clone(), column);
        }

        let keys: Vec<String> = sampled.iter().map(|(_, key)| key.clone()).collect();
        let ipc_base64 = base64(&arrow_ipc(&keys, &periods, &components, &values)?);
        Ok(SeriesBlock {
            components,
            periods,
            keys,
            values,
            ipc_base64,
        })
    }
}

/// The payload's Arrow IPC stream: `mp` (utf8), `t` (int32), one f64 column per
/// component — exactly the schema `datasource.ts` documents for `series_ipc`.
fn arrow_ipc(
    keys: &[String],
    periods: &[i32],
    components: &[String],
    values: &BTreeMap<String, Vec<f64>>,
) -> Result<Vec<u8>, String> {
    use arrow_array::{ArrayRef, Float64Array, Int32Array, RecordBatch, StringArray};
    use std::sync::Arc;

    let rows = keys.len() * periods.len();
    let mut mp = Vec::with_capacity(rows);
    let mut t = Vec::with_capacity(rows);
    for key in keys {
        for period in periods {
            mp.push(key.clone());
            t.push(*period);
        }
    }
    let mut columns: Vec<(String, ArrayRef)> = vec![
        (
            "mp".to_string(),
            Arc::new(StringArray::from(mp)) as ArrayRef,
        ),
        ("t".to_string(), Arc::new(Int32Array::from(t)) as ArrayRef),
    ];
    for component in components {
        columns.push((
            component.clone(),
            Arc::new(Float64Array::from(values[component].clone())) as ArrayRef,
        ));
    }
    let batch = RecordBatch::try_from_iter(columns).map_err(|e| e.to_string())?;
    predictable_viz::to_ipc(&batch).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The model, for the embedded engine
// ---------------------------------------------------------------------------

/// Everything `predictable-wasm` needs to rebuild this run in the page: source
/// text, table bytes, assumption values and the sampled modelpoint rows.
///
/// The modelpoint CSV is filtered to the sample, so a 200-policy pack does not
/// carry a 2-million-row file it cannot show.
fn engine_inputs(
    manifest: &Manifest,
    modules: &[(String, String)],
    sampled: &[(u32, String)],
) -> Result<Inputs, String> {
    let sources: Vec<SourceFile> = modules
        .iter()
        .map(|(name, text)| SourceFile {
            name: name.clone(),
            text: text.clone(),
        })
        .collect();

    let mut assumptions = BTreeMap::new();
    if let Some(a) = &manifest.inputs.assumptions {
        if let Ok(text) = std::fs::read_to_string(&a.path) {
            let mut syn = predictable_syntax::SourceMap::new();
            let doc = predictable_syntax::parse(&mut syn, a.path.clone(), text);
            assumptions = predictable_runner::config::assumption_values(&doc.document);
        }
    }

    // Tables are keyed by the `source` string their declaration carries, which
    // is what `PackResolver` resolves against; the digest is then re-checked
    // inside the session, so an edited pack refuses to run rather than lying.
    let mut tables = Vec::new();
    for module in predictable_plan::lower_modules(
        &sources
            .iter()
            .map(|s| predictable_check::Input::new(s.name.clone(), s.text.clone()))
            .collect::<Vec<_>>(),
    ) {
        for decl in &module.tables {
            if let TableSource::File(path) = &decl.source {
                let bytes = manifest
                    .inputs
                    .tables
                    .iter()
                    .find(|t| t.name == decl.name)
                    .and_then(|t| std::fs::read_to_string(&t.path).ok())
                    .or_else(|| std::fs::read_to_string(path).ok());
                if let Some(text) = bytes {
                    tables.push(TableBytes {
                        name: path.clone(),
                        text,
                    });
                }
            }
        }
    }

    let modelpoints = std::fs::read_to_string(&manifest.inputs.modelpoints.path)
        .map(|csv| filter_csv(&csv, sampled))
        .unwrap_or_default();

    Ok(Inputs {
        sources,
        assumptions,
        tables,
        modelpoints,
        allow_table_drift: false,
    })
}

/// Keep the header and the sampled rows, in file order.
fn filter_csv(text: &str, sampled: &[(u32, String)]) -> String {
    let rows = predictable_wasm::csv::rows(text);
    let Some(header) = rows.first() else {
        return String::new();
    };
    let keys: Vec<&str> = sampled.iter().map(|(_, k)| k.as_str()).collect();
    let mut out = String::new();
    let render = |row: &Vec<String>| {
        row.iter()
            .map(|cell| {
                if cell.contains([',', '"', '\n']) {
                    format!("\"{}\"", cell.replace('"', "\"\""))
                } else {
                    cell.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    out.push_str(&render(header));
    out.push('\n');
    for row in rows.iter().skip(1) {
        if row.iter().any(|cell| keys.contains(&cell.as_str())) {
            out.push_str(&render(row));
            out.push('\n');
        }
    }
    out
}

/// Pre-bake `--with-traces` (§1.4). The traces come from the *same*
/// `predictable-wasm` session the page's embedded engine would use, so a pack
/// exported with `--engine wasm` cannot disagree with itself.
fn bake_traces(
    inputs: &Inputs,
    components: &[String],
    sampled: &[(u32, String)],
) -> Result<Vec<Value>, String> {
    if components.is_empty() {
        return Ok(Vec::new());
    }
    let session = Session::new(inputs).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    // Enough to be useful, bounded so a pack cannot quietly become 40 MB of
    // JSON: the engine build is what makes the rest reachable.
    for (_, key) in sampled.iter().take(3) {
        for component in components {
            let trace = session.explain(
                component,
                key,
                Some(0),
                &Default::default(),
                &BTreeMap::new(),
            );
            match trace {
                Ok(trace) => out.push(json!({
                    "component": component,
                    "modelpoint": key,
                    "t": 0,
                    "trace": serde_json::to_value(&trace).map_err(|e| e.to_string())?,
                })),
                Err(e) => return Err(format!("--with-traces {component}: {e}")),
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// The single-file pack. Everything inline: no `<link>`, no `<script src>`, no font.
#[allow(clippy::too_many_arguments)]
fn html_pack(
    manifest: &Manifest,
    manifest_text: &str,
    verified: bool,
    modules: &[(String, String)],
    results_schema: Option<&str>,
    series: &SeriesBlock,
    payload: &Value,
    live: bool,
    baked: &[String],
) -> String {
    let mut s = String::new();
    s.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n");
    s.push_str(&format!(
        "<title>predictable governance pack — {}</title>\n",
        esc(&manifest.run_id)
    ));
    s.push_str(
        "<style>\n\
         :root{color-scheme:light}\n\
         body{font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;padding:2rem;background:#fff;color:#111}\n\
         h1{font-size:1.3rem;margin:0 0 .25rem} h2{font-size:1rem;margin:2rem 0 .5rem;border-bottom:1px solid #ddd;padding-bottom:.25rem}\n\
         table{border-collapse:collapse;width:100%;margin:.5rem 0} td,th{border:1px solid #ddd;padding:.3rem .5rem;text-align:left;vertical-align:top;word-break:break-all}\n\
         th{background:#f6f6f6;width:14rem} pre{background:#f6f6f6;padding:.75rem;overflow-x:auto;white-space:pre-wrap}\n\
         .warn{background:#fff4e5;border:1px solid #e0a800;padding:.5rem .75rem;margin:1rem 0}\n\
         .live{background:#e8f4ff;border:1px solid #4a90d9;padding:.5rem .75rem;margin:1rem 0}\n\
         .numbers td,.numbers th{width:auto;font-variant-numeric:tabular-nums}\n\
         details>summary{cursor:pointer;margin:.5rem 0}\n\
         svg{max-width:100%;height:auto}\n\
         .fan path{fill:none} .fan .base{stroke:#111;stroke-width:2} .fan .band{stroke:#4a90d9;stroke-width:1}\n\
         .fan circle{fill:#4a90d9}\n\
         form{margin:.5rem 0;display:flex;gap:.5rem;flex-wrap:wrap;align-items:center}\n\
         input,select,button{font:inherit;padding:.2rem .4rem}\n\
         @media print{body{padding:0} h2{page-break-before:auto} pre{white-space:pre-wrap}\n\
           details{display:block} details>summary{display:none} .live,form,button{display:none}\n\
           :root{color-scheme:light}}\n\
         </style>\n</head><body>\n",
    );
    s.push_str(&format!(
        "<h1>predictable governance pack</h1>\n<p>run <b>{}</b> — exported {}</p>\n",
        esc(&manifest.run_id),
        esc(&crate::clock::now_iso8601())
    ));

    // The manifest as a visible header, per §1.4.
    s.push_str("<h2>Manifest</h2>\n<table>\n");
    let mut row = |k: &str, v: String| {
        s.push_str(&format!(
            "<tr><th>{}</th><td>{}</td></tr>\n",
            esc(k),
            esc(&v)
        ));
    };
    row("manifest_digest", manifest.manifest_digest.clone());
    row(
        "manifest_digest re-derived",
        if verified {
            "verified".into()
        } else {
            "MISMATCH — this file has been edited".to_string()
        },
    );
    row("model_digest", manifest.inputs.model.digest.clone());
    row(
        "assumption_digest",
        manifest
            .inputs
            .assumptions
            .as_ref()
            .map(|a| a.digest.clone())
            .unwrap_or_else(|| "none".into()),
    );
    row(
        "modelpoint_digest",
        manifest.inputs.modelpoints.digest.clone(),
    );
    row("run_digest", manifest.run_config.digest.clone());
    row("results_digest", manifest.results.digest.clone());
    row("ir_version", manifest.versions.ir_version.clone());
    row("engine_version", manifest.versions.engine_version.clone());
    row(
        "timeline",
        format!(
            "{} × {} periods from {}",
            manifest.timeline.basis, manifest.timeline.periods, manifest.timeline.origin
        ),
    );
    row("outcome", format!("{:?}", manifest.execution.outcome));
    row(
        "modelpoints",
        format!(
            "{} projected, {} trapped",
            manifest.execution.modelpoints_projected, manifest.execution.modelpoints_trapped
        ),
    );
    for table in &manifest.inputs.tables {
        row(
            &format!("table {}", table.name),
            format!(
                "{} ({}{})",
                table.digest,
                table.path,
                if table.drift { ", DRIFT" } else { "" }
            ),
        );
    }
    row(
        "embedded engine",
        if live {
            format!(
                "predictable-wasm {} (wasm32-unknown-unknown)",
                predictable_wasm::version()
            )
        } else {
            "none — traces are limited to the pre-baked set".to_string()
        },
    );
    s.push_str("</table>\n");

    s.push_str(&format!(
        "<div class=\"warn\">This pack embeds the manifest, the model text, the results schema and \
         <b>{} modelpoint(s) × {} component(s)</b> of results as Arrow IPC. \
         Modelpoints outside the sample are <b>not</b> here: read them from \
         <code>results.parquet</code> in the run directory.</div>\n",
        series.keys.len(),
        series.components.len()
    ));

    if live {
        s.push_str(
            "<div class=\"live\">This pack carries the engine (§1.5). Every trace below is \
             <b>recomputed in this page</b> from the embedded model and modelpoints — no network, \
             no server. WASM results are bit-identical to the native run: the IR forbids \
             reassociation, FMA contraction and parallel reductions, and CI asserts byte equality \
             under wasmtime before a pack is allowed to ship an engine.</div>\n",
        );
    } else if !baked.is_empty() {
        s.push_str(&format!(
            "<div class=\"warn\">Traces are pre-baked for <b>{}</b> only. Re-export with \
             <code>--engine wasm</code> to trace any cell.</div>\n",
            esc(&baked.join(", "))
        ));
    }

    // ---- results ---------------------------------------------------------
    s.push_str("<h2>Results</h2>\n");
    s.push_str(&fan_svg(series));
    s.push_str("<details><summary>show the numbers</summary>\n");
    s.push_str(&numbers_table(series));
    s.push_str("</details>\n");

    if live {
        s.push_str(&live_panels(series));
    }

    s.push_str("<h2>Model</h2>\n");
    for (path, text) in modules {
        s.push_str(&format!(
            "<h3>{}</h3>\n<pre>{}</pre>\n",
            esc(path),
            esc(text)
        ));
    }

    if let Some(schema) = results_schema {
        s.push_str(&format!(
            "<h2>Results schema</h2>\n<pre>{}</pre>\n",
            esc(schema)
        ));
    }
    s.push_str(&format!(
        "<h2>manifest.json</h2>\n<pre>{}</pre>\n",
        esc(manifest_text)
    ));

    // The payload last: it is the biggest thing in the file and the page must
    // be readable before it parses.
    s.push_str(&format!(
        "<script type=\"application/json\" id=\"{}\">{}</script>\n",
        PAYLOAD_ELEMENT_ID,
        payload
            .to_string()
            // `</script>` inside JSON would end the element early. This is the
            // only escaping a JSON island needs.
            .replace('<', "\\u003c")
    ));
    if live {
        s.push_str(LOADER_JS);
    }
    s.push_str("</body></html>\n");
    s
}

/// The first component, drawn as a line per sampled modelpoint — the same
/// "computed points only, visible markers" rule §3.3 sets for the fan, applied
/// to the static case. No interpolation: every marker is a value in the table.
fn fan_svg(series: &SeriesBlock) -> String {
    let Some(component) = series.components.first() else {
        return String::new();
    };
    let values = &series.values[component];
    let finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() || series.periods.len() < 2 {
        return String::new();
    }
    let (lo, hi) = finite
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = if (hi - lo).abs() < f64::EPSILON {
        1.0
    } else {
        hi - lo
    };
    let (w, h) = (720.0_f64, 200.0_f64);
    let periods = series.periods.len();
    let x = |i: usize| 40.0 + (w - 60.0) * (i as f64) / ((periods - 1) as f64);
    let y = |v: f64| h - 30.0 - (h - 50.0) * (v - lo) / span;

    let mut s = format!(
        "<svg class=\"fan\" viewBox=\"0 0 {w} {h}\" role=\"img\" \
         aria-label=\"{} by period\">\n",
        esc(component)
    );
    for (lane, _) in series.keys.iter().enumerate().take(24) {
        let mut d = String::new();
        for i in 0..periods {
            let v = values[lane * periods + i];
            if !v.is_finite() {
                continue;
            }
            d.push_str(&format!(
                "{}{:.2} {:.2}",
                if d.is_empty() { "M" } else { "L" },
                x(i),
                y(v)
            ));
            d.push(' ');
        }
        s.push_str(&format!(
            "<path class=\"{}\" d=\"{d}\"/>\n",
            if lane == 0 { "base" } else { "band" }
        ));
    }
    s.push_str(&format!(
        "<text x=\"40\" y=\"14\" font-size=\"12\">{} — {} modelpoint(s), \
         t = {}..{}</text>\n</svg>\n",
        esc(component),
        series.keys.len(),
        series.periods.first().copied().unwrap_or(0),
        series.periods.last().copied().unwrap_or(0)
    ));
    s
}

/// "Show the numbers" (§1.4): auditors copy numbers, so every chart has a table.
fn numbers_table(series: &SeriesBlock) -> String {
    let mut s = String::from("<table class=\"numbers\"><thead><tr><th>mp</th><th>t</th>");
    for component in &series.components {
        s.push_str(&format!("<th>{}</th>", esc(component)));
    }
    s.push_str("</tr></thead><tbody>\n");
    let periods = series.periods.len();
    // Bounded: the payload holds every sampled cell, the table shows the first
    // 2,000 rows so the file opens instantly in a browser.
    let mut printed = 0usize;
    'outer: for (lane, key) in series.keys.iter().enumerate() {
        for (i, t) in series.periods.iter().enumerate() {
            s.push_str(&format!("<tr><td>{}</td><td>{t}</td>", esc(key)));
            for component in &series.components {
                let v = series.values[component][lane * periods + i];
                // `{v}` prints `NaN` and `inf` as themselves: a cell the run
                // did not produce is stated, never blanked into a zero.
                s.push_str(&format!("<td>{v}</td>"));
            }
            s.push_str("</tr>\n");
            printed += 1;
            if printed >= 2_000 {
                break 'outer;
            }
        }
    }
    s.push_str("</tbody></table>\n");
    if printed >= 2_000 {
        s.push_str(
            "<p>Table truncated at 2,000 rows; the embedded Arrow payload holds every sampled \
             cell.</p>\n",
        );
    }
    s
}

/// The two things an engine unlocks (§1.5): trace any cell, and move an
/// assumption without a round trip.
fn live_panels(series: &SeriesBlock) -> String {
    let components = series
        .components
        .iter()
        .map(|c| format!("<option>{}</option>", esc(c)))
        .collect::<String>();
    let modelpoints = series
        .keys
        .iter()
        .take(200)
        .map(|k| format!("<option>{}</option>", esc(k)))
        .collect::<String>();
    format!(
        "<h2>Explain any cell</h2>\n\
         <p>Replayed in this page by the embedded engine. `E0901` is asserted on every trace: a \
         replay that disagreed with the run is an error, not a rendering.</p>\n\
         <form id=\"explain-form\">\n\
           <label>component <select id=\"explain-component\">{components}</select></label>\n\
           <label>modelpoint <select id=\"explain-mp\">{modelpoints}</select></label>\n\
           <label>t <input id=\"explain-t\" type=\"number\" value=\"0\" min=\"0\" size=\"4\"></label>\n\
           <button type=\"submit\">trace</button>\n\
         </form>\n\
         <pre id=\"explain-out\">—</pre>\n\
         <h2>Sensitivity</h2>\n\
         <p>Each band is a real projection with its own lineage (`group_id`, `label`, `varied[]`). \
         Only computed points are drawn — no interpolation, ever.</p>\n\
         <form id=\"fan-form\">\n\
           <label>assumption <input id=\"fan-assumption\" size=\"22\" value=\"mortality_loading\"></label>\n\
           <label>× <input id=\"fan-factors\" size=\"22\" value=\"0.9,1.0,1.1,1.25\"></label>\n\
           <label>output <select id=\"fan-component\">{components}</select></label>\n\
           <button type=\"submit\">run</button>\n\
         </form>\n\
         <div id=\"fan-out\"></div>\n"
    )
}

/// The loader: `pv_alloc` / `pv_call` / `pv_free` over a length-prefixed UTF-8
/// buffer, plus the two panels above. Thirty lines, checked in, no generated
/// glue — which is why the pack is one file and why its contents are
/// re-derivable from this repository alone.
const LOADER_JS: &str = r#"<script>
(() => {
  const payload = JSON.parse(document.getElementById("predictable-payload").textContent);
  if (!payload.engine) return;
  const bytes = Uint8Array.from(atob(payload.engine.wasm_base64), (c) => c.charCodeAt(0));
  const out = document.getElementById("explain-out");
  let call = null;

  WebAssembly.instantiate(bytes, {}).then(({ instance }) => {
    const { pv_alloc, pv_free, pv_call, memory } = instance.exports;
    const enc = new TextEncoder(), dec = new TextDecoder();
    call = (request) => {
      const body = enc.encode(JSON.stringify(request));
      const ptr = pv_alloc(body.length);
      new Uint8Array(memory.buffer, ptr, body.length).set(body);
      const res = pv_call(ptr, body.length);
      const len = new DataView(memory.buffer).getUint32(res, true);
      const text = dec.decode(new Uint8Array(memory.buffer, res + 4, len));
      pv_free(res, len + 4);
      return JSON.parse(text);
    };
    window.predictableEngine = { call, version: () => payload.engine.version };
    document.dispatchEvent(new Event("predictable-engine-ready"));
  }).catch((e) => { out.textContent = "the embedded engine did not load: " + e; });

  const inputs = () => payload.engine.inputs;

  document.getElementById("explain-form").addEventListener("submit", (e) => {
    e.preventDefault();
    if (!call) return;
    const r = call({
      op: "explain", inputs: inputs(),
      component: document.getElementById("explain-component").value,
      modelpoint: document.getElementById("explain-mp").value,
      t: Number(document.getElementById("explain-t").value),
      depth: -1,
    });
    out.textContent = r.ok ? JSON.stringify(r.trace, null, 2) : r.kind + ": " + r.error;
  });

  document.getElementById("fan-form").addEventListener("submit", (e) => {
    e.preventDefault();
    if (!call) return;
    const name = document.getElementById("fan-assumption").value.trim();
    const base = inputs().assumptions[name];
    const component = document.getElementById("fan-component").value;
    const scenarios = document.getElementById("fan-factors").value.split(",")
      .map((f) => f.trim()).filter(Boolean)
      .map((f) => ({ label: name + " × " + f, set: { [name]: base * Number(f) } }));
    const r = call({
      op: "sensitivity", inputs: inputs(), scenarios, components: [component],
      modelpoints: [document.getElementById("explain-mp").value],
      group_id: "pack-fan-" + name,
    });
    const host = document.getElementById("fan-out");
    if (!r.ok) { host.textContent = r.kind + ": " + r.error; return; }
    const rows = [["scenario", "varied", ...r.base.t.map((t) => "t=" + t)]];
    rows.push(["base", "—", ...r.base.columns[component]]);
    for (const s of r.scenarios) {
      const varied = s.lineage.varied.map((v) => v.path + " " + v.from + "→" + v.to).join("; ");
      rows.push([s.lineage.label, varied, ...s.series.columns[component]]);
    }
    host.innerHTML = "<table class=\"numbers\"><tbody>" + rows.map((row, i) =>
      "<tr>" + row.map((c) => (i === 0 ? "<th>" + c + "</th>" : "<td>" + c + "</td>")).join("") +
      "</tr>").join("") + "</tbody></table>";
  });
})();
</script>
"#;

/// Standard base64, no line breaks. Twenty lines beats a dependency in a crate
/// that already compiles slowly enough.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }
}
