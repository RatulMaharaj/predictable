# Prophet construct → IR construct

Condensed from `docs/design/04-verify.md` §4 and `docs/v2/prophet-readers.md`. Read the reader's
diagnostics (`P0xxx`) as you go: they are the reader telling you which of these decisions it could
not make for you.

## Files

| Prophet | Contains | Becomes |
|---|---|---|
| `.MPF` | model point file | `[[modelpoint_field]]` declarations in `schema.pir` + a long-format modelpoint CSV |
| `.FAC` | factor / parameter table | `[[table]]` + a long-format table CSV |
| `.RPT` | results | a first-class **run directory** (`results.parquet` + `manifest.json`, `system = "prophet"`) — the baseline you diff against |
| workspace source | variable definitions | `[[component]]` in `model.pir` |

## Types (`VARIABLE_TYPES` on a `.MPF`)

| Prophet | IR `dtype` | Note |
|---|---|---|
| `I` | `i64` | integer / spcode |
| `N` | `f64` | a trailing `%` is divided by 100 and reported as `P0108` |
| `S`, `T` | `str` | codes become `enum(...)` when the domain is closed — prefer this |
| `D` | `date` | ambiguous formats are `P0109`, never guessed |
| `B` | `bool` | |

An empty field is null. A null in a `required` field fails at *load* time, not import time — so
decide `required` vs `default` deliberately (`E0050` if you forget).

## Variables

| Prophet idiom | IR |
|---|---|
| a variable referenced bare, same period | `Ref` — just `x` |
| `x(t-1)` / previous-period reference | `x[t-1]` (a `Lag`) |
| a value fixed at issue | `x[0]` (an `At`), or a `PerMP` component |
| an anticipated/forward reference | **not expressible** — restate it as an `Agg` over the finished series (second pass) |
| a `SELECT`/`IF` chain | nested `if C then A else B`; both arms required, traps in the untaken arm are suppressed |
| a table read `MORT_RATE(AGE, SEX)` | `mortality@(age, sex)` with an explicit `on_missing` policy |
| a global/parameter | a `Scalar` assumption in `assumptions.pir` |
| a run setting (periods, basis) | `[timeline]`, never a component |

## Timing

Prophet's convention is carried by naming and by the variable's role, not by a tag; the IR makes it
explicit, so this is a **decision you make once per component**:

| Prophet role | IR `timing` | Discount exponent |
|---|---|---|
| premium, in-advance annuity, expense at period start | `start` | `v^t` |
| claim paid, interest credited, expense at period end | `end` | `v^(t+1)` |
| claims assumed uniform over the period (the usual approximation) | `mid` | `v^(t+0.5)` |
| reserve, in-force count, any state measured at an instant | `point` | `v^t` |

Getting this wrong produces a difference at *every* timestep, which the run diff reports as `H0201`
(off-by-one in `t`) or `H0202` (timing basis). Fix the `timing`; do not paper over it with
`timing_shift` in `mapping.toml` unless the Prophet side is genuinely on a different convention.

## Rates — the `/12` trap

Prophet code is full of `ANN_RATE / 12`. In the IR that is an error, not a translation (IR §5):

| Intent | Write |
|---|---|
| an annual probability on a monthly timeline | `to_monthly(q)` — i.e. `1 - (1 - q)^(1/12)` |
| a monthly effective rate from an annual one | `to_monthly(i)` |
| a nominal rate genuinely divided by frequency | `nominal_to_periodic(r, 12)` — named so it shows up in the diff |
| a discount factor from a rate | `v_from_i(i)`, and pass the *factor* to `npv` |

If the source model really divided by 12, use `nominal_to_periodic` so the choice is legible and
reviewable. `H0401` is the run diff catching you having chosen the other one.

## Tables (`.FAC`)

* Values after `DATA` are row-major with the **last dimension varying fastest**. Getting this wrong
  transposes a mortality table silently, so the reader restates the ordering and the first four
  resolved keys as `P0202` — check them against the source by eye.
* Value count must equal `∏(HIₖ − LOₖ + 1)` exactly; no padding, no truncation (`P0203`).
* Every proposed lookup policy (`clamp` for a contiguous integer dimension such as age, `exact`
  otherwise) is emitted as `P0204`: a reviewed decision, not a silent default. A wrong policy shows
  up later as `H0301` — divergence starting exactly at a key boundary.
* Dimension names are snake-cased through `migration/name_map.json`, which the source importer
  reuses so lookups resolve.

## Results (`.RPT`)

* The period base is **never guessed**. Prophet models are commonly 1-based; the reader requires
  `--period-base {0|1}` (or `period_base` in `mapping.toml`) and errors with `P0302` otherwise.
  An off-by-one in `t` is the single most expensive false alarm in this loop.
* `TIME_UNITS` must match the timeline `basis` (`P0303`).
* Unmapped columns keep their Prophet identity as `prophet.<name>` — nothing is dropped, nothing is
  renamed behind your back.
* A grouped `.rpt` (no model point key) diffs at group level and says so (`P0304`).
* Component `timing` from a `.rpt` is `null`, because the file never states it. That is deliberate:
  inventing one would silently justify a timing shift that the diff should be reporting.

## `migration/mapping.toml`

Name correspondence is data, not heuristics. Every `sign`, `scale` and `timing_shift` is an
assertion about Prophet's conventions rather than a reproduction of its arithmetic, so every one
goes on the human sign-off sheet in the final report.

```toml
format = "pvf/1"
prophet_run = "TERM_BASE_2026Q2"
model_module = "term_assurance"
period_base = 1
mp_key = { prophet = "POL_NUM", predictable = "policy_number" }

[[component]]
prophet = "PREM_INC"
predictable = "premium_income"
sign = 1            # -1 for a convention flip (H0103)
scale = 1.0         # 1000.0 where Prophet reports in thousands (H0101)
timing_shift = 0    # integer periods (H0201) — prefer fixing `timing` instead

[[unmapped]]
prophet = "RESERVE_INT"
reason  = "intermediate; no predictable equivalent"
```
