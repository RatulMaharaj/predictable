"""`ifrs17_gmm` — the IFRS 17 general measurement model on the `savings_monthly` product.

The realistic-workload model of the corpus, and the one that exercises the engine's
**substage levels**. Everything above the `IFRS 17` banner is the `savings_monthly` model
verbatim: the same contract, the same tables, the same fulfilment cash flows. Everything
below it is the measurement model laid on top.

Why this model is structurally different
----------------------------------------
IFRS 17 asks for quantities that depend on the *whole* projection and are then released
*over* the projection. That is a loop the IR deliberately refuses to close in one stage, so
it is expressed as the one legal backward channel (`01-ir.md` §8.2) used twice:

    stage 1  fulfilment cash flows, coverage units          (a Series)
    stage 2  their present values                           (level 1: an Agg)
    stage 1  balances seeded from those values via `init`   (CSM, RA, FCF roll-forwards)
    stage 2  present values of what those balances released (level 2: an Agg)

`03-engine.md` §5.3's substage levelling is what makes the second stage-2 pass legal, and
this model is the corpus's test of it: two levels, no more, and `W0110` if a model ever
needs a third.

Measurement summary
-------------------
* **FCF** — the discounted fulfilment cash flows: `bel` below.
* **RA**  — a risk adjustment, taken as a percentage loading on the present value of
  claims, released in proportion to coverage units.
* **CSM** — the contractual service margin, `max(-(FCF + RA), 0)` at initial recognition,
  accreted at the locked-in rate and released in proportion to coverage units.
* **Loss component** — `max(FCF + RA, 0)`: an onerous contract recognises its loss at once
  and has no CSM.
* **LRC / LIC** — the liability for remaining coverage is `FCF balance + RA + CSM`; the
  liability for incurred claims carries one month of settlement lag.

The identity worth knowing: for a profitable contract, `lrc` at `t = 0` is **zero** — no
gain is recognised at initial recognition. `lrc_at_issue` below is that check, as an output.

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
from predictable.fn import first, max_, min_, npv, pow_, retime, sum_, when
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
    """One unit-linked endowment, measured under IFRS 17."""

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
ra_pct = assumption(Factor)
locked_in_rate = assumption(Rate.annual)

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



# =======================================================================================
# IFRS 17 — general measurement model
# =======================================================================================


@per_mp()
def locked_in_rate_m(locked_in_rate: Rate.annual) -> Num:
    """The discount rate locked in at initial recognition, monthly. The CSM accretes at it."""
    return pow_(1 + locked_in_rate, 1.0 / 12.0) - 1


@series(timing=START, output=True)
def net_outflow(
    premium_income: Money,
    death_claims: Money,
    surrender_claims: Money,
    maturity_claims: Money,
    renewal_expenses: Money,
) -> Money:
    """Fulfilment cash *out*flow net of premiums, in month `t`. The mirror of `net_cashflow`."""
    return (
        retime(death_claims, START)
        + retime(surrender_claims, START)
        + retime(maturity_claims, START)
        + renewal_expenses
        - premium_income
    )


# -- level 1: quantities that need the whole projection ---------------------------------


@per_mp(output=True)
def risk_adjustment(
    pv_death_claims: Money, pv_surrender_claims: Money, pv_maturity_claims: Money, ra_pct: Factor
) -> Money:
    """Risk adjustment for non-financial risk, at initial recognition.

    A percentage loading on the present value of claims. This is a *confidence-level-free*
    proxy, chosen because it is auditable in one line; a cost-of-capital or quantile
    calibration replaces exactly this component and nothing else.
    """
    return ra_pct * (pv_death_claims + pv_surrender_claims + pv_maturity_claims)


@per_mp(output=True)
def csm_initial(bel: Money, risk_adjustment: Money) -> Money:
    """CSM at initial recognition: `max(-(FCF + RA), 0)`. Never negative — that is the point."""
    return max_(0.0 - (bel + risk_adjustment), 0.0)


@per_mp(output=True)
def loss_component_initial(bel: Money, risk_adjustment: Money) -> Money:
    """The onerous-contract loss recognised immediately: `max(FCF + RA, 0)`."""
    return max_(bel + risk_adjustment, 0.0)


@per_mp(output=True)
def is_onerous(loss_component_initial: Money) -> Num:
    """1 for an onerous contract, 0 otherwise. Reported so a cohort can be split on it."""
    return when(loss_component_initial > 0.0, 1.0, 0.0)


@series(timing=START, output=True)
def coverage_units(num_pols_if: Count, sum_assured: Money) -> Money:
    """The service provided in month `t`: sum assured in force, per 1000.

    Coverage units are a policy choice under IFRS 17.B119. Sum assured in force is used
    here because the benefit is a death benefit; a savings-dominant contract would more
    often use the account value, and swapping the definition is a one-line change that
    `predictable diff` classifies as a formula change to this component alone.
    """
    return num_pols_if * sum_assured / 1000.0


@series(timing=START)
def coverage_units_disc(coverage_units: Money, disc_factor: Factor) -> Money:
    """Coverage units discounted to `t = 0`, which is the basis the CSM is released on."""
    return coverage_units * retime(disc_factor, START)


@per_mp(output=True)
def total_coverage_units(coverage_units_disc: Money) -> Money:
    """The whole projection's discounted coverage units — the CSM release denominator."""
    return sum_(coverage_units_disc)


