# The viz data source contract

Every predictable screen — the model explorer, the modelpoint drill-down, the run diff — is
written against **one interface**, `DataSource`. Nothing in the UI knows whether the numbers it is
drawing came from a local server, from a file that was emailed to an auditor, or from an engine
compiled to WebAssembly running inside the page.

That is the whole point of this page: the delivery mode is a deployment decision, not a product
decision. A chart that works in `results.show()` works unchanged in a governance pack.

## The three ways a screen gets its data

| Mode | Implementation | Where it comes from | What it can do |
|---|---|---|---|
| Local server | `HttpDataSource` | `predictable viz` serves the SPA and answers `/api/*` | everything the run supports, including `explain()` |
| Governance pack | `InlineDataSource` | `predictable export --format html` embeds the payload in the page | read the graph, the manifest, the sampled results, the pre-baked traces and the pre-baked diff |
| Governance pack + engine | `InlineDataSource` with an engine attached | `predictable export … --engine wasm` | the above, plus `explain()` on *any* cell and re-running scenarios in the browser |

The third row is deliberately not a third class. WebAssembly is a **capability**, not a delivery
mode: an inline pack is upgraded in place by attaching an engine to it, and every screen carries on
calling the same six methods.

## The calls

```ts
interface DataSource {
  manifest(): Promise<unknown>;             // the four digests, shown as a header
  graph(): Promise<GraphDoc>;               // the planner's own order — never recomputed
  component(name: string): Promise<ComponentDoc>;
  series(q: SeriesQuery): Promise<Table>;   // Arrow, zero-copy into typed arrays
  explain(q: ExplainQuery): Promise<unknown>;   // a trace tree; the only call that may compute
  diff(a: string, b: string): Promise<RunDiffDoc>;  // the run diff document
  capabilities(): Promise<Capabilities>;
}
```

Two invariants hold across every implementation, and are checked by tests rather than trusted:

1. **The viz layer never computes an actuarial number.** `series()` returns what the engine wrote.
   `explain()` is the only call permitted to compute, and only by replaying the engine. Anything a
   chart derives for its own presentation is labelled in the UI as a derived quantity.
2. **Capabilities never lie.** A source reporting `explain: false` rejects `explain()` — it does not
   return an empty trace, and the UI greys the button out rather than offering a dead one.

## Capabilities, and what a rejection means

`capabilities()` answers three booleans: `explain`, `recompute`, `sensitivity`. They are derived
from what the source actually holds, so a frozen pack exported with `--with-traces` reports
`explain: true` and a pack exported without them reports `explain: false`.

Failures are typed, and the status is the contract:

| Status | Meaning | What the UI does |
|---|---|---|
| 404 | the component, run or modelpoint is not in this source | says which name was not found |
| 400 | the query is malformed | fixes the query; never retried as-is |
| 501 | the capability is absent from this source | hides or greys the control |

A 501 is not an error to report to the user as a failure — it is the source being honest about
what it is. In the browser it is recognised with `isUnsupported(err)`, never by matching on the
message text.

## Reading a run diff

`diff(a, b)` returns the `pvf/1` `rundiff` document produced by
[`predictable diff run`](run-diff.md) — the same document, field for field, whether it arrives
from the server or was baked into a governance pack. It carries:

- both sides' manifest, model digest, engine version and system (`predictable` or `prophet`), which
  is what the reconciliation banner compares before showing any number;
- `summary`, with the verdict, per-output totals, cell counts and the first divergence;
- `findings`, each classified `root` / `inherited` / `structural` / `tolerance_only` **with the
  basis for that classification**, plus the exemplar and worst cells, the share of the headline
  output's movement, whether a model change explains it, and the
  [hypotheses](hypotheses.md) that fired with their evidence.

The screen renders that document. It does not re-rank findings, re-derive the verdict or recompute
a delta — if the number on the screen disagrees with the number the CLI printed, that is a bug in
one writer, not two opinions.

## Governance packs

A pack embeds its payload in the page as a single JSON block: the manifest, the graph, the sampled
results as base64 Arrow IPC (not JSON — that is the size discipline), any pre-baked `explain()`
traces, and any pre-baked run diff. There is no network access of any kind; the file works from a
USB stick in ten years.

Where a pack was exported *without* an engine, `explain()` answers pre-baked cells and reports
`not_prebaked` for the rest, naming the flag that would have included them. Attaching a WASM engine
removes the distinction: every cell becomes traceable, because a trace is a replay of a single
modelpoint and costs microseconds. WASM results are bit-identical to native — the IR forbids
reassociation, FMA contraction and parallel reductions, and the golden corpus is run under
`wasmtime` and asserted byte-equal against the native run.

## Extending it

Adding a call means adding it in four places: the TypeScript interface, every TypeScript
implementation, the Rust `DataSource` trait, and the Rust conformance suite that holds all
implementations to the same behaviour. A call only one implementation can answer belongs behind a
capability — never behind a check for which implementation you were handed.
