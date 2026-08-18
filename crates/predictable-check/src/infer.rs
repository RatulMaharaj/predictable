//! Passes 4 and 5: the shape lattice, dtypes, units, timing tags, lookup arity
//! and the presence-bit rule.
//!
//! All four run in one walk of the expression, because they are one question
//! asked four ways: *what is this subexpression, and is it allowed here?* The
//! walk returns an [`Info`] for every node and reports as it goes, so a single
//! `expr` yields every independent problem in it rather than the first.
//!
//! Two rules keep the noise down:
//!
//! * **Literals are unit-polymorphic** (§2.4). `sum_assured + 100.0` is `money`,
//!   not an error, because the literal unifies with its context. That is
//!   represented as `unit: None` — "no opinion" — not as `Unit::None`.
//! * **Only the expression is unit-checked, never the declaration.** `unit` on a
//!   component is an assertion about what the value *is*; §2.4's rules are about
//!   what arithmetic *does*. Checking one against the other would reject
//!   `floor(money / money)` declared `factor`, which §6's own worked example
//!   writes.

use predictable_diagnostics::Diagnostic;
use predictable_syntax::ast::{DType, RateBasis, Shape, Timing, Unit};
use predictable_syntax::expr::{BinOp, Expr, ExprId, Lit, UnaryOp};
use predictable_syntax::source::{SourceMap, Span};
use predictable_syntax::{Component, PirDocument};

use crate::emit::{self, diagnostic};
use crate::world::{Ns, World};

/// What a subexpression is.
#[derive(Debug, Clone)]
pub struct Info {
    pub shape: Shape,
    pub dtype: DType,
    /// `None` means unit-polymorphic (a literal, or something unresolved).
    pub unit: Option<Unit>,
    /// The timing tag a `Series` value carries, when it is knowable.
    pub timing: Option<Timing>,
}

impl Info {
    fn poly(dtype: DType) -> Info {
        Info {
            shape: Shape::Scalar,
            dtype,
            unit: None,
            timing: None,
        }
    }
}

/// Everything one expression walk needs.
pub struct Ctx<'a> {
    pub map: &'a SourceMap,
    pub world: &'a World,
    pub doc: &'a PirDocument,
    pub component: &'a Component,
}

impl std::fmt::Debug for Ctx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("component", &self.component.name.value)
            .finish_non_exhaustive()
    }
}

impl<'a> Ctx<'a> {
    fn span(&self, id: ExprId) -> Span {
        self.doc.arena.span(id)
    }
}

