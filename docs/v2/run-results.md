# Run results and manifests

Every predictable run writes a **run directory**. This page is the user-facing guide to what is
in it, how to read it, and what the guarantees attached to it actually mean.

Normative sources: `04-verify.md` §2 (results schema) and §6 (manifests), plus the IR rulings
Q12 (lineage), Q13 (table copies) and Q15 (`f64` only). The implementation is the
`outbound` module of the `predictable-io` crate (merged in from the temporary
`predictable-io-out` crate when the runner landed); the runner that fills these artefacts in is
documented in [Running a model](running-a-model.md).

```text
run/
  manifest.json            what produced these numbers, and can I get them again
  results.parquet          one row per (modelpoint, component, t)
  results.schema.json      the component list, so nothing needs to parse .pir
  aggregates.parquet       portfolio aggregations                      [if declared]
  solves/<name>.parquet    per-modelpoint solve outcomes               [if declared]
  tables/<name>.parquet    a copy of every table the run actually read
```

`manifest.json` is written **last**, because it records the digests of everything else.

---

## 1. `results.parquet` — long format, and why

One row per `(modelpoint, component, t)`:

| column | type | meaning |
|---|---|---|
| `mp_key` | `string` | the modelpoint field marked `key = true` |
| `mp_row` | `uint32` | 0-based row index in the modelpoint file — stable ordering |
| `component` | `dictionary<string>` | qualified `<module_path>.<name>` |
| `stage` | `int8` | `1` or `2`, copied from the IR |
| `t` | `int32` | `-1` for `Scalar` and `PerMP` components |
| `value` | `double` | `f64` components |
| `value_i` | `int64` | nullable — `i64` components |
| `value_b` | `bool` | nullable — `bool` components |
| `value_s` | `dictionary<string>` | nullable — `str`, `enum`, `date` (ISO-8601) |

Wide format (one column per component) is the obvious choice and the wrong one: two runs that
emit different component sets stop being comparable at the schema level, and the diff can then
only tell you *that* something changed. Long format keeps the join key in the data, so
`predictable diff` can align on `(mp_key, component, t)` and say *where*.

Three rules worth knowing:

**Exactly one `value*` column is non-null per row**, and *which* one is decided by the
component's IR `dtype` — never by inspecting the number. A component declared `i64` writes
`value_i` even if every value happens to be integral. Reading `value` for an `i64` component and
finding nulls is not a bug; it is the schema telling you to look at `results.schema.json`.

**There are no runtime nulls** (IR §2.11). The nullable `value_*` columns express dtype
selection only. A modelpoint that trapped under `--continue-on-trap` contributes **no rows** —
not null rows — and appears in `manifest.execution.traps` instead.

**`value` is Parquet `double`, unconditionally** (Q15). `run.storage_precision` is reserved in
IR 1.0 and accepts `"f64"` only; `"f32"` is `E0108`. The key exists so that a consumer never has
to assume the precision it is reading.

### Reading it

```python
import pyarrow.parquet as pq

t = pq.read_table("run/results.parquet")
bel = t.filter(
    (t["component"] == "term_assurance.bel") & (t["mp_key"] == "POL0001")
).sort_by("t")
print(bel.column("value").to_pylist()[:5])
```

```sql
-- duckdb
SELECT t, value
FROM 'run/results.parquet'
WHERE component = 'term_assurance.bel' AND mp_key = 'POL0001'
ORDER BY t;
```

### Row order is part of the contract

Rows leave the writer in **`(chunk_idx, offset)` order**: `chunk_idx` is the runner's chunk
number, `offset` is the modelpoint's position inside that chunk. That pair is a total order over
the portfolio which does not depend on how many threads ran, which is what makes
`run(threads=1)` and `run(threads=64)` byte-identical.

The writer enforces the order rather than trusting it. A runner that finishes chunk 7 before
chunk 5 must buffer:

```rust
let mut w = run.results_writer(schema, WriterOptions::default())?;
w.write_chunk(&chunk_0)?;
w.write_chunk(&chunk_2)?;   // Err(ChunkOutOfOrder { expected: 1, got: 2 })
```

---

## 2. `results.schema.json`

The document beside the results restates the emitted component list, so no consumer ever parses
`.pir` to interpret a result set:

