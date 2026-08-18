# 03 — The predictable engine

Status: **normative** for the Rust runtime. Version: engine `0.1`, executes IR `pir/1`.
Depends on: `01-ir.md` (normative). Where this document and the IR spec disagree, the IR spec wins;
gaps are recorded in §12.

The engine is the only thing that executes a model. Its job statement, in order of priority:

1. **Correct and bit-reproducible.** Same manifest → byte-identical results, on any thread count,
   any machine, any of native / WASM / hosted.
2. **Fast enough that the benchmark demo is not close.** Target: ≥ 20× cashflower, ≥ 5× lifelib on
   the reference term-assurance model (§11), single core; near-linear to core count.
3. **Explainable.** Every value is reproducible by a single-modelpoint replay with recording on,
   with zero cost paid in the hot path.

---

## 1. Crate structure

Nine crates, one workspace. The split is drawn along *dependency weight*, because the WASM and the
`no-io` builds must not drag in rayon, mmap, or Arrow.

```
crates/
  predictable-syntax/     # .pir lexer, parser, formatter, spans, diagnostics rendering
  predictable-ir/         # IR data model, resolver, checker (passes 1–6), digests, JSON encoding
  predictable-engine/     # planner + runtime. NO io, NO threads, NO python. `#![no_std]`-adjacent.
  predictable-prophet/    # MPF / .fac / .rpt readers (04-verify §4). Pure `bytes -> (Value, [Diag])`.
  predictable-io/         # modelpoint readers, table loaders, result writers
  predictable-runner/     # orchestration: chunking, rayon, solve loop, aggregation, manifest
  predictable-cli/        # `predictable` binary
  predictable-py/         # PyO3 module `predictable._engine`, maturin-built wheel
  predictable-wasm/       # wasm-bindgen surface for the browser UI
```

Dependency rules, enforced by a `cargo-deny`-style CI check plus `#![forbid]` lints:

| crate | may depend on | forbidden |
|---|---|---|
| `syntax` | `logos`, `annotate-snippets` | serde_json in the hot path |
| `ir` | `syntax`, `serde`, `sha2` | rayon, arrow, pyo3, std::fs |
| `engine` | `ir` | **rayon, std::fs, std::time, pyo3, arrow** |
| `prophet` | `ir`, `syntax` | rayon, arrow, pyo3, std::fs (callers supply bytes) |
| `io` | `ir`, `prophet`, `arrow`, `parquet`, `csv`, `memmap2` | pyo3 |
| `runner` | all of the above, `rayon` | pyo3 |
| `py` | `runner`, `io`, `pyo3`, `arrow` | — |
| `wasm` | `ir`, `engine`, `prophet`, `io/no-mmap` | rayon (default), memmap2 |

`predictable-engine` being free of threads and IO is the load-bearing rule: it makes the kernel
trivially testable, trivially WASM-able, and makes "the same engine runs in the browser" true rather
than aspirational. Parallelism lives one layer up, in `runner`, and is a *partition* of the
modelpoint set, never a change to the kernel.

Feature flags: `engine/trace` (recording evaluator, compiled out by default), `io/mmap`,
`runner/parallel` (default on native, off on wasm32), `io/parquet`.

---

## 2. Loading: text → IR → plan

Three artefacts, three lifetimes.

```rust
// predictable-ir
pub struct Module   { path: ModulePath, components: Vec<Component>, tables: Vec<TableDecl>, .. }
pub struct Program  { modules: Vec<Module>, timeline: Timeline, enums: Vec<EnumDecl>,
                      digest: Sha256 }          // checked; immutable; the IR of 01-ir.md §2
pub struct Plan     { .. }                       // §3; a function of Program + RunConfig only
pub struct Run      { plan: Arc<Plan>, tables: Arc<TableSet>, assumptions: AssumptionValues, .. }
```

### 2.1 Parse and check

`predictable-syntax` parses `.pir` with a hand-written recursive-descent parser over a `logos` lexer.
Hand-written, not a generator: error recovery and span quality are the product, and §7 of the IR spec
requires multiple diagnostics per pass.

Spans are `(FileId, u32, u32)` byte offsets into an interned `SourceMap`; line/col is computed lazily
on render only. Every `Expr` node is 16 bytes plus its span in a side table indexed by `ExprId`:

```rust
pub struct ExprArena { nodes: Vec<ExprNode>, spans: Vec<Span> }   // ExprId = u32 index
pub enum ExprNode {
    Lit(LitId), Ref(SlotId), Lag(SlotId, u32), At(SlotId, u32),
    Unary(UnOp, ExprId), Binary(BinOp, ExprId, ExprId),
    If(ExprId, ExprId, ExprId), Call(Builtin, ExprRange),
    Lookup(TableId, ExprRange), Agg(AggOp, ExprId, Option<ExprId>),
}
```

Flat arena, `u32` ids, no `Box`. This matters twice: it keeps the checker cache-friendly, and it
makes the AST-level model diff (IR spec §11.3) a straightforward structural walk with stable ids.

