# Determinism and the golden corpus

`predictable` makes an unusually strong promise about results: **the same model, assumptions,
modelpoints and tables produce the same bits** — not "the same to twelve decimal places", the same
IEEE-754 bit patterns — regardless of how many threads you gave it, how it chunked the file, which
executor ran it, or which of the supported platforms it ran on.

An actuary reconciling a valuation to the penny will notice a last-ULP difference. So the engine
treats reproducibility as a tested property rather than an aspiration, and
`crates/predictable-determinism` is where the tests live. This page is the map: what is promised,
what enforces it, what is known not to hold yet, and how to regenerate a golden when a change is
supposed to move the numbers.

Spec: [`03-engine.md` §7](../design/03-engine.md).

---

## 1. What is promised

| # | Promise | Test |
|---|---|---|
| 1 | Lane independence: `run(C = 1) ≡ run(C = 1024)`, `threads = 1 ≡ threads = 8` | `tests/chunk_invariance.rs` |
| 2 | `sum` / `npv` walk `t` in order, per lane — no reassociation | `tests/float_contract.rs` |
| 3 | `a * b + c` is a multiply then an add, never a fused multiply-add | `tests/float_contract.rs` |
| 4 | No fast-math; transcendentals pinned to the bit | `tests/float_contract.rs` |
| 5 | No wall clock, no RNG, no environment reads inside the kernel | `tests/float_contract.rs` |
| 6 | The digest chain is a function of the model, not of the run's schedule | `tests/golden.rs` |
| 7 | A golden corpus, byte-identical on every supported target | `tests/golden.rs` |

Run all of it:

```bash
cargo test -p predictable-determinism
```

Promise 1 is about *values*. The stronger claim — that `results.parquet` is byte-identical, and
so `results.digest` is stable, under any `--chunk-size` or `--threads` — is
[Byte-deterministic result files](parquet-determinism.md).

---

## 2. What a golden looks like

A golden is a whole run rendered as reviewable text: the digest chain, the planner's evaluation
order, the tables it read, and then **every emitted value as its bit pattern**, in
`(chunk_idx, offset)` order — the same order the results writer emits.

```text
case v02-term-assurance
modelpoints data/term.mpf.csv
modelpoint_digest sha256:8a29a3eb…
model_digest      sha256:e1a00149…
order_digest      03db52ec…
plan_digest       a752868a…
tape_digest       b0af92ef…
periods 40
order policy_number entry_age gender … reserve pv_premiums pv_claims bel
table sa8990 rows=12 index=cartesian dense array drifted=false digest=sha256:9c0ebb1a…
assumption valuation_rate 0x3fa1eb851eb851ec 0.035
outcome Completed exit=0
projected 2 trapped 0 traps 0
POL00001 term_assurance.net_cashflow 0 0x4061800000000000 140.0
POL00001 term_assurance.net_cashflow 1 0x405f5733b89430a6 125.36253180000003
…
POL00001 term_assurance.bel . 0xc0a1af0545b5ada7 -2263.510297467659
```

The hex is the assertion; the decimal beside it is for the human reading the diff. A golden that
compared only decimals would happily pass on a platform whose `pow` is one ULP out, which is the
exact bug the corpus exists to catch.

Lines beginning with `#` — the generating target, the regeneration hint — are provenance and are
stripped before comparison.

### Where the models come from

The [conformance corpus](conformance-corpus.md) is the source. Every `valid/**` case is planned,
lowered and projected:

* if the case ships its own `data/` modelpoint file (as `v02-term-assurance` does), that file is
  used verbatim;
* otherwise the harness synthesises one **from the declared schema alone** — the unit picks the
  magnitude (`prob` gets a probability, `money` gets money), nothing is zero, and nothing depends
  on the host. `v05-canonical-fmt` was written as a formatter fixture and has no data; it still
  produces a golden, because a synthetic modelpoint file derived deterministically from a
  declaration is as reproducible as a committed one.

A case the harness genuinely cannot execute is not silently dropped: it lands in
`golden/_skipped.txt`, which is itself a golden. Today that file holds exactly one entry —
`v06-tables`, whose `expense_scale` table lives behind a `resource:` URI that only a host can
supply.

---

## 3. Regenerating a golden

A changed golden is a changed result, and the diff is the review:

```bash
UPDATE_GOLDEN=1 cargo test -p predictable-determinism
git diff crates/predictable-determinism/golden/
```

The rule from `03-engine.md` §7 clause 7 applies: **a deliberate result change regenerates the
goldens in the same commit as the change that caused it**. A commit that moves numbers without
touching `golden/` is either wrong or lying, and a commit that touches only `golden/` needs to say
why in its message.

---

## 4. Cross-platform

The committed goldens were generated on **aarch64 macOS** (Apple silicon). There is deliberately
**one golden per case, not one per target**: §7 requires byte-identical output on
`x86_64-linux`, `aarch64-macos` and `wasm32-wasi`, so a per-target golden would encode the very
divergence the corpus forbids.

