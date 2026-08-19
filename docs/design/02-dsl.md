# 02 — The predictable Python DSL

Status: **normative**. Version: DSL `1.0`, targets IR `1.0` (`pir/1`).
Audience: actuaries authoring models, LLMs migrating Prophet libraries, and implementers of
`predictable.dsl`.

The DSL is an **authoring layer, not a runtime**. It has exactly one job: turn readable Python into
canonical `.pir` text (spec `01-ir.md`), and produce every diagnostic it possibly can while doing so.
Once `predictable build` has written the `.pir`, the Python is no longer in the loop — the Rust
engine reads the IR. Deleting the DSL from a project would not change a single number.

Three rules follow from that, and from the IR's "no arbitrary code crosses the boundary":

1. **Function bodies are traced, once, at build time.** A component body executes exactly once, with
   symbolic proxies in place of values, and yields an `Expr` tree. It never executes per modelpoint
   and never per `t`. There is no interpreter, no `eval`, no callback.
2. **Everything the tracer cannot represent is a definition-time error with a fix.** Python `if`,
   `for`, `while`, `and/or`, comprehensions, `numpy`, and calls to non-builtins on a traced value all
   fail loudly and name their replacement. The DSL is a *subset* of Python and says so.
3. **The DSL adds no semantics.** Every DSL construct maps to an IR construct one-to-one. If you
   cannot see how a line becomes IR, the line does not belong in the DSL.

---

## 1. The shape of a model on disk

```
models/term_assurance/
  __init__.py            # re-exports; declares the product entry point
  schema.py              # modelpoint schema, assumptions, tables, timeline
  decrements.py          # components
  cashflows.py
  reserves.py
  tables/
    sa8990.csv
    lapses.csv
  build/                 # generated, committed
    schema.pir
    decrements.pir
    cashflows.pir
    reserves.pir
  assumptions/
    base.pir             # authored as data, not Python (see §9)
```

The `build/` directory **is committed**. The `.pir` files are the reviewable artefact; the Python is
the convenient way to produce them. `predictable build --check` in CI fails if the committed IR does
not match a fresh build, exactly as `gofmt -l` or a lockfile check does.

---

## 2. Component declaration

### 2.1 The decorators

There are three shape decorators and nothing else. Shape is the axis the IR types on (`01-ir.md`
§2.3), so it is the axis the API names.

```python
from predictable import scalar, per_mp, series
from predictable.units import Money, Prob, Count, Rate, Years, Factor, Flag, Num
from predictable.timing import START, END, MID, POINT
```

```python
@series(timing=END)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    """Death outgo in year t."""
    return sum_assured * deaths
```

compiles to

```toml
[[component]]
name   = "death_claims"
kind   = "Derived"
dtype  = "f64"
shape  = "Series"
unit   = "money"
timing = "end"
expr   = "sum_assured * deaths"
doc    = "Death outgo in year t."
```

Decorator signatures, complete:

```python
def series(
    *,
    timing: Timing,                 # required — no default; see §2.5
    init: Any | None = None,        # literal, component ref, or a traced expression
    output: bool = False,
    unit: Unit | None = None,       # overrides the return annotation
    dtype: DType | None = None,
    doc: str | None = None,         # overrides the docstring
    tags: Sequence[str] = (),
) -> Callable[[F], Component]: ...

def per_mp(*, output=False, unit=None, dtype=None, doc=None, tags=()) -> ...
def scalar(*, output=False, unit=None, dtype=None, doc=None, tags=()) -> ...
```

- The **function name is the component name**. No `name=` kwarg exists; a rename is a rename, and it
  shows up in the IR diff as a rename because the normalised expression is unchanged (`01-ir.md`
  §11.3).
- The **docstring becomes `doc`**. First line only if a blank line follows; otherwise the whole
  docstring, dedented.
- The **return annotation carries dtype and unit**. `Money` is
  `Annotated[float, Unit.money]`; `Prob` is `Annotated[float, Unit.prob]`; `Rate.annual`,
  `Rate.monthly`, `Rate.period` are the three rate bases; `Flag` is `Annotated[bool, Unit.none]`;
  `Num` is `Annotated[float, Unit.none]` and triggers lint `W0102` where money arithmetic is
  involved. A missing return annotation is error `E1101` — the DSL never infers a unit.
- `kind` is never written by hand. It is `Derived`, or `Output` when `output=True`, or an input kind
  determined by the declaration form (§4, §5, §6).

### 2.2 Dependencies are the parameter list

**Decision: a component's dependencies are exactly its function parameters.** Free variables that
resolve to other components are an error, not a convenience.

```python
@series(timing=START)
def premium_income(premium_rate: Money, num_pols_if: Count, in_term: Flag) -> Money:
    return premium_rate * num_pols_if * when(in_term, 1.0, 0.0)
```

Why parameters and not implicit lexical capture (lifelib/cashflower style):

- The dependency set is greppable, diffable, and visible without parsing the body — the model
  explorer and `explain()` get it for free.
