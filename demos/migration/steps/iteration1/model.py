"""`term_uk` — the migrated model, **iteration 1**. This is the draft that does NOT reconcile.

It is kept in the repository, unedited, because the demo is about the *loop* and a loop
with only its final state recorded is a claim, not a transcript. Two translation errors are
in here, both of the kind that are invisible on a read-through and obvious to a differ:

  1. `qx` reads the SA8990 table and forgets `MORT_LOAD`. The loading lives in the Prophet
     run settings rather than in `TERM_UK.MOD`, which is exactly why it gets missed.
  2. `renewal_expenses` inflates with the exponent `t + 1` rather than `t` — the
     "is the first year inflated or not?" question, answered the wrong way round.

Nothing here was written to make the demo work. `README.md` is the transcript of what the
differ said about it.

Build:  predictable build demos/migration/model.py --out demos/migration/build
Run:    predictable run demos/migration/run.pir
Diff:   predictable diff run demos/migration/prophet/TERM_BASE_2026Q2.rpt \\
            demos/migration/runs/base --mapping demos/migration/migration/mapping.toml \\
            --tolerance-profile reconcile
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
# schema — written and checked before a single formula (step 2)
# ---------------------------------------------------------------------------------------

timeline(
    basis="annual",
    periods=40,
    origin="policy",
    valuation_date=date(2026, 6, 30),
    year_convention="act/365",
)


class TermUkMP(ModelPoint):
    """One TERM_UK policy, from `TERM_UK.mpf`.

    `SPCODE` is dropped (the product is named once in the product block, not per row) and
    `SEX` becomes the rating factor itself rather than Prophet's 1/2 code.
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
# decrements — TERM_UK.MOD lines 21-33
# ---------------------------------------------------------------------------------------


@series(timing=START)
def age(entry_age: Years) -> Years:
    """`ATT_AGE = AGE_AT_ENTRY + t`."""
    return entry_age + t


@series(timing=START)
def in_term(policy_term: Years) -> Flag:
    """`IN_TERM = IF t < POL_TERM THEN 1 ELSE 0`, as a boolean rather than a 0/1 number."""
    return t < policy_term


@series(timing=START)
def in_force_factor(in_term: Flag) -> Factor:
    """The 0/1 form of `IN_TERM`, so every cashflow multiplies by one named switch."""
    return when(in_term, 1.0, 0.0)


@series(timing=END)
def qx(sex: str, smoker: bool, age: Years, mortality=mortality) -> Prob:
    """`MORT_RATE = SA8990(ATT_AGE, SEX, SMOKER) * MORT_LOAD`.

    Iteration 1: the loading is missing.
    """
    return mortality(age, sex, smoker)


@series(timing=END)
def wx(policy_year: Years, lapse_loading: Factor, lapses=lapses) -> Prob:
    """`LAPSE_RATE = LAPSE_TERM(t + 1) * LAPSE_LOAD`.

    Prophet's `t + 1` is predictable's `policy_year` builtin on an annual timeline, and
    saying it that way is what stops the key drifting when the basis changes.
    """
    return lapses(policy_year) * lapse_loading


@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """`POLS_IF` — death first, then lapse on the survivors."""
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * when(in_term, 1.0, 0.0)


@series(timing=END, output=True)
def deaths(num_pols_if: Count, qx: Prob, in_force_factor: Factor) -> Count:
    return num_pols_if * qx * in_force_factor


@series(timing=END, output=True)
def surrenders(num_pols_if: Count, qx: Prob, wx: Prob, in_force_factor: Factor) -> Count:
    return num_pols_if * (1 - qx) * wx * in_force_factor


# ---------------------------------------------------------------------------------------
# cashflows — TERM_UK.MOD lines 36-43
# ---------------------------------------------------------------------------------------


@per_mp()
def expense_scale(expense_band: int, expenses=expenses) -> Factor:
    """`EXP_BAND(EXP_BAND_CD)`. Constant in `t`, so `PerMP` rather than a flat series."""
    return expenses(expense_band)


@series(timing=START, output=True)
def premium_income(annual_premium: Money, num_pols_if: Count, in_force_factor: Factor) -> Money:
    """`PREM_INC = ANN_PREM * POLS_IF * IN_TERM`, received **in advance**: `timing = START`."""
    return annual_premium * num_pols_if * in_force_factor


@series(timing=END, output=True)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    """`DTH_CLAIM = SUM_ASSURED * DEATHS`, paid at the end of the year: `timing = END`."""
    return sum_assured * deaths


@series(timing=START, output=True)
def renewal_expenses(
    renewal_expense_pa: Money,
    expense_inflation: Rate.annual,
    expense_scale: Factor,
    num_pols_if: Count,
    in_force_factor: Factor,
) -> Money:
    """`EXPENSE = EXP_PP * (1 + EXP_INFL)^t * EXP_BAND(...) * POLS_IF * IN_TERM`.

    Iteration 1: inflated with the exponent `t + 1`.
    """
    return (
        renewal_expense_pa
        * compound(expense_inflation, t + 1)
        * expense_scale
        * num_pols_if
        * in_force_factor
    )


@series(timing=MID, output=True)
def net_cashflow(premium_income: Money, death_claims: Money, renewal_expenses: Money) -> Money:
    """`NET_CF = PREM_INC - DTH_CLAIM - EXPENSE`.

    Prophet has no timing tags, so the three flows are simply subtracted. predictable makes
    the restatement explicit; the arithmetic is unchanged, and now the convention is a
    thing a reviewer can see and a differ can compare.
    """
    return retime(premium_income, MID) - retime(death_claims, MID) - retime(renewal_expenses, MID)


# ---------------------------------------------------------------------------------------
# valuation — TERM_UK.MOD lines 46-57
# ---------------------------------------------------------------------------------------


@series(timing=POINT, init=1.0)
def disc_factor(disc_factor: Factor, valuation_rate: Rate.annual) -> Factor:
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
def initial_expense(initial_expense_pct: Factor, annual_premium: Money) -> Money:
    return initial_expense_pct * annual_premium


@per_mp(output=True)
def bel(
    pv_claims: Money, pv_expenses: Money, pv_premiums: Money, initial_expense: Money
) -> Money:
    return pv_claims + pv_expenses + initial_expense - pv_premiums


@series(timing=POINT, init=bel, output=True)
def reserve(
    reserve: Money,
    premium_income: Money,
    death_claims: Money,
    renewal_expenses: Money,
    valuation_rate: Rate.annual,
    policy_term: Years,
) -> Money:
    """The retrospective roll-forward, seeded from `BEL` — the one legal stage-2 → `init`
    back-channel (`01-ir.md` §8.2), and the reason `BEL` need not be recomputed each year."""
    return (
        (reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1] - renewal_expenses[t - 1])
        * (1 + valuation_rate)
        * when(t <= policy_term, 1.0, 0.0)
    )


product(
    name="TERM_UK",
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
        "reserve",
    ],
    key_field="policy_number",
    doc="Migrated from FIS Prophet library TERM_UK, run TERM_BASE_2026Q2 (30/06/2026).",
)
