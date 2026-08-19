# The DSL tracer

The v2 Python package (`packages/predictable/`) is an **authoring layer, not a runtime**. Its job
is to turn a readable Python function into an IR expression tree — the `expr = "..."` of a
[`.pir` component](pir-syntax.md) — and to produce every diagnostic it possibly can while doing
so. Nothing here computes a number: once `predictable build` has written the `.pir`, the Python is
out of the loop.

The tracer is the piece that makes that work. A component body executes **exactly once**, at build
time, with symbolic proxies in place of its parameters. Operators on those proxies do not compute;
they build IR nodes.

```python
from predictable import Param, t, trace
from predictable.fn import when

def net_death_strain(sum_assured, reserve, deaths):
    strain_per_death = sum_assured - reserve[t - 1]
    return strain_per_death * deaths

result = trace(net_death_strain, [Param("sum_assured"), Param("reserve"), Param("deaths")])
result.pir            # '(sum_assured - reserve[t-1]) * deaths'
result.dependencies   # ('sum_assured', 'reserve', 'deaths')
result.stage2         # False
```

Three things are worth noticing in that example, because they are the three design decisions the
tracer exists to enforce.

**Local variables inline.** `strain_per_death` is a Python name bound to a subtree, so using it
splices the subtree in. It does not become a component, and the IR graph stays flat by
construction ([IR §12](../design/01-ir.md)). If you want the intermediate in your results, make it a
component — that is a deliberate act, not an accident of how you wrote the formula.

**Dependencies are the parameter list.** `result.dependencies` is read back off the expression,
and it is exactly the set the engine's checker will rebuild from the emitted `.pir`. A mismatch
between them is a bug in one of the two, which is why the tracer reports it at all.

**The text is already canonical.** `result.pir` is what `predictable fmt` writes — same
precedence rules, same spacing, same shortest-round-trip floats. The test suite feeds every
expression the tracer produces through the real `predictable fmt` binary and requires it to come
back byte-identical, so the two implementations of that rule cannot drift.

---

## The pieces

| Module | What it is |
| --- | --- |
| `predictable.ir` | The IR expression tree as Python dataclasses, mirroring `predictable-ir`'s serde model field for field. `to_json()` is what serde emits; `to_pir()` is what `fmt` writes. |
| `predictable.proxy` | `ExprProxy` — the symbolic value a body sees — plus `t`, `TableProxy`, and every refusal. |
| `predictable.fn` | The closed builtin set of [IR §2.8](../design/01-ir.md), one Python callable per IR node. |
| `predictable.tracer` | `trace()` / `trace_init()`, `Param`, and the dependency + stage-2 analysis. |
| `predictable.errors` | The `E12xx` catalogue, with worked messages. |
| `predictable.diagnostics` | `{code, severity, message, spans, suggestions, doc_url}` and the terminal renderer. |

The declaration layer — `@series`, `@per_mp`, `@scalar`, `ModelPoint`, `table`, `timeline`,
`product` — sits on top of this and is task T20. Everything below is usable today.

---

## Time: `t`, lags, and absolute indices

`t` is imported and used as an index *and* as a value.

```python
from predictable import t

def age(entry_age):
    return entry_age + t                  # 'entry_age + t'

def survivorship(num_pols_if, qx):
    return num_pols_if[t - 1] * (1 - qx[t - 1])
    # 'num_pols_if[t-1] * (1 - qx[t-1])'

def issue_premium(premium_rate):
    return premium_rate[0]                # 'premium_rate[0]' — an absolute index
```

`x[t-k]` needs an integer `k >= 1`. A plain Python `int` is fine and is constant-folded:
`LAG = 3; x[t - LAG]` traces to `x[t-3]`. A *traced* offset is `E1203` — the planner sizes the
retention ring from `k` before any data is read, and a data-dependent lag has no ring size.

Forward references are `E1204`. A projection is a single forward pass; the legal backward channel
is an aggregate over the completed series, read from `init`.

---

## Conditionals