- Typos become "unknown parameter `num_pols_iff`, did you mean `num_pols_if`?" at the signature,
  with a one-token fix, instead of a `NameError` from inside a body halfway through a trace.
- An LLM writing a component states its inputs before writing the formula, which is the order it
  reasons in, and the checker can validate the signature even if the body is wrong.
- Parameter **annotations are optional but checked**. Writing `deaths: Count` when `deaths` is
  declared `unit=money` is error `E1104`. This makes an LLM's own restatement of its assumptions a
  test.

Resolution order for a parameter name, first match wins, ties are an error (`E1105`):
component in the same module → imported module component → modelpoint field → assumption → timeline
input → table. Tables are usually passed as parameters too (§6).

### 2.3 Recursion and `t-1`

`t` is imported and used as an index. Lagging is `[t-1]`; absolute indexing is `[0]`, `[12]`.

```python
from predictable import t

@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """Survivorship. Self-referential with lag 1 — legal per IR §3.1."""
    return num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * when(in_term, 1.0, 0.0)
```

- A component **may take its own name as a parameter**. Reading it *unlagged* inside its own body is
  error `E1202` (self-loop at lag 0), which is the DSL-side statement of the IR cycle rule.
- `x[t-k]` requires an integer literal `k >= 1`. `x[t-n]` where `n` is a Python variable is fine
  *if* `n` is a plain `int` at build time — it is constant-folded — and error `E1203` if it is a
  traced value.
- `x[t+1]`, `x[t]` with any positive offset, and `x[T]` raise `E1204` with the IR's rationale and a
  pointer to `Agg` + `init` as the legal backward channel.
- `init=` accepts a literal (`1.0`), a component reference (`init=bel`), or a small traced lambda
  (`init=lambda annual_premium: annual_premium * 1.0`). The lambda form is traced identically to a
  body and is the only place a lambda appears in the DSL — it never crosses the engine boundary; it
  is erased into an `init` expression string at build.

### 2.4 Conditionals, and the Python subset

`if` on a traced value cannot work — the tracer would have to pick a branch. So traced values raise
on `__bool__`, and the value conditional is a function:

```python
from predictable.fn import when, min_, max_, clamp, round_, exp, ln, pow_, sqrt
from predictable.fn import to_monthly, to_annual, nominal_to_periodic, compound, annuity_factor, v_from_i
from predictable.fn import shift, retime, cum, diff, coalesce, is_null
from predictable.fn import all_, any_, not_
from predictable.fn import npv, sum_, first, last, at, max_over, min_over, count_while, sum_kahan
```

`when(cond, then, otherwise)` is `If`. It evaluates both branches (IR §2.6); it is not a guard.
Trailing-underscore names (`min_`, `sum_`, `round_`, `pow_`) avoid shadowing builtins; the underscore
is dropped in the IR.

Operators that work on traced values: `+ - * / ** - (unary)` and `== != < <= > >=`. Boolean
combination is `all_(a, b)` / `any_(a, b)` / `not_(a)`, because `and`/`or`/`not` are `__bool__`-based
in Python and cannot be overloaded — using them raises `E1201`.

Everything else is out: `for`, `while`, comprehensions, `try`, assignment to a traced value's
attribute, `numpy`, `math`, calls to user functions on traced values. Plain Python **is** allowed
where it produces constants before tracing — module-level `TERM_CAP = 40`, f-strings in docstrings,
a `for` loop that *declares* components (§7.3) — because that is code generation, not computation.

Local variables inside a body are fine and are **inlined**, not turned into components:

```python
@series(timing=END)
def net_death_strain(sum_assured: Money, reserve: Money, deaths: Count) -> Money:
    strain_per_death = sum_assured - reserve[t-1]
    return strain_per_death * deaths
```
→ `expr = "(sum_assured - reserve[t-1]) * deaths"`. If you want it in results, make it a component.
The DSL keeps the IR graph flat by construction (IR §12: no user-defined functions).

### 2.5 `timing` is required

`@series` has no default timing. An actuary who has not decided whether a cashflow is in advance or
in arrears has not finished modelling it, and the IR consumes the tag for discounting (IR §2.5).
Omitting it is `E1102`, whose message lists the four tags with a one-line gloss each. `per_mp` and
`scalar` have no timing.

---

## 3. Modelpoint schema

```python
from predictable import ModelPoint, Enum, key
from predictable.units import Money, Years, Flag

class Gender(Enum):
    M = "M"
    F = "F"

class TermMP(ModelPoint):
    policy_number: str = key()
    entry_age:     Years
    gender:        Gender
    smoker:        bool
    sum_assured:   Money
    annual_premium: Money
    policy_term:   Years
    channel:       str = "DIRECT"        # optional field; the value is its IR default
```

- Every annotated attribute becomes a `[[modelpoint_field]]`. No default → `required = true`.
  A default → `required = false, default = <lit>`.
- `key()` marks the identity column used in results, `explain()`, and run-diff joins. Exactly one
  required (`E1301` for zero or two).
