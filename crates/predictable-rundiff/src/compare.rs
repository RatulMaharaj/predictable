//! The algorithm of `04-verify.md` §5.3: align, compare, localise, classify, rank.

use std::collections::{BTreeMap, BTreeSet};

use predictable_ir::Unit;
use predictable_modeldiff::ModelSide;

use crate::graph::DepGraph;
use crate::mapping::{matches_predictable, ComponentMap, Mapping};
use crate::model::*;
use crate::run_side::{RunSide, Value};
use crate::tolerance::{self, Profile, SourceTol, Tol, ToleranceReport};
use crate::{DiffOptions, RunDiff};

/// How one `a` component lines up with one `b` component.
#[derive(Debug, Clone)]
struct Pair {
    a_id: String,
    b_id: String,
    entry: Option<ComponentMap>,
}

/// The whole diff (§5.3).
pub fn diff_runs(a: &RunSide, b: &RunSide, opts: &DiffOptions) -> RunDiff {
    let mut tolerances = ToleranceReport::new(opts.profile.clone());

    // ---- 1. align -------------------------------------------------------
    let (pairs, mut structural) = align_components(a, b, opts.mapping.as_ref());
    let (common_mps, mp_counts) = align_modelpoints(a, b, opts, &mut structural);

    let emit_mismatch = emit_mismatch(a, b);
    let graph = build_graph(opts);
    let model_changes = model_changes(opts);

    let mut diff = RunDiff {
        a_path: a.path.clone(),
        b_path: b.path.clone(),
        a_run: a.run_id().to_string(),
        b_run: b.run_id().to_string(),
        a_system: a.system().to_string(),
        b_system: b.system().to_string(),
        a_manifest_digest: a.manifest.manifest_digest.clone(),
        b_manifest_digest: b.manifest.manifest_digest.clone(),
        a_engine_version: a.manifest.versions.engine_version.clone(),
        b_engine_version: b.manifest.versions.engine_version.clone(),
        a_model_digest: a.manifest.inputs.model.digest.clone(),
        b_model_digest: b.manifest.inputs.model.digest.clone(),
        mapping: opts.mapping.clone(),
        verdict: Verdict::Matched,
        incomparable: Vec::new(),
        emit_mismatch,
        components: SetCounts {
            a: a.components.len(),
            b: b.components.len(),
            common: pairs.len(),
            only_a: structural.only_in_a.len(),
            only_b: structural.only_in_b.len(),
        },
        modelpoints: mp_counts,
        cells: CellCounts::default(),
        outputs: Vec::new(),
        findings: Vec::new(),
        findings_truncated: None,
        structural,
        first_divergence: None,
        root_divergences: 0,
        tolerances: ToleranceReport::new(opts.profile.clone()),
        model_attribution_available: opts.model_a.is_some() && opts.model_b.is_some(),
        graph_available: !graph.is_empty(),
    };

    // ---- structurally incomparable (§5.1) -------------------------------
    if a.manifest.timeline.periods != b.manifest.timeline.periods {
        diff.incomparable.push(format!(
            "timeline.periods differ: a has {}, b has {}",
            a.manifest.timeline.periods, b.manifest.timeline.periods
        ));
    }
    if common_mps.is_empty() {
        diff.incomparable
            .push("the modelpoint sets are disjoint".to_string());
    }
    if opts.require_same_emit && diff.emit_mismatch.is_some() {
        diff.incomparable
            .push("--require-same-emit and the two runs' component sets differ".to_string());
    }
    if !diff.incomparable.is_empty() {
        diff.verdict = Verdict::Incomparable;
        return diff;
    }

    // ---- 2/3. compare, and localise per component -----------------------
    let mut per_component: Vec<ComponentResult> = Vec::new();
    for pair in &pairs {
        let result = compare_component(a, b, pair, &common_mps, opts, &mut tolerances);
        diff.cells.compared += result.compared;
        diff.cells.diverged += result.cells.len() as u64;
        diff.cells.absorbed_by_override += result.absorbed;
        if result.max_abs > diff.cells.max_abs {
            diff.cells.max_abs = result.max_abs;
        }
        if result.max_rel > diff.cells.max_rel {
            diff.cells.max_rel = result.max_rel;
        }
        per_component.push(result);
    }
    diff.tolerances = tolerances;

    // ---- summary.outputs, and the output whose movement gets apportioned -
    for result in &per_component {
        let Some(descriptor) = b.descriptor(&result.b_id) else {
            continue;
        };
        if !descriptor.output {
            continue;
        }
        let tol = diff
            .tolerances
            .resolutions
            .get(&result.b_id)
            .map(|r| r.tol)
            .unwrap_or(opts.profile.base);
        diff.outputs.push(OutputTotal {
            component: result.b_id.clone(),
            a_total: result.a_total,
            b_total: result.b_total,
            abs: (result.b_total - result.a_total).abs(),
            rel: tolerance::rel_diff(result.a_total, result.b_total),
            within_tolerance: tolerance::matches(result.a_total, result.b_total, tol),
        });
    }
    let headline = diff
        .outputs
        .iter()
        .max_by(|x, y| x.abs.total_cmp(&y.abs))
        .filter(|o| o.abs > 0.0)
        .cloned();

    // ---- 4/5/6. classify, attribute, rank -------------------------------
    let diverging: BTreeSet<&str> = per_component
        .iter()
        .filter(|r| !r.cells.is_empty())
        .map(|r| r.b_id.as_str())
        .collect();

    let mut findings: Vec<Finding> = Vec::new();
    for result in &per_component {
        if let Some(filter) = &opts.filter_component {
            if !name_matches(&result.b_id, filter) {
                continue;
            }
        }
        if result.cells.is_empty() {
            // A component whose only differences an override absorbed is still worth a line:
            // a loosened tolerance must never be invisible (§5.6).
            if result.absorbed > 0 {
                findings.push(tolerance_only_finding(a, b, result, &diff));
            }
            continue;
        }
        findings.push(value_finding(
            a,
            b,
            result,
            &graph,
            &diverging,
            &per_component,
            &model_changes,
            headline.as_ref(),
            opts,
        ));
    }
    // Structural findings: a one-sided component that no emit difference explains is a surprise,
    // and a surprise belongs in the findings list, not only in a footnote.
    if diff.emit_mismatch.is_none() && a.system() == b.system() {
        for one in diff
            .structural
            .only_in_a
            .iter()
            .chain(diff.structural.only_in_b.iter())
        {
            findings.push(structural_finding(one, a, b));
        }
    }

    rank(&mut findings);
    diff.root_divergences = findings.iter().filter(|f| f.class == Class::Root).count();
    // "Earliest" means the earliest *timestep*, and a root before what inherited from it. `t = -1`
    // is not a timestep at all — it is a `PerMP` aggregate over the whole series — so it sorts
    // last rather than first, where a naive numeric minimum would put it.
    diff.first_divergence = findings
        .iter()
        .filter(|f| f.class == Class::Root || f.class == Class::Inherited)
        .min_by_key(|f| {
            (
                u8::from(f.class != Class::Root),
                if f.t_first < 0 { i32::MAX } else { f.t_first },
                f.component.clone(),
            )
        })
        .map(|f| FirstDivergence {
            component: f.component.clone(),
            t: f.t_first,
            mp_key: f.exemplar.mp_key.clone(),
        });

    diff.verdict = if findings.iter().any(|f| f.class != Class::ToleranceOnly) {
        Verdict::Diverged
    } else {
        Verdict::Matched
    };

    // ---- 7. hypotheses (§5.5) -------------------------------------------
    // After ranking and before truncation, so the report's hypotheses are the ones on the
    // findings the report prints, and the work is bounded by `--top` rather than by the run.
    if opts.top > 0 && findings.len() > opts.top {
        diff.findings_truncated = Some((opts.top, findings.len()));
        findings.truncate(opts.top);
    }
    let ctx = crate::hypothesis::Context {
        a,
        b,
        model: opts.model_b.as_ref(),
        graph: &graph,
        source: &opts.source_b,
    };
    for finding in &mut findings {
        if finding.class == Class::Structural || finding.class == Class::ToleranceOnly {
            continue;
        }
        finding.hypotheses = crate::hypothesis::detect(&ctx, finding);
    }

    diff.findings = findings;
    diff
}

