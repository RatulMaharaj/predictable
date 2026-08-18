# Planning a model

`predictable-plan` is the compiler in the middle of the toolchain. The checker decides whether a
model is *well formed*; the planner decides how it will be *evaluated* — what the runtime can name,
in what order, how much of each series it keeps in memory, what work can leave the per-modelpoint
loop, and what can simply be computed now. It implements
[`03-engine.md` §3 and §4.3](../design/03-engine.md).

A plan is a pure function of a checked model and a handful of options. No IO, no threads, no clock,
no allocator dependence: plan the same model twice, on two machines, and the two plans compare equal
field for field and digest for digest. Everything downstream — the tape lowering, the kernel, the
graph explorer — reads the plan and is forbidden from re-deriving any of it.

---

## 1. The five decisions

| Decision | What the planner does | Spec |
|---|---|---|
| **Slots** | Every value the runtime can name gets a dense `u32`, partitioned by shape into `scalars` / `permp` / `series` | §3.1 |
| **Order** | Kahn's algorithm with the ready set as a min-heap on `(module_path, declaration_index)`, hashed into `order_digest` | §3.2 |
| **Retention** | `Ring(max_lag + 1)` by default; `Full` when the slot is an output, is reduced, or is the target of an `At` | §4.3 |
| **Hoisting** | A `Series` whose whole dependency closure is modelpoint-independent is computed **once per run**, not once per modelpoint | §3.4 |
| **Folding** | Constant folding, restricted to rewrites that are exactly IEEE-preserving | §3.5 |

Only the last two are optional. `--O0` turns both off — and nothing else, because slots, order and
retention are semantics rather than optimisation.

---

## 2. Using it

```rust
use predictable_check::Input;
use predictable_plan::{plan_sources, OptLevel, PlanOptions};

let plan = plan_sources(
    &[Input::new("model.pir", model_text)],
    &PlanOptions {
        opt: OptLevel::O1,
        retain_all: false,
        program_digest: model_digest,   // from `predictable-fmt`
        periods: None,                  // `[timeline].periods` is the normative source
    },
)?;

println!("{} slots, order {}", plan.slot_refs.len(), plan.order_digest);
```

`plan_sources` checks first and refuses to plan a model that does not check:

```rust
match plan_sources(&inputs, &options) {
    Err(predictable_plan::PlanError::NotChecked(diagnostics)) => { /* the checker's product */ }
    Err(predictable_plan::PlanError::Order(cycle)) => { /* unreachable after a clean check */ }
    Ok(plan) => { /* … */ }
}
```

There is deliberately no path that half-plans a broken model. Diagnostics are the checker's product,
and a planner that produced its own would be a second, divergent opinion on the same file.

If you already hold `predictable_ir::Module` values — from `pir.json`, or from the DSL's build step —
call `plan(&modules, &options)` instead and skip the parse.

---

## 3. A worked example

The model below is the fixture the planner's own tests run on. It has one of everything that makes
planning interesting: a scalar derivation, a recursive series with an `init`, an absolute `At`
reference, two aggregates and a piece of `PerMP` arithmetic downstream of one.

```toml
[timeline]
basis = "annual"
periods = 12
origin = "policy"
valuation_date = 2026-06-30

# inputs
modelpoint_field  sum_assured : f64 money
modelpoint_field  entry_age   : i64 years
assumption        valuation_rate    : f64 Scalar rate(annual)
assumption        mortality_loading : f64 Scalar factor

# components
annual_rate     Scalar  = valuation_rate * (2.0 + 3.0)
discount_factor Series  = if t == 0 then 1.0 else discount_factor[t-1] / (1.0 + annual_rate)
q_x             Series  = 0.001 * (1.0 + 0.05 * (entry_age + t)) * mortality_loading
num_pols_if     Series  = num_pols_if[t-1] * (1.0 - q_x[t-1])        init = 1.0
claims          Output  = num_pols_if * q_x * sum_assured             (Series)
reserve_seed    Output  = q_x[0] * (10.0 - 4.0)                       (Series)
pv_claims       Output  = npv(claims, discount_factor)                (PerMP)
total_claims    Output  = sum(claims)                                 (PerMP)
margin          Output  = pv_claims * (1.0 + 0.0)                     (PerMP)
```

