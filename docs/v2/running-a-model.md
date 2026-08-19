# Running a model

The kernel projects one chunk of modelpoints. **The runner** (`predictable-runner`) is everything
that has to happen around it for a *run* to exist: reading modelpoints, deciding where chunks
execute, solving, aggregating, writing the run directory and stamping the manifest.

Normative sources: `03-engine.md` §5.1 (the chunk pipeline), §5.4 (solvers), §10 (the executor
seam), and `01-ir.md` §8.3 (aggregation, Q4), §8.4 (products and run config, Q1/Q8), §9.3.1 (trap
policy, Q7) and §9.4.1 (lineage and table copies, Q12/Q13).

```text
[run].pir ──► RunFile ─┐
modelpoints ──► Chunks ─┼──► Runner ──► ChunkExecutor ──► kernel (per chunk)
plan + tapes ──────────┘                    │
                                            ├──► [[solve]]        Brent, outside the projection
                                            ├──► [[aggregation]]  monoid fold, chunk-index order
                                            └──► run/             results, sidecars, manifest.json
```

---

## 1. The four things the runner promises

1. **It never computes a number.** Every value comes from the kernel. The runner decides *what*
   is projected and *what is written*, never *what a value is*.
2. **Order is a function of the data, not of the schedule.** Chunks come back in chunk index
   order from every executor, aggregation partials combine in chunk index order, results are
   written in `(chunk_idx, offset)` order. So `run(threads = 1) ≡ run(threads = 64)` and
   `run(C = 1) ≡ run(C = 1024)` — bit for bit, not to a tolerance.
3. **A short result set never looks complete.** A trap under `on_trap = "abort"` writes a
   manifest and no results; `continue` drops the modelpoint entirely and counts it; a cancelled
   run is stamped `"outcome": "cancelled"` with a projected count short of the file's row count.
4. **A result set without provenance is not evidence.** Every run writes the six reproducibility
   digests, `lineage`, the solve outcomes, and — unless suppressed — the table content that was
   actually read.

---

## 2. A run, end to end

```rust
use std::collections::BTreeMap;
use predictable_check::Input;
use predictable_plan::{plan_sources, PlanOptions};
use predictable_tape::lower_plan;
use predictable_io::{CsvSource, MpSchema};
use predictable_runner::{
    load_chunks, CancelFlag, LocalExecutor, RunInputs, Runner,
};
use predictable_runner::config::run_file;
use predictable_runner::run::{execute, RunSpec};

// 1. Model → plan → tapes (the checker has already run inside `plan_sources`).
let inputs = [Input::new("term.pir", model_source)];
let plan = plan_sources(&inputs, &PlanOptions::default())?;
let tapes = lower_plan(&plan)?;
let modules = predictable_plan::lower_modules(&inputs);

// 2. The `[run]` file → typed configuration, with §8.4.2's defaults applied exactly once.
let run = run_file(&parsed_run_document)?;

// 3. Modelpoints → chunks of exactly `chunk_size` rows.
let schema = MpSchema::from_module(&modules[0])?;
let mut source = CsvSource::open(schema, "data/term.csv")?;
let chunks = load_chunks(&mut source, "policy_number", run.run.exec.chunk_size as usize)?;

// 4. Bind, then run.
let mut runner = Runner::new(RunInputs {
    modules: &modules,
    plan: &plan,
    tapes: &tapes,
    tables,          // resolved, digest-checked, compiled
    assumptions,     // name -> f64
    run: &run,
})?;

let report = execute(
    &mut runner,
    chunks,
    &LocalExecutor::new(8),
    &CancelFlag::new(),
    &spec,               // run id, input digests, versions, lineage, table copies
    std::path::Path::new("runs/2026-06-30-base"),
)?;

assert_eq!(report.exit_code, 0);
println!("{}", report.manifest.manifest_digest);
```

`execute` is the whole run: solves first (in declaration order, each leaving its solved input in
place), then the final projection, then the run directory. If you already have a projection — a
hosted or distributed runner that did the work elsewhere — call `write_run` instead and hand it
the chunks and the projection.

---

## 3. Executors: where a chunk runs

```rust
pub trait ChunkExecutor: Send + Sync {
    fn map_chunks<'w>(
        &self,
        chunks: &[Chunk],
        make_worker: &WorkerFactory<'w>,
        cancel: &CancelFlag,
    ) -> Vec<ChunkResult>;
    fn threads(&self) -> usize;
    fn name(&self) -> &'static str;
}
```

