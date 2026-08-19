# 05 — Visualisation

Status: **normative for v1**. Depends on: `01-ir.md` (IR `1.0`, format `pir/1`) and `04-verify.md`
(formats `pvf/1` — results schema §2, trace schema §3.2, `RunDiffDoc` §5.4, manifest §6). Where this
document names a schema those two define, they win; this document specifies only `GraphDoc` (§2.1).
Audience: whoever builds `predictable-viz`, and LLMs generating governance packs.

The visualisation layer has exactly one job: **close the verification loop faster than reading
numbers in a spreadsheet.** An actuary migrating a Prophet model asks three questions, in this
order, hundreds of times a day:

1. *Where does this number come from?* → graph explorer + `explain()`.
2. *Why did it change?* → run-vs-run diff, attributed to a model change.
3. *Which policy is breaking it?* → portfolio → modelpoint drill-down.

Everything below serves those three. Anything that does not is out of scope for v1.

Non-negotiable constraint inherited from the IR: **the viz layer is a pure consumer.** It reads
`.pir` modules, result sets, run manifests and `explain()` trees. It never computes an actuarial
number itself — no re-deriving NPVs in JavaScript, no client-side re-aggregation with different
float semantics. Every displayed number came out of the engine (§9 determinism), or it is labelled
as a chart-only derived quantity (a bar height, a percentage of total).

---

## 1. Architecture

### 1.1 Three delivery modes, one bundle

```
                     ┌───────────────────────────────┐
                     │  predictable-viz (one SPA)     │
                     │  React + TypeScript, Vite      │
                     └───────────────┬───────────────┘
                                     │ same bundle, three hosts
        ┌────────────────────────────┼────────────────────────────┐
        │                            │                            │
  model.show()                notebook embed              predictable export
  results.show()              (_repr_html_)               --format html
        │                            │                            │
  local axum server           <iframe srcdoc>            single .html file
  :7391, data over HTTP       data inlined as JSON       everything inlined
  + WASM engine for           + WASM if payload          no server, no network
  on-demand explain()         small enough               frozen snapshot
```

**Decision: one SPA, three data-provider implementations.** The app is written against a
`DataSource` interface; the only difference between modes is which implementation is injected.

```ts
interface DataSource {
  manifest(): Promise<RunManifest>;
  graph(): Promise<GraphDoc>;                 // §2.1
  component(name: string): Promise<ComponentDoc>;
  series(q: SeriesQuery): Promise<Float64Array | ColumnBatch>;
  explain(q: ExplainQuery): Promise<ExplainNode>;   // the only *computing* call
  diff(a: RunId, b: RunId): Promise<RunDiffDoc>;
  capabilities(): { explain: boolean; recompute: boolean; sensitivity: boolean };
}
```

- `HttpDataSource` — talks to the local server. All capabilities true.
- `InlineDataSource` — reads a `window.__PREDICTABLE__` payload. `explain` true only if a WASM
  engine and the model+modelpoint were embedded; otherwise pre-baked traces only (§4.3).
- `WasmDataSource` — the engine compiled to `wasm32-unknown-unknown`, holding a loaded run in
  browser memory; used by notebook embeds over small runs and by "live" governance packs.

`capabilities()` drives the UI honestly: a static export greys out "trace this cell" with the
tooltip *"This is a frozen export. Run `predictable show run/2026-06-30` for live traces."* Never a
dead button.

### 1.2 The local server

`model.show()` / `results.show()` start an **axum** server in a background thread of the same
process that already holds the engine — the Rust side of the PyO3 wheel, not a separate Python
web framework. Rationale: the data is already in Rust memory as columnar arrays; going through
Python to serialise it would dominate the latency, and a second process would need the model
loaded twice.

```python
model.show()                      # graph explorer only, no results needed
results.show()                    # results views, graph explorer available
results.show(port=7391, open_browser=True, bind="127.0.0.1")
```

- Binds `127.0.0.1` only. No auth in v1, but a random 32-byte token in the URL fragment is
  required by every API call — enough to stop a drive-by from another local process.
- Lifetime: lives as long as the Python object; `results.show()` in a script blocks with
  `Ctrl-C to stop` unless `block=False`.
