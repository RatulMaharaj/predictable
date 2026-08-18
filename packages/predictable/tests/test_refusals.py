"""Every refusal in the `E12xx` catalogue, and the quality of the message it produces.

These tests assert on the *content* of the diagnostic, not only its code: the whole thesis of
`02-dsl.md` §10.1 is that a message names its replacement, so a message that stops naming one is
a regression worth failing on.
"""

from __future__ import annotations

import pytest

from predictable import DslError, Param, t, trace
from predictable.fn import npv, when
from predictable.proxy import TableProxy


def refuse(func, params=(), **kwargs) -> DslError:
    with pytest.raises(DslError) as excinfo:
        trace(func, [Param(p) if isinstance(p, str) else p for p in params], **kwargs)
    return excinfo.value


# -- E1201 -------------------------------------------------------------------------------


def test_e1201_python_if_on_a_traced_value():
    def premium_income(premium_rate, in_term):
        if in_term:
            return premium_rate
        return 0.0

    err = refuse(premium_income, ["premium_rate", "in_term"])
    diag = err.diagnostic
    assert diag.code == "E1201"
    assert "`if` cannot be used on a model value" == diag.message
    assert "`in_term` is a Series component, not a Python bool" == diag.spans[0].label
    assert "when(in_term, <then>, <otherwise>)" in err.args[0]
    assert "traced once at build time" in err.args[0]
    # the span points at the user's own line, not at anything inside the package
    assert diag.spans[0].file.endswith("test_refusals.py")
    assert diag.spans[0].start_pos.line > 0


def test_e1201_names_the_construct_that_truth_tested_the_value():
    def both(a, b):
        return a and b

    assert "`and` cannot be used on a model value" in refuse(both, ["a", "b"]).args[0]

    def either(a, b):
        return a or b

    assert "any_(a, b)" in refuse(either, ["a", "b"]).args[0]

    def negated(a):
        return not a

    assert "not_(x)" in refuse(negated, ["a"]).args[0]


def test_e1201_a_body_that_returns_nothing():
    def broken(a):
        a * 2

    err = refuse(broken, ["a"])
    assert err.code == "E1201"
    assert "did not return a model value" in err.args[0]
    assert "when(<cond>, <then>, <otherwise>)" in err.args[0]


# -- E1202 -------------------------------------------------------------------------------


def test_e1202_unlagged_self_reference():
    def num_pols_if(num_pols_if, qx):
        return num_pols_if * (1 - qx)

    err = refuse(num_pols_if, ["num_pols_if", "qx"])
    assert err.code == "E1202"
    assert err.diagnostic.message == "`num_pols_if` reads itself with no time lag"
    assert "num_pols_if[t-1]" in err.args[0]
    assert "init=1.0" in err.args[0]
    # the fix is mechanically applicable: a byte-range edit, not prose
    edit = err.diagnostic.suggestions[0].edits[0]
    assert edit.replacement == "num_pols_if[t-1]"
    assert edit.end > edit.start


def test_e1202_does_not_fire_on_a_different_component_of_the_same_name_elsewhere():
    def deaths(num_pols_if, qx):
        return num_pols_if * qx

    assert trace(deaths, [Param("num_pols_if"), Param("qx")]).pir == "num_pols_if * qx"


# -- E1203 -------------------------------------------------------------------------------


def test_e1203_lag_by_a_traced_value():
    def lagged(x, n):
        return x[t - n]

    err = refuse(lagged, ["x", "n"])
    assert err.code == "E1203"
    assert "not a build-time constant" in err.diagnostic.message
    assert "when(cond, x[t-1], x[t-2])" in err.args[0]


def test_e1203_negative_absolute_index():
    def bad(x):
        return x[-1]

    assert refuse(bad, ["x"]).code == "E1203"


def test_e1203_non_integer_index():
    def bad(x):
        return x[1.5]

    assert refuse(bad, ["x"]).code == "E1203"


# -- E1204 -------------------------------------------------------------------------------


def test_e1204_forward_reference():
    def peek(x):
        return x[t + 1]

    err = refuse(peek, ["x"])
    assert err.code == "E1204"
    assert err.diagnostic.message == "`x[t+1]` reads the future"
    assert "npv(" in err.args[0] and "init=" in err.args[0]
    assert "single forward pass" in err.args[0]


# -- E1205 -------------------------------------------------------------------------------


def test_e1205_iterating_a_traced_value():
    def total(x):
        return sum(list(x))

    err = refuse(total, ["x"])
    assert err.code == "E1205"
    assert "sum_(x)" in err.args[0]
    assert "npv(x, disc_factor)" in err.args[0]


def test_e1205_comprehension_over_a_traced_value():
    def total(x):
        return [v for v in x]

    assert refuse(total, ["x"]).code == "E1205"


def test_e1205_len_and_membership_and_hashing():
    def with_len(x):
        return len(x)

    def with_in(x):
        return 1 in x

    def with_hash(x):
        return {x: 1}

    for func in (with_len, with_in, with_hash):
        assert refuse(func, ["x"]).code == "E1205"


def test_e1205_indexing_a_computed_expression():
    def bad(a, b):
        return (a * b)[t - 1]

    assert refuse(bad, ["a", "b"]).code == "E1205"


