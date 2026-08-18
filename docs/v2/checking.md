# Checking a model

`predictable-check` is the part of the toolchain that decides whether a `.pir` model is well formed,
and — when it is not — says so in a way a human can act on and an agent can apply mechanically. It
implements the six passes of [`01-ir.md` §7](../design/01-ir.md), plus a seventh that only runs when
a run configuration is present.

Everything the checker knows, it knows **before a single modelpoint is read**. That is the point:
the IR is designed so that every error which can be raised at definition time *is* raised at
definition time, with a span, a message and a fix.

---

## 1. The passes

| Pass | Question it answers | Codes |
|---|---|---|
| 1 · parse | What does the file *say*? (owned by `predictable-syntax`) | `E00xx` |
| 2 · resolve | Does every name bind, and bind to exactly one thing? | `E0203`, `E0204`, `E0205` |
| 3 · cycles | Can the components be put in an order? | `E0201`, `E0202` |
| 4 · shapes | Is every value the shape its component promises? | `E0301` |
| 5 · types | Do the dtypes, units, timing tags and lookups agree? | `E0501`–`E0509`, `E0602`, `E0801`, `E1402` |
| 6 · lints | What is legal but probably not what you meant? | `W0101`–`W0105` |
| 7 · run | Do the run file's ids resolve, and are its aggregations well shaped? | `E0106`, `E0402`–`E0404` |

Three rules shape the output more than any individual check:

**Every diagnostic carries a suggested edit.** Not prose describing a fix: a literal byte-range
replacement. An agent in the migration loop applies it and re-checks without a human in the middle,
and a conformance test asserts the property over every case in the corpus.

**Lints run only when passes 1–5 are clean.** A broken model produces misleading lints — an
unresolved name looks unused, a mistyped component looks constant — and a wall of warnings beneath a
real error is how a toolchain teaches people to ignore warnings.

**One mistake is one diagnostic.** An unresolved name binds to nothing and the later passes treat it
as unknown rather than reporting it again as a unit error and a shape error. Likewise, at most one
lint fires per component: when a component is both unread *and* mis-united, the specific problem is
the one worth saying.

---

## 2. Using it

```rust
use predictable_check::{check, Input};

let result = check(&[
    Input::new("schema.pir", schema_text),
    Input::new("model.pir", model_text),
    Input::new("product.pir", product_text),   // optional: turns on the §8.4.1 manifest checks
    Input::new("run.pir", run_text),           // optional: turns on pass 7
]);

if !result.is_ok() {
    print!(
        "{}",
        predictable_diagnostics::render_all(
            &result.diagnostics,
            &result.sources,
            &Default::default(),
        )
    );
}
std::process::exit(result.exit_code());   // 0 clean, 1 lints only, 2 errors
```

Files are classified by their **content**, never by their name: a file with a `[product]` block is a
product, a file with `[run]` is a run configuration, and a file with `module = "..."` is a model
module. A model checks perfectly well on its own — pass 7 simply does not run — which is what keeps a
module library checkable without inventing a run for it.

For the machine-readable form, `predictable_diagnostics::to_json(&result.diagnostics)` emits the
`{code, severity, message, spans, suggestions, doc_url}` array that `--json` will carry.

---

## 3. Worked examples

Every example below is a case in the [conformance corpus](conformance-corpus.md), rendered by the
checker itself.

### A misspelled name (`E0203`)

The suggestion pool is restricted to the namespace of the *use site*: an identifier resolves against
components, modelpoint fields, assumptions and timeline inputs; `tbl@(k)` resolves against tables
alone. A misspelled table is never "corrected" into a component, and no suggestion is ever a builtin.

```text
error[E0203]: cannot find `valuation_rat` in this model
  --> model.pir:35:18
   |
35 | expr = "v_from_i(valuation_rat)"
   |                  ^^^^^^^^^^^^^ nothing in the model, the modelpoint schema or the assumption set is named this
   |
   = note: `valuation_rat` is resolved against components, modelpoint fields, assumptions and timeline inputs (01-ir.md §7 pass 2).
help: there is an assumption named `valuation_rate`
   |
35 | expr = "v_from_i(valuation_rate)"
   |                               +
```

### A cycle in the same period (`E0201`)

