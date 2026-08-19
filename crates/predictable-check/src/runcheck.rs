//! The product manifest (§8.4.1) and the run-scoped seventh pass (§7, §8.3,
//! §8.4.3).
//!
//! The seventh pass runs at `predictable check <run>.pir` and at `predictable
//! run`, never at model-check time: a model stays checkable without a run.

use std::collections::BTreeSet;

use predictable_diagnostics::Diagnostic;
use predictable_syntax::ast::{DType, Shape};
use predictable_syntax::raw::{RawDocument, Value};
use predictable_syntax::source::{SourceMap, Span};
use predictable_syntax::PirDocument;

use crate::emit::{self, diagnostic, inner};
use crate::world::{Ns, World};

/// `E0105` and `E0107`: the closed module set and the validated output manifest.
pub fn product(
    map: &SourceMap,
    world: &World,
    docs: &[(usize, &PirDocument)],
    product_doc: &PirDocument,
    product_raw: &RawDocument,
    out: &mut Vec<Diagnostic>,
) {
    let Some(product) = &product_doc.product else {
        return;
    };
    let section = product_raw.section("product");

    // ---- E0105: imported but not listed ----------------------------------
    let listed: BTreeSet<&str> = product.modules.iter().map(|m| leaf(m)).collect();
    let mut reported = BTreeSet::new();
    for (_, doc) in docs {
        for import in &doc.imports {
            let name = leaf(import);
            if listed.contains(name) || !reported.insert(name.to_string()) {
                continue;
            }
            let importer = doc
                .module
                .as_ref()
                .map(|m| m.value.clone())
                .unwrap_or_default();
            let span = section
                .and_then(|s| s.table.get("modules"))
                .map(|v| v.span)
                .unwrap_or(product.span);
            out.push(
                diagnostic(
                    "E0105",
                    format!("`{name}` is imported but not listed in `modules`"),
                )
                .span(emit::primary(
                    map,
                    span,
                    format!("module `{importer}` imports `{name}`, which this list does not name"),
                ))
                .note(
                    "`modules` is the closed module set: exactly the modules loaded, in the order \
                     given, and that order is the `module_path` order used by the evaluation-order \
                     tiebreak and by `model_digest` (01-ir.md §8.4.1).",
                )
                .suggestion(emit::replace(
                    map,
                    span,
                    add_to_list(map, span, name),
                    format!("add `{name}` to `modules`"),
                )),
            );
        }
    }

    // ---- E0107: the outputs manifest -------------------------------------
    let declared: BTreeSet<&str> = product.outputs.iter().map(String::as_str).collect();
    let actual: BTreeSet<&str> = docs
        .iter()
        .flat_map(|(_, d)| d.outputs())
        .map(|c| c.name.value.as_str())
        .collect();
    if declared != actual {
        let missing: Vec<&str> = actual.difference(&declared).copied().collect();
        let extra: Vec<&str> = declared.difference(&actual).copied().collect();
        let span = section
            .and_then(|s| s.table.get("outputs"))
            .map(|v| v.span)
            .unwrap_or(product.span);
        let mut d = diagnostic("E0107", "product output list disagrees with the model")
            .span(emit::primary(
                map,
                span,
                "this list must equal the set of `kind = \"Output\"` components",
            ))
            .note(
                "`outputs` is a validated manifest, not a selector (01-ir.md §2.2): a reviewer of \
                 the product file sees the promised result set without reading every module, and \
                 adding an Output shows up as a diff in two places.",
            );
        if !missing.is_empty() {
            d = d.note(format!(
                "Declared `kind = \"Output\"` but absent from the list: {}.",
                quoted(&missing)
            ));
        }
        if !extra.is_empty() {
            d = d.note(format!(
                "Listed here but not an `Output` in the model: {}.",
                quoted(&extra)
            ));
        }
        let mut correct: Vec<&str> = actual.iter().copied().collect();
        correct.sort_unstable();
        out.push(d.suggestion(emit::replace(
            map,
            span,
            format!(
                "[{}]",
                correct
                    .iter()
                    .map(|n| format!("\"{n}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "list exactly the model's Output components",
        )));
    }

    let _ = world;
}

/// The seventh pass: `emit_list`, `[[aggregation]]` and `[[solve]]`.
pub fn run(
    map: &SourceMap,
    world: &World,
    run_doc: &PirDocument,
    run_raw: &RawDocument,
    out: &mut Vec<Diagnostic>,
) {
    let Some(run) = &run_doc.run else { return };
    let section = run_raw.section("run");

    // ---- E0106: emit_list ids resolve ------------------------------------
    for (i, id) in run.emit_list.iter().enumerate() {
        if world.component(id).is_some() {
            continue;
        }
        let span = section
            .and_then(|s| s.table.get("emit_list"))
            .map(|v| element_span(v, i))
            .unwrap_or(run.span);
        out.push(
            diagnostic("E0106", format!("`{id}` does not resolve to a component"))
                .span(emit::primary(
                    map,
                    span,
                    "no component in the product has this name",
                ))
                .note(
                    "`emit = \"list\"` emits `emit_list` ∪ outputs, and every id in the list must \
                     resolve (01-ir.md §8.4.3). `emit` never removes an Output.",
                )
                .suggestion(match crate::world::nearest(id, world.value_names()) {
                    Some(near) => emit::replace(
                        map,
                        span,
                        near,
                        format!("there is a component named `{near}`"),
                    ),
                    None => emit::maybe(emit::delete(map, span, "drop it from `emit_list`")),
                }),
        );
    }

    // ---- E0402 / E0403 / E0404: aggregation shapes -----------------------
    let sections: Vec<&predictable_syntax::raw::Section> =
        run_raw.sections_named("aggregation").collect();
    for (i, aggregation) in run_doc.aggregations.iter().enumerate() {
        let table = sections.get(i).map(|s| &s.table);
        for (k, key) in aggregation.group_by.iter().enumerate() {
            let span = table
                .and_then(|t| t.get("group_by"))
                .map(|v| element_span(v, k))
                .unwrap_or(aggregation.span);
            let Some(sym) = world.lookup_value(key) else {
                out.push(unresolved_run(map, world, key, span, "group key"));
                continue;
            };
            if sym.shape == Shape::Series {
                out.push(
                    diagnostic("E0402", format!("group key `{key}` is a `Series`"))
                        .span(emit::primary(
                            map,
                            span,
                            "a group whose membership changes with t is not a group",
                        ))
                        .note(
                            "`group_by` keys are `PerMP` or `Scalar` (01-ir.md §8.3). Reduce the \
                             series to one value per modelpoint first.",
                        )
                        .suggestion(emit::maybe(emit::replace(
                            map,
                            span,
                            format!("{key}_at_issue"),
                            format!("group on a PerMP derived from `{key}`, e.g. `at({key}, 0)`"),
                        ))),
                );
                continue;
            }
            if sym.dtype == DType::F64 {
                out.push(
                    diagnostic("E0403", format!("group key `{key}` is `f64`"))
                        .span(emit::primary(map, span, "a float is not a group identity"))
                        .note(
                            "Group keys are `i64`, `bool`, `str`, `date` or `enum(...)` (01-ir.md \
                             §8.3): band the value into an enum or an i64 first, so the banding is \
                             a named, diffable thing in the model.",
                        )
                        .suggestion(emit::maybe(emit::replace(
                            map,
                            span,
                            format!("{key}_band"),
                            format!("band `{key}` into an i64 or enum component and group on that"),
                        ))),
                );
            }
        }

        // measure, filter and weight must resolve; over_t is only for a Series
        // measure.
        let measure_shape = match world.lookup_value(&aggregation.measure) {
            Some(sym) => Some(sym.shape),
            None => {
                let span = table
                    .and_then(|t| t.get("measure"))
                    .map(|v| inner(v.span))
                    .unwrap_or(aggregation.span);
                out.push(unresolved_run(
                    map,
                    world,
                    &aggregation.measure,
                    span,
                    "measure",
                ));
                None
            }
        };
        if let Some(filter) = &aggregation.filter {
            if world.lookup_value(filter).is_none() {
                let span = table
                    .and_then(|t| t.get("filter"))
                    .map(|v| inner(v.span))
                    .unwrap_or(aggregation.span);
                out.push(unresolved_run(map, world, filter, span, "filter"));
            }
        }
        if aggregation.over_t.is_some() && measure_shape == Some(Shape::PerMP) {
            let span = table
                .and_then(|t| t.get("over_t"))
                .map(|v| inner(v.span))
                .unwrap_or(aggregation.span);
            out.push(
                diagnostic("E0404", "`over_t` is only for a `Series` measure")
                    .span(emit::primary(
                        map,
                        span,
                        format!(
                            "`{}` is a PerMP: it has no t axis to fold",
                            aggregation.measure
                        ),
                    ))
                    .note(
                        "For a Series measure, `over_t` chooses one row per (group, t) or one row \
                         per group summed over t. For a PerMP measure it must be absent (01-ir.md \
                         §8.3).",
                    )
                    .suggestion(emit::delete(
                        map,
                        Span::new(
                            span.file,
                            emit::line_range(map, span).start,
                            emit::line_range(map, span).end,
                        ),
                        "remove the `over_t` key",
                    )),
            );
        }
    }

    // ---- [[solve]] targets and varied inputs resolve ---------------------
    let solve_sections: Vec<&predictable_syntax::raw::Section> =
        run_raw.sections_named("solve").collect();
    for (i, solve) in run_doc.solves.iter().enumerate() {
        let table = solve_sections.get(i).map(|s| &s.table);
        let known_aggregation = run_doc.aggregations.iter().any(|a| a.name == solve.target);
        if world.lookup_value(&solve.target).is_none() && !known_aggregation {
            let span = table
                .and_then(|t| t.get("target"))
                .map(|v| inner(v.span))
                .unwrap_or(solve.span);
            out.push(unresolved_run(
                map,
                world,
                &solve.target,
                span,
                "solve target",
            ));
        }
        if world.lookup_value(&solve.vary).is_none() {
            let span = table
                .and_then(|t| t.get("vary"))
                .map(|v| inner(v.span))
                .unwrap_or(solve.span);
            out.push(unresolved_run(
                map,
                world,
                &solve.vary,
                span,
                "varied input",
            ));
        }
    }
}

fn unresolved_run(
    map: &SourceMap,
    world: &World,
    name: &str,
    span: Span,
    role: &str,
) -> Diagnostic {
    let mut d = diagnostic("E0203", format!("cannot find `{name}` in this model"))
        .span(emit::primary(
            map,
            span,
            format!("the {role} names nothing in the product"),
        ))
        .note(
            "A run file is checked against the product it names, so every id in it is a component, \
             a modelpoint field or an assumption (01-ir.md §7 pass 7).",
        );
    if let Some(near) = crate::world::nearest(name, world.value_names()) {
        d = d.suggestion(emit::replace(
            map,
            span,
            near,
            format!("did you mean `{near}`?"),
        ));
    } else {
        d = d.suggestion(emit::maybe(emit::replace(
            map,
            span,
            name,
            "declare it in the model, or correct the spelling",
        )));
    }
    d
}

/// The span of element `i` of an array value, falling back to the whole array.
///
/// The corpus's convention: a list value carets its `[`, unless one element is
/// at fault, in which case it carets that element (`conformance/README.md` §3).
fn element_span(value: &predictable_syntax::source::Spanned<Value>, i: usize) -> Span {
    match &value.value {
        Value::Array(items) => items.get(i).map(|s| inner(s.span)).unwrap_or(value.span),
        _ => value.span,
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rewrite a `["a", "b"]` list with one more element, as a literal edit.
fn add_to_list(map: &SourceMap, span: Span, name: &str) -> String {
    let text = map.snippet(span);
    match text.rfind(']') {
        Some(i) if text[..i].trim_end().ends_with('[') => format!("[\"{name}\"]"),
        Some(i) => format!("{}, \"{name}\"]", &text[..i]),
        None => text.to_string(),
    }
}

/// Names read by a run file, so that `W0101` does not fire on a component whose
/// only reader is an `[[aggregation]]` filter.
pub fn run_reads(run_doc: &PirDocument, into: &mut BTreeSet<String>) {
    if let Some(run) = &run_doc.run {
        into.extend(run.emit_list.iter().cloned());
    }
    for a in &run_doc.aggregations {
        into.insert(a.measure.clone());
        into.extend(a.group_by.iter().cloned());
        into.extend(a.filter.iter().cloned());
        into.extend(a.weight.iter().cloned());
    }
    for s in &run_doc.solves {
        into.insert(s.target.clone());
        into.insert(s.vary.clone());
    }
}

/// `Ns` is re-exported here so run checks can talk about namespaces without
/// pulling the world module into scope at every call site.
pub type Namespace = Ns;
