"""The NumPy reference implementation — "how fast could you do this yourself?".

This is a hand-written, fully vectorised implementation of the five scenarios: one
`float64` array per component, shaped `(M,)` for a `PerMP` and `(M,)` per period inside a
Python loop over `t` for a `Series`. It is the honest floor of the benchmark. It is also
the *semantic* reference for the gate: it re-implements the `01-ir.md` timing and
discounting rules explicitly rather than borrowing them from the engine.

Two rules are transcribed here rather than assumed, because getting them wrong is the
usual source of a cross-engine disagreement:

`npv(x, disc)` (`01-ir.md` §2.5, `predictable-engine::reduce`)
    `disc` is a cumulative discount factor: `disc[t]` discounts a flow at the *start* of
    period `t`. A `start` or `point` flow uses `disc[t]`; an `end` or `mid` flow needs the
    fractional step, taken from the curve's own one-period factor
    `f = disc[t+1] / disc[t]`, as `disc[t] * f ** frac` with `frac = 1.0` / `0.5`. At the
    last period there is no `t + 1`, so `f = 1`.

recursion
    A `Series` runs `t = 0 .. T` inclusive — `T + 1` values, not `T`. `init` supplies
    `t = 0`; every later period reads lag-1 values only.
"""

from __future__ import annotations

import numpy as np

from ..basis import (
    EXPENSE_BAND,
    IFRS17_BASIS,
    MAX_AGE,
    MIN_AGE,
    SAVINGS_BASIS,
    TERM_BASIS,
    lapse_table,
    mortality_table,
    surrender_table,
    yield_table,
)
from ..outputs import Outputs

NAME = "numpy"


# -- shared helpers ----------------------------------------------------------------------


def _npv(series: list[np.ndarray], disc: list[np.ndarray], frac: float) -> np.ndarray:
    """Sequential, left-to-right in `t` — no pairwise reassociation (`01-ir.md` §9.2)."""
    t_max = len(series) - 1
    acc = np.zeros_like(series[0])
    for t in range(t_max + 1):
        d = disc[t]
        if frac == 0.0:
            f = d
        else:
            nxt = disc[t + 1] if t < t_max else d
            step = np.where(d == 0.0, 1.0, nxt / np.where(d == 0.0, 1.0, d))
            f = d * step**frac
        acc = acc + series[t] * f
    return acc


#: The fractional discount exponent implied by each timing tag (`01-ir.md` §2.5).
FRAC = {"start": 0.0, "point": 0.0, "end": 1.0, "mid": 0.5}

#: `models/term_solve/run.pir` sets `max_iter = 60`. The scalar engines use the same budget.
SOLVE_MAX_ITER = 60
SOLVE_BRACKET = (1.0, 100_000.0)


def _mortality_lookup(sex: np.ndarray, smoker: np.ndarray, age: np.ndarray) -> np.ndarray:
    """`clamp` on age, exact match on sex and smoker."""
    table = mortality_table()
    idx = np.clip(age, MIN_AGE, MAX_AGE).astype(np.int64) - MIN_AGE
    out = np.empty(len(age), dtype=np.float64)
    for (s, sm), col in table.items():
        mask = (sex == s) & (smoker == sm)
        if mask.any():
            out[mask] = np.asarray(col, dtype=np.float64)[idx[mask]]
    return out


def _step_lookup(table: list[float], keys: np.ndarray) -> np.ndarray:
    """`step`: the value of the greatest key at or below the lookup, clamped at both ends."""
    arr = np.asarray(table, dtype=np.float64)
    return arr[np.clip(keys, 1, len(arr) - 1).astype(np.int64)]


def _stack(rows: list[np.ndarray]) -> np.ndarray:
    return np.stack(rows, axis=1)


def _mp_arrays(mps: list[dict], fields: dict[str, type]) -> dict[str, np.ndarray]:
    out = {}
    for name, kind in fields.items():
        col = [mp[name] for mp in mps]
        if kind is float:
            out[name] = np.asarray(col, dtype=np.float64)
        elif kind is int:
            out[name] = np.asarray([int(v) for v in col], dtype=np.int64)
        elif kind is bool:
            out[name] = np.asarray([v == "true" or v is True for v in col])
        else:
            out[name] = np.asarray(col, dtype=object)
    return out


