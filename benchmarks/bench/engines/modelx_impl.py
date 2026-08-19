"""lifelib / modelx — the reference open-source actuarial modelling stack.

lifelib's projection models *are* modelx models: a space parametrised by the model point,
memoised cells as formulas of `t`. This module builds exactly that shape for the benchmark
scenarios, out of the formulas in `mx_models.py`, rather than running a lifelib library
model — because a lifelib library model is a different product on a different basis, and
comparing timings across different models measures nothing.

What is being measured, then, is **modelx as an engine**: its dependency tracking, its
memoisation, its dynamic-space machinery, on the same five formulas the other three
engines run.

Memory is the constraint. modelx retains every computed cell value for dependency tracing,
so a portfolio is projected in batches and the model is cleared between them; the batch
size is a property of modelx's memory behaviour, not a tuning knob, and it is reported.
"""

from __future__ import annotations

import numpy as np

from ..basis import SAVINGS_BASIS, TERM_BASIS
from ..outputs import Outputs
from . import mx_models, scalar_basis
from .numpy_ref import SOLVE_BRACKET, SOLVE_MAX_ITER

NAME = "modelx"

#: `ifrs17_gmm` is not implemented here — see the caveats in `docs/v2/benchmarks.md` §5.
SUPPORTED = ("term_annual", "term_monthly", "savings_monthly", "term_solve")

PERIODS = {"term_annual": 40, "term_solve": 40, "term_monthly": 480, "savings_monthly": 360}

#: Model points per modelx model instance, before the model is discarded and rebuilt.
BATCH = 250

PREFIX = {
    "term_annual": "ta_",
    "term_solve": "ta_",
    "term_monthly": "tm_",
    "savings_monthly": "sv_",
}

BASIS = {
    "term_annual": TERM_BASIS,
    "term_solve": TERM_BASIS,
    "term_monthly": TERM_BASIS,
    "savings_monthly": SAVINGS_BASIS,
}

SERIES_OUT = {
    "term_annual": [
        "deaths", "surrenders", "premium_income", "death_claims",
        "renewal_expenses", "net_cashflow", "reserve",
    ],
    "savings_monthly": [
        "deaths", "surrenders", "maturities", "allocated_premium", "cost_of_insurance",
        "management_charge", "policy_fee", "guarantee_cost", "account_at_maturity",
        "death_claims", "surrender_claims", "maturity_claims", "premium_income",
        "renewal_expenses", "net_cashflow",
    ],
}
SERIES_OUT["term_monthly"] = SERIES_OUT["term_annual"]
SERIES_OUT["term_solve"] = SERIES_OUT["term_annual"]

PER_MP_OUT = {
    "term_annual": [
        "pv_premiums", "pv_claims", "pv_expenses", "initial_expense", "bel", "profit_margin",
    ],
    "savings_monthly": [
        "pv_premiums", "pv_death_claims", "pv_surrender_claims", "pv_maturity_claims",
        "pv_expenses", "pv_guarantee_cost", "initial_expense", "bel",
        "final_account_value", "profit_margin",
    ],
}
PER_MP_OUT["term_monthly"] = PER_MP_OUT["term_annual"]
PER_MP_OUT["term_solve"] = PER_MP_OUT["term_annual"]


def version() -> str:
    from importlib.metadata import version as v

    return f"modelx {v('modelx')} / lifelib {v('lifelib')}"


#: Modelpoint files are text; every other engine coerces on load, so this one does too.
_STR_FIELDS = {"policy_number", "sex", "cohort"}


def _typed(mps: list[dict]) -> list[dict]:
    out = []
    for mp in mps:
        row = {}
        for k, v in mp.items():
            if k in _STR_FIELDS:
                row[k] = v
            elif k == "smoker":
                row[k] = v is True or v == "true"
            else:
                row[k] = float(v)
        out.append(row)
    return out


