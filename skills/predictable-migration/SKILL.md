---
name: predictable-migration
description: >
  Migrate an FIS Prophet library (.MPF, .FAC, .RPT, workspace source) into a predictable model
  and reconcile it to the Prophet run. Use when the user mentions Prophet, MPF, .fac, .rpt,
  actuarial model migration, or asks to reimplement/validate a life insurance projection model.
allowed-tools: Read, Write, Edit, Bash, Grep, Glob
---

# Migrating a Prophet library into predictable

You are reimplementing an actuarial projection model in a format you can check, and then
proving the reimplementation right by diffing it against the run it came from. The proof is
the deliverable. A model that produces plausible numbers and cannot be reconciled is not a
migration; it is a second opinion.

Work the steps in order. They are ordered by how errors propagate, not by convenience.

## 0. Ground rules

* **Never widen a tolerance to make a finding go away.** This is the one hard prohibition.
  A tolerance is a statement about what difference is immaterial; loosening it to clear a
  finding converts an unexplained difference into an invisible one. Fix the model, or stop
  and put the trade-off to the user in writing. `scripts/loop.py` refuses `--abs`/`--rel`
  outright, and flags any mapping override that loosens a comparison.
* **Do not edit goldens, expected files, or `mapping.toml` to make a run pass.** A
  `mapping.toml` entry is a claim about the *Prophet* side (a sign convention, a scale, a
  period base) and every entry ends up on a human sign-off sheet.
* **Every code you are shown is documented.** A diagnostic's `doc_url` resolves to
  `docs/llm/diagnostics.md#<code>` — read the entry before guessing at a fix.
* **Prefer `--json` everywhere.** Terminal output is a projection of it and drops fields.

## 1. Inventory before you write anything

Read the Prophet workspace first and produce the variable inventory. Do not open an editor
on a `.pir` file until you can name every Prophet variable you are expected to reproduce
and every one you are deliberately not.

* `.MPF` → the modelpoint file: the field list, types and the model's population.
* `.FAC` → parameter/factor tables: dimensions, extents, key order.
* `.RPT` → the run you must reconcile to. This is the baseline; keep it.
* Workspace source → the variable definitions themselves.

Record the inventory as `migration/coverage.json`: every Prophet output variable, and
whether it is `mapped`, `derived` (reproduced but not named the same), or `out_of_scope`
with a reason. Nothing may be silently absent. See `references/prophet-idioms.md`.

## 2. Schema first

Emit `schema.pir` — modelpoint fields, enums, assumptions, tables — and run
`predictable check` on it *before writing a single formula*.

Rationale, because it changes the order you would otherwise work in: a missing or misnamed
schema field surfaces as a resolution error (`E0203`) in every formula that reads it. Ten
formulas written against a wrong schema produce forty errors, of which one is real. Get the
schema clean and the later error stream is short and true.

## 3. Units and timing are a decision, not a default

For every numeric field and every component, choose `unit` and `timing` explicitly and
write them down. Do not let them default and do not copy them from a neighbour.

* An annual rate divided by 12 is **an error, not a translation** (IR §5). A monthly model
  needs a monthly timeline and a rate expressed on it — `q_monthly = 1 - (1 - q)^(1/12)`,
  not `q/12` — and the unit system will tell you which one you wrote.
* Prophet timing → IR timing is a real mapping, not a formality: see the table in
  `references/prophet-idioms.md`. A `start`/`end` mistake is a one-period shift that the
  run diff will later report as `H0201`, at the cost of an hour.
* `W0102` (a unit lint) is not noise. It is exactly this decision, unmade. Step 7 does not
  accept a model that still emits one.

## 4. Translate leaves upward

Work up the dependency order, checking after every layer:

1. decrements (`qx`, `wx`, and their loadings)
2. in-force / survivorship
3. cashflows (premiums, claims, expenses, commission)
4. discounting
5. reserves and the aggregates built on them

Every component gets `meta.source` naming the Prophet variable and its `file:line`
(IR §11.1):

```toml
[[component]]
name = "qx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "end"
expr = "mortality@(age, sex, smoker) * mortality_loading"

[component.meta.source]
system = "prophet"
variable = "MORT_RATE"
file = "TERM.MOD:118"
```

**`meta.source` coverage is the completion criterion for this step**, not a nicety. A
component nobody can trace back to a Prophet variable is a formula you invented, and it
will be the one nobody can defend in review. `scripts/loop.py` reports coverage on every
iteration and refuses to call the loop done while any component is missing it.

## 5. Run and diff

```console
$ python skills/predictable-migration/scripts/loop.py \
      --model build --run run.pir \
      --baseline prophet_run/ --tolerance-profile reconcile
```

The loop runs `check` → `run --retain-all` → `diff run --json` and prints one report.

Use `--retain-all` (the loop always does). "Which component is the root" is partly a fact
about what you asked the engine to write down: the diff cannot localise to a component that
was never emitted, and will name the nearest one it *can* see instead.

## 6. Work the findings top-down

Only `class = "root"` findings are actionable. An `inherited` finding is the same error seen
further downstream; it disappears when its root is fixed. Fixing inherited findings
individually is how a migration takes three weeks.

For each root, in order of `contribution.share_of_total_delta`:

* `confidence = "high"` **and** a `suggested_edit` → apply the edit, re-run the loop.
* otherwise → run the printed `explain_command` on **both** sides and compare the traces
  before editing anything. The trace shows the arithmetic; the hypothesis is only a guess
  about its shape.
* `explained_by_model_change` tells you whether the structural diff independently agrees.
  When it does, the localisation is corroborated by two different methods and you can move
  quickly. When it does not, slow down.

Hypothesis codes and what each one implies are in `references/diagnostics.md`. `H0201`
(a whole-period shift) is nearly always a `timing` decision from step 3, not a formula bug —
fix the timing, do not add a `timing_shift` to `mapping.toml` unless the *Prophet* side is
genuinely on a different convention.

**Re-read ground rule 0 before touching a tolerance.**

## 7. Stop condition

The migration is done when all of these hold — `loop.py` reports them as booleans in
`stop_condition` and exits `0` only when every one is true:

* `verdict = "matched"` at the `reconcile` profile;
* the coverage report shows no unmapped Prophet output variable;
* `predictable check` is clean of errors and of `W0102` unit lints;
* no tolerance was widened.

Anything less, say so plainly. "Reconciles except for `reserve` at t > 30" is a useful
result; "reconciled" when it did not is a defect you have shipped into a valuation.

## 8. Hand back a migration report

* coverage: Prophet variables mapped / derived / out of scope, with reasons;
* remaining differences, each with a justification and its root finding id;
* `mapping.toml` in full, with every `sign`, `scale` and `timing_shift` listed for human
  sign-off — these are the places where you asserted something about Prophet's conventions
  rather than reproduced its arithmetic;
* the run manifests of both sides, so the comparison can be re-executed.

## Success metric

*A Prophet library of ~50 variables reconciles to `reconcile` tolerance in under an hour of
agent time, with no human edit to the generated `.pir`.* If you are outside that, the
bottleneck is nearly always step 2 or step 3 done too fast.

## References

| File | Read it when |
|---|---|
| `references/ir-cheatsheet.md` | writing any `.pir`: shapes, units, timing, grammar, builtins |
| `references/prophet-idioms.md` | translating a Prophet construct, or choosing a timing |
| `references/diagnostics.md` | you hit an `E`/`W`/`P`/`N`/`H` code |
| `references/verify-formats.md` | reading `diff.json`, a trace, or a manifest |
| `scripts/loop.py` | every iteration — it is the loop, and the stop condition |
