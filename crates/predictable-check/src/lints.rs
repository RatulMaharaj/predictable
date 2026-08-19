//! Pass 6: the `W01xx` lints of `01-ir.md` §7.
//!
//! Lints run **only when passes 1–5 found no errors**. A broken model produces
//! misleading lints — an unresolved name looks unused, a mistyped component
//! looks constant — and a wall of warnings underneath a real error is how a
//! toolchain teaches people to ignore warnings. Every lint here fires on a model
//! that otherwise checks clean, which is exactly what the conformance corpus's
//! `v07-lints` case asserts.

use std::collections::BTreeSet;

use predictable_diagnostics::Diagnostic;
use predictable_syntax::ast::{Kind, Shape, Timing, Unit};
use predictable_syntax::expr::{BinOp, Expr, ExprId};
use predictable_syntax::raw::RawDocument;
use predictable_syntax::source::SourceMap;
use predictable_syntax::PirDocument;

use crate::emit::{self, diagnostic, inner};
use crate::infer::{self, Ctx, ShapeJoin};
use crate::world::World;

/// Names read anywhere: by another component, by an `init`, or by the run file.
#[derive(Debug, Default)]
pub struct Reads(pub BTreeSet<String>);

/// Every lint, in declaration order so the output is deterministic.
pub fn check(
    map: &SourceMap,
    world: &World,
    docs: &[(usize, &PirDocument)],
    raws: &[(usize, &RawDocument)],
    reads: &Reads,
    out: &mut Vec<Diagnostic>,
) {
    for (index, doc) in docs {
        let raw = raws.iter().find(|(i, _)| i == index).map(|(_, r)| *r);
        for component in &doc.components {
            let mut mine = Vec::new();
            if let Some(expr) = component.expr {
                let ctx = Ctx {
                    map,
                    world,
                    doc,
                    component,
                };
                let mut sink = Vec::new();
                let info = infer::infer(&ctx, expr, &mut sink);

                money_without_unit(&ctx, raw, component, expr, &info, &mut mine);
                constant_series(&ctx, raw, component, expr, &info, &mut mine);
                timing_walk(&ctx, expr, &mut mine);
            }
            // One lint per component, and the specific one wins. `W0101` says
            // "nothing reads this"; when the component *also* has a unit or a
            // timing problem, that is the thing to say first — a reader who
            // fixes it will keep the component, and a reader who deletes it does
            // not need two reasons.
            if mine.is_empty() {
                unused(map, raw, component, reads, &mut mine);
            }
            out.append(&mut mine);
        }
    }
}

/// `W0101` — declared, never read, and not an `Output`.
fn unused(
    map: &SourceMap,
    raw: Option<&RawDocument>,
    component: &predictable_syntax::Component,
    reads: &Reads,
    out: &mut Vec<Diagnostic>,
) {
    if matches!(
        component.kind,
        Kind::Output
            | Kind::InputModelpoint
            | Kind::InputAssumption
            | Kind::InputTable
            | Kind::InputTimeline
    ) {
        return;
    }
    if reads.0.contains(&component.name.value) {
        return;
    }
    let name = &component.name.value;
    out.push(
        diagnostic("W0101", format!("`{name}` is never read"))
            .span(emit::primary(
                map,
                inner(component.name.span),
                "nothing depends on this component, and it is not an Output",
            ))
            .note(
                "A component nothing reads is either a leftover or a missing `kind = \"Output\"`. \
                 It costs a lane in every projection either way.",
            )
            .suggestion(emit::maybe(emit::replace(
                map,
                key_span(raw, &component.name.value, "kind")
                    .unwrap_or_else(|| inner(component.name.span)),
                "Output",
                format!("mark `{name}` as an Output, or delete it"),
            ))),
    );
}

/// `W0102` — money-shaped arithmetic declared `unit = "none"`.
fn money_without_unit(
    ctx: &Ctx<'_>,
    raw: Option<&RawDocument>,
    component: &predictable_syntax::Component,
    expr: ExprId,
    info: &infer::Info,
    out: &mut Vec<Diagnostic>,
) {
    if component.unit != Unit::None || info.unit != Some(Unit::Money) {
        return;
    }
    let span = ctx.doc.arena.span(expr);
    out.push(
        diagnostic("W0102", "money arithmetic with unit = \"none\"")
            .span(emit::primary(
                ctx.map,
                span,
                format!(
                    "this expression is `money`, but `{}` is declared `none`",
                    component.name.value
                ),
            ))
            .note(
                "`unit = \"none\"` opts out of dimensional checking, so nothing downstream will \
                 catch a money value added to a probability (01-ir.md §2.4).",
            )
            .suggestion(emit::replace(
                ctx.map,
                key_span(raw, &component.name.value, "unit")
                    .unwrap_or_else(|| inner(component.name.span)),
                "money",
                "declare the unit the arithmetic already implies",
            )),
    );
}