- **Transport: JSON for metadata, Apache Arrow IPC for numbers.** `/api/series` returns an Arrow
  record batch, `application/vnd.apache.arrow.stream`. A 5,000-modelpoint × 480-period `f64` series
  is 19 MB as JSON and 2.4 MB as Arrow zero-copy into a typed array. Arrow is also what the
  notebook path already has (`results.to_arrow()`), so there is one wire format, not two.

API surface (stable, documented, and usable directly — it is also how an LLM agent inspects a run
without a browser):

```
GET  /api/manifest
GET  /api/graph?product=term_assurance&depth=all
GET  /api/component/{name}
GET  /api/series?run=<id>&components=premium_income,death_claims&mp=POL00042&t=0..40   -> Arrow
GET  /api/aggregate?run=<id>&measure=bel&group_by=product,cohort                      -> Arrow
POST /api/explain   {run, component, modelpoint, t}                                   -> ExplainNode
GET  /api/diff?a=<runId>&b=<runId>&tol=1e-6                                           -> RunDiffDoc
POST /api/sensitivity {base_run, vary: "valuation_rate", values: [...]}               -> runs N projections
```

### 1.3 Notebook embedding

`results.show()` in Jupyter detects the kernel and returns an object whose `_repr_html_` emits an
`<iframe srcdoc="...">` with a fixed height (default 640 px, `height=` overridable). Two payload
strategies, chosen automatically:

- **Server-backed (default when the kernel is local):** iframe points at `http://127.0.0.1:7391/#<token>`.
  Full interactivity, no payload size limit. Survives kernel restarts poorly, which is fine.
- **Inline (remote kernels, Colab, nbconvert):** the whole `InlineDataSource` payload is embedded.
  Hard-capped at 25 MB; above that the object renders a summary table plus
  *"payload too large to inline (48 MB); call `.show(server=True)` or narrow with `.filter(...)`"*.

`nbconvert`-ed notebooks therefore keep working charts, which matters because the notebook *is*
the working paper in a lot of shops.

### 1.4 Static HTML export for governance packs

```bash
predictable export run/2026-06-30 --format html --out pack/valuation-2026Q2.html \
    --include graph,waterfall,diff:run/2026-03-31 \
    --modelpoints sample:200 \
    --with-traces bel,reserve \
    --engine wasm            # optional; embeds the engine for live explain()
```

Produces **one self-contained file**: no CDN, no fonts fetched, no network at all. It contains
the run manifest (all four digests), the full IR text of every module, the selected results, and
the selected pre-baked `explain()` traces.

This is the artefact that goes into the valuation file and gets emailed to the auditor. Design
consequences that follow from that audience:

- The manifest is a **visible header**, not a tooltip: model digest, assumption digest, modelpoint
  digest, table digests, engine version, IR version, export timestamp.
- Every chart has a "show the numbers" toggle rendering the underlying table, because auditors
  copy numbers.
- `Ctrl-P` produces a sane paginated print layout — page breaks between sections, charts as vector
  SVG, no dark mode in print.
- A `--sign` flag appends a detached signature over the file's canonical content hash. Out of
  scope for v1 mechanics; the hash line is present from day one so the format does not change later.

Size discipline: results are stored in the payload as base64 Arrow IPC, not JSON. A 200-modelpoint
sample with 30 output series over 480 periods is ~5 MB embedded, ~1.4 MB gzipped over HTTP.

### 1.5 Where WASM fits

The engine compiles to WASM through the `predictable-wasm` crate (`03-engine.md` §1, §9): no rayon,
no filesystem, single-threaded, `wasm32-unknown-unknown` + wasm-bindgen. It is **not** required for any view; it unlocks three
things client-side:

1. **On-demand `explain()` in a static export.** The trace is a replay of one modelpoint (IR §11.2),
   which costs microseconds. Embedding the WASM engine (~1.8 MB brotli) plus the model and the
   modelpoint file lets a frozen governance pack trace *any* cell, not just the pre-baked ones. This
   is the single highest-value use of WASM and the reason it is in v1.
2. **Sensitivity fans without a round trip** — re-run 9 scenarios on a 200-policy sample in the
   browser, ~40 ms, so the slider is continuous rather than request-per-drag.
3. **The docs site and the demo.** `predictable.dev/try` runs a real projection in the page. This is
   the marketing surface for "an LLM writes this IR and it runs anywhere".

