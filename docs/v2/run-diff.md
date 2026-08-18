# Diffing two runs

`predictable diff run A B` compares two sets of **numbers**. Where
[`diff model`](model-diff.md) answers *given this change, what can move?*, this answers the
question that follows it: *what actually moved, where did it start, and how much of the headline
does it explain?*

The one-line signal it exists to produce:

```
BEL diverges at t=7 in component renewal_expenses: 10.00 vs 10.31  (Δ 0.31, 3.1%)  [mp POL00042]
```

Three facts in one line, and each is a different piece of work: the **output** is the affected
output whose movement the finding explains, the **component** is the earliest *upstream* component
that diverges, and **t** is the earliest period at which it does. Computing that triple is the
substance of the diff.

Normative source: [`04-verify.md` §5](../design/04-verify.md).

## Either side may be a Prophet run

`A` and `B` are run directories — or a Prophet `.rpt`, which the command imports into a run
directory first. After that there is exactly one comparison path, because a Prophet run *is* a run:
same `results.parquet`, same `results.schema.json`, same `manifest.json` with `system = "prophet"`.

What is Prophet-specific is **declared, never inferred**:

- component correspondence comes from [`migration/mapping.toml`](prophet-readers.md);
- so do `sign`, `scale` and `timing_shift`, and every one of them is printed in the report header;
- the `.rpt`'s own fixed precision raises the tolerance for the columns it wrote, and says so.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | every compared cell is within tolerance |
| `1` | at least one cell is beyond it |
| `2` | the runs are structurally incomparable — different `T`, disjoint modelpoints, or `--require-same-emit` with different component sets |

A differing **component set** is not by itself exit 2. Diffing an `--emit outputs` run against an
`--emit all` run is a routine thing to want: the diff reports `summary.emit_mismatch`, prints a
banner, diffs the intersection, lists the one-sided components, and exits on the intersection's
outcome. `--require-same-emit` restores exit 2 for that case.

## A worked example

Two runs of [`models/term_annual`](reference-models.md), identical but for one assumption —
`expense_inflation` moved from `0.028` to `0.038`:

```console
$ predictable run a/run.pir --out run_a
$ predictable run b/run.pir --out run_b        # b/base.pir: expense_inflation = 0.038
$ predictable diff run run_a run_b --top 2

  a  predictable  2026-08-17T06:48:28Z-8bca5b57 run_a
  b  predictable  2026-08-17T06:48:29Z-61c66f46 run_b
  tolerance  reconcile   abs 0.005  rel 0.000001

  DIVERGED   1,522 of 7,325 cells   1 root divergence   6 outputs affected

  ▸ F001  root   renewal_expenses   affects bel, net_cashflow, profit_margin, pv_expenses, renewal_expenses, reserve   8% of Δreserve
      RESERVE diverges at t=1 in component renewal_expenses: 35.84 vs 36.19  (Δ 0.35, 1.0%)  [mp TA00001]
      first at t=1, persists to t=29, 25 of 25 modelpoints, 475 cells
      exemplar TA00001      worst TA00021 t=29  31.05 vs 41.11  (Δ 10.06, 24.5%)
      explained by a model change: upstream
      → predictable explain run_b --component renewal_expenses --mp TA00001 --t 1   # a: run_a

  ▸ F002  inherited   reserve   affects reserve   100% of Δreserve
      RESERVE diverges at t=0 in component reserve: -223.20 vs -212.50  (Δ 10.70, 4.8%)  [mp TA00001]
      first at t=0, persists to t=29, 25 of 25 modelpoints, 500 cells
      exemplar TA00001      worst TA00021 t=9  549.38 vs 651.85  (Δ 102.47, 15.7%)
      explained by a model change: upstream
      → predictable explain run_b --component reserve --mp TA00001 --t 0   # a: run_a

  … 4 more findings (2 of 6 shown; --top 0 for all)
```

Six components diverge; **one** of them is the root. `renewal_expenses` is the only component that
reads `expense_inflation`, so it is where the change surfaces, and `t = 1` is the first period it
can — `compound(i, 0)` is `1` whatever `i` is. Everything else on the list is downstream of it and
is classified `inherited`, which is the difference between a report you can act on and a list of
six numbers that moved.

The renderer is deterministic to the byte for a given diff: no timestamps, no colour, fixed
thousands grouping. Re-running the same comparison produces the same report.

## Root versus inherited