# -- back into stage 1: the balances, seeded through `init` ------------------------------


@series(timing=POINT, init=total_coverage_units)
def remaining_coverage_units(remaining_coverage_units: Money, coverage_units_disc: Money) -> Money:
    """Discounted coverage units still to be provided at the start of month `t`.

    Seeded from a stage-2 reduction through `init` — the one legal backward channel.
    Floored at zero so that the release fraction below can never exceed 1 through rounding.
    """
    return max_(remaining_coverage_units[t - 1] - coverage_units_disc[t - 1], 0.0)


@series(timing=START)
def release_fraction(coverage_units_disc: Money, remaining_coverage_units: Money) -> Factor:
    """The proportion of the remaining service provided in month `t`.

    `max(denominator, 1e-9)` rather than a branch: after the contract has run off both
    numerator and denominator are zero, and the guard keeps the quotient at zero instead of
    trapping.
    """
    return min_(coverage_units_disc / max_(remaining_coverage_units, 1e-9), 1.0)


@series(timing=POINT, init=csm_initial)
def csm(csm: Money, csm_release: Money, locked_in_rate_m: Num) -> Money:
    """Contractual service margin, accreted at the locked-in rate and released for service."""
    return max_((csm[t - 1] - csm_release[t - 1]) * (1 + locked_in_rate_m), 0.0)


@series(timing=START, output=True)
def csm_release(csm: Money, release_fraction: Factor) -> Money:
    """The CSM recognised in profit or loss in month `t`."""
    return csm * release_fraction


@series(timing=POINT, init=risk_adjustment)
def ra_balance(ra_balance: Money, ra_release: Money, locked_in_rate_m: Num) -> Money:
    """The unreleased risk adjustment, on the same accretion and release mechanics."""
    return max_((ra_balance[t - 1] - ra_release[t - 1]) * (1 + locked_in_rate_m), 0.0)


@series(timing=START, output=True)
def ra_release(ra_balance: Money, release_fraction: Factor) -> Money:
    """The risk adjustment recognised in profit or loss in month `t`."""
    return ra_balance * release_fraction


@series(timing=POINT, init=bel, output=True)
def fcf_balance(fcf_balance: Money, net_outflow: Money, locked_in_rate_m: Num) -> Money:
    """Retrospective roll-forward of the fulfilment cash flows.

    Identical in form to `term_annual`'s reserve: seeded from the prospective present value
    and rolled forward by removing each month's net outflow and accreting interest.
    """
    return (fcf_balance[t - 1] - net_outflow[t - 1]) * (1 + locked_in_rate_m)


