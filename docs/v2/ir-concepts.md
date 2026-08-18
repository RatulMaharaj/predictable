# The IR: concepts reference

Predictable v2 splits authoring from execution. You write a model in Python (or by hand), and
what actually runs is a file: the **predictable IR**, `format = "pir/1"`. It is plain text, it
lives in git, and it is reviewable in a pull request.

This page is the reference for what is *in* that file — the concepts an actuary or a developer
needs in order to read one, write one, or build a tool that consumes one. The crate
`predictable-ir` is the Rust data model of exactly these concepts, and a generated JSON encoding
(`pir.json`) makes them available to any other language.

Three rules shape everything below:

1. **No arbitrary code crosses the boundary.** Every formula is a typed expression tree. No
   lambdas, no `eval`, no plugin callbacks during a projection.
2. **The text form is the source of truth.** The engine parses the text; the Python DSL emits it;
   you can hand-write it and it will run.
3. **Everything checkable is checked before execution.** Shapes, dtypes, units, timing, table
   keys and missingness are declared, so a mistake is a definition-time error with a fix, not a
   `KeyError` on row 40,000.

---

## Components

A **component** is a named, typed node of the computation graph — the unit of definition, of
results, and of diffing. It is Prophet's *variable*, with types.

```toml
[[component]]
name   = "premium_income"
kind   = "Derived"
dtype  = "f64"
shape  = "Series"
unit   = "money"
timing = "start"
expr   = "premium_rate * num_pols_if"
doc    = "Office premium received, in advance."
```

Every component has a `kind`:

| Kind | Meaning |
|---|---|
| `Input.Modelpoint` | read from the modelpoint file |
| `Input.Assumption` | read from the assumption set |
| `Input.Table` | an external table declaration |
| `Input.Timeline` | `t`, `policy_year`, `period_start_date`, … |
| `Derived` | computed from an `expr` |
| `Output` | `Derived`, additionally marked for emission |

Inputs have no `expr`; everything else must have one. There is no "declared but undefined" kind:
a `.pir` file never contains a hole.

**`Output` is the single source of truth for what a run emits.** A product file also lists its
outputs, but that list is a *checked manifest*, not a selector — if it disagrees with the model,
you get `E0107` rather than a quietly different result set.

**Identity.** A component's canonical id is `<module_path>.<name>`, dot-separated:
`term_assurance.decrements.num_pols_if`. That is the join key in results, traces, run diffs and
mapping files, and no other separator is legal.

---

## Shape — the three-way distinction

Every component is exactly one of:

| Shape | One value per… | Example |
|---|---|---|
| `Scalar` | run | `valuation_rate` |
| `PerMP` | modelpoint | `entry_age`, `bel` |
| `Series` | modelpoint × period `t` | `premium[t]`, `reserve[t]` |

Broadcasting **widens** implicitly and for free:

```
Scalar ⊔ PerMP = PerMP      PerMP ⊔ Series = Series      Scalar ⊔ Series = Series
```

Narrowing is **never** implicit. To go from `Series` to `PerMP` you write an explicit aggregate —
`sum`, `npv`, `first`, `last`, `at`, `max_over`, `min_over`, `count_while`. Going from `PerMP` to
`Scalar` is a portfolio-level operation and is not expressible inside a projection at all; it is
a declared `[[aggregation]]` in the run file.

There is deliberately no "per-`t` but not per-modelpoint" shape. A yield curve indexed by `t`
alone is just a `Series` that happens to reference no `PerMP` value; the engine hoists it out of
the modelpoint loop, but the model sees a uniform `Series` and formulas stay free of broadcast
syntax.

---

## DType and Unit

```
DType ::= f64 | i64 | bool | date | str | enum(<EnumName>)
```

`f64` is the only float. There is no `f32` and no decimal type.

`Unit` is a lightweight dimensional tag — checked, never converted:

```
none | money | rate(annual|monthly|period) | prob | count | years | months | factor
```