- `Enum` subclasses emit `[[enum]]` blocks and are usable as parameter annotations and as `Lookup`
  key types. Comparison is `gender == Gender.F`.
- The class is *only* a schema. It is never instantiated, has no methods, and no component receives
  a `TermMP` object — components take individual fields as parameters. Attempting `mp.sum_assured`
  inside a body is `E1302` with the fix "add `sum_assured: Money` to the parameter list".
- Binding a data file: `predictable run --mpf data/term_2026Q2.csv`. Column/type mismatches are
  checked against the schema before row 1 is projected, and the error names the file, the column, the
  schema line, and the components that read it.

---

## 4. Assumptions

```python
from predictable import assumption
from predictable.units import Rate, Money, Factor

valuation_rate     = assumption(Rate.annual)
premium_escalation = assumption(Rate.annual)
expense_inflation  = assumption(Rate.annual)
renewal_expense_pa = assumption(Money)
mortality_loading  = assumption(Factor, default=1.0)
```

An assumption is a `Scalar` `Input.Assumption` by default; `assumption(Money, shape=PER_MP)` and
`shape=SERIES` exist for per-policy and term-structure inputs. Values live in a separate
assumption-set file (§9), never in Python, because scenario runs vary them while the model is fixed.
`default=` supplies a value used when the set omits the name; without a default, omission is a run
error naming the assumption and every component that reads it.

---

## 5. Timeline

```python
from predictable import timeline

timeline(basis="annual", periods=40, origin="policy",
         valuation_date=date(2026, 6, 30), year_convention="act/365")
```

