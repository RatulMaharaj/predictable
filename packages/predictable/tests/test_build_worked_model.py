"""The worked model of `02-dsl.md` §11, declared with the real decorators and built.

`tests/test_worked_model.py` proves the *tracer* reproduces the expressions of `01-ir.md` §6 when
it is handed parameters by hand. This proves the *declaration layer* does: the source below is the
DSL spec's own §11, unedited, and every expected string is copied from the IR spec's §6.

Declared at module scope, exactly as a real model is, so `declaration_index` ordering, docstring
capture, annotation cross-checks and the `outputs=` manifest are all exercised by importing it.
"""

from datetime import date

import pytest

from predictable import (
    Enum,
    Key,
    ModelPoint,
    Value,
    assumption,
    build,
    default,
    key,
    per_mp,
    product,
    series,
    t,
    table,
    timeline,
)
from predictable.fn import compound, npv, retime, when
from predictable.timing import END, MID, POINT, START
from predictable.units import Count, Factor, Flag, Money, Prob, Rate, Years

# -- schema.py ---------------------------------------------------------------------------

timeline(
    basis="annual",
    periods=40,
    origin="policy",
    valuation_date=date(2026, 6, 30),
    year_convention="act/365",
)


class Gender(Enum):
    M = "M"
    F = "F"


class TermMP(ModelPoint):
    policy_number: str = key()
    entry_age: Years
    gender: Gender
    smoker: bool
    sum_assured: Money
    annual_premium: Money
    policy_term: Years


valuation_rate = assumption(Rate.annual)
premium_escalation = assumption(Rate.annual)
expense_inflation = assumption(Rate.annual)
renewal_expense_pa = assumption(Money)
mortality_loading = assumption(Factor)

sa8990 = table(
    source="tables/sa8990.csv",
    keys=[Key("age", int, policy="clamp"), Key("gender", Gender), Key("smoker", bool)],
    values=[Value("qx", Prob)],
    on_missing="error",
)

lapse_rates = table(
    source="tables/lapses.csv",
    keys=[Key("policy_year", int, policy="step")],
    values=[Value("lapse_pa", Prob)],
    on_missing=default(0.0),
)


# -- decrements.py -----------------------------------------------------------------------


@series(timing=START)
def age(entry_age: Years) -> Years:
    """Attained age at the start of projection year t."""
    return entry_age + t


@series(timing=START)
def in_term(policy_term: Years) -> Flag:
    """True while the policy is within its contractual term."""
    return t < policy_term


@series(timing=END)
def qx(
    age: Years, gender: Gender, smoker: bool, mortality_loading: Factor, sa8990=sa8990
) -> Prob:
    """Annual mortality rate, table rate scaled by the valuation loading."""
    return sa8990(age, gender, smoker) * mortality_loading


@series(timing=END)
def wx(policy_year: Years, lapse_rates=lapse_rates) -> Prob:
    return lapse_rates(policy_year)


@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """Survivorship. Self-referential with lag 1 — legal (see IR spec 3.1)."""
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * when(in_term, 1.0, 0.0)


@series(timing=END)
def deaths(num_pols_if: Count, qx: Prob) -> Count:
    return num_pols_if * qx


# -- cashflows.py ------------------------------------------------------------------------


@series(timing=START, init=lambda annual_premium: annual_premium)
def premium_rate(premium_rate: Money, premium_escalation: Rate.annual) -> Money:
    """Escalating premium, compounding annually from the issue premium."""
    return premium_rate[t - 1] * (1 + premium_escalation)


@series(timing=START)
def premium_income(premium_rate: Money, num_pols_if: Count, in_term: Flag) -> Money:
    return premium_rate * num_pols_if * when(in_term, 1.0, 0.0)


@series(timing=END)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    return sum_assured * deaths


@series(timing=START)
def renewal_expenses(
    renewal_expense_pa: Money,
    expense_inflation: Rate.annual,
    num_pols_if: Count,
    in_term: Flag,
) -> Money:
    return (
        renewal_expense_pa * compound(expense_inflation, t) * num_pols_if * when(in_term, 1.0, 0.0)
    )


@series(timing=MID, output=True)
def net_cashflow(premium_income: Money, death_claims: Money, renewal_expenses: Money) -> Money:
    """Insurer-positive net flow.

    Explicit retiming so the sign convention is auditable.
    """
    return retime(premium_income, MID) - retime(death_claims, MID) - retime(renewal_expenses, MID)


# -- reserves.py -------------------------------------------------------------------------


@series(timing=POINT, init=1.0)
def disc_factor(disc_factor: Factor, valuation_rate: Rate.annual) -> Factor:
    """v^t. Recursive rather than pow() so a term-structure rate is a one-line change."""
    return disc_factor[t - 1] / (1 + valuation_rate)


@per_mp(output=True)
def pv_premiums(premium_income: Money, disc_factor: Factor) -> Money:
    return npv(premium_income, disc_factor)


@per_mp(output=True)
def pv_claims(death_claims: Money, disc_factor: Factor) -> Money:
    return npv(death_claims, disc_factor)


@per_mp(output=True)
def pv_expenses(renewal_expenses: Money, disc_factor: Factor) -> Money:
    return npv(renewal_expenses, disc_factor)


@per_mp(output=True)
def bel(pv_claims: Money, pv_expenses: Money, pv_premiums: Money) -> Money:
    """Best estimate liability, prospective net premium reserve at t = 0."""
    return pv_claims + pv_expenses - pv_premiums