The six checker passes of IR spec §7 run in `predictable-ir` and produce `Vec<Diagnostic>`. A
`Program` cannot be constructed while any `severity = Error` diagnostic exists — the type system
enforces "checked before executed", so the runtime has no code path for an unchecked model.

**The engine never sees text.** `Plan` construction takes `&Program`. Deserialisation of the JSON
encoding (`pir.json`) is an alternate front end into the same `Program`, used by the WASM build when
the browser already holds a parsed model.

### 2.2 The `.pir` load is not the hot path

Parsing + checking a 500-component model is budgeted at < 20 ms. It is not optimised beyond arena
allocation. Loading a 5-million-row modelpoint file is the hot path and is `predictable-io`'s problem
(§6).

---

## 3. Planning: from graph to tape

The planner is the compiler. It runs once per `(Program, RunConfig)` and produces a `Plan` that is
`Send + Sync` and shared by every worker thread.

### 3.1 Slot allocation

Every value the runtime can name is a **slot**, assigned a dense `u32` at plan time, partitioned by
shape:

```rust
pub struct Plan {
    pub scalars:  Vec<ScalarSlot>,     // Shape::Scalar   — computed once per run
    pub permp:    Vec<PerMpSlot>,      // Shape::PerMP    — one f64 per modelpoint
    pub series:   Vec<SeriesSlot>,     // Shape::Series   — one f64 per (modelpoint, t)
    pub stage1:   Tape,                // per-t body, deterministic topo order (IR §3.2)
    pub stage2:   Tape,                // Agg reductions + PerMP arithmetic
    pub prologue: Tape,                // Scalar + PerMP inputs and pure-PerMP derivations
    pub hoisted:  Tape,                // loop-invariant Series (IR §2.3) — computed once per run
    pub layout:   Layout,              // §4
    pub outputs:  Vec<SlotId>,
    pub digest:   PlanDigest,          // sha256(program_digest ‖ run_config ‖ engine_semver_major)
}
```

`SeriesSlot` carries the fields the runtime actually needs, denormalised out of the IR:

```rust
pub struct SeriesSlot {
    pub id: SlotId, pub dtype: DType, pub timing: Timing,
    pub max_lag: u32,              // largest k of any Lag(self, k) — 0 if never lagged
    pub retention: Retention,      // Ring(k+1) | Full   — §4.3
    pub init: Option<TapeRange>,   // t=0 seed program; may read stage-2 slots (IR §8.2)
    pub hoistable: bool,           // expr's transitive refs touch no PerMP slot and no modelpoint field
    pub component: ComponentId,    // back-pointer for diagnostics and explain()
}
```

### 3.2 Ordering

Kahn's algorithm with a `BinaryHeap<Reverse<(ModulePathId, DeclIndex)>>` ready set, exactly as IR
spec §3.2 mandates. The planner asserts the resulting order is a total order and stores it; nothing
downstream may reorder. A `Plan` records `order_digest = sha256(slot ids in order)`, checked in the
golden-corpus tests — a change in topo order is a diffable, reviewable event, not an accident of a
`HashMap`.

All internal maps in the planner are `IndexMap`/`BTreeMap`. `std::collections::HashMap` iteration is
banned in `ir` and `engine` by lint; where a hash map is used for lookup only, its iteration is never
observed.

### 3.3 Lowering `Expr` to a tape

The runtime does **not** walk the `Expr` tree. Each component's expression is lowered to a flat,
register-machine tape in reverse-postorder, so evaluation is a single forward pass over a `Vec` with
no pointer chasing and no recursion (and therefore no stack-depth limit — relevant for WASM).

```rust
pub struct Tape { pub ops: Vec<Op>, pub n_regs: u16, pub const_pool: Vec<f64>, pub spans: Vec<Span> }

#[repr(u8)]
pub enum Op {
    ConstF(Reg, ConstId),
    LoadScalar(Reg, SlotId),
    LoadPerMp(Reg, SlotId),
    LoadCur(Reg, SlotId),                  // x[t]
    LoadLag(Reg, SlotId, u32),             // x[t-k], with init/zero fill below origin
    LoadAt(Reg, SlotId, u32),              // x[k]
    LoadTime(Reg, TimeField),              // t, policy_year, year_frac, ...
    Add(Reg, Reg, Reg), Sub(..), Mul(..), Div(Reg, Reg, Reg, SpanId),   // Div carries a span: it traps
    Neg(Reg, Reg), Pow(Reg, Reg, Reg, SpanId),
    Cmp(Reg, CmpOp, Reg, Reg), And(..), Or(..), Not(..),
    Select(Reg, Reg, Reg, Reg),            // if — both arms already evaluated (IR §2.6)
    Call1(Reg, Fn1, Reg), Call2(Reg, Fn2, Reg, Reg), CallN(Reg, FnN, RegRange),
    Lookup(Reg, TableId, RegRange, SpanId),
    StoreCur(SlotId, Reg),                 // commit x[t]
    StorePerMp(SlotId, Reg),
    // stage 2 only:
    Reduce(Reg, AggOp, SlotId, Option<Reg>),        // sum / max_over / count_while over the full series
    Npv(Reg, SlotId, SlotId, Timing),               // value series, discount series, timing exponent
}
```

