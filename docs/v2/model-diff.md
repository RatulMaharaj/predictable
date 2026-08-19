# Diffing two models

`predictable diff model A B` compares two models **structurally**. It parses both
sides into the IR and compares the expression trees, so what it reports is
semantic change: a formula that moved, a timing that flipped, a component that
was renamed — and, crucially, everything downstream of each of those.

It is the answer to the question a text diff cannot answer: *given this change,
what can move?*

Normative source: [`01-ir.md` §11.3](../design/01-ir.md).

## What it will not tell you

Three non-changes, by construction:

- **Reformatting.** The comparison happens after parsing, so whitespace,
  redundant parentheses and `1.050` written for `1.05` are gone before anything
  is compared. Running `predictable fmt` over a model never shows up here.
- **A rename.** `premium_income → premiums`, applied consistently, is one
  `renamed` entry and nothing else — not a formula change in each of the
  fourteen components that referenced it.
- **A comment.** A change to `doc` or `tags` alone is classified `doc-only` and
  seeds no impact, because it cannot move a number.

## A worked example

Take `models/term_annual`, copy it, and make three edits of the kind that
actually happen in a valuation cycle:

1. hard-code the mortality loading in `qx`: `mortality@(age, sex, smoker) *
   mortality_loading` → `… * 1.1`;
2. move one rate in `tables/mortality.csv`;
3. move `valuation_rate` from `0.035` to `0.04` in the assumption set.

```console
$ predictable diff model ./a ./b
./a → ./b
  * qx [formula]
      expr.rhs subtree replaced: mortality_loading → 1.1
  T changed table mortality
      rows: +0 -0 ~1
        [18, F, false] 0.00017432 → 0.00019000
  A changed assumption valuation_rate: 0.035 → 0.04

3 change(s) affect 14 downstream component(s); 12 output(s) affected: bel,
death_claims, deaths, net_cashflow, premium_income, profit_margin, pv_claims,
pv_expenses, pv_premiums, renewal_expenses, reserve, surrenders
```

Every line carries a location. `expr.rhs` is an
[`ExprPath`](ir-concepts.md) (§3.0.1): a path from the component's `expr` root
to the node that moved. Unlike a byte span it survives reformatting, so the
anchor is still valid after `predictable fmt`.

## The JSON document

`--json` writes one `pvf/1` document (and `--out-json <path>` writes it to a
file). Abridged, from the run above:

```json
{
  "format": "pvf/1",
  "kind": "modeldiff",
  "changed": [
    {
      "name": "qx",
      "classes": ["formula"],
      "expr_nodes": [
        {
          "path": "expr.rhs",
          "change": "replaced",
          "before": "mortality_loading",
          "after": "1.1"
        }
      ]
    }
  ],
  "tables": [
    {
      "name": "mortality",
      "status": "changed",
      "rows": {
        "added": [],
        "removed": [],
        "changed": [
          { "key": ["18", "F", "false"],
            "before": ["0.00017432"], "after": ["0.00019000"] }
        ]
      }
    }
  ],
  "assumptions": [
    { "name": "valuation_rate", "status": "changed",
      "value_before": "0.035", "value_after": "0.04" }
  ],
  "impact": {
    "seeds": ["mortality", "qx", "valuation_rate"],
    "impacted": ["bel", "death_claims", "deaths", "..."],
    "outputs_affected": ["bel", "death_claims", "..."]
  },
  "summary": {
    "added": 0, "removed": 0, "renamed": 0, "changed": 1,
    "doc_only": 0, "tables": 1, "assumptions": 1,
    "components_impacted": 14, "outputs_affected": 12
  }
}
```

The top-level keys are `added`, `removed`, `renamed`, `changed`, `tables`,
`assumptions`, `impact` and `summary`. `summary` is what an agent branches on;
everything in it is derived from the arrays beside it, never separately.

## Change classification

A changed component carries every class that applies (§11.3):

| Class | Meaning |
|---|---|
| `formula` | the `expr` tree differs |
| `init` | the `init` tree differs — a recursion's seed |
| `timing` | `start`/`end`/`mid`/`point` changed |
| `unit` | the declared unit changed |
| `dtype` | the declared dtype changed |
| `shape` | `Scalar`/`PerMP`/`Series` changed |
| `kind` | e.g. `Output` became `Derived`, so the model stopped emitting it |
| `doc-only` | only documentation moved; never reported alongside another class |

`kind` is an addition to §11.3's seven names. The IR's `kind` field decides
emission (Q2), and a component that silently stopped being an `Output` would
otherwise be reported as no change at all.

## AST-level node changes