Unit checking is intentionally shallow, because it targets the mistakes actuaries actually make:

- `money + prob` → error.
- `rate(annual) + rate(monthly)` → error, with the fix `to_monthly(x)`.
- `money * prob` → `money`; `money * money` → error.
- `money / money` → `factor`; `1 - prob` → `prob`.

`unit = "none"` opts out. It is legal, occasionally right, and always linted.

---

## Timing

`Series` components declare *when within period `t`* their value is realised:

| Timing | Meaning | Typical use |
|---|---|---|
| `start` | in advance | premiums, annuities-due |
| `end` | in arrears | claims paid, interest credited |
| `mid` | uniformly over the period | a common claims approximation |
| `point` | a state at an instant | reserve, in-force count |

Timing is not decoration. `npv(x, disc)` reads the timing of `x` to pick the discount exponent —
`start` → `v^t`, `end` → `v^(t+1)`, `mid` → `v^(t+0.5)`, `point` → `v^t` — which removes the most
common off-by-one-period error in hand-rolled models. `retime(x, timing)` is the escape hatch,
and because it is a visible call it shows up in the diff.

`Scalar` and `PerMP` components have **no** timing, and the JSON writes `timing: null` for them
rather than omitting the key: an aggregate's result is untimed because the reduction already
consumed the timing. Applying `retime`/`shift`/`cum`/`diff` to an untimed value is the lint
`W0105`, whose suggested fix pushes the call inside the aggregate.

---

## Expressions

```
Expr ::= Lit | Ref | Lag(name, k) | At(name, k)
       | Unary | Binary | If(cond, then, else)
       | Call(fn, args) | Lookup(table, keys) | Agg(op, value, pred?)
```

Written infix, inside a string:

```toml
expr = "num_pols_if[t-1] * (1 - qx[t-1]) * (if in_term then 1.0 else 0.0)"
```

Notably absent: user-defined functions, loops, mutable state, lists, dictionaries and anything
that can fail to terminate. An expression evaluates in time proportional to its own size.

`if … then … else …` is a **value** conditional — both arms are typed and neither is control
flow. A trap raised inside an arm that is not taken is suppressed, derived syntactically from the
enclosing conditions, which is what makes the standard defensive idiom correct:

```toml
expr = "if exposure == 0 then 0.0 else claims / exposure"
```

### Time references

| Written | Node | Meaning |
|---|---|---|
| `x` | `Ref` | `x` in the same period |
| `x[t-1]` | `Lag` | the previous period; `k` may be any positive integer |
| `x[0]`, `x[12]` | `At` | an absolute period — how you reference an issue-time value |

Forward references (`x[t+1]`, `x[T]`) are **illegal**: a projection is a single forward pass. A
value that depends on the whole completed series is expressed with an aggregate, which runs in
stage 2.

Out-of-range reads are fixed, not configurable. `x[t-k]` before the origin gives `x`'s `init` if
one is declared, else the zero of its dtype — and `explain()` shows that it did. `date` and `str`
have no zero, so a lag on them without an `init` is a definition-time error.

### Builtins

The function set is closed for IR 1.0; adding to it is a minor version bump.

```
arithmetic   min max abs floor ceil round(x, dp) clamp(x, lo, hi) sign
exp/log      exp ln pow(x, y) sqrt
rates        to_monthly to_annual nominal_to_periodic v_from_i i_from_v annuity_factor compound
timing       shift(x, k) retime(x, timing) cum(x) diff(x)
logic        and or not eq ne lt le gt ge is_null coalesce
dates        year month day add_months months_between year_frac
aggregates   sum sum_kahan npv first last at max_over min_over count_while
```

Two behaviours worth memorising: `round(x, dp)` is round-half-**away-from-zero** on the shortest
decimal representation of `x` (Prophet convention, not IEEE round-half-even), and
`count_while(cond)` stops at the first period where `cond` is false rather than counting all true
periods.