Python's `if` cannot work on a traced value: the tracer would have to pick a branch, and there is
no single value to branch on. So traced values refuse `__bool__`, and the conditional is a
function:

```python
from predictable.fn import when, all_, any_, not_

def premium_income(premium_rate, num_pols_if, in_term):
    return premium_rate * num_pols_if * when(in_term, 1.0, 0.0)
    # 'premium_rate * num_pols_if * (if in_term then 1.0 else 0.0)'
```

`when(cond, then, otherwise)` is the IR's `If`. It evaluates **both** branches — it is not a
guard. Traps in the arm that is not taken are suppressed, which is what makes the standard
defensive idiom legal:

```python
when(n > 0, x / max_(n, 1.0), 0.0)
```

`and` / `or` / `not` are `__bool__`-based in Python and cannot be overloaded, so boolean
combination is `all_(a, b)` / `any_(a, b)` / `not_(a)`. They lower to *infix* IR:
`all_(a, b, c)` becomes `a and b and c`. There is no variadic logic node.

---

## Tables

A table is bound into a body through the default-argument idiom, which keeps the
"dependencies are parameters" rule intact for tables too:

```python
from predictable.proxy import TableProxy

sa8990 = TableProxy("sa8990", ("qx",))

def qx(age, gender, smoker, mortality_loading, sa8990=sa8990):
    return sa8990(age, gender, smoker) * mortality_loading
    # 'sa8990@(age, gender, smoker) * mortality_loading'
```

A single-value table allows the bare form. A multi-value table names its column:
`rates(age).wx` traces to `rates.wx@(age)`.

---

## Stage 2

A body that contains an aggregate (`npv`, `sum_`, `last`, `max_over`, `count_while`, …) reduces
the whole projection, so it runs in stage 2, after every period is known
([IR §8.2](../design/01-ir.md)).

```python
from predictable.fn import npv

def pv_premiums(premium_income, disc_factor):
    return npv(premium_income, disc_factor)

trace(pv_premiums, [Param("premium_income"), Param("disc_factor")]).stage2   # True
```

Reading a stage-2 value inside `expr` would make period 0 depend on period T, so it is `E1207`.
Seeding a recursion with it is legal, and is the single backward channel the IR allows — which is
why `init` is traced in its own root:

```python
from predictable import trace_init

trace_init(lambda bel: bel, [Param("bel", shape="PerMP", stage2=True)]).pir   # 'bel'
```

---

## What the tracer refuses, and what it says

Every refusal is a diagnostic with a code, a span pointing at **your** line, and a fix. The rule
the messages are written to is: *state what is wrong, why the rule exists, and the exact
replacement text.*

```python
def premium_income(premium_rate, in_term):
    if in_term:                      # ← E1201
        return premium_rate
    return 0.0
```

```
error[E1201]: `if` cannot be used on a model value

   ┌─ models/term_assurance/cashflows.py:34:8
 34 │     if in_term:
    │        ^^^^^^^ `in_term` is a Series component, not a Python bool

  A component body is traced once at build time, so there is no single value here to
  branch on — `in_term` is a different value for every modelpoint and every t.

  help: Use the value conditional `when(cond, then, otherwise)`, which compiles to the IR's
  `if ... then ... else ...` and evaluates per period:

      return when(in_term, <then>, <otherwise>)

  help: Note `when` evaluates both branches; it is not a guard. To avoid a
  division by zero, guard the denominator instead:
      when(n > 0, x / max_(n, 1.0), 0.0)

  see https://predictable.dev/llm/diagnostics/#E1201
```

The full catalogue the tracer can raise:

