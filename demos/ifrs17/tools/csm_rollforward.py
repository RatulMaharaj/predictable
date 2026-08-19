#!/usr/bin/env python3
"""Build the IFRS 17 disclosure tables from the run, and reconcile them.

Reads `runs/valuation/` and writes, into `out/`:

    csm_rollforward.csv          CSM roll-forward, per cohort per projection year
    ra_rollforward.csv           the same for the risk adjustment
    initial_recognition.csv      the IFRS 17.47 initial-recognition split, per cohort
    reconciliations.json         five identities, each checked and reported with its residual
    summary.md                   the same, rendered

Nothing here re-derives a number the engine computed. Every figure is either read from
`aggregates.parquet` / `results.parquet` or is a difference of two figures that were, and
the reconciliations exist to say so out loud: a roll-forward whose "interest accreted" line
is a plug that absorbs whatever is left over is not a roll-forward.

Usage:  python demos/ifrs17/tools/csm_rollforward.py [--run runs/valuation] [--out out]
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from pathlib import Path

import pyarrow.compute as pc
import pyarrow.dataset as ds
import pyarrow.parquet as pq

HERE = Path(__file__).resolve().parent
DEMO = HERE.parent

MONTHS = 12
LOCKED_IN_RATE = 0.038  # base.pir locked_in_rate; monthly equivalent below
LOCKED_IN_M = (1 + LOCKED_IN_RATE) ** (1 / 12) - 1

#: PerMP components read from `results.parquet` (they live at `t = -1`).
PER_MP = [
    "model.bel",
    "model.risk_adjustment",
    "model.csm_initial",
    "model.loss_component_initial",
    "model.is_onerous",
    "model.lrc_at_issue",
    "model.total_coverage_units",
    "model.pv_csm_release",
    "model.pv_ra_release",
    "model.pv_insurance_service_result",
]


# --------------------------------------------------------------------------------- inputs


def read_aggregates(run: Path) -> dict[tuple[str, str], dict[int, float]]:
    """`{(aggregation, group_key): {t: value}}`."""
    table = pq.read_table(run / "aggregates.parquet")
    out: dict[tuple[str, str], dict[int, float]] = defaultdict(dict)
    for agg, key, t, v in zip(
        table["aggregation"].to_pylist(),
        table["group_key"].to_pylist(),
        table["t"].to_pylist(),
        table["value"].to_pylist(),
    ):
        out[(agg, key)][t] = v
    return out


def read_per_mp(run: Path) -> tuple[dict[str, float], dict[str, dict[str, float]]]:
    """Portfolio totals and per-cohort totals of every `PerMP` component in `PER_MP`.

    Streamed in batches: the result set is 101 million rows and there is no reason to hold
    it in memory to add up ten of its components.
    """
    cohort_of = _cohort_of()
    dataset = ds.dataset(run / "results.parquet", format="parquet")
    scanner = dataset.scanner(
        columns=["mp_key", "component", "value"],
        filter=(pc.field("t") == -1) & pc.field("component").isin(PER_MP),
        batch_size=1 << 16,
    )
    totals: dict[str, float] = defaultdict(float)
    by_cohort: dict[str, dict[str, float]] = defaultdict(lambda: defaultdict(float))
    for batch in scanner.to_batches():
        keys = batch["mp_key"].to_pylist()
        comps = batch["component"].to_pylist()
        vals = batch["value"].to_pylist()
        for key, comp, val in zip(keys, comps, vals):
            if val is None:
                continue
            totals[comp] += val
            by_cohort[cohort_of[key]][comp] += val
    return dict(totals), {k: dict(v) for k, v in by_cohort.items()}


def _cohort_of() -> dict[str, str]:
    import csv

    with (DEMO / "data" / "modelpoints.csv").open(encoding="utf-8") as fh:
        return {r["policy_number"]: r["cohort"] for r in csv.DictReader(fh)}


# ---------------------------------------------------------------------------- roll-forward


def rollforward(
    balance: dict[int, float], release: dict[int, float], periods: int, label: str
) -> list[dict]:
    """One balance's annual roll-forward.

    Per projection year `y` covering months `12y .. 12y+11`:

        closing = opening + accretion - release            (the identity)
        accretion = sum over the year of (b[t] - r[t]) * i_m

    `accretion` is **computed from the model's own recursion**, not plugged. `residual` is
    what the identity leaves over, and it is where the `max(..., 0)` floor in the model
    shows up once a group's balance is exhausted. A roll-forward that hid that behind a
    balancing item would be reporting the identity rather than testing it.
    """
    rows = []
    years = periods // MONTHS
    for y in range(years):
        lo, hi = y * MONTHS, (y + 1) * MONTHS
        opening = balance.get(lo, 0.0)
        closing = balance.get(hi, 0.0)
        released = sum(release.get(t, 0.0) for t in range(lo, hi))
        accreted = sum((balance.get(t, 0.0) - release.get(t, 0.0)) * LOCKED_IN_M
                       for t in range(lo, hi))
        rows.append(
            {
                "quantity": label,
                "year": y + 1,
                "opening": opening,
                "accreted": accreted,
                "released": released,
                "closing": closing,
                "residual": closing - (opening + accreted - released),
            }
        )
    return rows


# ------------------------------------------------------------------------------ the script


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", type=Path, default=DEMO / "runs" / "valuation")
    ap.add_argument("--out", type=Path, default=DEMO / "out")
    ap.add_argument("--periods", type=int, default=360)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    aggs = read_aggregates(args.run)
    cohorts = sorted({key.split("=", 1)[1] for (name, key) in aggs if name == "csm_by_cohort"})
    totals, by_cohort = read_per_mp(args.run)

    # ---------------------------------------------------------------- initial recognition
    ir_rows = []
    for c in cohorts:
        b = by_cohort.get(c, {})
        ir_rows.append(
            {
                "cohort": c,
                "contracts": int(round(_count(c))),
                "fcf_at_issue": b.get("model.bel", 0.0),
                "risk_adjustment": b.get("model.risk_adjustment", 0.0),
                "csm_at_issue": b.get("model.csm_initial", 0.0),
                "loss_component": b.get("model.loss_component_initial", 0.0),
                "onerous_contracts": int(round(b.get("model.is_onerous", 0.0))),
                "lrc_at_issue": b.get("model.lrc_at_issue", 0.0),
            }
        )
    _write_csv(args.out / "initial_recognition.csv", ir_rows)

    # ---------------------------------------------------------------------- roll-forwards
    csm_rows, ra_rows = [], []
    for c in cohorts:
        key = f"cohort={c}"
        csm = aggs.get(("csm_by_cohort", key), {})
        csm_rel = aggs.get(("csm_release_by_cohort", key), {})
        ra = aggs.get(("ra_balance_by_cohort", key), {})
        ra_rel = aggs.get(("ra_release_by_cohort", key), {})
        for r in rollforward(csm, csm_rel, args.periods, "CSM"):
            csm_rows.append({"cohort": c, **r})
        for r in rollforward(ra, ra_rel, args.periods, "RA"):
            ra_rows.append({"cohort": c, **r})
    _write_csv(args.out / "csm_rollforward.csv", csm_rows)
    _write_csv(args.out / "ra_rollforward.csv", ra_rows)

    # -------------------------------------------------------------------- reconciliations
    recs = reconcile(aggs, cohorts, totals, csm_rows, args.periods)
    (args.out / "reconciliations.json").write_text(json.dumps(recs, indent=2) + "\n")
    (args.out / "summary.md").write_text(render(ir_rows, csm_rows, ra_rows, recs, cohorts))

    print(f"wrote {args.out}/csm_rollforward.csv, ra_rollforward.csv, "
          f"initial_recognition.csv, reconciliations.json, summary.md")
    failed = [r for r in recs["checks"] if not r["ok"]]
    for r in recs["checks"]:
        print(f"  {'PASS' if r['ok'] else 'FAIL'}  {r['name']}  residual={r['residual']:.6g}")
    return 1 if failed else 0


_COUNTS: dict[str, int] = {}


def _count(cohort: str) -> int:
    if not _COUNTS:
        for k, v in _cohort_of().items():
            _COUNTS[v] = _COUNTS.get(v, 0) + 1
    return _COUNTS.get(cohort, 0)


def reconcile(aggs, cohorts, totals, csm_rows, periods) -> dict:
    checks = []

    def add(name, statement, residual, tol, detail=""):
        checks.append(
            {
                "name": name,
                "statement": statement,
                "residual": residual,
                "tolerance": tol,
                "ok": abs(residual) <= tol,
                "detail": detail,
            }
        )

    # 1. IFRS 17.47: every contract is either profitable (CSM > 0, no loss component) or
    #    onerous (loss component > 0, no CSM). The product of the two totals is not the
    #    test — the test is that no contract has both, which the model enforces by
    #    construction and which shows up as LRC at issue equalling the loss component.
    add(
        "lrc_at_issue = loss_component",
        "No gain is recognised at initial recognition (IFRS 17.38): LRC at t=0 equals the "
        "loss component of the onerous contracts and nothing else.",
        totals.get("model.lrc_at_issue", 0.0) - totals.get("model.loss_component_initial", 0.0),
        1e-6 * max(1.0, abs(totals.get("model.loss_component_initial", 0.0))),
    )

    # 2. CSM at initial recognition = max(-(FCF + RA), 0), so for the profitable part of
    #    the book, CSM + FCF + RA = 0 exactly.
    fcf = totals.get("model.bel", 0.0)
    ra = totals.get("model.risk_adjustment", 0.0)
    csm0 = totals.get("model.csm_initial", 0.0)
    lc = totals.get("model.loss_component_initial", 0.0)
    add(
        "csm - loss_component = -(fcf + ra)",
        "CSM and the loss component are the two sides of max(-(FCF+RA),0) and "
        "max(FCF+RA,0); their difference is exactly -(FCF+RA).",
        (csm0 - lc) + (fcf + ra),
        1e-6 * max(1.0, abs(fcf + ra)),
    )

    # 3. The roll-forward closes: opening + accretion - release = closing, every year,
    #    every cohort, up to the floor residual.
    worst = max(csm_rows, key=lambda r: abs(r["residual"]))
    add(
        "csm roll-forward closes",
        "For every cohort and every projection year, closing CSM = opening + interest "
        "accreted - CSM released. The accretion is computed from the model's own monthly "
        "recursion, not plugged.",
        worst["residual"],
        0.01,
        f"worst year {worst['year']} cohort {worst['cohort']}",
    )

    # 4. The CSM is fully released by the end of the projection.
    closing = sum(
        aggs.get(("csm_by_cohort", f"cohort={c}"), {}).get(periods, 0.0) for c in cohorts
    )
    add(
        "csm fully released by t = T",
        "Every contract in the portfolio matures inside the 360-month projection, so the "
        "closing CSM is zero: nothing is left unrecognised.",
        closing,
        0.01,
    )

    # 5. On the central assumption set, the insurance service result is exactly the RA and
    #    CSM released: there is no experience variance in a run against its own basis.
    isr = _sum_over_t(aggs, "insurance_service_result_by_cohort", cohorts, periods)
    rel = _sum_over_t(aggs, "csm_release_by_cohort", cohorts, periods) + _sum_over_t(
        aggs, "ra_release_by_cohort", cohorts, periods
    )
    add(
        "service result = ra release + csm release",
        "On the central assumptions there is no experience variance, so the insurance "
        "service result is exactly what the RA and the CSM released.",
        isr - rel,
        1e-6 * max(1.0, abs(rel)),
    )

    return {
        "format": "ifrs17-reconciliation/1",
        "totals": {k: totals.get(k, 0.0) for k in PER_MP},
        "checks": checks,
        "all_ok": all(c["ok"] for c in checks),
    }


def _sum_over_t(aggs, name, cohorts, periods) -> float:
    total = 0.0
    for c in cohorts:
        series = aggs.get((name, f"cohort={c}"), {})
        total += sum(v for t, v in series.items() if 0 <= t <= periods)
    return total


def _write_csv(path: Path, rows: list[dict]) -> None:
    import csv

    if not rows:
        return
    with path.open("w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=list(rows[0]), lineterminator="\n")
        w.writeheader()
        for r in rows:
            w.writerow({k: (f"{v:.2f}" if isinstance(v, float) else v) for k, v in r.items()})


def render(ir_rows, csm_rows, ra_rows, recs, cohorts) -> str:
    def money(x: float) -> str:
        return f"{x:,.0f}"

    out = ["# IFRS 17 sample valuation — generated tables", ""]
    out += ["## Initial recognition, by annual cohort (IFRS 17.47)", ""]
    out += ["| cohort | contracts | FCF at issue | RA | CSM | loss component | onerous | LRC at issue |",
            "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for r in ir_rows:
        out.append(
            f"| {r['cohort']} | {r['contracts']:,} | {money(r['fcf_at_issue'])} | "
            f"{money(r['risk_adjustment'])} | {money(r['csm_at_issue'])} | "
            f"{money(r['loss_component'])} | {r['onerous_contracts']:,} | "
            f"{money(r['lrc_at_issue'])} |"
        )
    tot = {k: sum(r[k] for r in ir_rows) for k in
           ("fcf_at_issue", "risk_adjustment", "csm_at_issue", "loss_component", "lrc_at_issue")}
    out.append(
        f"| **total** | {sum(r['contracts'] for r in ir_rows):,} | {money(tot['fcf_at_issue'])} | "
        f"{money(tot['risk_adjustment'])} | {money(tot['csm_at_issue'])} | "
        f"{money(tot['loss_component'])} | {sum(r['onerous_contracts'] for r in ir_rows):,} | "
        f"{money(tot['lrc_at_issue'])} |"
    )

    for label, rows in (("CSM", csm_rows), ("Risk adjustment", ra_rows)):
        out += ["", f"## {label} roll-forward — first ten projection years, all cohorts", ""]
        out += ["| year | opening | interest accreted | released | closing |",
                "|---:|---:|---:|---:|---:|"]
        for y in range(1, 11):
            sel = [r for r in rows if r["year"] == y]
            out.append(
                f"| {y} | {money(sum(r['opening'] for r in sel))} | "
                f"{money(sum(r['accreted'] for r in sel))} | "
                f"{money(sum(r['released'] for r in sel))} | "
                f"{money(sum(r['closing'] for r in sel))} |"
            )

    out += ["", "## Reconciliations", "", "| check | residual | tolerance | result |",
            "|---|---:|---:|---|"]
    for c in recs["checks"]:
        out.append(
            f"| {c['name']} | {c['residual']:.6g} | {c['tolerance']:.3g} | "
            f"{'PASS' if c['ok'] else 'FAIL'} |"
        )
    out.append("")
    return "\n".join(out)


if __name__ == "__main__":
    raise SystemExit(main())
