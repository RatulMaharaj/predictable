# Declaring a model in the DSL

[The tracer](dsl-tracer.md) turns one function body into one IR expression. This page is everything
around it: how a component is declared, where its dtype and unit come from, how the modelpoint
schema, assumptions, tables and timeline are written, and how a product team specialises a company
library without forking it.

Two rules shape the whole layer, and everything else follows from them.

**Dependencies are the parameter list.** A component reads exactly what its signature names. A free
variable that resolves to another declaration is an error (`E1103`), not a convenience — because the
dependency set is what the model explorer, `explain()` and the impact analysis read, and a set you
have to parse a body to discover is a set no reviewer has.

**Nothing is inferred.** `timing` is required, the return annotation carries dtype and unit, a
redefinition of an inherited name needs `@override`, and an override that changes the contract is
refused. Each of those is a decision an actuary has to make anyway; the DSL only insists it be
written down, where a diff can see it.

---

## A component

```python
from predictable import series, t
from predictable.fn import when
from predictable.timing import END, START
from predictable.units import Count, Money, Prob

@series(timing=END)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    """Death outgo in year t."""
    return sum_assured * deaths
```

builds to the component block of [`01-ir.md` §2.1](../design/01-ir.md):

```toml
[[component]]
name   = "death_claims"
kind   = "Derived"
dtype  = "f64"
shape  = "Series"
unit   = "money"
timing = "end"
expr   = "sum_assured * deaths"
```

| Where it comes from | What it becomes |
|---|---|
| the function name | `name` — there is no `name=` kwarg; a rename is a rename |
| the decorator | `shape` (`@series` / `@per_mp` / `@scalar`) and `timing` |
| the return annotation | `dtype` and `unit` |
| `output=True` | `kind = "Output"` |
| the docstring | `meta.doc` — first line only when a blank line follows |
| the parameter list | the graph edges |
| declaration order | `declaration_index`, the IR's deterministic-order tiebreak |

### The units are the annotations

```python
from predictable.units import Money, Prob, Count, Rate, Years, Months, Factor, Flag, Num
```

| Annotation | dtype | unit |
|---|---|---|
| `Money` | `f64` | `money` |
| `Prob` | `f64` | `prob` |
| `Count` | `f64` | `count` |
| `Factor` | `f64` | `factor` |
| `Years` / `Months` | `i64` | `years` / `months` |
| `Flag` | `bool` | `none` |
| `Num` | `f64` | `none` |
| `Rate.annual` / `Rate.monthly` / `Rate.period` | `f64` | `rate(<basis>)` |
| a `ModelPoint` `Enum` subclass | `enum(<Name>)` | `none` |

A missing return annotation is `E1101`: the DSL never infers a unit, because an inferred unit
cannot catch the error it exists to catch — adding money to a probability.

Parameter annotations are **optional but checked**. Annotating a parameter with a unit the
declaration contradicts is `E1104`, and the message shows both lines:

```
error[E1104]: parameter annotation contradicts the declaration of `deaths`

   ┌─ cashflows.py:41:24
41 │ def death_claims(deaths: Money, sum_assured: Money) -> Money:
   │                  ^^^^^^ you annotated `deaths` as money (f64)
   ┌─ decrements.py:58:1
58 │ def deaths(num_pols_if: Count, qx: Prob) -> Count:
   │     ------ but `deaths` is declared as count (f64)
```

That makes an LLM's own restatement of its assumptions into a test.

### Recursion, `init`, and stage 2

```python
@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """Survivorship. Self-referential with lag 1 — legal per IR §3.1."""
    return num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * when(in_term, 1.0, 0.0)
```

`init=` takes three forms, and all three appear in the worked model:

| Form | Example | `init` in the IR |
|---|---|---|
| a literal | `init=1.0` | `"1.0"` |
| a component reference | `init=bel` | `"bel"` |
| a small traced lambda | `init=lambda annual_premium: annual_premium` | `"annual_premium"` |

The lambda is traced exactly like a body and is the only place a lambda appears in the DSL. It is
also the only legal backward channel: a stage-2 value — anything derived from an aggregate over `t`,
such as `bel = npv(...)` — may seed `init` but may not be read at time `t`. Reading one inside a
`@series` body is `E1207`:

```
error[E1207]: `bel` is computed after the projection completes
```

Reading it from another `@per_mp` body is fine, and is exactly how `bel` sums three `npv`s: that
body is stage 2 as well.

---

## The schema module

