"""cashflower model — scenario 3, `savings_monthly`: the unit-linked endowment.

The structural workout: a rolling account value, four table lookups a month, and benefits
defined by `max`/`min` on the fund. In a scalar engine every one of those is a Python
branch per policy per month, which is precisely the cost this scenario is here to expose.
"""

from __future__ import annotations

import numpy as np
from cashflower.core import variable

from ...basis import SAVINGS_BASIS as b
from .. import scalar_basis as tb

main = None
T = 360

CREDIT_M = (1 + b["credit_rate"]) ** (1 / 12) - 1
AMC_M = 1 - (1 - b["amc_pa"]) ** (1 / 12)


@variable()
def in_force_factor(t):
    return 1.0 if t < main.get("policy_term") * 12 else 0.0


@variable()
def is_maturity(t):
    return 1.0 if t == main.get("policy_term") * 12 - 1 else 0.0


@variable()
def qx(t):
    rate = tb.qx_table(main.get("sex"), main.get("smoker"), main.get("entry_age") + t // 12)
    return 1 - (1 - rate * b["mortality_loading"]) ** (1 / 12)


@variable()
def wx(t):
    if is_maturity(t):
        return 0.0
    return 1 - (1 - tb.lapse_pa(t // 12 + 1) * b["lapse_loading"]) ** (1 / 12)


@variable()
def num_pols_if(t):
    if t == 0:
        return 1.0
    return num_pols_if(t - 1) * (1 - qx(t - 1)) * (1 - wx(t - 1)) * in_force_factor(t)


@variable()
def deaths(t):
    return num_pols_if(t) * qx(t) * in_force_factor(t)


@variable()
def surrenders(t):
    return num_pols_if(t) * (1 - qx(t)) * wx(t) * in_force_factor(t)


@variable()
def maturities(t):
    return num_pols_if(t) * (1 - qx(t)) * (1 - wx(t)) * is_maturity(t)


@variable()
def allocated_premium(t):
    rate = b["alloc_rate_year1"] if t // 12 + 1 == 1 else b["alloc_rate_renewal"]
    return main.get("monthly_premium") * rate * in_force_factor(t)


@variable()
def account_value(t):
    if t == 0:
        return main.get("initial_account")
    prior = account_value(t - 1) + allocated_premium(t - 1) - policy_charges(t - 1)
    return prior * (1 + CREDIT_M) * in_force_factor(t)


@variable()
def cost_of_insurance(t):
    sar = main.get("sum_assured") - account_value(t)
    return (sar if sar > 0.0 else 0.0) * qx(t) * b["coi_loading"]


@variable()
def management_charge(t):
    return account_value(t) * AMC_M


@variable()
def policy_fee(t):
    return b["policy_fee_pm"] * (1 + b["expense_inflation"]) ** (t // 12)


@variable()
def policy_charges(t):
    return (cost_of_insurance(t) + management_charge(t) + policy_fee(t)) * in_force_factor(t)


@variable()
def premiums_paid(t):
    if t == 0:
        return main.get("monthly_premium")
    return premiums_paid(t - 1) + main.get("monthly_premium") * in_force_factor(t)


@variable()
def death_claims(t):
    av = account_value(t)
    sa = main.get("sum_assured")
    return (sa if sa > av else av) * deaths(t)


@variable()
def surrender_claims(t):
    ben = account_value(t) * (1 - tb.surrender_penalty(t // 12 + 1))
    return (ben if ben > 0.0 else 0.0) * surrenders(t)


@variable()
def maturity_benefit(t):
    av = account_value(t)
    guarantee = premiums_paid(t) * b["guarantee_pct"]
    return av if av > guarantee else guarantee


@variable()
def maturity_claims(t):
    return maturity_benefit(t) * maturities(t)


@variable()
def guarantee_cost(t):
    return (maturity_benefit(t) - account_value(t)) * maturities(t)


@variable()
def account_at_maturity(t):
    return account_value(t) * maturities(t)


@variable()
def premium_income(t):
    return main.get("monthly_premium") * num_pols_if(t) * in_force_factor(t)


@variable()
def renewal_expenses(t):
    return (
        b["renewal_expense_pa"]
        / 12
        * (1 + b["expense_inflation"]) ** (t // 12)
        * tb.expense_scale(main.get("expense_band"))
        * num_pols_if(t)
        * in_force_factor(t)
    )


@variable()
def net_cashflow(t):
    return (
        premium_income(t)
        - death_claims(t)
        - surrender_claims(t)
        - maturity_claims(t)
        - renewal_expenses(t)
    )


@variable()
def disc_factor(t):
    return (1 + tb.spot(t // 12 + 1)) ** (-t / 12.0)


@variable(array=True)
def pv_premiums():
    return np.full(T + 1, tb.pv_start(premium_income(), disc_factor()))


@variable(array=True)
def pv_death_claims():
    return np.full(T + 1, tb.pv_end(death_claims(), disc_factor()))


@variable(array=True)
def pv_surrender_claims():
    return np.full(T + 1, tb.pv_end(surrender_claims(), disc_factor()))


@variable(array=True)
def pv_maturity_claims():
    return np.full(T + 1, tb.pv_end(maturity_claims(), disc_factor()))


@variable(array=True)
def pv_expenses():
    return np.full(T + 1, tb.pv_start(renewal_expenses(), disc_factor()))


@variable(array=True)
def pv_guarantee_cost():
    return np.full(T + 1, tb.pv_end(guarantee_cost(), disc_factor()))


@variable()
def initial_expense(t):
    return b["initial_expense_pct"] * main.get("monthly_premium") * 12


@variable()
def bel(t):
    return (
        pv_death_claims(t)
        + pv_surrender_claims(t)
        + pv_maturity_claims(t)
        + pv_expenses(t)
        + initial_expense(t)
        - pv_premiums(t)
    )


@variable(array=True)
def final_account_value():
    return np.full(T + 1, float(np.sum(account_at_maturity())))


@variable()
def profit_margin(t):
    return -bel(t) / pv_premiums(t)