Determinism caveat, stated in the UI and in the export header: **WASM results are bit-identical to
native** because the IR forbids reassociation, FMA contraction and parallel reductions (IR §9.2) and
wasm32 uses the same IEEE-754 `f64` semantics. The CI golden corpus runs under `wasmtime` and
asserts byte equality against the native run; if that ever fails, the WASM path is disabled rather
than shipped with a footnote.

### 1.6 Tech choices

| Concern | Decision | Why not the alternative |
|---|---|---|
| Framework | React 19 + TypeScript, Vite, no SSR | Not Svelte/Solid: hiring surface and the ecosystem below assumes React. Not vanilla: three complex stateful screens. |
| Charts | **Observable Plot** for standard charts, hand-written SVG/Canvas for the waterfall and the fan | Not ECharts/Highcharts: bundle weight, license, and awkward custom marks. Not raw D3 everywhere: Plot gets 80% of charts in 5 lines and drops to D3 scales when needed. Plot is 130 kB and MIT. |
| Graph layout | **ELK.js** (`elkjs`, `layered` algorithm) in a Web Worker | Not dagre (unmaintained, worse port/edge routing). Not force-directed: a dependency DAG with a topological order must be read left-to-right; force layouts destroy that. Not cytoscape.js: layout quality on layered DAGs is the whole game and ELK wins it. |
| Graph rendering | **Canvas 2D** above 300 visible nodes, SVG below | SVG keeps hit-testing, CSS and accessibility simple at small sizes; a 2,000-component Prophet library needs Canvas to stay at 60 fps. One renderer interface, two backends. |
| Tables | TanStack Table (headless) + custom virtualised body | 50,000 modelpoints must scroll. |
| Numbers on the wire | Apache Arrow IPC (`apache-arrow` JS) | §1.2. |
| Styling | CSS custom properties + a small hand-written system; no Tailwind, no component library | The print/export requirement and the theming requirement both argue for owning the CSS. |
| State | Zustand + URL as the source of truth for view state | Every view is deep-linkable and pasteable into a ticket — see §5.1. |
| Layout engine for the whole app | CSS grid, panels resizable, no drag-and-drop dashboards | Fixed, opinionated screens beat a dashboard builder for a verification tool. |

Total JS budget: **< 900 kB brotli** without WASM, < 2.7 MB with. Enforced in CI.

---

## 2. The graph explorer

### 2.1 The data it renders

`GET /api/graph` returns a document derived straight from IR §3, with nothing invented:

```json
{
  "ir_version": "1.0",
  "modules": ["term_assurance.schema", "term_assurance.decrements",
              "term_assurance.cashflows", "term_assurance.reserves"],
  "nodes": [
    {
      "id": "term_assurance.num_pols_if",
      "name": "num_pols_if",
      "module": "term_assurance",
      "kind": "Derived", "dtype": "f64", "shape": "Series",
      "unit": "count", "timing": "start",
      "stage": 1,
      "expr": "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)",
      "init": "1.0",
      "doc": "Survivorship. Self-referential with lag 1.",
      "tags": [],
      "span": {"file": "model.pir", "line": 122, "col": 9, "byte_start": 3140, "byte_end": 3216},
      "source": {"system": "prophet", "library": "TERM_UK", "variable": "NUM_POLS_IF",
                 "file": "TERM.VAR:42"},
      "declaration_index": 6,
      "depth": 4,
      "lints": [{"code": "W0104", "severity": "warn", "message": "..."}]
    }
  ],
  "edges": [
    {"from": "term_assurance.qx", "to": "term_assurance.num_pols_if",
     "lag": 1, "stage": 1, "via": "expr.Binary.rhs.Lag", "span": {...}}
  ],
  "layers": [["t","entry_age",...], ["age","in_term"], ...]
}
```

`layers` is the engine's own deterministic topological ordering (IR §3.2), bucketed by longest-path
depth. **The viz layer does not compute the topological order** — it renders the engine's, so the
picture and the evaluation order can never disagree.

### 2.2 Layout

- ELK `layered`, direction `RIGHT`, `nodePlacement.strategy = NETWORK_SIMPLEX`,
  `layering.strategy = LONGEST_PATH` seeded with the engine's `depth` so the visual layer index
  equals the evaluation depth.