*(The `.pir` file is `crates/predictable-plan/tests/model.pir`; the shorthand above is for reading.)*

### 3.1 The order it produces

```
prologue   sum_assured  entry_age  valuation_rate  mortality_loading  annual_rate
hoisted    t  period_start_date  period_end_date  year_frac  month_of_year
           policy_year  policy_month  is_anniversary  discount_factor
stage 1    q_x  num_pols_if  claims  reserve_seed
stage 2    pv_claims  total_claims  margin
```

Four things in that listing are decisions rather than accidents:

**The timeline fields are slots.** `t`, `policy_year` and the rest of `01-ir.md` §5 are `Series`
slots in a module path of `<timeline>`. `<` sorts below every ASCII letter, so the timeline is always
at the head of the min-heap, and `t` is a stable head of the order in every model.

**`discount_factor` is in `hoisted`, not `stage 1`.** Its closure is `t`, itself at lag 1, and a
scalar — no modelpoint field anywhere in it. So it is computed once per run into a shared `(T+1)`
array and read by every chunk, instead of being recomputed identically for all five million policies.
`q_x` reads `entry_age` and stays in the loop; `num_pols_if` reads `q_x` and stays with it.

**`num_pols_if` appears once, despite reading itself.** `x[t-1]` is a self-loop with `lag = 1`, which
is legal (`01-ir.md` §3.1) and — crucially — is *not* an ordering constraint. Last period is already
computed; it is just data.

**`margin` is in stage 2 although it contains no aggregate.** `01-ir.md` §2.2 computes a component's
`stage` from its own expression, and that field is recorded verbatim on the slot. But a `PerMP`
component that *reads* a stage-2 value cannot run before the loop either, so tape membership is the
transitive closure of the IR's stage — §3.1's "stage 2: `Agg` reductions **and `PerMP` arithmetic**".

### 3.2 The retention it produces

| slot | retention | why |
|---|---|---|
| `num_pols_if` | `Ring { len: 2 }` | read at lag 1 and nothing more |
| `period_start_date` | `Ring { len: 1 }` | never lagged |
| `claims` | `Full` | `kind = "Output"` |
| `discount_factor` | `Full` | the second argument of an `npv` — a reduce target |
| `q_x` | `Full` | `reserve_seed` reads `q_x[0]`: an `At` target |

Ring lengths are rounded up to a power of two so indexing is `t & (len - 1)` — no modulo, no branch.
Each `Full` decision carries a `full_reason`, so the CLI can explain a memory projection rather than
just quote a number:

```rust
let bytes = plan.series_bytes_per_chunk(1024);
for slot in &plan.series {
    if let Some(reason) = slot.full_reason {
        println!("{} retained whole: {reason:?}", slot.info.name);
    }
}
```

`PlanOptions::retain_all` forces every series `Full` — needed for whole-run drill-down in the UI —
and `series_bytes_per_chunk` is what the CLI prints before allocating.

---

## 4. What folding will and will not do

The interesting half of `03-engine.md` §3.5 is the refusals. **Optimisation may change speed, never
bits.**

