"""Generate the synthetic tables and modelpoint files the reference models read.

Everything here is deterministic and closed-form: no RNG, no external data, no licence
encumbrance. Run it and you get byte-identical files, which is what lets the table digests
in the committed `.pir` builds and the run goldens be checked in.

    python models/tools/make_fixtures.py

The mortality basis is a Makeham-style law, `q(x) = 1 - exp(-(A + B*c^x))`, fitted by eye to
sit in the neighbourhood of a modern insured-lives table. It is *not* a published table and
must never be used for pricing; it exists so that the reference models have a monotone,
smooth, reproducible set of rates with a plausible sex and smoker differential.
"""

from __future__ import annotations

import math
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# -- mortality ---------------------------------------------------------------------------

AGES = range(18, 121)
A, B, C = 0.00022, 2.7e-6, 1.124  # Makeham parameters

#: Multiplicative loadings applied to the Makeham base rate. Female lighter than male,
#: smoker roughly double non-smoker — the standard shape of an insured-lives basis.
SEX_FACTOR = {"M": 1.00, "F": 0.72}
SMOKER_FACTOR = {False: 1.00, True: 2.10}


def makeham_q(age: int) -> float:
    """Annual probability of death at exact age `age` under the base law."""
    mu = A + B * C**age
    return 1.0 - math.exp(-mu)


def mortality_rows() -> list[tuple[int, str, str, float]]:
    rows = []
    for age in AGES:
        for sex in ("F", "M"):
            for smoker in (False, True):
                q = makeham_q(age) * SEX_FACTOR[sex] * SMOKER_FACTOR[smoker]
                q = min(q, 1.0)
                rows.append((age, sex, "true" if smoker else "false", round(q, 8)))
    rows.sort(key=lambda r: (r[0], r[1], r[2]))
    return rows


# -- lapses ------------------------------------------------------------------------------

#: Select-then-ultimate lapse shape: heavy in year 1, decaying to a 4% ultimate.
LAPSE = [0.145, 0.098, 0.072, 0.058, 0.050, 0.046, 0.043, 0.041, 0.040]


def lapse_rows(max_year: int) -> list[tuple[int, float]]:
    out = []
    for y in range(1, max_year + 1):
        out.append((y, LAPSE[min(y, len(LAPSE)) - 1]))
    return out


# -- surrender penalty scale (savings) ---------------------------------------------------

#: Surrender penalty as a proportion of the account value, by completed policy year.
SURRENDER_PENALTY = [0.070, 0.055, 0.040, 0.030, 0.020, 0.010, 0.005, 0.000]


def surrender_rows(max_year: int) -> list[tuple[int, float]]:
    return [
        (y, SURRENDER_PENALTY[min(y, len(SURRENDER_PENALTY)) - 1]) for y in range(1, max_year + 1)
    ]


# -- expense scale by band ---------------------------------------------------------------

EXPENSE_BAND = {1: 0.85, 2: 1.00, 3: 1.25}


def expense_rows() -> list[tuple[int, float]]:
    return sorted(EXPENSE_BAND.items())


# -- yield curve -------------------------------------------------------------------------


def spot(term: int) -> float:
    """A smooth Nelson-Siegel-ish upward curve, 2.4% short to 4.4% long."""
    b0, b1, tau = 0.044, -0.020, 7.0
    if term == 0:
        return round(b0 + b1, 6)
    z = term / tau
    return round(b0 + b1 * (1 - math.exp(-z)) / z, 6)


def yield_rows(max_term: int) -> list[tuple[int, float]]:
    return [(t, spot(t)) for t in range(0, max_term + 1)]


# -- modelpoints -------------------------------------------------------------------------


