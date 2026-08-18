"""The builtin function set, as Python callables that build IR nodes (`01-ir.md` §2.8).

The set is **closed**: everything here maps one-to-one onto a `Call` (or `If`, or an infix
`Binary`) node, and nothing else may be called on a traced value. Trailing-underscore names
(``min_``, ``sum_``, ``round_``, ``pow_``) avoid shadowing Python builtins; the underscore is
dropped in the IR.
"""

from __future__ import annotations

from typing import Any

from . import errors
from .ir import AGGREGATES, At, Binary, Call, Expr, If, Lit, Ref, Unary
from .proxy import ExprProxy, TraceContext, _operand, _stage2_of

__all__ = [
    "when",
    "min_", "max_", "clamp", "round_", "abs_", "floor", "ceil", "sign",
    "exp", "ln", "pow_", "sqrt",
    "to_monthly", "to_annual", "nominal_to_periodic", "compound", "annuity_factor",
    "v_from_i", "i_from_v",
    "shift", "retime", "cum", "diff",
    "coalesce", "is_null", "all_", "any_", "not_",
    "npv", "sum_", "sum_kahan", "first", "last", "at", "max_over", "min_over", "count_while",
    "year", "month", "day", "add_months", "months_between", "year_frac",
]


def _ctx_of(*args: Any) -> TraceContext:
    for a in args:
        if isinstance(a, ExprProxy):
            return a._ctx
    return TraceContext()


def _call(func: str, *args: Any, unit: str | None = None) -> ExprProxy:
    ctx = _ctx_of(*args)
    nodes = tuple(_operand(a, ctx) for a in args)
    return ExprProxy(
        Call(func, nodes),
        shape="PerMP" if func in AGGREGATES else "Series",
        unit=unit,
        stage2=func in AGGREGATES or any(_stage2_of(a) for a in args),
        ctx=ctx,
    )


# --------------------------------------------------------------------------------------
# the value conditional
# --------------------------------------------------------------------------------------


def when(cond: Any, then: Any, otherwise: Any) -> ExprProxy:
    """``if cond then ... else ...`` as a *value*.

    Both arms are evaluated (`01-ir.md` §2.6); this is not a guard. A trap raised inside an arm
    that is not taken is suppressed, which is what makes ``when(x == 0, 0.0, 1.0 / x)`` legal.
    """
    ctx = _ctx_of(cond, then, otherwise)
    node = If(_operand(cond, ctx), _operand(then, ctx), _operand(otherwise, ctx))
    unit = then.unit if isinstance(then, ExprProxy) else None
    return ExprProxy(
        node,
        unit=unit,
        stage2=any(_stage2_of(a) for a in (cond, then, otherwise)),
        ctx=ctx,
    )


# --------------------------------------------------------------------------------------
# arithmetic
# --------------------------------------------------------------------------------------


def min_(*args: Any) -> ExprProxy:
    return _call("min", *args)


def max_(*args: Any) -> ExprProxy:
    return _call("max", *args)


def clamp(x: Any, lo: Any, hi: Any) -> ExprProxy:
    return _call("clamp", x, lo, hi)


def round_(x: Any, dp: int = 0) -> ExprProxy:
    """Round half away from zero on the shortest decimal representation (`01-ir.md` §2.8)."""
    return _call("round", x, dp)


def abs_(x: Any) -> ExprProxy:
    return _call("abs", x)


def floor(x: Any) -> ExprProxy:
    return _call("floor", x)


def ceil(x: Any) -> ExprProxy:
    return _call("ceil", x)


def sign(x: Any) -> ExprProxy:
    return _call("sign", x)


def exp(x: Any) -> ExprProxy:
    return _call("exp", x)


def ln(x: Any) -> ExprProxy:
    return _call("ln", x)


def pow_(x: Any, y: Any) -> ExprProxy:
    return _call("pow", x, y)


def sqrt(x: Any) -> ExprProxy:
    return _call("sqrt", x)


# --------------------------------------------------------------------------------------
# rates — the only legal basis conversions (`02-dsl.md` §5)
# --------------------------------------------------------------------------------------


def to_monthly(r: Any) -> ExprProxy:
    return _call("to_monthly", r, unit="rate(monthly)")


def to_annual(r: Any) -> ExprProxy:
    return _call("to_annual", r, unit="rate(annual)")


def nominal_to_periodic(r: Any, n: Any) -> ExprProxy:
    """``r / n`` — simple division, named so the choice is visible in the diff."""
    return _call("nominal_to_periodic", r, n, unit="rate(period)")


def compound(r: Any, n: Any) -> ExprProxy:
    return _call("compound", r, n)


def annuity_factor(i: Any, n: Any) -> ExprProxy:
    return _call("annuity_factor", i, n)