```python
from datetime import date
from predictable import ModelPoint, Enum, key, assumption, table, timeline, Key, Value, default
from predictable.units import Money, Years, Prob, Rate, Factor

timeline(basis="annual", periods=40, origin="policy",
         valuation_date=date(2026, 6, 30), year_convention="act/365")

class Gender(Enum):
    M = "M"
    F = "F"

class TermMP(ModelPoint):
    policy_number:  str = key()
    entry_age:      Years
    gender:         Gender
    smoker:         bool
    sum_assured:    Money
    annual_premium: Money
    policy_term:    Years
    channel:        str = "DIRECT"      # optional field; the value is its IR default

valuation_rate     = assumption(Rate.annual)
mortality_loading  = assumption(Factor, default=1.0)

sa8990 = table(
    source="tables/sa8990.csv",
    keys=[Key("age", int, policy="clamp"),
          Key("gender", Gender),
          Key("smoker", bool)],
    values=[Value("qx", Prob)],
    on_missing="error",
)

lapse_rates = table(
    source="tables/lapses.csv",
    keys=[Key("policy_year", int, policy="step")],
    values=[Value("lapse_pa", Prob)],
    on_missing=default(0.0),
)
```

- **`ModelPoint` is a schema, never a record.** It is not instantiated and no component receives one;
  components take individual fields as parameters. Touching it as a value is `E1302`. Exactly one
  field must be `key()` — zero or two is `E1301` — because the key is the join column for results,
  `explain()` and every run-diff alignment.
- **Enums are unordered.** Members compare by equality, never by ordinal, and a `clamp` / `step` /
  `interpolate` policy on an enum table key is refused at declaration.
- **Assumptions name themselves** from the left-hand side of the assignment, and their *values* live
  in a separate assumption set, never in Python — a scenario run varies values thousands of times
  while the model is fixed.
- **The timeline** makes `t`, `policy_year`, `policy_month`, `period_start_date`, `period_end_date`,
  `year_frac`, `month_of_year` and `is_anniversary` available as parameter names everywhere.
  Declaring a component with one of those names is `E1401`.
- **Tables are parameters too.** The `sa8990=sa8990` default-argument idiom keeps the dependency
  rule intact and makes a table swap a signature-level change:

```python
@series(timing=END)
def qx(age: Years, gender: Gender, smoker: bool,
       mortality_loading: Factor, sa8990=sa8990) -> Prob:
    return sa8990(age, gender, smoker) * mortality_loading
    # expr = "sa8990@(age, gender, smoker) * mortality_loading"
```

Using a table the signature does not declare — or declaring one the body never looks up — is
`E1106`, in both directions, with the exact line to add or remove.

Multi-value tables name their column (`rates(policy_year).lapse_pa`); single-value tables allow the
bare form.

---

## The product

```python
from predictable import product

product(
    "term_assurance",
    modules=["models.term_assurance.schema",
             "models.term_assurance.decrements",
             "models.term_assurance.cashflows",
             "models.term_assurance.reserves"],
    outputs=["net_cashflow", "bel", "reserve", "pv_premiums", "pv_claims", "pv_expenses"],
    key_field="policy_number",
)
```

`outputs=` is a **checked manifest, not a selector**. `output=True` on the component is the single
source of emission truth ([IR §13 Q2](../design/01-ir.md)); the list exists so a reviewer sees the
promised result set in one readable place, and so adding an output shows up as a diff in two places.
A disagreement is `E1601`, and it says which way round:

```
error[E1601]: product `term_assurance` output list disagrees with the model

  marked `output=True` but absent from `outputs=`: surrender_value
```

---

## Libraries: `extends`, `@override`, `@abstract`, `@final`

This is the section that decides whether a company can adopt the tool. Prophet shops have a central
library team and product teams that must specialise it without forking.

```python
# products/term_2026/decrements.py
from predictable import extends, override
import acme_lib.mortality as acme

extends(acme)          # this module inherits every component of acme.mortality

@override(acme.qx_loading)
@series(timing=END)
def qx_loading(policy_year: Years) -> Factor:
    """Term-2026 uses a select loading; the library's flat 1.0 does not apply."""
    return when(policy_year <= 5, 0.75, 1.0)
```

| Construct | Rule | Code when broken |
|---|---|---|
| `extends(m)` | copies `m`'s components in — copies, not links, so the built IR is self-contained and the engine never resolves inheritance | — |
| redefining an inherited name | needs `@override` | `E1501` |
| `@override(x)` | must preserve shape, dtype, unit **and** timing | `E1502` |
| `@abstract` | a product that leaves one undischarged cannot be built | `E1203` |
| `@final` | forbids override | `E1504` |
| `name_suffix=` | the only sanctioned way to compose a name programmatically | — |

An override is recorded in the IR — `meta.overrides` plus a `override:acme_lib.mortality.qx_loading`
tag — so the explorer can render the library-vs-product delta and `predictable diff` can show a
product's whole deviation surface from its library.

`extends()` also accepts a `Library`, which is how a distributed package names its modules:

```python
# acme_lib/__init__.py
from predictable import Library
LIBRARY = Library(name="acme", version="3.2.0", modules=["acme_lib.mortality"])
```

### Abstracts fail the build, not the checker

`@abstract` is **DSL-only**. There is no `Kind::Abstract` in the IR: an undischarged abstract fails
`predictable build` and *no `.pir` is written*, so the engine's resolve pass never sees a hole
([IR §13 Q10](../design/01-ir.md)).