- **Layout is computed once and cached to `.predictable/viz-layout.json`, keyed by
  `model_digest`.** Opening the explorer on an unchanged model is instant, and — importantly —
  node positions are *stable across sessions*, so a screenshot in a review comment stays
  meaningful. A changed model relayouts, with unchanged subgraphs pinned where ELK allows.
- Edge styling encodes IR semantics and nothing else:

| Edge | Style |
|---|---|
| `lag = 0`, stage 1 | solid, 1 px |
| `lag = k ≥ 1` | dashed, labelled `t-k` at the midpoint |
| `At(x, k)` seed edge | dotted, labelled `[k]` |
| stage-2 (`Agg`) whole-series edge | double stroke, labelled `⇒` |
| `init` reference to a stage-2 value | dotted with a small circle at the target — the one legal backward channel (IR §8.2), so it is drawn distinctly |
| table dependency | edge from a table-shaped node (rounded rect, different fill) |

Node shape encodes `Kind`: inputs are pills on the far left, `Derived` are rectangles, `Output`
rectangles with a heavier border and a filled left edge. `stage = 2` components (those whose `expr`
contains an `Agg`, IR §2.2) get a clipped corner regardless of kind. Node fill encodes `unit` — money, prob, rate, count, factor each get a hue from the same
palette used by the results charts, so a component keeps its colour identity across screens.

### 2.3 Interaction

- Click a node → the **inspector panel** (right, 420 px, resizable): full metadata, the `expr` with
  syntax highlighting and every identifier a link to its own node, the raw `.pir` text with the
  span highlighted, lints, the Prophet `source` provenance line, and — when a run is loaded — a
  sparkline of the component for the currently selected modelpoint.
- Double-click → **focus mode**: the subgraph within N hops (default 2 up, 2 down), everything else
  dimmed to 8% opacity rather than removed, so context is preserved.
- `Cmd-K` command palette over component names, docs, tags and Prophet variable names. Typing a
  Prophet variable (`NUM_POLS_IF`) finds the predictable component that claims it — this is how a
  migrating actuary navigates, and it is worth more than any other feature in this screen.
- Filters: by module, by tag, by `unit`, by `Kind`, by "affects output X" (downstream closure),
  by "changed vs run B" (highlights the impact set from `predictable diff`, IR §11.3).
- Impact overlay: select a node, press `I`, and its transitive downstream closure is highlighted in
  amber with a count — *"changing `qx` affects 14 components, 4 of them Outputs."*
- Search results, filters and selection are all in the URL.

### 2.4 Click-through to `explain()`

From a node's inspector, with a run loaded: pick a modelpoint and a `t` → `POST /api/explain` →
the trace opens as an **overlay tree over the graph**, not a separate page. The trace's nodes are
visually anchored to the graph nodes they correspond to, with the numeric value shown inline.
This is deliberately not a modal: the whole point is seeing the trace *in the shape of the model*.

---

## 3. Results views

### 3.1 Cashflow waterfall

Per modelpoint or per aggregate group, at a chosen `t`, or as a stacked time series.

- Hand-drawn SVG. Bars ordered by the model's declaration order (IR §4.1 rule 2) — authorial
  intent, not magnitude — with a "sort by magnitude" toggle.
- Timing tags are **shown, not hidden**: each bar carries a small `start`/`end`/`mid`/`point`
  glyph, because the single most common Prophet reconciliation error is a timing mismatch, and a
  chart that hides timing hides the bug.
- The connector line carries the running total; the final bar is the Output component, drawn with
  the Output node styling from the graph so the identity carries over.
- Click a bar → `explain()` for that component at that `t`.

### 3.2 Run-vs-run diff

Consumes `RunDiffDoc` from `predictable diff runA runB` (04-verify §5.4) — the viz does not compute
differences, it renders the engine's, including the model-diff cross-reference that attributes each
movement to a changed component. Detailed in §4.2.

### 3.3 Sensitivity fan

`POST /api/sensitivity` runs N projections varying one assumption (or an assumption set), returns
N result sets sharing a manifest lineage.

- A fan chart: the base run as a solid line, each sensitivity as a band, quantile bands when N ≥ 20.
- A tornado view for the same data at a single `t`: horizontal bars of Δ(output) per varied
  assumption, sorted by absolute impact. This is the view management actually asks for.