def v_from_i(i: Any) -> ExprProxy:
    return _call("v_from_i", i)


def i_from_v(v: Any) -> ExprProxy:
    return _call("i_from_v", v)


# --------------------------------------------------------------------------------------
# timing
# --------------------------------------------------------------------------------------


def shift(x: Any, k: Any) -> ExprProxy:
    return _call("shift", x, k)


def retime(x: Any, timing: Any) -> ExprProxy:
    """Restate ``x`` on another timing tag. The tag is a keyword, carried as a str literal."""
    tag = getattr(timing, "_predictable_tag", timing)
    ctx = _ctx_of(x)
    return ExprProxy(
        Call("retime", (_operand(x, ctx), Lit("str", str(tag)))),
        unit=x.unit if isinstance(x, ExprProxy) else None,
        stage2=_stage2_of(x),
        ctx=ctx,
    )


def cum(x: Any) -> ExprProxy:
    return _call("cum", x)


def diff(x: Any) -> ExprProxy:
    return _call("diff", x)


# --------------------------------------------------------------------------------------
# logic — `and`/`or`/`not` are infix in the IR; there is no variadic logic node
# --------------------------------------------------------------------------------------


def _fold(op: str, args: tuple[Any, ...]) -> ExprProxy:
    if len(args) < 2:
        raise TypeError(f"{op} needs at least two operands")
    ctx = _ctx_of(*args)
    node: Expr = _operand(args[0], ctx)
    for arg in args[1:]:
        node = Binary(op, node, _operand(arg, ctx))
    return ExprProxy(node, stage2=any(_stage2_of(a) for a in args), ctx=ctx)


def all_(*args: Any) -> ExprProxy:
    """``all_(a, b, c)`` lowers to infix ``a and b and c``."""
    return _fold("and", args)


def any_(*args: Any) -> ExprProxy:
    """``any_(a, b, c)`` lowers to infix ``a or b or c``."""
    return _fold("or", args)


def not_(x: Any) -> ExprProxy:
    ctx = _ctx_of(x)
    return ExprProxy(Unary("not", _operand(x, ctx)), stage2=_stage2_of(x), ctx=ctx)


def coalesce(x: Any, y: Any) -> ExprProxy:
    return _call("coalesce", x, y)


def is_null(x: Any) -> ExprProxy:
    return _call("is_null", x)


# --------------------------------------------------------------------------------------
# aggregates — every one of these makes the component stage 2 (`01-ir.md` §8.2)
# --------------------------------------------------------------------------------------


def npv(x: Any, disc: Any) -> ExprProxy:
    """``npv(x, disc)`` takes a discount-factor *Series*, not a rate, and uses ``x``'s timing."""
    return _call("npv", x, disc)


def sum_(x: Any, pred: Any = None) -> ExprProxy:
    return _call("sum", x) if pred is None else _call("sum", x, pred)


def sum_kahan(x: Any, pred: Any = None) -> ExprProxy:
    return _call("sum_kahan", x) if pred is None else _call("sum_kahan", x, pred)


def first(x: Any) -> ExprProxy:
    return _call("first", x)


def last(x: Any) -> ExprProxy:
    return _call("last", x)


def at(x: Any, k: int) -> ExprProxy:
    """``at(x, k)`` and ``x[k]`` are the same operation; both lower to ``At`` (`01-ir.md` §2.8)."""
    if not isinstance(k, int) or isinstance(k, bool) or k < 0:
        errors.raise_e1203_non_constant_lag(
            x.name if isinstance(x, ExprProxy) and x.name else "x", repr(k)
        )
    if isinstance(x, ExprProxy) and isinstance(x._expr, Ref):
        return ExprProxy(At(x._expr.name, int(k)), ctx=x._ctx)
    return _call("at", x, k)


def max_over(x: Any) -> ExprProxy:
    return _call("max_over", x)


def min_over(x: Any) -> ExprProxy:
    return _call("min_over", x)


def count_while(cond: Any) -> ExprProxy:
    """Stops at the first ``t`` where ``cond`` is false and returns that count."""
    return _call("count_while", cond)


# --------------------------------------------------------------------------------------
# dates
# --------------------------------------------------------------------------------------


def year(d: Any) -> ExprProxy:
    return _call("year", d)


def month(d: Any) -> ExprProxy:
    return _call("month", d)


def day(d: Any) -> ExprProxy:
    return _call("day", d)


def add_months(d: Any, n: Any) -> ExprProxy:
    return _call("add_months", d, n)


def months_between(a: Any, b: Any) -> ExprProxy:
    return _call("months_between", a, b)


def year_frac(a: Any, b: Any, convention: str) -> ExprProxy:
    return _call("year_frac", a, b, convention)