Within `expr` and `init`, each difference is anchored and typed rather than
being rendered as replacement text:

| `change` | Fires when |
|---|---|
| `operator` | same operands, different operator (`a * b` → `a / b`) |
| `offset` | same component, different time reference (`r` → `r[t-1]`, `r[t-1]` → `r[t-2]`) |
| `reference` | the node now names a different component |
| `literal` | a constant or its dtype moved |
| `function` | a different builtin, same arguments |
| `table` | a lookup retargeted to a different table |
| `aggregate` | a different reduction over the same series (`sum` → `npv`) |
| `predicate` | an aggregate gained or lost its predicate |
| `arity` | the argument or key count changed |
| `replaced` | the subtree is structurally different |

The reason for the split is the discounting bug. `v ^ t` becoming `v ^ (t + 1)`
is a two-character textual edit that a line diff buries in a 120-character
formula. Here it is one `replaced` at `expr.rhs` with `before: "t"` and
`after: "t + 1"`, and nothing else is reported — the rest of the formula is
provably untouched.

## Rename detection

A rename is detected in two ways, in this order:

1. **`meta.id`**. The DSL writes an opaque stable identity that is excluded from
   `model_digest` precisely so a rename is a fact rather than a guess (§11.1).
   Evidence is reported as `meta.id`.
2. **Identical normalised `expr`, `unit` and `shape`** among the components that
   are otherwise unmatched. Evidence is `expr+unit`.

If the match is ambiguous on either side — two removed components with the same
normalised expression, two added ones — no rename is claimed and the components
are reported as an add plus a remove. A guessed rename is worse than a verbose
report.

Once a rename is known, side A's expressions are read *through* it before being
compared, which is what stops one rename from reporting a formula change in
every dependent.

## The impact set

The impact set is the transitive downstream closure of every change, taken over
the union of both sides' dependency edges — a removed component's dependents
exist only on side A, an added one's only on side B, and taking either side
alone drops half the blast radius.

- `seeds` — the changed things: components, tables and assumptions. A *pure*
  rename and a `doc-only` change are deliberately not seeds.
- `impacted` — components strictly downstream of a seed. Self-recursion
  (`x[t-1]` inside `x`) is not an edge: a component is not downstream of itself.
- `outputs_affected` — those of `seeds ∪ impacted` declared `kind = "Output"` on
  either side. This is the list a reviewer signs off.

A table change propagates through exactly the components whose expressions
contain a `Lookup` on it; an assumption change through exactly the components
that reference it by name.

## Tables and assumptions

Table declarations are compared field by field — `keys`, `values`, `on_missing`,
`source` and the pinned `digest` — and, when the CSV of both sides is readable,
row by row. Rows are joined on the leading key columns the declaration defines,
so a reordered file is not a change and a moved rate is:

```text
  T changed table mortality
      rows: +0 -0 ~1
        [18, F, false] 0.00017432 → 0.00019000
```

Assumption *values* come from the assumption-set files (`assumption_set = "…"`)
on each side; assumption *declarations* come from the modules. Both are reported
under `assumptions`.

## Exit codes

Following `04-verify.md` §7:

| Code | When |
|---|---|
| `0` | the comparison ran — **whether or not anything differed** |
| `1` | `--fail-on-change` was given and something semantic moved |
| `2` | a usage or structural failure: a missing path, one path instead of two, an unknown flag |

A difference is a *result*, not a failure, which is why the gate is opt-in.
`--fail-on-change` is measured against semantic change only: a pure rename or a
`doc-only` edit leaves it green.

```console
$ predictable diff model ./a ./b --fail-on-change ; echo "exit $?"
…
exit 1
```

## Scope

`predictable diff model` compares models. `predictable diff <runA> <runB>` — the
run diff of `04-verify.md` §5, which joins on modelpoint key, applies tolerance
profiles and ranks contributors — is a separate command and is still a stub in
this build. The subject word is mandatory rather than inferred from what the
paths look like, so the same invocation never sometimes compares formulas and
sometimes compares numbers.

## Using it from Rust

The comparison is a pure function of two `ModelSide` values, so the CLI has no
privileged path into it:

```rust
use predictable_modeldiff::{diff, report, ModelSide};

let a = ModelSide::load("before", &before_files)?;
let b = ModelSide::load("after", &after_files)?;
let d = diff(&a, &b);

println!("{}", report::render_text(&d));
assert!(d.summary.outputs_affected <= 2);
# Ok::<(), String>(())
```

A `ModelSide` can equally be built by hand from `predictable_ir::Module` values —
useful when the "before" model comes from a database or a migration tool rather
than from disk.