```json
{
  "format": "pvf/1",
  "kind": "results_schema",
  "emit": "all",
  "component_set_digest": "sha256:aa47…",
  "columns": [
    {"name": "mp_key", "type": "string", "nullable": false, "doc": "…"}
  ],
  "components": [
    {
      "id": "term_assurance.bel",
      "name": "bel",
      "kind": "Output",
      "dtype": "f64",
      "shape": "Series",
      "unit": "money",
      "timing": "end",
      "stage": 1,
      "output": true,
      "display": {"dp": 2}
    }
  ],
  "storage_precision": "f64"
}
```

`components` is sorted by `id`, and `timing` is `null` rather than absent on `Scalar`/`PerMP`
components — "this value has no timing" is information (Q14).

**`component_set_digest`** is SHA-256 over the sorted qualified ids. Two result sets with equal
digests are directly comparable; unequal ones are an `emit_mismatch` — a named, first-class diff
condition that compares the intersection, not a structural error (IR §8.4.3). Because `emit` is
also a `run_digest` field, an emit difference is never invisible.

The JSON Schema for this document ships at
`crates/predictable-io/schemas/results.schema.json`.

---

## 3. `manifest.json`

> A result set without a manifest is not evidence.

The manifest answers *what exactly produced these numbers, and can I get them again?* Its key
order is fixed by the spec so that manifests diff cleanly in git, and every array in it is
sorted (files by path, tables by name, packages by name).

Two digests do the load-bearing work, and the difference between them is the point:

| digest | over | excludes |
|---|---|---|
| `manifest_digest` | everything that can change a number | `run_id`, `execution`, `results.digest`, `provenance.user`, `environment`, itself |
| `environment_hash` | the `environment` block alone | — |

**Two runs with the same `manifest_digest` must produce a byte-identical `results.parquet`.**
CI asserts this across platforms; a violation is a P0 engine bug.

The environment sits *outside* `manifest_digest` deliberately. A matching `manifest_digest` with
a differing `environment_hash` is exactly the case a diff must call out first — *same inputs,
different machine* — so `predictable diff` prints

```text
environment differs: aarch64-apple-darwin vs x86_64-unknown-linux-gnu
```

above the findings, and nobody spends an afternoon on a 1-ULP mystery.

### `execution.outcome` is load-bearing

`completed` | `completed_with_traps` | `aborted` | `cancelled`. An `aborted` run has a manifest
and no `results.parquet`; a `cancelled` run has both and its result set is explicitly partial.
Any report derived from a run whose outcome is not `completed` carries a banner naming the
outcome and the trapped or missing modelpoint count. A consumer that ignores `outcome` will
eventually report on a truncated portfolio as though it were whole.

### `lineage` — how a sensitivity fan is grouped (Q12)

```json
"lineage": {
  "parent_run": "2026-08-17T09:14:22Z-3f0a",
  "parent_manifest_digest": "sha256:3f0a…",
  "group_id": "sens-2026-06-30-mort",
  "label": "mortality +10%",
  "varied": [{"path": "assumptions.mortality_loading", "from": 1.0, "to": 1.1}]
}
```

A base run says `"parent_run": null` explicitly rather than omitting the key. Fans are grouped
by `group_id` and never inferred from filenames — a regulator reads these. `varied` is always
*derivable* by diffing the two manifests, so it is never the only record of what changed:

```rust
let varied = Manifest::derive_varied(&parent, &child);
// [VariedInput { path: "assumptions.base", .. }, VariedInput { path: "run", .. }]
```

### Table copies (Q13)

Every table actually read during a run is copied to `run/tables/<name>.parquet`, and its
manifest entry gains `copy` and `copy_digest`:

```json
{"name": "sa8990", "path": "tables/sa8990.csv", "resolver": "fs",
 "digest": "sha256:9f2c…", "declared_digest": "sha256:9f2c…", "drift": false,
 "copy": "tables/sa8990.parquet", "copy_digest": "sha256:b30d…", "rows": 412}
```

This is not bookkeeping. `explain()` shows the resolved *row* of a lookup, and an archived run
with no access to the original CSV cannot show that row otherwise — the trace would degrade to
"some row, value 0.00214", which is precisely the silent-wrongness surface tables are dangerous
for. `--no-table-copy` stamps `"copy": null`, and a consumer that finds it must say **"table
content unavailable in this run"** rather than render a partial row.