// ---------------------------------------------------------------------------
// alignment
// ---------------------------------------------------------------------------

fn align_components(
    a: &RunSide,
    b: &RunSide,
    mapping: Option<&Mapping>,
) -> (Vec<Pair>, Structural) {
    let mut structural = Structural::default();
    let mut pairs: Vec<Pair> = Vec::new();
    let mut used_b: BTreeSet<String> = BTreeSet::new();

    // The mapping is written prophet-side; it applies to whichever side is not a predictable run,
    // and to `a` when both are (which is the "we renamed a component" case).
    let map_to_b = mapping.is_some() && b.system() != "predictable" && a.system() == "predictable";

    for a_id in a.components.keys() {
        let mut matched: Option<Pair> = None;
        if let Some(m) = mapping {
            if !map_to_b {
                if let Some(entry) = entry_for(m, a_id) {
                    if let Some(b_id) = b
                        .components
                        .keys()
                        .find(|b_id| matches_predictable(entry, b_id))
                    {
                        matched = Some(Pair {
                            a_id: a_id.clone(),
                            b_id: b_id.clone(),
                            entry: Some(entry.clone()),
                        });
                    }
                }
            }
        }
        if matched.is_none() && b.components.contains_key(a_id) {
            matched = Some(Pair {
                a_id: a_id.clone(),
                b_id: a_id.clone(),
                entry: None,
            });
        }
        if matched.is_none() && map_to_b {
            // The mapping names `b`'s columns: find the `b` component whose prophet name maps to
            // this predictable one.
            if let Some(m) = mapping {
                if let Some(entry) = m.components.iter().find(|e| matches_predictable(e, a_id)) {
                    if let Some(b_id) = b.components.keys().find(|b_id| is_named(entry, b_id)) {
                        matched = Some(Pair {
                            a_id: a_id.clone(),
                            b_id: b_id.clone(),
                            entry: Some(entry.clone()),
                        });
                    }
                }
            }
        }
        match matched {
            Some(pair) => {
                used_b.insert(pair.b_id.clone());
                pairs.push(pair);
            }
            None => structural.only_in_a.push(OneSided {
                component: a_id.clone(),
                reason: one_sided_reason(mapping, a_id),
            }),
        }
    }
    for b_id in b.components.keys() {
        if !used_b.contains(b_id) {
            structural.only_in_b.push(OneSided {
                component: b_id.clone(),
                reason: one_sided_reason(mapping, b_id),
            });
        }
    }
    (pairs, structural)
}

