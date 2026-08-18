"""`term_monthly` — the `term_annual` product, projected monthly over 480 periods.

Same contract, same portfolio file, same tables. The only thing that changes is the
projection basis, and this model exists to make the *consequences* of that change explicit:
rate conversion, an attained age that only steps on the policy anniversary, a policy term
measured in years against a `t` measured in months, and 480 rather than 40 iterations of the
`t` loop.

Actuarial documentation: `docs/v2/reference-models.md`.

Build:  `predictable build models/term_monthly/model.py --out models/term_monthly/build`
Run:    `predictable run models/term_monthly/run.pir`
"""

from datetime import date

from predictable import (
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
from predictable.fn import npv, pow_, retime, when
from predictable.timing import END, MID, POINT, START
from predictable.units import Count, Factor, Flag, Money, Num, Prob, Rate, Years

# ---------------------------------------------------------------------------------------
# schema
# ---------------------------------------------------------------------------------------

timeline(
    basis="monthly",
    periods=480,
    origin="policy",
    valuation_date=date(2026, 6, 30),
    year_convention="act/365",
)


class TermMP(ModelPoint):
    """One level-term policy. Identical to `term_annual`'s: the file is shared."""

    policy_number: str = key()
    entry_age: Years
    sex: str
    smoker: bool
    sum_assured: Money
    annual_premium: Money
    policy_term: Years
    expense_band: int


valuation_rate = assumption(Rate.annual)
mortality_loading = assumption(Factor)
lapse_loading = assumption(Factor)
expense_inflation = assumption(Rate.annual)
renewal_expense_pa = assumption(Money)
initial_expense_pct = assumption(Factor)

mortality = table(
    source="tables/mortality.csv",
    keys=[Key("age", int, policy="clamp"), Key("sex", str), Key("smoker", bool)],
    values=[Value("qx", Prob)],
    on_missing="error",
)

lapses = table(
    source="tables/lapses.csv",
    keys=[Key("policy_year", int, policy="step")],
    values=[Value("lapse_pa", Prob)],
    on_missing="error",
)

expenses = table(
    source="tables/expenses.csv",
    keys=[Key("band", int, policy="step")],
    values=[Value("scale", Factor)],
    on_missing="error",
)


# ---------------------------------------------------------------------------------------
# the annual -> monthly bridge
# ---------------------------------------------------------------------------------------


@series(timing=START)
def age(entry_age: Years, policy_year: Years) -> Years:
    """Attained age.

    `policy_year` is `t // 12 + 1` on a monthly basis, so the age steps on the policy
    anniversary and not every month — which is what makes the annual and monthly models
    read the same row of the mortality table within a policy year.
    """
    return entry_age + policy_year - 1


@series(timing=START)
def in_term(policy_term: Years) -> Flag:
    """`policy_term` is in years; `t` is in months. The conversion is the whole point."""
    return t < policy_term * 12


@series(timing=START)
def in_force_factor(in_term: Flag) -> Factor:
    return when(in_term, 1.0, 0.0)


@series(timing=END)
def qx_annual(
    sex: str, smoker: bool, age: Years, mortality_loading: Factor, mortality=mortality
) -> Prob:
    """The table's annual rate at the attained age."""
    return mortality(age, sex, smoker) * mortality_loading


@series(timing=END)
def qx(qx_annual: Prob) -> Prob:
    """Monthly mortality, on the *constant force* assumption within the policy year.

    `q_m = 1 - (1 - q_a)^(1/12)`. The uniform-distribution-of-deaths alternative,
    `q_a / 12`, is a materially different number at high ages and is deliberately not used;
    a model that wants it should say so in one visible line here.
    """
    return 1 - pow_(1 - qx_annual, 1.0 / 12.0)


@series(timing=END)
def wx(policy_year: Years, lapse_loading: Factor, lapses=lapses) -> Prob:
    """Monthly lapse rate, from the annual rate on the same constant-force basis."""
    return 1 - pow_(1 - lapses(policy_year) * lapse_loading, 1.0 / 12.0)


@per_mp()
def monthly_valuation_rate(valuation_rate: Rate.annual) -> Num:
    """`(1 + i)^(1/12) - 1`: the effective monthly rate equivalent to the annual basis."""
    return pow_(1 + valuation_rate, 1.0 / 12.0) - 1


@per_mp()
def monthly_expense_inflation(expense_inflation: Rate.annual) -> Num:
    return pow_(1 + expense_inflation, 1.0 / 12.0) - 1


# ---------------------------------------------------------------------------------------
# decrements
# ---------------------------------------------------------------------------------------


@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """Policies in force at the start of month `t`, per policy sold."""
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * when(in_term, 1.0, 0.0)


@series(timing=END, output=True)
def deaths(num_pols_if: Count, qx: Prob, in_force_factor: Factor) -> Count:
    return num_pols_if * qx * in_force_factor


@series(timing=END, output=True)
def surrenders(num_pols_if: Count, qx: Prob, wx: Prob, in_force_factor: Factor) -> Count:
    return num_pols_if * (1 - qx) * wx * in_force_factor


# ---------------------------------------------------------------------------------------
# cashflows
# ---------------------------------------------------------------------------------------


@per_mp()
def expense_scale(expense_band: int, expenses=expenses) -> Factor:
    return expenses(expense_band)


@per_mp()
def monthly_premium(annual_premium: Money) -> Money:
    """Premiums are payable monthly in advance at one twelfth of the office premium.

    No fractional-premium loading is applied. That is a pricing decision, and if it were
    applied it would be an assumption, not a hard-coded factor.
    """
    return annual_premium / 12


@series(timing=START, output=True)
def premium_income(monthly_premium: Money, num_pols_if: Count, in_force_factor: Factor) -> Money:
    return monthly_premium * num_pols_if * in_force_factor


@series(timing=END, output=True)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    return sum_assured * deaths


@series(timing=START, output=True)
def renewal_expenses(
    renewal_expense_pa: Money,
    monthly_expense_inflation: Num,
    expense_scale: Factor,
    num_pols_if: Count,
    in_force_factor: Factor,
) -> Money:
    """One twelfth of the annual per-policy expense, inflated monthly."""
    return (
        renewal_expense_pa
        / 12
        * pow_(1 + monthly_expense_inflation, t)
        * expense_scale
        * num_pols_if
        * in_force_factor
    )


@per_mp(output=True)
def initial_expense(initial_expense_pct: Factor, annual_premium: Money) -> Money:
    return initial_expense_pct * annual_premium


@series(timing=MID, output=True)
def net_cashflow(premium_income: Money, death_claims: Money, renewal_expenses: Money) -> Money:
    return retime(premium_income, MID) - retime(death_claims, MID) - retime(renewal_expenses, MID)


# ---------------------------------------------------------------------------------------
# valuation
# ---------------------------------------------------------------------------------------


@series(timing=POINT, init=1.0)
def disc_factor(disc_factor: Factor, monthly_valuation_rate: Num) -> Factor:
    """v^t at the monthly-equivalent rate."""
    return disc_factor[t - 1] / (1 + monthly_valuation_rate)


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
def bel(
    pv_claims: Money, pv_expenses: Money, pv_premiums: Money, initial_expense: Money
) -> Money:
    return pv_claims + pv_expenses + initial_expense - pv_premiums


@per_mp(output=True)
def profit_margin(bel: Money, pv_premiums: Money) -> Num:
    return -bel / pv_premiums


@series(timing=POINT, init=bel, output=True)
def reserve(
    reserve: Money,
    premium_income: Money,
    death_claims: Money,
    renewal_expenses: Money,
    monthly_valuation_rate: Num,
    policy_term: Years,
) -> Money:
    """Monthly retrospective roll-forward, seeded from `bel` exactly as in `term_annual`."""
    return (
        (reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1] - renewal_expenses[t - 1])
        * (1 + monthly_valuation_rate)
        * when(t <= policy_term * 12, 1.0, 0.0)
    )


product(
    name="TERM_MONTHLY",
    modules=["model"],
    outputs=[
        "deaths",
        "surrenders",
        "premium_income",
        "death_claims",
        "renewal_expenses",
        "initial_expense",
        "net_cashflow",
        "pv_premiums",
        "pv_claims",
        "pv_expenses",
        "bel",
        "profit_margin",
        "reserve",
    ],
    key_field="policy_number",
    doc="Level term assurance on a monthly basis — reference model 2 of 03-engine.md §11.2.",
)