`G₀` — the edges with no time lag — must be acyclic. Lagged edges are unconstrained, so
`reserve[t] = reserve[t-1] * (1 + i)` is a legal self-loop; two components that read each other *at
time t* are not.

```text
error[E0201]: cyclic dependency in the same period
  --> model.pir:30:9
   |
30 | expr = "bel * 1.05"
   |         ^^^ `reserve` reads `bel` at time t
...
39 | expr = "reserve + sum_assured"
   |         ------- `bel` reads `reserve` at time t
   |
   = note: `bel` depends on `reserve`, which depends on `bel` — with no time lag between them, so neither can be computed first.
   = note: Cycles across periods are fine. If this really is simultaneous, it cannot be expressed in a projection IR — solve it outside the loop and feed the result in as an assumption.
help: read last period's `bel` instead
   |
30 | expr = "bel[t-1] * 1.05"
   |            +++++
help: or seed the recursion with an `init`
   |
24 ~ name = "reserve"
25 ~ init = "<seed value at t = 0>"
   |
```

`x[k]` is the subtlety. An `At` edge is instantaneous *at the single period `t = k`*, so `x[0]`
inside `x` is a seed and is legal, while `x[5]` inside `x` is a cycle — and the message names the
period:

```text
   |         ^^^^^^^^^^^^^^^ `illegal_seed` reads `illegal_seed` at time t = 5
```

A cycle that runs through `init` is `E0202` instead. `init` may read a stage-2 aggregate — that is
the one legal backward channel in the IR (§8.2) — but the chain of such reads must terminate.

### Narrowing without an aggregate (`E0301`)

Widening is implicit and free. Narrowing never is: going from `Series` to `PerMP` means choosing a
reduction, and the checker will not choose for you.

```text
error[E0301]: cannot narrow `Series` to `PerMP` without an aggregate
  --> model.pir:38:9
   |
38 | expr = "flow * 2.0"
   |         ^^^^^^^^^^ this expression is a `Series`, but `total` is declared `PerMP`
   |
   = note: Widening is implicit and free; narrowing never is (01-ir.md §2.3). Choose the reduction you mean: `sum`, `npv`, `last`, `first`, `at(x, k)`, `max_over`, `min_over` or `count_while`.
help: reduce the series with `sum(...)`
   |
38 - expr = "flow * 2.0"
38 + expr = "sum(flow * 2.0)"
   |
help: or declare `total` a `Series`
   |
36 - shape = "PerMP"
36 + shape = "Series"
   |
```

Both fixes are offered because both are plausible and the checker cannot tell which you meant; each
is a literal edit, and each is marked `maybe_incorrect` in the JSON so an agent knows to show them
rather than pick one.

### A basis conversion written as arithmetic (`E1402`)

Prophet models are full of `r / 12`. It is almost never what the author meant, and the message names
both the compounding conversion and the sanctioned escape hatch:

```text
error[E1402]: dividing a `rate(annual)` by 12 is not a basis conversion
  --> model.pir:29:9
   |
29 | expr = "valuation_rate / 12"
   |         ^^^^^^^^^^^^^^^^^^^ annual to periodic compounds; it does not divide
   |
   = note: `to_monthly(r)` is (1 + r)^(1/12) - 1. If simple division really is what you want, say so with `nominal_to_periodic(r, n)`, which is named so the choice shows up in the diff (01-ir.md §2.8).
help: compound the annual rate down to the model's basis
   |
29 - expr = "valuation_rate / 12"
29 + expr = "to_monthly(valuation_rate)"
   |
help: keep the simple division, named
   |
29 - expr = "valuation_rate / 12"
29 + expr = "nominal_to_periodic(valuation_rate, 12)"
   |
```

### Units

Unit checking is shallow on purpose — it catches the mistakes actuaries actually make, and stays out
of the way otherwise:

| Expression | Result |
|---|---|
| `money + prob` | `E0501` |
| `rate(annual) + rate(monthly)` | `E0502` |
| `money * money` | `E0503` |
| `money * prob` | `money` |
| `money / money` | `factor` |
| `1 - prob` | `prob` — literals are unit-polymorphic and unify with their context |