One call per model, at module scope in `schema.py`. It emits the `[timeline]` block, and makes
`t`, `policy_year`, `policy_month`, `period_start_date`, `period_end_date`, `year_frac`,
`month_of_year`, `is_anniversary` available as parameter names in any component. Declaring a
component with one of those names is `E1401` ("`policy_year` is a timeline input; you cannot
redefine it — did you mean `policy_year_capped`?").

`to_monthly()` / `to_annual()` are the only legal basis conversions. Writing `annual_rate / 12` on a
`Rate.annual` value is `E1402` (§10, and the reason every Prophet `/12` is flagged in migration).

---

## 6. Tables

```python
from predictable import table, Key, Value
from predictable.units import Prob

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

Lookup syntax is a call on the table object; the IR's `@` sigil is emitted by the compiler:

```python
@series(timing=END)
def qx(age: Years, gender: Gender, smoker: bool,
       mortality_loading: Factor, sa8990=sa8990) -> Prob:
    return sa8990(age, gender, smoker) * mortality_loading
```

The `sa8990=sa8990` default-argument idiom keeps the "dependencies are parameters" rule intact for
tables too, and makes table swaps a signature-level change. It is checked: passing a table that the
body does not use, or using a table not in the signature, is `E1106`.

Multi-value tables index the result: `sa8990(age, gender, smoker).qx`. Single-value tables allow the
bare form. Key arity and dtypes are checked against the CSV header at build time; the SHA-256
`digest` is computed by `predictable build` and written into the `.pir`, so a table edit is a line in
the diff (IR §2.9).

---

## 7. Libraries, packaging, and override

This is the section that decides whether a company can actually adopt the tool. Prophet shops have a
central library team and product teams that must specialise it without forking.

### 7.1 Distribution

A predictable library is an ordinary Python package with an entry point:

```toml
# pyproject.toml of an in-company library
[project]
name = "acme-actuarial-lib"
version = "3.2.0"

[project.entry-points."predictable.library"]
acme = "acme_lib:LIBRARY"
```

```python
# acme_lib/__init__.py
from predictable import Library
LIBRARY = Library(name="acme", version="3.2.0", modules=["acme_lib.mortality", "acme_lib.expenses"])
```

Consumers `pip install acme-actuarial-lib==3.2.0` and import modules normally. Versioning is
semver on the library, pinned in the model's lockfile, and the resolved version is recorded in the
run manifest — so "which library version produced this valuation" is answerable from the results
alone.

### 7.2 Import, extend, override

```python
from predictable import module, extends, override, abstract, final
from acme_lib.mortality import base_qx, qx_loading
```

Plain `from ... import name` brings a component into scope for use as a parameter. The interesting
case is specialising:

```python
# products/term_2026/decrements.py
from predictable import extends, override
import acme_lib.mortality as acme

extends(acme)           # this module inherits every component of acme.mortality

@override(acme.qx_loading)
@series(timing=END)
def qx_loading(policy_year: Years) -> Factor:
    """Term-2026 uses a select loading; the library's flat 1.0 does not apply."""
    return when(policy_year <= 5, 0.75, 1.0)
```

Rules, all enforced at build:

- `extends(m)` copies `m`'s components into this module's namespace. Copies, not links — the built
  IR is fully self-contained, so the engine never resolves inheritance and `explain()` never shows a
  virtual dispatch.
- Redefining an inherited name **without** `@override` is `E1501`. Silent shadowing is the failure
  mode that makes large Prophet libraries unmaintainable.
- `@override(x)` requires the new component to match `x`'s **shape, dtype, unit, and timing**.
  Changing any of them is `E1502`; changing them deliberately means the base component was wrong or
  you want a new name.
- `@abstract` declares a component with a signature and no body. A module that `extends` a library
  containing abstracts and does not override all of them cannot be built into a product (`E1503`,
  listing each unimplemented name and its required signature). This is the "the library defines the
  skeleton, the product fills in the product-specific bits" pattern, made checkable.
- `@final` forbids override (`E1504`). Reserved for regulatory or group-standard components.
- Overrides are recorded in `meta.tags` as `override:acme.qx_loading`, so the model explorer can
  render the library-vs-product delta, and `predictable diff` can show a product's whole deviation
  surface from its library.

### 7.3 Generated components

Declaring components in a loop is legitimate code generation and is allowed, because it runs before
tracing and emits ordinary declarations:

```python
for band, lo, hi in EXPENSE_BANDS:
    @series(timing=START, name_suffix=band)          # only place a name is composed
    def expense_band(sum_assured: Money, lo=lo, hi=hi) -> Money:
        return when(all_(sum_assured >= lo, sum_assured < hi), 1.0, 0.0)
```

`name_suffix` is the single sanctioned way to build a name programmatically; it appends `_<suffix>`.
Generated components are marked `meta.authored_by = "dsl"` with a `generated_from` span so a reader
of the IR can find the loop.

---

## 8. Compilation to IR

`predictable build [--check] [--out build/]`.

1. **Collect.** Import each module in the product's declared module list. Decorators register
   `Component` objects into a module registry, recording a monotonically increasing
   `declaration_index` — this is the IR's tiebreak for deterministic topological order (IR §3.2), so
   *source order in Python is preserved into the IR and into evaluation order*.
2. **Trace.** For each component, bind each parameter to an `ExprProxy` carrying the parameter's
   name and resolved kind, call the function once, and require the return value to be an `ExprProxy`
   or a literal. Operator overloads on the proxy build `Expr` nodes. `__bool__`, `__iter__`,
   `__len__`, `__hash__` and attribute access other than table `.value_name` raise DSL errors with
   fixes.
3. **Resolve and check locally.** Parameter names → declarations (§2.2), annotation cross-checks,
   override rules, self-lag rules. Everything catchable in Python is caught here, because a Python
   traceback pointing at the user's own line is a better error than an IR span.
4. **Emit.** Render each component to canonical `.pir` (IR §4.1): fixed key order, normalised
   expression spacing, Ryū floats. Compute table digests. Emit `[timeline]`, `[[enum]]`,
   `[[modelpoint_field]]`, `[[assumption]]`, `[[table]]` from `schema.py`.
5. **Full check.** Shell out to the engine's checker (via the PyO3 wheel, in-process) over the emitted
   text. Cycles, shapes, units, timing lints — all IR-level diagnostics (IR §7) come back as JSON,
   are mapped back to the Python span via a build-time span map, and are printed against the
   *Python* source.
6. **Write or diff.** `--check` compares to the committed `build/` and exits non-zero on drift.

Span mapping is the piece that must not be cut: every `Expr` node records the Python
`(file, line, col_start, col_end)` it came from, the compiler stores that alongside the `.pir` span,
and every diagnostic — including ones raised by the Rust checker on the IR text — is rendered
against the Python the user actually wrote. An engine-level `E0201` cycle error shows two Python
function bodies, not two TOML lines.

Running a model from Python is a thin wrapper over the built IR:

```python
from predictable import Model, AssumptionSet, ModelPointFile

model  = Model.build("models/term_assurance")          # or Model.load("build/")
result = model.run(
    modelpoints=ModelPointFile("data/term_2026Q2.csv"),
    assumptions=AssumptionSet("assumptions/base.pir"),
)
result.to_parquet("out/")
result.explain("reserve", modelpoint="POL00042", t=3)
model.show()          # dependency-graph explorer
result.show()         # waterfalls, run-diff, drill-down
```

---

## 9. Assumption sets stay data

Assumption *declarations* are Python (§4); assumption *values* are `.pir` data files. This is
deliberate and non-negotiable: a scenario run varies values thousands of times, values are edited by
people who do not edit models, and a value change must be a one-line diff with no code review of
control flow. `AssumptionSet` objects can be constructed in Python for exploration —
`AssumptionSet("base").with_(valuation_rate=0.0325)` — and that produces a derived set whose delta
is recorded in the run manifest, so an ad-hoc sensitivity is still reproducible.

---

## 10. Error catalogue

DSL-originated diagnostics are `E1xxx` / `W1xxx`, distinct from the IR checker's `E0xxx` / `W0xxx`.
Every diagnostic carries `{code, severity, message, spans, suggestions, doc_url}` in `--json`, with
`suggestions` as literal byte-range replacements so an agent can apply and re-check mechanically.

| Code | Meaning |
|---|---|
| `E1101` | Missing return annotation (no dtype/unit) |
| `E1102` | `@series` without `timing` |
| `E1103` | Unknown parameter name |
| `E1104` | Parameter annotation contradicts the referenced declaration |
| `E1105` | Ambiguous name across namespaces |
| `E1106` | Table used but not in the signature (or vice versa) |
| `E1201` | Python `if` / `and` / `or` / `not` / truth-testing a traced value |
| `E1202` | Unlagged self-reference |
| `E1203` | Non-constant lag index |
| `E1204` | Forward time reference |
| `E1205` | Loop or comprehension over a traced value |
| `E1206` | Call to a non-builtin function on a traced value |
| `E1207` | Stage-2 (`Agg`-derived) value read in `expr` rather than `init` |
| `E1301` | Modelpoint schema has zero or multiple `key()` fields |
| `E1302` | Modelpoint object accessed as a value |
| `E1401` | Redefinition of a timeline input |
| `E1402` | Arithmetic basis conversion on a `Rate` |
| `E1501` | Shadowing an inherited component without `@override` |
| `E1502` | `@override` changes shape/dtype/unit/timing |
| `E1503` | Unimplemented `@abstract` component in a product |
| `E1504` | Override of a `@final` component |
| `E1601` | `product(outputs=...)` disagrees with the set of `output=True` components |
| `W1101` | Component declared but never read and not `output=True` |
| `W1102` | `Num` (unitless) in money arithmetic |
| `W1103` | Timing mismatch in a sum |

### 10.1 Worked messages

These are written to be actionable by an LLM with no other context: state what is wrong, why the
rule exists, and the exact replacement text.

```
error[E1201]: `if` cannot be used on a model value

   ┌─ models/term_assurance/cashflows.py:34:12
34 │     if in_term:
   │        ^^^^^^^ `in_term` is a Series component, not a Python bool

  A component body is traced once at build time, so there is no single value here to
  branch on — `in_term` is a different value for every modelpoint and every t.

  Use the value conditional `when(cond, then, otherwise)`, which compiles to the IR's
  `if ... then ... else ...` and evaluates per period:

34 │     return when(in_term, premium_rate * num_pols_if, 0.0)

  Note `when` evaluates both branches; it is not a guard. To avoid a division by zero,
  guard the denominator instead: `when(n > 0, x / max_(n, 1.0), 0.0)`.
```

```
error[E1103]: unknown name `num_pols_iff` in the parameter list

   ┌─ models/term_assurance/cashflows.py:28:26
28 │ def premium_income(premium_rate: Money, num_pols_iff: Count) -> Money:
   │                                         ^^^^^^^^^^^^ not a component, modelpoint
   │                                                      field, assumption, table, or
   │                                                      timeline input
  help: there is a component named `num_pols_if` (declared at decrements.py:41)

28 │ def premium_income(premium_rate: Money, num_pols_if: Count) -> Money:
   │                                         ~~~~~~~~~~~

  Names in scope from this module: num_pols_if, deaths, qx, wx, in_term, age
  Modelpoint fields: entry_age, gender, smoker, sum_assured, annual_premium, policy_term
```

```
error[E1104]: parameter annotation contradicts the declaration

   ┌─ models/term_assurance/cashflows.py:41:24
41 │ def death_claims(deaths: Money, sum_assured: Money) -> Money:
   │                          ^^^^^ you annotated `deaths` as money
   ┌─ models/term_assurance/decrements.py:58:1
58 │ def deaths(num_pols_if: Count, qx: Prob) -> Count:
   │ ------ but `deaths` is declared as count

  Parameter annotations are optional — but when present they are checked, so that a
  misunderstanding about what a component means fails at the signature rather than
  producing a plausible-looking wrong number.

  Either fix the annotation:
41 │ def death_claims(deaths: Count, sum_assured: Money) -> Money:

  or, if you meant a money-valued input, you are probably looking for `death_outgo`.
```

```
error[E1202]: `num_pols_if` reads itself with no time lag

   ┌─ models/term_assurance/decrements.py:47:12
47 │     return num_pols_if * (1 - qx)
   │            ^^^^^^^^^^^ this is num_pols_if[t], inside num_pols_if

  A component cannot depend on its own value in the same period — there is no order in
  which to compute it. Recursion across periods is fine and is how survivorship is
  written:

47 │     return num_pols_if[t-1] * (1 - qx[t-1])

  You will also need a starting value, since num_pols_if[-1] does not exist:

44 │ @series(timing=START, init=1.0)
```

```
error[E1402]: a rate on an annual basis cannot be divided to change basis

   ┌─ models/term_assurance/reserves.py:22:12
22 │     return valuation_rate / 12
   │            ^^^^^^^^^^^^^^^^^^^ valuation_rate is Rate.annual; the model timeline is monthly

  Dividing an annual rate by 12 gives a nominal rate, not the equivalent monthly
  effective rate, and understates discounting by roughly (1+i)^(1/12) - 1 - i/12.

  If you want the compounding conversion (almost always the case in a valuation):
22 │     return to_monthly(valuation_rate)

  If you genuinely want simple division — e.g. reproducing a Prophet variable that
  does `/12` and which you are matching to the penny — say so explicitly:
22 │     return nominal_to_periodic(valuation_rate, 12)

  The explicit form appears in the IR diff, so a reviewer sees the choice was made.
```

```
error[E1501]: `qx_loading` shadows an inherited component

   ┌─ products/term_2026/decrements.py:18:1
18 │ def qx_loading(policy_year: Years) -> Factor:
   │ ^^^^^^^^^^^^^^ redefined here
   ┌─ acme_lib/mortality.py:112:1  (acme-actuarial-lib 3.2.0)
112│ def qx_loading() -> Factor:
   │ ------------- inherited via extends(acme.mortality)

  Silent shadowing of a library component is not allowed: a reader of this product
  cannot tell whether the library value or the local one is in force.

  If you mean to replace it, say so — the decorator also checks that shape, dtype,
  unit and timing still match the library's contract:

17 │ @override(acme.qx_loading)
18 │ @series(timing=END)
19 │ def qx_loading(policy_year: Years) -> Factor:

  If you meant a different quantity, give it a different name (e.g. `qx_loading_select`)
  and reference the library one alongside it.
```

```
error[E1503]: product `term_2026` has unimplemented abstract components

   ┌─ products/term_2026/__init__.py:9:1
 9 │ product("term_2026", modules=[...], outputs=[...])
   │ ^^^^^^^ this product extends acme.base_life, which declares 2 abstract components

  Required, with the signature each override must match:

    surrender_value(policy_year: Years, sum_assured: Money) -> Money
        @series(timing=POINT)          acme_lib/base_life.py:88
    commission(premium_income: Money, policy_year: Years) -> Money
        @series(timing=START)          acme_lib/base_life.py:96

  Add them to a module in this product, decorated with @override(acme.<name>).
  If this product genuinely has no surrender value, override it with a zero and say
  why in the docstring — an explicit zero is auditable, a missing component is not.
```

```
warning[W1103]: adding flows with different timing

   ┌─ models/term_assurance/cashflows.py:73:12
73 │     return premium_income - death_claims
   │            ^^^^^^^^^^^^^^   ^^^^^^^^^^^^ timing=END
   │            timing=START

  Premiums are received at the start of period t, claims are paid at the end. Summing
  them treats them as simultaneous, which biases the discounted value by roughly one
  period of interest on the smaller leg.

  If the net flow is a mid-period approximation (the usual convention), retime both
  legs explicitly so the choice is visible in the diff:

73 │     return retime(premium_income, MID) - retime(death_claims, MID)

  If premiums really should be treated as end-of-period, use shift(premium_income, 1).
  Suppress with `# noqa: W1103` if the mismatch is intentional and documented.
```

---

## 11. Complete worked model — term assurance

The DSL source that compiles to `01-ir.md` §6. One `.pir` per Python module — `build/schema.pir`,
`build/decrements.pir`, `build/cashflows.pir`, `build/reserves.pir` — whose components, concatenated
in declaration order, are exactly the components shown there (the IR spec presents them merged into
one `model.pir` for readability).

### `models/term_assurance/schema.py`

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

valuation_rate     = assumption(Rate.annual)
premium_escalation = assumption(Rate.annual)
expense_inflation  = assumption(Rate.annual)
renewal_expense_pa = assumption(Money)
mortality_loading  = assumption(Factor)

sa8990 = table(
    source="tables/sa8990.csv",
    keys=[Key("age", int, policy="clamp"), Key("gender", Gender), Key("smoker", bool)],
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

### `models/term_assurance/decrements.py`

```python
from predictable import series, t
from predictable.fn import when
from predictable.timing import START, END
from predictable.units import Money, Years, Prob, Count, Factor, Flag
from .schema import Gender, sa8990, lapse_rates

@series(timing=START)
def age(entry_age: Years) -> Years:
    """Attained age at the start of projection year t."""
    return entry_age + t

@series(timing=START)
def in_term(policy_term: Years) -> Flag:
    """True while the policy is within its contractual term."""
    return t < policy_term

@series(timing=END)
def qx(age: Years, gender: Gender, smoker: bool,
       mortality_loading: Factor, sa8990=sa8990) -> Prob:
    """Annual mortality rate, table rate scaled by the valuation loading."""
    return sa8990(age, gender, smoker) * mortality_loading

@series(timing=END)
def wx(policy_year: Years, lapse_rates=lapse_rates) -> Prob:
    return lapse_rates(policy_year)

@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, wx: Prob, in_term: Flag) -> Count:
    """Survivorship. Self-referential with lag 1 — legal (see IR spec 3.1)."""
    return num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * when(in_term, 1.0, 0.0)