The classification comes from the IR dependency graph, with the lag on each edge:

- a diverging component whose inputs **at the relevant `t`** all agree within tolerance is a
  **root** divergence;
- one whose inputs also diverge is **inherited** — `x[t-1]` makes `t - 1` the relevant period, not
  `t`, and a `PerMP` aggregate like `npv(...)` reads the whole series.

Every finding carries `class_basis`, so the claim is falsifiable:

| `class_basis` | What it means |
|---|---|
| `ir_graph` | every input of this component was emitted and was checked |
| `partial_graph` | some inputs are `Derived` components this run did not emit, so they could not be checked; run with `--emit all` to close the gap |
| `no_graph_earliest_t` | no model was available at all — the ordering is by earliest `t` and nothing more |
| `override_absorbed` | a `tolerance-only` finding: a loosened tolerance swallowed these cells |
| `component_set` | a `structural` finding: the component exists on one side only |

The model each side ran is found through its own manifest, and **its digest is verified**. A `.pir`
edited since the run is not that run's model, so the diff drops it rather than classify one run's
numbers from another run's formulas — `summary.graph_available` and
`summary.model_attribution_available` say which of the two it lost.

## Explained and unexplained

When both models are available, the run diff cross-references
[`diff model`](model-diff.md). A root divergence in a component the model diff marks as changed —
directly, or through a changed table or assumption it reads — is reported as *explained*. One in an
unchanged component is *unexplained*, and unexplained roots are promoted to the top of the report,
because that is where bugs live.

Findings are ranked: unexplained roots, then explained roots, then inherited, then structural, then
tolerance-only; within each, by `|contribution.share_of_total_delta|` descending. `--top N`
truncates *after* ranking and records `findings_truncated`, so the root never falls off the list.

## Tolerance

Comparison is defined in exactly one place. Two `f64` values match iff

```
|a − b| ≤ abs_tol   OR   |a − b| ≤ rel_tol × max(|a|, |b|)
```

Disjunctive, so `abs_tol` covers values near zero and `rel_tol` covers large ones; and
`max(|a|,|b|)` rather than `|a|`, so `diff A B` and `diff B A` report identically. `NaN` never
matches anything, including `NaN`; `+Inf` matches `+Inf` only; `−0.0` matches `+0.0`. Integers,
booleans, strings, dates and enums compare **exactly** — no tolerance applies.

| `--tolerance-profile` | `abs` | `rel` | Use |
|---|---|---|---|
| `exact` | 0 | 0 | two runs of the same IR on the same engine. Any difference is a bug. |
| `regression` | 1e-9 | 1e-12 | same model, engine upgrade. Guards against reassociation drift. |
| `reconcile` *(default)* | 0.005 | 1e-6 | money reconciliation to the half-cent. |
| `materiality` | 0.01 | 1e-4 | sign-off level: close enough to ship. |
| `--abs N --rel N` | yours | yours | overrules the profile, per-unit table included |

`reconcile` ships the per-unit table of §5.6, because a half-cent tolerance on a probability is
nonsense: `prob` and `factor` are compared at `1e-9`/`1e-10`, `count` at `1e-6`, `money` at the
half-cent.

**A loosened tolerance is never invisible.** Every rule that is not the profile's own is recorded in
`tolerance.overrides_applied`; every rule that permits *more* difference than the profile is
additionally printed in the report header and filed as its own `tolerance-only` finding, with the
cell count it absorbed. `--explain-tolerance` prints the rule that matched each component:

```console
$ predictable diff run run_a run_b --explain-tolerance
  …
  tolerance by component
    model.bel                                abs 0.005        rel 0.000001     by_unit[money]
    model.profit_margin                      abs 0.005        rel 0.000001     profile
```

### Rounding is not tolerance

The engine never rounds implicitly. `round(x, dp)` is round-**half-away-from-zero** on the shortest
decimal representation — `2.675` at 2dp is `2.68`, not the `2.67` a binary-scaled implementation
gives — and reporting rounds for display only, after comparison, never before.

When one side is a Prophet `.rpt` written at fixed precision, the diff reads the precision back off
the values themselves and raises that component's `abs_tol` to `0.5 × 10⁻ᵈᵖ`, scaled by the
mapping's `scale` (a column reported in thousands is reported to 2dp *of thousands*). The raise is
recorded with the note `tolerance_raised_by_source_precision`. This removes the most common class of
phantom diff without anyone loosening a tolerance by hand. `--no-source-precision` turns it off.