---

## The graph, cycles, and stages

Components form a directed graph whose edges carry a **lag**. Within a single period values must
be computable in some order, so the rule is:

> The model is valid iff the graph of `lag = 0` edges is **acyclic**. Edges with `lag ≥ 1` are
> unconstrained and may form cycles, including self-loops.

That is why

```toml
expr = "(reserve[t-1] + premium_income[t-1] - death_claims[t-1]) * (1 + valuation_rate)"
```

is legal on a component named `reserve`, while `a = b + 1; b = a + 1` is `E0201`.

**Stage** is computed, never authored: a component is **stage 2** iff its expression contains an
aggregate, otherwise **stage 1**. Stage 1 runs inside the `t` loop; stage 2 runs after it, over
the completed series. A stage-2 value may feed back into a stage-1 component through `init` and
only through `init` — which is exactly how a `reserve` seeds itself from a prospective `bel`:

```toml
[[component]]
name  = "reserve"
kind  = "Output"
shape = "Series"
init  = "bel"                       # a stage-2 npv result
expr  = "(reserve[t-1] + premium_income[t-1] - death_claims[t-1]) * (1 + valuation_rate)"
```

**`ExprPath`** names any node inside an expression with a dotted field path from `expr` or
`init` — `expr.lhs.arg0.key1` is "the second lookup key of the first argument of the call on the
left of the top-level operator". Graph edges, trap reports and traces all carry one. It is
derived from the canonical form, so reformatting a file never changes a path, unlike a byte span.

---

## Tables

A table is a declared input, not a file path buried in a formula:

```toml
[[table]]
name       = "sa8990"
keys       = [
  { name = "age",    dtype = "i64",          policy = "clamp" },
  { name = "gender", dtype = "enum(Gender)", policy = "exact" },
  { name = "smoker", dtype = "bool",         policy = "exact" },
]
values     = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source     = "tables/sa8990.csv"
digest     = "sha256:9f2c…"
```

You look it up with the `@` sigil, which cannot be confused with a function call:

```toml
expr = "sa8990@(age, gender, smoker) * mortality_loading"
```

- **Key policies** are per key: `exact`, `clamp` (to the key range), `step` (last value ≤ key, for
  banded tables) and `interpolate` (linear, numeric keys only). Enum keys are unordered and
  support `exact` only.
- **`on_missing`** is `error` by default, or `default(<lit>)`, or `interpolate(<key>)`. Silent NaN
  propagation is not available at any setting.
- **`digest` pins the content.** A run whose table bytes differ from the declared digest is
  refused unless you pass `--allow-table-drift`, and the change is visible in the git diff. Table
  changes are as reviewable as code changes.
- **`source` resolves relative to the declaring `.pir` file**, sandboxed to the project root. It
  may also be `resource:<name>` (host-supplied — this is what WASM and hosted runs use) or
  `inline`, where the rows live in the file itself for a self-contained governance pack.

---

## The modelpoint schema

The schema is part of the model, not inferred from the data file — which is what turns an MPF
mismatch into a definition-time error naming the file, the column and the components that read
it.

```toml
[[modelpoint_field]]
name = "sum_assured"; dtype = "f64"; unit = "money"; required = true

[[modelpoint_field]]
name = "commission_rate"; dtype = "f64"; unit = "factor"; default = 0.0
```

Exactly one field carries `key = true`; it is the results join key. An optional field **must**
declare a `default`.

### There are no nulls at runtime

Every value of every component is defined for every `t`. There is no validity bitmap and no
three-valued logic — this is what keeps the engine's inner loop a dense scan and keeps traces
free of "null so everything downstream is null" cascades.

Missingness is eliminated at the boundary, in exactly two places: an optional modelpoint field's
`default` at load, and a table's `on_missing` at lookup. Accordingly `is_null(x)` and
`coalesce(x, y)` observe a **presence bit**, not a value, and are legal only on an optional
modelpoint field or on a lookup into a table with `on_missing = "default(…)"`. Anywhere else they
are `E0602` — "this can never be missing" — with the suggested edit being deletion.