/// `W0104` — a `Series` that varies with neither the modelpoint nor `t`.
fn constant_series(
    ctx: &Ctx<'_>,
    raw: Option<&RawDocument>,
    component: &predictable_syntax::Component,
    expr: ExprId,
    info: &infer::Info,
    out: &mut Vec<Diagnostic>,
) {
    if component.shape != Shape::Series || info.shape.rank() != Shape::Scalar.rank() {
        return;
    }
    let span = ctx.doc.arena.span(expr);
    out.push(
        diagnostic("W0104", "constant across modelpoints and time")
            .span(emit::primary(
                ctx.map,
                span,
                "nothing here depends on a modelpoint value, on `t`, or on a lag",
            ))
            .note(
                "The engine will hoist this out of the projection loop anyway; declaring it a \
                 Scalar assumption says the same thing to a reader (01-ir.md §7).",
            )
            .suggestion(emit::maybe(emit::replace(
                ctx.map,
                key_span(raw, &component.name.value, "shape")
                    .unwrap_or_else(|| inner(component.name.span)),
                "Scalar",
                "make it a Scalar",
            ))),
    );
}

/// `W0103` and `W0105`, both of which are about a node rather than a component.
fn timing_walk(ctx: &Ctx<'_>, id: ExprId, out: &mut Vec<Diagnostic>) {
    match ctx.doc.arena.get(id) {
        Expr::Binary {
            op: BinOp::Add | BinOp::Sub,
            lhs,
            rhs,
        } => {
            if let (Some(a), Some(b)) = (direct_timing(ctx, *lhs), direct_timing(ctx, *rhs)) {
                if a != b {
                    mixed_timing(ctx, id, *rhs, a, b, out);
                }
            }
        }
        Expr::Call { func, args } if predictable_ir::is_timing_op(func) => {
            if let Some(arg) = args.first() {
                let mut sink = Vec::new();
                let info = infer::infer(ctx, *arg, &mut sink);
                if info.shape.rank() < Shape::Series.rank() {
                    untimed(ctx, id, func, out);
                }
            }
        }
        _ => {}
    }
    for child in ctx.doc.arena.children(id) {
        timing_walk(ctx, child, out);
    }
}

/// The timing of an operand, but only when it is a bare reference to a `Series`
/// component: `premium + claims` is a timing mismatch, `premium[t-1] +
/// claims[t-1]` is not — those are already-computed values from a period that
/// has closed.
fn direct_timing(ctx: &Ctx<'_>, id: ExprId) -> Option<Timing> {
    match ctx.doc.arena.get(id) {
        Expr::Ref(name) => ctx.world.lookup_value(name).and_then(|s| s.timing),
        _ => None,
    }
}

fn mixed_timing(
    ctx: &Ctx<'_>,
    id: ExprId,
    rhs: ExprId,
    a: Timing,
    b: Timing,
    out: &mut Vec<Diagnostic>,
) {
    let span = ctx.doc.arena.span(id);
    let rhs_span = ctx.doc.arena.span(rhs);
    let rhs_text = infer::snippet(ctx.map, rhs_span);
    out.push(
        diagnostic("W0103", format!("adding a `{a}` flow to an `{b}` flow"))
            .span(emit::primary(
                ctx.map,
                span,
                format!("one side is realised at the {a} of period t, the other at the {b}"),
            ))
            .note(
                "This is a warning, not an error: the sum is well defined, but it is half a \
                 period out. Say which you meant (01-ir.md §2.5).",
            )
            .suggestion(emit::maybe(emit::replace(
                ctx.map,
                rhs_span,
                format!("shift({rhs_text}, 1)"),
                "move the second flow into the same period",
            )))
            .suggestion(emit::maybe(emit::replace(
                ctx.map,
                rhs_span,
                format!("retime({rhs_text}, {a})"),
                format!("or retime it to `{a}` and accept the approximation"),
            ))),
    );
}

fn untimed(ctx: &Ctx<'_>, id: ExprId, func: &str, out: &mut Vec<Diagnostic>) {
    let span = ctx.doc.arena.span(id);
    let inner_text = match ctx.doc.arena.get(id) {
        Expr::Call { args, .. } if !args.is_empty() => {
            infer::snippet(ctx.map, ctx.doc.arena.span(args[0]))
        }
        _ => String::new(),
    };
    out.push(
        diagnostic(
            "W0105",
            format!("`{func}` on a value that has no timing"),
        )
        .span(emit::primary(
            ctx.map,
            span,
            "this call has no effect",
        ))
        .note(
            "An `Agg` result is a PerMP and is untimed — the timing of the series was consumed by \
             the reduction (01-ir.md §2.5). Each of retime/shift/cum/diff is a well-defined no-op \
             here, so the model still runs; it just does not do what it looks like it does.",
        )
        .suggestion(emit::maybe(emit::replace(
            ctx.map,
            span,
            inner_text,
            format!("drop the `{func}`, or apply it one level down, to the flow"),
        ))),
    );
}

/// The span of a component key's value, for a suggestion that edits a
/// declaration rather than a formula.
fn key_span(
    raw: Option<&RawDocument>,
    component: &str,
    key: &str,
) -> Option<predictable_syntax::source::Span> {
    crate::decls::value_span(raw?, "component", component, key)
}