@series(timing=END)
def deaths(num_pols_if: Count, qx: Prob) -> Count:
    return num_pols_if * qx
```

### `models/term_assurance/cashflows.py`

```python
from predictable import series, t
from predictable.fn import when, compound, retime
from predictable.timing import START, END, MID
from predictable.units import Money, Count, Rate, Flag

@series(timing=START, init=lambda annual_premium: annual_premium)
def premium_rate(premium_rate: Money, premium_escalation: Rate.annual) -> Money:
    """Escalating premium, compounding annually from the issue premium."""
    return premium_rate[t-1] * (1 + premium_escalation)

@series(timing=START)
def premium_income(premium_rate: Money, num_pols_if: Count, in_term: Flag) -> Money:
    return premium_rate * num_pols_if * when(in_term, 1.0, 0.0)

@series(timing=END)
def death_claims(sum_assured: Money, deaths: Count) -> Money:
    return sum_assured * deaths

@series(timing=START)
def renewal_expenses(renewal_expense_pa: Money, expense_inflation: Rate.annual,
                     num_pols_if: Count, in_term: Flag) -> Money:
    return (renewal_expense_pa * compound(expense_inflation, t)
            * num_pols_if * when(in_term, 1.0, 0.0))

@series(timing=MID, output=True)
def net_cashflow(premium_income: Money, death_claims: Money,
                 renewal_expenses: Money) -> Money:
    """Insurer-positive net flow. Explicit retiming so the sign convention is auditable."""
    return (retime(premium_income, MID)
            - retime(death_claims, MID)
            - retime(renewal_expenses, MID))
