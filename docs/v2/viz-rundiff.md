# The Run Diff screen

This is the reconciliation screen: two runs went in, the numbers moved, and the
question is *which line of the model moved them*. It is the
[verification loop](verification-walkthrough.md) with a mouse instead of a
terminal, and it is deliberately arranged in the order a reviewer should read it.

| Band | What it answers |
|---|---|
| **Reconciliation banner** | were these two runs even given the same inputs? |
| **Output movements** | which reported numbers moved, and by how much |
| **Findings list** | which components moved, and which one is the *root* |
| **Attribution panel** | *why* it moved — the model diff's own answer, plus the hypotheses |
| **Contributor bars** | which modelpoints carry the movement, and what they share |
| **Side by side** | one modelpoint, both grids, both `explain()` traces |

Specification: [05-viz.md §4.2](../design/05-viz.md).

## Opening it

The screen renders whatever `DataSource` the page was given (see
[the data source contract](viz-datasource.md)). It needs `diff(a, b)` — the local
server's `/api/diff`, or a governance pack exported with `--include diff:<run>`.
A source that cannot diff says so and tells you which flag to add, rather than
rendering an empty screen.

```
http://127.0.0.1:7391/?view=diff&a=<run-a>&b=<run-b>#<token>
```

!!! note "Current status"

    The server exposes `GET /api/diff?a=&b=`, and the in-memory source it is
    built on answers `501 unsupported` unless the host loaded a diff — the same
    honest-501 shape the drill-down meets. So today the screen is reached from a
    host that carries one: the notebook path, or a pack exported with
    `--include diff:<run>`. The screen itself is complete against the contract
    and against the real `diff.json` this page quotes; wiring
    `predictable serve` to a pair of run directories is that command's own piece
    of work.

`view=diff` with both `a=` and `b=` selects Screen 2; anything else falls
through to the [drill-down](viz-drilldown.md) or the
[model explorer](model-explorer.md). The run ids are the ones
`predictable diff run` prints.

## 1. Reconciliation comes first

Before a single number is rendered, the banner compares what the diff document
recorded about each side: system, model digest, manifest digest, engine version,
and the modelpoint and component sets.

```
A  predictable  2026-08-17T08:08:59Z-6b512e30  good/runs/base
B  predictable  2026-08-17T08:08:59Z-1e9bd949  bad/runs/base        DIVERGED

✓ system        predictable
✕ model digest  1c7a63b → 972877f
✕ manifest      eb888af → 14fab3b
✓ engine        0.0.1
✓ modelpoints   25
✓ components    13
```

Half of all "the numbers don't match" incidents are a different modelpoint file.
A difference in the modelpoint or component sets is marked **blocking**: the
screen says, in words, that the two runs were not given the same inputs and that
the numbers below should be read only after that is understood. A model-digest
difference is not blocking — that is the thing you are usually here to study.

Two honest cases the banner handles rather than papering over:

* **A Prophet side A.** A `.rpt` read into the results schema has no predictable
  model digest, so that row reads `unknown`, never `different`.
* **An emit mismatch.** If one side wrote `emit = "outputs"` and the other
  `emit = "all"`, the banner says so and repeats the walkthrough's advice: the
  root divergence can only be localised to a component that was written down.

## 2. Findings, roots first

The list is the diff's own `findings[]`, reordered only for reading: `root`
findings first, then by the share of the movement the diff itself attributed to
them. Nothing is re-classified and no delta is recomputed — `root` versus
`inherited` is the run diff's decision, made against the model graph
(04-verify §5.3), and the screen shows which basis it used.

On the walkthrough's scenario the list is one root (`premium_income`) and five
inherited components, including `reserve` — which accounts for 100 % of the
`reserve` movement and is still not the cause.

## 3. Attribution — the sentence that ends the argument

The left panel turns the finding into a claim about the model:

> **root** `premium_income`
> an upstream change to premium_income explains this movement
> model diff: upstream
> impact set: 5 components, reaching bel, net_cashflow, profit_margin, pv_premiums, reserve

The first line is `finding.explained_by_model_change` from `diff.json`; the
impact line is the transitive downstream closure of the planner's own graph.
Below them sit the hypotheses (`H0101`, `H0201`, …) with their confidence, their
`held on 24/24` support, and the suggested edit rendered as a patch.

