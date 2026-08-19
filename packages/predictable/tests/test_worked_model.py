"""The whole worked model of `02-dsl.md` §11, traced, against the IR of `01-ir.md` §6.

This is the specification test for the tracer: the Python is copied from the DSL spec, the
expected strings are copied from the IR spec, and nothing in between is allowed to differ. If the
printer, the operator overloads or the lag handling drift, one of these strings changes.
"""

from __future__ import annotations

import pytest

from predictable import Param, t, trace, trace_init
from predictable.fn import compound, npv, retime, when
from predictable.proxy import TableProxy
from predictable.timing import MID

sa8990 = TableProxy("sa8990", ("qx",))
lapse_rates = TableProxy("lapse_rates", ("lapse_pa",))


# -- decrements.py -----------------------------------------------------------------------


def age(entry_age):
    """Attained age at the start of projection year t."""
    return entry_age + t


def in_term(policy_term):
    return t < policy_term


def qx(age, gender, smoker, mortality_loading, sa8990=sa8990):
    return sa8990(age, gender, smoker) * mortality_loading


def wx(policy_year, lapse_rates=lapse_rates):
    return lapse_rates(policy_year)


def num_pols_if(num_pols_if, qx, wx, in_term):
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * when(in_term, 1.0, 0.0)


def deaths(num_pols_if, qx):
    return num_pols_if * qx


# -- cashflows.py ------------------------------------------------------------------------


def premium_rate(premium_rate, premium_escalation):
    return premium_rate[t - 1] * (1 + premium_escalation)


def premium_income(premium_rate, num_pols_if, in_term):
    return premium_rate * num_pols_if * when(in_term, 1.0, 0.0)


def death_claims(sum_assured, deaths):
    return sum_assured * deaths


def renewal_expenses(renewal_expense_pa, expense_inflation, num_pols_if, in_term):
    return (
        renewal_expense_pa * compound(expense_inflation, t) * num_pols_if * when(in_term, 1.0, 0.0)
    )


def net_cashflow(premium_income, death_claims, renewal_expenses):
    return (
        retime(premium_income, MID) - retime(death_claims, MID) - retime(renewal_expenses, MID)
    )


# -- reserves.py -------------------------------------------------------------------------


def disc_factor(disc_factor, valuation_rate):
    return disc_factor[t - 1] / (1 + valuation_rate)


def pv_premiums(premium_income, disc_factor):
    return npv(premium_income, disc_factor)


def pv_claims(death_claims, disc_factor):
    return npv(death_claims, disc_factor)


def pv_expenses(renewal_expenses, disc_factor):
    return npv(renewal_expenses, disc_factor)


def bel(pv_claims, pv_expenses, pv_premiums):
    return pv_claims + pv_expenses - pv_premiums


def reserve(reserve, premium_income, death_claims, renewal_expenses, valuation_rate):
    return (
        reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1] - renewal_expenses[t - 1]
    ) * (1 + valuation_rate)


#: name -> (body, extra Param overrides). Expected text is `01-ir.md` §6, verbatim.
EXPECTED = [
    (age, "entry_age + t"),
    (in_term, "t < policy_term"),
    (qx, "sa8990@(age, gender, smoker) * mortality_loading"),
    (wx, "lapse_rates@(policy_year)"),
    (
        num_pols_if,
        "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)",
    ),
    (deaths, "num_pols_if * qx"),
    (premium_rate, "premium_rate[t-1] * (1 + premium_escalation)"),
    (premium_income, "premium_rate * num_pols_if * (if in_term then 1.0 else 0.0)"),
    (death_claims, "sum_assured * deaths"),
    (
        renewal_expenses,
        "renewal_expense_pa * compound(expense_inflation, t) * num_pols_if "
        "* (if in_term then 1.0 else 0.0)",
    ),
    (
        net_cashflow,
        "retime(premium_income, mid) - retime(death_claims, mid) - retime(renewal_expenses, mid)",
    ),
    (disc_factor, "disc_factor[t-1] / (1 + valuation_rate)"),
    (pv_premiums, "npv(premium_income, disc_factor)"),
    (pv_claims, "npv(death_claims, disc_factor)"),
    (pv_expenses, "npv(renewal_expenses, disc_factor)"),
    (bel, "pv_claims + pv_expenses - pv_premiums"),
    (
        reserve,
        "(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) "
        "* (1 + valuation_rate)",
    ),
]


def _params(func):
    import inspect

    return [
        Param(name)
        for name, p in inspect.signature(func).parameters.items()
        if not isinstance(p.default, TableProxy)
    ]


@pytest.mark.parametrize("func,expected", EXPECTED, ids=lambda v: getattr(v, "__name__", ""))
def test_the_worked_model_traces_to_the_ir_spec_text(func, expected):
    assert trace(func, _params(func)).pir == expected


def test_the_stage_2_components_are_exactly_the_npv_derived_ones():
    stage2 = {func.__name__ for func, _ in EXPECTED if trace(func, _params(func)).stage2}
    assert stage2 == {"pv_premiums", "pv_claims", "pv_expenses"}


def test_the_premium_rate_init_lambda_traces_to_the_ir_init_text():
    assert trace_init(lambda annual_premium: annual_premium, [Param("annual_premium")]).pir == (
        "annual_premium"
    )


def test_reserve_seeds_its_init_from_a_stage_2_component():
    # `init = "bel"` in `01-ir.md` §6: legal in `init`, `E1207` in `expr`.
    result = trace_init(lambda bel: bel, [Param("bel", shape="PerMP", stage2=True)])
    assert result.pir == "bel"


def test_the_docstring_of_a_body_does_not_reach_the_expression():
    assert "Attained age" not in trace(age, _params(age)).pir


def test_every_component_reports_the_dependencies_its_signature_declares():
    for func, _ in EXPECTED:
        declared = {p.name for p in _params(func)}
        used = set(trace(func, _params(func)).dependencies) - {"t"}
        assert used <= declared, func.__name__
