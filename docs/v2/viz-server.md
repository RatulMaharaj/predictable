# The graph document and the viz server

*Specified by `05-viz.md` §1.1, §1.2 and §2.1. Implemented by `predictable-viz`,
`predictable graph` and `predictable serve`.*

The visualisation layer has one job — close the verification loop faster than
reading numbers in a spreadsheet — and one rule that makes it trustworthy:

> **It is a pure consumer.** It never computes an actuarial number, and it never
> works out its own evaluation order. Every number it shows came out of the
> engine; the order it draws is the planner's own.

That rule is why this page exists in the same shape as the code: a document
(`GraphDoc`), a contract (`DataSource`), and a transport (JSON plus Arrow IPC).

## `predictable graph --json`

`graph` plans the model and prints the `GraphDoc` of §2.1.

```console
$ predictable graph models/term_annual/build/model.pir models/term_annual/build/schema.pir
48 node(s), 67 edge(s), 9 layer(s) in evaluation order
   0  mortality, lapses, expenses, policy_number, entry_age, sex, smoker, sum_assured, …
   1  expense_scale, initial_expense, wx, disc_factor, age, in_term, premium_rate
   2  in_force_factor, qx
   3  num_pols_if
   4  deaths, surrenders, premium_income, renewal_expenses
   5  death_claims, pv_premiums, pv_expenses
   6  net_cashflow, reserve, pv_claims
   7  bel
   8  profit_margin
```

Add `--json` for the document itself:

```console
$ predictable graph models/term_annual/build/*.pir --json
```

```json
{
  "format": "pvf/1",
  "kind": "graph",
  "ir_version": "pir/1",
  "model_digest": "sha256:…",
  "plan_digest": "sha256:…",
  "order_digest": "sha256:…",
  "modules": ["model", "schema"],
  "nodes": [
    {
      "id": "model.num_pols_if",
      "name": "num_pols_if",
      "module": "model",
      "kind": "Derived",
      "dtype": "f64",
      "shape": "Series",
      "unit": "count",
      "timing": "start",
      "stage": 1,
      "expr": "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)",
      "init": "1.0",
      "doc": "Policies in force at the start of year `t`, per policy sold.",
      "tags": [],
      "span": {"file": "models/term_annual/build/model.pir", "line": 56, "col": 8,
               "byte_start": 1189, "byte_end": 1202},
      "declaration_index": 5,
      "depth": 3,
      "retention": "ring(2)",
      "hoistable": false
    }
  ],
  "edges": [
    {"from": "model.qx", "to": "model.num_pols_if", "lag": 1, "at": false,
     "stage": 1, "via": "expr.lhs.lhs.rhs.rhs",
     "span": {"file": "models/term_annual/build/model.pir", "line": 63, "col": 33,
              "byte_start": 1328, "byte_end": 1335}}
  ],
  "layers": [["mortality", "entry_age", "t"], ["age", "wx"], ["qx"], ["num_pols_if"]]
}
```

What each part is, and where it comes from:

| Field | Meaning | Source |
|---|---|---|
| `nodes` | one entry per component, **in the planner's evaluation order** | `Plan::order()` |
| `nodes[].depth` | longest-path depth measured along that order | derived from the plan, not re-sorted |
| `nodes[].expr` | the expression **as the author wrote it**, parentheses and all | the raw key/value tree |
| `nodes[].retention` | `full` or `ring(n)` — the planner's memory decision | `SeriesSlot::retention` |
| `edges[].lag` | `0` same period, `k` for `x[t-k]`, `null` for a table read | the reference's `Lag` |
| `edges[].at` | true for an absolute `At(x, k)` seed edge | the reference's `Lag` |
| `layers` | node names bucketed by `depth` | a *view* of `nodes`, never a second sort |
| `order_digest` | the planner's own ordering hash | `Plan::order_digest` |

Inputs, assumptions, timeline fields and tables are nodes too — §2.2 draws them
as pills and rounded rectangles on the far left — so no edge in the document ever
points at a name that is not in `nodes`.

!!! note "Why `layers` is not computed in the browser"
    If the picture were laid out from a topological sort of its own, the drawing
    and the run could disagree — silently, and exactly on the models complicated
    enough for the disagreement to matter. Bucketing the planner's order makes
    that impossible rather than merely unlikely. The conformance case
    `graph.order_respects_edges` fails any data source that re-sorts.

## `predictable serve`

`serve` is the command-line face of `model.show()`: it plans the model, builds
the same document, and serves it on loopback with a token.

```console
$ predictable serve models/term_annual/build/model.pir models/term_annual/build/schema.pir
serving 48 components at http://127.0.0.1:7391/#8f3c…e21a
the token is in the fragment; every API call must carry it
Ctrl-C to stop
```

- `--port N` — `0` asks the OS for a free port.
- `--no-block` — start and return, for scripts and tests.
- `--json` — print the URL, port, token and capabilities as a `pvf/1` document.

The token is a fresh 32 bytes per server and lives in the **URL fragment**:
browsers never send a fragment to a server, so it stays out of request lines,
proxy logs and shell history. Every API call replays it as `?token=…` or an
`X-Predictable-Token` header. Anything else gets `401`. The bind address must be
loopback — starting on `0.0.0.0` is refused, not warned about.

## The API

| Route | Returns |
|---|---|
| `GET /api/health` | liveness, no token needed, says nothing about the run |
| `GET /api/capabilities` | `{explain, recompute, sensitivity}` |
| `GET /api/manifest` | the run manifest (`04-verify.md` §6) |
| `GET /api/graph` | the `GraphDoc` above |
| `GET /api/component/{name}` | one node plus `upstream` / `downstream` |
| `GET /api/series?components=a,b&mp=POL1&t=0..40` | **Arrow IPC stream** |
| `GET /api/aggregate?measure=bel&group_by=product,cohort` | Arrow IPC stream |
| `POST /api/explain` `{run, component, modelpoint, t}` | a trace tree |
| `GET /api/diff?a=…&b=…` | a `RunDiffDoc` |

