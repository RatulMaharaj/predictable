# IR cheatsheet

Condensed from `docs/design/01-ir.md` §2–§5. When this page and the spec disagree, the spec wins.

## A `.pir` file

TOML-shaped, one module per file. Keys are ordered canonically by `predictable fmt`, so write
whatever order you like and format afterwards.

```toml
format = "pir/1"
module = "model"
imports = ["schema"]

[[component]]
name = "qx"
kind = "Derived"          # Input | Derived | Output
dtype = "f64"             # f64 | i64 | bool | date | str | enum(Name)
shape = "Series"          # Scalar | PerMP | Series
unit = "prob"
timing = "end"            # Series only; omit on Scalar/PerMP
expr = "mortality@(age, sex, smoker) * mortality_loading"
init = "0.0"              # the value at t = 0 for a recursive series
doc = "Annual mortality rate."

[component.meta.source]   # required by the migration loop, step 4
system = "prophet"
variable = "MORT_RATE"
file = "TERM.MOD:118"
```

## Shapes

| Shape | One value per | Example |
|---|---|---|
| `Scalar` | run | `valuation_rate` |
| `PerMP` | modelpoint | `entry_age`, `sum_assured`, any `npv(...)` |
| `Series` | modelpoint × `t` | `premium[t]`, `reserve[t]` |

Widening (`Scalar → PerMP → Series`) is implicit and free. **Narrowing is never implicit**: use an
explicit `Agg` (`sum`, `npv`, `at`, `first`, `last`, `max_over`, `min_over`, `count_while`) to go
from `Series` to `PerMP`. There is no "per-t but not per-modelpoint" shape; a yield curve is a
`Series` that happens to reference no `PerMP`, and the engine hoists it.

## Units

`none | money | rate(annual|monthly|period) | prob | count | years | months | factor`

Checked, never converted. `money + prob` is an error; `money * prob` is `money`; `money / money` is
`factor`; `1 - prob` is `prob`. `rate(annual) + rate(monthly)` is an error — convert with
`to_monthly` / `to_annual`. `unit = "none"` opts out and earns a lint (`W0102`), which the migration
stop condition does not accept.

## Timing

| Tag | Meaning | `npv` exponent |
|---|---|---|
| `start` | in advance, beginning of period `t` | `v^t` |
| `end` | in arrears, end of period `t` | `v^(t+1)` |
| `mid` | uniformly through the period | `v^(t+0.5)` |
| `point` | a state at an instant, not a flow | `v^t` |

Timing is consumed by `npv`, by the mixed-timing addition lint, and by `explain()`. `retime(x, tag)`
is a no-op on values and a cast on the tag — visible in the diff, which is the point. `Scalar` and
`PerMP` values have **no** timing (an `Agg` consumes it); retiming them is `W0105`.

## Expressions

```
Lit | Ref(x) | Lag(x, k) -> x[t-k] | At(x, k) -> x[k]
Unary | Binary | if C then A else B | Call(builtin, args)
table@(key, ...) | Agg(op, expr [, predicate])
```

* `x` means `x[t]`. `x[t-1]` is the previous period; `k ≥ 1`.
* `x[0]`, `x[12]` are absolute periods.
* **Forward references are illegal.** `x[t+1]` does not exist; a projection is one forward pass.
  A value that depends on the whole future is an `Agg`, evaluated in a second pass.
* `if` is a *value* conditional — both arms are required and both are evaluated — but a trap in an
  untaken arm is suppressed syntactically, so `if x == 0 then 0.0 else 1.0 / x` is correct.
* `x[t-k]` before the origin uses the component's `init` if it has one, else the dtype's zero, and
  records `N0301`/`N0302` in the trace.

## Builtins (closed set)

```
arithmetic   min max abs floor ceil round(x, dp) clamp(x, lo, hi) sign
exp/log      exp ln pow(x, y) sqrt
rates        to_monthly(r) to_annual(r) nominal_to_periodic(r, n) v_from_i(i) i_from_v(v)
             annuity_factor(i, n) compound(r, n)
timing       shift(x, k) retime(x, timing) cum(x) diff(x)
logic        and or not eq ne lt le gt ge is_null coalesce(x, y)
dates        year month day add_months(d, n) months_between(a, b) year_frac(a, b, convention)
aggregates   sum sum_kahan npv(x, disc) first last at(x, k) max_over min_over count_while(cond)
```

Traps to remember:

* `npv(x, disc)` takes a discount-**factor** series, not a rate, and uses `x`'s timing.
* `round` is half-away-from-zero on the shortest decimal representation — Prophet's convention,
  not IEEE half-even.
* `count_while(cond)` stops at the first false `t`; it does not count all true periods.
* `nominal_to_periodic(r, n)` is literally `r / n`, named so that the choice is visible in a diff.
  It is the only sanctioned way to write that division.

## Timeline and basis (§5)

The timeline's `basis` (`annual`/`monthly`) is a property of the run, and rates must be expressed on
it. **`annual_rate / 12` is an error, not a conversion** — use `to_monthly(r)`
(`1 - (1 - r)^(1/12)` for a probability) or, if a simple division really is what the source model
did, `nominal_to_periodic(r, 12)` so the decision is legible.

## Cycles

`x[t] = f(x[t-1])` is legal and is how every recursion is written. `x[t] = f(x[t])` is `E0201`.
A cycle through `init` is `E0202`.
