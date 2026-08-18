# Tape lowering

`predictable-tape` sits between the planner and the kernel. The planner
([Planning a model](planning.md)) decides *what* is computed and *in what order*; this crate decides
*how* — it turns each component's expression into a flat, branch-free, register-machine **tape** that
the kernel runs with a single forward pass over a `Vec`.

It implements [`03-engine.md` §3.3 and §5.2](../design/03-engine.md) and the `If` semantics of
[`01-ir.md` §2.6](../design/01-ir.md).

Four properties are the whole point:

| Property | Why it matters |
|---|---|
| **No tree walk** | evaluation is `for op in tape.ops`, so there is no recursion and no stack-depth limit — which is what makes the wasm build possible |
| **No control flow** | `If` lowers to `Select` over two already-evaluated arms, so the 1024-wide lane loop never diverges and LLVM auto-vectorises it |
| **No run-time time checks** | "is `t - k` below the origin?" is answered at lowering by peeling the loop, not at run time by branching |
| **No dynamic trap state** | a division inside an untaken `If` arm is suppressed by a *statically derived* mask register, not by a predicate stack |

---

## 1. A worked example

Take the reference term model: a survivorship recursion, a discount recursion that depends only on a
scalar assumption, a cashflow, and a present value.

```toml
[[assumption]]
name = "valuation_rate"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"

[[component]]
name = "disc"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "point"
init = "1.0"
expr = "disc[t-1] / (1 + valuation_rate)"

[[component]]
name = "num_pols"
kind  = "Derived"
shape = "Series"
timing = "start"
init  = "1.0"
expr  = "num_pols[t-1] * (1 - qx)"

[[component]]
name = "claims"
kind = "Output"
shape = "Series"
timing = "end"
expr = "num_pols * qx * sum_assured"

[[component]]
name = "pv_claims"
kind = "Output"
shape = "PerMP"
expr = "npv(claims, disc)"
```

Lower it:

```console
$ cargo run -p predictable-tape --example dump_tape -- term.pir
```

```text
# tape_digest ead246be93cdb1633f434d008e706649d9ef20a33270acfd02afed024e2a8041
# peel 1 | ops 34
# prologue (n_regs=0)
# hoisted.t0 (n_regs=1)
    constf #0 = 1.0
  0 | r0 = constf #0
  1 | store.seed s11 <- r0
  2 | r0 = load.seed s11
  3 | store.hoisted s11 <- r0
# hoisted.body (n_regs=3)
    constf #0 = 1.0
  0 | r0 = load.hoisted.lag s11 1
  1 | r1 = constf #0
  2 | r2 = load.scalar s10
  3 | r1 = add r1, r2
  4 | r0 = div r0, r1 @0
  5 | store.hoisted s11 <- r0
# stage1.t0 (n_regs=2)
    constf #0 = 1.0
  0 | r0 = constf #0
  1 | store.seed s12 <- r0
  2 | r0 = load.seed s12
  3 | store.cur s12 <- r0
  4 | r0 = load.cur s12
  5 | r1 = load.permp s9
  6 | r0 = mul r0, r1
  7 | r1 = load.permp s8
  8 | r0 = mul r0, r1
  9 | store.cur s13 <- r0
# stage1.body (n_regs=3)
    constf #0 = 1.0
  0 | r0 = load.lag s12 1
  1 | r1 = constf #0
  2 | r2 = load.permp s9
  3 | r1 = sub r1, r2
  4 | r0 = mul r0, r1
  5 | store.cur s12 <- r0
  6 | r0 = load.cur s12
  7 | r1 = load.permp s9
  8 | r0 = mul r0, r1
  9 | r1 = load.permp s8
 10 | r0 = mul r0, r1
 11 | store.cur s13 <- r0
# stage2 (n_regs=1)
  0 | r0 = npv s13, s11 timing end
  1 | store.permp s14 <- r0
```

Read across that output and every decision in this crate is visible:

- `disc` is **hoisted**: it depends only on a scalar, so it lives in its own tape, is computed once
  per run into a shared `(T+1)` array, and is read from stage 1 as `load.hoisted.*` — a stride-0
  broadcast rather than a per-modelpoint load.
- Both recursions have an `init`, so the `t = 0` tape has a **seed section** (`store.seed`) before the
  body. At `t = 0`, `num_pols` *is* its seed; the formula is never evaluated there.
- The `body` tape reads `load.lag s12 1` unconditionally: at `t >= peel` no lag can be pre-origin, so
  there is nothing to test.