`drift` is computed, not asserted: it is `true` when the bytes read differ from the digest the
model declared.

---

## 4. The side files

**`aggregates.parquet`** — `aggregation, group_key, measure, t, value`. `group_key` is the
ordered `k1=v1|k2=v2` rendering of IR §8.3, and `t` is `-1` unless the aggregation declared
`over_t = "each"`. Groupings do not nest: a drill-down tree is built from successive prefixes of
the ordered key tuple, and there are no subtotal rows.

**`solves/<name>.parquet`** — `mp_key, mp_row, solved_value, residual, iterations, converged`,
written whenever a `[[solve]]` has `scope = "per_mp"`. The solved value is *also* materialised as
an ordinary `PerMP` component in `results.parquet`, so downstream joins never special-case
solves.

---

## 5. Worked example: writing a run directory

The runner drives this crate; the shape below is what T15 calls.

```rust
use predictable_io_out::*;
use predictable_ir::{Component, DType, Expr, Kind, Shape};

// 1. Fix the emitted component set. `stage`, `dtype` and `timing` are copied from the IR,
//    never re-derived.
let mut bel = Component::derived("bel", DType::F64, Shape::Series, Expr::f64(0.0));
bel.kind = Kind::Output;
let schema = ResultsSchemaDoc::new(
    "outputs",
    vec![ComponentDescriptor::from_ir("term_assurance", &bel)],
)?;

// 2. Open the run directory. This writes results.schema.json immediately, so an aborted run
//    still records the component set it intended to emit.
let run = RunDir::create("run")?;
let mut w = run.results_writer(schema, WriterOptions::default())?;

// 3. Feed chunks in (chunk_idx, offset) order.
let mut chunk = ResultsChunk::new(0);
chunk.push(ModelpointRows {
    offset: 0,
    mp_key: "POL0001".into(),
    mp_row: 0,
    cells: vec![
        Cell::f64("term_assurance.bel", 0, 1234.5),
        Cell::f64("term_assurance.bel", 1, 1300.0),
    ],
});
w.write_chunk(&chunk)?;
let results = w.finish()?;   // rows, digest, component_set_digest

// 4. Side files.
let (table_path, table_copy) = run.write_table_copy(&TableCopy {
    name: "sa8990".into(),
    columns: vec![
        ("age".into(), ColumnData::I64(vec![40, 41, 42])),
        ("qx".into(), ColumnData::F64(vec![0.0021, 0.0023, 0.0026])),
    ],
})?;

// 5. The manifest, last, recording what the writers just committed to.
// `Manifest::draft(...)` takes the versions, inputs, timeline, run config, results ref,
// execution block and provenance; `write_manifest` then stamps `environment_hash` and
// `manifest_digest` and writes the file.
let manifest = run.write_manifest(draft)?;
assert!(manifest.verify_digest()?);
```

### What the writer refuses

Each of these is a runner bug caught at the boundary rather than a corrupt result set noticed
three weeks later:

| you did | you get |
|---|---|
| chunk `2` after chunk `0` | `ChunkOutOfOrder { expected: 1, got: 2 }` |
| offsets `3` then `1` in one chunk | `OffsetOutOfOrder { previous: 3, got: 1 }` |
| a component not in the schema | `UnknownComponent("term.not_a_component")` |
| an `i64` value for an `f64` component | `DTypeMismatch { component, dtype, found }` |
| `t = 0` on a `PerMP` component | `BadT { expected: "-1", got: 0 }` |
| `storage_precision = "f32"` | `UnsupportedStoragePrecision` (`E0108`) |

---

## 6. Reproducing a run

`predictable rerun run/manifest.json` re-executes from the manifest alone, verifying every
digest before it starts and refusing on any mismatch. `--allow-drift` proceeds and stamps
`"drift": true` into the new manifest permanently.

Reproducibility is the six-tuple `(model_digest, assumption_digest, modelpoint_digest,
table_digests, run_digest, engine_version)`; the manifest carries all six. A `dirty: true` in
`provenance.cwd_git` means the tree was not clean, the run is not reproducible, and every report
derived from it says so.
