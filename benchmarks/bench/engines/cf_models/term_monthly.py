"""cashflower model — scenario 2, `term_monthly`: the same product, 480 monthly periods."""

from __future__ import annotations

import numpy as np
from cashflower.core import variable

from ...basis import TERM_BASIS as b
from .. import scalar_basis as tb

main = None
T = 480

I_M = (1 + b["valuation_rate"]) ** (1 / 12) - 1
INFL_M = (1 + b["expense_inflation"]) ** (1 / 12) - 1


@variable()
def in_force_factor(t):
    return 1.0 if t < main.get("policy_term") * 12 else 0.0


@variable()
def qx(t):
    rate = tb.qx_table(main.get("sex"), main.get("smoker"), main.get("entry_age") + t // 12)
    return 1 - (1 - rate * b["mortality_loading"]) ** (1 / 12)


@variable()
def wx(t):
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
def premium_income(t):
    return main.get("annual_premium") / 12 * num_pols_if(t) * in_force_factor(t)


@variable()
def death_claims(t):
    return main.get("sum_assured") * deaths(t)


@variable()
def renewal_expenses(t):
    return (
        b["renewal_expense_pa"]
        / 12
        * (1 + INFL_M) ** t
        * tb.expense_scale(main.get("expense_band"))
        * num_pols_if(t)
        * in_force_factor(t)
    )


@variable()
def net_cashflow(t):
    return premium_income(t) - death_claims(t) - renewal_expenses(t)


@variable()
def disc_factor(t):
    if t == 0:
        return 1.0
    return disc_factor(t - 1) / (1 + I_M)


@variable(array=True)
def pv_premiums():
    return np.full(T + 1, tb.pv_start(premium_income(), disc_factor()))


@variable(array=True)
def pv_claims():
    return np.full(T + 1, tb.pv_end(death_claims(), disc_factor()))


@variable(array=True)
def pv_expenses():
    return np.full(T + 1, tb.pv_start(renewal_expenses(), disc_factor()))


@variable()
def initial_expense(t):
    return b["initial_expense_pct"] * main.get("annual_premium")


@variable()
def bel(t):
    return pv_claims(t) + pv_expenses(t) + initial_expense(t) - pv_premiums(t)


@variable()
def profit_margin(t):
    return -bel(t) / pv_premiums(t)


@variable()
def reserve(t):
    if t == 0:
        return bel(0)
    prior = reserve(t - 1) + premium_income(t - 1) - death_claims(t - 1) - renewal_expenses(t - 1)
    return prior * (1 + I_M) * (1.0 if t <= main.get("policy_term") * 12 else 0.0)