- The whole projection runs in **three registers**. Register numbering is a function of the tape
  alone, which is what makes `tape_digest` reproducible.
- `npv` is one stage-2 op over two whole retained series, strictly sequential in `t`.

---

## 2. `Select` and static trap masking

`If` is a value conditional: `01-ir.md` §2.6 requires both arms to be evaluated. That is what keeps
the lane loop branch-free — but it means the most common defensive idiom in actuarial code would
divide by zero on every lane:

```toml
expr = "if exposure == 0.0 then 0.0 else premium / exposure"
```

The ruling (Q7) is that a trap raised inside an arm is **suppressed when that arm is not taken**, and
the mask is derived *syntactically* from the enclosing chain of `If` conditions. Lowering
materialises it:

```text
# stage1.body (n_regs=5)
    constf #0 = 0.0
  0 | r0 = load.permp s9          ; exposure
  1 | r1 = constf #0
  2 | r2 = cmp r0 == r1           ; the condition
  3 | r3 = not r2                 ; the else arm's mask
  4 | r4 = load.permp s8          ; premium
  5 | r0 = div r4, r0 mask r3 @0  ; traps only where r3 is true
  6 | r0 = select r2 ? r1 : r0
  7 | store.cur s10 <- r0
```

Three things to note:

- The mask is a register, but *which ops carry a mask and what it is made of* is fixed at lowering.
  There is no predicate stack, no data-dependent control flow, and no way for a mask to depend on
  anything the kernel discovers at run time.
- Masks are only materialised for arms that **can** trap. `if a then b else c` over plain arithmetic
  emits no `not` and no `and` at all — you pay for the mask exactly when you need it.
- Nested `If`s conjoin: an arm two levels deep carries `and(outer, inner)`.

`@0` on the `div` is its **site**: the slot plus the `ExprPath` of `01-ir.md` §3.0.1 (`expr.else`,
`expr.rhs.arg1`, …). That is what a trap is reported against, and what `explain()` uses to point at
the sub-expression the author actually wrote. Sites are stable across reformatting, which spans are
not.

---

## 3. Three tapes, one loop

`t = 0` is the only period where `init` applies, and `t < max_lag_global` is the only range where a
lag can fall below the origin. Rather than branch inside the hot loop, the loop is peeled:

```text
t0      : init seeds, then the t = 0 body   — every lag is pre-origin
prefix  : one tape per t in 1 .. peel       — lags with k > t are pre-origin
body    : t >= peel                         — every lag is in range
```

With the usual `max_lag_global = 1` the prefix is empty and this is exactly the spec's "three tapes".
A model with a deeper lag gets one prefix tape per peeled period, because a single prefix tape could
not resolve `x[t-3]` statically for both `t = 1` and `t = 2`:

```rust
let prog = lower_plan(&plan)?;
assert_eq!(prog.peel, 3);                 // max_lag_global
assert_eq!(prog.stage1.prefix.len(), 2);  // t = 1 and t = 2
prog.stage1.at(1);                        // the tape that runs at t = 1
prog.stage1.at(9);                        // ... and the body, for everything from t = 3
```

Pre-origin reads resolve two ways (`01-ir.md` §2.7):

| The series | `x[t-k]` for `t - k < 0` lowers to |
|---|---|
| declares an `init` | `load.seed` — the seed evaluated at the top of the `t0` tape |
| declares none | the zero of its dtype, as a constant |

Either way the site is tagged `pre_origin_default`, so `explain()` can say where the number came
from rather than showing an unexplained `0.0`. A `date`/`str`/`enum` series with no `init` has no
zero and is refused (`LowerError::NoPreOriginValue`) rather than defaulted.

---

## 4. Registers

Lowering hands out one virtual register per value — SSA, so a value's identity never changes — and
then renumbers onto a small physical file by linear scan. The tape is straight-line, so live ranges
are plain intervals and the scan is exact rather than approximate.

Two rules do most of the work:

- **Uses are freed before the definition is allocated**, so `r0 = mul r0, r1` is reachable. Every op
  is elementwise over lanes and reads a lane before writing it, so reusing the register is safe. This
  is why the term model above runs in three registers rather than fifteen.
- **The free list is a min-heap.** The lowest free register is always taken, so the numbering is a
  function of the tape alone — same plan, same registers, on any machine.

There is no spilling. A tape needing more than 65,536 simultaneously live values is rejected
(`LowerError::TooManyRegisters`) rather than quietly generating memory traffic; the kernel allocates
`C × n_regs` scratch floats per worker, so the register count is a memory decision as much as a
speed one.