```
error[E1203]: product `term_2026` has 1 unimplemented abstract component

  Required, with the signature each override must match:

    surrender_value(policy_year: years, sum_assured: money) -> money
        @series(timing=END)    acme_lib.base_life:12

  help: Add them to a module in this product, decorated with `@override(<lib>.<name>)`.
  If this product genuinely has no such value, override it with a zero and say why in the
  docstring — an explicit zero is auditable, a missing component is not.
```

!!! note "Two meanings of `E1203`"
    The DSL catalogue in `02-dsl.md` §10 lists this case as `E1503`, while the IR decision log
    (§13 Q10) fixes the emitted code at `E1203` — the same code the tracer uses for a non-constant
    lag index. The implementation follows the IR decision log and emits `E1203`, with the difference
    visible in the message. The two are distinguishable by shape (one names a product and lists
    signatures, the other names one component and one lag).

### Generated declarations

A `for` loop that *declares* components is code generation, not computation, and is allowed —
it runs before any tracing happens:

```python
for band, lo, hi in EXPENSE_BANDS:
    @series(timing=START, name_suffix=band)
    def expense_band(sum_assured: Money, lo=lo, hi=hi) -> Money:
        return when(all_(sum_assured >= lo, sum_assured < hi), 1.0, 0.0)
```

`name_suffix` appends `_<suffix>`. Plain-Python defaults (`lo=lo`) are build-time constants and are
folded into the expression as literals, so the first iteration produces

```toml
name = "expense_band_a"
expr = "if sum_assured >= 0.0 and sum_assured < 100.0 then 1.0 else 0.0"
```

with `meta.authored_by = "dsl"` and a `generated_from` span pointing at the loop, so a reader of the
IR can find where it came from.

---

## Building

```python
from predictable import build

model = build(TERM)                    # or build() for everything declared

model.component("qx").expr.to_pir()    # 'sa8990@(age, gender, smoker) * mortality_loading'
model.component("qx").dependencies     # ('age', 'gender', 'smoker', 'mortality_loading')
model.component("reserve").init.to_pir()   # 'bel'
model.outputs                          # ('net_cashflow', 'pv_premiums', ..., 'reserve')
model.to_ir()                          # IR-shaped data: timeline, enums, fields, tables, components
[w.code for w in model.warnings]       # ['W1101'] — declared, never read, not an output
```

`build()` is steps 1–3 of [`02-dsl.md` §8](../design/02-dsl.md): collect (recording declaration
order), trace (once per body), then resolve and check. What comes back has no unresolved names, no
untraced bodies and no undischarged abstracts. Rendering it to canonical `.pir` text, computing
table digests and re-running the engine's own checker against the Python source are the next task in
the chain.

Resolution order for a parameter name, first match wins, ties are `E1105`:

1. a component in the same module (including inherited ones),
2. a component in another module of the build,
3. a modelpoint field,
4. an assumption,
5. a timeline input,
6. a table.

An unknown name is `E1103`, with the nearest name in scope attached as a machine-applicable edit and
every namespace listed so the next attempt has the whole vocabulary:

```
error[E1103]: unknown name `num_pols_iff` in the parameter list of `premium_income`

   ┌─ cashflows.py:28:26
28 │ def premium_income(premium_rate: Money, num_pols_iff: Count) -> Money:
   │                                         ^^^^^^^^^^^^ not a component, modelpoint field,
   │                                                      assumption, table, or timeline input

  Components in scope: num_pols_if, deaths, qx, wx, in_term, age
  Modelpoint fields: entry_age, gender, smoker, sum_assured, annual_premium, policy_term
  …
  help: there is a name `num_pols_if` in scope; did you mean it?
```

---

## The whole catalogue this layer raises

| Code | Meaning |
|---|---|
| `E1101` | missing return annotation (no dtype or unit) |
| `E1102` | `@series` without `timing` |
| `E1103` | unknown parameter name, or a free variable read from the body |
| `E1104` | parameter annotation contradicts the referenced declaration |
| `E1105` | ambiguous name across namespaces |
| `E1106` | table used but not in the signature, or in the signature and never used |
| `E1203` | undischarged `@abstract` in a product (IR §13 Q10) |
| `E1207` | stage-2 value read in `expr` rather than `init` |
| `E1301` | modelpoint schema has zero or multiple `key()` fields |
| `E1302` | modelpoint object accessed as a value |
| `E1401` | redefinition of a timeline input |
| `E1402` | arithmetic basis conversion on a `Rate` |
| `E1501` | shadowing an inherited component without `@override` |
| `E1502` | `@override` changes shape, dtype, unit or timing |
| `E1504` | override of a `@final` component |
| `E1601` | `product(outputs=...)` disagrees with the `output=True` set |
| `W1101` | component declared but never read and not `output=True` |

The tracer's own codes — `E1201`–`E1206` — are on [the tracer page](dsl-tracer.md). Every code is
anchored in the [diagnostics catalogue](../llm/diagnostics.md).
