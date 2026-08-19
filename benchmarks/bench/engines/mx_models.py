"""modelx formulas for the benchmark scenarios.

These are ordinary modelx cells: memoised functions of `t` inside a space parametrised by
the model point index, which is how lifelib's own projection models are written. Each
function below becomes one `Cells` object; the space-level references `MP`, `b`, `T` and
`tb` are injected by `modelx_impl.build`.

Written as module-level functions so modelx can capture their source — a modelx model is
meant to be readable in the model tree, and a benchmark implementation that a reader
cannot inspect is not a fair one.
"""

from __future__ import annotations

# The names below are resolved by modelx at call time from the space's references and
# sibling cells; the linter cannot see them, and that is expected.
# ruff: noqa: F821


# -- shared: model point access ----------------------------------------------------------


def mp():
    """The model point dict for this space's `pol` index."""
    return MP[pol]


# -- scenario 1 and 5: term_annual -------------------------------------------------------


def ta_in_force_factor(t):
    return 1.0 if t < mp()["policy_term"] else 0.0


def ta_qx(t):
    rate = tb.qx_table(mp()["sex"], mp()["smoker"], mp()["entry_age"] + t)
    return rate * b["mortality_loading"]


def ta_wx(t):
    return tb.lapse_pa(t + 1) * b["lapse_loading"]


def ta_num_pols_if(t):
    if t == 0:
        return 1.0
    return (
        ta_num_pols_if(t - 1)
        * (1 - ta_qx(t - 1))
        * (1 - ta_wx(t - 1))
        * ta_in_force_factor(t)
    )


def ta_deaths(t):
    return ta_num_pols_if(t) * ta_qx(t) * ta_in_force_factor(t)


def ta_surrenders(t):
    return ta_num_pols_if(t) * (1 - ta_qx(t)) * ta_wx(t) * ta_in_force_factor(t)


def ta_premium_rate(t):
    if t == 0:
        return premium()
    return ta_premium_rate(t - 1) * (1 + b["premium_escalation"])


def ta_premium_income(t):
    return ta_premium_rate(t) * ta_num_pols_if(t) * ta_in_force_factor(t)


def ta_death_claims(t):
    return mp()["sum_assured"] * ta_deaths(t)


def ta_renewal_expenses(t):
    return (
        b["renewal_expense_pa"]
        * (1 + b["expense_inflation"]) ** t
        * tb.expense_scale(mp()["expense_band"])
        * ta_num_pols_if(t)
        * ta_in_force_factor(t)
    )


def ta_net_cashflow(t):
    return ta_premium_income(t) - ta_death_claims(t) - ta_renewal_expenses(t)


def ta_disc_factor(t):
    if t == 0:
        return 1.0
    return ta_disc_factor(t - 1) / (1 + b["valuation_rate"])


def ta_pv_premiums():
    return sum(ta_premium_income(k) * ta_disc_factor(k) for k in range(T + 1))


def ta_pv_claims():
    total = 0.0
    for k in range(T + 1):
        d = ta_disc_factor(k)
        nxt = ta_disc_factor(k + 1) if k < T else d
        total += ta_death_claims(k) * (nxt if d else 0.0)
    return total


def ta_pv_expenses():
    return sum(ta_renewal_expenses(k) * ta_disc_factor(k) for k in range(T + 1))


def ta_initial_expense():
    return b["initial_expense_pct"] * premium()


def ta_bel():
    return ta_pv_claims() + ta_pv_expenses() + ta_initial_expense() - ta_pv_premiums()


def ta_profit_margin():
    return -ta_bel() / ta_pv_premiums()


def ta_reserve(t):
    if t == 0:
        return ta_bel()
    prior = (
        ta_reserve(t - 1)
        + ta_premium_income(t - 1)
        - ta_death_claims(t - 1)
        - ta_renewal_expenses(t - 1)
    )
    return prior * (1 + b["valuation_rate"]) * (1.0 if t <= mp()["policy_term"] else 0.0)


# -- scenario 2: term_monthly ------------------------------------------------------------


def tm_in_force_factor(t):
    return 1.0 if t < mp()["policy_term"] * 12 else 0.0


