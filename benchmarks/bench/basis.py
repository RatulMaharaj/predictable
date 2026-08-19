"""The shared basis: assumption sets, table fixtures and modelpoint generation.

Every engine in this repository reads its inputs from here, so a disagreement between
two engines can never be a disagreement about the inputs. The tables and the modelpoint
lattice are the ones `models/tools/make_fixtures.py` writes for the reference corpus —
imported from it rather than re-derived, so the benchmark and the golden corpus cannot
drift apart.
"""

from __future__ import annotations

import importlib.util
import sys
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODELS = ROOT / "models"


def _load_make_fixtures():
    spec = importlib.util.spec_from_file_location(
        "_bench_make_fixtures", MODELS / "tools" / "make_fixtures.py"
    )
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    return mod


fixtures = _load_make_fixtures()

#: Assumption sets, **parsed from the committed `base.pir` of each reference model** rather
#: than copied. Copying would let the benchmark value on a stale basis while still passing
#: its own internal checks; parsing means a change to `models/*/base.pir` reaches every
#: engine at once, or fails loudly here.


def _read_base_pir(model: str) -> dict[str, float]:
    """Read the flat `key = value` assumptions out of a model's `base.pir`.

    `base.pir` at assumption-set level is a flat table of scalars — no sections, no
    nesting — so a five-line reader is honest here and avoids a TOML dependency in the
    benchmark venv. Anything that is not a bare float is skipped: `format`,
    `assumption_set` and `model_module` are metadata, not basis.
    """
    out: dict[str, float] = {}
    for line in (MODELS / model / "base.pir").read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if not line or "=" not in line:
            continue
        key, _, value = line.partition("=")
        try:
            out[key.strip()] = float(value.strip())
        except ValueError:
            continue
    return out


TERM_BASIS = _read_base_pir("term_annual")
SAVINGS_BASIS = _read_base_pir("savings_monthly")
IFRS17_BASIS = _read_base_pir("ifrs17_gmm")


# -- tables ------------------------------------------------------------------------------

MIN_AGE, MAX_AGE = 18, 120


@lru_cache(maxsize=1)
def mortality_table() -> dict[tuple[str, bool], list[float]]:
    """`{(sex, smoker): [qx by age from MIN_AGE]}` — the `clamp` key policy applied by index."""
    out: dict[tuple[str, bool], list[float]] = {}
    for age, sex, smoker, q in fixtures.mortality_rows():
        out.setdefault((sex, smoker == "true"), []).append(q)
    return out


@lru_cache(maxsize=1)
def lapse_table() -> list[float]:
    """`lapse_pa` by policy year, index 0 unused (policy years are 1-based)."""
    return [0.0] + [q for _, q in fixtures.lapse_rows(40)]


@lru_cache(maxsize=1)
def surrender_table() -> list[float]:
    return [0.0] + [p for _, p in fixtures.surrender_rows(40)]


@lru_cache(maxsize=1)
def yield_table() -> list[float]:
    """Spot rate by integer term, index 0..40, `clamp` above 40."""
    return [s for _, s in fixtures.yield_rows(40)]


EXPENSE_BAND = dict(fixtures.EXPENSE_BAND)


# -- modelpoints -------------------------------------------------------------------------


def term_modelpoints(n: int) -> list[dict]:
    return fixtures.term_modelpoints(n)


def savings_modelpoints(n: int) -> list[dict]:
    return fixtures.savings_modelpoints(n)


TERM_HEADER = [
    "policy_number",
    "entry_age",
    "sex",
    "smoker",
    "sum_assured",
    "annual_premium",
    "policy_term",
    "expense_band",
]

SAVINGS_HEADER = [
    "policy_number",
    "entry_age",
    "sex",
    "smoker",
    "monthly_premium",
    "policy_term",
    "initial_account",
    "sum_assured",
    "expense_band",
    "cohort",
]


def write_modelpoints(path: Path, header: list[str], rows: list[dict]) -> None:
    fixtures.write_csv(path, header, rows)