| Code | Fires when | Names as the fix |
| --- | --- | --- |
| `E1201` | `if` / `and` / `or` / `not` / `bool()` on a traced value; a body that returns nothing | `when(...)`, `all_`, `any_`, `not_` |
| `E1202` | A component reads itself with no lag | `x[t-1]` plus an `init=` |
| `E1203` | The lag or index is not a build-time constant | a `when()` over two fixed lags |
| `E1204` | `x[t+1]`, `x[T]` — reading the future | an aggregate read from `init` |
| `E1205` | Iterating, `len()`, membership, hashing, returning a container | `sum_`, `npv`, `last`, or an index |
| `E1206` | Calling a non-builtin, attribute access, `numpy`, `math`, `float()` | the builtin that replaces it |
| `E1207` | A stage-2 value read in `expr` | move the reference into `init=` |
| `E1302` | A modelpoint object used as a value | add the field to the parameter list |
| `E1402` | `annual_rate / 12` | `to_monthly(r)`, or the explicit `nominal_to_periodic(r, 12)` |

Each one is machine-readable as well as readable:

```python
from predictable import DslError

try:
    trace(premium_income, [Param("premium_rate"), Param("in_term")])
except DslError as err:
    err.code                       # 'E1201'
    err.diagnostic.to_json()       # {'code': ..., 'severity': 'error', 'spans': [...], ...}
```

The JSON is the same shape `predictable-diagnostics` serialises, so DSL diagnostics and engine
diagnostics merge into one `--json` stream without translation, and `suggestions[].edits` are
literal byte-range replacements an agent can apply and re-check mechanically.

`E1402` is worth calling out, because it is the single most common Prophet migration error:

```
error[E1402]: a rate on an annual basis cannot be divided to change basis

  Dividing an annual rate by 12 gives a nominal rate, not the equivalent effective rate
  for the shorter period, and understates discounting by roughly (1+i)^(1/12) - 1 - i/12.

  help: If you want the compounding conversion (almost always the case in a valuation):

      to_monthly(valuation_rate)

  help: If you genuinely want simple division — e.g. reproducing a Prophet variable
  that does `/12` and which you are matching to the penny — say so explicitly:

      nominal_to_periodic(valuation_rate, 12)

  The explicit form appears in the IR diff, so a reviewer sees the choice was made.
```

---

## Worked example: the term assurance model

The bodies below are the ones in [the DSL spec](../design/02-dsl.md) §11, without their decorators
(those are T20). Each traces to the `expr` string of the same component in
[the IR spec](../design/01-ir.md) §6 — that equality is a test, not a claim.

```python
from predictable import Param, t, trace
from predictable.fn import compound, npv, retime, when
from predictable.timing import MID

def in_term(policy_term):
    return t < policy_term
# 't < policy_term'

def num_pols_if(num_pols_if, qx, wx, in_term):
    return num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * when(in_term, 1.0, 0.0)
# 'num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1]) * (if in_term then 1.0 else 0.0)'

def renewal_expenses(renewal_expense_pa, expense_inflation, num_pols_if, in_term):
    return (renewal_expense_pa * compound(expense_inflation, t)
            * num_pols_if * when(in_term, 1.0, 0.0))
# 'renewal_expense_pa * compound(expense_inflation, t) * num_pols_if
#  * (if in_term then 1.0 else 0.0)'

def net_cashflow(premium_income, death_claims, renewal_expenses):
    return (retime(premium_income, MID)
            - retime(death_claims, MID)
            - retime(renewal_expenses, MID))
# 'retime(premium_income, mid) - retime(death_claims, mid) - retime(renewal_expenses, mid)'

def reserve(reserve, premium_income, death_claims, renewal_expenses, valuation_rate):
    return ((reserve[t-1] + premium_income[t-1] - death_claims[t-1]
             - renewal_expenses[t-1]) * (1 + valuation_rate))
# '(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1])
#  * (1 + valuation_rate)'
```

---

## Running the tests

```console
$ cd packages/predictable
$ uv venv && uv pip install -e '.[dev]'
$ pytest -q
171 passed
```

The suite is a specification test rather than a snapshot: the worked model's Python comes from the
DSL spec, its expected text comes from the IR spec, the builtin set is compared against
`crates/predictable-ir/src/builtins.rs`, the diagnostic codes against the registry in
`crates/predictable-diagnostics`, and the canonical text against the `predictable fmt` binary
itself (that last test skips if `cargo build -p predictable-cli` has not been run).
