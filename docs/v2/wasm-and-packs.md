# WASM and governance packs

A **governance pack** is one HTML file that holds a whole run: the manifest with
every digest, the model's IR text, the results, and — optionally — the engine
itself. It is the artefact that goes into the valuation file and gets emailed to
an auditor, so it fetches nothing, ever: no CDN, no font, no API, no network at
all.

With the engine embedded, the same file also *runs*. It can trace any cell
(`explain()`), re-project a sampled portfolio, and draw a sensitivity fan — in
the page, offline, three years later.

Specification: [03-engine.md §9](../design/03-engine.md),
[05-viz.md §1.4, §1.5, §3.3](../design/05-viz.md).

## Exporting a pack

```bash
predictable export runs/base --format html --out pack/valuation-2026Q2.html \
    --include graph,diff:runs/2026-03-31 \
    --modelpoints sample:200 \
    --with-traces model.bel,model.reserve \
    --engine wasm
```

| Flag | Meaning |
|---|---|
| `--format` | `html` (the pack) or `json` (the same payload, for tooling) |
| `--out` | the file to write — the *only* file written |
| `--include` | `graph` and `diff:<run dir>`; a diff embeds the `pvf/1` rundiff document verbatim |
| `--modelpoints` | `all`, `sample:N`, or an explicit `KEY,KEY` list |
| `--with-traces` | components to pre-bake `explain()` traces for |
| `--engine` | `wasm` embeds the engine; omitted, the pack is frozen |
| `--engine-path` | where to find the `.wasm` (defaults to the cargo build path, or `$PREDICTABLE_WASM`) |

`sample:N` is the **first N modelpoints in file order**, never a random draw: a
pack an auditor cannot re-export byte for byte is not evidence.

Build the engine first — one cargo command, no `wasm-pack`, no
`wasm-bindgen-cli`:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p predictable-wasm --lib --profile wasm-release \
    --target wasm32-unknown-unknown
```

## What is in the file

Opened in any browser, the pack reads top to bottom:

1. **The manifest, as a visible header** — not a tooltip. `manifest_digest` is
   re-derived from the embedded content and reported as `verified` or
   `MISMATCH — this file has been edited`. Every other digest (model,
   assumptions, modelpoints, run, results, each table) is printed beside it.
2. **The results**, drawn as a chart and repeated under a *show the numbers*
   toggle, because auditors copy numbers.
3. **The model**, as the full canonical IR text of every module the run pinned.
4. **`results.schema.json`** and **`manifest.json`**, verbatim.
5. **The payload** — a JSON island holding the graph document, the results as
   base64 Arrow IPC (Arrow, never JSON: a 200-modelpoint sample stays a few MB),
   any embedded diffs, the pre-baked traces and, if asked for, the engine.

`Ctrl-P` prints it: page breaks between sections, charts as vector SVG, the
interactive controls hidden.

The pack is honest about its limits. It states the sample size and tells you the
rest is in `results.parquet`; a pack exported without an engine says so and names
the components whose traces were pre-baked.

## What the engine unlocks

Embedding `predictable-wasm` (~880 kB, ~310 kB gzipped) turns three things on:

- **`explain()` for any cell.** A trace is a replay of one modelpoint, which
  costs microseconds. Without the engine a pack answers only the pre-baked
  traces; with it, every cell of every embedded modelpoint is traceable. This is
  the highest-value use of WASM and the reason it is in v1.
- **Sensitivity fans without a round trip.** Nine scenarios over a sampled
  portfolio run in the page, so an assumption slider is continuous rather than
  request-per-drag.
- **The docs demo.** The same module runs a real projection in a web page.

Every trace carries the `E0901` assertion the CLI applies: a replay whose value
disagrees with the projection is an error, not a rendering.

## Are the numbers the same?

Yes, to the bit — and it is checked, not assumed.

`wasm32` uses the same IEEE-754 `f64` semantics as x86-64 and aarch64, and the
IR forbids the three things that would otherwise let targets diverge:
reassociation, FMA contraction and parallel reductions. CI runs the whole
conformance corpus twice — natively, and inside `wasm32-wasip1` under
`wasmtime` — rendering every emitted value as its **IEEE-754 bit pattern**, and
requires the two renderings to be byte-identical
(`crates/predictable-wasm/tests/wasm_gate.rs`). If that gate ever fails, the WASM
path is disabled rather than shipped with a footnote.

The same test enforces §9's **size budget**: engine + parser + checker ≤ 1.5 MB
gzipped, built with `opt-level = "z"`, `panic = "abort"`, LTO and
`+simd128,+bulk-memory`. Today it is about a fifth of that. SIMD128 accelerates
only lane-parallel operations, never reductions, which is why turning it on
cannot move a number.

## Using the engine from your own page

The module exports three functions and its memory — no generated JS glue, which
is what lets a pack be a single re-derivable file:

```js
const { instance } = await WebAssembly.instantiate(bytes, {});
const { pv_alloc, pv_free, pv_call, memory } = instance.exports;