---

## Timeline

```toml
[timeline]
basis           = "monthly"      # monthly | quarterly | annual
periods         = 480            # T; the projection runs t = 0..=480
origin          = "policy"       # policy | valuation
valuation_date  = 2026-06-30
year_convention = "act/365"
```

This block generates the always-available timeline inputs: `t`, `period_start_date`,
`period_end_date`, `year_frac`, `month_of_year`, `policy_year`, `policy_month`,
`is_anniversary`.

`periods` is the **only** source of `T`. A run file cannot override it, or any other timeline
field (`E0101`): changing the projection length is a model change and must show up in the model's
digest.

Basis conversion is explicit. A `rate(annual)` assumption in a monthly model must pass through
`to_monthly()`, defined as `(1 + r)^(1/12) - 1`. Writing `r / 12` on an annual rate is an error
that names both the compounding conversion and `nominal_to_periodic(r, 12)` if simple division is
genuinely what you want.

---

## Products, runs, solves and aggregations

Four file kinds share the `.pir` format. Beyond model modules and assumption sets:

**Product** — a named entry point: which modules make up the model, and what it promises.

```toml
[product]
name        = "TERM_UK"
modules     = ["term_assurance/schema", "term_assurance/model"]
outputs     = ["bel", "reserve", "net_cashflow"]
key_field   = "policy_number"
assumptions = "assumptions/base"
```

**Run** — what you are executing, and how.

```toml
[run]
product      = "products/term_uk"
assumptions  = "assumptions/base"
modelpoints  = "data/term.mpf.parquet"
out          = "runs/2026-06-30-base"
emit         = "outputs"          # outputs | all | list
retain       = "ring"             # ring | full
on_trap      = "abort"            # abort | continue
max_errors   = 100
sum_kahan    = false

[run.exec]                        # non-semantic: threads, chunk size, progress
threads    = 8
chunk_size = 1024
```

The partition between the two halves is a rule rather than a hand-maintained list:

> A field participates in `run_digest` **iff changing it can change a number in the results.**

So `[run.exec]` and `out` are excluded — `threads = 1` and `threads = 64` are bit-identical, and
where results are written is not what they are — while `emit`, `retain`, `on_trap`, `max_errors`,
`allow_table_drift`, `sum_kahan`, table overrides, solves and aggregations are all included. A
run is reproducible from six digests: model, assumptions, modelpoints, tables, run, and the
engine version.

**Solve** — a root-find wrapped *around* the projection, never inside it, so the projection stays
a pure function:

```toml
[[solve]]
name      = "premium_solve"
target    = "bel"
to        = 0.0
vary      = "annual_premium"
scope     = "per_mp"      # per_mp | portfolio
tolerance = 1e-8
max_iter  = 50
method    = "brent"
bracket   = [0.0, 1.0e6]
```

The run manifest records how it went — converged and non-converged counts, iteration statistics,
the largest residual and which modelpoint produced it — and the solved value is also written out
as an ordinary `PerMP` component so downstream joins need no special case.

**Aggregation** — portfolio reporting, declared in the run file because it is a property of what
you are reporting, not of the model:

```toml
[[aggregation]]
name     = "bel_by_cohort"
group_by = ["product_code", "entry_year_band"]
measure  = "bel"
op       = "sum"                # sum | mean | min | max | count | weighted_mean
filter   = "in_force_at_val"    # a bool PerMP *component*, never an inline expression
over_t   = "each"               # each | total; Series measures only
```

Groupings **do not nest**: `group_by` is one ordered key tuple producing one flat row per distinct
tuple, with no subtotal rows. A drill-down UI is built from successive prefixes of the tuple, and
declaring both `["product"]` and `["product", "cohort"]` as two blocks is how you ask for both
levels. The output's `group_key` is rendered `product_code=TERM_UK|entry_year_band=2015-2019`, in
`group_by` order — it is a stable join key.

