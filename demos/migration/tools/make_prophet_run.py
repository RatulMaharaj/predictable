#!/usr/bin/env python3
"""Generate the synthetic Prophet workspace this demo migrates from.

Writes, under ``demos/migration/prophet/``:

* ``TERM_UK.mpf``            the model point file (16 policies)
* ``SA8990.fac``             mortality factor table
* ``LAPSE_TERM.fac``         lapse factor table
* ``EXP_BAND.fac``           expense band scaling
* ``TERM_BASE_2026Q2.rpt``   the run to reconcile to

The ``.rpt`` numbers come from the transcription of ``TERM_UK.MOD`` in :func:`project`
below — a plain-Python implementation written straight from the Prophet variable
definitions, with no reference to predictable at all. That independence is the whole point:
if the two agree to the penny it is because they compute the same thing, not because one
was derived from the other.

Prophet writes money to two decimals and rates to ten; the demo keeps that, because the
half-cent rounding is exactly what the ``reconcile`` tolerance profile exists to absorb.

Usage:  python demos/migration/tools/make_prophet_run.py
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "prophet"

# --------------------------------------------------------------------------- the basis
# `TERM_UK` run parameters, as they appear in the Prophet run settings.
VAL_RATE = 0.035  # VAL_RATE
MORT_LOAD = 1.05  # MORT_LOAD
LAPSE_LOAD = 1.00  # LAPSE_LOAD
EXP_INFL = 0.028  # EXP_INFL
EXP_PP = 48.00  # EXP_PP, per policy per annum at the valuation date
INIT_EXP_PCT = 1.35  # INIT_EXP_PCT, of the first office premium
PERIODS = 40

# --------------------------------------------------------------------------- the tables

# SA8990: a Makeham-shaped rate, tabulated 18..100 by sex and smoker status.
AGES = list(range(18, 101))
SEXES = [1, 2]  # 1 = male, 2 = female
SMOKERS = [0, 1]


def sa8990(age: int, sex: int, smoker: int) -> float:
    """A Makeham law, rounded to eight decimals the way a published table would be."""
    base = 0.00022 + 0.0000045 * (1.1015 ** (age - 18)) * 10.0
    if sex == 2:
        base *= 0.72
    if smoker == 1:
        base *= 2.10
    return round(base, 8)


LAPSE_PA = [0.145, 0.098, 0.072, 0.058, 0.050, 0.046, 0.043, 0.041, 0.040] + [0.040] * 31
EXP_SCALE = {1: 0.85, 2: 1.00, 3: 1.25}

# --------------------------------------------------------------------------- the portfolio

FIELDS = [
    "SPCODE",
    "POL_NUM",
    "AGE_AT_ENTRY",
    "SEX",
    "SMOKER",
    "SUM_ASSURED",
    "ANN_PREM",
    "POL_TERM",
    "EXP_BAND",
]
TYPES = ["I", "S", "I", "I", "B", "N", "N", "I", "I"]


def portfolio() -> list[dict]:
    """A 16-policy lattice: 4 entry ages x 2 sexes x smoker/non x term, deterministic."""
    rows = []
    n = 0
    for age in (28, 35, 45, 55):
        for sex in (1, 2):
            for smoker in (0, 1):
                n += 1
                term = {28: 25, 35: 20, 45: 15, 55: 10}[age]
                sum_assured = 50000.0 + 25000.0 * ((n - 1) % 4)
                # A crude but deterministic office premium: risk premium at entry,
                # loaded, and rounded to the penny the way a quotation system would.
                q = sa8990(age, sex, smoker) * MORT_LOAD
                prem = round(sum_assured * q * 1.9 + 42.0, 2)
                rows.append(
                    {
                        "SPCODE": 1,
                        "POL_NUM": f"UK{n:05d}",
                        "AGE_AT_ENTRY": age,
                        "SEX": sex,
                        "SMOKER": smoker,
                        "SUM_ASSURED": sum_assured,
                        "ANN_PREM": prem,
                        "POL_TERM": term,
                        "EXP_BAND": (n % 3) + 1,
                    }
                )
    return rows


# --------------------------------------------------------------------------- TERM_UK.MOD


def project(mp: dict) -> dict[str, list[float]]:
    """Transcription of ``TERM_UK.MOD``. One model point, 41 periods (t = 0..40).

    Prophet indexes periods from 1; this function works in 0-based ``t`` and the writer
    adds one when it writes the file, which is why ``mapping.toml`` declares
    ``period_base = 1``.
    """
    T = PERIODS
    age0 = mp["AGE_AT_ENTRY"]
    term = mp["POL_TERM"]
    sa = mp["SUM_ASSURED"]
    prem = mp["ANN_PREM"]
    scale = EXP_SCALE[mp["EXP_BAND"]]

    att_age = [age0 + t for t in range(T + 1)]
    in_term = [1.0 if t < term else 0.0 for t in range(T + 1)]
    mort = [sa8990(min(att_age[t], 100), mp["SEX"], mp["SMOKER"]) * MORT_LOAD for t in range(T + 1)]
    lapse = [LAPSE_PA[min(t, T - 1)] * LAPSE_LOAD for t in range(T + 1)]

    pols = [0.0] * (T + 1)
    pols[0] = 1.0
    for t in range(1, T + 1):
        pols[t] = pols[t - 1] * (1 - mort[t - 1]) * (1 - lapse[t - 1]) * in_term[t]

    deaths = [pols[t] * mort[t] * in_term[t] for t in range(T + 1)]
    surrs = [pols[t] * (1 - mort[t]) * lapse[t] * in_term[t] for t in range(T + 1)]

    prem_inc = [prem * pols[t] * in_term[t] for t in range(T + 1)]
    dth_claim = [sa * deaths[t] for t in range(T + 1)]
    expense = [EXP_PP * ((1 + EXP_INFL) ** t) * scale * pols[t] * in_term[t] for t in range(T + 1)]
    net_cf = [prem_inc[t] - dth_claim[t] - expense[t] for t in range(T + 1)]

    disc = [1.0] * (T + 1)
    for t in range(1, T + 1):
        disc[t] = disc[t - 1] / (1 + VAL_RATE)

    # Premiums and expenses fall at the start of the year and are discounted at DISC(t);
    # claims fall at the end of it and are discounted a further period, DISC(t+1). The
    # last period has no successor and reuses DISC(T), which costs nothing because the
    # cashflow there is zero for every policy in this portfolio.
    disc_end = [disc[min(t + 1, T)] for t in range(T + 1)]

    pv_prem = sum(prem_inc[t] * disc[t] for t in range(T + 1))
    pv_clm = sum(dth_claim[t] * disc_end[t] for t in range(T + 1))
    pv_exp = sum(expense[t] * disc[t] for t in range(T + 1))
    init_exp = INIT_EXP_PCT * prem
    bel = pv_clm + pv_exp + init_exp - pv_prem

    reserve = [0.0] * (T + 1)
    reserve[0] = bel
    for t in range(1, T + 1):
        reserve[t] = (
            (reserve[t - 1] + prem_inc[t - 1] - dth_claim[t - 1] - expense[t - 1])
            * (1 + VAL_RATE)
            * (1.0 if t <= term else 0.0)
        )

    # RESERVE_INT is a Prophet working variable with no predictable counterpart; it is in
    # the report because Prophet reports what the run settings say, not what is useful.
    reserve_int = [reserve[t] * VAL_RATE for t in range(T + 1)]

    return {
        "MORT_RATE": mort,
        "LAPSE_RATE": lapse,
        "POLS_IF": pols,
        "DEATHS": deaths,
        "SURRENDERS": surrs,
        "PREM_INC": prem_inc,
        "DTH_CLAIM": dth_claim,
        "EXPENSE": expense,
        "NET_CF": net_cf,
        "RESERVE": reserve,
        "RESERVE_INT": reserve_int,
    }


# --------------------------------------------------------------------------- writers

RATE_COLS = {"MORT_RATE", "LAPSE_RATE", "POLS_IF", "DEATHS", "SURRENDERS"}
RPT_COLS = [
    "MORT_RATE",
    "LAPSE_RATE",
    "POLS_IF",
    "DEATHS",
    "SURRENDERS",
    "PREM_INC",
    "DTH_CLAIM",
    "EXPENSE",
    "NET_CF",
    "RESERVE",
    "RESERVE_INT",
]


def write_mpf(rows: list[dict]) -> None:
    out = [
        "! TERM_UK model points, extract 2026-06-30",
        "! Produced by: DCS export, Prophet 2023.2",
        "MPF_VERSION,3",
        f"VARIABLE_TYPES,{','.join(TYPES)}",
        f"NUMLINES,{len(rows)}",
        ",".join(FIELDS),
    ]
    for r in rows:
        cells = [
            str(r["SPCODE"]),
            r["POL_NUM"],
            str(r["AGE_AT_ENTRY"]),
            str(r["SEX"]),
            "Y" if r["SMOKER"] else "N",
            f"{r['SUM_ASSURED']:.2f}",
            f"{r['ANN_PREM']:.2f}",
            str(r["POL_TERM"]),
            str(r["EXP_BAND"]),
        ]
        out.append("*," + ",".join(cells))
    out.append("##END##")
    (OUT / "TERM_UK.mpf").write_text("\n".join(out) + "\n", encoding="utf-8")


def write_fac_mortality() -> None:
    lines = [
        "! Mortality SA8990, loaded basis applied in TERM_UK.MOD",
        "TABLE_NAME, SA8990",
        "DIMENSIONS, 3",
        f"DIM1, AGE, {AGES[0]}, {AGES[-1]}",
        "DIM2, SEX, 1, 2",
        "DIM3, SMOKER, 0, 1",
        "DATA",
    ]
    # Last dimension varies fastest.
    for age in AGES:
        vals = [f"{sa8990(age, sex, sm):.8f}" for sex in SEXES for sm in SMOKERS]
        lines.append(", ".join(vals))
    lines.append("##END##")
    (OUT / "SA8990.fac").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_fac_lapse() -> None:
    lines = [
        "! Lapse rates by policy year",
        "TABLE_NAME, LAPSE_TERM",
        "DIMENSIONS, 1",
        f"DIM1, POL_YEAR, 1, {PERIODS}",
        "DATA",
    ]
    lines.append(", ".join(f"{v:.6f}" for v in LAPSE_PA[:PERIODS]))
    lines.append("##END##")
    (OUT / "LAPSE_TERM.fac").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_fac_expense() -> None:
    lines = [
        "! Servicing expense scaling by band",
        "TABLE_NAME, EXP_BAND",
        "DIMENSIONS, 1",
        "DIM1, BAND, 1, 3",
        "DATA",
        ", ".join(f"{EXP_SCALE[b]:.6f}" for b in (1, 2, 3)),
        "##END##",
    ]
    (OUT / "EXP_BAND.fac").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_rpt(rows: list[dict]) -> None:
    lines = [
        "! Prophet results",
        "RUN, TERM_BASE_2026Q2",
        "PRODUCT, TERM_UK",
        "RUN_DATE, 30/06/2026",
        "TIME_UNITS, YEARS",
        f"NUM_PERIODS, {PERIODS}",
        "POL_NUM,PERIOD," + ",".join(RPT_COLS),
    ]
    for r in rows:
        proj = project(r)
        for t in range(PERIODS + 1):
            cells = [r["POL_NUM"], str(t + 1)]
            for col in RPT_COLS:
                v = proj[col][t]
                cells.append(f"{v:.10f}" if col in RATE_COLS else f"{v:.2f}")
            lines.append(",".join(cells))
    (OUT / "TERM_BASE_2026Q2.rpt").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    rows = portfolio()
    write_mpf(rows)
    write_fac_mortality()
    write_fac_lapse()
    write_fac_expense()
    write_rpt(rows)
    print(f"wrote {len(rows)} model points and {PERIODS + 1} periods into {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