const call = (request) => {
  const body = new TextEncoder().encode(JSON.stringify(request));
  const ptr = pv_alloc(body.length);
  new Uint8Array(memory.buffer, ptr, body.length).set(body);
  const out = pv_call(ptr, body.length);              // consumes the request
  const len = new DataView(memory.buffer).getUint32(out, true);
  const text = new TextDecoder().decode(new Uint8Array(memory.buffer, out + 4, len));
  pv_free(out, len + 4);
  return JSON.parse(text);
};
```

A response buffer is a little-endian `u32` length followed by that many UTF-8
bytes. `memory.buffer` is replaced whenever the guest grows its heap, so take
every view *after* the call.

The operations are the four of §9 plus the two the viz layer needs:

| `op` | Answers |
|---|---|
| `version` | the engine version and its default chunk size |
| `check` | the checker's diagnostics — the same messages the CLI prints |
| `plan` | `model_digest`, `plan_digest`, `order_digest`, outputs, modelpoint keys |
| `run` | a projection, as long-form columns |
| `explain` | one trace, `E0901` asserted |
| `sensitivity` | N scenarios, each with its lineage |

Every request carries the model with it — `{sources, assumptions, tables,
modelpoints}` — so there is no session handle to go stale. Responses are
`{"ok": true, …}` or `{"ok": false, "kind": …, "error": …}`, and `kind` is what
the UI maps onto a status code (`unknown_component` → 404, `unsupported` → 501).

!!! note "One deviation from §9"

    §9 lists `run(...)` as returning Arrow IPC. It returns columns instead:
    emitting IPC from the guest would put the whole `arrow-ipc` stack inside the
    1.5 MB budget to build a buffer the page unpacks again immediately. The
    `DataSource` contract is unchanged — `series()` still hands screens an Arrow
    `Table` — and the pack's *stored* results are still base64 Arrow IPC.

## Sensitivity fans and lineage

A fan is N runs that share a parent, and that relationship is recorded, never
inferred from filenames ([IR Q12](../design/01-ir.md)). Every scenario the engine
computes comes back with:

```jsonc
"lineage": {
  "parent_run": "2026-06-30T00:00:00Z-base",
  "parent_manifest_digest": "sha256:…",
  "group_id": "sens-2026-06-30-mort",
  "label": "mortality_loading × 1.1",
  "varied": [{"path": "assumptions.mortality_loading", "from": 1.0, "to": 1.1}]
}
```

The chart reads those fields rather than re-deriving them from the request it
sent. Two rules follow, and both are structural in `src/wasm/fan.ts` rather than
stylistic:

- **No interpolation between computed scenarios, ever.** A band holds only
  computed `(t, value)` points, each drawn with a visible marker. A period the
  run produced nothing for is a gap, not a zero.
- **Each band is a real run and is inspectable**, so the UI can hand one to the
  diff screen as "run B".

The tornado view is the same data at one `t`: Δ(output) per varied assumption,
sorted by absolute impact. A band with no computed point at that `t` is omitted
rather than drawn as a zero-length bar, because a zero bar claims insensitivity
that was never measured.

## In the viz app

WASM is a capability, not a delivery mode. `selectDataSource()` returns an
`InlineDataSource` when a payload is embedded; if that payload carries an engine,
the source is upgraded in place before the first render and `capabilities()`
flips from `{explain: <the pre-baked few>}` to
`{explain, recompute, sensitivity}`. No screen branches on which implementation
it got. A pack whose engine fails to instantiate still opens — the manifest, the
IR and the numbers are the evidence — with the trace button greyed out.