```

### `models/term_assurance/reserves.py`

```python
from predictable import series, per_mp, t
from predictable.fn import npv
from predictable.timing import POINT
from predictable.units import Money, Factor, Rate

@series(timing=POINT, init=1.0)
def disc_factor(disc_factor: Factor, valuation_rate: Rate.annual) -> Factor:
    """v^t. Recursive rather than pow() so a term-structure rate is a one-line change."""
    return disc_factor[t-1] / (1 + valuation_rate)

@per_mp(output=True)
def pv_premiums(premium_income: Money, disc_factor: Factor) -> Money:
    return npv(premium_income, disc_factor)

@per_mp(output=True)
def pv_claims(death_claims: Money, disc_factor: Factor) -> Money:
    return npv(death_claims, disc_factor)

@per_mp(output=True)
def pv_expenses(renewal_expenses: Money, disc_factor: Factor) -> Money:
    return npv(renewal_expenses, disc_factor)

@per_mp(output=True)
def bel(pv_claims: Money, pv_expenses: Money, pv_premiums: Money) -> Money:
    """Best estimate liability, prospective net premium reserve at t = 0."""
    return pv_claims + pv_expenses - pv_premiums

@series(timing=POINT, init=bel, output=True)
def reserve(reserve: Money, premium_income: Money, death_claims: Money,
            renewal_expenses: Money, valuation_rate: Rate.annual) -> Money:
    """Retrospective roll-forward; the reconciliation to `bel` is a test, not a component."""
    return ((reserve[t-1] + premium_income[t-1] - death_claims[t-1]
             - renewal_expenses[t-1]) * (1 + valuation_rate))