def term_modelpoints(n: int = 25) -> list[dict]:
    """A small, deliberately varied term-assurance portfolio.

    Ages, terms, sums assured and premiums are laid out on a lattice rather than sampled, so
    every combination of interest (young/old, short/long, smoker/non, both sexes) is present
    exactly once and the file re-generates identically.
    """
    ages = [28, 35, 42, 49, 56]
    terms = [10, 15, 20, 25, 30]
    rows = []
    for i in range(n):
        age = ages[i % 5]
        term = terms[(i // 5) % 5]
        sex = "M" if (i % 2 == 0) else "F"
        smoker = (i % 3) == 0
        # Sum assured scales with age band; premium is a crude rate per mille so that the
        # model has something to project rather than something already solved.
        sum_assured = 50_000.0 + 25_000.0 * (i % 8)
        q = makeham_q(age) * SEX_FACTOR[sex] * SMOKER_FACTOR[smoker]
        premium = round(sum_assured * q * 2.4 + 78.0, 2)
        rows.append(
            {
                "policy_number": f"TA{i + 1:05d}",
                "entry_age": age,
                "sex": sex,
                "smoker": "true" if smoker else "false",
                "sum_assured": f"{sum_assured:.1f}",
                "annual_premium": f"{premium:.2f}",
                "policy_term": term,
                "expense_band": 1 + (i % 3),
            }
        )
    return rows


def savings_modelpoints(n: int = 20) -> list[dict]:
    ages = [30, 38, 45, 52]
    terms = [10, 15, 20, 25, 30]
    rows = []
    for i in range(n):
        age = ages[i % 4]
        term = terms[(i // 4) % 5]
        sex = "M" if (i % 2 == 0) else "F"
        smoker = (i % 4) == 0
        premium = 100.0 + 25.0 * (i % 6)
        rows.append(
            {
                "policy_number": f"SV{i + 1:05d}",
                "entry_age": age,
                "sex": sex,
                "smoker": "true" if smoker else "false",
                "monthly_premium": f"{premium:.2f}",
                "policy_term": term,
                "initial_account": f"{premium * 12 * (i % 3):.2f}",
                "sum_assured": f"{max(premium * 12 * term * 0.6, 25000.0):.1f}",
                "expense_band": 1 + (i % 3),
                "cohort": f"{2026 - (i % 3)}",
            }
        )
    return rows


# -- writing -----------------------------------------------------------------------------


def write_csv(path: Path, header: list[str], rows) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = [",".join(header)]
    for row in rows:
        cells = row.values() if isinstance(row, dict) else row
        lines.append(",".join(str(c) for c in cells))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def main() -> None:
    mort = mortality_rows()
    yc = yield_rows(40)

    for model in ("term_annual", "term_monthly", "term_solve"):
        d = ROOT / model
        write_csv(d / "tables" / "mortality.csv", ["age", "sex", "smoker", "qx"], mort)
        write_csv(d / "tables" / "lapses.csv", ["policy_year", "lapse_pa"], lapse_rows(40))
        write_csv(d / "tables" / "expenses.csv", ["band", "scale"], expense_rows())
        write_csv(
            d / "data" / "modelpoints.csv",
            [
                "policy_number",
                "entry_age",
                "sex",
                "smoker",
                "sum_assured",
                "annual_premium",
                "policy_term",
                "expense_band",
            ],
            term_modelpoints(),
        )

    for model in ("savings_monthly", "ifrs17_gmm"):
        d = ROOT / model
        write_csv(d / "tables" / "mortality.csv", ["age", "sex", "smoker", "qx"], mort)
        write_csv(d / "tables" / "lapses.csv", ["policy_year", "lapse_pa"], lapse_rows(40))
        write_csv(d / "tables" / "surrender.csv", ["policy_year", "penalty"], surrender_rows(40))
        write_csv(d / "tables" / "yield_curve.csv", ["term", "spot"], yc)
        write_csv(d / "tables" / "expenses.csv", ["band", "scale"], expense_rows())
        write_csv(
            d / "data" / "modelpoints.csv",
            [
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
            ],
            savings_modelpoints(),
        )

    print("fixtures written under", ROOT)


if __name__ == "__main__":
    main()