Because aggregation happens strictly after all projections, and no expression can reference
another modelpoint, modelpoints are independent by construction — which is what makes the
parallel and serial results bit-identical.

---

## Metadata

Every component may carry a `meta` block. None of it participates in evaluation:

```toml
[component.meta]
id          = "01J8Q2K7…"                # stable identity, survives renames
authored_by = "migration"                # dsl | hand | migration
source      = { system = "prophet", library = "TERM_UK", variable = "BEL_TOT" }
origin_span = { file = "reserves.py", line = 34, col_start = 12, col_end = 44 }
display     = { dp = 2, scale = 1.0 }    # rendering only
```

`origin_span` is load-bearing: it is what lets a diagnostic raised against the IR be rendered
against the Python (or Prophet) source that produced it. `source` is written by the migration
tool, and is what makes a Prophet conversion reviewable variable by variable.

---

## `pir.json`

`.pir` is the source of truth; `pir.json` is a **generated, lossless** JSON encoding for tooling
that should not embed a parser — a browser, a notebook, another language. It is not committed by
default.

The transcription is mechanical: array-of-table names become array fields, and expressions become
node-tagged trees, so every node is self-describing.

```json
{
  "name": "qx",
  "kind": "Derived",
  "dtype": "f64",
  "shape": "Series",
  "unit": "prob",
  "timing": "end",
  "expr": {
    "node": "Binary", "op": "mul",
    "lhs": {
      "node": "Lookup", "table": "sa8990",
      "keys": [
        {"node": "Ref", "name": "age"},
        {"node": "Ref", "name": "gender"},
        {"node": "Ref", "name": "smoker"}
      ]
    },
    "rhs": {"node": "Ref", "name": "mortality_loading"}
  }
}
```

Reading and writing it from Rust:

```rust
use predictable_ir::{Component, DType, Expr, Module, PirFile, Shape, Stage};

let mut module = Module::new("term_assurance");
module.components.push(Component::derived(
    "pv_claims",
    DType::F64,
    Shape::PerMp,
    // npv(death_claims, disc_factor)
    Expr::Agg {
        op: predictable_ir::AggOp::Npv,
        value: Box::new(Expr::r#ref("death_claims")),
        pred: Some(Box::new(Expr::r#ref("disc_factor"))),
    },
));

// The aggregate makes the component stage 2 — computed, never authored.
assert_eq!(module.components[0].stage(), Stage::Two);

let json = PirFile::from(module.clone()).to_json_pretty()?;
let decoded = PirFile::from_json(&json)?;
assert_eq!(decoded, PirFile::from(module));
# Ok::<(), predictable_ir::IrError>(())
```

Round-tripping is a tested guarantee, construct by construct: decode ∘ encode is the identity on
values, and encode ∘ decode is byte-identical on text.

---

## Versioning

Every file begins `format = "pir/N"`, and the IR follows semver on `MAJOR.MINOR`.

- **Minor bump** (`1.0` → `1.1`): new builtins, new optional fields, new unit tags, new table
  policies. Old files load unchanged.
- **Major bump** (`1.x` → `2.0`): grammar or semantic changes and removals. The engine reads the
  previous major and points anything older at `predictable migrate`.
- **Semantics never change silently under a fixed version.** If period-0 handling of `npv`
  changed, that would be a major bump — regression packs depend on it.

Two things are reserved and rejected today so that enabling them later is a minor bump rather
than a format change: `run.storage_precision = "f32"` (`E0108`; all arithmetic stays `f64`
regardless), and a stochastic scenario axis.

---

## Where this is normative

This page is the user-facing reading of `docs/design/01-ir.md`, which is the normative
specification, and of the `predictable-ir` crate, which is its Rust data model. Where the two
disagree, the design document wins — and that is a bug worth reporting.