- Each band is a real run with its own manifest and is inspectable — click a band, and it becomes
  "run B" in the diff view. **No interpolation between computed scenarios, ever**; the fan draws
  only computed points, with visible markers.
- With WASM and a sampled portfolio, the assumption slider is live (§1.5).

### 3.4 Portfolio → modelpoint → component drill-down

Three levels, one breadcrumb, state in the URL:

```
/results/run-2026Q2/portfolio                            grouped by the [[aggregation]] blocks
  → /results/run-2026Q2/group/product=TERM&cohort=2019    the group's own waterfall + distributions
    → /results/run-2026Q2/mp/POL00042                     one policy: all series, all periods
      → /results/run-2026Q2/mp/POL00042/reserve@t=3        one cell: explain() trace
```

Level 1 renders only what `[[aggregation]]` declared (IR §8.3) — the viz never invents a grouping
the model did not declare, because an undeclared cross-modelpoint sum is exactly the class of
silent wrongness the IR was designed to prevent. Adding a grouping means editing the model, which
means it shows in the diff.

Level 2 gives distributions (histogram of `bel` per policy, box plot by attained age band) and a
**contribution table**: the policies contributing most to the group total, and to the group's
*movement* if a comparison run is loaded.

Level 3 is the modelpoint grid: components on rows, `t` on columns, virtualised, with a heat
overlay toggle. Selecting a cell is the entry point to `explain()`.

---

## 4. The three screens, in detail

### 4.1 Screen 1 — Model Explorer

The screen an actuary lives in during migration.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ predictable · term_assurance    [model.pir @ 9f2c4b1]   Run: 2026Q2 base   [⌘K]      │
├────────────┬────────────────────────────────────────────────┬────────────────────────┤
│ MODULES    │                                                │ num_pols_if            │
│ ▾ term_ass │   t ──▶ age ──▶ qx ⇢────────┐                  │ Derived · Series · f64 │
│   schema   │            ↘                │                  │ unit count · start     │
│   model    │             in_term ────┐   │                  │ stage 1 · depth 4      │
│            │                         ▼   ▼                  ├────────────────────────┤
│ FILTER     │   entry_age ──▶ ┌──────────────────┐           │ init = 1.0             │
│ unit  ▾    │                 │  num_pols_if     │╌╌╌ t-1 ╌╮ │ expr                   │
│ kind  ▾    │                 └────────┬─────────┘  ◀──────╯ │  num_pols_if[t-1]      │
│ tag   ▾    │                          │                     │  * (1 - qx[t-1])       │
│            │                          ▼                     │  * (1 - wx[t-1])       │
│ OVERLAYS   │       premium_income ─▶ net_cashflow ▶ ▮bel▮   │  * (if in_term         │
│ ☑ impact   │                                                │      then 1.0 else 0)  │
│ ☐ changed  │                                                ├────────────────────────┤
│ ☐ lints    │       [ fit ] [ 1:1 ] [ focus ] [ layers ▾ ]   │ from Prophet           │
│            │                                                │  TERM_UK.NUM_POLS_IF   │
│ 47 nodes   │                                                │  TERM.VAR:42           │
│ 118 edges  │                                                ├────────────────────────┤
│  2 lints   │                                                │ POL00042  ▁▂▄▆▇▇▆▄▂▁   │
│            │                                                │ [ trace at t = ▢ 3  ]  │
└────────────┴────────────────────────────────────────────────┴────────────────────────┘
```

What makes it earn its place, concretely:

- **The dashed `t-1` self-loop is drawn.** Recursion is the thing that confuses every reader of an
  actuarial model, and it is visible here as a loop-back edge with a lag label rather than hidden
  in a formula string.
- **Prophet provenance in the inspector.** During migration the reviewer's question is always
  "which Prophet variable is this?" and the answer is one panel away. The module tree has a
  **coverage mode** that colours Prophet variables green/amber/red by whether a component claims
  them — this is the migration progress bar.
- **Lints are on the graph**, as a small triangle on the node, not in a separate console. `W0104`
  ("constant across modelpoints and time") is visible exactly where you can act on it.
- **`explain()` opens in place** (§2.4), anchored to the graph.
- **Layer bands** (optional, toggle) draw the evaluation depth as vertical bands with the layer
  index — literally reading the engine's execution plan left to right.

Empty state (`model.show()` with no run loaded) is not an error state: the graph is fully usable,
the sparkline area says *"load a run to see values: `results = model.run(...); results.show()`"*.

### 4.2 Screen 2 — Run Diff

The Prophet reconciliation screen. This is the demo that sells the product.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ DIFF   A: prophet TERM_UK 2026Q2.rpt      B: predictable run 2026Q2      tol 1e-6    │
│        mp digest ✓ same    assumptions ✓ same    timeline ✓ same   engine 0.4.1     │
├──────────────────────────────────────────────────────────────────────────────────────┤
│ 12,481 modelpoints · 8 outputs · 6 outputs match within tolerance · 2 differ         │
│                                                                                      │
│  bel          Δ  +14,203.55   (+0.021%)   in 412 of 12,481 mp   ████░░░░░░░░░░░     │
│  reserve      Δ   -1,204.10   (-0.003%)   in 412 of 12,481 mp   █░░░░░░░░░░░░░░     │
│  pv_premiums  ✓ exact         pv_claims ✓ exact    net_cashflow ✓ exact             │
├───────────────────────────────┬──────────────────────────────────────────────────────┤
│ ATTRIBUTION                   │ CONTRIBUTORS to bel movement                         │
│                               │                                                      │
│ The model diff explains this  │  POL00042   +   812.40  ██████████                   │
│ movement:                     │  POL01187   +   604.02  ███████                      │
│                               │  POL00913   +   598.71  ███████                      │
│  qx   formula changed         │  … 409 more            ▔▔▔▔                          │
│   -  sa8990@(age,gender,smk)  │                                                      │
│   +  sa8990@(age,gender,smk)  │  All 412 share: smoker = true, entry_age ≥ 55        │
│      * mortality_loading      │  ← the cohort the change bites                       │
│                               │                                                      │
│  impact set: 9 components,    │  [ open POL00042 side-by-side ]                      │
│  reaching bel and reserve     │                                                      │
│                               │                                                      │
│  [ show in graph ]            │                                                      │
└───────────────────────────────┴──────────────────────────────────────────────────────┘
```