```

### `models/term_assurance/__init__.py`

```python
from predictable import product

product(
    "term_assurance",
    modules=["models.term_assurance.schema",
             "models.term_assurance.decrements",
             "models.term_assurance.cashflows",
             "models.term_assurance.reserves"],
    outputs=["net_cashflow", "bel", "reserve", "pv_premiums", "pv_claims", "pv_expenses"],
)
```

`outputs=` is a cross-check, not a second source of truth: it must list exactly the components
carrying `output=True`, and a mismatch is `E1601`. It exists so the results schema is stated in one
readable place for a reviewer, and so an LLM can be told "produce these outputs" and be checked.

Note `init=bel` on `reserve`: `bel` is a stage-2 `Agg`-derived `PerMP`, referenced from `init` only.
The DSL enforces the IR's rule (§8.2) — a stage-2 value in `expr` is `E1207` with the message
"`bel` is computed after the projection completes; it can seed `init` but cannot be read at time t".

---

## 12. Prophet concept mapping

| Prophet | predictable DSL | Notes |
|---|---|---|
| **Variable** (`.VAR` entry with type, form, dimensions) | A `@series` / `@per_mp` / `@scalar` component | Prophet's *form* (`t`-dependent vs constant) maps to `shape`; Prophet's *type* to `dtype` + `unit`. Prophet has no timing tag, so migration must decide `START`/`END`/`MID` per variable — the migration tool proposes one from the variable's discounting usage and marks it `# TODO(timing)` for confirmation. |
| **Indicator variable** (`IND_*`, integer 0/1 used multiplicatively) | A `Flag` component (`dtype=bool`), used via `when(flag, x, 0.0)` | Prophet's `PREM_IND * PREM` becomes `when(prem_ind, prem, 0.0)`. The migration tool rewrites `IND * x` → `when(ind, x, 0.0)` and lifts the indicator's dtype from `i64` to `bool`, which makes `IND1 * IND2` become `all_(ind1, ind2)` and makes `IND1 + IND2 > 0` a type error instead of a silent 2. |
| **Extended formula** (multi-line Prophet code with `IF/ELSE`, local temporaries, `RETURN`) | One component body: local Python variables for temporaries (inlined, §2.4), `when()` for branches | Prophet's sequential `IF` cascade becomes nested `when()`; the tracer flattens local temporaries into the single `expr` string, so a 20-line extended formula becomes one IR component with one expression and one `explain()` tree. Prophet extended formulas that *assign to other variables* are not expressible — that is a side effect, and the migration tool splits it into separate components, reporting each split. |
| **Master product / product variant** (`master` library with product-specific overrides) | `Library` package + `extends()` + `@override` + `@abstract` (§7.2) | Prophet's master/variant relationship is by convention and file layout; here it is checked. Prophet's "variable exists in the variant, overrides master" becomes `@override`, and unimplemented product hooks that Prophet leaves as zero-valued stubs become `@abstract` — so a product cannot be built with a hook silently unfilled. |
| **MPF column** (`!1` header line, positional, typed by the `.VAR` `MP_Field` declarations) | A field on a `ModelPoint` subclass (§3), read by naming it as a parameter | Prophet resolves MPF columns positionally against the variable list and fails at runtime on a mismatch; here the schema is in the IR, the CSV/MPF is validated against it before projection, and an unused column is a lint while a missing required one names the file, the column, and every component that reads it. `predictable import-mpf` generates the `ModelPoint` class from an existing `.MPF` header. |