Design notes that are decisions, not incidentals:

- **`Select`, not a branch.** Both arms are evaluated (IR §2.6 requires it), so the op is
  branch-free and the block loop (§5) has no per-modelpoint divergence. Trapping ops inside an
  untaken arm are the one wrinkle: `Div` and `Lookup` inside an `If` arm are lowered with a
  *masked* variant (`DivMasked`) that suppresses the trap where the mask is false. The planner
  computes the mask statically from the enclosing `If` chain; there is no dynamic predicate stack.
- **Registers are per-block scratch**, `u16`-indexed into a `SmallVec`-backed frame reused across
  `t` and across chunks. Linear-scan allocation over the tape; typical models use < 32.
- **`Reduce`/`Npv` read a whole retained series** and are strictly sequential in `t`
  (IR spec §9.2). No pairwise summation, no SIMD reassociation, ever. `sum_kahan` is a distinct
  `AggOp` and is the only compensated variant.
- **`LoadLag` below the origin** resolves at plan time, not run time: the planner emits either
  `LoadLag` (in range) or a `ConstF`/`init`-tape reference for the `t < k` prefix, by peeling the
  first `max_lag` iterations of the `t` loop (§5.2). The `pre_origin_default` provenance note is
  recorded on the peeled ops so `explain()` can report it.
- **`At(x, k)`** is a `LoadAt` against a retained series; the planner forces
  `retention = Full` on any slot that is the target of an `At`, and hoists the load out of the loop
  when `k < t_min` for the current peel region.

### 3.4 Loop-invariant hoisting

IR spec §2.3 promises the model sees a uniform `Series` while a yield curve indexed by `t` alone is
computed once. The planner implements exactly that: a `Series` slot is `hoistable` iff its transitive
`lag = 0` and `lag ≥ 1` dependency closure contains no `PerMP` slot, no modelpoint field, and no
`Lookup` whose keys depend on either. Hoisted slots are evaluated once per *run* into a
`(T+1)`-length shared `Arc<[f64]>` and read by every chunk. `LoadCur` on a hoisted slot lowers to
`LoadHoisted`, a stride-0 broadcast.

For a typical monthly model this hoists discount factors, inflation indices, and the whole timeline
— often 10–20% of series ops removed from the per-modelpoint work.

### 3.5 Constant folding and the small stuff

Constant folding over `Scalar` inputs, algebraic identities restricted to those that are exactly
IEEE-preserving (`x * 1.0`, `x + 0.0` are **not** folded — they differ on `-0.0` and NaN; `x - x` is
not folded), common-subexpression elimination within a single tape only (across components CSE is
suppressed because it would collapse nodes that `explain()` must show separately). Optimisation is
allowed to change *speed*, never *bits*: every planner pass is covered by a differential test
asserting `plan_opt(m) ≡bits plan_noopt(m)` on the golden corpus, and `--O0` disables all of it for
bisecting.

---

## 4. Storage layout

### 4.1 The shape of the problem

A run is `M` modelpoints × `T+1` periods × `S` series slots. For a monthly 40-year model,
`T+1 = 481`; a rich IFRS 17 model has `S ≈ 300`. Per modelpoint that is 481 × 300 × 8 B ≈ **1.15 MB**
— fine alone, catastrophic at `M = 5,000,000` (5.8 TB). So the layout question is really the
*retention and streaming* question.

### 4.2 Chunked, column-major, blocked

Modelpoints are processed in **chunks** of `C` (default 1024, tunable, always a power of two). Within
a chunk, every series slot is a contiguous `[f64; C]` **lane** per period:

```
series_buf[slot][t_ring][c]        // c = modelpoint index within chunk, contiguous, stride 8 B
```

The innermost loop is over `c`. That is the single most important layout decision in the engine:

- **Vectorisation for free.** `Add(r0, r1, r2)` becomes a 1024-wide `f64` loop that LLVM
  auto-vectorises to AVX2/AVX-512/NEON without any intrinsics, and — critically — element-wise
  arithmetic is *lane-independent*, so vectorising it does not reassociate anything and cannot change
  a bit. (Reductions are the opposite case and are never vectorised: §7.)
- **Interpreter dispatch amortised 1024×.** The cost of the `match` on `Op` is paid once per 1024
  modelpoints, so a plain interpreter reaches within ~1.5–2× of compiled code. This is why the engine
  ships an interpreter in 0.1 and treats JIT as an optional later win (§10), not a prerequisite.
- **Cache-resident working set.** With `C = 1024`, one lane is 8 KB. A model with 40 live series and
  a lag window of 2 has a hot set of ~640 KB — L2-resident on every current target. `C` is chosen at
  runtime as `clamp(L2_bytes / (live_lanes * 8 * 2), 256, 8192)`, rounded down to a power of two, and
  is *not* allowed to affect results (§7), so tuning it is safe.

`PerMP` slots are `[f64; C]` singletons; `Scalar` slots are a per-run `Vec<f64>` read by all chunks.
`i64`/`bool`/`date` slots use `[i64; C]` / bitset lanes; `str` and `enum` modelpoint fields are
dictionary-encoded to `u32` at load, and only equality is defined on them (IR §2.6), so the runtime
compares `u32`s.

