# Diagnostics you will meet in a migration

The complete catalogue — every code, generated from the registry — is `docs/llm/diagnostics.md`,
published at <https://predictable.dev/llm/diagnostics/>. **Every diagnostic you receive carries a
`doc_url` that resolves to its section there**; a CI test asserts that for every code emitted
anywhere in the codebase, so if a `doc_url` 404s that is a bug worth reporting, not a dead end.

This page is the short list: the codes that actually fire during a Prophet migration, and what to do
about each. Read the catalogue entry for anything not here.

## Namespaces

| Prefix | Source | Blocks the run? |
|---|---|---|
| `E0xxx` | `.pir` syntax, lowering, and the checker | yes |
| `W0xxx` | IR lints | no — but `W0102` blocks *this loop's* stop condition |
| `E1xxx` / `W1xxx` | the Python DSL, at build time | `E1xxx` yes |
| `P0xxx` | Prophet readers (`.MPF`, `.FAC`, `.RPT`) | some |
| `N0xxx` | provenance notes in an `explain()` trace | no — they are explanations |
| `H0xxx` | run-diff hypotheses | no — they are proposals |

## Step 2 — schema

| Code | Meaning | Standard fix |
|---|---|---|
| `E0203` | a name does not resolve | the schema is wrong or the module is not imported — fix the schema, not the formula |
| `E0041` / `E0042` / `E0044` | a block is missing a key, has a key of the wrong type, or has a key that is not part of it | the message names the key and the expected type |
| `E0043` | a value is not one of the legal choices | the message lists them; the suggestion is usually right |
| `E0050` | an optional modelpoint field has no `default` | decide: `required = true`, or give it a default. A Prophet blank is a null |
| `E0051` | an enum has no values | close the domain or use `str` |

## Step 3 — units and timing

| Code | Meaning | Standard fix |
|---|---|---|
| `W0102` | `unit = "none"` in money arithmetic | **give it a real unit.** This is the units decision, unmade; the stop condition does not accept it |
| `E0501` | incompatible units in an addition (`money + prob`) | one of the two components has the wrong unit — decide which |
| `E0502` | rates on different bases cannot be added | convert explicitly (`to_monthly`, `to_annual`) — never divide a rate by 12 by hand |
| `E0503` | `money * money` is not a unit | you wanted `money * factor` or `money * prob` |
| `W0103` | timing mismatch in a sum | `shift(x, 1)` or an explicit `retime` — and check step 3 |
| `W0105` | `retime`/`shift`/`cum`/`diff` on a value that has no timing | apply it one level down, to the flow rather than to the aggregate |
| `E0048` | a non-`Series` component was given a `timing` | remove the key; only `Series` values have timing |

## Step 4 — formulas

| Code | Meaning | Standard fix |
|---|---|---|
| `E0201` | cyclic dependency in the same period | one of the two reads should be `x[t-1]` |
| `E0202` | cycle through `init` | break the recursion in the initial value |
| `E0030` | a forward reference (`x[t+1]`) | not expressible; restate as an `Agg` over the finished series |
| `E0029` | `x[t-0]` | that is just `x` |
| `E0049` | a component with no `expr` | a `Derived`/`Output` component must have a formula |

## Prophet readers

| Code | Meaning | Standard fix |
|---|---|---|
| `P0104` / `P0105` / `P0106` | `VARIABLE_TYPES` length, or a short/long data row | the file is malformed or the delimiter guess is wrong; look at the raw line the message quotes |
| `P0108` | a percentage column was divided by 100 | confirm that is what the column meant |
| `P0109` | ambiguous date format | state it; the reader will not guess |
| `P0202` | `.fac` key ordering restated with the first four resolved keys | **eyeball these against the source table** — this is your transposition check |
| `P0203` | `.fac` value count does not match the extents | the dimensions or the data are wrong; no padding happens |
| `P0204` | a lookup policy was proposed | accept or override it deliberately; a wrong policy becomes `H0301` later |
| `P0301` / `P0302` / `P0303` | `.rpt` has no period axis / period base not stated / `TIME_UNITS` mismatch | pass `--period-base`, or fix the timeline basis |
| `P0304` | the `.rpt` is aggregate-level | modelpoint-level diffing is unavailable; the diff runs at group level |

## Trace notes (`explain()`)

`N0xxx` codes are why a cell computed what it did — a pre-origin default, an `init` used at `t = 0`,
a lookup clamped to a boundary, an untaken `if` arm. They are not problems; they are the answer to
"why is this number different from every other period". Read them before disputing a hypothesis.

## Run-diff hypotheses

| Code | Pattern | What it usually means in a migration |
|---|---|---|
| `H0101` | constant ratio `b/a` | a scale (thousands), or an escalation applied one period early |
| `H0102` | constant offset | a missing additive term |
| `H0103` | sign flip | a convention mismatch — `sign = -1` in `mapping.toml` |
| `H0201` | `b[t] ≈ a[t±1]` | a `timing` decision from step 3, or the `.rpt` period base |
| `H0202` | ratio ≈ `(1+i)^0.5` or `(1+i)` | `start`/`mid`/`end` mismatch inside `npv` |
| `H0301` | divergence starts at a table key boundary | lookup policy (`clamp` vs `step`) |
| `H0302` | one side is zero from `t = k` | term expiry / indicator off by one |
| `H0303` | only a subset of modelpoints sharing a field value | a segment-specific rate or a missing enum branch |
| `H0401` | ratio ≈ `r/12 ÷ ((1+r)^(1/12)−1)` | the Prophet `/12` idiom; decide `to_monthly` vs `nominal_to_periodic` |
| `H0402` | all `\|Δ\|` ≤ 0.5 × 10^−dp | the difference is rounding only — the *only* case where a tolerance profile is the right answer, and it is a profile, not a hand-widened bound |
| `H0501` | NaN/Inf on one side | a trap. Never a tolerance question; always fix it |

Confidence is `high` at 32/32 supporting modelpoints, `medium` at ≥ 28/32, `low` otherwise. Low
confidence hypotheses are still shown because you can cheaply falsify them with `explain()`; a
silent omission cannot be falsified at all.
