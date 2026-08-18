#!/usr/bin/env python3
"""Step 1-2 of the migration: turn the Prophet *inputs* into predictable inputs.

The ``.rpt`` is not touched here — it is the reconciliation target and is read by
``predictable diff run`` through the Rust readers in ``predictable-prophet``. What this
script converts is the two things predictable must be able to *run* on:

* ``TERM_UK.mpf``  -> ``data/modelpoints.csv``
* ``*.fac``        -> ``tables/*.csv``

Three translations happen here and each one is a decision that is written down rather than
inferred (``skills/predictable-migration`` step 3):

1. ``SEX`` is a Prophet integer code (1 = male, 2 = female); predictable carries the
   rating factor itself, so it becomes ``M`` / ``F``.
2. ``SMOKER`` is ``Y``/``N`` in the MPF and a boolean in predictable.
3. ``SA8990`` holds the **unloaded** table rate. ``MORT_LOAD`` is applied in the model, not
   baked into the table, because it is a valuation assumption and belongs in ``base.pir``
   where it can be varied.

Usage:  python demos/migration/tools/import_prophet_inputs.py
"""

from __future__ import annotations

import csv
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEMO = HERE.parent
PROPHET = DEMO / "prophet"

SEX_CODE = {"1": "M", "2": "F"}
SMOKER_CODE = {"Y": "true", "N": "false"}


def read_fac(path: Path) -> tuple[list[tuple[str, int, int]], list[float]]:
    """Minimal ``.fac`` reader: returns the dimensions and the flat value list.

    The last dimension varies fastest — the same convention ``predictable-prophet``'s
    ``read_fac`` documents and enforces with ``P0202``.
    """
    dims: list[tuple[str, int, int]] = []
    values: list[float] = []
    in_data = False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("!") or line == "##END##":
            continue
        if line == "DATA":
            in_data = True
            continue
        if in_data:
            values.extend(float(x) for x in line.split(",") if x.strip())
            continue
        head, *rest = [x.strip() for x in line.split(",")]
        if head.startswith("DIM") and head != "DIMENSIONS":
            dims.append((rest[0], int(rest[1]), int(rest[2])))
    expected = 1
    for _, lo, hi in dims:
        expected *= hi - lo + 1
    if expected != len(values):
        raise SystemExit(f"{path.name}: expected {expected} values, found {len(values)} (P0203)")
    return dims, values


def mpf_rows(path: Path) -> tuple[list[str], list[list[str]]]:
    names: list[str] = []
    rows: list[list[str]] = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("!") or line == "##END##":
            continue
        if line.startswith("*,"):
            rows.append(next(csv.reader([line[2:]])))
            continue
        key = line.split(",", 1)[0].strip()
        if key in {"MPF_VERSION", "VARIABLE_TYPES", "NUMLINES", "OUTPUT_FORMAT"}:
            continue
        names = [x.strip() for x in line.split(",")]
    return names, rows


def write_modelpoints() -> int:
    names, rows = mpf_rows(PROPHET / "TERM_UK.mpf")
    idx = {n: i for i, n in enumerate(names)}
    out = DEMO / "data" / "modelpoints.csv"
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh, lineterminator="\n")
        w.writerow(
            [
                "policy_number",
                "entry_age",
                "sex",
                "smoker",
                "sum_assured",
                "annual_premium",
                "policy_term",
                "expense_band",
            ]
        )
        for r in rows:
            # SPCODE is dropped: predictable has no system product code, and the product is
            # named once in `product.pir` instead of once per row.
            w.writerow(
                [
                    r[idx["POL_NUM"]],
                    r[idx["AGE_AT_ENTRY"]],
                    SEX_CODE[r[idx["SEX"]]],
                    SMOKER_CODE[r[idx["SMOKER"]].upper()],
                    r[idx["SUM_ASSURED"]],
                    r[idx["ANN_PREM"]],
                    r[idx["POL_TERM"]],
                    r[idx["EXP_BAND"]],
                ]
            )
    return len(rows)


def write_mortality() -> int:
    dims, values = read_fac(PROPHET / "SA8990.fac")
    (_, age_lo, age_hi), (_, sex_lo, sex_hi), (_, sm_lo, sm_hi) = dims
    out = DEMO / "tables" / "mortality.csv"
    out.parent.mkdir(parents=True, exist_ok=True)
    n = 0
    with out.open("w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh, lineterminator="\n")
        w.writerow(["age", "sex", "smoker", "qx"])
        i = 0
        for age in range(age_lo, age_hi + 1):
            for sex in range(sex_lo, sex_hi + 1):
                for sm in range(sm_lo, sm_hi + 1):
                    w.writerow(
                        [age, SEX_CODE[str(sex)], "true" if sm else "false", f"{values[i]:.8f}"]
                    )
                    i += 1
                    n += 1
    return n


def write_lapses() -> int:
    dims, values = read_fac(PROPHET / "LAPSE_TERM.fac")
    (_, lo, hi) = dims[0]
    out = DEMO / "tables" / "lapses.csv"
    with out.open("w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh, lineterminator="\n")
        w.writerow(["policy_year", "lapse_pa"])
        for i, year in enumerate(range(lo, hi + 1)):
            w.writerow([year, f"{values[i]:.6f}"])
    return len(values)


def write_expenses() -> int:
    dims, values = read_fac(PROPHET / "EXP_BAND.fac")
    (_, lo, hi) = dims[0]
    out = DEMO / "tables" / "expenses.csv"
    with out.open("w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh, lineterminator="\n")
        w.writerow(["band", "scale"])
        for i, band in enumerate(range(lo, hi + 1)):
            w.writerow([band, f"{values[i]:.6f}"])
    return len(values)


def main() -> int:
    print(f"model points : {write_modelpoints():>5}  -> data/modelpoints.csv")
    print(f"SA8990       : {write_mortality():>5}  -> tables/mortality.csv")
    print(f"LAPSE_TERM   : {write_lapses():>5}  -> tables/lapses.csv")
    print(f"EXP_BAND     : {write_expenses():>5}  -> tables/expenses.csv")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