## `diff.json`

`--json` writes one `pvf/1` document to stdout and nothing else; `--out-json <path>` writes the same
document to a file. The two renderings never both go to stdout — a consumer that has to find the
JSON inside a report is a consumer that will eventually parse the report.

The JSON is the product; the terminal output is a projection of it. Nothing is printed that is not
also a field. Abridged, from the run above:

```jsonc
{
  "format": "pvf/1",
  "kind": "rundiff",
  "a": {"path": "run_a", "run": "2026-…-8bca5b57", "system": "predictable",
        "manifest": "sha256:2bdccb…", "engine_version": "0.0.1", "model_digest": "sha256:4fbed8…"},
  "b": {"path": "run_b", "…": "…"},

  "tolerance": {"profile": "reconcile", "abs": 0.005, "rel": 1e-6, "money_dp": 2,
                "by_unit": {"money": {"abs": 0.005, "rel": 1e-6}, "prob": {"…": "…"}},
                "overrides_applied": [
                  {"component": "model.bel", "rule": "by_unit[money]",
                   "abs": 0.005, "rel": 1e-6, "looser": false, "note": null}
                ],
                "loosened": 0},
  "mapping": null,

  "summary": {
    "verdict": "diverged",                       // matched | diverged | incomparable
    "incomparable": [],                          // why, when it is
    "modelpoints": {"a": 25, "b": 25, "common": 25, "only_a": 0, "only_b": 0},
    "components": {"a": 13, "b": 13, "common": 13, "only_a": 0, "only_b": 0},
    "emit_mismatch": null,
    "cells": {"compared": 7325, "diverged": 1522, "absorbed_by_override": 0,
              "max_abs": 102.46749133178355, "max_rel": 1.9362735789438228},
    "outputs": [
      {"component": "model.bel", "a_total": 9902.382978187943, "b_total": 10917.626421298755,
       "abs": 1015.2434431108122, "rel": 0.09299122391019123, "within_tolerance": false}
    ],
    "root_divergences": 1,
    "first_divergence": {"component": "model.renewal_expenses", "t": 1, "mp_key": "TA00001"},
    "graph_available": true,
    "model_attribution_available": true
  },

  "findings": [
    {
      "id": "F001",
      "class": "root",                           // root | inherited | structural | tolerance-only
      "category": "value",                       // value | missing | extra | shape | dtype | nan
      "class_basis": "partial_graph",
      "component": "model.renewal_expenses",
      "source_component": null,                  // the a-side name when a mapping renamed it
      "affects_outputs": ["model.bel", "model.net_cashflow", "model.reserve", "…"],
      "t_first": 1,
      "t_range": [1, 29],
      "n_modelpoints": 25,
      "n_cells": 475,
      "exemplar": {"mp_key": "TA00001", "mp_row": 0, "t": 1,
                   "a": 35.83772482497012, "b": 36.186340825213016,
                   "abs": 0.34861600024289885, "rel": 0.009633911368015384, "category": "value"},
      "worst": {"mp_key": "TA00021", "mp_row": 20, "t": 29, "…": "…"},
      "contribution": {"output": "model.reserve", "share_of_total_delta": 0.08195947998397694,
                       "method": "delta_sum_ratio"},
      "explained_by_model_change": {"changed": true, "what": ["upstream"]},
      "message": "RESERVE diverges at t=1 in component renewal_expenses: 35.84 vs 36.19  (Δ 0.35, 1.0%)  [mp TA00001]",
      "hypotheses": [],
      "explain_command": "predictable explain run_b --component renewal_expenses --mp TA00001 --t 1"
    }
  ],
  "findings_truncated": {"kept": 2, "of": 6},
  "structural": {"only_in_a": [], "only_in_b": [],
                 "modelpoints_only_in_a": [], "modelpoints_only_in_b": []}
}
```

Notes for a consumer:

- **`exemplar` is deterministic**: the lowest `mp_row` exhibiting the divergence at `t_first`. Two
  runs of the diff over the same data produce byte-identical JSON.
- **Floats round-trip exactly.** `serde_json`'s `float_roundtrip` is on, so the value in the report
  is the value in the run, to the last bit.
- **`contribution.method`** says how the share was computed: `identity` when the component *is* the
  output, `delta_sum_ratio` otherwise — exact for a component the output sums linearly, an
  approximation elsewhere. Stated, so it is never mistaken for an exact attribution.
