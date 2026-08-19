"""The fixture model `predictable build` is tested end to end against.

Small on purpose and complete on purpose: a timeline, an enum, a modelpoint schema, an
assumption, a CSV-backed table (so the build has a real digest to compute), a recursive series
with an `init`, a stage-2 reduction and an output manifest. Everything `02-dsl.md` §8 has to
render is exercised by one import of this file.
"""

from datetime import date

from predictable import (
    Enum,
    Key,
    ModelPoint,
    Value,
    assumption,
    key,
    per_mp,
    product,
    series,
    t,
    table,
    timeline,
)
from predictable.fn import npv
from predictable.timing import END, START
from predictable.units import Count, Factor, Flag, Money, Prob, Rate, Years

timeline(basis="annual", periods=10, origin="policy", valuation_date=date(2026, 6, 30))


class Gender(Enum):
    M = "M"
    F = "F"


class TermMP(ModelPoint):
    policy_number: str = key()
    entry_age: Years
    gender: Gender
    sum_assured: Money
    policy_term: Years


valuation_rate = assumption(Rate.annual)
mortality_loading = assumption(Factor)

mortality = table(
    source="tables/mortality.csv",
    keys=[Key("age", int, policy="clamp")],
    values=[Value("qx", Prob)],
    on_missing="error",
)


@series(timing=START)
def age(entry_age: Years) -> Years:
    """Attained age at the start of projection year t."""
    return entry_age + t


@series(timing=START)
def in_term(policy_term: Years) -> Flag:
    return t < policy_term


@series(timing=END)
def qx(age: Years, mortality_loading: Factor, mortality=mortality) -> Prob:
    """Table rate scaled by the valuation loading."""
    return mortality(age) * mortality_loading


@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, in_term: Flag) -> Count:
    """Survivorship, self-referential at lag 1."""
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * in_term


@series(timing=END, output=True)
def death_claims(num_pols_if: Count, qx: Prob, sum_assured: Money) -> Money:
    """Expected claims in the year."""
    return num_pols_if * qx * sum_assured


@per_mp(output=True)
def pv_claims(death_claims: Money, valuation_rate: Rate.annual) -> Money:
    """The present value of claims — a stage-2 reduction over the whole projection."""
    return npv(death_claims, valuation_rate)


product(
    name="TERM_ANNUAL",
    modules=["model"],
    outputs=["death_claims", "pv_claims"],
    key_field="policy_number",
    doc="The fixture product of the predictable build tests.",
)