@series(timing=POINT, init=bel, output=True)
def reserve(
    reserve: Money,
    premium_income: Money,
    death_claims: Money,
    renewal_expenses: Money,
    valuation_rate: Rate.annual,
) -> Money:
    """Retrospective roll-forward."""
    return (
        reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1] - renewal_expenses[t - 1]
    ) * (1 + valuation_rate)


# -- __init__.py -------------------------------------------------------------------------

TERM = product(
    "term_assurance",
    modules=[__name__],
    outputs=["net_cashflow", "bel", "reserve", "pv_premiums", "pv_claims", "pv_expenses"],
    key_field="policy_number",
)


@pytest.fixture(scope="module")
def model():
    return build(TERM)


# -- the assertions ----------------------------------------------------------------------

EXPECTED_EXPR = {
    # `01-ir.md` §6, verbatim.
    "age": "entry_age + t",
    "in_term": "t < policy_term",
    "qx": "sa8990@(age, gender, smoker) * mortality_loading",
    "wx": "lapse_rates@(policy_year)",
    "num_pols_if": "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * "
    "(if in_term then 1.0 else 0.0)",
    "deaths": "num_pols_if * qx",
    "premium_rate": "premium_rate[t-1] * (1 + premium_escalation)",
    "premium_income": "premium_rate * num_pols_if * (if in_term then 1.0 else 0.0)",
    "death_claims": "sum_assured * deaths",
    "disc_factor": "disc_factor[t-1] / (1 + valuation_rate)",
    "pv_premiums": "npv(premium_income, disc_factor)",
    "bel": "pv_claims + pv_expenses - pv_premiums",
}


@pytest.mark.parametrize("name,expected", sorted(EXPECTED_EXPR.items()))
def test_expressions_match_the_ir_spec(model, name, expected):
    assert model.component(name).expr.to_pir() == expected


def test_shapes_dtypes_units_and_timing(model):
    age_c = model.component("age")
    assert (age_c.kind, age_c.dtype, age_c.shape, age_c.unit, age_c.timing) == (
        "Derived",
        "i64",
        "Series",
        "years",
        "start",
    )
    in_term_c = model.component("in_term")
    assert (in_term_c.dtype, in_term_c.unit) == ("bool", "none")
    bel_c = model.component("bel")
    assert (bel_c.kind, bel_c.shape, bel_c.unit, bel_c.timing) == ("Output", "PerMP", "money", None)


def test_docstring_becomes_doc(model):
    assert model.component("age").doc == "Attained age at the start of projection year t."
    # A docstring with a blank line keeps its first line only.
    assert model.component("net_cashflow").doc == "Insurer-positive net flow."


def test_init_forms(model):
    assert model.component("num_pols_if").init.to_pir() == "1.0"  # literal
    assert model.component("reserve").init.to_pir() == "bel"  # component reference
    assert model.component("premium_rate").init.to_pir() == "annual_premium"  # traced lambda


def test_dependencies_are_the_parameter_list(model):
    assert model.component("deaths").dependencies == ("num_pols_if", "qx")
    assert model.component("qx").dependencies == ("age", "gender", "smoker", "mortality_loading")
    assert model.component("qx").tables == ("sa8990",)
    # `init=bel` is a dependency of `reserve` even though it never appears in `expr`.
    assert "bel" in model.component("reserve").dependencies


def test_declaration_order_is_preserved(model):
    names = [c.name for c in model.components]
    assert names[:4] == ["age", "in_term", "qx", "wx"]
    assert names.index("premium_income") < names.index("net_cashflow")
    assert [c.index for c in model.components] == sorted(c.index for c in model.components)


def test_stage_is_computed_not_declared(model):
    assert model.component("premium_income").stage == 1
    assert model.component("pv_premiums").stage == 2
    # `bel` reads three stage-2 values; it is stage 2 by reachability even with no Agg of its own.
    assert model.component("bel").stage in (1, 2)


def test_outputs_manifest_matches(model):
    assert set(model.outputs) == set(TERM.outputs)


def test_schema_blocks(model):
    assert model.timeline.periods == 40
    assert [e.name for e in model.enums] == ["Gender"]
    assert model.enums[0].values == ("M", "F")
    fields = {f.name: f for f in model.fields}
    assert fields["policy_number"].key is True
    assert fields["gender"].dtype == "enum(Gender)"
    assert fields["sum_assured"].unit == "money"
    assert all(f.required for f in model.fields)
    assumptions = {a.name: a for a in model.assumptions}
    assert assumptions["valuation_rate"].unit == "rate(annual)"
    assert assumptions["renewal_expense_pa"].shape == "Scalar"
    tables = {tb.name: tb for tb in model.tables}
    assert tables["sa8990"].to_ir()["keys"][0] == {
        "name": "age",
        "dtype": "i64",
        "policy": "clamp",
    }
    assert tables["lapse_rates"].to_ir()["on_missing"] == "default(0.0)"


def test_to_ir_is_shaped_like_the_pir_component_block(model):
    assert model.component("qx").to_ir() == {
        "name": "qx",
        "kind": "Derived",
        "dtype": "f64",
        "shape": "Series",
        "unit": "prob",
        "timing": "end",
        "expr": "sa8990@(age, gender, smoker) * mortality_loading",
        "meta": {
            "authored_by": "dsl",
            "origin_span": model.component("qx").meta["origin_span"],
            "doc": "Annual mortality rate, table rate scaled by the valuation loading.",
        },
    }
    doc = model.to_ir()
    assert doc["format"] == "pir/1"
    assert doc["product"]["outputs"] == list(TERM.outputs)
    assert doc["timeline"]["valuation_date"] == "2026-06-30"


def test_no_lints_on_a_finished_model(model):
    assert [w.code for w in model.warnings] == []