| Executor | What it does |
|---|---|
| `SerialExecutor` | one worker, chunks in order. The reference: every other executor must produce byte-identical results to it. |
| `LocalExecutor::new(n)` | rayon over `n` threads (`0` = rayon's global pool). One worker — engine plus arena — per thread. |

A worker is made by a factory, once per thread, and owns its own `Engine`. Nothing is shared
mutably, so there is no lock in the hot path. Results come back as a `Vec` indexed by chunk
position, so parallelism cannot reorder anything:

```rust
let serial   = runner.project(&chunks, &SerialExecutor, &cancel)?;
let parallel = runner.project(&chunks, &LocalExecutor::new(4), &cancel)?;
// bit equality, asserted in the crate's own tests
assert_eq!(serial.chunks[0].column("term.bel").unwrap().values,
           parallel.chunks[0].column("term.bel").unwrap().values);
```

A distributed executor (`03-engine.md` §10) slots in behind the same trait without the kernel
knowing anything about it.

### Cancellation

`CancelFlag` is cooperative and **chunk-granular**: a worker checks it before starting a chunk,
never inside the `t` loop. Set it from any thread — a Ctrl-C handler, a UI button, Python across
`allow_threads` — and the run stops at the next chunk boundary:

```rust
let cancel = CancelFlag::new();
let flag = cancel.clone();
std::thread::spawn(move || { /* on Ctrl-C */ flag.cancel(); });

let projection = runner.project(&chunks, &executor, &cancel)?;
assert_eq!(projection.outcome, Outcome::Cancelled);   // and the manifest says so
```

---

## 4. Which components come out

`emit` (`01-ir.md` §8.4.3) chooses the emission set, and the runner adds whatever the
aggregations need on top:

| `emit` | Emitted |
|---|---|
| `"outputs"` (default) | every `kind = "Output"` component |
| `"all"` | every `Derived` and `Output` component — the migration mode |
| `"list"` | `emit_list` ∪ the outputs; an id that does not resolve is `E0106` |

Two rules worth knowing before you hit them:

- **`emit` never removes an `Output`.** There is no way to produce a result set that does not
  honour the product's promised manifest.
- **A `Series` component can only be emitted if the plan retained it in full.** Plan with
  `retain = "full"` (or `PlanOptions::retain_all`) when you want `emit = "all"`; otherwise the
  runner refuses with "…is retained as a ring and cannot be emitted" rather than writing a
  window of the projection and calling it the projection.

---

## 5. `[[aggregation]]`: the monoid stage

Aggregation runs strictly after all projections and cannot be written in an `Expr` — that is what
guarantees modelpoint independence, and therefore both the parallelism and the determinism.

```toml
[[aggregation]]
name     = "bel_by_cohort"
group_by = ["product_code", "entry_year_band"]   # ordered key tuple
measure  = "bel"
op       = "sum"          # sum | mean | min | max | count | weighted_mean
weight   = "num_pols_if"  # required iff op = "weighted_mean"
filter   = "in_force_at_val"
over_t   = "each"         # each | total ; only for a Series measure
```

What comes out, in `aggregates.parquet`:

```text
aggregation        group_key                            measure   t   value
bel_by_cohort      product_code=TERM_UK|entry_year…     bel      -1   12345678.90
```

- **Every `op` is a monoid**, so a chunk folds into a partial on whatever thread ran it, and
  partials combine **in chunk index order** — never completion order. Within a chunk, `sum`
  accumulates sequentially in `mp_row` order.
- **Groupings do not nest.** One flat row per distinct key tuple, no subtotals. A drill-down tree
  is built by a UI from successive prefixes of the ordered tuple, which is why the order is
  preserved into `group_key`.
- **Group keys are canonical text**, read from the modelpoint columns themselves — a `str` lane
  inside the kernel is a dictionary code private to the engine that ran it, and a code is not a
  join key.
- **A filtered-out or trapped modelpoint contributes to no group at all.**

A `Series` measure with `over_t = "total"` sums left to right in `t`, the same order the kernel's
own `sum` uses, so the aggregate and the component agree to the last bit.

---

## 6. `[[solve]]`: Brent around the projection

A solve is a root-find *outside* the engine (`03-engine.md` §5.4). The runner perturbs one input,
re-runs the whole chunk pipeline, and Brent-steps the residual `target - to`. The projection stays
a pure function; each iteration is itself deterministic, so the solved value is reproducible.

```toml
[[solve]]
name      = "premium_solve"
target    = "bel"          # a PerMP component (per_mp) or an [[aggregation]] (portfolio)
to        = 0.0
vary      = "annual_premium"
scope     = "per_mp"       # per_mp | portfolio
tolerance = 1e-8
max_iter  = 50
method    = "brent"
bracket   = [0.0, 1.0e6]
```

**`scope = "per_mp"` advances every modelpoint's own Brent state in lockstep**: one pipeline pass
evaluates the residual for every unconverged modelpoint at its own candidate value. Modelpoints
are independent, so this changes no answer — it turns `max_iter × n` projections into at most
`max_iter` of them. `vary` must be a modelpoint field here; a `Scalar` assumption has one value
for the whole run.

**`scope = "portfolio"`** is one scalar Brent over an `[[aggregation]]` value, varying either a
uniform modelpoint field or an assumption.

The manifest records the outcome in full (Q8) — a solved run whose solve is not recorded is not
reproducible:

```jsonc
"solves": [{
  "name": "premium_solve", "target": "term.bel", "to": 0.0,
  "vary": "annual_premium", "scope": "per_mp", "method": "brent",
  "converged": 9998, "not_converged": 2,
  "iterations": {"min": 4, "max": 50, "mean": 7.2, "total": 72431},
  "residual": {"max_abs": 4.1e-9, "argmax_mp": "POL03917"},
  "per_mp": {"path": "solves/premium_solve.parquet", "digest": "sha256:1d7c…"},
  "on_not_converged": "warn"
}]
```

`solves/<name>.parquet` carries `mp_key, mp_row, solved_value, residual, iterations, converged`,
and the solved value is *also* an ordinary `PerMP` component named after the solve in
`results.parquet`, so downstream joins do not special-case solves. A bracket with no sign change
is reported as not-converged rather than guessed at; `on_not_converged = "error"` (the default for
a portfolio solve) fails the run with `E0903`.

---

## 7. Traps and the outcome

`on_trap` (`01-ir.md` §9.3.1) is a run-level policy over a modelpoint-level failure:

| `on_trap` | Results | `execution.outcome` | Exit |
|---|---|---|---|
| `"abort"` (default) | **none written** | `aborted` | 2 |
| `"continue"` | written, without the trapping modelpoints | `completed_with_traps` | 1 |
| — (cancelled) | what completed | `cancelled` | 1 |
| — (clean) | written | `completed` | 0 |

Under `continue` a trapping modelpoint contributes **no rows at all** — not null rows, since there
are no nulls — and is excluded from every aggregation. `--max-errors N` caps how many `E0902`
envelopes are *retained*; the counts stay exact:

```rust
assert_eq!(projection.traps.len(), 1);            // retained, capped
assert_eq!(projection.modelpoints_trapped, 3);    // exact
```

Every retained trap is the `E0902` envelope of §9.3.1, with the component, the `ExprPath`, the
modelpoint key, `t` and the real operand values — captured by the kernel's scalar replay, not
re-derived.

---

## 8. What lands in `run/`

```text
run/
  manifest.json                what produced these numbers, and can I get them again
  results.parquet              long format, (chunk_idx, offset) order
  results.schema.json          the emitted component set + component_set_digest
  aggregates.parquet           one flat row per (aggregation, group tuple, t)   [if declared]
  solves/<name>.parquet        per-modelpoint solve outcomes                    [if per_mp]
  tables/<name>.parquet        the table content that was actually read         [unless suppressed]
```

Three manifest fields the runner owns:

- **`run_digest`** (Q1) — SHA-256 over the run file's canonical text with `[run.exec]`, `out` and
  `progress` removed. The rule, not a list: *a field participates iff changing it can change a
  number in `results.parquet`*. So threads and chunk size are out; `emit`, `retain`, `on_trap`,
  `max_errors`, `[run.tables]` and every solve and aggregation are in.
- **`lineage`** (Q12) — `parent_run`, `parent_manifest_digest`, `group_id`, `label`, `varied`. A
  sensitivity fan is grouped by `group_id`, never inferred from a filename, and `varied` is always
  *derivable* by diffing two manifests (`Manifest::derive_varied`).
- **Table copies** (Q13) — every table read is copied into `run/tables/<name>.parquet` with a
  `copy_digest` over the copy's own bytes, so an archived run can still show the resolved row
  behind a lookup. `--no-table-copy` stamps `copy: null`, and a consumer that finds it must say
  "table content unavailable in this run" rather than render a partial row.

For the full field list of each artefact, see [Run results and manifests](run-results.md).

---

## 9. Known edges

- **Column pruning is currently off.** `03-engine.md` §6 prunes a load to the fields some
  component reads, but the planner allocates a `PerMP` input slot for *every* declared
  `modelpoint_field` and the kernel binds all of them. Load with `MpSchema::from_module` until
  the planner drops slots nothing reads. Aggregation group keys are a second reason to: a
  `group_by` field no expression mentions still has to be in the chunk.
- **`str` and `enum` components are not emitted.** Their lanes hold per-engine dictionary codes;
  writing the code would put a meaningless integer in a result set, so the runner refuses instead.
  Group keys are unaffected — they are read from the source columns as text.
- **Chunks are materialised up front.** A modelpoint file is columns of `f64`; the projection is
  what costs memory, and having the whole work list in hand is what lets the solver replay it
  without re-reading the file.
