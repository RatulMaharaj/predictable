# Demo 3 — an IFRS 17 sample valuation

A complete IFRS 17 general measurement model valuation of a 10,000-contract portfolio,
using the committed `ifrs17_gmm` reference model **unmodified**.

The actuarial write-up — basis, formulas, results commentary, reconciliations and the
`explain()` trace of one contract's CSM — is
[`docs/v2/ifrs17-valuation.md`](../../docs/v2/ifrs17-valuation.md). This file is the
operating manual.

```console
$ ./demos/ifrs17/run_demo.sh
...
IFRS 17 DEMO: OK
```

Roughly 90 seconds: 10,000 contracts × 361 months × 47 components = **101,270,000 result
rows**, of which about 3 seconds is the projection itself and the rest is Parquet.

## What it does

| step | command | output |
|---|---|---|
| 1 | `tools/make_portfolio.py` | `data/modelpoints.csv` — 10,000 contracts, one seed, three cohorts |
| 2 | `predictable run run.pir --retain-all` | `runs/valuation/` — results, aggregates, manifest |
| 3 | `tools/csm_rollforward.py` | `out/*.csv`, `out/reconciliations.json`, `out/summary.md` |
| 4 | `predictable export` | `out/governance-pack.html` — one self-contained file |
| 5 | `predictable explain` | the CSM of `UL000001` at `t = 12`, traced |
| 6 | assertions | 19 checks, including all five reconciliations |

## Options

```console
$ N=1000 ./demos/ifrs17/run_demo.sh     # a smaller portfolio, for a quick pass
$ PREDICTABLE=/path/to/predictable ./demos/ifrs17/run_demo.sh
```

## Notes

* `retain = "full"` and `--retain-all` are required, not incidental: `csm` and `ra_balance`
  are internal to the model rather than declared outputs, and the roll-forward needs their
  opening and closing balance in every period. They are picked up through
  `[[aggregation]]` entries in `run.pir`.
* `tables/` is a symlink to `models/ifrs17_gmm/tables/`. The table digests recorded in the
  committed `build/schema.pir` therefore still verify — this demo runs the reference model
  on the reference tables, and only the portfolio is its own.
* `runs/` and `out/` are generated and git-ignored. The run directory is about 600 MB.
* The five reconciliations are computed in `tools/csm_rollforward.py` and are the reason
  the script can fail: a roll-forward that does not close, or a CSM that is not fully
  released by the end of coverage, exits non-zero.