- **`hypotheses` carries the §5.5 detectors' proposals** — `H0101` constant ratio, `H0401` the
  Prophet `/12` idiom, `H0301` a table-policy boundary, and the rest — each with its evidence, its
  support over the population, and an applicable `suggested_edit` where the model source anchors
  one. See [Hypotheses and mutation testing](hypotheses.md). The array is always present, so a
  consumer never has to branch on the key existing.

## Prophet reconciliation, end to end

```console
$ predictable diff run TERM_BASE.rpt run_a/ --mapping migration/mapping.toml

  a  prophet      TERM_BASE_2026Q2       /tmp/predictable-rpt-a-90186cc3d4e4f027
  b  predictable  2026-08-17T06:50:29Z-b6978aa1 run_a
  tolerance  reconcile   abs 0.005  rel 0.000001   mapping migration/mapping.toml
  component sets         both `outputs`, but the component sets differ — diffing the 2 in common

  DIVERGED   500 of 2,050 cells   1 root divergence   1 output affected

  ▸ F001  root   renewal_expenses   affects bel, net_cashflow, profit_margin, pv_expenses, renewal_expenses, reserve   100% of Δrenewal_expenses
      RENEWAL_EXPENSES diverges at t=0 in component renewal_expenses: 42.84 vs 40.80  (Δ 2.04, 4.8%)  [mp TA00001]
      first at t=0, persists to t=29, 25 of 25 modelpoints, 500 cells
      exemplar TA00001      worst TA00003 t=0  63.00 vs 60.00  (Δ 3.00, 4.8%)
      → predictable explain run_a --component renewal_expenses --mp TA00001 --t 0

  only in a   prophet.reserve_int  (declared unmapped)
  only in b   model.bel  (unmapped)
  only in b   model.deaths  (unmapped)
  …
```

The `.rpt` carried three columns; the mapping named two of them and declared the third to have no
counterpart. Everything the file did not report is listed as one-sided rather than quietly dropped,
and the two columns it did report are compared — the diff exits on *those*, not on the set
difference. `--period-base 1` (or `period_base = 1` in the mapping, as here) settles the 1-based
periods once, at import.

A mapping that carries adjustments prints them, and so does a tolerance the source's precision
raised:

```
  mapping adjustment     PREM_INC → premium_income  × 1000
  tolerance loosened     model.premium_income  source precision 2dp  abs 5  rel 0.000001  (tolerance_raised_by_source_precision)
```

The `× 1000` is a **declared claim** from the mapping file, printed because a scale factor applied
invisibly inside a comparison would be indistinguishable from a bug. The loosened tolerance on
`premium_income` is printed for the same reason: 2dp *of thousands* is half a currency unit either
side, and a diff that swallowed that silently would be a diff you could not trust when it said
`MATCHED`.

## Command reference

```
predictable diff run <a> <b>
    --tolerance-profile exact|regression|reconcile|materiality
    --abs N --rel N              overrule the profile, per-unit table included
    --mapping migration/mapping.toml
    --period-base 0|1            for a .rpt whose periods are 1-based
    --top N                      keep N findings after ranking (0 = all; default 20)
    --component <name>           report only this component (qualified or bare)
    --mp <key>                   compare only this modelpoint
    --fail-on any|root|never     which findings make the exit code 1 (default any)
    --require-same-emit          a differing component set becomes exit 2
    --explain-tolerance          print the rule that matched each component
    --no-source-precision        do not raise tolerances from a source's own precision
    --model-a <p> --model-b <p>  models to classify from, when the manifests' paths have moved
    --json  --out-json <path>
```

`--fail-on` narrows the exit code without ever hiding a finding: the report is identical either way.
`--fail-on root` is the CI gate for "no *new* divergence"; `--fail-on never` is for a report you want
to read rather than gate on.

## What this does not do

- **It does not re-run anything.** Both sides are already-computed run directories; the diff reads
  results and manifests and never invokes the kernel. To drill into a single cell, follow the
  `explain_command` on each finding into [`predictable explain`](explain.md).
- **It proposes, but it does not apply.** The §5.5 detectors fill `hypotheses`, including literal
  edits, but nothing is ever written to a model by the diff itself — see
  [Hypotheses and mutation testing](hypotheses.md).
- **It does not compare aggregates or solves.** `aggregates.parquet` and `solves/*.parquet` are not
  yet part of the comparison; the per-`(mp, component, t)` result set is.