def tm_qx(t):
    rate = tb.qx_table(mp()["sex"], mp()["smoker"], mp()["entry_age"] + t // 12)
    return 1 - (1 - rate * b["mortality_loading"]) ** (1 / 12)


def tm_wx(t):
    return 1 - (1 - tb.lapse_pa(t // 12 + 1) * b["lapse_loading"]) ** (1 / 12)


def tm_num_pols_if(t):
    if t == 0:
        return 1.0
    return (
        tm_num_pols_if(t - 1)
        * (1 - tm_qx(t - 1))
        * (1 - tm_wx(t - 1))
        * tm_in_force_factor(t)
    )


def tm_deaths(t):
    return tm_num_pols_if(t) * tm_qx(t) * tm_in_force_factor(t)


def tm_surrenders(t):
    return tm_num_pols_if(t) * (1 - tm_qx(t)) * tm_wx(t) * tm_in_force_factor(t)


def tm_premium_income(t):
    return mp()["annual_premium"] / 12 * tm_num_pols_if(t) * tm_in_force_factor(t)


def tm_death_claims(t):
    return mp()["sum_assured"] * tm_deaths(t)


def tm_renewal_expenses(t):
    infl_m = (1 + b["expense_inflation"]) ** (1 / 12) - 1
    return (
        b["renewal_expense_pa"]
        / 12
        * (1 + infl_m) ** t
        * tb.expense_scale(mp()["expense_band"])
        * tm_num_pols_if(t)
        * tm_in_force_factor(t)
    )


def tm_net_cashflow(t):
    return tm_premium_income(t) - tm_death_claims(t) - tm_renewal_expenses(t)


def tm_disc_factor(t):
    if t == 0:
        return 1.0
    return tm_disc_factor(t - 1) / (1 + (1 + b["valuation_rate"]) ** (1 / 12) - 1)


def tm_pv_premiums():
    return sum(tm_premium_income(k) * tm_disc_factor(k) for k in range(T + 1))


def tm_pv_claims():
    total = 0.0
    for k in range(T + 1):
        d = tm_disc_factor(k)
        nxt = tm_disc_factor(k + 1) if k < T else d
        total += tm_death_claims(k) * (nxt if d else 0.0)
    return total


def tm_pv_expenses():
    return sum(tm_renewal_expenses(k) * tm_disc_factor(k) for k in range(T + 1))


def tm_initial_expense():
    return b["initial_expense_pct"] * mp()["annual_premium"]


def tm_bel():
    return tm_pv_claims() + tm_pv_expenses() + tm_initial_expense() - tm_pv_premiums()


def tm_profit_margin():
    return -tm_bel() / tm_pv_premiums()


def tm_reserve(t):
    if t == 0:
        return tm_bel()
    prior = (
        tm_reserve(t - 1)
        + tm_premium_income(t - 1)
        - tm_death_claims(t - 1)
        - tm_renewal_expenses(t - 1)
    )
    i_m = (1 + b["valuation_rate"]) ** (1 / 12) - 1
    return prior * (1 + i_m) * (1.0 if t <= mp()["policy_term"] * 12 else 0.0)


# -- scenario 3: savings_monthly ---------------------------------------------------------


def sv_in_force_factor(t):
    return 1.0 if t < mp()["policy_term"] * 12 else 0.0


def sv_is_maturity(t):
    return 1.0 if t == mp()["policy_term"] * 12 - 1 else 0.0


def sv_qx(t):
    rate = tb.qx_table(mp()["sex"], mp()["smoker"], mp()["entry_age"] + t // 12)
    return 1 - (1 - rate * b["mortality_loading"]) ** (1 / 12)


def sv_wx(t):
    if sv_is_maturity(t):
        return 0.0
    return 1 - (1 - tb.lapse_pa(t // 12 + 1) * b["lapse_loading"]) ** (1 / 12)


def sv_num_pols_if(t):
    if t == 0:
        return 1.0
    return (
        sv_num_pols_if(t - 1)
        * (1 - sv_qx(t - 1))
        * (1 - sv_wx(t - 1))
        * sv_in_force_factor(t)
    )


def sv_deaths(t):
    return sv_num_pols_if(t) * sv_qx(t) * sv_in_force_factor(t)


def sv_surrenders(t):
    return sv_num_pols_if(t) * (1 - sv_qx(t)) * sv_wx(t) * sv_in_force_factor(t)


def sv_maturities(t):
    return sv_num_pols_if(t) * (1 - sv_qx(t)) * (1 - sv_wx(t)) * sv_is_maturity(t)


def sv_allocated_premium(t):
    rate = b["alloc_rate_year1"] if t // 12 + 1 == 1 else b["alloc_rate_renewal"]
    return mp()["monthly_premium"] * rate * sv_in_force_factor(t)


def sv_account_value(t):
    if t == 0:
        return mp()["initial_account"]
    credit_m = (1 + b["credit_rate"]) ** (1 / 12) - 1
    prior = sv_account_value(t - 1) + sv_allocated_premium(t - 1) - sv_policy_charges(t - 1)
    return prior * (1 + credit_m) * sv_in_force_factor(t)


def sv_cost_of_insurance(t):
    sar = mp()["sum_assured"] - sv_account_value(t)
    return (sar if sar > 0.0 else 0.0) * sv_qx(t) * b["coi_loading"]


def sv_management_charge(t):
    amc_m = 1 - (1 - b["amc_pa"]) ** (1 / 12)
    return sv_account_value(t) * amc_m


def sv_policy_fee(t):
    return b["policy_fee_pm"] * (1 + b["expense_inflation"]) ** (t // 12)


def sv_policy_charges(t):
    charges = sv_cost_of_insurance(t) + sv_management_charge(t) + sv_policy_fee(t)
    return charges * sv_in_force_factor(t)


def sv_premiums_paid(t):
    if t == 0:
        return mp()["monthly_premium"]
    return sv_premiums_paid(t - 1) + mp()["monthly_premium"] * sv_in_force_factor(t)


def sv_death_claims(t):
    av = sv_account_value(t)
    sa = mp()["sum_assured"]
    return (sa if sa > av else av) * sv_deaths(t)


def sv_surrender_claims(t):
    ben = sv_account_value(t) * (1 - tb.surrender_penalty(t // 12 + 1))
    return (ben if ben > 0.0 else 0.0) * sv_surrenders(t)


def sv_maturity_benefit(t):
    av = sv_account_value(t)
    guarantee = sv_premiums_paid(t) * b["guarantee_pct"]
    return av if av > guarantee else guarantee


def sv_maturity_claims(t):
    return sv_maturity_benefit(t) * sv_maturities(t)


def sv_guarantee_cost(t):
    return (sv_maturity_benefit(t) - sv_account_value(t)) * sv_maturities(t)


def sv_account_at_maturity(t):
    return sv_account_value(t) * sv_maturities(t)


def sv_premium_income(t):
    return mp()["monthly_premium"] * sv_num_pols_if(t) * sv_in_force_factor(t)


def sv_renewal_expenses(t):
    return (
        b["renewal_expense_pa"]
        / 12
        * (1 + b["expense_inflation"]) ** (t // 12)
        * tb.expense_scale(mp()["expense_band"])
        * sv_num_pols_if(t)
        * sv_in_force_factor(t)
    )


def sv_net_cashflow(t):
    return (
        sv_premium_income(t)
        - sv_death_claims(t)
        - sv_surrender_claims(t)
        - sv_maturity_claims(t)
        - sv_renewal_expenses(t)
    )


def sv_disc_factor(t):
    return (1 + tb.spot(t // 12 + 1)) ** (-t / 12.0)


def sv_pv_premiums():
    return sum(sv_premium_income(k) * sv_disc_factor(k) for k in range(T + 1))


def sv_pv_death_claims():
    total = 0.0
    for k in range(T + 1):
        d = sv_disc_factor(k)
        nxt = sv_disc_factor(k + 1) if k < T else d
        total += sv_death_claims(k) * (nxt if d else 0.0)
    return total


def sv_pv_surrender_claims():
    total = 0.0
    for k in range(T + 1):
        d = sv_disc_factor(k)
        nxt = sv_disc_factor(k + 1) if k < T else d
        total += sv_surrender_claims(k) * (nxt if d else 0.0)
    return total


def sv_pv_maturity_claims():
    total = 0.0
    for k in range(T + 1):
        d = sv_disc_factor(k)
        nxt = sv_disc_factor(k + 1) if k < T else d
        total += sv_maturity_claims(k) * (nxt if d else 0.0)
    return total


def sv_pv_expenses():
    return sum(sv_renewal_expenses(k) * sv_disc_factor(k) for k in range(T + 1))


def sv_pv_guarantee_cost():
    total = 0.0
    for k in range(T + 1):
        d = sv_disc_factor(k)
        nxt = sv_disc_factor(k + 1) if k < T else d
        total += sv_guarantee_cost(k) * (nxt if d else 0.0)
    return total


def sv_initial_expense():
    return b["initial_expense_pct"] * mp()["monthly_premium"] * 12


def sv_bel():
    return (
        sv_pv_death_claims()
        + sv_pv_surrender_claims()
        + sv_pv_maturity_claims()
        + sv_pv_expenses()
        + sv_initial_expense()
        - sv_pv_premiums()
    )


def sv_final_account_value():
    return sum(sv_account_at_maturity(k) for k in range(T + 1))


def sv_profit_margin():
    return -sv_bel() / sv_pv_premiums()
