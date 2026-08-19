//! Pass 2: every identifier binds to something, and binds to exactly one thing.
//!
//! Two rules, both from `01-ir.md` §7:
//!
//! 1. **Unresolved names get a Levenshtein suggestion restricted to the correct
//!    namespace.** A misspelled table name is never "corrected" to a builtin,
//!    and a misspelled component is never corrected to a table: `@` and `(` put
//!    them in different namespaces (§4.2), so the suggestion pool follows the
//!    use site, not the whole model.
//! 2. **Shadowing across modules is an error, not a silent win.** Unqualified
//!    names are globally unique within a product (§2.2), so `a.loading` and
//!    `b.loading` are `E0204` at the second declaration.

use std::collections::BTreeMap;

use predictable_diagnostics::Diagnostic;
use predictable_syntax::expr::Lag;
use predictable_syntax::source::{SourceMap, Span};
use predictable_syntax::PirDocument;

use crate::emit::{self, diagnostic, inner};
use crate::world::{nearest, Ns, World};

/// Report unresolved names in every component expression and `init`.
pub fn check(
    map: &SourceMap,
    world: &World,
    docs: &[(usize, &PirDocument)],
    out: &mut Vec<Diagnostic>,
) {
    for (_, doc) in docs {
        for component in &doc.components {
            for (root, id) in [("expr", component.expr), ("init", component.init)] {
                let Some(id) = id else { continue };
                for reference in doc.arena.references(id, root) {
                    match reference.lag {
                        Lag::Table => {
                            if world.tables.contains_key(&reference.name) {
                                continue;
                            }
                            let near = nearest(&reference.name, world.table_names());
                            out.push(unresolved(
                                map,
                                &reference.name,
                                reference.span,
                                "table",
                                near.map(|n| (n, "a table")),
                            ));
                        }
                        _ => {
                            if world.values.contains_key(&reference.name) {
                                continue;
                            }
                            let near = nearest(&reference.name, world.value_names());
                            out.push(unresolved(
                                map,
                                &reference.name,
                                reference.span,
                                "value",
                                near.map(|n| {
                                    (
                                        n,
                                        world
                                            .lookup_value(n)
                                            .map(|s| s.ns.describe_with_article())
                                            .unwrap_or("a component"),
                                    )
                                }),
                            ));
                        }
                    }
                }
            }
        }
    }
}

/// `namespace` is the namespace of the *use site* — "table" for `t@(k)`,
/// "value" for a bare identifier — and the suggestion is drawn from it alone.
fn unresolved(
    map: &SourceMap,
    name: &str,
    span: Span,
    namespace: &str,
    suggestion: Option<(&str, &str)>,
) -> Diagnostic {
    let (label, note) = if namespace == "table" {
        (
            "no `[[table]]` is declared with this name",
            format!("`{name}@(...)` is a table lookup, so `{name}` is resolved in the table namespace alone — a component or a builtin of the same name would not satisfy it (01-ir.md §4.2)."),
        )
    } else {
        (
            "nothing in the model, the modelpoint schema or the assumption set is named this",
            format!("`{name}` is resolved against components, modelpoint fields, assumptions and timeline inputs (01-ir.md §7 pass 2)."),
        )
    };
    let mut d = diagnostic("E0203", format!("cannot find `{name}` in this model"))
        .span(emit::primary(map, span, label))
        .note(note);
    if let Some((near, kind)) = suggestion {
        d = d.suggestion(emit::replace(
            map,
            // Only the identifier is replaced; `x[t-1]`'s index stays put.
            Span::new(
                span.file,
                span.start as usize,
                span.start as usize + name.len(),
            ),
            near,
            format!("there is {kind} named `{near}`"),
        ));
    } else {
        d = d.suggestion(emit::maybe(emit::replace(
            map,
            span,
            name,
            "declare it, or correct the spelling",
        )));
    }
    d
}

/// `E0204`: the same unqualified name declared in two modules of one product.
pub fn shadowing(map: &SourceMap, docs: &[(usize, &PirDocument)], out: &mut Vec<Diagnostic>) {
    // name -> (module, span) of the first declaration.
    let mut seen: BTreeMap<String, (String, Span)> = BTreeMap::new();

    for (_, doc) in docs {
        let module = doc
            .module
            .as_ref()
            .map(|m| m.value.clone())
            .unwrap_or_default();
        let declarations = doc
            .components
            .iter()
            .map(|c| (c.name.value.clone(), c.name.span, Ns::Component))
            .chain(
                doc.modelpoint_fields
                    .iter()
                    .map(|f| (f.name.value.clone(), f.name.span, Ns::ModelpointField)),
            )
            .chain(
                doc.assumptions
                    .iter()
                    .map(|a| (a.name.value.clone(), a.name.span, Ns::Assumption)),
            );

        for (name, span, ns) in declarations {
            match seen.get(&name) {
                Some((first_module, first_span)) if *first_module != module => {
                    out.push(
                        diagnostic(
                            "E0204",
                            format!("`{name}` is already defined in module `{first_module}`"),
                        )
                        .span(emit::primary(
                            map,
                            inner(span),
                            format!("redefined here, in module `{module}`"),
                        ))
                        .span(emit::secondary(
                            map,
                            inner(*first_span),
                            format!("first defined here, in module `{first_module}`"),
                        ))
                        .note(
                            "Unqualified names are globally unique within a product (01-ir.md \
                             §2.2), so `a.loading` and `b.loading` cannot coexist: the qualified \
                             form is presentational, not disambiguating.",
                        )
                        .suggestion(emit::maybe(emit::replace(
                            map,
                            inner(span),
                            format!("{module}_{name}"),
                            format!("rename one of them, e.g. to `{module}_{name}`"),
                        ))),
                    );
                }
                Some(_) => {}
                None => {
                    seen.insert(name, (module.clone(), span));
                    let _ = ns;
                }
            }
        }
    }
}