| rewrite | folded? | why |
|---|---|---|
| `2.0 * 3.0` → `6.0` | yes | one IEEE multiply, done now instead of later |
| `valuation_rate * (2.0 + 3.0)` → `valuation_rate * 5.0` | yes | the constant subtree only; an assumption is not a constant at plan time |
| `if true then a else b` → `a` | yes | `If` is a value conditional over pure arms |
| `x * 1.0` → `x` | **no** | changes NaN payloads, and the multiply is not free of meaning |
| `x + 0.0` → `x` | **no** | `-0.0 + 0.0` is `+0.0`, not `-0.0` |
| `x - x` → `0.0` | **no** | `inf - inf` is NaN |
| `1.0 / 0.0` | **no** | division traps at run time with a span and a lane; folding moves the trap somewhere useless |
| `round(2.675, 2)` | **no** | `01-ir.md` §2.8 pins round-half-away-from-zero on the shortest decimal — one implementation, in the kernel |
| `1.05 ^ 3.0` | **no** | `pow` is libm's, and §7 vendors exactly one libm |
| `2 * 3.0` | **no** | mixed dtypes; the conversion is the checker's business, not the folder's |

This is not a promise made in prose. Every planner pass that is optional is covered by a
**differential test**: the same model planned at `--O0` and at `--O1`, both evaluated by the same
reference interpreter, results compared with `f64::to_bits` rather than `==` — because `==` says
`0.0 == -0.0` and says nothing at all about NaN, and those are precisely the cases an unsound
identity produces. The test runs over signed zeros, subnormals and `1e300` modelpoints, and it also
asserts that `--O1` actually folded something, so it cannot pass vacuously.

---

## 5. Digests

Two digests come out of a plan, and they answer different questions.

**`order_digest`** — `sha256` of the slot ids in evaluation order, one tape at a time, with the tape
name mixed in. A change in topological order is a diffable, reviewable event rather than an accident
of a `HashMap`: it shows up as a one-line change in a pull request. Moving a slot *between* tapes
changes it too, even though the concatenated id sequence would not, because where the work happens is
a real semantic change.

**`digest`** — `sha256(program_digest ‖ run_config ‖ order_digest ‖ engine_major)`. This is the plan's
identity in a manifest: it moves when the model moves, when the plan-affecting run options move, and
when the engine's major version moves.

```rust
assert_eq!(plan_a.order_digest, plan_b.order_digest);  // same model, same order, anywhere
assert_ne!(o1.order_digest, o0.order_digest);          // --O0 moved work between tapes
```

Determinism here is structural, not incidental. Every map in the crate is a `BTreeMap` or a
`BTreeSet`; `std::collections::HashMap` does not appear, so even a debug print is deterministic. The
ready-set heap compares interned `u32` module ids whose numeric order equals the string order of the
paths, so it orders by `(module_path, declaration_index)` exactly as the spec words it, without
comparing strings in the hot loop.

---

## 6. Reading a plan

```rust
plan.periods                       // T; the projection runs t = 0..=T
plan.order()                       // prologue → hoisted → stage1 → stage2
plan.order_names()                 // the same, by name — what the tests assert on
plan.info(slot_id)                 // name, module path, decl index, kind, dtype, unit, stage
plan.series_named("claims")        // retention, max_lag, timing, init, hoistable
plan.outputs                       // the emission set, in declaration order
plan.series_bytes_per_chunk(1024)  // projected footprint
```

`Plan` is `serde`-serialisable in full, which is what lets the viz server (`05-viz.md` §2.1) read the
ordering from the planner rather than recomputing it.

---

## 7. Scope

The planner produces the *order* and the *storage decisions*. It does not yet produce the tapes of
`03-engine.md` §3.3 — the `Op` set, register allocation, `Select` masking and three-tape loop peeling
are the tape-lowering task, and they consume the four `Vec<SlotId>` orderings here. Two smaller
deferrals go with them:

- **Common-subexpression elimination** is specified as within-a-single-tape only, so it belongs to the
  tape rather than to the expression tree, and lands with lowering.
- **`init` scheduling.** An `init` is its own tape range and may read stage-2 values — the one legal
  backward channel (`01-ir.md` §8.2). The planner records each `init` on its slot but does not order
  it into the stage-1 body, because that ordering is the substage levelling of §5.3.