**JSON for metadata, Arrow IPC for numbers.** A 5,000-modelpoint × 480-period
`f64` series is 19 MB as JSON and 2.4 MB as an Arrow stream that goes zero-copy
into a typed array. The `series` schema is fixed: `modelpoint: Utf8`,
`t: UInt32`, then one `Float64` column per requested component **in the order
requested**, with rows ordered by modelpoint then by ascending `t`.

Errors are typed, so a UI can branch on them instead of on prose:

| Status | `kind` | When |
|---|---|---|
| 400 | `bad_request` | no components, an inverted `t` range, `t=notarange` |
| 401 | — | missing or wrong token |
| 404 | `unknown_component` / `unknown_run` / `unknown_modelpoint` | a name that is not there |
| 501 | `unsupported` | a capability this source does not have |

A missing capability is a `501`, never an empty `200`: the UI greys the button
out with *"this is a frozen export"* instead of offering a dead one.

## The `DataSource` contract

One SPA, three hosts (local server, notebook embed, static export) means one
interface and three implementations. The Rust half:

```rust
pub trait DataSource: Send + Sync + 'static {
    fn capabilities(&self) -> Capabilities;
    fn manifest(&self) -> Result<serde_json::Value>;
    fn graph(&self) -> Result<GraphDoc>;
    fn component(&self, name: &str) -> Result<ComponentDoc>;   // defaults to a view over graph()
    fn series(&self, query: &SeriesQuery) -> Result<RecordBatch>;
    fn aggregate(&self, query: &AggregateQuery) -> Result<RecordBatch>;
    fn explain(&self, query: &ExplainQuery) -> Result<serde_json::Value>;
    fn diff(&self, a: &str, b: &str) -> Result<serde_json::Value>;
}
```

The TypeScript half is the same contract in `frontend/src/datasource.ts`, with
`HttpDataSource` implementing it against the routes above.

### A worked example

Serve a model and two modelpoints' results from Rust:

```rust
use std::sync::Arc;
use predictable_check::Input;
use predictable_plan::{plan_sources, PlanOptions};
use predictable_viz::{graph_doc, InMemoryDataSource, RunData, ServerOptions, VizServer};

let inputs = [Input::new("term.pir", std::fs::read_to_string("term.pir")?)];
let plan = plan_sources(&inputs, &PlanOptions::default())?;
let graph = graph_doc(&inputs, &plan, "sha256:…");

let run = RunData::new(5)
    .with_series("death_claims", "POL0001", vec![10.0, 9.5, 9.0, 8.5, 8.0])
    .with_series("death_claims", "POL0002", vec![1.0, 2.0, 3.0, 4.0, 5.0]);

let server = VizServer::start(
    Arc::new(InMemoryDataSource::new(graph).with_run("run/2026-06-30", run)),
    ServerOptions { port: 0, ..ServerOptions::default() },
)?;
println!("open {}", server.url());
```

Then, from the browser (or `curl`):

```js
const source = new HttpDataSource(location.hash.slice(1));
const table = await source.series({
  components: ["death_claims"],
  modelpoints: ["POL0002"],
  tFrom: 1,
  tTo: 3,
});
// t = [1, 2, 3], death_claims = [2, 3, 4] — exactly the numbers the engine wrote.
```

Dropping the `VizServer` shuts it down; the handle's lifetime *is* the server's.

### The conformance suite

Three implementations of one interface is three chances to behave differently.
The suite in `predictable_viz::conformance` is written once, against
`dyn DataSource`, and every implementation must pass it:

```rust
predictable_viz::conformance::assert_conformant(&source, Some("death_claims"), None);
```

The cases, and the UI behaviour each one protects:

| Case | What breaks without it |
|---|---|
| `graph.ids_unique`, `graph.edges_resolve` | an edge that points at nothing |
| `graph.layers_partition_nodes`, `graph.layer_index_is_node_depth` | a node drawn twice, or in the wrong column |
| `graph.order_respects_edges` | a picture that disagrees with the run |
| `graph.stable_across_calls` | a layout that jumps between refreshes |
| `component.matches_graph` | an inspector that contradicts the canvas |
| `series.schema`, `series.column_order_follows_request` | a chart plotting the wrong series |
| `series.rows_sorted_by_modelpoint_then_t` | a waterfall reading the wrong period |
| `series.t_range_is_respected` | a zoom that silently returns everything |
| `series.unknown_component_is_unknown` | a typo that renders as an empty chart |
| `series.deterministic` | a governance pack that changes bytes between exports |
| `capabilities.explain_is_honest` | a dead "trace this cell" button |

The suite is written to bite: the crate's own tests point it at sources that
shuffle rows, re-sort the graph and lie about `explain`, and assert that the
matching case *fails*.

## The frontend

`crates/predictable-viz/frontend` is a Vite + React + TypeScript app. It builds
to a single self-contained file, `dist/index.html`, which the server embeds with
`include_str!` — so the binary is the whole product, and a governance pack
inlines the same bytes with no CDN and no network at all.

```console
$ cd crates/predictable-viz/frontend && ./build.sh
built 12384 bytes into dist/index.html
```

`dist/index.html` is checked in on purpose: `cargo build` must work on a machine
with no Node installed. Today it is a placeholder shell that proves the contract
end to end — it reads the token from the fragment, calls `/api/capabilities` and
`/api/graph`, and prints the planner's layers. The Model Explorer and the
drill-down screens replace it without any change on the Rust side.