/// The mapping entry that governs a component of the Prophet-flavoured side.
///
/// Matched on the Prophet name *or* the predictable one, because an importer may already have
/// renamed the columns. The entry carries `sign`/`scale`/`timing_shift` as well as the rename, and
/// those adjustments must still apply to a column that arrived under its final name.
fn entry_for<'m>(m: &'m Mapping, id: &str) -> Option<&'m ComponentMap> {
    m.components
        .iter()
        .find(|e| is_named(e, id) || matches_predictable(e, id))
}

/// Does this mapping entry name `id` on the Prophet side?
///
/// Case-insensitively: Prophet writes `PREM_INC`, the importer snake-cases it to
/// `prophet.prem_inc`, and a mapping file quotes the column as the `.rpt` spells it. All three
/// are the same column, and a diff that lost the correspondence over letter case would report a
/// full-portfolio structural difference for a naming convention.
fn is_named(entry: &ComponentMap, id: &str) -> bool {
    entry.prophet.eq_ignore_ascii_case(id)
        || id
            .rsplit_once('.')
            .map(|(_, tail)| tail.eq_ignore_ascii_case(&entry.prophet))
            .unwrap_or(false)
}

fn one_sided_reason(mapping: Option<&Mapping>, id: &str) -> String {
    let bare = id.rsplit_once('.').map(|(_, t)| t).unwrap_or(id);
    match mapping {
        Some(m) if m.is_declared_unmapped(bare) || m.is_declared_unmapped(id) => {
            "declared unmapped".to_string()
        }
        Some(_) => "unmapped".to_string(),
        None => "no counterpart".to_string(),
    }
}

/// `mp_key → (a_row, b_row)` for the modelpoints both runs have.
type CommonMps = Vec<(String, u32, u32)>;

fn align_modelpoints(
    a: &RunSide,
    b: &RunSide,
    opts: &DiffOptions,
    structural: &mut Structural,
) -> (CommonMps, SetCounts) {
    let a_by_key: BTreeMap<&str, u32> = a.mp_keys.iter().map(|(r, k)| (k.as_str(), *r)).collect();
    let b_by_key: BTreeMap<&str, u32> = b.mp_keys.iter().map(|(r, k)| (k.as_str(), *r)).collect();

    let mut common: CommonMps = Vec::new();
    for (key, a_row) in &a_by_key {
        match b_by_key.get(key) {
            Some(b_row) => common.push(((*key).to_string(), *a_row, *b_row)),
            None => structural.modelpoints_only_in_a.push((*key).to_string()),
        }
    }
    for key in b_by_key.keys() {
        if !a_by_key.contains_key(key) {
            structural.modelpoints_only_in_b.push((*key).to_string());
        }
    }
    let counts = SetCounts {
        a: a_by_key.len(),
        b: b_by_key.len(),
        common: common.len(),
        only_a: structural.modelpoints_only_in_a.len(),
        only_b: structural.modelpoints_only_in_b.len(),
    };
    // `--mp` narrows what is compared, not merely what is printed: a diff of one modelpoint is a
    // diff of one modelpoint, and its cell counts should say so.
    if let Some(key) = &opts.filter_mp {
        common.retain(|(k, _, _)| k == key);
    }
    // Exemplar order is `b`'s row order, so the same pair of runs always yields the same exemplar.
    common.sort_by_key(|(_, _, b_row)| *b_row);
    (common, counts)
}

