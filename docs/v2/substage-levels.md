# Substage levels and aggregate terms

Two features of [the kernel](kernel.md) exist for the same reason: an actuarial model does not run
strictly forwards, and an actuary reconciling a number needs to see *how it was built*, not just
what it came to.

- **Substage levelling** implements the one legal backward edge in the IR — a stage-1 `init` reading
  a stage-2 aggregate ([`01-ir.md` §8.2](../design/01-ir.md), [`03-engine.md`
  §5.3](../design/03-engine.md)). `reserve` seeding itself from `bel`, a present value over the very
  projection `reserve` takes part in, is the canonical case.
- **Per-`t` term retention** makes every aggregate explainable: the periods, values, discount
  factors and contributions behind one number, replayed on demand
  ([`01-ir.md` §11.2, decision Q11](../design/01-ir.md)).

Both are engine-side, both are exact, and neither costs an ordinary run anything.

---

## 1. The back-channel

The evaluation model is two stages: the `t` loop, then the aggregates that reduce completed series
to one value per modelpoint. Stage 2 reads stage 1. The IR allows exactly one edge going the other
way, and only through `init`:

```toml
[[component]]
name = "bel"
kind = "Derived"
shape = "PerMP"
unit = "money"
expr = "npv(claims, disc)"          # stage 2: reduces the whole projection

[[component]]
name = "reserve"
kind = "Output"
shape = "Series"
unit = "money"
timing = "start"
init = "bel"                        # ...and stage 1 seeds itself from it
expr = "reserve[t-1] * 1.05 - claims"
```

`expr` may **not** read a stage-2 value — that would be a genuine cycle, and the checker says so
(`E0202`). `init` may, and `G_init` is checked for acyclicity separately. Anything that needs real
iteration to a fixed point is a `[[solve]]` block wrapped around the whole projection, not a
construct inside the model.

## 2. How the engine runs it

Not with a second pass over the data, and not by guessing. The engine partitions the work into
**levels** and runs the `t` loop once per level, over that level's slots only:

```text
level 0   t = 0 … T   disc, survivors, claims        (no stage-2 seed)
          stage 2     bel = npv(claims, disc)
level 1   t = 0 … T   reserve                        (seeded from bel)
          stage 2     pv_reserve = sum(reserve)
```

The levels **partition** the slots: every component is evaluated exactly once, so total op count is
unchanged. A level costs one extra traversal of the `t` axis, never extra memory — the arena is
sized for the union of all levels.

```rust
use predictable_engine::Levels;

let levels = Levels::analyze(&plan, &tapes);
assert_eq!(levels.count(), 2);
assert!(!levels.is_flat());
println!("{}", levels.text(&plan));
// level 0: stage1 [ref.disc, ref.survivors, ref.claims] stage2 [ref.bel]
// level 1: stage1 [ref.reserve] stage2 [ref.pv_reserve]
```

A model with no stage-2 `init` has exactly one level, `is_flat()` is true, and `run_chunk` takes its
original single-pass path with no per-slot dispatch at all. Levelling taxes only the models that
use it.

The schedule is derived from the **lowered tapes**, not re-derived from the expressions: the tape is
what actually runs, so it is what the schedule must agree with. Three rules fix every level:

1. A stage-1 slot whose `init` reads stage-2 values sits one level above the highest of them.
2. A stage-2 slot sits at the level of the highest series it reduces.
3. Anything reading a slot at level `L` is itself at level `L` or above — including lagged reads,
   because a lag still needs the value to exist.

### Retention repairs itself

A level-1 component reading a level-0 series reads it *after* level 0's loop has finished. If the
planner gave that series a ring buffer of two periods, its history would be long gone. Retention is
a layout decision, so the engine repairs it where the decision is made:

```rust
let cross = levels.cross_level_reads();      // slots read from a level above
let layout = Layout::with_full(&plan, chunk, n_regs, n_accs, cross);
```

Slots that are *not* read across levels keep the planner's ring — levelling is not an excuse to
retain everything.

### `W0110`: more than three levels

Each level is a full re-traversal of the projection. Two is the reference IFRS 17 shape; three is
generous. Above that the engine lints:

```rust
for lint in engine.level_lints() {
    eprintln!("{} {}", lint.code, lint.message);
}
// W0110 this model needs 5 stage-2 substage levels; each level is a full
//       re-traversal of the projection
```