def test_e1205_returning_a_container():
    def bad(a):
        return [a, a]

    assert refuse(bad, ["a"]).code == "E1205"


# -- E1206 -------------------------------------------------------------------------------


def test_e1206_calling_a_user_function_on_a_traced_value():
    import math

    def bad(x):
        return math.exp(x)

    err = refuse(bad, ["x"])
    assert err.code == "E1206"
    assert "no user-defined functions" in err.args[0]


def test_e1206_attribute_access_on_a_traced_value():
    def bad(x):
        return x.value

    err = refuse(bad, ["x"])
    assert err.code == "E1206"
    assert "x.value" in err.diagnostic.message


def test_e1206_calling_a_traced_value():
    def bad(x):
        return x(1)

    assert refuse(bad, ["x"]).code == "E1206"


def test_e1206_using_a_table_as_a_value():
    tbl = TableProxy("sa8990", ("qx",))

    def bad(age, sa8990=tbl):
        return sa8990 * age

    err = refuse(bad, ["age"])
    assert err.code == "E1206"
    assert err.diagnostic.suggestions[0].edits[0].replacement == "sa8990(<keys>)"


def test_e1206_unknown_value_column_on_a_table():
    tbl = TableProxy("rates", ("qx", "wx"))

    def bad(age, rates=tbl):
        return rates(age).mortality

    err = refuse(bad, ["age"])
    assert err.code == "E1206"
    assert "rates(...).qx" in err.args[0]


# -- E1207 -------------------------------------------------------------------------------


def test_e1207_stage_2_value_read_in_expr():
    def reserve(bel, premium_income):
        return bel + premium_income

    err = refuse(reserve, [Param("bel", shape="PerMP", stage2=True), Param("premium_income")])
    assert err.code == "E1207"
    assert "computed after the projection completes" == err.diagnostic.spans[0].label
    assert "init=bel" in err.args[0]


def test_e1207_does_not_fire_in_the_init_root():
    from predictable import trace_init

    result = trace_init(lambda bel: bel * 1.0, [Param("bel", shape="PerMP", stage2=True)])
    assert result.pir == "bel * 1.0"


def test_a_locally_computed_aggregate_is_still_stage_2_but_not_refused():
    def pv(premium_income, disc_factor):
        return npv(premium_income, disc_factor) * 1.0

    assert trace(pv, [Param("premium_income"), Param("disc_factor")]).stage2 is True


# -- E1402 -------------------------------------------------------------------------------


def test_e1402_dividing_an_annual_rate_to_change_basis():
    def monthly(valuation_rate):
        return valuation_rate / 12

    err = refuse(monthly, [Param("valuation_rate", unit="rate(annual)")])
    assert err.code == "E1402"
    assert "to_monthly(valuation_rate)" in err.args[0]
    assert "nominal_to_periodic(valuation_rate, 12)" in err.args[0]
    assert err.diagnostic.suggestions[0].edits[0].replacement == "to_monthly(valuation_rate)"


def test_e1402_does_not_fire_on_a_unitless_value_or_on_the_sanctioned_builtin():
    from predictable.fn import nominal_to_periodic

    def plain(x):
        return x / 12

    assert trace(plain, [Param("x")]).pir == "x / 12"

    def explicit(valuation_rate):
        return nominal_to_periodic(valuation_rate, 12)

    assert (
        trace(explicit, [Param("valuation_rate", unit="rate(annual)")]).pir
        == "nominal_to_periodic(valuation_rate, 12)"
    )


# -- E1302 -------------------------------------------------------------------------------


def test_e1302_modelpoint_object_accessed_as_a_value():
    from predictable.errors import raise_e1302_modelpoint_object

    with pytest.raises(DslError) as excinfo:
        raise_e1302_modelpoint_object("mp", "sum_assured")
    err = excinfo.value
    assert err.code == "E1302"
    assert "add `sum_assured` to the parameter list" in err.args[0]


# -- shape of every diagnostic ------------------------------------------------------------


def test_every_diagnostic_is_json_serialisable_with_the_engine_field_names():
    def bad(x, in_term):
        return when(in_term, x, x)[t + 1]

    diag = refuse(bad, ["x", "in_term"]).diagnostic
    doc = diag.to_json()
    assert set(doc) >= {"code", "severity", "message", "spans", "suggestions", "doc_url"}
    assert doc["severity"] == "error"
    assert doc["doc_url"].endswith("#E1205")
    span = doc["spans"][0]
    assert set(span) >= {"file", "start", "end", "primary"}
    assert span["start_pos"]["line"] > 0
    for suggestion in doc["suggestions"]:
        assert set(suggestion) == {"message", "edits", "applicability"}


def test_the_rendered_message_shows_the_offending_source_line():
    def premium_income(premium_rate, in_term):
        if in_term:
            return premium_rate
        return 0.0

    rendered = refuse(premium_income, ["premium_rate", "in_term"]).args[0]
    assert "error[E1201]" in rendered
    assert "if in_term:" in rendered
    assert "^" in rendered
    assert "https://predictable.dev/llm/diagnostics/#E1201" in rendered
