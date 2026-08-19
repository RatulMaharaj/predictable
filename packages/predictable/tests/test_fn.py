"""The builtin library: every function lowers to a node the IR's closed set recognises."""

from __future__ import annotations

import pytest

from predictable import fn
from predictable.ir import AGGREGATES, BUILTINS, At, Call, Lit, Ref, Unary, walk
from predictable.proxy import ExprProxy, TraceContext


def value(name: str) -> ExprProxy:
    return ExprProxy(Ref(name), name=name, ctx=TraceContext())


x, y, z = value("x"), value("y"), value("z")


@pytest.mark.parametrize(
    "expression,text",
    [
        (fn.when(x, y, z), "if x then y else z"),
        (fn.min_(x, y), "min(x, y)"),
        (fn.max_(x, 0.0), "max(x, 0.0)"),
        (fn.clamp(x, 0.0, 1.0), "clamp(x, 0.0, 1.0)"),
        (fn.round_(x, 2), "round(x, 2)"),
        (fn.abs_(x), "abs(x)"),
        (fn.floor(x), "floor(x)"),
        (fn.ceil(x), "ceil(x)"),
        (fn.sign(x), "sign(x)"),
        (fn.exp(x), "exp(x)"),
        (fn.ln(x), "ln(x)"),
        (fn.pow_(x, 2), "pow(x, 2)"),
        (fn.sqrt(x), "sqrt(x)"),
        (fn.to_monthly(x), "to_monthly(x)"),
        (fn.to_annual(x), "to_annual(x)"),
        (fn.nominal_to_periodic(x, 12), "nominal_to_periodic(x, 12)"),
        (fn.compound(x, y), "compound(x, y)"),
        (fn.annuity_factor(x, y), "annuity_factor(x, y)"),
        (fn.v_from_i(x), "v_from_i(x)"),
        (fn.i_from_v(x), "i_from_v(x)"),
        (fn.shift(x, 1), "shift(x, 1)"),
        (fn.cum(x), "cum(x)"),
        (fn.diff(x), "diff(x)"),
        (fn.coalesce(x, 0.0), "coalesce(x, 0.0)"),
        (fn.is_null(x), "is_null(x)"),
        (fn.npv(x, y), "npv(x, y)"),
        (fn.sum_(x), "sum(x)"),
        (fn.sum_kahan(x), "sum_kahan(x)"),
        (fn.first(x), "first(x)"),
        (fn.last(x), "last(x)"),
        (fn.max_over(x), "max_over(x)"),
        (fn.min_over(x), "min_over(x)"),
        (fn.count_while(x), "count_while(x)"),
        (fn.year(x), "year(x)"),
        (fn.month(x), "month(x)"),
        (fn.day(x), "day(x)"),
        (fn.add_months(x, 3), "add_months(x, 3)"),
        (fn.months_between(x, y), "months_between(x, y)"),
        (fn.year_frac(x, y, "act/365"), 'year_frac(x, y, "act/365")'),
    ],
)
def test_each_builtin_lowers_to_its_ir_spelling(expression, text):
    assert expression.to_expr().to_pir() == text


@pytest.mark.parametrize(
    "call,ir_name",
    [
        (lambda: fn.min_(x, y), "min"),
        (lambda: fn.max_(x, y), "max"),
        (lambda: fn.round_(x, 2), "round"),
        (lambda: fn.pow_(x, 2), "pow"),
        (lambda: fn.sum_(x), "sum"),
        (lambda: fn.abs_(x), "abs"),
    ],
)
def test_the_underscore_is_dropped_in_the_ir(call, ir_name):
    node = call().to_expr()
    assert isinstance(node, Call)
    assert node.func == ir_name


def test_logic_connectives_lower_to_infix_not_a_variadic_node():
    assert fn.all_(x, y, z).to_expr().to_pir() == "x and y and z"
    assert fn.any_(x, y).to_expr().to_pir() == "x or y"
    assert fn.not_(x).to_expr() == Unary("not", Ref("x"))


def test_at_and_absolute_indexing_are_the_same_operation():
    assert fn.at(x, 3).to_expr() == At("x", 3)


def test_retime_carries_the_timing_tag_as_a_str_literal():
    from predictable.timing import END

    node = fn.retime(x, END).to_expr()
    assert node == Call("retime", (Ref("x"), Lit("str", "end")))
    assert node.to_pir() == "retime(x, end)"


def test_only_aggregates_mark_a_value_stage_2():
    assert fn.npv(x, y)._stage2 is True
    assert fn.sum_(x)._stage2 is True
    assert fn.exp(x)._stage2 is False
    # stage-2-ness is contagious: a value derived from an aggregate is still stage 2
    assert (fn.npv(x, y) * 2)._stage2 is True


def test_every_call_this_library_can_emit_is_in_the_closed_builtin_set():
    emitted = [
        fn.when(x, y, z), fn.min_(x, y), fn.clamp(x, 0.0, 1.0), fn.round_(x, 2), fn.exp(x),
        fn.to_monthly(x), fn.nominal_to_periodic(x, 12), fn.shift(x, 1), fn.retime(x, "mid"),
        fn.coalesce(x, y), fn.npv(x, y), fn.count_while(x), fn.year_frac(x, y, "act/365"),
        fn.all_(x, y), fn.not_(x), fn.at(x, 1),
    ]
    for expression in emitted:
        for node in walk(expression.to_expr()):
            if isinstance(node, Call):
                assert node.func in BUILTINS, node.func


def test_the_aggregate_set_is_a_subset_of_the_builtin_set():
    assert AGGREGATES <= BUILTINS