The lint carries the usual notes and doc URL: consider whether every component with a stage-2 `init`
really needs a prospective seed, or whether one of them can be a plain constant.

---

## 3. Aggregate terms

Reductions are **strictly sequential in `t`** ([`01-ir.md` §9.2](../design/01-ir.md)): a plain
left-to-right fold, `#[inline(never)]`, never vectorised, because a pairwise or SIMD sum
reassociates additions and moves the last bits of a number an actuary is reconciling to the penny.
The compensated variant is a separately named builtin the model has to ask for:

```toml
expr = "sum(flow)"          # left to right, exactly
expr = "sum_kahan(flow)"    # Neumaier compensation, still strictly in t order
```

That guarantee is what makes the trace meaningful. `Engine::agg_trace` replays a reduction over the
buffers a completed chunk left behind and hands back every term:

```rust
let out = engine.run_chunk(&mut bufs, &chunk)?;
let trace = engine.agg_trace_default(&bufs, "pv_premiums", /* lane */ 0).unwrap();

assert_eq!(trace.value, out.column("pv_premiums").unwrap().lane(0)[0]);  // bit for bit
assert!(trace.sums_exactly());
```

Serialised, that is the `Agg` node of an `explain()` trace verbatim:

```json
{
  "node": "Agg", "op": "npv", "ref": "t.premium_income", "timing_used": "start",
  "value": 1883.36, "term_count": 4,
  "terms": [
    {"t": 0, "value": 500.0, "disc": 1.0,      "contribution": 500.0},
    {"t": 1, "value": 500.0, "disc": 0.96,     "contribution": 480.0},
    {"t": 2, "value": 500.0, "disc": 0.9216,   "contribution": 460.8},
    {"t": 3, "value": 500.0, "disc": 0.884736, "contribution": 442.368}
  ]
}
```

Four properties are contractual:

- **`contribution` re-adds to `value` exactly.** The recorder walks `t = 0..=T` in the kernel's own
  order with the kernel's own additions; it does not re-add the terms afterwards.
- **`disc` is the factor *after* the timing exponent.** `timing = "end"` discounts a period further
  than `timing = "start"`, and that is exactly the bug that hides in a 3%-wrong present value — so
  it is visible as data, with `timing_used` naming the timing that chose it.
- **Predicates are visible.** `sum(x, cond)` marks each term `included`, and `count_while` marks the
  first false period `stopped_here` — it stops there rather than counting every true period.
- **Truncation is loud.** `agg_trace(&bufs, name, lane, max)` keeps the first and last `max/2`
  terms, sets `terms_truncated`, and still reports the untruncated `term_count`. The default is
  4096.

### Replay-only

Nothing in the recorder is reachable from the hot loop. `Op::Reduce` and `Op::Npv` are untouched; a
trace is produced *after* a chunk has run, from the arena it left behind, and only when someone asks
for one. A run that never calls `agg_trace` records nothing and pays nothing — which is the whole
point of decision Q11's "replay-only, zero hot-path cost", and the same technique the trap reporter
uses in [§5.5](kernel.md). It is exact for the same reason: the projection is deterministic, so a
replay reproduces the traced value rather than approximating it.

---

## 4. Worked example

```rust
use predictable_engine::{Engine, Levels, RunConfig};

// A model where `reserve` is seeded from `bel = npv(claims, disc)`.
let levels = Levels::analyze(&plan, &tapes);
assert_eq!(levels.count(), 2);

let mut engine = Engine::new(&plan, &tapes, vec![], &timeline, RunConfig::default())?;
let mut bufs = engine.buffers();
engine.prepare(&assumptions, &mut bufs)?;
let out = engine.run_chunk(&mut bufs, &chunk)?;

// reserve[0] *is* the stage-2 value: the back-channel worked.
let bel = engine.agg_trace_default(&bufs, "bel", 0).unwrap();
assert_eq!(out.column("reserve").unwrap().lane(0)[0], bel.value);

// And every period behind it is inspectable.
for term in &bel.terms {
    println!("t={} claims={} disc={} -> {}",
             term.t, term.value, term.disc.unwrap(), term.contribution);
}
```

Both features are covered by behavioural tests in `crates/predictable-engine/tests/levels.rs` and
`tests/terms.rs`: the levelled run is checked against a hand-written scalar reference, chunk
invariance is re-asserted under levelling, the ring-widening is checked to change a value that would
otherwise be wrong, and the term recorder is checked against the kernel's own output on adversarial
magnitudes where any reassociation would show.