Two deliberate non-rules. **A literal never clashes**: `sum_assured + 100.0` is `money`, not an
error. And **the declaration is not checked against the expression**: `unit` on a component says what
the value *is*, while §2.4's rules say what arithmetic *does*, and conflating them would reject
`floor(money / money)` declared `factor` — which the spec's own worked example writes.

### Lints

```text
warning[W0102]: money arithmetic with unit = "none"
  --> model.pir:44:9
   |
44 | expr = "sum_assured + 100.0"
   |         ^^^^^^^^^^^^^^^^^^^ this expression is `money`, but `untagged_money` is declared `none`
   |
   = note: `unit = "none"` opts out of dimensional checking, so nothing downstream will catch a money value added to a probability (01-ir.md §2.4).
help: declare the unit the arithmetic already implies
   |
43 - unit = "none"
43 + unit = "money"
   |
```

| Lint | Fires when |
|---|---|
| `W0101` | a component is never read and is not an `Output` |
| `W0102` | money-shaped arithmetic is declared `unit = "none"` |
| `W0103` | a `start` flow and an `end` flow are added in the same period |
| `W0104` | a `Series` depends on neither a modelpoint value, nor `t`, nor a lag |
| `W0105` | `retime` / `shift` / `cum` / `diff` is applied to an untimed value |

`W0103` deliberately looks only at *this* period's flows: `premium[t-1] - claims[t-1]` reads a period
that has closed, where the timing question does not arise, and flagging it would make the lint
useless on every roll-forward reserve in the world.

---

## 4. Lowering: `Call` → `Agg`

The expression grammar (§4.2) has no aggregate form. `npv(x, disc)` and `sum(flow)` parse as ordinary
calls, because syntactically that is all they are. The IR *does* have an `Agg` node, because
semantically an aggregate is not a call: it reduces a completed series to a `PerMP`, and that is
precisely what makes its component **stage 2** (§2.2, §8.2). Deciding which calls are reductions is
a checker's job, so the rewrite lives here:

```rust
use predictable_check::lower;

let ir = lower::expr(&document.arena, component.expr.unwrap()).unwrap();
// npv(flow, disc)  ->  Expr::Agg { op: Npv, value: Ref("flow"), pred: Some(Ref("disc")) }

let module = lower::module(&document);
assert_eq!(module.component("pv").unwrap().stage(), predictable_ir::Stage::Two);
```

Two spellings collapse in the same step: `at(x, k)` and `x[k]` are the same operation (§2.8) and both
lower to `Expr::At`. An `Agg` has one optional second operand — the predicate of `sum(x, cond)`, or
the discount-factor series of `npv(x, disc)` — reached by the `pred` segment of an `ExprPath`.

An expression containing a parse-error node has **no** IR form: `lower::expr` returns `None` rather
than inventing a node, because inventing one would hide the diagnostic that produced it.

---

## 5. What the corpus pins

The checker is tested against the [conformance corpus](conformance-corpus.md), which was written from
the IR spec *before* this crate existed. The comparison is exact set equality per case — code, file,
line, column and message substring — so an unexpected extra diagnostic fails the case just as a
missing one does. That is what stops a checker from being noisy.

Three properties are asserted over the whole corpus at once:

1. every diagnostic the checker emits has a primary span;
2. every diagnostic carries at least one suggested edit;
3. every suggested edit applies cleanly to the file it names.

When the checker and the corpus disagree, the default assumption is that the checker is wrong.
Changing an `expected.diag` file is a review-visible act and needs a spec citation.

---

## 6. Diagnostic codes this crate owns

The five checker classes the corpus originally left as family wildcards were numbered when this crate
landed. They are registered like every other code and documented in the
[diagnostics catalogue](../llm/diagnostics.md):

| Code | Meaning |
|---|---|
| `E0203` | cannot find a name in this model |
| `E0204` | the same name is defined in two modules |
| `E0205` | absolute period index beyond the projection horizon |
| `E0501` | incompatible units in an addition |
| `E0502` | rates on different bases cannot be added |
| `E0503` | `money * money` is not a unit |
| `E0504` | wrong number of lookup keys |
| `E0505` | lookup key dtype does not match the table |
| `E0506` | `timing` on a component that is not a `Series` |
| `E0507` | a `Series` component with no `timing` |
| `E0508` | unknown timing tag |
| `E0509` | a `Lag` on a dtype with no zero and no `init` |