If the diff was run without `--model-a`/`--model-b`, the panel is **silent**, not
empty: it says no model diff was supplied and tells you how to supply one.
Claiming "no model change" from a missing model diff would be a lie, and
04-verify §5.4 is explicit about the distinction.

## 4. Contributor bars and the cohort

The diff document carries an exemplar and a worst case per finding, not every
diverging cell. The per-modelpoint ranking is therefore **derived in the
browser** from both runs' series, and it says so on screen, next to the method:

```
derived in the browser · Σ|b − a| over t per modelpoint, from both runs' series;
|b − a| > 0.005 counts as a diverging cell
```

Under the bars sits the cohort hypothesis: a depth-2 decision-tree split over the
*declared* modelpoint fields, looking for the predicates that best separate the
differing modelpoints from the matching ones. It is labelled `HYPOTHESIS`, it
reports its own precision and recall, and — importantly — it is allowed to fail:

* every modelpoint differs → *"all 25 modelpoints differ — there is no cohort,
  the change bites everywhere"* (this is the walkthrough's case: a premium
  escalation applied a year early bites every policy);
* no predicate separates the two sets → the screen says exactly that.

A fabricated cohort that covers 60 % of the differing set is worse than silence,
so nothing is reported below an F1 of 0.8.

!!! note "String-valued fields"

    The series wire is `Float64`-valued, so the split is offered over numeric and
    boolean modelpoint fields (`entry_age`, `smoker`, `sum_assured`, …). A
    string-valued field such as `sex` is simply not offered rather than being
    silently encoded into a number.

## 5. Side by side, with both traces

Clicking a contributor — or the exemplar button in the attribution panel — opens
the modelpoint on both sides at once:

* two column-aligned grids over the **union** of components and periods, so a
  component present on only one side shows as an empty row rather than shifting
  every column to its right;
* differing cells highlighted with their delta chip, and a "differing rows only"
  filter;
* the first period at which the two grids disagree marked and scrolled into
  view;
* both `explain()` traces stacked, rendered by the same panel the
  [drill-down](viz-drilldown.md) uses — the engine's tree, verbatim.

The traces are aligned *by position*, not by path string. Where the models agree
the trees have the same shape; the deepest node whose shape differs is the edit
itself rather than its consequence, and it is called out above the traces:

> the trees differ in shape inside `model.premium_rate`: Lit is present only on
> side B · at the root A 151.39 vs B 155.93

On the seeded off-by-one that is exactly where the mistake lives: side B's
`premium_rate` used an `init` with an extra `1 + premium_escalation` factor, so
year 0 already carries a year of growth the policy has not lived through. The
root of the trace is `premium_income`, six outputs downstream; the sentence
points past it to the component that was actually edited.

## What the screen refuses to do

* It does not recompute a delta, re-rank a finding, or re-decide `root` versus
  `inherited`. All of that is in `diff.json` and is rendered verbatim.
* Everything it *does* derive — the per-modelpoint bars, the cohort — carries
  the word "derived" and its method, per 05-viz.md §0.
* It never constructs a `DataSource`; it is handed one, so the same screen serves
  the local server, a static governance pack, and a wasm-backed pack.

## Tests

`crates/predictable-viz/frontend/src/rundiff/` is tested against the real
artefacts of the [verification walkthrough](verification-walkthrough.md), not
against hand-written fixtures. `src/rundiff/__fixtures__/` holds the verbatim
output of

```console
$ predictable run  good/run.pir --out good/runs/base
$ predictable run  bad/run.pir  --out bad/runs/base
$ predictable diff run good/runs/base bad/runs/base \
      --model-a good/build --model-b bad/build --json      > diff.json
$ predictable explain good/runs/base --component premium_income \
      --mp TA00001 --t 0 --json                            > trace-a.json
$ predictable explain bad/runs/base  --component premium_income \
      --mp TA00001 --t 0 --json                            > trace-b.json
```

plus both runs' `results.parquet` pivoted into the `/api/series` wire shape. The
tests assert against the diff's own numbers: the contributor ranking must
reproduce the exemplar cell and agree with `n_modelpoints`, and the top bar must
be the modelpoint the diff called `worst`.

```console
$ cd crates/predictable-viz/frontend && npm test
```
