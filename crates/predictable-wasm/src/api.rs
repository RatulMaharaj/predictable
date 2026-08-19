//! The JSON call surface — `03-engine.md` §9's four calls, plus the two the
//! viz layer needs (`sensitivity`, `render`).
//!
//! One entry point, [`call`], taking a JSON request and returning a JSON
//! response. It never panics on bad input and it never returns a bare string
//! error: a response is either `{"ok": true, …}` or `{"ok": false, "kind": …,
//! "error": …}`, and `kind` is what the page maps onto the `DataSource`
//! contract's status codes (`datasource.ts`: `unknown_component` → 404,
//! `unsupported` → 501).
//!
//! Requests are self-contained. There is no session handle, no `open`/`close`
//! pair and no hidden state, because the alternative — a handle table living
//! across `pv_call` boundaries — is exactly the kind of thing that makes a
//! browser answer differ from a CLI answer after the twentieth call.

use std::collections::BTreeMap;

use predictable_engine::explain::ExplainOptions;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::session::{Inputs, Session, SessionError};

/// A `pvf/1` response envelope. `kind` is stable; `error` is human text.
fn err(kind: &str, message: impl std::fmt::Display) -> Value {
    json!({"ok": false, "kind": kind, "error": message.to_string()})
}

fn kind_of(e: &SessionError) -> &'static str {
    match e {
        SessionError::Check(_) => "check_failed",
        SessionError::UnknownModelpoint(_) => "unknown_modelpoint",
        SessionError::UnknownComponent(_) => "unknown_component",
        SessionError::MissingColumn(_) | SessionError::BadValue { .. } => "bad_modelpoints",
        SessionError::NoTimeline => "no_timeline",
        SessionError::Engine(_) => "engine",
    }
}

/// One scenario of a sensitivity fan (`05-viz.md` §3.3, `01-ir.md` Q12).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Scenario {
    /// What the legend calls it, e.g. `mortality +10%`.
    pub label: String,
    /// Assumption name → value for this scenario. Applied over the base.
    pub set: BTreeMap<String, f64>,
}

/// The `varied[]` entry Q12 requires: re-derivable by diffing two manifests,
/// and emitted here so the page never has to infer it.
#[derive(Debug, Clone, Serialize)]
pub struct Varied {
    /// `assumptions.<name>`.
    pub path: String,
    /// The base value.
    pub from: f64,
    /// The scenario's value.
    pub to: f64,
}

/// Handle one request. Returns JSON text — never an error, always a response.
pub fn call(request: &str) -> String {
    let value: Value = match serde_json::from_str(request) {
        Ok(v) => v,
        Err(e) => return err("bad_request", e).to_string(),
    };
    let op = value.get("op").and_then(Value::as_str).unwrap_or("");
    let response = match op {
        "version" => json!({"ok": true, "version": crate::version(),
                            "ir_version": "pir/1",
                            "chunk": crate::session::MAX_CHUNK}),
        "check" => check(&value),
        "plan" => plan(&value),
        "run" => run(&value),
        "explain" => explain(&value),
        "sensitivity" => sensitivity(&value),
        "render" => render(&value),
        other => err("unknown_op", format!("`{other}` is not an operation")),
    };
    response.to_string()
}

fn inputs_of(v: &Value) -> Result<Inputs, Value> {
    serde_json::from_value(v.get("inputs").cloned().unwrap_or(Value::Null))
        .map_err(|e| err("bad_request", format!("`inputs`: {e}")))
}

fn session_of(v: &Value) -> Result<Session, Value> {
    let inputs = inputs_of(v)?;
    Session::new(&inputs).map_err(|e| match &e {
        SessionError::Check(diags) => json!({
            "ok": false,
            "kind": "check_failed",
            "error": e.to_string(),
            "diagnostics": diags,
        }),
        _ => err(kind_of(&e), e),
    })
}

/// `check(pir_text) -> Diagnostic[]` (§9). Every diagnostic, not just errors:
/// the page shows lints the same way the CLI does.
fn check(v: &Value) -> Value {
    let inputs = match inputs_of(v) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let checked: Vec<predictable_check::Input> = inputs
        .sources
        .iter()
        .map(|s| predictable_check::Input::new(s.name.clone(), s.text.clone()))
        .collect();
    let result = predictable_check::check(&checked);
    json!({
        "ok": true,
        "checked": result.is_ok(),
        "diagnostics": result.diagnostics,
    })
}