def _build(scenario: str, batch: list[dict], premiums: list[float] | None):
    import modelx as mx

    prefix = PREFIX[scenario]
    T = PERIODS[scenario]
    model = mx.new_model()
    space = model.new_space("Proj", formula=lambda pol: None)
    space.MP = batch
    space.b = BASIS[scenario]
    space.T = T
    space.tb = scalar_basis
    space.new_cells(name="mp", formula=mx_models.mp)
    if scenario == "term_solve":
        space.PREMIUM = list(premiums)

        def premium():
            return PREMIUM[pol]  # noqa: F821

        space.new_cells(name="premium", formula=premium)
    else:

        def premium():
            return mp()["annual_premium"]  # noqa: F821

        space.new_cells(name="premium", formula=premium)

    names = []
    for attr in dir(mx_models):
        if not attr.startswith(prefix):
            continue
        short = attr[len(prefix) :]
        space.new_cells(name=attr, formula=getattr(mx_models, attr))
        names.append(short)
    return model, space, names


def _project(scenario: str, mps: list[dict], *, collect_series: bool, premiums=None):
    import modelx as mx

    prefix = PREFIX[scenario]
    T = PERIODS[scenario]
    per_mp = {c: [] for c in PER_MP_OUT[scenario]}
    series = {c: [] for c in SERIES_OUT[scenario]} if collect_series else {}

    typed = _typed(mps)
    for start in range(0, len(mps), BATCH):
        batch = typed[start : start + BATCH]
        batch_prem = None if premiums is None else premiums[start : start + BATCH]
        model, space, _ = _build(scenario, batch, batch_prem)
        for i in range(len(batch)):
            proj = space[i]
            for comp in PER_MP_OUT[scenario]:
                per_mp[comp].append(getattr(proj, prefix + comp)())
            if collect_series:
                for comp in SERIES_OUT[scenario]:
                    cells = getattr(proj, prefix + comp)
                    series[comp].append([cells(t) for t in range(T + 1)])
            else:
                # Still force the projection itself, so a timed run is a real run.
                cells = getattr(proj, prefix + "net_cashflow")
                for t in range(T + 1):
                    cells(t)
        model.close()
        mx.core.system.serializing = None

    out = Outputs(keys=[m["policy_number"] for m in mps])
    out.per_mp = {k: np.asarray(v, dtype=np.float64) for k, v in per_mp.items()}
    if collect_series:
        out.series = {k: np.asarray(v, dtype=np.float64) for k, v in series.items()}
    return out


def run(scenario: str, mps: list[dict], *, collect_series: bool = False) -> Outputs:
    if scenario not in SUPPORTED:
        raise NotImplementedError(f"modelx: {scenario} is not implemented")
    if scenario == "term_solve":
        return _run_solve(mps, collect_series=collect_series)
    return _project(scenario, mps, collect_series=collect_series)


def _run_solve(mps: list[dict], *, collect_series: bool) -> Outputs:
    """Per-modelpoint bisection to `bel = 0`, rebuilding the model each iteration.

    Bisection, not Brent — see `numpy_ref.term_solve` for why the four engines share the
    solver and what that means for the timing.
    """
    n = len(mps)
    lo = [SOLVE_BRACKET[0]] * n
    hi = [SOLVE_BRACKET[1]] * n

    def bels(premiums):
        return _project("term_solve", mps, collect_series=False, premiums=premiums).per_mp["bel"]

    f_lo = bels(lo)
    for _ in range(SOLVE_MAX_ITER):
        mid = [0.5 * (lo[i] + hi[i]) for i in range(n)]
        f_mid = bels(mid)
        for i in range(n):
            if (f_mid[i] > 0) == (f_lo[i] > 0):
                lo[i], f_lo[i] = mid[i], f_mid[i]
            else:
                hi[i] = mid[i]
        if max(hi[i] - lo[i] for i in range(n)) < 1e-10:
            break
    solved = [0.5 * (lo[i] + hi[i]) for i in range(n)]
    out = _project("term_solve", mps, collect_series=collect_series, premiums=solved)
    out.per_mp["breakeven_premium"] = np.asarray(solved, dtype=np.float64)
    return out
