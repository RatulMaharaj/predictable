# Verify-layer formats

Condensed from `docs/design/04-verify.md` §3, §5 and §6. Everything here has a terminal rendering,
and **the JSON is the normative form**: nothing is printed that is not also a field, so always pass
`--json`.

## `diff.json` (run diff)

```jsonc
{
  "format": "pvf/1", "kind": "rundiff",
  "a": {"system": "prophet", "path": "...", "run": "...", "manifest": "sha256:...",
        "model_digest": "sha256:...", "engine_version": "..."},
  "b": { ... },
  "tolerance": {"profile": "reconcile", "abs": 0.005, "rel": 1e-6,
                "by_unit": {...}, "by_component": {...},
                "overrides_applied": [{"component": "...", "rule": "...", "looser": false}],
                "loosened": 0},
  "structural": {"only_in_a": [], "only_in_b": [],
                 "modelpoints_only_in_a": [], "modelpoints_only_in_b": []},
  "summary": {"verdict": "matched" | "diverged" | "incomparable",
              "cells": {"compared": 15550, "diverged": 2600, "max_abs": ..., "max_rel": ...},
              "root_divergences": 1, "graph_available": true,
              "model_attribution_available": true,
              "first_divergence": {"component": "...", "mp_key": "...", "t": 0},
              "outputs": [{"component": "model.bel", "a_total": ..., "b_total": ...,
                           "abs": ..., "rel": ..., "within_tolerance": false}]},
  "findings": [ ... ]
}
```

`loosened` and `overrides_applied[].looser` are how you check that nothing widened a tolerance.
`loop.py` asserts both.

### A finding

```jsonc
{
  "id": "F001",
  "component": "model.premium_rate",
  "class": "root" | "inherited",
  "class_basis": "ir_graph",          // or a weaker basis, when the graph was unavailable
  "category": "value",
  "affects_outputs": ["model.bel", "model.reserve", ...],
  "explained_by_model_change": {"changed": true, "what": ["init"]},
  "contribution": {"method": "delta_sum_ratio", "output": "model.reserve",
                   "share_of_total_delta": -0.687},
  "exemplar": {"mp_key": "TA00001", "mp_row": 0, "t": 0, "a": 151.39, "b": 155.93,
               "abs": 4.54, "rel": 0.0291},
  "explain_command": "predictable explain b/ --component premium_rate --mp TA00001 --t 0",
  "hypotheses": [{"code": "H0201", "confidence": "high",
                  "evidence": {"shift": 1, "t_matched": 40},
                  "evidence_support": {"held": 24, "tested": 24},
                  "message": "...",
                  "suggested_edit": {"file": "...", "byte_start": 2292, "byte_end": 2338,
                                     "old": "...", "new": "..."}}]
}
```

Reading discipline:

* **`class`** — only `root` findings are actionable; `inherited` ones disappear when their root is
  fixed. `class_basis` tells you how the call was made: `ir_graph` means both models' dependency
  graphs were available and verified, and is the strong form. Anything weaker, discount accordingly.
* **`explained_by_model_change`** — the structural diff independently agrees. Two methods
  concurring is what makes a localisation trustworthy rather than an arithmetic coincidence.
* **`contribution.share_of_total_delta`** — how much of the headline output's movement this finding
  accounts for. Order your work by it. It can be negative (offsetting movements).
* **`suggested_edit`** — the same `{file, byte_start, byte_end, old, new}` shape as a checker
  suggestion, so one code path applies both.

The one-line terminal form is
`<output> diverges at t=<t> in component <component>: <a> vs <b>  (Δ <abs>, <rel>%)  [mp <key>]` —
the affected *output*, the earliest upstream *component* that diverges, and the earliest `t`.

Exit codes: `0` within tolerance, `1` diverged, `2` incomparable.

## Tolerance profiles

| Profile | `abs` | `rel` | Use |
|---|---|---|---|
| `exact` | 0 | 0 | same IR, same engine — any difference is a bug |
| `regression` | 1e-9 | 1e-12 | same model, engine upgrade |
| `reconcile` | 0.005 | 1e-6 | **money reconciliation to the half-cent — the migration default** |
| `materiality` | 0.01 | 1e-4 | sign-off: "close enough to ship" |

Match predicate, applied per cell: `|a−b| ≤ abs` **or** `|a−b| ≤ rel × max(|a|,|b|)`. Disjunctive
and symmetric. `NaN` matches nothing, including itself. Integers, booleans, strings, dates and enums
compare exactly — no tolerance applies.

Choosing a *looser named profile*, or hand-setting `--abs`/`--rel`, to clear a finding is the
prohibited move. The one legitimate case is `H0402` (the whole difference is rounding), and even
then it is a documented profile choice you put to the user, not a number you nudge.

## `explain()` trace

```
predictable explain run/ --component reserve --mp POL00042 --t 3 [--depth 3] [--json]
```

The JSON is normative; the tree text and the HTML are renderings of it. Each node carries its value,
its unit and timing, its span in the `.pir`, and — for a suppressed `if` arm — `"suppressed": true`.
`notes` carries the `N0xxx` provenance codes: an `init` used at `t = 0`, a pre-origin default, a
clamped lookup. Read the notes before disputing a hypothesis; they explain why one cell behaves
unlike its neighbours.

Run it on **both** sides and compare. A hypothesis proposes a shape; a trace shows the arithmetic.

## Run manifests

Every run directory has `manifest.json`: the model digest, the assumption-set digest, every input
file with its SHA-256, the engine version and git sha, the CLI version, the timeline, the emit
setting, and the run id.

* A Prophet-imported run has `"system": "prophet"`, `inputs.modelpoints.source = {system, file,
  digest}`, and **explicit `null`/`"unknown"`** for everything the `.rpt` did not disclose — never
  omitted, never invented. Component `timing` is `null` for the same reason.
* `predictable rerun manifest.json` verifies every input digest before re-executing, so a
  reconciliation can be reproduced by someone who was not there.
* Keep both manifests in the hand-back report; they are what makes the comparison re-executable.