# -- the reported balances ---------------------------------------------------------------


@series(timing=POINT, output=True)
def lrc(fcf_balance: Money, ra_balance: Money, csm: Money) -> Money:
    """Liability for remaining coverage: FCF + RA + CSM."""
    return fcf_balance + ra_balance + csm


@per_mp(output=True)
def lrc_at_issue(lrc: Money) -> Money:
    """`LRC` at `t = 0`.

    Zero for a profitable contract — IFRS 17 recognises no gain at initial recognition —
    and equal to the loss component for an onerous one. This is the model's own audit
    check, published as an output so a run can be reconciled without opening the model.
    """
    return first(lrc)


@series(timing=END, output=True)
def claims_incurred(
    death_claims: Money, surrender_claims: Money, maturity_claims: Money
) -> Money:
    """Claims incurred in month `t`, before any settlement lag."""
    return death_claims + surrender_claims + maturity_claims


@series(timing=POINT, init=0.0, output=True)
def lic(lic: Money, claims_incurred: Money) -> Money:
    """Liability for incurred claims, on a one-month settlement lag.

    Claims incurred at the end of month `t - 1` are carried at the start of month `t` and
    paid during it. One month is short, and deliberately so: it is the smallest lag that
    makes the LRC/LIC split a real split rather than a relabelling.
    """
    return claims_incurred[t - 1] * 1.0 + lic[t - 1] * 0.0


@series(timing=START, output=True)
def insurance_revenue(
    net_outflow: Money, ra_release: Money, csm_release: Money, premium_income: Money
) -> Money:
    """Insurance revenue: expected claims and expenses, plus the RA and CSM released."""
    return net_outflow + premium_income + ra_release + csm_release


@series(timing=START, output=True)
def insurance_service_expense(net_outflow: Money, premium_income: Money) -> Money:
    """Insurance service expense: the expected claims and expenses actually incurred."""
    return net_outflow + premium_income


@series(timing=START, output=True)
def insurance_service_result(
    insurance_revenue: Money, insurance_service_expense: Money
) -> Money:
    """Revenue less expense. On the central assumptions this is `RA release + CSM release`."""
    return insurance_revenue - insurance_service_expense


# -- level 2: present values of what the balances released ------------------------------


@per_mp(output=True)
def pv_csm_release(csm_release: Money, disc_factor: Factor) -> Money:
    """PV of the CSM recognised over the contract.

    A second stage-2 reduction over a series that was itself seeded by a stage-2 value.
    That is substage **level 2** (`03-engine.md` §5.3) and is the reason this model is in
    the corpus.
    """
    return npv(csm_release, disc_factor)


@per_mp(output=True)
def pv_ra_release(ra_release: Money, disc_factor: Factor) -> Money:
    """PV of the risk adjustment recognised over the contract. Also level 2."""
    return npv(ra_release, disc_factor)


@per_mp(output=True)
def pv_insurance_service_result(insurance_service_result: Money, disc_factor: Factor) -> Money:
    """PV of the insurance service result — the contract's recognised profit. Level 2."""
    return npv(insurance_service_result, disc_factor)


product(
    name="IFRS17_GMM",
    modules=["model"],
    outputs=[
        # the underlying savings product
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
        # the IFRS 17 measurement model
        "net_outflow",
        "risk_adjustment",
        "csm_initial",
        "loss_component_initial",
        "is_onerous",
        "coverage_units",
        "total_coverage_units",
        "csm_release",
        "ra_release",
        "fcf_balance",
        "lrc",
        "lrc_at_issue",
        "claims_incurred",
        "lic",
        "insurance_revenue",
        "insurance_service_expense",
        "insurance_service_result",
        "pv_csm_release",
        "pv_ra_release",
        "pv_insurance_service_result",
    ],
    key_field="policy_number",
    doc="IFRS 17 general measurement model on a unit-linked endowment — reference model 4.",
)