Adding a platform therefore means adding a runner, not adding files.
`.github/workflows/determinism.yaml` runs the same `cargo test -p predictable-determinism` on
Ubuntu (x86-64), macOS 14 (aarch64) and macOS 13 (x86-64), with a separate, currently
non-blocking `wasm32-wasip1` job under `wasmtime`.

The wasm job is `continue-on-error` on purpose, and it is worth being explicit about why: the
kernel currently calls `f64::exp`, `f64::ln` and `f64::powf` — the *platform* libm — where §7
clause 4 requires a vendored `libm`. glibc's `pow` and macOS's `pow` differ in the last ULP.
Until that is fixed, `transcendentals.golden` is a platform-dependent file, and the honest thing
is to say so here rather than to weaken the test. When the engine vendors `libm`, drop
`continue-on-error` and the job becomes a real gate.

---

## 5. Property tests over generated programs

Hand-written models test the constructs someone thought of. Clause 1 is a claim about *every*
program, so it is also tested against programs generated from a seed:

```rust
for seed in 0..64 {
    let p = program(seed);          // deterministic: seed → source, forever
    // plan, lower, load, then compare C = 1, 2, 3, 5, 1024
    // across serial, local(1) and local(8) executors
}
```

The generator (`src/gen.rs`) is a twenty-line LCG rather than a dependency, for one reason: a
property test whose failure cannot be reproduced from a printed seed is a flake. `program(7)` is
the same program on every machine, this year and next, so a failure message that says `seed 7` is
a complete bug report.

It generates only programs the checker accepts — every value `f64`, every self-reference at lag 1
with an `init`, every division guarded — because the target here is *expression shape*, not the
checker's error surface, which the [conformance corpus](conformance-corpus.md) owns.

---

## 6. The float contract, by example

**Reductions are sequential.** The series `[1e16, 1.0, -1e16, 1.0]` is chosen so that the order of
summation is visible in the answer:

| Summation | Result |
|---|---|
| Left to right (what the engine must do) | `1.0` — both `1.0`s are lost against `1e16`, one is recovered by the cancellation |
| Pairwise / tree (a reassociation) | `2.0` |
| `sum_kahan` (compensated, and a different builtin) | `2.0` |

The test asserts the engine returns `1.0`, *and* asserts that pairwise summation returns something
else — so a future edit that makes the input non-adversarial fails rather than passing vacuously.

**No FMA contraction.** The test picks triples where `a * b + c` and `fma(a, b, c)` genuinely
differ (`(1+ε)² − 1`, `⅓ × 3 − 1`, `0.1 × 0.1 − 0.01`, …), refuses to run unless at least three of
them still distinguish the two, then asserts the engine's answer equals the unfused one and
differs from `f64::mul_add`.

---

## 7. Known divergences

The harness records what does not hold yet, in the test that would otherwise hide it.

**`at(series, k)` is not lane-independent.** In `v04-expressions`, `flow_at_five = at(flow, 5)`
returns lane 0's value for every lane in a chunk wider than one modelpoint (`0.0` for the rest),
and under a rayon executor — where one engine is reused across chunks — the wrong value leaks
between chunks too, so `serial` and `local(8)` disagree even at `C = 1`. The accumulator is
evidently per engine rather than per lane; this is a `predictable-engine` stage-2 defect
(`03-engine.md` §5.3).

It is listed in `KNOWN_DIVERGENCES` in `tests/chunk_invariance.rs`, which does two things: it
redacts those lines from the invariance comparison so the rest of the model is still covered, and
it asserts through `known_divergences_still_reproduce` that the divergence *still happens*. Fix
the engine and that test fails, with a message telling you to delete the entry. A list of known
bugs that quietly stops being true is worse than no list.

**`enum` columns cross the io → engine boundary in two codings.** `predictable-io` decodes an
`enum` modelpoint column to its declared *ordinal* (`M` → 0), while `predictable-tables` keeps an
`enum` table key as text for the executing engine's dictionary to intern. An `enum`-keyed lookup
therefore misses every row. With the harness's workaround removed, `v02-term-assurance` — the
corpus's only `runtime = true` case — aborts at `t = 0` with two `LookupMiss` traps and projects
nothing. `corpus::restore_enum_text` puts the declared spelling back before the engine sees the
chunk; it is commented as the workaround it is and should be deleted when the representation is
settled between io, runner and engine.

---

## 8. Tools

```bash
# The whole gate.
cargo test -p predictable-determinism

# Which components move under chunking or executor choice, case by case.
cargo run -p predictable-determinism --example divergence_survey
```

`divergence_survey` prints `stable` or the moving components per case; it is what to reach for
when an invariance test fails and you want the shape of the failure rather than its first line.