/// Walk `id`, reporting pass-5 problems into `out`.
pub fn infer(ctx: &Ctx<'_>, id: ExprId, out: &mut Vec<Diagnostic>) -> Info {
    let span = ctx.span(id);
    match ctx.doc.arena.get(id) {
        Expr::Error => Info::poly(DType::F64),
        Expr::Lit(lit) => Info::poly(match lit {
            Lit::Int(_) => DType::I64,
            Lit::Float(_) => DType::F64,
            Lit::Bool(_) => DType::Bool,
            Lit::Str(_) => DType::Str,
        }),
        Expr::Ref(name) => match ctx.world.lookup_value(name) {
            Some(sym) => Info {
                shape: sym.shape,
                dtype: sym.dtype.clone(),
                unit: Some(sym.unit.clone()),
                timing: sym.timing,
            },
            None => Info::poly(DType::F64),
        },
        Expr::Lag { name, k: _ } => {
            let Some(sym) = ctx.world.lookup_value(name) else {
                return Info::poly(DType::F64);
            };
            // §2.7: a pre-origin `Lag` falls back to `init`, else to the dtype's
            // zero — and `date`, `str` and `enum` have no zero.
            if !has_zero(&sym.dtype) && !sym.has_init && sym.ns == Ns::Component {
                let d = diagnostic(
                    "E0509",
                    format!("`{}` has no zero value; declare an `init`", sym.dtype),
                )
                .span(emit::primary(
                    ctx.map,
                    span,
                    "at t = 0 this reads before the start of the projection",
                ))
                .span(emit::secondary(
                    ctx.map,
                    sym.name_span,
                    format!("`{}` is declared `{}` here", sym.name, sym.dtype),
                ))
                .note(
                    "A lag before t = 0 falls back to the component's `init`, or to the zero of \
                     its dtype. `date`, `str` and `enum` have no zero, so the `init` is required.",
                )
                .suggestion(emit::maybe(emit::replace(
                    ctx.map,
                    emit::inner(ctx.component.name.span),
                    format!("{}\"\ninit = \"<value at t = 0>", ctx.component.name.value),
                    "give the component an `init` for t = 0",
                )));
                out.push(d);
            }
            Info {
                shape: sym.shape,
                dtype: sym.dtype.clone(),
                unit: Some(sym.unit.clone()),
                timing: None,
            }
        }
        Expr::At { name, k } => {
            if let Some(periods) = ctx.world.periods {
                if i64::from(*k) > periods {
                    let index_span = index_span(ctx.map, span).unwrap_or(span);
                    out.push(
                        diagnostic(
                            "E0205",
                            format!("period {k} is beyond the projection horizon"),
                        )
                        .span(emit::primary(
                            ctx.map,
                            index_span,
                            format!("the timeline declares periods = {periods}, so t runs 0..={periods}"),
                        ))
                        .note(
                            "`x[k]` with a literal `k` and a known `T` is a definition-time error, \
                             not a run-time trap (01-ir.md §2.7).",
                        )
                        .suggestion(emit::maybe(emit::replace(
                            ctx.map,
                            index_span,
                            periods.to_string(),
                            format!("read the last projected period, {periods}"),
                        ))),
                    );
                }
            }
            match ctx.world.lookup_value(name) {
                Some(sym) => Info {
                    shape: sym.shape,
                    dtype: sym.dtype.clone(),
                    unit: Some(sym.unit.clone()),
                    timing: None,
                },
                None => Info::poly(DType::F64),
            }
        }
        Expr::Unary { op, operand } => {
            let inner = infer(ctx, *operand, out);
            match op {
                UnaryOp::Neg => inner,
                UnaryOp::Not => Info {
                    dtype: DType::Bool,
                    unit: Some(Unit::None),
                    ..inner
                },
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let l = infer(ctx, *lhs, out);
            let r = infer(ctx, *rhs, out);
            binary(ctx, id, *op, &l, &r, out)
        }
        Expr::If { cond, then_, else_ } => {
            let c = infer(ctx, *cond, out);
            let t = infer(ctx, *then_, out);
            let e = infer(ctx, *else_, out);
            Info {
                shape: c.shape.max_shape(t.shape).max_shape(e.shape),
                dtype: unify_dtype(&t.dtype, &e.dtype),
                unit: t.unit.clone().or(e.unit.clone()),
                timing: t.timing.or(e.timing),
            }
        }
        Expr::Call { func, args } => call(ctx, id, func, args, out),
        Expr::Lookup { table, keys } => lookup(ctx, id, table, keys, out),
    }
}

// ---------------------------------------------------------------------------
// binary operators
// ---------------------------------------------------------------------------

fn binary(
    ctx: &Ctx<'_>,
    id: ExprId,
    op: BinOp,
    l: &Info,
    r: &Info,
    out: &mut Vec<Diagnostic>,
) -> Info {
    let span = ctx.span(id);
    let shape = l.shape.max_shape(r.shape);
    let numeric_dtype = unify_dtype(&l.dtype, &r.dtype);
    match op {
        BinOp::Add | BinOp::Sub => {
            let unit = match (&l.unit, &r.unit) {
                (Some(a), Some(b)) if a != b => {
                    additive_mismatch(ctx, span, a, b, out);
                    Some(a.clone())
                }
                (Some(a), _) => Some(a.clone()),
                (_, b) => b.clone(),
            };
            Info {
                shape,
                dtype: numeric_dtype,
                unit,
                timing: l.timing.or(r.timing),
            }
        }
        BinOp::Mul => {
            let unit = match (&l.unit, &r.unit) {
                (Some(Unit::Money), Some(Unit::Money)) => {
                    out.push(
                        diagnostic("E0503", "`money * money` is not a unit")
                            .span(emit::primary(
                                ctx.map,
                                span,
                                "multiplying two money amounts has no dimensional meaning",
                            ))
                            .note(
                                "money * prob = money, money / money = factor. If one side is a \
                                 rate or a factor, declare it as one.",
                            )
                            .suggestion(emit::maybe(emit::replace(
                                ctx.map,
                                span,
                                snippet(ctx.map, span),
                                "declare one operand as `factor`, `prob` or `rate(...)`",
                            ))),
                    );
                    Some(Unit::Money)
                }
                (Some(Unit::Money), _) | (_, Some(Unit::Money)) => Some(Unit::Money),
                (Some(a), Some(b)) if a == b => Some(a.clone()),
                (Some(a), None) => Some(a.clone()),
                (None, b) => b.clone(),
                (Some(a), Some(b)) => Some(dominant(a, b)),
            };
            Info {
                shape,
                dtype: numeric_dtype,
                unit,
                timing: l.timing.or(r.timing),
            }
        }
        BinOp::Div => {
            basis_division(ctx, id, l, r, out);
            let unit = match (&l.unit, &r.unit) {
                (Some(a), Some(b)) if a == b => Some(Unit::Factor),
                (Some(a), _) => Some(a.clone()),
                (None, _) => None,
            };
            Info {
                shape,
                dtype: DType::F64,
                unit,
                timing: l.timing.or(r.timing),
            }
        }
        BinOp::Pow => Info {
            shape,
            dtype: DType::F64,
            unit: l.unit.clone(),
            timing: l.timing,
        },
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Info {
            shape,
            dtype: DType::Bool,
            unit: Some(Unit::None),
            timing: None,
        },
        BinOp::And | BinOp::Or => Info {
            shape,
            dtype: DType::Bool,
            unit: Some(Unit::None),
            timing: None,
        },
    }
}

/// `money + prob` and `rate(annual) + rate(monthly)` — the two mistakes §2.4
/// exists to catch, each with its own message and its own fix.
fn additive_mismatch(ctx: &Ctx<'_>, span: Span, a: &Unit, b: &Unit, out: &mut Vec<Diagnostic>) {
    // A dimensionless operand unifies rather than clashing: `1 - prob` is `prob`.
    if is_neutral(a) || is_neutral(b) {
        return;
    }
    match (a, b) {
        (Unit::Rate(ba), Unit::Rate(bb)) => {
            let fix = match bb {
                RateBasis::Annual => "to_annual",
                _ => "to_monthly",
            };
            out.push(
                diagnostic(
                    "E0502",
                    format!(
                        "rates on different bases cannot be added: `rate({ba})` and `rate({bb})`"
                    ),
                )
                .span(emit::primary(
                    ctx.map,
                    span,
                    format!("one side is `rate({ba})`, the other `rate({bb})`"),
                ))
                .note(
                    "Basis conversion is explicit in the IR (01-ir.md §5): `to_monthly(r)` \
                     compounds, `nominal_to_periodic(r, n)` divides.",
                )
                .suggestion(emit::maybe(emit::replace(
                    ctx.map,
                    span,
                    format!("{fix}({})", snippet(ctx.map, span)),
                    format!("convert both sides to one basis with `{fix}(...)`"),
                ))),
            );
        }
        _ => out.push(
            diagnostic("E0501", format!("cannot add `{a}` and `{b}`"))
                .span(emit::primary(
                    ctx.map,
                    span,
                    format!("`{a}` on one side, `{b}` on the other"),
                ))
                .note(
                    "Units are checked, never converted (01-ir.md §2.4). Multiply instead of \
                     adding, or give the operands the same unit.",
                )
                .suggestion(emit::maybe(emit::replace(
                    ctx.map,
                    span,
                    snippet(ctx.map, span).replacen(" + ", " * ", 1),
                    format!("did you mean to multiply, giving `{a}`?"),
                ))),
        ),
    }
}

/// §5: `r / 12` on a `rate(annual)` in a periodic model is a basis conversion
/// spelled as arithmetic. `E1402` names both the compounding conversion and the
/// sanctioned escape hatch.
fn basis_division(ctx: &Ctx<'_>, id: ExprId, l: &Info, r: &Info, out: &mut Vec<Diagnostic>) {
    let Expr::Binary { rhs, .. } = ctx.doc.arena.get(id) else {
        return;
    };
    let Some(Unit::Rate(RateBasis::Annual)) = l.unit else {
        return;
    };
    let divisor = match ctx.doc.arena.get(*rhs) {
        Expr::Lit(Lit::Int(n)) => *n,
        Expr::Lit(Lit::Float(x)) if x.fract() == 0.0 => *x as i64,
        _ => return,
    };
    let per_year = ctx.world.basis_periods_per_year.unwrap_or(1);
    if per_year <= 1 || divisor != per_year || r.unit.is_some() {
        return;
    }
    let span = ctx.span(id);
    let numerator = snippet(
        ctx.map,
        ctx.span(match ctx.doc.arena.get(id) {
            Expr::Binary { lhs, .. } => *lhs,
            _ => id,
        }),
    );
    out.push(
        diagnostic(
            "E1402",
            format!("dividing a `rate(annual)` by {divisor} is not a basis conversion"),
        )
        .span(emit::primary(
            ctx.map,
            span,
            "annual to periodic compounds; it does not divide",
        ))
        .note(
            "`to_monthly(r)` is (1 + r)^(1/12) - 1. If simple division really is what you want, \
             say so with `nominal_to_periodic(r, n)`, which is named so the choice shows up in \
             the diff (01-ir.md §2.8).",
        )
        .suggestion(emit::replace(
            ctx.map,
            span,
            format!("to_monthly({numerator})"),
            "compound the annual rate down to the model's basis",
        ))
        .suggestion(emit::maybe(emit::replace(
            ctx.map,
            span,
            format!("nominal_to_periodic({numerator}, {divisor})"),
            "keep the simple division, named",
        ))),
    );
}

// ---------------------------------------------------------------------------
// calls
// ---------------------------------------------------------------------------

fn call(ctx: &Ctx<'_>, id: ExprId, func: &str, args: &[ExprId], out: &mut Vec<Diagnostic>) -> Info {
    let infos: Vec<Info> = args.iter().map(|a| infer(ctx, *a, out)).collect();
    let first = infos.first().cloned().unwrap_or(Info::poly(DType::F64));
    let span = ctx.span(id);

    if matches!(func, "is_null" | "coalesce") {
        presence(ctx, id, func, args, out);
    }

    let joined = infos
        .iter()
        .fold(Shape::Scalar, |acc, i| acc.max_shape(i.shape));

    if predictable_syntax::is_agg_fn(func) {
        // §2.3: an `Agg` is the explicit narrowing; its result is a PerMP and,
        // §2.5, it is untimed — the reduction consumed the timing.
        let dtype = match func {
            "count_while" => DType::I64,
            _ => first.dtype.clone(),
        };
        let unit = match func {
            "count_while" => Some(Unit::Count),
            _ => first.unit.clone(),
        };
        return Info {
            shape: Shape::PerMP,
            dtype,
            unit,
            timing: None,
        };
    }

    let (dtype, unit, timing) = match func {
        "min" | "max" | "abs" | "clamp" | "floor" | "ceil" | "round" => (
            first.dtype.clone(),
            infos.iter().find_map(|i| i.unit.clone()),
            first.timing,
        ),
        "sign" => (DType::F64, Some(Unit::None), None),
        "exp" | "ln" | "sqrt" | "pow" => (DType::F64, None, first.timing),
        "to_monthly" => (DType::F64, Some(Unit::Rate(RateBasis::Monthly)), None),
        "to_annual" => (DType::F64, Some(Unit::Rate(RateBasis::Annual)), None),
        "nominal_to_periodic" => (DType::F64, Some(Unit::Rate(RateBasis::Period)), None),
        "v_from_i" | "annuity_factor" | "compound" => (DType::F64, Some(Unit::Factor), None),
        "i_from_v" => (DType::F64, Some(Unit::Rate(RateBasis::Annual)), None),
        "retime" => (
            first.dtype.clone(),
            first.unit.clone(),
            timing_arg(ctx, args.get(1).copied()),
        ),
        "shift" | "cum" | "diff" => (first.dtype.clone(), first.unit.clone(), first.timing),
        "and" | "or" | "not" | "eq" | "ne" | "lt" | "le" | "gt" | "ge" | "is_null" => {
            (DType::Bool, Some(Unit::None), None)
        }
        "coalesce" => (
            first.dtype.clone(),
            first.unit.clone().or_else(|| infos.get(1)?.unit.clone()),
            None,
        ),
        "year" | "month" | "day" => (DType::I64, Some(Unit::None), None),
        "add_months" => (DType::Date, Some(Unit::None), None),
        "months_between" => (DType::I64, Some(Unit::Months), None),
        "year_frac" => (DType::F64, Some(Unit::Years), None),
        _ => (first.dtype.clone(), first.unit.clone(), first.timing),
    };
    let _ = span;
    Info {
        shape: joined,
        dtype,
        unit,
        timing,
    }
}

/// The timing tag argument of `retime(x, tag)`. The parser rewrites the bare
/// keyword to a `str` literal, so this reads it back.
fn timing_arg(ctx: &Ctx<'_>, arg: Option<ExprId>) -> Option<Timing> {
    match ctx.doc.arena.get(arg?) {
        Expr::Lit(Lit::Str(s)) => Timing::parse(s),
        _ => None,
    }
}

/// §2.11: `is_null` / `coalesce` observe a presence bit, and one exists in
/// exactly two places. Anywhere else the call is always false, and `E0602`'s
/// suggested edit is its deletion.
fn presence(ctx: &Ctx<'_>, id: ExprId, func: &str, args: &[ExprId], out: &mut Vec<Diagnostic>) {
    let Some(arg) = args.first().copied() else {
        return;
    };
    let (subject, ok) = match ctx.doc.arena.get(arg) {
        Expr::Ref(name) => (
            name.clone(),
            ctx.world
                .lookup_value(name)
                .map_or(true, |s| s.ns == Ns::ModelpointField && s.optional_field),
        ),
        Expr::Lookup { table, .. } => (
            table.clone(),
            ctx.world.table_decls.get(table).map_or(true, |(_, t)| {
                matches!(t.on_missing, predictable_syntax::OnMissing::Default(_))
            }),
        ),
        _ => return,
    };
    if ok {
        return;
    }
    let span = ctx.span(id);
    let replacement = if func == "is_null" {
        "false".to_string()
    } else {
        snippet(ctx.map, ctx.span(arg))
    };
    out.push(
        diagnostic("E0602", format!("`{subject}` can never be missing"))
            .span(emit::primary(
                ctx.map,
                span,
                format!("`{func}` here is always false"),
            ))
            .note(
                "There is no null at run time (01-ir.md §2.11). A presence bit exists only for an \
                 optional modelpoint field and for a lookup on a table with \
                 `on_missing = \"default(...)\"`.",
            )
            .suggestion(emit::replace(
                ctx.map,
                span,
                replacement,
                "drop the call; the value is always present",
            )),
    );
}

// ---------------------------------------------------------------------------
// lookups
// ---------------------------------------------------------------------------

fn lookup(
    ctx: &Ctx<'_>,
    id: ExprId,
    table: &str,
    keys: &[ExprId],
    out: &mut Vec<Diagnostic>,
) -> Info {
    let infos: Vec<Info> = keys.iter().map(|k| infer(ctx, *k, out)).collect();
    let span = ctx.span(id);
    let shape = infos
        .iter()
        .fold(Shape::Scalar, |acc, i| acc.max_shape(i.shape));

    let Some((_, decl)) = ctx.world.table_decls.get(table) else {
        return Info {
            shape,
            dtype: DType::F64,
            unit: None,
            timing: None,
        };
    };

    if decl.keys.len() != keys.len() {
        let expected: Vec<String> = decl
            .keys
            .iter()
            .map(|k| format!("{}: {}", k.name, k.dtype))
            .collect();
        out.push(
            diagnostic(
                "E0504",
                format!(
                    "`{table}` has {} keys, {} supplied",
                    decl.keys.len(),
                    keys.len()
                ),
            )
            .span(emit::primary(
                ctx.map,
                span,
                "expected one expression per key, in order",
            ))
            .span(emit::secondary(
                ctx.map,
                decl.name.span,
                format!("`{table}` is declared with keys ({})", expected.join(", ")),
            ))
            .suggestion(emit::maybe(emit::replace(
                ctx.map,
                span,
                format!(
                    "{table}@({})",
                    decl.keys
                        .iter()
                        .map(|k| k.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "supply one expression per declared key",
            ))),
        );
    } else {
        // One diagnostic per lookup: the first key that does not match. Keys are
        // positional, so a mismatch at key 1 usually means the whole tuple is in
        // the wrong order, and listing every position says the same thing twice.
        for (i, (key, info)) in decl.keys.iter().zip(&infos).enumerate() {
            if !dtype_accepts(&key.dtype, &info.dtype) {
                let already = out.iter().any(|d| {
                    d.code == "E0505"
                        && d.primary_span().map(|s| s.start) == Some(span.start as usize)
                });
                if already {
                    continue;
                }
                out.push(
                    diagnostic(
                        "E0505",
                        format!(
                            "key {} of `{table}` is `{}`, but `{}` was supplied",
                            i + 1,
                            key.dtype,
                            info.dtype
                        ),
                    )
                    .span(emit::primary(
                        ctx.map,
                        span,
                        format!("key `{}` expects `{}`", key.name, key.dtype),
                    ))
                    .span(emit::secondary(
                        ctx.map,
                        key.span,
                        format!("`{}` is declared `{}` here", key.name, key.dtype),
                    ))
                    .note(
                        "Lookup keys match the table declaration exactly; there is no implicit \
                         conversion on a key (01-ir.md §2.9).",
                    )
                    .suggestion(emit::maybe(emit::replace(
                        ctx.map,
                        ctx.span(keys[i]),
                        format!("<a {} key>", key.dtype),
                        format!("supply a `{}` expression for `{}`", key.dtype, key.name),
                    ))),
                );
            }
        }
    }

    Info {
        shape,
        dtype: decl
            .values
            .first()
            .map(|v| v.dtype.clone())
            .unwrap_or(DType::F64),
        unit: decl.values.first().map(|v| v.unit.clone()),
        timing: None,
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// The broadcast join `⊔` of §2.3, on the parser's `Shape`.
pub trait ShapeJoin {
    fn max_shape(self, other: Shape) -> Shape;
    fn rank(self) -> u8;
}

impl ShapeJoin for Shape {
    fn rank(self) -> u8 {
        match self {
            Shape::Scalar => 0,
            Shape::PerMP => 1,
            Shape::Series => 2,
        }
    }
    fn max_shape(self, other: Shape) -> Shape {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

fn unify_dtype(a: &DType, b: &DType) -> DType {
    match (a, b) {
        (DType::F64, _) | (_, DType::F64) => DType::F64,
        (DType::I64, _) | (_, DType::I64) => DType::I64,
        _ => a.clone(),
    }
}

/// A table key of dtype `want` accepts an expression of dtype `got`.
/// `i64` widens into an `f64` key; nothing else converts (§2.9).
fn dtype_accepts(want: &DType, got: &DType) -> bool {
    want == got || matches!((want, got), (DType::F64, DType::I64))
}

fn has_zero(dtype: &DType) -> bool {
    matches!(dtype, DType::F64 | DType::I64 | DType::Bool)
}

/// A unit that unifies with anything in an addition: literals and pure numbers.
fn is_neutral(u: &Unit) -> bool {
    matches!(u, Unit::None)
}

/// When two dimensioned units multiply and neither is money, the more specific
/// one survives. This is only used to keep inference total; nothing is reported.
fn dominant(a: &Unit, b: &Unit) -> Unit {
    if matches!(a, Unit::None | Unit::Factor) {
        b.clone()
    } else {
        a.clone()
    }
}

pub fn snippet(map: &SourceMap, span: Span) -> String {
    map.snippet(span).to_string()
}

/// The span of the `k` inside `x[k]`, so `E0205` carets the index and not the
/// whole reference.
fn index_span(map: &SourceMap, span: Span) -> Option<Span> {
    let text = map.snippet(span);
    let open = text.find('[')?;
    let close = text.find(']')?;
    Some(Span::new(
        span.file,
        span.start as usize + open + 1,
        span.start as usize + close,
    ))
}
