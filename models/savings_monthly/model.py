"""`savings_monthly` — unit-linked endowment with an account value, monthly, 360 periods.

The structural workout of the corpus. Where `term_annual` is a survival model with a few
cashflows hung off it, this one has a **fund**: a balance that rolls forward, is charged
against, and is what every benefit is defined in terms of. That makes it branch-heavy
(`max`, `min`, `when` everywhere — the sum at risk, the maturity guarantee, the surrender
floor) and table-heavy: four lookups per policy per month.

Product summary
---------------
Regular monthly premium, payable in advance for the whole term. Premiums are allocated to
the account at an allocation rate that is lower in the first policy year. The account is
credited monthly, charged a monthly policy fee, an annual management charge taken monthly,
and a cost of insurance on the sum at risk. Benefits:

* **death**   — the greater of the sum assured and the account value;
* **surrender** — the account value less a penalty that runs off over eight years, floored
  at zero;
* **maturity** — the greater of the account value and a guaranteed return of a stated
  proportion of the premiums paid.

Actuarial documentation: `docs/v2/reference-models.md`.
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
from predictable.fn import max_, npv, pow_, retime, sum_, when
from predictable.timing import END, MID, POINT, START
from predictable.units import Count, Factor, Flag, Money, Num, Prob, Rate, Years

# ---------------------------------------------------------------------------------------
# schema
# ---------------------------------------------------------------------------------------

timeline(
    basis="monthly",
    periods=360,
    origin="policy",
    valuation_date=date(2026, 6, 30),
    year_convention="act/365",
)


class SavingsMP(ModelPoint):
    """One unit-linked endowment."""

    policy_number: str = key()
    entry_age: Years
    sex: str
    smoker: bool
    monthly_premium: Money
    policy_term: Years
    initial_account: Money
    sum_assured: Money
    expense_band: int
    cohort: str


valuation_rate = assumption(Rate.annual)
credit_rate = assumption(Rate.annual)
mortality_loading = assumption(Factor)
lapse_loading = assumption(Factor)
coi_loading = assumption(Factor)
alloc_rate_year1 = assumption(Factor)
alloc_rate_renewal = assumption(Factor)
policy_fee_pm = assumption(Money)
amc_pa = assumption(Rate.annual)
expense_inflation = assumption(Rate.annual)
renewal_expense_pa = assumption(Money)
initial_expense_pct = assumption(Factor)
guarantee_pct = assumption(Factor)

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

surrender_penalty = table(
    source="tables/surrender.csv",
    keys=[Key("policy_year", int, policy="step")],
    values=[Value("penalty", Factor)],
    on_missing="error",
)

yield_curve = table(
    source="tables/yield_curve.csv",
    keys=[Key("term", int, policy="clamp")],
    values=[Value("spot", Rate.annual)],
    on_missing="error",
)

expenses = table(
    source="tables/expenses.csv",
    keys=[Key("band", int, policy="step")],
    values=[Value("scale", Factor)],
    on_missing="error",
)


# ---------------------------------------------------------------------------------------
# calendar and status
# ---------------------------------------------------------------------------------------


@series(timing=START)
def age(entry_age: Years, policy_year: Years) -> Years:
    """Attained age; steps on the policy anniversary, not every month."""
    return entry_age + policy_year - 1


@per_mp()
def term_months(policy_term: Years) -> Num:
    """The contract term in projection periods."""
    return policy_term * 12


@series(timing=START)
def in_term(term_months: Num) -> Flag:
    """True for `t = 0 .. term_months - 1` — the months a premium is actually paid."""
    return t < term_months


@series(timing=START)
def in_force_factor(in_term: Flag) -> Factor:
    return when(in_term, 1.0, 0.0)


@series(timing=END)
def is_maturity(term_months: Num) -> Flag:
    """The single month in which the contract matures."""
    return t == term_months - 1


# ---------------------------------------------------------------------------------------
# rates
# ---------------------------------------------------------------------------------------


@series(timing=END)
def qx_annual(
    sex: str, smoker: bool, age: Years, mortality_loading: Factor, mortality=mortality
) -> Prob:
    return mortality(age, sex, smoker) * mortality_loading


@series(timing=END)
def qx(qx_annual: Prob) -> Prob:
    """Monthly mortality on the constant-force assumption."""
    return 1 - pow_(1 - qx_annual, 1.0 / 12.0)


@series(timing=END)
def wx_annual(policy_year: Years, lapse_loading: Factor, lapses=lapses) -> Prob:
    return lapses(policy_year) * lapse_loading


@series(timing=END)
def wx(wx_annual: Prob, is_maturity: Flag) -> Prob:
    """Monthly surrender rate. Nobody surrenders in the maturity month."""
    return when(is_maturity, 0.0, 1 - pow_(1 - wx_annual, 1.0 / 12.0))


@series(timing=START)
def spot_rate(policy_year: Years, yield_curve=yield_curve) -> Rate.annual:
    """The annually compounded spot rate for a cashflow at the end of policy year `y`."""
    return yield_curve(policy_year)


@per_mp()
def credit_rate_m(credit_rate: Rate.annual) -> Num:
    """Monthly equivalent of the annual investment return credited to the account."""
    return pow_(1 + credit_rate, 1.0 / 12.0) - 1


@per_mp()
def amc_m(amc_pa: Rate.annual) -> Num:
    """Monthly equivalent of the annual management charge."""
    return 1 - pow_(1 - amc_pa, 1.0 / 12.0)


@per_mp()
def expense_scale(expense_band: int, expenses=expenses) -> Factor:
    return expenses(expense_band)


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


@series(timing=END, output=True)
def maturities(num_pols_if: Count, qx: Prob, wx: Prob, is_maturity: Flag) -> Count:
    """Everyone still standing at the end of the maturity month matures."""
    return num_pols_if * (1 - qx) * (1 - wx) * when(is_maturity, 1.0, 0.0)


# ---------------------------------------------------------------------------------------
# the account value
# ---------------------------------------------------------------------------------------


@series(timing=START)
def alloc_rate(policy_year: Years, alloc_rate_year1: Factor, alloc_rate_renewal: Factor) -> Factor:
    """Premium allocation rate: reduced in policy year 1 to recover acquisition costs."""
    return when(policy_year == 1, alloc_rate_year1, alloc_rate_renewal)


@series(timing=START, output=True)
def allocated_premium(monthly_premium: Money, alloc_rate: Factor, in_force_factor: Factor) -> Money:
    """The part of the premium that reaches the account, per policy in force."""
    return monthly_premium * alloc_rate * in_force_factor


@series(timing=POINT, init=lambda initial_account: initial_account)
def account_value(
    account_value: Money,
    allocated_premium: Money,
    policy_charges: Money,
    credit_rate_m: Num,
    in_force_factor: Factor,
) -> Money:
    """Account value per policy in force, at the start of month `t`.

    The order inside the month is fixed and visible: allocate the premium, take the
    charges, then credit the return on what is left. Changing that order changes the
    answer, so it is written as one expression rather than three components.
    """
    return (
        (account_value[t - 1] + allocated_premium[t - 1] - policy_charges[t - 1])
        * (1 + credit_rate_m)
        * in_force_factor
    )


@series(timing=START)
def sum_at_risk(sum_assured: Money, account_value: Money) -> Money:
    """`max(SA - AV, 0)`: the part of the death benefit the insurer actually carries."""
    return max_(sum_assured - account_value, 0.0)


@series(timing=START, output=True)
def cost_of_insurance(sum_at_risk: Money, qx: Prob, coi_loading: Factor) -> Money:
    """Monthly mortality charge on the sum at risk."""
    return sum_at_risk * qx * coi_loading


@series(timing=START, output=True)
def management_charge(account_value: Money, amc_m: Num) -> Money:
    """Annual management charge, taken monthly on the account value."""
    return account_value * amc_m


@series(timing=START, output=True)
def policy_fee(policy_fee_pm: Money, expense_inflation: Rate.annual, policy_year: Years) -> Money:
    """Flat monthly fee, escalating on each policy anniversary rather than each month."""
    return policy_fee_pm * pow_(1 + expense_inflation, policy_year - 1)


@series(timing=START)
def policy_charges(
    cost_of_insurance: Money, management_charge: Money, policy_fee: Money, in_force_factor: Factor
) -> Money:
    """Everything deducted from the account in month `t`."""
    return (cost_of_insurance + management_charge + policy_fee) * in_force_factor


@series(timing=POINT, init=lambda monthly_premium: monthly_premium)
def premiums_paid(premiums_paid: Money, monthly_premium: Money, in_force_factor: Factor) -> Money:
    """Cumulative premiums paid to the start of month `t`. The guarantee is defined on it."""
    return premiums_paid[t - 1] + monthly_premium * in_force_factor


# ---------------------------------------------------------------------------------------
# benefits
# ---------------------------------------------------------------------------------------


@series(timing=END)
def death_benefit(sum_assured: Money, account_value: Money) -> Money:
    """`max(SA, AV)` per policy: the account never buys less than the sum assured."""
    return max_(sum_assured, account_value)


@series(timing=END)
def surrender_benefit(
    account_value: Money, policy_year: Years, surrender_penalty=surrender_penalty
) -> Money:
    """Account value less the surrender penalty, floored at zero."""
    return max_(account_value * (1 - surrender_penalty(policy_year)), 0.0)


@series(timing=END)
def guaranteed_maturity(premiums_paid: Money, guarantee_pct: Factor) -> Money:
    """The maturity floor: a stated proportion of the premiums actually paid."""
    return premiums_paid * guarantee_pct


@series(timing=END)
def maturity_benefit(account_value: Money, guaranteed_maturity: Money) -> Money:
    """`max(AV, guarantee)` — the guarantee bites only when the fund underperforms."""
    return max_(account_value, guaranteed_maturity)


@series(timing=END, output=True)
def guarantee_cost(maturity_benefit: Money, account_value: Money, maturities: Count) -> Money:
    """The part of the maturity payment the guarantee, not the fund, is paying for."""
    return (maturity_benefit - retime(account_value, END)) * maturities


@series(timing=END, output=True)
def death_claims(death_benefit: Money, deaths: Count) -> Money:
    return death_benefit * deaths


@series(timing=END, output=True)
def surrender_claims(surrender_benefit: Money, surrenders: Count) -> Money:
    return surrender_benefit * surrenders


@series(timing=END, output=True)
def maturity_claims(maturity_benefit: Money, maturities: Count) -> Money:
    return maturity_benefit * maturities


# ---------------------------------------------------------------------------------------
# shareholder cashflow
# ---------------------------------------------------------------------------------------


@series(timing=START, output=True)
def premium_income(monthly_premium: Money, num_pols_if: Count, in_force_factor: Factor) -> Money:
    return monthly_premium * num_pols_if * in_force_factor


@series(timing=START, output=True)
def renewal_expenses(
    renewal_expense_pa: Money,
    expense_inflation: Rate.annual,
    expense_scale: Factor,
    num_pols_if: Count,
    in_force_factor: Factor,
    policy_year: Years,
) -> Money:
    return (
        renewal_expense_pa
        / 12
        * pow_(1 + expense_inflation, policy_year - 1)
        * expense_scale
        * num_pols_if
        * in_force_factor
    )


@per_mp(output=True)
def initial_expense(initial_expense_pct: Factor, monthly_premium: Money) -> Money:
    return initial_expense_pct * monthly_premium * 12


@series(timing=MID, output=True)
def net_cashflow(
    premium_income: Money,
    death_claims: Money,
    surrender_claims: Money,
    maturity_claims: Money,
    renewal_expenses: Money,
    num_pols_if: Count,
) -> Money:
    """Insurer-positive net flow, all legs restated to mid-month.

    Claims are per-policy benefits multiplied by decrement counts already, so the only
    per-policy quantity left to scale is the premium.
    """
    return (
        retime(premium_income, MID)
        - retime(death_claims, MID)
        - retime(surrender_claims, MID)
        - retime(maturity_claims, MID)
        - retime(renewal_expenses, MID)
    )


# ---------------------------------------------------------------------------------------
# valuation
# ---------------------------------------------------------------------------------------


@series(timing=POINT)
def disc_factor(spot_rate: Rate.annual) -> Factor:
    """`v(t) = (1 + s_y)^(-t/12)` off the spot curve, not a flat rate.

    The savings model discounts on a term structure precisely so that the corpus contains
    one model whose discounting is a per-period table lookup rather than a recursion.
    """
    return pow_(1 + spot_rate, 0 - t / 12.0)


@per_mp(output=True)
def pv_premiums(premium_income: Money, disc_factor: Factor) -> Money:
    return npv(premium_income, disc_factor)


@per_mp(output=True)
def pv_death_claims(death_claims: Money, disc_factor: Factor) -> Money:
    return npv(death_claims, disc_factor)


@per_mp(output=True)
def pv_surrender_claims(surrender_claims: Money, disc_factor: Factor) -> Money:
    return npv(surrender_claims, disc_factor)


@per_mp(output=True)
def pv_maturity_claims(maturity_claims: Money, disc_factor: Factor) -> Money:
    return npv(maturity_claims, disc_factor)


@per_mp(output=True)
def pv_expenses(renewal_expenses: Money, disc_factor: Factor) -> Money:
    return npv(renewal_expenses, disc_factor)


@per_mp(output=True)
def pv_guarantee_cost(guarantee_cost: Money, disc_factor: Factor) -> Money:
    """The cost of the maturity guarantee, on the central assumption set.

    This is a deterministic, not a stochastic, valuation of an option: it is the value of
    the guarantee *if the world does exactly what the assumptions say*. It is here because
    it is the number a sensitivity fan is most worth running over.
    """
    return npv(guarantee_cost, disc_factor)


@per_mp(output=True)
def bel(
    pv_death_claims: Money,
    pv_surrender_claims: Money,
    pv_maturity_claims: Money,
    pv_expenses: Money,
    initial_expense: Money,
    pv_premiums: Money,
) -> Money:
    """Best estimate liability at issue."""
    return (
        pv_death_claims
        + pv_surrender_claims
        + pv_maturity_claims
        + pv_expenses
        + initial_expense
        - pv_premiums
    )


@series(timing=END, output=True)
def account_at_maturity(account_value: Money, maturities: Count) -> Money:
    """The account value carried into the maturity payment, zero in every other month."""
    return retime(account_value, END) * maturities


@per_mp(output=True)
def final_account_value(account_at_maturity: Money) -> Money:
    """The account value at maturity, as a per-policy check figure.

    An aggregate's operand must be a plain series reference (`03-engine.md` §5.3), which is
    why the product above is a named component rather than an expression inside `sum`.
    """
    return sum_(account_at_maturity)


@per_mp(output=True)
def profit_margin(bel: Money, pv_premiums: Money) -> Num:
    return -bel / pv_premiums


product(
    name="SAVINGS_MONTHLY",
    modules=["model"],
    outputs=[
        "deaths",
        "surrenders",
        "maturities",
        "allocated_premium",
        "cost_of_insurance",
        "management_charge",
        "policy_fee",
        "guarantee_cost",
        "account_at_maturity",
        "death_claims",
        "surrender_claims",
        "maturity_claims",
        "premium_income",
        "renewal_expenses",
        "initial_expense",
        "net_cashflow",
        "pv_premiums",
        "pv_death_claims",
        "pv_surrender_claims",
        "pv_maturity_claims",
        "pv_expenses",
        "pv_guarantee_cost",
        "bel",
        "final_account_value",
        "profit_margin",
    ],
    key_field="policy_number",
    doc="Unit-linked endowment, monthly — reference model 3 of 03-engine.md §11.2.",
)
