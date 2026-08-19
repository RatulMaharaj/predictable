# The modelpoint drill-down

The drill-down is the screen you open when a single number is wrong and you need
to know why. It is three views of the same data at three zoom levels:

| View | What it shows |
|---|---|
| **Grid** | every `Series` component of one modelpoint, every period, virtualised |
| **Waterfall** | the flows at one period, in declaration order, each with its timing |
| **`explain()` panel** | the engine's provenance trace for one component at one period |

Selecting a grid cell moves the waterfall and opens the trace; clicking a
waterfall bar moves the trace; hovering a trace row highlights the bar. Nothing
on this screen computes an actuarial number — every value on it came out of the
engine, and the trace is rendered exactly as `explain()` serialised it.

Specification: [05-viz.md §3.1, §3.4, §4.3](../design/05-viz.md).

## Opening it

The screen renders whatever `DataSource` the page was given ([the graph document
and viz server](viz-server.md) describes the contract). It needs a source that
carries results: `series()` for the grid and `explain()` for the trace, with
`capabilities().explain` true.

```bash
predictable serve models/term_annual          # the model explorer
```

!!! note "Current status"

    `predictable serve` today serves a **model**, not a run: it reports
    `explain: false` and holds no series, so it opens on the model explorer. The
    drill-down is wired to the `DataSource` contract and to the trace schema, and
    is served by any results-backed host — the notebook `results.show()` path and
    the static export of §1.4. Pointing `serve` at a run directory is that
    command's own piece of work.

The screen is addressed by the query string, with the server token staying in
the fragment:

```
http://127.0.0.1:7391/?view=mp&run=runs/base&mp=TA00001&t=3&c=model.reserve#<token>
```

`view=mp` with an `mp=` key selects the drill-down; anything else stays on the
[model explorer](viz-server.md).

## The URL is the screen

Every piece of state is in the URL, including which trace rows are folded shut,
so pasting a link into a review comment reproduces the exact screen.

| Key | Meaning |
|---|---|
| `view` | `portfolio`, `group` or `mp` — the drill-down level |
| `run` | the run directory or id being viewed |
| `group` | the ordered key tuple of a declared `[[aggregation]]`, e.g. `product=TERM&cohort=2019` |
| `mp` | the modelpoint key |
| `t` | the selected period |
| `c` | the selected component — the entry point to `explain()` |
| `collapsed` | comma-separated trace node paths that are folded |
| `sort=magnitude` | the waterfall's sort toggle (default is declaration order) |
| `heat=1` | the grid's heat overlay |

`t=0` is a real selection, not an absent one; it survives a round trip.

## The grid

Components on rows in the model's **declaration order** (IR §4.1 rule 2), `t` on
columns. Only the visible window is mounted, so a 50,000 × 480 projection scrolls
at the same cost as a 4 × 4 one.

```
                t=0     t=1     t=2     t=3 ◀   t=4
num_pols_if   1.0000  0.9721  0.9447  0.9178  0.8914
premium_inc   1240.0  1265.7  1291.4  1317.3  1343.4
death_claims     —     134.2   138.9   143.7   148.6
reserve       1084.2  1131.8  1180.1  1284.6▮ 1341.0
```

- **Keyboard**: arrows move, `Shift`-arrows extend the selection, `Home`/`End`
  jump to the first and last period, `PageUp`/`PageDown` to the first and last
  component, `Enter` opens `explain()` for the focused cell.
- **`⌘C` copies TSV** that pastes into Excel unmangled: a `component` header, one
  `t=` column per period, and the engine's **full `f64`** in every cell — a copy
  that rounds is a copy that lies. A period the component has no value at copies
  as an empty cell, never as `0`.
- **`—` is not zero.** An absent value is drawn as an em-dash, and hovering any
  cell shows its exact `f64` because "it matched to 2 dp" is not a reconciliation.
- **Heat** (`heat=1`) is scaled per row, because a row is one component in one
  unit; a flat row and an empty cell get no heat rather than false heat.

Copied from the grid, three cells of the term-assurance model look like this:

```tsv
component	t=1	t=2
premium_income	1265.7	1291.4
death_claims	134.2	138.9
reserve	1131.8	1180.0999999999999
```

