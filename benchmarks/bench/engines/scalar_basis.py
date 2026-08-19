"""Scalar (one-policy-at-a-time) versions of the table lookups.

cashflower and modelx are both scalar engines: a formula sees one model point and one `t`.
They share these lookups so that a disagreement between them is a disagreement about the
*model*, never about the tables.
"""

from __future__ import annotations

import numpy as np

from ..basis import (
    EXPENSE_BAND,
    MAX_AGE,
    MIN_AGE,
    lapse_table,
    mortality_table,
    surrender_table,
    yield_table,
)


def qx_table(sex: str, smoker: bool, age: int) -> float:
    """`clamp` on age (`01-ir.md` key policy), exact on sex and smoker."""
    col = mortality_table()[(sex, bool(smoker))]
    return col[min(max(int(age), MIN_AGE), MAX_AGE) - MIN_AGE]


def lapse_pa(policy_year: int) -> float:
    tbl = lapse_table()
    return tbl[min(max(int(policy_year), 1), len(tbl) - 1)]


def surrender_penalty(policy_year: int) -> float:
    tbl = surrender_table()
    return tbl[min(max(int(policy_year), 1), len(tbl) - 1)]


def spot(term: int) -> float:
    tbl = yield_table()
    return tbl[min(max(int(term), 0), len(tbl) - 1)]


def expense_scale(band: int) -> float:
    return EXPENSE_BAND[int(band)]


# -- whole-projection reductions, for engines that expose a variable's full array ---------


def pv_start(flow, disc) -> float:
    """`npv` of a `start`- or `point`-timed flow: `sum(x[t] * disc[t])`."""
    return float((np.asarray(flow) * np.asarray(disc)).sum())


def pv_end(flow, disc) -> float:
    """`npv` of an `end`-timed flow: the factor is the curve's own next step, `disc[t+1]`."""
    d = np.asarray(disc)
    nxt = np.concatenate([d[1:], d[-1:]])
    return float((np.asarray(flow) * np.where(d == 0.0, 0.0, nxt)).sum())
