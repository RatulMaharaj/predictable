"""cashflower — the closest philosophical peer: a pure-Python, scalar cashflow framework.

The models live in `cf_models/`, one module per scenario, written the way a cashflower user
writes them. They are assembled and run through cashflower's own pipeline
(`get_variables` → `determine_calculation_order` → `Model.run`) rather than through
`cashflower.run()`, because `cashflower.run()` requires an `input.py`/`model.py`/
`settings.py` project layout on disk and writes CSV output.

That choice **excludes cashflower's file I/O from its measured time**, which favours
cashflower: the predictable numbers are taken through the CLI and include parse, plan,
modelpoint load and the Parquet write. The asymmetry is deliberate — a benchmark should
lean against its author — and it is stated in the published caveats.

Per-modelpoint results come from `GROUP_BY = "policy_number"`, one group per policy. That
allocates `O(M × T)` of `float64` on its own, so it is used only when the correctness gate
is running; timed runs aggregate, exactly as a production cashflower run would.
"""

from __future__ import annotations

import importlib
import inspect

import numpy as np
import pandas as pd
from cashflower.core import Model, ModelPointSet, Variable
from cashflower.start import determine_calculation_order, get_variables

from ..outputs import Outputs
from .numpy_ref import SOLVE_BRACKET, SOLVE_MAX_ITER

NAME = "cashflower"

#: `ifrs17_gmm` is not implemented here — see the caveats in `docs/v2/benchmarks.md` §5.
SUPPORTED = ("term_annual", "term_monthly", "savings_monthly", "term_solve")

PERIODS = {"term_annual": 40, "term_solve": 40, "term_monthly": 480, "savings_monthly": 360}

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

    return v("cashflower")


def _module(scenario: str):
    name = "term_annual" if scenario == "term_solve" else scenario
    return importlib.import_module(f"{__package__}.cf_models.{name}")


def _dataframe(mps: list[dict]) -> pd.DataFrame:
    df = pd.DataFrame(mps)
    for col in df.columns:
        if col in ("policy_number", "sex", "cohort"):
            continue
        if col == "smoker":
            df[col] = df[col].map({"true": True, "false": False, True: True, False: False})
        else:
            df[col] = pd.to_numeric(df[col])
    return df


def _settings(scenario: str, *, group: bool) -> dict:
    """`OUTPUT_VARIABLES` is left as `None` on purpose.

    With it set, cashflower 0.10.8 labels the output frame with the *sorted* names while
    filling it in *calculation* order, so the columns come back permuted. Leaving it unset
    takes the path where both are calculation order, and costs only the memory of a few
    unpublished columns.
    """
    T = PERIODS[scenario]
    return {
        "GROUP_BY": "policy_number" if group else None,
        "MULTIPROCESSING": False,
        "NUM_STOCHASTIC_SCENARIOS": None,
        "OUTPUT_VARIABLES": None,
        "SAVE_DIAGNOSTIC": False,
        "SAVE_LOG": False,
        "SAVE_OUTPUT": False,
        "T_MAX_CALCULATION": T,
        "T_MAX_OUTPUT": T,
    }


def _execute(scenario: str, df: pd.DataFrame, *, group: bool, premium_of=None):
    module = _module(scenario)
    settings = _settings(scenario, group=group)
    main = ModelPointSet(data=df, main=True, name="main", settings=settings)
    module.main = main
    if premium_of is not None:
        module.premium_of = premium_of
    members = [(n, v) for n, v in inspect.getmembers(module) if isinstance(v, Variable)]
    variables = determine_calculation_order(get_variables(members, settings), settings)
    output, _ = Model(variables, [main], settings).run()
    return output


def _to_outputs(scenario: str, keys: list[str], output) -> Outputs:
    """Split cashflower's stacked output back into one block of `T + 1` rows per policy.

    `Model.prepare_output` concatenates one `(T + 1)`-row frame per group and does not
    label the rows, so the blocks are located positionally: groups appear in the order they
    first occur in the model point set, which is the order of `keys`.
    """
    T = PERIODS[scenario]
    grouped = {k: output.iloc[i * (T + 1) : (i + 1) * (T + 1)] for i, k in enumerate(keys)}
    out = Outputs(keys=keys)
    for comp in SERIES_OUT[scenario]:
        out.series[comp] = np.stack([grouped[k][comp].to_numpy() for k in keys])
    for comp in PER_MP_OUT[scenario]:
        out.per_mp[comp] = np.array([grouped[k][comp].to_numpy()[0] for k in keys])
    return out


def run(scenario: str, mps: list[dict], *, collect_series: bool = False) -> Outputs:
    if scenario not in SUPPORTED:
        raise NotImplementedError(f"cashflower: {scenario} is not implemented")
    keys = [m["policy_number"] for m in mps]
    df = _dataframe(mps)
    if scenario == "term_solve":
        return _run_solve(keys, df, collect_series=collect_series)
    output = _execute(scenario, df, group=collect_series)
    if not collect_series:
        return Outputs(keys=keys)
    return _to_outputs(scenario, keys, output)


def _run_solve(keys: list[str], df: pd.DataFrame, *, collect_series: bool) -> Outputs:
    """Per-modelpoint bisection on the office premium to `bel = 0`.

    Every iteration re-projects every policy: the scalar-engine cost of an outer loop,
    shown rather than hidden. Bisection, not Brent — see `numpy_ref.term_solve`.
    """
    lo = dict.fromkeys(keys, SOLVE_BRACKET[0])
    hi = dict.fromkeys(keys, SOLVE_BRACKET[1])
    current = dict(lo)

    def premium_of(m):
        return current[m.get("policy_number")]

    def bels():
        output = _execute("term_solve", df, group=True, premium_of=premium_of)
        T = PERIODS["term_solve"]
        return {k: output["bel"].to_numpy()[i * (T + 1)] for i, k in enumerate(keys)}

    f_lo = bels()
    for _ in range(SOLVE_MAX_ITER):
        current = {k: 0.5 * (lo[k] + hi[k]) for k in keys}
        f_mid = bels()
        for k in keys:
            if (f_mid[k] > 0) == (f_lo[k] > 0):
                lo[k], f_lo[k] = current[k], f_mid[k]
            else:
                hi[k] = current[k]
        if max(hi[k] - lo[k] for k in keys) < 1e-10:
            break
    current = {k: 0.5 * (lo[k] + hi[k]) for k in keys}
    output = _execute("term_solve", df, group=True, premium_of=premium_of)
    if not collect_series:
        return Outputs(keys=keys)
    out = _to_outputs("term_solve", keys, output)
    out.per_mp["breakeven_premium"] = np.array([current[k] for k in keys])
    return out