Design decisions that matter here:

- **Manifest reconciliation first.** Before any number is shown, the four digests are compared and
  differences are called out at the top. Half of all "the numbers don't match" incidents are a
  different modelpoint file, and the tool should say so in the first second rather than the third
  hour.
- **Attribution, not just difference.** The left panel is the model diff's impact set (IR §11.3)
  cross-referenced with the movement. "These 3 changes affect 14 components; `qx`'s change reaches
  `bel`" is the sentence that turns a reconciliation into a finding.
- **Cohort detection on the contributors list.** Given the set of differing modelpoints, the tool
  reports the modelpoint-field predicates that best separate them from the matching ones (a
  depth-2 decision-tree split over declared modelpoint fields, presented as a hypothesis and
  labelled as such: *"all 412 share…"*). This is a *chart-only derived quantity* under §0's rule and
  is labelled — but it is what an actuary would spend two hours doing in Excel.
- **Prophet `.rpt` is a first-class side A.** It is read into the same result schema (04-verify §2), so
  this screen is identical whether B is a predictable run or a Prophet run. The migration loop is
  literally: run diff, click the biggest contributor, `explain()` both sides, fix, re-run.
- Side-by-side modelpoint view: two column-aligned grids, differing cells highlighted, with both
  `explain()` traces stacked so the divergence point is visible — the first `t` where the trees
  disagree is auto-scrolled to and marked.

