# The kernel

`predictable-engine` is the only crate that computes a number. It takes a [plan](planning.md), its
[tapes](tape-lowering.md), the [compiled tables](tables.md) and a chunk of modelpoints, and produces
values — with **no threads, no IO, no clock and no allocation in the projection loop**. It
implements [`03-engine.md` §4, §5.1 and §5.5](../design/03-engine.md) and the trap policy of
[`01-ir.md` §9.3.1](../design/01-ir.md) (decision Q7).

Everything about the design follows from four choices:

| Concern | Answer | Spec |
|---|---|---|
| Layout | `series_buf[slot][t_ring][c]` — chunks of `C = 1024` modelpoints, lanes innermost | §4.2 |
| Memory | one `ChunkBuffers` arena per worker, reused for every chunk, never zeroed | §4.4 |
| Errors | per-lane `TrapFlags`, checked once per period, then a scalar replay for the report | §5.5 |
| Reductions | strictly sequential in `t`, never vectorised | `01-ir.md` §9.2 |

Parallelism lives one layer up, in the runner, as a *partition of the modelpoint set*. It is never a
change to this crate — which is what makes "the same engine runs in the browser" true rather than
aspirational, and what makes `run(C = 1) ≡ run(C = 1024)` a property rather than a hope.

---

## 1. Running a model

```rust
use std::collections::BTreeMap;
use predictable_engine::{ChunkInput, Engine, RunConfig};

let mut engine = Engine::new(&plan, &tapes, tables, &timeline, RunConfig::default())?;

// One arena per worker thread, allocated once.
let mut bufs = engine.buffers();

// Scalars and the loop-invariant series: once per run.
engine.prepare(&assumptions, &mut bufs)?;

// Then every chunk through the same buffers.
for chunk in source {
    let out = engine.run_chunk(&mut bufs, &chunk)?;
}
```

A `ChunkInput` is columnar, exactly as `predictable-io` hands it over: one `Vec<f64>` per
modelpoint field, plus the keys and the file row of lane 0 (which the trap report needs).

```rust
let chunk = ChunkInput {
    index: 0,
    keys: vec!["POL1".into(), "POL2".into()],
    first_row: 0,
    columns: BTreeMap::from([
        ("sum_assured".to_string(), vec![100_000.0, 250_000.0]),
        ("q".to_string(), vec![0.01, 0.02]),
    ]),
};
```

Every lane carries an `f64`. `bool` is `0.0` / `1.0`; `date` is days since 1970-01-01; `str` and
`enum` are dictionary codes (`01-ir.md` §2.6 defines only equality on them, so comparing codes *is*
comparing values). `false` and `true` are interned first, so a boolean lane needs no special case in
the table-lookup path.

### The order of a run

```text
prepare:   Scalar slots, then the loop-invariant series               (once per run)
per chunk: bind modelpoint columns -> PerMP lanes
           prologue      PerMP derivations
           t = 0..=T     the stage-1 tape for the peel region of t
           stage 2       Reduce / Npv and the PerMP arithmetic over them
           emit          Output slots, tagged with the chunk index
```

---

## 2. Worked example: a term projection

```toml
[[component]]
name = "disc"
kind = "Derived"
shape = "Series"
timing = "start"
init = "1.0"
expr = "disc[t-1] * v_from_i(valuation_rate)"

[[component]]
name = "survivors"
kind = "Derived"
shape = "Series"
timing = "start"
init = "1.0"
expr = "survivors[t-1] * (1 - q)"

[[component]]
name = "claims"
kind = "Output"
shape = "Series"
timing = "start"
expr = "survivors * q * sum_assured"

[[component]]
name = "pv_claims"
kind = "Output"
shape = "PerMP"
expr = "npv(claims, disc)"
```

With `q = 0.01`, `sum_assured = 100 000` and `valuation_rate = 4%`:

```rust
let out = engine.run_chunk(&mut bufs, &chunk)?;
let claims = out.column("term.claims").unwrap();

assert_eq!(claims.lane(0)[0], 1000.0);              // 1.00 × 0.01 × 100 000
assert_eq!(claims.lane(0)[1], 0.99 * 0.01 * 100_000.0);
```

`out.column(name)` finds an output by qualified or bare name; `column.lane(c)` is that modelpoint's
values — `T + 1` of them for a series, one for a `PerMP` or `Scalar` output.

Three storage decisions are visible in that model, and the test suite asserts all of them:

* `survivors` is lagged by one and read by nothing else, so it is a **ring of 2 periods** — 16 KB per
  chunk instead of 4 MB.
* `claims` is an output *and* an `npv` argument, so it is **retained in full**, `(T+1) × C`.
* `disc` touches no modelpoint value at all, so the planner **hoisted** it: it lives in one shared
  `(T+1)` array with no modelpoint axis, computed once per run and broadcast to every lane.

---

## 3. Chunking is not observable

The chunk size `C` is a cache-tuning knob. It changes the working set, never the answer: lanes are
independent, arithmetic is element-wise, and reductions run left-to-right in `t` regardless of how
many modelpoints are in flight.

```rust
// 40 modelpoints in one chunk, and the same 40 one at a time.
assert_eq!(wide.lane(i).map(f64::to_bits), narrow[i].lane(0).map(f64::to_bits));
```

That is a real test (`tests/kernel.rs::chunk_size_one_equals_chunk_size_1024_bit_for_bit`), asserted
on **bit patterns** rather than on approximate equality, because "same manifest → byte-identical
results" is the product.

The arena is the other half of that guarantee. `ChunkBuffers::reset` clears bookkeeping but does not
zero memory — every slot is written before it is read, guaranteed by the topological order, and a
debug-build initialisation bitmap over the `PerMP` buffer asserts it rather than trusting it. A
narrower chunk after a wider one therefore cannot see the previous chunk's values, and the tests
check exactly that by running the same chunk through a reused arena and a fresh one.

---

## 4. Traps

A trap is a **modelpoint-level failure with a run-level policy** (`01-ir.md` §9.3.1). The hot loop
never returns a `Result` and never branches on data:

1. A trapping op (`Div`, `Pow`, `Lookup`, `ln`, …) writes a poison value and sets **one bit** in the
   chunk's `TrapFlags`.
2. Once per period, the kernel ORs the bitset — 16 `u64` words for `C = 1024` — instead of testing
   per op.
3. On a hit, and only then, the offending lanes are **replayed scalar-wise through the same tape**,
   with recording on. Because the kernel is deterministic the replay reproduces the trap exactly, so
   the operand values in the report are the ones the run really computed.

```rust
let err = engine.run_chunk(&mut bufs, &chunk).unwrap_err();
let EngineError::Trapped(log) = err else { unreachable!() };
let report = &log.reports()[0];
println!("{}", serde_json::to_string_pretty(&report.envelope())?);
```

```json
{
  "code": "E0902",
  "severity": "error",
  "kind": "trap",
  "trap": "div_by_zero",
  "component": "trap.ratio",
  "expr_path": "expr",
  "mp_key": "MP1",
  "mp_row": 1,
  "t": 0,
  "message": "division by zero evaluating `trap.ratio` at t = 0",
  "operands": { "lhs": 1.0, "rhs": 0.0 },
  "doc_url": "https://predictable.dev/diagnostics/E0902"
}
```

The trap kinds are closed: `div_by_zero`, `log_non_positive`, `pow_nan`, `lookup_miss`,
`index_out_of_range`, `overflow_to_inf`, `not_finite`. NaN and ±Inf never silently enter results.

### Policy

| `on_trap` | Behaviour |
|---|---|
| `abort` (default) | the run stops, `EngineError::Trapped` carries the log, and no results are written |
| `continue` | the modelpoint contributes **no rows at all** — not null rows, none — and is listed in `ChunkOutput::dropped`; the rest of the chunk finishes |