Common subexpressions are shared **within one component only**. Across components they are not, even
when identical: `explain()` must be able to show each component's own arithmetic (§3.5). Two
divisions are never merged either, however identical — merging them would merge their trap sites, and
a trap must name the expression the model author wrote.

---

## 5. The op set

```rust
pub enum Op {
    ConstF(Reg, ConstId), ConstI(..), ConstS(..),
    LoadScalar(Reg, SlotId), LoadPerMp(..),
    LoadCur(Reg, SlotId), LoadLag(Reg, SlotId, u32), LoadAt(Reg, SlotId, u32),
    LoadSeed(Reg, SlotId),                       // pre-origin: the init seed
    LoadHoistedCur(..), LoadHoistedLag(..), LoadHoistedAt(..),   // stride-0 broadcast
    LoadTime(Reg, TimeField),                    // t, policy_year, year_frac, ...
    Add(..), Sub(..), Mul(..), Neg(..),
    Div { dst, lhs, rhs, mask: Option<Reg>, site },   // masked = the spec's DivMasked
    Pow { .. },
    Cmp(Reg, CmpOp, Reg, Reg), And(..), Or(..), Not(..),
    Select(Reg, Reg, Reg, Reg),                  // if — both arms already evaluated
    Call1(Reg, Fn1, Reg), Call2(..), CallN(Reg, FnN, Vec<Reg>),
    Lookup { dst, table, keys, mask, site },
    Cum(Reg, SlotId, AccId),                     // cum(): a running total across t
    StoreCur(SlotId, Reg), StoreSeed(..), StoreHoisted(..), StorePerMp(..), StoreScalar(..),
    Reduce { dst, agg, series, pred },           // stage 2 only
    Npv { dst, value, disc, timing },            // stage 2 only
}
```

The timing builtins of `01-ir.md` §2.5 get no ops of their own — they are re-expressed in terms of
the loads they are sugar for, which keeps the kernel's op table small and makes them bit-identical to
the hand-written form:

| Builtin | Lowers to |
|---|---|
| `shift(x, k)` | `LoadLag(x, k)` — byte-for-byte the same tape as `x[t-k]` |
| `retime(x, timing)` | its own argument; timing is a tag that changes what `npv` does, never the number in the lane |
| `diff(x)` | `Sub(LoadCur(x), LoadLag(x, 1))` |
| `cum(x)` | `Cum` with a tape accumulator the kernel carries across `t` |

`Reduce` and `Npv` read a **whole retained series** and so take a `SlotId`, not a register. An
aggregate over a computed operand — `sum(a * b)` — is refused
(`LowerError::AggArgumentNotASeries`) rather than guessed at: materialising the temporary series is a
planner decision, and inventing one here would produce a tape that computes something the model does
not say.

---

## 6. Determinism

`TapeProgram::digest` is `sha256` over the canonical text of every tape, including the constant
pools. It sits one level below `order_digest`: the planner's digest makes a change in evaluation
order reviewable, and this one makes a change in *generated code* reviewable.

```rust
let a = lower_plan(&plan)?;
let b = lower_plan(&plan)?;
assert_eq!(a.digest, b.digest);
assert_eq!(a, b);           // field for field, not just the hash
```

Constants are interned by **bit pattern**, not by value, so `0.0` and `-0.0` stay distinct — interning
them together would change results. Every map in the crate is a `BTreeMap`; nothing iterates a hash
map.

The conformance corpus is lowered end to end in the crate's tests, at both `--O1` and `--O0`, and
every tape is checked against the invariants the kernel is allowed to assume without testing: every
register is inside the frame, every site and constant id resolves, only trapping ops carry masks, and
every slot's op range ends in a store.

---

## 7. What comes next

The tape is the kernel's input, and the kernel (T10) is what executes it: chunked column-major
evaluation with `series_buf[slot][t_ring][c]`, per-lane `TrapFlags`, and scalar diagnostic replay.
Two things this crate deliberately leaves to it:

- **Substage levelling** (§5.3). An `init` that reads a stage-2 value is a legal dependency
  inversion. Lowering emits the seed section that makes it expressible; deciding how many levels the
  `t` loop runs is T11's.
- **Table compilation.** `Lookup` carries a `TableId` into `TapeProgram::tables`, a list of names in
  first-use order. Binding those names to compiled structures is `predictable-tables`' job
  ([Tables and lookups](tables.md)).
