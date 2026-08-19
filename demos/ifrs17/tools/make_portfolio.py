#!/usr/bin/env python3
"""Generate the synthetic in-force portfolio this valuation runs over.

10,000 unit-linked endowments against the `ifrs17_gmm` model point schema, in three annual
cohorts. Deterministic: one seed, stdlib `random` only, so the same file comes out on every
machine and the run digests in the governance pack mean something.

The distribution is chosen to make the *measurement* interesting rather than to be
statistically defensible:

* a spread of entry ages, terms and premium sizes, so coverage units and hence CSM release
  patterns differ across the book;
* about one contract in eight written at a sum assured high enough relative to its premium
  that the cost of insurance eats the account — those come out **onerous**, with a loss
  component and no CSM, which is the branch of IFRS 17.47 a portfolio of only profitable
  contracts never exercises;
* three cohorts, because the annual-cohort requirement (IFRS 17.22) is the one that makes
  a CSM roll-forward a table rather than a number.

Usage:  python demos/ifrs17/tools/make_portfolio.py [--n 10000] [--out data/modelpoints.csv]
"""

from __future__ import annotations

import argparse
import csv
import random
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEMO = HERE.parent

SEED = 20260630
COHORTS = ["2024", "2025", "2026"]
COHORT_WEIGHTS = [0.28, 0.34, 0.38]
TERMS = [10, 15, 20, 25, 30]
TERM_WEIGHTS = [0.12, 0.18, 0.28, 0.22, 0.20]
PREMIUMS = [75.0, 100.0, 125.0, 150.0, 200.0, 250.0, 400.0]
PREMIUM_WEIGHTS = [0.14, 0.22, 0.20, 0.16, 0.14, 0.09, 0.05]


def build(n: int) -> list[dict]:
    rng = random.Random(SEED)
    rows: list[dict] = []
    for i in range(1, n + 1):
        cohort = rng.choices(COHORTS, COHORT_WEIGHTS)[0]
        term = rng.choices(TERMS, TERM_WEIGHTS)[0]
        premium = rng.choices(PREMIUMS, PREMIUM_WEIGHTS)[0]
        entry_age = rng.randint(22, 58)
        sex = "M" if rng.random() < 0.52 else "F"
        smoker = rng.random() < 0.18

        # Roughly one in eight is written on a "high cover" basis: the same premium buys a
        # much larger death benefit, so the monthly cost of insurance is heavy and the
        # contract is likely to be onerous at initial recognition.
        high_cover = rng.random() < 0.125
        multiple = rng.uniform(28.0, 34.0) if high_cover else rng.uniform(16.0, 22.0)
        sum_assured = round(premium * multiple * 12.0 / 12.0 * 12.0, 0)

        # Cohorts written before the valuation date have already accumulated an account.
        years_elapsed = {"2024": 2, "2025": 1, "2026": 0}[cohort]
        initial_account = round(premium * 12.0 * years_elapsed * rng.uniform(0.55, 0.85), 2)

        rows.append(
            {
                "policy_number": f"UL{i:06d}",
                "entry_age": entry_age,
                "sex": sex,
                "smoker": "true" if smoker else "false",
                "monthly_premium": f"{premium:.2f}",
                "policy_term": term,
                "initial_account": f"{initial_account:.2f}",
                "sum_assured": f"{sum_assured:.1f}",
                "expense_band": rng.choice([1, 2, 3]),
                "cohort": cohort,
            }
        )
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=10_000)
    ap.add_argument("--out", type=Path, default=DEMO / "data" / "modelpoints.csv")
    args = ap.parse_args()

    rows = build(args.n)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=list(rows[0]), lineterminator="\n")
        w.writeheader()
        w.writerows(rows)

    by_cohort: dict[str, int] = {}
    for r in rows:
        by_cohort[r["cohort"]] = by_cohort.get(r["cohort"], 0) + 1
    print(f"{len(rows):,} model points -> {args.out}")
    print("  by cohort: " + "  ".join(f"{k} {v:,}" for k, v in sorted(by_cohort.items())))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
