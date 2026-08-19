"""cashflower model — scenario 1, `term_annual`.

A plain cashflower model module: module-level `@variable()` functions of `t` reading the
current model point from the module-level `main`, which the driver sets. cashflower builds
its dependency graph by parsing each formula's *source*, so the variables must live at
module level and every dependency must appear as a literal call by name.

The present values are `@variable(array=True)` rather than functions of `t`, because
cashflower rejects a call like `disc_factor(k)` with a loop variable — the first argument
of a variable may only be `t`, `t ± integer`, or a literal. An array variable sees the
whole projection at once, which is the framework's own answer to a whole-projection
reduction, and is what a cashflower user would write here.

`premium_of` is the one hook the driver moves: `term_solve` (scenario 5) re-runs this same
model with a bisected premium, which is exactly how a scalar engine has to do an outer
solve.
"""

from __future__ import annotations

import numpy as np
from cashflower.core import variable

from ...basis import TERM_BASIS as b
from .. import scalar_basis as tb

main = None
T = 40


def premium_of(m):
    return m.get("annual_premium")


@variable()
def in_force_factor(t):
    return 1.0 if t < main.get("policy_term") else 0.0


@variable()
def qx(t):
    rate = tb.qx_table(main.get("sex"), main.get("smoker"), main.get("entry_age") + t)
    return rate * b["mortality_loading"]


@variable()
def wx(t):
    return tb.lapse_pa(t + 1) * b["lapse_loading"]


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
def premium_rate(t):
    if t == 0:
        return premium_of(main)
    return premium_rate(t - 1) * (1 + b["premium_escalation"])


@variable()
def premium_income(t):
    return premium_rate(t) * num_pols_if(t) * in_force_factor(t)


@variable()
def death_claims(t):
    return main.get("sum_assured") * deaths(t)


@variable()
def renewal_expenses(t):
    return (
        b["renewal_expense_pa"]
        * (1 + b["expense_inflation"]) ** t
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
    return disc_factor(t - 1) / (1 + b["valuation_rate"])


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
    return b["initial_expense_pct"] * premium_of(main)


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
    return prior * (1 + b["valuation_rate"]) * (1.0 if t <= main.get("policy_term") else 0.0)