### 4.3 Retention: ring vs full

Per slot, the planner picks:

- **`Ring(max_lag + 1)`** — the default. The slot is never read by an `At`, never reduced by a
  stage-2 `Agg`, and is not an `Output`. Storage is `(max_lag+1) × C × 8` B. For the overwhelming
  majority of intermediate series, `max_lag ∈ {0, 1}`, so 1–2 periods are kept. Indexing is
  `t & (ring_len - 1)` with ring lengths rounded up to a power of two — no modulo, no branch.
- **`Full((T+1) × C)`** — required when the slot is an `Output`, is the argument of a stage-2 `Agg`,
  or is the target of an `At(x, k)` with `k` not statically peelable.

This is a straightforward liveness analysis over the dependence graph and it is where the memory
strategy lives. On the reference term model, 4 of 17 series slots are `Full`; the chunk footprint
drops from 17 × 481 × 1024 × 8 B = 67 MB to 4 × 481 × 1024 × 8 B + 13 × 2 × 1024 × 8 B ≈ **16 MB**.
Reduce `C` to 256 and it is 4 MB per worker thread.

`--retain-all` forces every slot `Full` (needed for whole-run drill-down in the UI, and for the
`explain()` UI's "show me every series for this policy" view). The CLI warns with the projected
footprint before allocating.

### 4.4 Arenas and allocation

One `ChunkBuffers` per worker thread, allocated once at run start and **reused for every chunk** —
zero allocation in the projection loop, which is both a speed and a determinism property (no
allocator-dependent behaviour). Buffers are 64-byte aligned. `ChunkBuffers::reset()` does not zero
memory; every slot is written before it is read, guaranteed by the topological order, and this is
asserted in debug builds with a per-lane initialisation bitmap.

```rust
pub struct ChunkBuffers {
    series: AlignedVec<f64>,     // one flat allocation, slot offsets precomputed in Plan::layout
    permp:  AlignedVec<f64>,
    ints:   AlignedVec<i64>,
    bits:   BitVec,
    regs:   AlignedVec<f64>,     // C × n_regs scratch
}
```

`Plan::layout` holds `Vec<SlotOffset>` so slot → byte offset is one array read, no map.

---

## 5. Execution

### 5.1 The run

```
run(plan, tables, assumptions, mp_source) ->
    prologue:      evaluate Scalar slots                          (once)
    hoisted:       evaluate loop-invariant Series                 (once, T+1 wide, shared Arc)
    for each chunk of C modelpoints (parallel over chunks):
        load:      modelpoint columns -> PerMP lanes              (§6)
        prologue2: evaluate pure-PerMP derived slots
        t = 0:     init tapes (may read stage-2 slots -> deferred, see 5.3)
        for t in 0..=T:
            for op in plan.stage1.ops:  apply to all C lanes
        stage2:    Reduce / Npv over Full series -> PerMP lanes
        emit:      Output slots -> result writer, tagged with chunk index
```

### 5.2 Loop peeling for the origin

The first `max_lag_global` periods are the only ones where `LoadLag` can fall below the origin, and
`t = 0` is the only period where `init` applies. Rather than branch inside the hot loop, the planner
emits **three tapes**: `t0_tape` (init seeds + the `t = 0` body), `prefix_tape` for
`t ∈ 1..max_lag_global` (pre-origin lags resolved to `init`/zero constants), and `body_tape` for
`t ≥ max_lag_global`. Since `max_lag_global` is typically 1, this costs three tapes and removes a
branch from every op that reads a lag. `At(x, k)` similarly splits the loop at `t = k` when `k` is
small, per IR §3.1's period-specific acyclicity rule.

### 5.3 The two stages and the `init` back-channel

IR §8.2 allows a stage-1 `init` to read a stage-2 value (`reserve.init = bel`, where `bel` is an
npv). This is a genuine dependency inversion and the engine resolves it by **substage scheduling**
within a chunk, not by a second pass over the data:

1. Partition stage-1 slots into *levels*: a slot is at level `L` if its `init` reads stage-2 slots
   whose own series arguments are all at level `< L`.
2. Run the `t` loop once per level, over that level's slots only, with earlier levels' `Full` series
   already materialised.

Level 0 is everything without a stage-2 `init`; the reference model has exactly two levels. The
planner reports level count, and emits `W0110` if a model exceeds 3 levels ("each level is a full
re-traversal of the projection; consider whether `reserve` really needs a prospective seed"). Chunk
buffers are sized for the union, so levels cost time, never memory.

Cost: level `L`'s loop only touches slots at level `L`, so total work is still one pass over each
slot — the levels partition, they do not duplicate.

### 5.4 Solvers

`[[solve]]` blocks (IR §8.2) wrap the whole projection: `runner` re-executes the chunk pipeline with
a perturbed assumption slot, Brent on the scalar residual, `max_iter` bounded, and **each iteration
is itself deterministic**, so the solved value is reproducible. Solve state never enters the engine
kernel; `predictable-engine` remains a pure function.

### 5.5 Errors and traps

Traps (IR §9.3) are `Result`-free in the hot loop: an op that can trap writes into a per-lane
`TrapFlags` bitset and produces a poison value, and the loop checks `traps.any()` once per period
(one `u64` OR-reduce over 16 words for `C = 1024`) rather than per op. On the first period with a
trap set, the chunk is *re-run in scalar diagnostic mode* for the offending lanes only, which
reproduces the trap with the component, modelpoint key, `t`, and span attached. Same replay
technique as `explain()`, same guarantee: determinism makes the replay exact.

`--max-errors N` (default 20) collects traps across modelpoints before aborting, so a bad MPF
produces a report, not a first-failure stop.

---

## 6. IO and the modelpoint pipeline

`predictable-io` owns everything the kernel refuses to.

**Modelpoint sources**, behind one trait:

```rust
pub trait ModelpointSource: Send {
    fn schema(&self) -> &MpSchema;
    fn next_chunk(&mut self, n: usize, out: &mut ChunkColumns) -> Result<usize, IoError>;
}
```

Implementations: `ParquetSource` (preferred; column pruning to exactly the fields the model
references — a model touching 7 of an MPF's 60 columns reads 7), `CsvSource`, `MpfSource` (Prophet
MPF: the `!` header line, `VARIABLE_TYPES`, `*` comment rows, the `T`/`I`/`S` type codes),
`ArrowSource` (a `RecordBatchReader` handed over from Python, zero-copy).

Loading is **columnar and typed at plan time**: the schema is in the IR (IR §2.10), so the reader
knows every column's type and target slot before touching the file and can dispatch a
`Vec<ColumnCopy>` of monomorphised copy closures. Missing required column → the definition-time-shaped
error of IR §2.10, naming the file, the column, and the referencing components.

**Deterministic order.** Chunks are indexed by their position in the file. Results carry the chunk
index and the within-chunk offset, and the writer emits in `(chunk_idx, offset)` order regardless of
completion order (IR §9.1). With Parquet the row-group boundaries are ignored for chunking; the
source yields exactly `C` rows at a time so chunk boundaries are a function of `C` and the row count
only.

**Tables** are loaded once, validated against the declared key policies, digest-checked (IR §2.9),
and compiled into a lookup structure per policy:
- `exact` on integer keys → dense `Vec` indexed by `key - min` when density > 0.5, else a
  perfect-hash (`phf`-style) built at load.
- `clamp` / `step` → sorted `Vec` + branchless binary search; `step` returns the predecessor.
- `interpolate` → the same sorted vector plus a fused `lerp`.
Multi-key tables are compiled to a *row-major dense array* over the cartesian product when the
product is < 2²⁴ (the common mortality case: age × gender × smoker = 121 × 2 × 2), so a lookup is one
multiply-add and one load. This is the single largest speed win over pandas/dict-based competitors,
and it is exact: no float keys in the index path.

**Results** are Arrow `RecordBatch`es: one long-format batch per output shape
(`Series`: `modelpoint_key, t, component, value`; `PerMP`: `modelpoint_key, component, value`), or
wide format on request. Written to Parquet (default), CSV, or handed to Python zero-copy (§8). The
run manifest (IR §9.4) is a sidecar `manifest.json` plus an embedded Parquet key-value metadata copy.

---

## 7. Determinism and bit-reproducibility

The IR spec makes determinism a design property; the engine's job is not to lose it. Concretely:

1. **Lane independence.** No op reads across the `c` axis. Cross-modelpoint arithmetic is not
   expressible (IR §8.3), so `C`, thread count, chunk boundaries, and SIMD width are provably
   unobservable. This is the invariant that makes everything else easy, and it is fuzz-tested:
   `prop_assert_eq!(run(m, C=1), run(m, C=1024))` bit-for-bit, over random valid programs.
2. **Reductions are sequential.** `Reduce` and `Npv` walk `t = 0..=T` in order, per lane. They are
   explicitly `#[inline(never)]` and marked with a comment forbidding vectorisation; a CI test
   compares against a reference scalar implementation on adversarial inputs (alternating large/small
   magnitudes) where any reassociation would show.
3. **No FMA contraction.** The workspace builds with `-C llvm-args=-ffp-contract=off`; a CI test
   evaluates `a*b + c` against a soft-float reference on values chosen so that FMA and mul-then-add
   differ, on x86-64, aarch64, and wasm32. `f64::mul_add` is never called except behind the explicit
   `fma()` builtin (not in IR 1.0 — see §12).
4. **No fast-math, no `-Ofast`, no `unsafe` float shortcuts.** `libm` transcendentals are used
   instead of the platform libm for `exp`, `ln`, `pow`, `sqrt` — vendored, deterministic, identical
   on macOS/glibc/musl/wasm. This is non-negotiable: glibc's `pow` and macOS's `pow` differ in the
   last ULP, and an actuary reconciling to the penny will find it. `sqrt` uses the IEEE-exact
   hardware instruction.
5. **No wall-clock, no RNG, no environment reads** inside `predictable-engine` — enforced by the
   crate's forbidden-dependency list and a `#[deny]` on `std::time`.
6. **Digest chain.** `run_digest = sha256(model_digest ‖ assumption_digest ‖ modelpoint_digest ‖
   table_digests ‖ timeline ‖ run_config_canonical ‖ engine_version)`. `plan_digest` and
   `order_digest` are recorded too, so "why did results change" can be answered structurally before
   anyone looks at numbers.
7. **Golden corpus.** `tests/golden/` holds ~30 models with pinned result Parquets and their
   manifests. CI runs them on `x86_64-linux`, `aarch64-macos`, and `wasm32-wasi` under wasmtime, and
   requires byte-identical output on all three. A deliberate result change requires regenerating
   goldens in the same commit, which makes it reviewable — the same principle as `.pir` diffability.

The one honest caveat, stated in the docs rather than buried: results are bit-identical across
platforms **for the same engine version**. Across engine versions, patch releases must match
(enforced by the corpus), minor releases must match unless they declare a `results-affecting` flag in
the changelog, which requires a headline entry.

---

## 8. The PyO3 boundary

`predictable-py` exposes a deliberately small surface. The Python DSL does authoring and validation;
the engine does compilation and execution. **No Python callback is ever invoked during projection**
(IR rule 1), so the GIL is released for the entire run.

```python
from predictable._engine import Program, Plan, Run, check

prog   = Program.from_pir(paths)            # or Program.from_json(obj) — the DSL's emit path
diags  = check(prog)                        # list[dict]; the --json diagnostics of IR §7
plan   = prog.plan(run_config)              # raises PredictableError with rendered diagnostics
result = plan.run(modelpoints=<path | pyarrow.Table | pandas.DataFrame>,
                  assumptions={"valuation_rate": 0.035, ...},
                  tables=None,              # default: resolve from IR `source` + digest check
                  threads=None, chunk_size=None, retain="auto", progress=cb_or_None)
result.to_arrow()      # pyarrow.Table, zero-copy
result.to_pandas()     # via arrow, one copy
result.explain("reserve", modelpoint="POL00042", t=3)   # -> dict, the tree of IR §11.2
```

### 8.1 Zero-copy, via the Arrow C Data Interface — decided

Results are already Arrow `RecordBatch`es in `predictable-io` (§6). They cross into Python through
`arrow-rs`'s `FFI_ArrowArray` / `FFI_ArrowSchema` C Data Interface and are adopted by `pyarrow` with
`pyarrow.Array._import_from_c` — **no copy, no serialisation, ownership transferred via the release
callback**. `pyarrow` is a runtime-optional dependency: without it, `to_numpy()` falls back to the
buffer protocol over the same allocation, still zero-copy for the `f64` value column.

Inbound modelpoints take the same path in reverse: a `pyarrow.Table` is imported via
`ArrowArrayStreamReader`, and `ArrowSource` reads its buffers directly. A pandas DataFrame is
converted by pyarrow (one copy, on the user's side, visible in profiles) — the docs steer users to
Parquet paths or Arrow tables for large portfolios.

Rejected alternatives, briefly: pickling (copies, slow, version-fragile); numpy structured arrays
(no string/dictionary story, no nulls); a bespoke buffer protocol (would need re-implementing what
Arrow already standardised, and the CLI/WASM/hosted paths want Arrow anyway).

### 8.2 GIL, errors, cancellation

- `plan.run()` wraps the whole projection in `Python::allow_threads`. Progress callbacks are opt-in
  and are invoked **between chunks** with the GIL re-acquired, at a bounded rate (≥ 100 ms apart);
  they receive counts only and cannot influence results.
- Rust errors map to a `PredictableError` hierarchy (`ParseError`, `CheckError`, `DataError`,
  `TrapError`) each carrying `.diagnostics` (the JSON of IR §7) and a pre-rendered `.pretty` string.
  A traceback that begins with an Elm-grade rendered diagnostic is the whole point.
- `Ctrl-C`: the runner polls a `AtomicBool` cancel flag between chunks and Python's signal handler
  sets it, so long runs are interruptible without `SIGKILL`.

### 8.3 Wheels

maturin, `abi3-py39`, so one wheel per platform covers every Python ≥ 3.9. Targets:
`manylinux2014 x86_64/aarch64`, `macos universal2`, `windows x86_64`. Built with a fixed Rust
toolchain pinned in `rust-toolchain.toml`, because §7 promises cross-platform bit-identity and that
promise is only testable against a pinned compiler.

---

## 9. WASM

The browser build serves `model.show()` / `results.show()` (the visualisation layer) and the
"try it in the docs" demo. It runs the *same* `predictable-engine` code — this is why the crate has
no IO, no threads, and no `std::time`.

- Target `wasm32-unknown-unknown` + `wasm-bindgen` for the browser; `wasm32-wasip1` for the
  CI determinism check under wasmtime.
- **Threads off by default.** `runner/parallel` is a feature; the WASM build runs chunks serially in
  a Web Worker, which is fine at the sizes a browser sees (≤ 50k modelpoints). Optional
  `wasm-bindgen-rayon` + SharedArrayBuffer behind a feature flag, gated on COOP/COEP headers being
  present; §7's lane independence means enabling it cannot change results.
- **Determinism holds** because WASM `f64` arithmetic is IEEE-754 with no contraction by design, and
  the vendored `libm` removes the transcendental divergence. This is checked in CI, not assumed.
- **Memory.** `wasm32` is 32-bit: chunk sizing uses a 512 MB budget and `C` defaults to 256. Bulk
  memory and SIMD128 are enabled (`-C target-feature=+simd128,+bulk-memory`); SIMD128 only ever
  accelerates lane-parallel ops, never reductions.
- **Size budget:** engine + parser + checker ≤ 1.5 MB gzipped. `panic = "abort"`, `opt-level = "z"`
  for the wasm profile, `wasm-opt -Oz`, no `serde_json` in the wasm feature set (the browser hands in
  already-parsed JS objects via `serde-wasm-bindgen`).
- Exposed surface: `check(pir_text) -> Diagnostic[]`, `plan(program, config)`,
  `run(plan, modelpoints_arrow_ipc) -> Arrow IPC bytes`, `explain(...)`. The UI therefore gets
  live in-browser validation with real engine diagnostics — the same messages the CLI prints.

---

## 10. The seam for distributed / hosted execution

Not built in 0.1, but the seam is placed now because retrofitting it is expensive.

The observation: a run is a `map` over chunks followed by a `reduce` in the aggregation stage
(IR §8.3), with a small immutable shared context. That is exactly a distributable shape, and the
engine's structure already names every piece.

```rust
pub trait ChunkExecutor: Send + Sync {
    fn execute(&self, ctx: &RunContext, chunk: ChunkRef) -> Result<ChunkResult, RunError>;
}
pub struct LocalExecutor  { pool: rayon::ThreadPool }         // 0.1
pub struct RemoteExecutor { .. }                               // later: gRPC to workers
```

- `RunContext` = `Arc<Plan>` + `Arc<TableSet>` + `AssumptionValues` + timeline. It is **serialisable**
  (the `Plan` is derived from the IR, so shipping the IR + config to a worker is sufficient and is
  ~100 KB) and content-addressed by `plan_digest`, so workers cache compiled plans by digest.
- `ChunkRef` = `(source_uri, byte_or_row_range, chunk_index)`. A worker fetches its own rows; the
  coordinator ships coordinates, not data.
- `ChunkResult` = Arrow IPC bytes + `TrapReport` + `chunk_index`. Ordering is restored by
  `chunk_index` at the writer, so out-of-order completion — the norm in a distributed run — still
  yields byte-identical output.
- Aggregation (IR §8.3) is expressed as a monoid per `(group_by, measure, op)`; `sum`, `count`,
  `min`, `max` combine associatively, and for `sum` the coordinator re-reduces **in chunk-index
  order** to preserve §7.2. `npv`-style order-sensitive reductions are per-modelpoint and never cross
  chunks, so they are unaffected.

Hosted mode adds nothing to the kernel: it is a different `ChunkExecutor` and a different
`ModelpointSource`. A stretch consequence worth naming: the same seam supports a JIT backend later —
a `CraneliftExecutor` that compiles the tape to native code per chunk, validated against the
interpreter by the golden corpus. The interpreter stays as the reference semantics forever.

---

## 11. Benchmark plan

The benchmark is a **demo artefact**, not an internal metric. It ships as a repo
(`predictable-benchmarks`), runs in CI on a fixed runner, publishes a table and the reproduction
commands, and — this is the credibility bit — **verifies that all three engines produce the same
numbers** before reporting any timing.

### 11.1 Contenders

| engine | version | notes |
|---|---|---|
| predictable | current | interpreter, `C` auto, `threads=1` and `threads=N` rows |
| cashflower | latest | pure-Python cashflow modelling framework, closest philosophical peer |
| lifelib / modelx | latest | the reference open-source actuarial modelling stack |
| *(reference)* | — | a hand-written NumPy implementation, as the honest "how fast could you do this yourself" floor |

Prophet is not benchmarked: no license to publish numbers, and it would be a distraction. The Prophet
comparison is the *migration* demo, which is about correctness and time-to-model, not throughput.

### 11.2 Scenarios

Each exists in all four implementations, in the same repo, reviewable side by side.

1. **`term_annual`** — the IR spec §6 term-assurance model. 40 annual periods, 17 components. The
   "hello world" and the headline number.
2. **`term_monthly`** — the same product on a monthly basis, 480 periods. Tests the `t`-loop
   constant.
3. **`savings_monthly`** — endowment with account value, surrender values, guarantees, a `Select`-
   heavy structure and 3 table lookups per period. ~60 components. Tests branch-free `Select` and
   the table path.
4. **`ifrs17_gmm`** — the IFRS 17 general measurement model on scenario 3's product: BEL, risk
   adjustment, CSM roll-forward, coverage units, LIC/LRC split. ~180 components, two substage levels
   (§5.3). This is the realistic-workload number.
5. **`term_solve`** — scenario 1 with a `[[solve]]` block solving the premium to a target margin.
   Tests the outer loop and shows solver cost honestly.

### 11.3 Sizes

`M ∈ {1, 1_000, 100_000, 1_000_000, 10_000_000}`. The `M = 1` column is deliberate: it exposes
per-run overhead (parse, plan, table build) that a big-`M` benchmark hides, and it is the size a
developer iterating on a model actually feels. `M = 10_000_000` runs only for scenarios 1 and 4, and
only on the large CI runner.

### 11.4 Metrics

- **Wall time**, cold and warm, median of 5, with min/max reported. Not just the mean.
- **modelpoint-periods per second** (`M × (T+1) / s`) — the size-independent number, and the one
  quoted in the README.
- **Peak RSS**, sampled at 10 ms.
- **Scaling curve**: threads ∈ {1, 2, 4, 8, 16} with parallel efficiency, on scenario 4 at `M = 1e6`.
- **Time-to-first-result** for `M = 1` (the iteration-loop metric).
- **Correctness gate**: every scenario × size asserts predictable ≡ cashflower ≡ lifelib to
  1e-9 relative on every output, before any timing is published. Where they disagree, the benchmark
  fails and the discrepancy is written up — a disagreement is more interesting than a speedup.

### 11.5 Targets for 0.1 (stated up front so a miss is visible)

| scenario | size | target, 1 core | target, 8 cores |
|---|---|---|---|
| `term_annual` | 1e6 | ≥ 300 M mp-periods/s | ≥ 2 G |
| `ifrs17_gmm` | 1e6 | ≥ 20 M mp-periods/s | ≥ 140 M |
| `term_annual` | 1e6 | ≥ 20× cashflower | — |
| `ifrs17_gmm` | 1e6 | ≥ 5× lifelib, ≥ 0.5× the NumPy reference | — |
| any | 1 | < 50 ms end-to-end including parse and plan | — |

Peak RSS must stay under 2 GB at `M = 1e7` with 8 threads and `retain = "auto"` — the streaming
claim of §4.3 made falsifiable.

Micro-benchmarks (`criterion`) cover the tape interpreter per op-class, table lookup per policy,
Parquet load throughput, and plan construction. They exist to catch regressions, and CI fails on a
> 5% regression, but they are not published numbers.

---

## 12. IR feedback

Gaps found while designing the engine against `01-ir.md`. None of these are divergences — the design
above conforms to the spec as written — but each is something the IR should settle.

**Settled in IR 1.0** (raised here, now normative in `01-ir.md`):

- Chained stage-2 → stage-1 `init` references are legal; `G_init` must be acyclic, `E0202` (IR §3.1).
  The engine's substage levels (§5.3) are the sanctioned implementation.
- `count_while` **stops at the first false** (IR §2.8).
- `at(x, k)` and `x[k]` are the same operation; `at` is the call spelling (IR §2.8).
- Traps in a provably-untaken `If` arm are **suppressed**, mask derived syntactically — the engine's
  static masking (§3.3) is now the specified semantics, not a liberty (IR §2.6).
- `round(x, dp)` is round-half-away-from-zero on the shortest decimal representation (IR §2.8).
- `stage` is an explicit computed field on every component (IR §2.2).

**Also settled (IR decision log Q1–Q15, `01-ir.md` §13):**

1. **`RunConfig`** (Q1) is now a `[run]` block in its own `.pir` file (IR §8.4.2), carrying `product`,
   `assumptions`, `modelpoints`, `emit`/`emit_list`, `retain`, `on_trap`, `max_errors`,
   `allow_table_drift`, `sum_kahan`, `[run.tables]` overrides, `[[solve]]` and `[[aggregation]]`
   blocks, plus a non-semantic `[run.exec]` (threads, chunk size, progress). `[timeline].periods` is
   the **only** source of `T` and a run may not override any timeline field (`E0101`). `run_digest`
   covers everything that can change a number and excludes `[run.exec]` and `out` — the engine's
   proposal, adopted as written.
2. **`npv` result timing** (Q2 of this list) is a lint: `Agg` results are untimed `PerMP`, and
   `retime`/`shift`/`cum`/`diff` on an untimed value is `W0105` (IR §2.5).
3. **Nulls** (Q3): the engine's assumption is now normative. No runtime nulls, no validity bitmap;
   missingness is eliminated at load and at lookup, and `is_null`/`coalesce` read an opt-in presence
   lane materialised only where the model calls them (IR §2.11).
4. **`f32` storage** (Q4 of this list) is deferred to IR 1.1 with the key reserved now:
   `run.storage_precision` accepts `"f64"` only, `"f32"` is `E0108` (IR §8.4.5).
5. **`TableResolver`** (Q5): `source` resolves relative to the declaring `.pir` file, sandboxed to the
   project root (`E0801`); `FsResolver` / `MapResolver` (`resource:`, used by WASM and hosted runs) /
   `InlineResolver` (`source = "inline"`); the digest, not the path, is the table's identity
   (IR §2.9.1).
6. **Cancellation** has a manifest home: `manifest.execution.outcome ∈ completed |
   completed_with_traps | aborted | cancelled`, alongside the trap policy of IR §9.3.1 — a truncated
   result set can no longer look complete.

**Still open:** nothing. All questions raised by this layer are closed in `01-ir.md` §13.