TERM_FIELDS = {
    "policy_number": str,
    "entry_age": int,
    "sex": str,
    "smoker": bool,
    "sum_assured": float,
    "annual_premium": float,
    "policy_term": int,
    "expense_band": int,
}

SAVINGS_FIELDS = {
    "policy_number": str,
    "entry_age": int,
    "sex": str,
    "smoker": bool,
    "monthly_premium": float,
    "policy_term": int,
    "initial_account": float,
    "sum_assured": float,
    "expense_band": int,
    "cohort": str,
}


# -- scenario 1: term_annual -------------------------------------------------------------


def term_annual(mps: list[dict], *, collect_series: bool, premium=None) -> Outputs:
    b = TERM_BASIS
    T = 40
    mp = _mp_arrays(mps, TERM_FIELDS)
    M = len(mps)
    sa, term = mp["sum_assured"], mp["policy_term"].astype(np.float64)
    prem0 = mp["annual_premium"] if premium is None else premium
    scale = np.asarray([EXPENSE_BAND[int(b_)] for b_ in mp["expense_band"]], dtype=np.float64)

    qx, wx, in_force, npif = [], [], [], []
    prem_rate, prem_inc, deaths, surrenders = [], [], [], []
    death_claims, renewal, disc = [], [], []

    n = np.ones(M)
    p = prem0.copy()
    d = np.ones(M)
    for t in range(T + 1):
        age = mp["entry_age"] + t
        it = (t < term).astype(np.float64)
        q = _mortality_lookup(mp["sex"], mp["smoker"], age) * b["mortality_loading"]
        w = _step_lookup(lapse_table(), np.full(M, t + 1)) * b["lapse_loading"]
        if t > 0:
            n = npif[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * it
            p = prem_rate[t - 1] * (1 + b["premium_escalation"])
            d = disc[t - 1] / (1 + b["valuation_rate"])
        qx.append(q)
        wx.append(w)
        in_force.append(it)
        npif.append(n)
        prem_rate.append(p)
        disc.append(d)
        deaths.append(n * q * it)
        surrenders.append(n * (1 - q) * w * it)
        prem_inc.append(p * n * it)
        death_claims.append(sa * deaths[t])
        renewal.append(
            b["renewal_expense_pa"] * (1 + b["expense_inflation"]) ** t * scale * n * it
        )

    net_cf = [prem_inc[t] - death_claims[t] - renewal[t] for t in range(T + 1)]
    pv_premiums = _npv(prem_inc, disc, FRAC["start"])
    pv_claims = _npv(death_claims, disc, FRAC["end"])
    pv_expenses = _npv(renewal, disc, FRAC["start"])
    initial_expense = np.full(M, b["initial_expense_pct"]) * prem0
    bel = pv_claims + pv_expenses + initial_expense - pv_premiums

    reserve = [bel]
    for t in range(1, T + 1):
        reserve.append(
            (reserve[t - 1] + prem_inc[t - 1] - death_claims[t - 1] - renewal[t - 1])
            * (1 + b["valuation_rate"])
            * (t <= term)
        )

    out = Outputs(keys=[m["policy_number"] for m in mps])
    out.per_mp = {
        "pv_premiums": pv_premiums,
        "pv_claims": pv_claims,
        "pv_expenses": pv_expenses,
        "initial_expense": initial_expense,
        "bel": bel,
        "profit_margin": -bel / pv_premiums,
    }
    if collect_series:
        out.series = {
            "deaths": _stack(deaths),
            "surrenders": _stack(surrenders),
            "premium_income": _stack(prem_inc),
            "death_claims": _stack(death_claims),
            "renewal_expenses": _stack(renewal),
            "net_cashflow": _stack(net_cf),
            "reserve": _stack(reserve),
        }
    return out


# -- scenario 5: term_solve --------------------------------------------------------------


def term_solve(mps: list[dict], *, collect_series: bool) -> Outputs:
    """`term_annual` with the office premium solved to `bel = 0`, per modelpoint.

    predictable uses Brent; this — and the cashflower and modelx implementations — use
    bisection on the same bracket, `[1, 100000]`, with the same iteration cap. The residual
    is strictly monotone decreasing in the premium, so all four converge on the same root
    and the gate compares roots, not iteration paths. The *timing* of this scenario is
    therefore not a like-for-like solver comparison and is reported as such: Brent reaches
    the root in far fewer model evaluations than bisection does.
    """
    lo = np.full(len(mps), SOLVE_BRACKET[0])
    hi = np.full(len(mps), SOLVE_BRACKET[1])

    def residual(p):
        return term_annual(mps, collect_series=False, premium=p).per_mp["bel"]

    f_lo = residual(lo)
    for _ in range(SOLVE_MAX_ITER):
        mid = 0.5 * (lo + hi)
        f_mid = residual(mid)
        same = np.sign(f_mid) == np.sign(f_lo)
        lo = np.where(same, mid, lo)
        f_lo = np.where(same, f_mid, f_lo)
        hi = np.where(same, hi, mid)
        if np.max(hi - lo) < 1e-10:
            break
    solved = 0.5 * (lo + hi)
    out = term_annual(mps, collect_series=collect_series, premium=solved)
    out.per_mp["breakeven_premium"] = solved
    return out


# -- scenario 2: term_monthly ------------------------------------------------------------


def term_monthly(mps: list[dict], *, collect_series: bool) -> Outputs:
    b = TERM_BASIS
    T = 480
    mp = _mp_arrays(mps, TERM_FIELDS)
    M = len(mps)
    sa = mp["sum_assured"]
    term_m = mp["policy_term"].astype(np.float64) * 12
    scale = np.asarray([EXPENSE_BAND[int(x)] for x in mp["expense_band"]], dtype=np.float64)
    monthly_prem = mp["annual_premium"] / 12
    i_m = (1 + b["valuation_rate"]) ** (1 / 12) - 1
    infl_m = (1 + b["expense_inflation"]) ** (1 / 12) - 1

    qx, wx, npif, disc = [], [], [], []
    deaths, surrenders, prem_inc, death_claims, renewal = [], [], [], [], []
    n = np.ones(M)
    d = np.ones(M)
    for t in range(T + 1):
        py = t // 12 + 1
        age = mp["entry_age"] + py - 1
        it = (t < term_m).astype(np.float64)
        q = 1 - (1 - _mortality_lookup(mp["sex"], mp["smoker"], age) * b["mortality_loading"]) ** (
            1 / 12
        )
        w = 1 - (
            1 - _step_lookup(lapse_table(), np.full(M, py)) * b["lapse_loading"]
        ) ** (1 / 12)
        if t > 0:
            n = npif[t - 1] * (1 - qx[t - 1]) * (1 - wx[t - 1]) * it
            d = disc[t - 1] / (1 + i_m)
        qx.append(q)
        wx.append(w)
        npif.append(n)
        disc.append(d)
        deaths.append(n * q * it)
        surrenders.append(n * (1 - q) * w * it)
        prem_inc.append(monthly_prem * n * it)
        death_claims.append(sa * deaths[t])
        renewal.append(b["renewal_expense_pa"] / 12 * (1 + infl_m) ** t * scale * n * it)

    net_cf = [prem_inc[t] - death_claims[t] - renewal[t] for t in range(T + 1)]
    pv_premiums = _npv(prem_inc, disc, FRAC["start"])
    pv_claims = _npv(death_claims, disc, FRAC["end"])
    pv_expenses = _npv(renewal, disc, FRAC["start"])
    initial_expense = b["initial_expense_pct"] * mp["annual_premium"]
    bel = pv_claims + pv_expenses + initial_expense - pv_premiums

    reserve = [bel]
    for t in range(1, T + 1):
        reserve.append(
            (reserve[t - 1] + prem_inc[t - 1] - death_claims[t - 1] - renewal[t - 1])
            * (1 + i_m)
            * (t <= term_m)
        )

    out = Outputs(keys=[m["policy_number"] for m in mps])
    out.per_mp = {
        "pv_premiums": pv_premiums,
        "pv_claims": pv_claims,
        "pv_expenses": pv_expenses,
        "initial_expense": initial_expense,
        "bel": bel,
        "profit_margin": -bel / pv_premiums,
    }
    if collect_series:
        out.series = {
            "deaths": _stack(deaths),
            "surrenders": _stack(surrenders),
            "premium_income": _stack(prem_inc),
            "death_claims": _stack(death_claims),
            "renewal_expenses": _stack(renewal),
            "net_cashflow": _stack(net_cf),
            "reserve": _stack(reserve),
        }
    return out


# -- scenarios 3 and 4: savings_monthly, and the IFRS 17 model laid on top ----------------


def _savings_core(mps: list[dict], basis: dict):
    """The fulfilment cash flows shared by `savings_monthly` and `ifrs17_gmm`."""
    b = basis
    T = 360
    mp = _mp_arrays(mps, SAVINGS_FIELDS)
    M = len(mps)
    sa = mp["sum_assured"]
    term_m = mp["policy_term"].astype(np.float64) * 12
    prem = mp["monthly_premium"]
    scale = np.asarray([EXPENSE_BAND[int(x)] for x in mp["expense_band"]], dtype=np.float64)
    credit_m = (1 + b["credit_rate"]) ** (1 / 12) - 1
    amc_m = 1 - (1 - b["amc_pa"]) ** (1 / 12)
    yc = np.asarray(yield_table(), dtype=np.float64)

    s: dict[str, list[np.ndarray]] = {
        k: []
        for k in (
            "qx", "wx", "npif", "disc", "av", "prem_paid", "in_force", "charges",
            "deaths", "surrenders", "maturities", "alloc_prem", "coi", "amc", "fee",
            "death_claims", "surrender_claims", "maturity_claims", "guarantee_cost",
            "premium_income", "renewal", "acct_at_mat",
        )
    }
    av = mp["initial_account"].astype(np.float64).copy()
    pp = prem.copy()
    n = np.ones(M)
    for t in range(T + 1):
        py = t // 12 + 1
        age = mp["entry_age"] + py - 1
        it = (t < term_m).astype(np.float64)
        is_mat = (t == term_m - 1).astype(np.float64)
        qa = _mortality_lookup(mp["sex"], mp["smoker"], age) * b["mortality_loading"]
        q = 1 - (1 - qa) ** (1 / 12)
        wa = _step_lookup(lapse_table(), np.full(M, py)) * b["lapse_loading"]
        w = np.where(is_mat > 0, 0.0, 1 - (1 - wa) ** (1 / 12))
        spot = yc[min(py, len(yc) - 1)]
        if t > 0:
            n = s["npif"][t - 1] * (1 - s["qx"][t - 1]) * (1 - s["wx"][t - 1]) * it
            av = (
                s["av"][t - 1] + s["alloc_prem"][t - 1] - s["charges"][t - 1]
            ) * (1 + credit_m) * it
            pp = s["prem_paid"][t - 1] + prem * it
        alloc = np.where(py == 1, b["alloc_rate_year1"], b["alloc_rate_renewal"])
        alloc_prem = prem * alloc * it
        sar = np.maximum(sa - av, 0.0)
        coi = sar * q * b["coi_loading"]
        amc = av * amc_m
        fee = np.full(M, b["policy_fee_pm"] * (1 + b["expense_inflation"]) ** (py - 1))
        charges = (coi + amc + fee) * it

        deaths = n * q * it
        surr = n * (1 - q) * w * it
        mats = n * (1 - q) * (1 - w) * is_mat
        death_ben = np.maximum(sa, av)
        surr_ben = np.maximum(av * (1 - _step_lookup(surrender_table(), np.full(M, py))), 0.0)
        mat_ben = np.maximum(av, pp * b["guarantee_pct"])

        s["qx"].append(q)
        s["wx"].append(w)
        s["npif"].append(n)
        s["av"].append(av)
        s["prem_paid"].append(pp)
        s["in_force"].append(it)
        s["alloc_prem"].append(alloc_prem)
        s["coi"].append(coi)
        s["amc"].append(amc)
        s["fee"].append(fee)
        s["charges"].append(charges)
        s["deaths"].append(deaths)
        s["surrenders"].append(surr)
        s["maturities"].append(mats)
        s["death_claims"].append(death_ben * deaths)
        s["surrender_claims"].append(surr_ben * surr)
        s["maturity_claims"].append(mat_ben * mats)
        s["guarantee_cost"].append((mat_ben - av) * mats)
        s["acct_at_mat"].append(av * mats)
        s["premium_income"].append(prem * n * it)
        s["renewal"].append(
            b["renewal_expense_pa"] / 12
            * (1 + b["expense_inflation"]) ** (py - 1)
            * scale
            * n
            * it
        )
        s["disc"].append(np.full(M, (1 + spot) ** (-t / 12.0)))
    return T, M, mp, s


def _savings_valuation(T, M, mp, s, basis):
    b = basis
    pv_premiums = _npv(s["premium_income"], s["disc"], FRAC["start"])
    pv_death = _npv(s["death_claims"], s["disc"], FRAC["end"])
    pv_surr = _npv(s["surrender_claims"], s["disc"], FRAC["end"])
    pv_mat = _npv(s["maturity_claims"], s["disc"], FRAC["end"])
    pv_exp = _npv(s["renewal"], s["disc"], FRAC["start"])
    pv_guar = _npv(s["guarantee_cost"], s["disc"], FRAC["end"])
    initial_expense = b["initial_expense_pct"] * mp["monthly_premium"] * 12
    bel = pv_death + pv_surr + pv_mat + pv_exp + initial_expense - pv_premiums
    final_av = np.zeros(M)
    for t in range(T + 1):
        final_av = final_av + s["acct_at_mat"][t]
    return {
        "pv_premiums": pv_premiums,
        "pv_death_claims": pv_death,
        "pv_surrender_claims": pv_surr,
        "pv_maturity_claims": pv_mat,
        "pv_expenses": pv_exp,
        "pv_guarantee_cost": pv_guar,
        "initial_expense": initial_expense,
        "bel": bel,
        "final_account_value": final_av,
        "profit_margin": -bel / pv_premiums,
    }


def _savings_series(T: int, s: dict) -> dict[str, np.ndarray]:
    """The fulfilment-cash-flow series, shared by scenarios 3 and 4."""
    net_cf = [
        s["premium_income"][t]
        - s["death_claims"][t]
        - s["surrender_claims"][t]
        - s["maturity_claims"][t]
        - s["renewal"][t]
        for t in range(T + 1)
    ]
    return {
        "deaths": _stack(s["deaths"]),
        "surrenders": _stack(s["surrenders"]),
        "maturities": _stack(s["maturities"]),
        "allocated_premium": _stack(s["alloc_prem"]),
        "cost_of_insurance": _stack(s["coi"]),
        "management_charge": _stack(s["amc"]),
        "policy_fee": _stack(s["fee"]),
        "guarantee_cost": _stack(s["guarantee_cost"]),
        "account_at_maturity": _stack(s["acct_at_mat"]),
        "death_claims": _stack(s["death_claims"]),
        "surrender_claims": _stack(s["surrender_claims"]),
        "maturity_claims": _stack(s["maturity_claims"]),
        "premium_income": _stack(s["premium_income"]),
        "renewal_expenses": _stack(s["renewal"]),
        "net_cashflow": _stack(net_cf),
    }


def savings_monthly(mps: list[dict], *, collect_series: bool) -> Outputs:
    T, M, mp, s = _savings_core(mps, SAVINGS_BASIS)
    per_mp = _savings_valuation(T, M, mp, s, SAVINGS_BASIS)
    out = Outputs(keys=[m["policy_number"] for m in mps], per_mp=per_mp)
    if collect_series:
        out.series = _savings_series(T, s)
    return out


def ifrs17_gmm(mps: list[dict], *, collect_series: bool) -> Outputs:
    b = IFRS17_BASIS
    T, M, mp, s = _savings_core(mps, b)
    per_mp = _savings_valuation(T, M, mp, s, b)
    bel = per_mp["bel"]
    locked_m = (1 + b["locked_in_rate"]) ** (1 / 12) - 1

    net_outflow = [
        s["death_claims"][t]
        + s["surrender_claims"][t]
        + s["maturity_claims"][t]
        + s["renewal"][t]
        - s["premium_income"][t]
        for t in range(T + 1)
    ]
    ra = b["ra_pct"] * (
        per_mp["pv_death_claims"] + per_mp["pv_surrender_claims"] + per_mp["pv_maturity_claims"]
    )
    csm_initial = np.maximum(-(bel + ra), 0.0)
    loss_component = np.maximum(bel + ra, 0.0)
    is_onerous = np.where(loss_component > 0.0, 1.0, 0.0)

    cu = [s["npif"][t] * mp["sum_assured"] / 1000.0 for t in range(T + 1)]
    cu_disc = [cu[t] * s["disc"][t] for t in range(T + 1)]
    total_cu = np.zeros(M)
    for t in range(T + 1):
        total_cu = total_cu + cu_disc[t]

    rcu, rel_frac = [total_cu], []
    csm, csm_rel = [csm_initial], []
    rab, ra_rel = [ra], []
    fcf = [bel]
    for t in range(T + 1):
        if t > 0:
            rcu.append(np.maximum(rcu[t - 1] - cu_disc[t - 1], 0.0))
            csm.append(np.maximum((csm[t - 1] - csm_rel[t - 1]) * (1 + locked_m), 0.0))
            rab.append(np.maximum((rab[t - 1] - ra_rel[t - 1]) * (1 + locked_m), 0.0))
            fcf.append((fcf[t - 1] - net_outflow[t - 1]) * (1 + locked_m))
        f = np.minimum(cu_disc[t] / np.maximum(rcu[t], 1e-9), 1.0)
        rel_frac.append(f)
        csm_rel.append(csm[t] * f)
        ra_rel.append(rab[t] * f)

    lrc = [fcf[t] + rab[t] + csm[t] for t in range(T + 1)]
    claims_incurred = [
        s["death_claims"][t] + s["surrender_claims"][t] + s["maturity_claims"][t]
        for t in range(T + 1)
    ]
    lic = [np.zeros(M)] + [claims_incurred[t - 1] for t in range(1, T + 1)]
    revenue = [
        net_outflow[t] + s["premium_income"][t] + ra_rel[t] + csm_rel[t] for t in range(T + 1)
    ]
    expense = [net_outflow[t] + s["premium_income"][t] for t in range(T + 1)]
    isr = [revenue[t] - expense[t] for t in range(T + 1)]

    per_mp.update(
        {
            "risk_adjustment": ra,
            "csm_initial": csm_initial,
            "loss_component_initial": loss_component,
            "is_onerous": is_onerous,
            "total_coverage_units": total_cu,
            "lrc_at_issue": lrc[0],
            "pv_csm_release": _npv(csm_rel, s["disc"], FRAC["start"]),
            "pv_ra_release": _npv(ra_rel, s["disc"], FRAC["start"]),
            "pv_insurance_service_result": _npv(isr, s["disc"], FRAC["start"]),
        }
    )
    out = Outputs(keys=[m["policy_number"] for m in mps], per_mp=per_mp)
    if collect_series:
        out.series = {
            **_savings_series(T, s),
            "net_outflow": _stack(net_outflow),
            "coverage_units": _stack(cu),
            "csm_release": _stack(csm_rel),
            "ra_release": _stack(ra_rel),
            "fcf_balance": _stack(fcf),
            "lrc": _stack(lrc),
            "claims_incurred": _stack(claims_incurred),
            "lic": _stack(lic),
            "insurance_revenue": _stack(revenue),
            "insurance_service_expense": _stack(expense),
            "insurance_service_result": _stack(isr),
        }
    return out


SCENARIOS = {
    "term_annual": term_annual,
    "term_monthly": term_monthly,
    "savings_monthly": savings_monthly,
    "ifrs17_gmm": ifrs17_gmm,
    "term_solve": term_solve,
}


#: Cells (`policies × periods`) held in flight at once. A vectorised implementation that
#: allocates `M × (T + 1)` for every component does not survive a large portfolio, so the
#: reference does what a competent hand-written one does: it batches. The budget, not the
#: policy count, is the constant — so a 480-period model gets a proportionally smaller
#: batch and the peak RSS stays flat across scenarios.
BATCH_CELLS = 500_000


def run(scenario: str, mps: list[dict], *, collect_series: bool = False) -> Outputs:
    fn = SCENARIOS[scenario]
    if collect_series:
        return fn(mps, collect_series=True)
    periods = PERIODS[scenario]
    batch = max(1, BATCH_CELLS // (periods + 1))
    if len(mps) <= batch:
        return fn(mps, collect_series=False)
    parts = [fn(mps[i : i + batch], collect_series=False) for i in range(0, len(mps), batch)]
    merged = Outputs(keys=[m["policy_number"] for m in mps])
    for name in parts[0].per_mp:
        merged.per_mp[name] = np.concatenate([p.per_mp[name] for p in parts])
    return merged


PERIODS = {
    "term_annual": 40,
    "term_monthly": 480,
    "savings_monthly": 360,
    "ifrs17_gmm": 360,
    "term_solve": 40,
}