Two further mappings worth stating because they are the ones that go wrong:

- **Prophet `.fac` table** → `table(...)` with explicit `Key` policies. Prophet's implicit
  out-of-range behaviour (clamp on some table types, zero on others) must be chosen explicitly, and
  the migration tool sets `policy` from the `.fac` type and records the choice in the diff.
- **Prophet run settings / parameter files** → assumption sets (§9). Anything Prophet keeps in a run
  setting that actually changes the *formula* (a switch that selects a calculation basis) becomes an
  assumption plus `when()`, not a build-time flag — because a build-time flag would mean two
  different models produce results labelled with one model digest.

---

## 13. Non-goals for DSL 1.0

- **No runtime in Python.** No pandas, no numpy, no Python-side projection loop, not even as a
  reference implementation — a second implementation is a second set of answers.
- **No dynamic model construction from data.** Components are declared by source code; a model whose
  graph depends on the modelpoint file cannot be diffed or explained.
- **No user-defined functions crossing into the IR** (IR §12). Python helpers that *generate*
  declarations are fine; helpers called on traced values are `E1206`.
- **No decorator for `Aggregate`.** Aggregation over `t` is `npv`/`sum_`/`last` inside a `@per_mp`
  body; aggregation over modelpoints is a run-config `[[aggregation]]` block, not DSL (IR §8.3).
- **No notebook-first authoring API.** `Model.build` and `result.show()` work in a notebook, but the
  unit of authorship is a file in git.

---

## IR feedback

Things `01-ir.md` does not currently specify that the DSL layer needs. None of these were worked
around silently; each is a request for an IR decision.

**Settled in IR 1.0** (raised here, now normative in `01-ir.md` — kept for the audit trail):

- `nominal_to_periodic(r, n)` added to the `rates` builtins (IR §2.8).
- `all_(a, b, c)` lowers to infix `a and b and c`; there is no variadic logic node (IR §2.8).
- `meta.overrides`, `meta.generated_from`, `meta.origin_span`, `meta.id`, `meta.display` are
  structured fields on `[component.meta]` (IR §11.1). `origin_span` is a hard requirement: every
  diagnostic must render against the authoring language when it is present.
- Enums are unordered; `clamp`/`step`/`interpolate` on an enum key is `E0305` (IR §2.9).

**Also settled (IR decision log Q1–Q15, `01-ir.md` §13):**

1. **`@abstract` is DSL-only** (Q10). There is no `Kind::Abstract`; an undischarged abstract fails
   `predictable build` with `E1203` and no `.pir` is written, so the resolve pass never sees a hole
   (IR §2.2). The assumption this document was written under is now normative.
2. **`Output` wins** (Q2). The component-level `Output` flag is the single source of emission truth;
   the product's `outputs` list is a checked manifest (`E0107`), not a selector, and the DSL's
   `outputs=` restatement (`E1601`) is the same check one layer up (IR §2.2, §8.4.1).
3. **`product` has an IR block** (Q2). `[product]` carries `name`, `modules`, `outputs`, `key_field`,
   default `assumptions` and `doc`, in its own `.pir` file covered by `model_digest`; run-level
   settings live in a separate `[run]` file (IR §8.4.1, §8.4.2). "Which modules and which outputs
   constitute this run" is now expressible without Python, so deleting the DSL still changes no
   number.

**Still open:** nothing. All questions raised by this layer are closed in `01-ir.md` §13.