(the last cell is what an `f64` sum really is — the grid shows `1,180.1` and
copies the value the engine holds)

## The waterfall

Per modelpoint at the selected `t`, hand-drawn SVG:

- Bars are in **declaration order** — authorial intent, not magnitude — with an
  opt-in "sort by magnitude" toggle.
- Every bar carries a **timing glyph and the timing word**: `start ▙`, `end ▟`,
  `mid ◧`, `point ◆`, `untimed ·`. Timing mismatches are the single most common
  Prophet reconciliation error, so a chart that hides timing hides the bug. When
  more than one timing is summed, the chart says so in words.
- The connector line carries the running total; the selected component closes the
  chart as an Output-styled bar drawn from zero.
- Clicking (or tabbing to and pressing `Enter` on) a bar explains that component
  at that `t`. Every bar's tooltip carries the exact value, its timing and the
  running total.
- "chart as table" renders the same numbers as a table — both an accessibility
  feature and the auditor's view.

## The `explain()` panel

The panel renders the trace tree of [04-verify §3.2](../design/04-verify.md)
**verbatim**: the engine's node order, the engine's values, the engine's notes.
Generate the same document on the command line with:

```bash
predictable explain models/term_annual/runs/base \
  --component reserve --mp TA00001 --t 3 --json
```

```jsonc
{
  "format": "pvf/1",
  "kind": "trace",
  "run": "sha256:e2c34ad6…",
  "root": {
    "node": "Component", "path": "root", "id": "model.reserve", "t": 3,
    "modelpoint": {"key": "TA00001", "row": 0},
    "value": -31.48375754036055, "unit": "money", "shape": "Series",
    "expr": "(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + valuation_rate) * if(t <= policy_term, 1.0, 0.0)",
    "children": [ /* Binary, Ref, Lag, Lookup, If, Lit, Input, Agg … */ ]
  },
  "notes": [
    {"code": "N0202", "severity": "info",
     "path": "root.children[0]…children[0]",   // elided for the page
     "message": "timing-mismatched +: `reserve[t-1]` is point, `premium_income[t-1]` is start",
     "component": "model.reserve", "t": 2}
  ],
  "truncated": {"depth": 2, "elided_nodes": 25}
}
```

What the panel adds is affordances, never facts:

- **Every row is clickable.** A `Ref`, `Lag` or `At` navigates to that
  component's own trace at that `t`; a span opens the `.pir` at that line; a
  `Lookup` opens the table viewer at the resolved row.
- **Notes are attached to their own node** by `path`, and tint that row. `N0101`
  (clamped key), `N0102` (stepped key) and friends are exactly where silent
  wrongness lives, and the row says so.
- **Lookup policies that fired get a warning badge** — `age clamp 121 → 120` —
  and the ones that did not fire get nothing, so the badge always means something.
- **Pre-origin reads are badged**: `t=-1 before origin → used init = bel`, or
  `t=-1 before origin → no init, used 0`. Off-by-one at `t = 0` is a real bug
  class and it is now visible.
- **`retime` is labelled a timing cast**, `start → mid, value unchanged` — the
  audit-relevant no-op.
- **`Agg` nodes show every per-`t` contribution** (IR §11.2): `t`, `x`, the
  discount factor after the timing exponent, and the contribution, which sum
  left to right to the aggregate exactly.
- **Truncation is never silent.** `25 nodes elided` under the tree, and
  `n children elided` on the node that was cut.

Fold a subtree and the fold goes into the URL, so a colleague opening your link
sees the same shape of tree you did.

## What it will not do

- It will not invent a grouping the model did not declare. Level 1 renders only
  the `[[aggregation]]` blocks of the run file (IR §8.3); adding a grouping means
  editing the model, which means it shows in the model diff.
- It will not recompute or interpolate. If a number is on the screen, the engine
  produced it.

## Testing

The screen's tests run with the rest of the SPA:

```bash
cd crates/predictable-viz/frontend && npm test
```

The trace panel's tests run against **real engine output** — fixtures produced by
`predictable explain` on `models/term_annual` and checked in under
`src/drilldown/__fixtures__/` — so the panel and the trace schema cannot drift
apart without a red test.