/// `plan(program, config)` (§9): the three digests and the evaluation order,
/// which is what makes a pack's page provably the same plan as the run's.
fn plan(v: &Value) -> Value {
    let session = match session_of(v) {
        Ok(s) => s,
        Err(e) => return e,
    };
    json!({
        "ok": true,
        "model_digest": session.model_digest(),
        "plan_digest": session.plan_digest(),
        "order_digest": session.order_digest(),
        "periods": session.periods(),
        "outputs": session.outputs(),
        "modelpoints": session.modelpoint_keys(),
    })
}

fn overrides_of(v: &Value) -> BTreeMap<String, f64> {
    v.get("overrides")
        .and_then(|o| serde_json::from_value(o.clone()).ok())
        .unwrap_or_default()
}

fn keys_of(v: &Value) -> Vec<String> {
    v.get("modelpoints")
        .and_then(|m| serde_json::from_value(m.clone()).ok())
        .unwrap_or_default()
}

fn components_of(v: &Value) -> Vec<String> {
    v.get("components")
        .and_then(|m| serde_json::from_value(m.clone()).ok())
        .unwrap_or_default()
}

/// `run(plan, modelpoints)` (§9), returning columns rather than Arrow IPC.
///
/// **Deviation from §9, deliberate.** Emitting Arrow IPC from the engine would
/// put `arrow-ipc` in the wasm dependency graph for the sake of a format the
/// page immediately re-wraps with `tableFromArrays`. The `DataSource` contract
/// is unaffected — `WasmEngine.series()` still returns an Arrow `Table` — and
/// the pack's *stored* results stay base64 Arrow IPC as §1.4 requires. What
/// crosses the ABI is `{names, mp, t, columns}`; see `wasm/engine.ts`.
fn run(v: &Value) -> Value {
    let session = match session_of(v) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let wanted = components_of(v);
    match session.project(&keys_of(v), &overrides_of(v)) {
        Ok(p) => series_value(&session, &p, &wanted),
        Err(e) => err(kind_of(&e), e),
    }
}

/// Long-form columns: one row per `(mp, t)`, which is the shape the grid, the
/// waterfall and the fan all read.
fn series_value(session: &Session, p: &crate::session::Projection, wanted: &[String]) -> Value {
    let names: Vec<String> = if wanted.is_empty() {
        p.columns.keys().cloned().collect()
    } else {
        wanted.to_vec()
    };
    let mut resolved = Vec::new();
    for name in &names {
        match p
            .columns
            .keys()
            .find(|k| *k == name || k.rsplit('.').next() == Some(name.as_str()))
        {
            Some(k) => resolved.push(k.clone()),
            None => {
                return err(
                    "unknown_component",
                    format!("`{name}` is not an output of this model"),
                )
            }
        }
    }

    let periods = p.periods as usize + 1;
    let mut mp = Vec::with_capacity(p.keys.len() * periods);
    let mut t = Vec::with_capacity(p.keys.len() * periods);
    for key in &p.keys {
        for period in 0..periods {
            mp.push(key.clone());
            t.push(period as u32);
        }
    }
    let mut columns = serde_json::Map::new();
    for (name, key) in names.iter().zip(&resolved) {
        let stride = p.strides.get(key).copied().unwrap_or(1);
        let values = &p.columns[key];
        let mut flat = Vec::with_capacity(mp.len());
        for lane in 0..p.keys.len() {
            for period in 0..periods {
                // A `PerMP` output has one value per lane; it repeats down the
                // period axis rather than being dropped, so a fan can plot it
                // beside a series without a second query shape.
                let index = lane * stride + if stride == 1 { 0 } else { period };
                flat.push(values.get(index).copied().unwrap_or(f64::NAN));
            }
        }
        columns.insert(name.clone(), json!(flat));
    }

    json!({
        "ok": true,
        "model_digest": session.model_digest(),
        "plan_digest": session.plan_digest(),
        "rows": mp.len(),
        "mp": mp,
        "t": t,
        "columns": columns,
        "dropped": p.dropped,
    })
}