### 4.3 Screen 3 — Modelpoint Drill-down + `explain()`

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ portfolio ▸ product=TERM ▸ cohort=2019 ▸ POL00042            [ compare with run A ]  │
│ entry_age 47 · M · smoker · SA 250,000 · premium 1,240 · term 20                     │
├──────────────────────────────────────────────────────────────────────────────────────┤
│                t=0    t=1    t=2    t=3 ◀    t=4    t=5   …                          │
│ num_pols_if  1.0000 0.9721 0.9447 0.9178  0.8914 0.8655                              │
│ premium_rate 1240.0 1302.0 1367.1 1435.5  1507.2 1582.6                              │
│ premium_inc  1240.0 1265.7 1291.4 1317.3  1343.4 1369.8                              │
│ death_claims   —    134.2  138.9  143.7   148.6  153.7                               │
│ reserve      1084.2 1131.8 1180.1 1284.6▮ 1341.0 1402.7      [ heat ▢ ]              │
├───────────────────────────────┬──────────────────────────────────────────────────────┤
│ WATERFALL at t = 3            │ EXPLAIN  reserve · POL00042 · t = 3  = 1,284.55 money │
│                               │                                                      │
│  premium_inc  +1317.3 ▮start  │ (reserve[t-1] + premium_income[t-1]                   │
│  death_claims  -143.7 ▮end    │   - death_claims[t-1] - renewal_expenses[t-1])        │
│  renewal_exp    -48.2 ▮start  │   * (1 + valuation_rate)          model.pir:148       │
│  interest      +43.6  ▮       │ ├─ reserve[t-1]            t=2      1,180.11          │
│  ─────────────────────────    │ ├─ premium_income[t-1]     t=2        312.40          │
│  Δ reserve   +1,169.0         │ │  ├─ premium_rate         t=2        330.75          │
│                               │ │  └─ num_pols_if          t=2          0.9447        │
│  ⚠ start and end flows are    │ ├─ death_claims[t-1]       t=2        134.20          │
│    summed here; timings are   │ ├─ renewal_expenses[t-1]   t=2         48.20          │
│    shown per bar.             │ └─ valuation_rate    assumption:base    0.035         │
│                               │                                                      │
│                               │  ⓘ qx used sa8990 row 312, keys (47, M, false),       │
│                               │    policy `exact`.  [ open table at row 312 ]         │
└───────────────────────────────┴──────────────────────────────────────────────────────┘
```

Design decisions:

- The trace renders the trace tree of 04-verify §3.2 **verbatim**, with values, units, timings
  and spans. Every line is clickable: a `Ref` navigates to that component's own trace at that `t`;
  a span opens the `.pir` at that line; a `Lookup` opens the table viewer at the resolved row with
  the applied key policy called out. Clamped and stepped lookups get a warning tint — 04-verify §3.4 is
  explicit that this is where silent wrongness lives, and the UI should be too.
- `pre_origin_default` notes render as an inline badge: *"`t-1` before origin → used `init = 1.0`"*.
  Off-by-one at `t = 0` is a real bug class and it is now visible.
- The waterfall and the trace are the same data at different zoom. Clicking a waterfall bar moves
  the trace; clicking a trace node highlights the bar.
- The grid is a plain virtualised table, keyboard-navigable, `Cmd-C` copies TSV that pastes into
  Excel unmangled. Actuaries will do this constantly and it must be perfect.
- With a comparison run loaded, every cell shows a delta chip and the grid can filter to
  "differing cells only".

---

## 5. Cross-cutting

### 5.1 URL as state

Every view is fully described by its URL: run ids, selection, filters, `t`, modelpoint,
graph focus, diff tolerance. Pasting a URL into a review comment reproduces the exact screen.
In a static export the same URLs work as fragments. No exceptions, including transient panel state.

### 5.2 Colour, accessibility, print

- One palette shared across graph and charts, keyed to `Unit` for identity and to a diverging
  scale for diffs (negative/positive movement). Colour-blind safe (checked against deuteranopia and
  protanopia); **colour is never the only channel** — diffs also carry sign glyphs, timings carry
  letters, edge lag carries dash pattern and a text label.
- Dark and light themes, both defined explicitly; print forces light.
- Keyboard: full navigation of graph, grid and trace without a mouse. Focus visible. Charts have
  table equivalents behind a toggle — which is both an accessibility feature and the auditor
  feature from §1.4.
- Number formatting: thousands separators, sign always explicit on deltas, and **no rounding
  without saying so** — a cell shows the engine's `f64` on hover at full precision, because "it
  matched to 2 dp" is not a reconciliation.

### 5.3 Performance targets

| Scenario | Target |
|---|---|
| Graph explorer, 2,000 components, cached layout | first paint < 400 ms, pan/zoom 60 fps |
| Graph relayout after model change | < 2.5 s in a worker, with progress |
| Modelpoint grid, 50,000 mp × 480 t | scroll at 60 fps, initial view < 300 ms |
| `explain()` round trip (local server) | < 30 ms p95 |
| Run diff, 50,000 modelpoints × 20 outputs | rendered summary < 1.5 s |
| Static export, 200 mp sample + WASM | < 8 MB file, opens < 2 s |

### 5.4 Testing

- Golden-image tests (Playwright + pixel diff) for all three screens against the term-assurance
  corpus model; they run in CI and are the regression net for layout stability.
- The `DataSource` contract has a conformance suite run against all three implementations, so the
  static export cannot silently diverge from the server.
- Every screenshot in the docs is generated by a CI job from the corpus, never hand-captured.

### 5.5 Build order

1. `GET /api/graph` + graph explorer read-only (no run). Ships with `model.show()`.
2. Modelpoint grid + `explain()` panel. This makes `results.show()` useful.
3. Waterfall.
4. Run diff, Prophet `.rpt` side A. — *the demo*.
5. Static export.
6. WASM `explain()` in exports; sensitivity fans.

---

## 6. Non-goals for viz v1

- Editing the model in the browser. The `.pir` file and git are the editing surface; a graph editor
  would create a second source of truth and break the diff story.
- A dashboard builder / user-defined layouts.
- Multi-user hosting, accounts, a server deployment mode. `--bind 0.0.0.0` is not offered.
- Stochastic scenario views — waits for the IR 1.1 scenario axis (IR §12).
- Charting the aggregate of things the model did not declare aggregating (§3.4).
- Excel add-in. Copy-to-TSV is the v1 answer.

---

## 7. IR feedback

Things this layer needs that `01-ir.md` does not currently specify. None are divergences — they are
gaps I have assumed a resolution for above, flagged for the IR spec to make normative.

**Settled** (raised here, now closed):

- The results schema is normative in `04-verify.md` §2 — long format, one table, not one per shape.
  This document's §3 and §4 now consume that.
- `RunDiffDoc` is normative in `04-verify.md` §5.4, including the classification enum, findings
  ranking, and the attribution linkage the diff screen renders.
- The `explain()` node-variant list is closed and enumerated in `04-verify.md` §3.2, with the
  `pre_origin_default` resolution and the `N0xxx` notes the trace UI badges.
- A stable optional `meta.id`, preserved across renames and excluded from `model_digest`, exists
  (IR §11.1); it is the graph-layout cache key and the cross-run join key.
- Component ids are `<module_path>.<name>`, dot-separated (IR §2.2). `GraphDoc` uses that form.

**Also settled (IR decision log Q1–Q15, `01-ir.md` §13):**

1. **`GraphDoc`** stays owned by this document; IR §11 cross-references it and `predictable graph
   --json` emits it from the planner's own ordering (backlog T28).
2. **`ExprPath`** (Q9) has a grammar: a dotted field path rooted at `expr` or `init` with segments
   `lhs/rhs/operand/cond/then/else/arg<i>/key<i>/value/pred`, derived from the canonical expression
   and stable across reformatting (IR §3.0.1). `edge.via` uses exactly that string, so the explorer
   can resolve it to a sub-expression without byte offsets — and a `None` resolution is the signal
   that a cached layout is stale.
3. **`[[aggregation]]`** (Q4) is `{name, group_by (ordered tuple), measure, op, weight, filter,
   over_t}`, declared in the run file. **Groupings do not nest**: one flat row per key tuple, and the
   portfolio screen builds a drill-down tree from successive prefixes of the ordered tuple, with
   `group_key` rendered `key=value|key=value` in declaration order (IR §8.3).
4. **Sensitivity lineage** (Q12) is a first-class manifest block:
   `lineage = {parent_run, parent_manifest_digest, group_id, label, varied[]}`. The fan screen groups
   by `group_id` and orders by `varied`; it never parses a filename (IR §9.4.1).
5. **WASM determinism** is named in the IR: `wasm32-wasip1` under wasmtime is a supported determinism
   target and the golden corpus asserts byte equality there in CI (IR §9, rule 7).
6. **Table content travels** (Q13). Every table read is copied to `run/tables/<name>.parquet` with its
   own digest, written from the same bytes that were hashed, so a static export can open a lookup at
   the resolved row. `--no-table-copy` opts out and the UI must then say "table content unavailable in
   this run" rather than render a partial row (IR §9.4.1).

**Still open:** nothing. All questions raised by this layer are closed in `01-ir.md` §13.