```rust
assert_eq!(out.keys,    vec!["MP0", "MP2"]);   // survivors, in file order
assert_eq!(out.dropped, vec!["MP1"]);          // abandoned at the trap
```

### `--max-errors`

`RunConfig::max_errors` (default **100**, per `01-ir.md` §9.3.1) caps the number of reports
*retained*. The counts stay exact:

```rust
assert_eq!(traps.total(), 4);          // every trap is counted
assert_eq!(traps.reports().len(), 2);  // only two are kept
assert!(traps.truncated());
```

Reports are retained in ascending lane order, so a capped set is still deterministic.

### Guarded division does not trap

`If` evaluates both arms (`01-ir.md` §2.6), so the division below really does execute on every lane.
What stops it being reported is the *static* mask register the lowering attached to it:

```toml
expr = "if den == 0.0 then 0.0 else num / den"
```

```rust
assert_eq!(ratio.lane(1)[0], 0.0);     // the guarded lane took the safe arm
assert!(out.traps.is_empty());         // and nothing was reported
```

---

## 5. Numerical conventions

* **`round(x, dp)` is half-away-from-zero on the shortest decimal representation of `x`**, not IEEE
  half-even (`01-ir.md` §2.8). This is the `2.675` trap: the binary value is really
  `2.67499999999999982…`, so scaling and rounding gives `2.67`, while Prophet and actuarial
  convention give `2.68`. Rounding the shortest decimal — what the author actually wrote — gives
  `2.68`.
* **Reductions are a plain left-to-right fold.** No pairwise summation, no SIMD reassociation.
  `sum_kahan` is a separately named builtin and the only compensated variant.
* **`count_while` stops at the first false** and returns that count; it does not count all true
  periods.
* **`npv(x, disc)` uses `x`'s timing tag.** `disc` is a cumulative discount-factor series, so a
  `start`/`point` flow discounts at `disc[t]` directly. An `end` or `mid` flow needs the fractional
  step, and the kernel takes it from the curve itself — `f = disc[t+1] / disc[t]`, applied as
  `disc[t] · f^(exponent − t)`. On a flat curve that is exactly the spec's `v^t`, `v^(t+1)`,
  `v^(t+0.5)`; on a real curve it stays consistent with the curve rather than silently assuming a
  flat one.
* **Dates** are days since 1970-01-01, converted with Hinnant's algorithms — exact, branch-light and
  identical on every target. `add_months` clamps a 31st into February to the 28th/29th.

---

## 6. What this crate does not do

| Not here | Where |
|---|---|
| Threads, rayon, chunk scheduling | `predictable-runner` (T15) |
| Reading Parquet/CSV/MPF, writing results | `predictable-io` (T13/T14) |
| Substage levelling for the stage-2 → `init` back-channel | T11 |
| `explain()` recording traces | T23 |

The kernel executes every op in the set, including `Reduce` and `Npv`; what T11 adds is the
*scheduling* of stage-1 slots into levels when an `init` reads a stage-2 value
(`03-engine.md` §5.3).

---

## 7. Known deviations

* **All lanes are `f64`.** `03-engine.md` §4.2 describes `[i64; C]` and bitset lanes for integer and
  boolean slots. The kernel keeps a single `f64` lane per slot, which is exact for every integer
  below 2⁵³ and for every date the IR can express; splitting the lane types is a
  performance/precision change that can be made without touching the op set.
* **Timeline dates are computed from `valuation_date` for both origins.** Under `origin = "policy"` a
  policy-relative `period_start_date` needs the modelpoint's entry date, which arrives with T13's
  modelpoint schema; `policy_year`, `policy_month` and `is_anniversary` are already correct because
  they are functions of `t` alone.
* **`libm` is the host's.** `exp`, `ln` and `powf` come from `std`. Byte-identical results across
  x86-64, aarch64 and wasm need the vendored `libm` of `03-engine.md` §7; that is T16's gate, and the
  kernel is structured so the swap is a one-line change in `ops.rs`.
