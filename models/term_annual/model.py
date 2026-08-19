"""`term_annual` — level term assurance, annual projection, 40 periods.

The reference model. It is deliberately the smallest thing that is still recognisably an
actuarial model: a survival model with two decrements, an escalating premium, inflating
renewal expenses, a discounted best-estimate liability and a retrospective reserve
roll-forward seeded from that liability.

Actuarial documentation, including every formula in symbols, lives in
`docs/v2/reference-models.md`. This file is the model; that page is the basis note.

Build:  `predictable build models/term_annual/model.py --out models/term_annual/build`
Run:    `predictable run models/term_annual/run.pir`
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
from predictable.fn import compound, npv, retime, when
from predictable.timing import END, MID, POINT, START
from predictable.units import Count, Factor, Flag, Money, Num, Prob, Rate, Years

# ---------------------------------------------------------------------------------------
# schema
# ---------------------------------------------------------------------------------------

timeline(
    basis="annual",
    periods=40,
    origin="policy",
    valuation_date=date(2026, 6, 30),
    year_convention="act/365",
)


class TermMP(ModelPoint):
    """One level-term policy.

    `sex` and `smoker` are rating factors and also the mortality table's key columns, so
    they are carried as `str`/`bool` rather than as a derived code.
    """

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
premium_escalation = assumption(Rate.annual)
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
# decrements
# ---------------------------------------------------------------------------------------


@series(timing=START)
def age(entry_age: Years) -> Years:
    """Attained age at the start of projection year `t`."""
    return entry_age + t


@series(timing=START)
def in_term(policy_term: Years) -> Flag:
    """True while the policy is inside its contractual term."""
    return t < policy_term


@series(timing=START)
def in_force_factor(in_term: Flag) -> Factor:
    """1 inside the term, 0 after it. The single on/off switch every cashflow multiplies by."""
    return when(in_term, 1.0, 0.0)


@series(timing=END)
def qx(sex: str, smoker: bool, age: Years, mortality_loading: Factor, mortality=mortality) -> Prob:
    """Annual mortality rate: the table rate scaled by the valuation loading."""
    return mortality(age, sex, smoker) * mortality_loading


@series(timing=END)
def wx(policy_year: Years, lapse_loading: Factor, lapses=lapses) -> Prob:
    """Annual lapse rate, stepped on policy year, scaled by the lapse loading."""
    return lapses(policy_year) * lapse_loading


@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """Policies in force at the start of year `t`, per policy sold.

    Decrements are applied in the multiple-decrement order death-then-lapse, so the two
    do not compete for the same exposure. Self-referential at lag 1, which is the only
    self-reference the IR allows (`01-ir.md` §3.1).
    """
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * when(in_term, 1.0, 0.0)


@series(timing=END, output=True)
def deaths(num_pols_if: Count, qx: Prob, in_force_factor: Factor) -> Count:
    """Expected deaths during year `t`."""
    return num_pols_if * qx * in_force_factor


@series(timing=END, output=True)
def surrenders(num_pols_if: Count, qx: Prob, wx: Prob, in_force_factor: Factor) -> Count:
    """Expected lapses during year `t`, on the lives that survived the year."""
    return num_pols_if * (1 - qx) * wx * in_force_factor


# ---------------------------------------------------------------------------------------
# cashflows
# ---------------------------------------------------------------------------------------


@per_mp()
def expense_scale(expense_band: int, expenses=expenses) -> Factor:
    """Per-policy expense scaling from the servicing band. Constant in `t`, so `PerMP`."""
    return expenses(expense_band)


@series(timing=START, init=lambda annual_premium: annual_premium)
def premium_rate(premium_rate: Money, premium_escalation: Rate.annual) -> Money:
    """Office premium payable in year `t`, escalating annually from the issue premium."""
    return premium_rate[t - 1] * (1 + premium_escalation)


@series(timing=START, output=True)
def premium_income(premium_rate: Money, num_pols_if: Count, in_force_factor: Factor) -> Money:
    """Premium received in advance at the start of year `t`."""
    return premium_rate * num_pols_if * in_force_factor


@series(timing=END, output=True)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    """Death benefit paid at the end of year `t`."""
    return sum_assured * deaths


@series(timing=START, output=True)
def renewal_expenses(
    renewal_expense_pa: Money,
    expense_inflation: Rate.annual,
    expense_scale: Factor,
    num_pols_if: Count,
    in_force_factor: Factor,
) -> Money:
    """Per-policy servicing expense, inflated from the valuation date and band-scaled."""
    return (
        renewal_expense_pa
        * compound(expense_inflation, t)
        * expense_scale
        * num_pols_if
        * in_force_factor
    )


@per_mp(output=True)
def initial_expense(initial_expense_pct: Factor, annual_premium: Money) -> Money:
    """Acquisition expense, incurred once at issue as a percentage of the first premium."""
    return initial_expense_pct * annual_premium


@series(timing=MID, output=True)
def net_cashflow(premium_income: Money, death_claims: Money, renewal_expenses: Money) -> Money:
    """Insurer-positive net flow, restated mid-year.

    Retiming is explicit rather than implied so the sign and timing conventions are visible
    to a reviewer and diffable by `predictable diff`.
    """
    return retime(premium_income, MID) - retime(death_claims, MID) - retime(renewal_expenses, MID)


# ---------------------------------------------------------------------------------------
# valuation
# ---------------------------------------------------------------------------------------


@series(timing=POINT, init=1.0)
def disc_factor(disc_factor: Factor, valuation_rate: Rate.annual) -> Factor:
    """v^t at the flat valuation rate.

    Written as a recursion rather than `pow` so that moving to a term structure is a
    one-line change to the divisor and nothing else.
    """
    return disc_factor[t - 1] / (1 + valuation_rate)


@per_mp(output=True)
def pv_premiums(premium_income: Money, disc_factor: Factor) -> Money:
    """PV of future office premiums."""
    return npv(premium_income, disc_factor)


@per_mp(output=True)
def pv_claims(death_claims: Money, disc_factor: Factor) -> Money:
    """PV of future death claims."""
    return npv(death_claims, disc_factor)


@per_mp(output=True)
def pv_expenses(renewal_expenses: Money, disc_factor: Factor) -> Money:
    """PV of future renewal expenses."""
    return npv(renewal_expenses, disc_factor)


@per_mp(output=True)
def bel(
    pv_claims: Money, pv_expenses: Money, pv_premiums: Money, initial_expense: Money
) -> Money:
    """Best estimate liability at `t = 0` — the prospective gross premium reserve."""
    return pv_claims + pv_expenses + initial_expense - pv_premiums


@per_mp(output=True)
def profit_margin(bel: Money, pv_premiums: Money) -> Num:
    """New-business margin: minus the BEL as a proportion of the PV of premiums."""
    return -bel / pv_premiums


@series(timing=POINT, init=bel, output=True)
def reserve(
    reserve: Money,
    premium_income: Money,
    death_claims: Money,
    renewal_expenses: Money,
    valuation_rate: Rate.annual,
    policy_term: Years,
) -> Money:
    """Retrospective reserve roll-forward, seeded from the prospective `bel`.

    The `init = bel` link is the one legal backward channel from stage 2 into stage 1
    (`01-ir.md` §8.2): a reduction over the whole projection may seed a series at `t = 0`,
    and nowhere else.
    """
    return (
        (reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1] - renewal_expenses[t - 1])
        * (1 + valuation_rate)
        * when(t <= policy_term, 1.0, 0.0)
    )


product(
    name="TERM_ANNUAL",
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
    doc="Level term assurance on an annual basis — the reference model of 03-engine.md §11.2.",
)
