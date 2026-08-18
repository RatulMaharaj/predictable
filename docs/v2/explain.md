# Explaining a number

`explain()` answers the only question that matters when a valuation is wrong: **where did this
number come from?** It replays one modelpoint and returns the whole provenance tree behind one
cell — every reference, every lag, every table lookup, every discount factor — down to the
modelpoint columns and assumptions it ultimately rests on.

It is specified by [`04-verify.md` §3](../design/04-verify.md) and
[`01-ir.md` §11.2](../design/01-ir.md).

## The one property everything rests on

A trace is a **replay**, not a tape.

Projection is deterministic ([Determinism and goldens](determinism.md)), so the engine can
re-project a single modelpoint through a fresh single-lane kernel and be certain it will get the
same numbers back. That is what makes tracing free: there is no "tracing mode" on a run, nothing
in the hot loop records anything, and a run that never calls `explain()` pays exactly nothing.

It is also what makes a trace *evidence*. Every leaf of the tree — every `Ref`, `Lag` and `At` —
reports the value the vectorised kernel actually stored, read out of its buffers. The interior
arithmetic is recomputed (the kernel keeps no sub-expression values) using the same `ops` module
the hot loop calls, and then **checked**: the replayed value of every component in the tree is
compared bit-for-bit with the slot the kernel wrote. A mismatch is
[`E0901`](../llm/diagnostics.md#E0901), an engine bug, and it is raised rather than rendered.

So "this test passed" and "the trace reproduces the run" are the same statement.

## From the command line

```
predictable explain <run/> --component <name> --mp <key> [--t N]
```

Run it from the directory the run was launched from; the manifest names the `[run]` file, and its
paths resolve there.

```console
$ cd models/term_annual
$ predictable explain runs/base --component bel --mp TA00001 --depth 1
model.bel  TA00001  = -223.20  money
│ pv_claims + pv_expenses + initial_expense - pv_premiums
│
└─ -  -223.202
   ├─ +  697.2599
   │  ├─ +  492.8834
   │  │  ├─ model.pv_claims  217.49                                                            money
   │  │  │  └─ model.pv_claims  217.49                                                         money
   │  │  │     └─ npv(model.death_claims)  41 term(s)  217.494
   │  │  │           t=0               32.11  disc 0.966184  → 31.02
   │  │  │           t=1               28.27  disc 0.933511  → 26.39
   │  │  │           t=2               26.32  disc 0.901943  → 23.74
   │  │  │           ...
   │  │  │           t=40               0.00  disc 0.252572  → 0.00
   │  │  └─ model.pv_expenses  275.39                                                          money
   │  │     └─ …
   │  └─ model.initial_expense  204.38                                                         money
   │     └─ model.initial_expense  204.38                                                      money
   │        └─ *  204.3765
   │           ├─ initial_expense_pct  assumption[base]  1.35000                               factor
   │           └─ annual_premium  modelpoint  151.39                                            money
   └─ model.pv_premiums  920.46                                                                money
      └─ …
```

Read the `npv` block first. Those per-`t` contributions are not decoration: a BEL that is 3% wrong
tells you nothing, but a `disc` column whose exponent is `v^t` where it should be `v^(t+1)` tells
you everything at a glance. They are mandatory on every aggregate node
([`01-ir.md` §11.2, Q11](../design/01-ir.md)), they are in ascending `t` with no gaps, and they sum
left to right to the aggregate's value **exactly** — because the recorder replays the same
sequential reduction the kernel did rather than re-adding them in a different order.

### A period of a series

`--t` is required for a `Series` component and refused for a `Scalar` or `PerMP` one — a `t` on a
value that does not vary with `t` would be a lie about the model.

```console
$ predictable explain runs/base --component qx --mp TA00005 --t 40 --depth -1
model.qx[t=40]  TA00005  = 0.192071  prob end
│ mortality@(age, sex, smoker) * mortality_loading
│
└─ *  0.1921
   ├─ mortality@(age=96, sex=M, smoker=0)  0.1829
   │  ├─ model.age  t=40  96                                                            years  start
   │  │  └─ model.age[t=40]  96                                                         years  start
   │  │     └─ +  96
   │  │        ├─ entry_age  modelpoint  56                                                    years
   │  │        └─ t  timeline  40
   │  ├─ sex  modelpoint  "M"
   │  └─ smoker  modelpoint  0
   └─ mortality_loading  assumption[base]  1.05000                                            factor
```

### Options

| Flag | Meaning |
|---|---|
| `--component <name>` | Bare or qualified (`bel` or `model.bel`). A `[[solve]]` name resolves to its `vary` field. |
| `--mp <key>` | The modelpoint key. |
| `--t N` | Required for a `Series`, refused otherwise. |
| `--depth N` | Levels of `Ref` expansion; `-1` expands to the leaves. Default `2`. |
| `--expand a,b` | Expand these components fully whatever `--depth` says. |
| `--values-only` | Drop spans and type metadata for a compact trace. |
| `--trace-max-terms N` | Aggregate term budget (default 4096); truncation keeps both ends and says so. |
| `--width N` | Terminal width for the right margin. |
| `--json` | Write the trace document instead of the rendering. |

## From Python

```python
from predictable_engine import Program

plan  = Program.from_pir("models/term_annual").plan()
trace = plan.explain("bel", "TA00001")

trace.value                 # -223.2024…  — the same number the run wrote
print(trace.text)           # the rendering above
trace.json["root"]["expr"]  # 'pv_claims + pv_expenses + initial_expense - pv_premiums'
trace.notes                 # [{'code': 'N0202', 'message': '…', 'component': 'model.reserve', …}]
```

A `Trace` renders itself in a notebook (`_repr_html_`), and `trace.show()` prints it. Series
components take a period, and the same rules apply:

```python
plan.explain("reserve", "TA00003", 5)            # a Series at t = 5
plan.explain("reserve", "TA00003", 5, depth=-1)  # …all the way to the leaves
plan.explain("claims", "TA00003", 5, expand=["survivors"])
```

## The JSON is normative

The text is a **pure projection** of the JSON: no fact appears in the rendering that is absent from
the document, and the same tree always renders identically, so a rendered trace can be committed as
a golden. The schema is [`04-verify.md` §3.2](../design/04-verify.md); the node variants are a
closed set mirroring `Expr` ([IR concepts](ir-concepts.md)) one-to-one:

`Component`, `Binary`, `Unary`, `Ref`, `Lag`, `At`, `Input`, `Lit`, `If`, `Lookup`, `Call`, `Agg`.

```jsonc
{
  "format": "pvf/1",
  "kind": "trace",
  "run": "sha256:cf52d6e8…",          // the manifest digest of the run explained
  "root": {
    "node": "Component",
    "path": "root",
    "id": "model.qx",
    "t": 40,
    "value": 0.19207…,
    "dtype": "f64", "unit": "prob", "timing": "end", "shape": "Series", "stage": 1,
    "expr": "mortality@(age, sex, smoker) * mortality_loading",
    "modelpoint": {"key": "TA00005", "row": 4},
    "children": [ /* Expr nodes */ ]
  },
  "notes": [ /* N0xxx */ ],
  "truncated": {"depth": -1, "elided_nodes": 0}
}
```

Every node carries `path` (`root.children[0].children[2]`), which is the join key notes attach to
and the key a run diff aligns two traces on.

### What a `Ref` expands into

A `Ref` node's `children` is the **evaluated tree of the referenced component at that `t`**, not a
placeholder. That is what makes this a provenance trace rather than a one-level formula dump: at
`--depth -1` the tree bottoms out entirely in `Input` and `Lit` leaves, and the arithmetic in it
reproduces the cell exactly.

Recursion across periods terminates by itself, because a `Lag` decrements `t`. At `t < 0` the node
says how the value was resolved:

| `resolution` | Meaning |
|---|---|
| `computed` | The value the kernel stored for that period. |
| `init` | Below the origin, or at `t = 0`, with an `init` declared — `init_expr` names it. |
| `pre_origin_default` | No `init`: the zero of the dtype (`01-ir.md` §2.7). Raises `N0301`. |

### Notes — the "look here" channel

Traces carry `notes`, a flat list of coded observations pinned to node paths. They are how the
verify layer says *this is probably where your model is wrong*, and they are the first thing a
diffing agent reads.

| Code | Fires when |
|---|---|
| [`N0101`](../llm/diagnostics.md#N0101) | A lookup key was clamped to the table's domain. |
| `N0102` / `N0103` | A key was stepped to a band, or interpolated between two rows. |
| `N0104` | A lookup missed and used `on_missing = default(...)`. |
| `N0201` | `retime` was applied — a timing cast, value unchanged. |
| `N0202` | A `start`-timed value was added to an `end`-timed one. |
| `N0301` / `N0302` | A pre-origin default was used; an `init` was used at `t = 0`. |
| `N0401` | The value is exactly `0.0` and a factor in the product chain is `0.0` — the "why is my BEL zero" note, naming the responsible factor. |
| `N0402` | A `money` value outside `1e-12 … 1e12` — a probable scaling error. |
| `N0403` | A denominator within `1e-12` of zero: a near-trap. |
| `N0501` | One branch of an `If` is never taken for any `t` in this projection. |

`N0401` earns its place. The single most common migration failure is a whole component collapsing
to zero because an in-force count ran off earlier than the model it is being compared against, and
this note points straight at the factor responsible.

## Cost, and what it is not

A trace is one modelpoint projection plus tree allocation — single-digit milliseconds on a
480-period model. Three consequences:

* **There is no tracing flag on a run.** Ask for a trace when you want one.
* **A trace never perturbs a run.** The replay uses a separate engine instance; running a model
  before and after explaining it gives bit-identical results.
* **The replay is planned with full retention** (`--retain-all`), because a ring buffer has already
  overwritten the history a trace at `t` asks about. Retention is a storage decision and cannot
  change a value ([The kernel](kernel.md) §4.3), which is exactly what makes the `E0901` check
  meaningful rather than circular.

A `[[solve]]` model is replayed *after* its solves, because the solver writes the solved `vary`
value back into the modelpoint columns; replaying the file on disk would explain a projection
nobody ran.

## Reading a wrong number, end to end

The workflow the whole verify layer is built around:

1. A component's value looks wrong. `explain` it at the period where it first looks wrong.
2. Read the `notes` before the tree — a clamped lookup or an `N0401` zero usually is the answer.
3. Walk down the tree until the children stop being what you expect. Because a `Ref` expands into
   the referenced component, this is one continuous descent rather than a series of separate
   queries.
4. The first node whose inputs are right and whose output is wrong is the bug.

Step 4 is what [run diff](../design/04-verify.md) automates across two runs; `explain` is what it
automates *with*.