fn emit_mismatch(a: &RunSide, b: &RunSide) -> Option<EmitMismatch> {
    if a.schema.emit == b.schema.emit
        && a.schema.component_set_digest == b.schema.component_set_digest
    {
        return None;
    }
    Some(EmitMismatch {
        a: a.schema.emit.clone(),
        b: b.schema.emit.clone(),
        a_component_set_digest: a.schema.component_set_digest.clone(),
        b_component_set_digest: b.schema.component_set_digest.clone(),
    })
}

fn build_graph(opts: &DiffOptions) -> DepGraph {
    // `b` is the model under development, so its graph is the one that explains `b`'s numbers.
    match (&opts.model_b, &opts.model_a) {
        (Some(m), _) | (None, Some(m)) => DepGraph::from_model(m),
        _ => DepGraph::default(),
    }
}

/// `component id → the change classes the model diff reported` (§5.3 step 5).
fn model_changes(opts: &DiffOptions) -> Option<BTreeMap<String, Vec<String>>> {
    let (Some(a), Some(b)) = (&opts.model_a, &opts.model_b) else {
        return None;
    };
    let d = predictable_modeldiff::diff(a, b);
    let graph = DepGraph::from_model(b);
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let qualify = |name: &str| -> String { graph.qualify(name).unwrap_or(name).to_string() };
    for c in &d.changed {
        out.entry(qualify(&c.name))
            .or_default()
            .extend(c.classes.iter().map(|k| {
                serde_json::to_value(k)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("{k:?}"))
            }));
    }
    for c in d.added.iter().chain(d.removed.iter()) {
        out.entry(qualify(&c.name))
            .or_default()
            .push("declaration".to_string());
    }
    // A table or assumption change is a model change too, and it reaches the components that read
    // it — so the impact set, not just the seed list, is what "explained" means.
    for name in &d.impact.impacted {
        out.entry(qualify(name))
            .or_default()
            .push("upstream".to_string());
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// per-component comparison
// ---------------------------------------------------------------------------

struct ComponentResult {
    a_id: String,
    b_id: String,
    cells: Vec<Cell>,
    compared: u64,
    absorbed: u64,
    max_abs: f64,
    max_rel: f64,
    a_total: f64,
    b_total: f64,
    tol: Tol,
}

fn compare_component(
    a: &RunSide,
    b: &RunSide,
    pair: &Pair,
    common: &CommonMps,
    opts: &DiffOptions,
    tolerances: &mut ToleranceReport,
) -> ComponentResult {
    let a_cells = a.components.get(&pair.a_id);
    let b_cells = b.components.get(&pair.b_id);
    let unit: Option<Unit> = b
        .descriptor(&pair.b_id)
        .map(|d| d.unit.clone())
        .or_else(|| a.descriptor(&pair.a_id).map(|d| d.unit.clone()));
    let source_dp = source_precision(a, b, pair, opts);
    let tol = tolerances.resolve(&pair.b_id, unit.as_ref(), source_dp);
    let base = tolerances.profile.base;

    let shift = pair.entry.as_ref().map(|e| e.timing_shift).unwrap_or(0);
    let mut result = ComponentResult {
        a_id: pair.a_id.clone(),
        b_id: pair.b_id.clone(),
        cells: Vec::new(),
        compared: 0,
        absorbed: 0,
        max_abs: 0.0,
        max_rel: 0.0,
        a_total: 0.0,
        b_total: 0.0,
        tol,
    };

    for (mp_key, a_row, b_row) in common {
        // Both sides' cells for this modelpoint, on `b`'s time axis.
        let mut a_series: BTreeMap<i32, &Value> = BTreeMap::new();
        if let Some(cells) = a_cells {
            for ((row, t), v) in cells.range((*a_row, i32::MIN)..=(*a_row, i32::MAX)) {
                debug_assert_eq!(*row, *a_row);
                a_series.insert(t + shift, v);
            }
        }
        let mut b_series: BTreeMap<i32, &Value> = BTreeMap::new();
        if let Some(cells) = b_cells {
            for ((_, t), v) in cells.range((*b_row, i32::MIN)..=(*b_row, i32::MAX)) {
                b_series.insert(*t, v);
            }
        }

        let ts: BTreeSet<i32> = a_series.keys().chain(b_series.keys()).copied().collect();
        for t in ts {
            let av = a_series.get(&t).copied();
            let bv = b_series.get(&t).copied();
            let adjusted = av.map(|v| adjust(v, pair.entry.as_ref()));
            if let Some(Value::F64(x)) = adjusted.as_ref() {
                result.a_total += *x;
            }
            if let Some(Value::F64(x)) = bv {
                result.b_total += *x;
            }
            match (adjusted.as_ref(), bv) {
                (Some(x), Some(y)) => {
                    result.compared += 1;
                    match compare_values(x, y, tol, base) {
                        Comparison::Same => {}
                        Comparison::Absorbed => result.absorbed += 1,
                        Comparison::Differs { abs, rel, category } => {
                            result.max_abs =
                                result.max_abs.max(if abs.is_finite() { abs } else { 0.0 });
                            result.max_rel =
                                result.max_rel.max(if rel.is_finite() { rel } else { 0.0 });
                            result.cells.push(Cell {
                                mp_key: mp_key.clone(),
                                mp_row: *b_row,
                                t,
                                a: Some(x.clone()),
                                b: Some(y.clone()),
                                abs,
                                rel,
                                category,
                            });
                        }
                    }
                }
                (Some(x), None) => result.cells.push(Cell {
                    mp_key: mp_key.clone(),
                    mp_row: *b_row,
                    t,
                    a: Some(x.clone()),
                    b: None,
                    abs: f64::NAN,
                    rel: f64::NAN,
                    category: Category::Missing,
                }),
                (None, Some(y)) => result.cells.push(Cell {
                    mp_key: mp_key.clone(),
                    mp_row: *b_row,
                    t,
                    a: None,
                    b: Some(y.clone()),
                    abs: f64::NAN,
                    rel: f64::NAN,
                    category: Category::Extra,
                }),
                (None, None) => {}
            }
        }
    }
    result.cells.sort_by_key(|c| (c.t, c.mp_row));
    result
}

enum Comparison {
    Same,
    /// Beyond the profile's own tolerance, inside the override that was applied.
    Absorbed,
    Differs {
        abs: f64,
        rel: f64,
        category: Category,
    },
}

fn compare_values(a: &Value, b: &Value, tol: Tol, base: Tol) -> Comparison {
    match (a, b) {
        (Value::F64(x), Value::F64(y)) => {
            // A NaN or an infinity is never a tolerance question (§5.6, H0501).
            if x.is_nan() || y.is_nan() || x.is_infinite() || y.is_infinite() {
                if x.is_nan() && y.is_nan() {
                    return Comparison::Differs {
                        abs: f64::NAN,
                        rel: f64::NAN,
                        category: Category::Nan,
                    };
                }
                if x == y {
                    return Comparison::Same;
                }
                return Comparison::Differs {
                    abs: (x - y).abs(),
                    rel: f64::NAN,
                    category: Category::Nan,
                };
            }
            if tolerance::matches(*x, *y, tol) {
                if !tolerance::matches(*x, *y, base) {
                    return Comparison::Absorbed;
                }
                return Comparison::Same;
            }
            Comparison::Differs {
                abs: (x - y).abs(),
                rel: tolerance::rel_diff(*x, *y),
                category: Category::Value,
            }
        }
        // Integers, booleans, strings, dates and enums compare exactly; no tolerance applies.
        (Value::I64(x), Value::I64(y)) if x == y => Comparison::Same,
        (Value::Bool(x), Value::Bool(y)) if x == y => Comparison::Same,
        (Value::Str(x), Value::Str(y)) if x == y => Comparison::Same,
        (Value::I64(x), Value::I64(y)) => Comparison::Differs {
            abs: (*x as f64 - *y as f64).abs(),
            rel: tolerance::rel_diff(*x as f64, *y as f64),
            category: Category::Value,
        },
        (Value::Bool(_), Value::Bool(_)) | (Value::Str(_), Value::Str(_)) => Comparison::Differs {
            abs: f64::NAN,
            rel: f64::NAN,
            category: Category::Value,
        },
        // Different lanes entirely: the two runs disagree about what the component *is*.
        _ => Comparison::Differs {
            abs: f64::NAN,
            rel: f64::NAN,
            category: Category::Dtype,
        },
    }
}

fn adjust(v: &Value, entry: Option<&ComponentMap>) -> Value {
    match (v, entry) {
        (Value::F64(x), Some(e)) if e.sign != 1 || e.scale != 1.0 => Value::F64(e.adjust(*x)),
        _ => v.clone(),
    }
}

/// The reporting precision of the side that is not a predictable run (§5.6 rounding rule 4).
fn source_precision(
    a: &RunSide,
    b: &RunSide,
    pair: &Pair,
    opts: &DiffOptions,
) -> Option<SourceTol> {
    if !opts.use_source_precision {
        return None;
    }
    let (side, id) = if a.system() != "predictable" {
        (a, &pair.a_id)
    } else if b.system() != "predictable" {
        (b, &pair.b_id)
    } else {
        return None;
    };
    let cells = side.components.get(id)?;
    let dp = tolerance::observed_precision(cells.values().filter_map(Value::as_f64))?;
    // The precision is the *source's*, so a column reported in thousands is half a unit of a
    // thousand, not half a unit of the compared currency.
    let scale = pair.entry.as_ref().map(|e| e.scale).unwrap_or(1.0);
    Some(SourceTol::new(dp, scale))
}

// ---------------------------------------------------------------------------
// findings
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn value_finding(
    a: &RunSide,
    b: &RunSide,
    result: &ComponentResult,
    graph: &DepGraph,
    diverging: &BTreeSet<&str>,
    all: &[ComponentResult],
    model_changes: &Option<BTreeMap<String, Vec<String>>>,
    headline: Option<&OutputTotal>,
    opts: &DiffOptions,
) -> Finding {
    let t_first = result.cells.iter().map(|c| c.t).min().unwrap_or(-1);
    let t_last = result.cells.iter().map(|c| c.t).max().unwrap_or(-1);
    // "the lowest `mp_row` exhibiting the divergence at `t_first`" — the rule that makes the
    // report byte-identical on a re-run.
    let exemplar = result
        .cells
        .iter()
        .filter(|c| c.t == t_first)
        .min_by_key(|c| c.mp_row)
        .cloned()
        .expect("a finding has at least one cell");
    let worst = result
        .cells
        .iter()
        .max_by(|x, y| {
            x.abs
                .total_cmp(&y.abs)
                .then(y.mp_row.cmp(&x.mp_row))
                .then(y.t.cmp(&x.t))
        })
        .cloned()
        .expect("a finding has at least one cell");
    let category = worst_category(&result.cells);
    let (class, class_basis) = classify(result, graph, diverging, all);

    let affects_outputs = if graph.knows(&result.b_id) {
        graph.outputs_affected(&result.b_id)
    } else {
        b.descriptor(&result.b_id)
            .filter(|d| d.output)
            .map(|d| vec![d.id.clone()])
            .unwrap_or_default()
    };

    let contribution = headline.and_then(|out| {
        if !affects_outputs.contains(&out.component) {
            return None;
        }
        let total = out.b_total - out.a_total;
        if total == 0.0 {
            return None;
        }
        let delta: f64 = result
            .cells
            .iter()
            .filter_map(|c| Some(c.b.as_ref()?.as_f64()? - c.a.as_ref()?.as_f64()?))
            .sum();
        Some(Contribution {
            output: out.component.clone(),
            share_of_total_delta: delta / total,
            method: if result.b_id == out.component {
                "identity".to_string()
            } else {
                "delta_sum_ratio".to_string()
            },
        })
    });

    let explained = model_changes.as_ref().map(|changes| {
        let what = changes.get(&result.b_id).cloned().unwrap_or_default();
        Explained {
            changed: !what.is_empty(),
            what: dedup(what),
        }
    });

    // The `<output>` of the §5.2 line is the affected output the finding is being *ranked* by —
    // the one whose movement it explains. Falling back to the alphabetically first output would
    // name a different output in the headline than in the contribution beside it.
    let headline_output = contribution
        .as_ref()
        .map(|c| c.output.clone())
        .or_else(|| affects_outputs.first().cloned())
        .unwrap_or_else(|| result.b_id.clone());
    let message = message(&headline_output, &result.b_id, &exemplar);

    Finding {
        id: String::new(),
        class,
        category,
        class_basis,
        component: result.b_id.clone(),
        source_component: (result.a_id != result.b_id).then(|| result.a_id.clone()),
        affects_outputs,
        t_first,
        t_range: [t_first, t_last],
        n_modelpoints: result
            .cells
            .iter()
            .map(|c| c.mp_key.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        n_cells: result.cells.len(),
        exemplar: exemplar.clone(),
        worst,
        contribution,
        explained_by_model_change: explained,
        message,
        hypotheses: Vec::new(),
        explain_command: explain_command(a, b, result, &exemplar, opts),
        cells: result.cells.clone(),
    }
}

fn message(output: &str, component: &str, exemplar: &Cell) -> String {
    let bare = |s: &str| s.rsplit_once('.').map(|(_, t)| t).unwrap_or(s).to_string();
    let render = |v: &Option<Value>| match v {
        Some(Value::F64(x)) => format!("{:.2}", tolerance::round_half_away(*x, 2)),
        Some(Value::I64(x)) => x.to_string(),
        Some(Value::Bool(x)) => x.to_string(),
        Some(Value::Str(x)) => x.clone(),
        None => "<absent>".to_string(),
    };
    let mut line = format!(
        "{} diverges at t={} in component {}: {} vs {}",
        bare(output).to_uppercase(),
        exemplar.t,
        bare(component),
        render(&exemplar.a),
        render(&exemplar.b),
    );
    if exemplar.abs.is_finite() {
        line.push_str(&format!(
            "  (Δ {:.2}, {:.1}%)",
            tolerance::round_half_away(exemplar.abs, 2),
            exemplar.rel * 100.0
        ));
    }
    line.push_str(&format!("  [mp {}]", exemplar.mp_key));
    line
}

fn worst_category(cells: &[Cell]) -> Category {
    for wanted in [
        Category::Nan,
        Category::Dtype,
        Category::Missing,
        Category::Extra,
        Category::Shape,
    ] {
        if cells.iter().any(|c| c.category == wanted) {
            return wanted;
        }
    }
    Category::Value
}

/// §5.3 step 4. A diverging component whose inputs at the relevant `t` all agree is a *root*.
fn classify(
    result: &ComponentResult,
    graph: &DepGraph,
    diverging: &BTreeSet<&str>,
    all: &[ComponentResult],
) -> (Class, String) {
    if !graph.knows(&result.b_id) {
        // No IR for this component. Saying "root" without a graph would be a guess dressed as a
        // classification, so the basis says which it is: earliest-`t` order, and nothing more.
        let t_first = result.cells.iter().map(|c| c.t).min().unwrap_or(-1);
        let earlier = all.iter().any(|other| {
            other.b_id != result.b_id
                && !other.cells.is_empty()
                && other.cells.iter().map(|c| c.t).min().unwrap_or(i32::MAX) < t_first
        });
        return (
            if earlier {
                Class::Inherited
            } else {
                Class::Root
            },
            "no_graph_earliest_t".to_string(),
        );
    }
    let t_first = result.cells.iter().map(|c| c.t).min().unwrap_or(-1);
    let mut unknown_input = false;
    for edge in graph.inputs_of(&result.b_id) {
        if !diverging.contains(edge.input.as_str()) {
            // Either the input agrees, or it was never emitted. Emission is the difference
            // between "checked" and "unknown", and only the latter weakens the claim. An edge to
            // something the graph does not hold as a component — an assumption, a modelpoint
            // field, a table — is not an unknown: it has no cells to diverge in.
            if graph.knows(&edge.input) && !all.iter().any(|r| r.b_id == edge.input) {
                unknown_input = true;
            }
            continue;
        }
        let Some(input) = all.iter().find(|r| r.b_id == edge.input) else {
            unknown_input = true;
            continue;
        };
        // `None` is `x@k`, an absolute reference: any divergence in `x` can reach this cell.
        let relevant_t = edge.offset.map(|off| t_first + off);
        let hits = if t_first < 0 {
            // A `PerMP`/`Scalar` component is an aggregate over the whole series (`sum`, `npv`),
            // so *any* divergence in an input reaches it. Asking for "the input at t = -1" would
            // classify every aggregate as a root divergence.
            !input.cells.is_empty()
        } else {
            match relevant_t {
                // `c.t < 0` because a `PerMP` input diverges once, for every `t` that reads it.
                Some(t) => input.cells.iter().any(|c| c.t == t || c.t < 0),
                None => true,
            }
        };
        if hits {
            return (Class::Inherited, "ir_graph".to_string());
        }
    }
    (
        Class::Root,
        if unknown_input {
            "partial_graph".to_string()
        } else {
            "ir_graph".to_string()
        },
    )
}

fn tolerance_only_finding(
    a: &RunSide,
    b: &RunSide,
    result: &ComponentResult,
    diff: &RunDiff,
) -> Finding {
    let rule = diff
        .tolerances
        .resolutions
        .get(&result.b_id)
        .map(|r| r.rule.describe())
        .unwrap_or_else(|| "profile".to_string());
    let empty = Cell {
        mp_key: String::new(),
        mp_row: 0,
        t: -1,
        a: None,
        b: None,
        abs: f64::NAN,
        rel: f64::NAN,
        category: Category::Value,
    };
    let _ = (a, b);
    Finding {
        id: String::new(),
        class: Class::ToleranceOnly,
        category: Category::Value,
        class_basis: "override_absorbed".to_string(),
        component: result.b_id.clone(),
        source_component: (result.a_id != result.b_id).then(|| result.a_id.clone()),
        affects_outputs: Vec::new(),
        t_first: -1,
        t_range: [-1, -1],
        n_modelpoints: 0,
        n_cells: result.absorbed as usize,
        exemplar: empty.clone(),
        worst: empty,
        contribution: None,
        explained_by_model_change: None,
        message: format!(
            "{} matches only because {} loosened the tolerance to abs {} / rel {}: {} cells are outside the {} profile",
            result.b_id, rule, result.tol.abs, result.tol.rel, result.absorbed, diff.tolerances.profile.name
        ),
        hypotheses: Vec::new(),
        explain_command: String::new(),
        cells: Vec::new(),
    }
}

fn structural_finding(one: &OneSided, a: &RunSide, b: &RunSide) -> Finding {
    let side = if a.components.contains_key(&one.component) {
        "a"
    } else {
        "b"
    };
    let empty = Cell {
        mp_key: String::new(),
        mp_row: 0,
        t: -1,
        a: None,
        b: None,
        abs: f64::NAN,
        rel: f64::NAN,
        category: Category::Value,
    };
    let _ = b;
    Finding {
        id: String::new(),
        class: Class::Structural,
        category: if side == "a" {
            Category::Missing
        } else {
            Category::Extra
        },
        class_basis: "component_set".to_string(),
        component: one.component.clone(),
        source_component: None,
        affects_outputs: Vec::new(),
        t_first: -1,
        t_range: [-1, -1],
        n_modelpoints: 0,
        n_cells: 0,
        exemplar: empty.clone(),
        worst: empty,
        contribution: None,
        explained_by_model_change: None,
        message: format!(
            "{} is present only in {side} ({})",
            one.component, one.reason
        ),
        hypotheses: Vec::new(),
        explain_command: String::new(),
        cells: Vec::new(),
    }
}

fn explain_command(
    a: &RunSide,
    b: &RunSide,
    result: &ComponentResult,
    exemplar: &Cell,
    opts: &DiffOptions,
) -> String {
    let _ = opts;
    let bare = result
        .b_id
        .rsplit_once('.')
        .map(|(_, t)| t)
        .unwrap_or(&result.b_id);
    format!(
        "predictable explain {} --component {} --mp {} --t {}   # a: {}",
        b.path, bare, exemplar.mp_key, exemplar.t, a.path
    )
}

/// §5.4: sorted by class then contribution, descending — with unexplained roots promoted above
/// explained ones, because unexplained divergence is where bugs live (§5.3 step 5).
fn rank(findings: &mut [Finding]) {
    findings.sort_by(|x, y| {
        rank_key(x)
            .cmp(&rank_key(y))
            .then_with(|| share(y).total_cmp(&share(x)))
            .then_with(|| x.t_first.cmp(&y.t_first))
            .then_with(|| x.component.cmp(&y.component))
    });
    for (i, f) in findings.iter_mut().enumerate() {
        f.id = format!("F{:03}", i + 1);
    }
}

fn rank_key(f: &Finding) -> u8 {
    match f.class {
        Class::Root => match &f.explained_by_model_change {
            Some(e) if !e.changed => 0, // unexplained: promoted
            _ => 1,
        },
        Class::Inherited => 2,
        Class::Structural => 3,
        Class::ToleranceOnly => 4,
    }
}

fn share(f: &Finding) -> f64 {
    f.contribution
        .as_ref()
        .map(|c| c.share_of_total_delta.abs())
        .unwrap_or(0.0)
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn name_matches(id: &str, filter: &str) -> bool {
    id == filter
        || id
            .rsplit_once('.')
            .map(|(_, tail)| tail == filter)
            .unwrap_or(false)
}

/// Models, loaded for the graph and the model-diff attribution.
pub fn load_model(files: &[std::path::PathBuf], label: &str) -> Option<ModelSide> {
    if files.is_empty() {
        return None;
    }
    ModelSide::load(label, files)
        .ok()
        .filter(|s| !s.modules.is_empty())
}

/// The profile a `Profile` value came from, for the header line.
pub fn profile_line(p: &Profile) -> String {
    format!("{}   abs {}  rel {}", p.name, p.base.abs, p.base.rel)
}