/// `explain(...)` (§9) — the replay, in the page.
fn explain(v: &Value) -> Value {
    let session = match session_of(v) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some(component) = v.get("component").and_then(Value::as_str) else {
        return err("bad_request", "`component` is required");
    };
    let Some(modelpoint) = v.get("modelpoint").and_then(Value::as_str) else {
        return err("bad_request", "`modelpoint` is required");
    };
    let t = v.get("t").and_then(Value::as_u64).map(|t| t as u32);
    let options = ExplainOptions {
        depth: v.get("depth").and_then(Value::as_i64).unwrap_or(2) as i32,
        expand: v
            .get("expand")
            .and_then(|e| serde_json::from_value(e.clone()).ok())
            .unwrap_or_default(),
        values_only: v
            .get("values_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..ExplainOptions::default()
    };
    match session.explain(component, modelpoint, t, &options, &overrides_of(v)) {
        Ok(trace) => match serde_json::to_value(&trace) {
            Ok(trace) => json!({"ok": true, "trace": trace}),
            Err(e) => err("engine", e),
        },
        Err(e) => err(kind_of(&e), e),
    }
}

/// The fan (`05-viz.md` §3.3): N projections varying one assumption set, each
/// carrying the Q12 lineage fields that make it a real run in its own right.
///
/// No interpolation, ever — the response contains only computed scenarios, and
/// the page draws markers at exactly those points.
fn sensitivity(v: &Value) -> Value {
    let session = match session_of(v) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let scenarios: Vec<Scenario> = match v.get("scenarios").cloned() {
        Some(s) => match serde_json::from_value(s) {
            Ok(s) => s,
            Err(e) => return err("bad_request", format!("`scenarios`: {e}")),
        },
        None => return err("bad_request", "`scenarios` is required"),
    };
    let group_id = v
        .get("group_id")
        .and_then(Value::as_str)
        .unwrap_or("sensitivity")
        .to_string();
    let parent_run = v
        .get("parent_run")
        .and_then(Value::as_str)
        .map(String::from);
    let wanted = components_of(v);
    let keys = keys_of(v);

    let base = match session.project(&keys, &BTreeMap::new()) {
        Ok(p) => p,
        Err(e) => return err(kind_of(&e), e),
    };
    let base_series = series_value(&session, &base, &wanted);
    if base_series.get("ok") != Some(&Value::Bool(true)) {
        return base_series;
    }
    let base_assumptions = inputs_of(v).map(|i| i.assumptions).unwrap_or_default();

    let mut out = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        let projection = match session.project(&keys, &scenario.set) {
            Ok(p) => p,
            Err(e) => return err(kind_of(&e), e),
        };
        let series = series_value(&session, &projection, &wanted);
        if series.get("ok") != Some(&Value::Bool(true)) {
            return series;
        }
        let varied: Vec<Varied> = scenario
            .set
            .iter()
            .map(|(name, to)| Varied {
                path: format!("assumptions.{name}"),
                from: base_assumptions.get(name).copied().unwrap_or(f64::NAN),
                to: *to,
            })
            .collect();
        out.push(json!({
            "lineage": {
                "parent_run": parent_run,
                "parent_manifest_digest": v.get("parent_manifest_digest").cloned()
                    .unwrap_or(Value::Null),
                "group_id": group_id,
                "label": scenario.label,
                "varied": varied,
            },
            "series": series,
        }));
    }
    json!({
        "ok": true,
        "group_id": group_id,
        "base": base_series,
        "scenarios": out,
        "interpolated": false,
    })
}

/// The determinism gate's rendering: every emitted value as its IEEE-754 bit
/// pattern (`tests/wasm_gate.rs`). Text, not JSON numbers, because a decimal
/// rendering would hide the last ULP — the exact divergence the gate exists to
/// catch.
fn render(v: &Value) -> Value {
    let inputs = match inputs_of(v) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let name = v.get("name").and_then(Value::as_str).unwrap_or("case");
    match crate::render::render_case(name, &inputs) {
        Ok(text) => json!({"ok": true, "render": text}),
        Err(e) => err(kind_of(&e), e),
    }
}
