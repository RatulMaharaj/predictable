# 04 — The verification loop

Status: **normative**. Version: verify formats `pvf/1`. Depends on: `01-ir.md` (IR `pir/1`).
Audience: engine implementers (Rust), tooling implementers (Python/CLI), and LLM agents performing
Prophet → predictable migration.

The thesis of predictable is that the *verification loop* is the product. A migration is not "an LLM
wrote some TOML"; it is a closed loop:

```
  read Prophet source ──▶ emit .pir ──▶ check (E/W JSON) ──▶ run ──▶ diff vs .rpt ──▶ explain()
        ▲                                    │                          │              │
        └────────────────────── agent-actionable feedback ──────────────┴──────────────┘
```

Every arrow in that loop must produce **machine-readable, span-anchored, fix-suggesting** output.
This document specifies the four artefacts on the right-hand side of that loop — `explain()` traces,
Prophet interop readers, run-diff, run manifests — plus the tolerance policy that makes "equal" a
defined word, and the published agent skill that drives the loop.

Design rule for this whole layer, and the one thing to remember:

> **Every diagnostic answers three questions: *where*, *how much*, and *what to change*.**
> A verify output that reports a difference without localising it to a component and a `t`, and
> without naming a candidate cause, is a bug in the verify layer.

---

## 1. Artefact overview

| Artefact | File | Format | Produced by |
|---|---|---|---|
| Run manifest | `run/manifest.json` | `pvf/1` manifest | every run |
| Results | `run/results.parquet` + `run/results.schema.json` | Arrow/Parquet | every run |
| Trace | `run/traces/<mp>_<comp>_<t>.trace.json` | `pvf/1` trace | `explain()` |
| Check diagnostics | stdout / `check.json` | `pvf/1` diag (IR §7) | `predictable check` |
| Run diff | `diff.json` + terminal render | `pvf/1` rundiff | `predictable diff` |
| Migration coverage | `migration/coverage.json` | `pvf/1` coverage | `predictable prophet import` |

All JSON artefacts carry `{"format": "pvf/1", "kind": "<trace|rundiff|manifest|coverage>"}` as their
first two keys. All are UTF-8, LF, and canonically ordered (keys in the order given in this spec) so
that they are themselves diffable in git.

---

## 2. Results schema

Everything downstream (diff, explain, UI) joins on one schema. Long format, one row per
(modelpoint, component, t), because it is the only shape that diffs cleanly when two runs have
different component sets.

`results.parquet` columns:

```
mp_key      : string      -- the modelpoint field marked key = true
mp_row      : uint32      -- 0-based row index in the modelpoint file (stable ordering)
component   : dictionary<string>
stage       : int8        -- 1 | 2, copied from the IR (01-ir §2.2)
t           : int32       -- -1 for Scalar and PerMP components
value       : double      -- f64 components
value_i     : int64       -- nullable, i64 components
value_b     : bool        -- nullable
value_s     : dictionary<string>  -- nullable, str/enum/date(ISO) components
```

Exactly one `value*` column is non-null per row; which one is determined by the component's `dtype`
in the IR, not by inspection. `results.schema.json` restates the component list with
`{id, name, kind, dtype, shape, unit, timing, stage, output, display}` copied verbatim from the IR
(`id` being the qualified `<module_path>.<name>` of IR §2.2) so that a consumer
never needs to parse `.pir` to interpret a result set.

Only `kind = "Output"` components are emitted by default. `--emit all` emits every `Derived`
component; this is the mode the migration loop uses, because diffing only the final BEL tells an
agent nothing about *where* it went wrong. `emit` is a `run_digest` field and the emitted set is
summarised by `results.component_set_digest` (IR §8.4.3), so an `emit` difference is a named
condition here (§5.4, `emit_mismatch`) rather than a generic structural incomparability.

A trapped modelpoint under `--continue-on-trap` contributes **no rows** to this table — not null rows
— and is listed in `manifest.execution.traps` (IR §9.3.1). There are no runtime nulls (IR §2.11); the
nullable `value_*` columns above express dtype selection only.

Portfolio aggregations (IR §8.3) are emitted separately to `run/aggregates.parquet` with
`aggregation : string, group_key : string, measure : string, t : int32, value : double`, where
`group_key` is the ordered `key=value|key=value` rendering of IR §8.3 and `t` is `-1` unless the
aggregation declared `over_t = "each"`. Per-modelpoint solve outcomes go to
`run/solves/<name>.parquet` (IR §8.4.4).

---

## 3. `explain()` — provenance traces

### 3.1 Signature

Python DSL:

```python
def explain(
    component: str,
    modelpoint: str | int,        # mp_key or mp_row
    t: int | None = None,         # required for Series; must be None for Scalar/PerMP
    *,
    depth: int = 2,               # levels of children to expand; -1 = full
    expand: Sequence[str] = (),   # component names to expand to full depth regardless
    values_only: bool = False,    # drop spans/meta for a compact trace
) -> Trace
```

CLI:

```
predictable explain run/ --component reserve --mp POL00042 --t 3 [--depth 3] [--json]
```

`Trace` is a Python object with `.json` (dict), `.text` (str, the terminal rendering), and
`_repr_html_` / `.show()` for the notebook and web UI. **The JSON is normative; the text and HTML are
renderings of it.** No information exists in the text that is absent from the JSON.

### 3.2 Trace JSON schema

The trace is a tree of **nodes**. Every node has `node`, `value`, and `span`; the rest is per-variant.

```jsonc
{
  "format": "pvf/1",
  "kind": "trace",
  "run": "sha256:3f0a…",              // manifest digest of the run being explained
  "root": {
    "node": "Component",
    "id": "term_assurance.reserve",
    "t": 3,
    "modelpoint": {"key": "POL00042", "row": 41},
    "value": 1284.55,
    "dtype": "f64", "unit": "money", "timing": "point", "shape": "Series",
    "stage": 1,
    "expr": "(reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + valuation_rate)",
    "span": {"file": "models/term_assurance/model.pir", "line": 148, "col": 9,
             "byte_start": 4021, "byte_end": 4123},
    "children": [ /* Expr nodes */ ]
  },
  "notes": [ /* trace-level notes, see 3.4 */ ],
  "truncated": {"depth": 2, "elided_nodes": 37}
}
```

Node variants — this list is closed and mirrors `Expr` (IR §2.6) one-to-one:

```jsonc
// Binary / Unary — structural arithmetic
{"node":"Binary","op":"*","value":1284.55,"unit":"money","span":{…},"children":[lhs, rhs]}
{"node":"Unary","op":"-","value":-12.0,"unit":"money","span":{…},"children":[operand]}

// Ref — same-period read. Crosses into another component; `children` is that component's own tree.
{"node":"Ref","ref":"term_assurance.premium_rate","t":3,"value":330.75,
 "kind":"Derived","unit":"money","timing":"start","span":{…},"children":[…]}

// Lag — previous-period read
{"node":"Lag","ref":"term_assurance.reserve","lag":1,"t":2,"value":1180.11,
 "unit":"money","span":{…},"children":[…]}

// At — absolute-period read
{"node":"At","ref":"term_assurance.premium_rate","at":0,"value":300.00,"unit":"money","span":{…}}

// Input leaves — never have children
{"node":"Input","ref":"sum_assured","kind":"Modelpoint","value":100000.0,"unit":"money",
 "source":{"file":"data/term.mpf","row":41,"column":"SUM_ASSURED"}}
{"node":"Input","ref":"valuation_rate","kind":"Assumption","value":0.035,"unit":"rate(annual)",
 "source":{"assumption_set":"base","file":"assumptions/base.pir","line":6}}
{"node":"Input","ref":"t","kind":"Timeline","value":3,"unit":"none"}

// Literal
{"node":"Lit","value":1.0,"dtype":"f64","unit":"none","span":{…}}

// If — records which branch was taken; both branches are evaluated (IR §2.6) and both are recorded
{"node":"If","value":1.0,"taken":"then","span":{…},
 "children":[cond_node, then_node, else_node]}

// Lookup — the highest-value node in the whole trace
{"node":"Lookup","table":"sa8990","value":0.00214,"unit":"prob","span":{…},
 "keys":[{"name":"age","requested":47,"resolved":47,"policy":"clamp","fired":false,
          "child": {…}},
         {"name":"gender","requested":"M","resolved":"M","policy":"exact","fired":false},
         {"name":"smoker","requested":false,"resolved":false,"policy":"exact","fired":false}],
 "row":312,"table_digest":"sha256:9f2c…",
 "key_range":{"age":[18,120]}}

// Call — builtin
{"node":"Call","fn":"compound","value":1.0927,"unit":"factor","span":{…},"children":[…]}

// Agg — stage 2; children are the per-t contributions, elided beyond `agg_detail`
{"node":"Agg","op":"npv","value":18244.31,"unit":"money","span":{…},
 "over":{"component":"term_assurance.death_claims","timing":"end","t_from":0,"t_to":40,
         "discount":"term_assurance.disc_factor","exponent_rule":"v^(t+1)"},
 "terms":[{"t":0,"x":142.11,"disc":0.9662,"contrib":137.31}, …],
 "terms_truncated":false}
// `terms` is REQUIRED on every Agg node (IR §11.2, Q11): ascending t, no gaps, contributions
// summing left-to-right to `value` exactly. Beyond `--trace-max-terms` (default 4096) the first
// and last N/2 terms are kept and `terms_truncated` is `true`. Truncation is never silent.

// Retime — the audit-relevant no-op
{"node":"Call","fn":"retime","value":312.40,"from_timing":"start","to_timing":"mid",
 "span":{…},"children":[…]}
```

### 3.3 What a `Ref` expands into

A `Ref` node's `children` is the **evaluated tree of the referenced component at that `t`**, not a
placeholder. This is what makes `explain` a real provenance trace rather than a one-level formula
dump: `explain("bel", "POL00042")` with `depth=-1` bottoms out entirely in `Input` and `Lit` leaves,
and the arithmetic in the tree reproduces `bel` exactly.

Cycles across periods terminate naturally because a `Lag` decrements `t`; at `t < 0` the trace
records a `pre_origin` leaf:

```jsonc
{"node":"Lag","ref":"term_assurance.reserve","lag":1,"t":-1,"value":842.10,
 "resolution":"init","init_expr":"bel","children":[…]}
{"node":"Lag","ref":"x","lag":1,"t":-1,"value":0.0,"resolution":"pre_origin_default",
 "note":"N0301"}
```

`resolution` ∈ `{"computed","init","pre_origin_default"}`. This directly surfaces IR §2.7's
out-of-range rule at the exact node where it fired.

### 3.4 Notes — the machine-readable "look here" channel

Traces carry `notes`, a flat array of coded observations attached to node paths. Notes are how the
verify layer says *this is where your model is probably wrong*, and they are the first thing a
diffing agent reads.

```jsonc
{"code":"N0101","severity":"info","path":"root.children[0].children[2]",
 "message":"Lookup on sa8990 clamped key age 121 → 120.",
 "component":"term_assurance.qx","t":3}
```

Note codes (`N0xxx`, distinct from IR `E`/`W` codes):

| Code | Meaning |
|---|---|
| `N0101` | Lookup key clamped |
| `N0102` | Lookup key stepped (banded table, exact key absent) |
| `N0103` | Lookup key interpolated between rows *a* and *b* |
| `N0104` | Lookup hit `on_missing = default(...)` |
| `N0201` | `retime` applied — timing cast, value unchanged |
| `N0202` | Timing-mismatched addition (`start` + `end`) inside this expression |
| `N0301` | `pre_origin_default` used (no `init` declared) |
| `N0302` | `init` used at `t = 0` |
| `N0401` | Value is exactly `0.0` and at least one factor in a product chain is `0.0` — the "why is my BEL zero" note, naming the zeroing factor |
| `N0402` | Value magnitude > 1e12 or < 1e-12 while unit is `money` (probable scaling error) |
| `N0403` | Denominator within 1e-12 of zero (near-trap) |
| `N0501` | Branch of an `If` never taken for any `t` in this projection |

`N0401` deserves its name: the single most common migration failure is a whole component collapsing
to zero because an indicator or an in-force count went to zero earlier than the Prophet model's did,
and the note points straight at the responsible factor.

### 3.5 Human rendering

`trace.text` is a deterministic ASCII rendering; the same tree always renders identically, so a
rendered trace can be committed as a golden test.

```
reserve[t=3]  POL00042  = 1,284.55  money  point            model.pir:148
│ (reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + valuation_rate)
│
├─ * ──────────────────────────────────────────────── 1,284.55
│  ├─ ( … ) ────────────────────────────────────────  1,241.11
│  │  ├─ reserve[t-1]          t=2   1,180.11  money   (computed)
│  │  ├─ + premium_income[t-1] t=2     312.40  money
│  │  │    ├─ premium_rate     t=2     330.75  money
│  │  │    └─ num_pols_if      t=2      0.9447 count
│  │  ├─ − death_claims[t-1]   t=2     211.40  money
│  │  └─ − renewal_expenses[t-1] t=2    40.00  money
│  └─ (1 + valuation_rate) ─────────────────────────  1.035
│     └─ valuation_rate  assumption[base]  0.035  rate(annual)   base.pir:6
│
notes:
  N0101  qx  t=3  lookup sa8990 clamped age 121 → 120
```

Rules: values right-aligned, thousands-separated, 2 dp for `money`, 6 sf for `prob`/`factor`,
unit and timing after the value, span at the right margin. `--no-color` and `--width` respected;
colour is never the sole carrier of meaning.

### 3.6 Implementation — replay, not tape

Tracing is a **replay** of one modelpoint with a recording evaluator (IR §11.2). The hot path is
untouched. Because projection is deterministic (IR §9), the replayed value is guaranteed to equal the
recorded result; the engine asserts this and emits `E0901: trace replay diverged from run` if not —
which would be an engine bug, and is treated as one.

Replay cost is one modelpoint projection plus tree allocation. `explain()` on a 480-period model with
200 components is single-digit milliseconds. There is therefore no "tracing mode" flag on a run.

### 3.7 `explain_diff()` — the composite the agent actually calls

```python
explain_diff(component, modelpoint, t, run_a, run_b) -> DiffTrace
```

Produces two traces aligned node-by-node (structural alignment on `span` + node path) with a
`delta` on every node, so a divergence can be walked down to the first node where the two runs stop
agreeing. Output adds `"first_divergence": {"path": …, "component": …, "t": …, "a": …, "b": …}` —
this single field is the highest-value output in the whole system for an LLM, because it converts
"the answer is wrong" into "this input differs".

For a Prophet comparison, `run_b` may be a `.rpt`-backed result set; alignment then stops at
component granularity (Prophet gives no sub-expression detail), and the trace of `run_a` is annotated
with the Prophet values wherever a component mapping exists.

---

## 4. Prophet interop readers

Scope: **read-only import**. predictable never writes Prophet formats. Three readers, one command
surface:

```
predictable prophet import  <workspace>  -o models/<module>/   # source → .pir (best effort, annotated)
predictable prophet mpf     <file.mpf>  -o data/term.mpf.parquet + schema fragment
predictable prophet fac     <file.fac>  -o tables/<name>.csv   + [[table]] fragment
predictable prophet rpt     <file.rpt>  -o run_prophet/        # → results.parquet + manifest
```

Each reader is a pure function `bytes -> (Value, [Diagnostic])`. Diagnostics use the same JSON shape
as IR §7 (`{code, severity, message, spans, suggestions, doc_url}`) with codes in the `P0xxx` range,
and spans pointing into the Prophet file. The readers are `no_std`-friendly Rust in
`crates/predictable-prophet`, so they also work in the WASM build.

### 4.1 MPF reader

Prophet model point files: a header block of `key,value` control lines, a `VARIABLE_TYPES` line, an
`OUTPUT_FORMAT` line naming the columns, then `*` -prefixed data rows. Real-world files are
comma-separated, may be quoted, are frequently Windows-1252, and always end lines with CRLF.

```
! Term assurance model points, 2026-06-30
Output_Format, .......
VARIABLE_TYPES,I,S,N,N,N,N,N,N,T,D
NUMLINES,4
SPCODE,POL_NUM,AGE_AT_ENTRY,SEX,SMOKER_STAT,SUM_ASSURED,ANN_PREM,POL_TERM,PROD_CD,ENTRY_DATE
*,1,POL00042,47,M,N,100000,540.00,25,TERM01,20240601
```

**Parsing spec:**

1. **Encoding.** Try UTF-8; on failure, decode Windows-1252 (never fail on encoding — emit `P0101`
   lint naming the byte offset). Strip a UTF-8 BOM. Accept LF, CRLF, or CR line endings.
2. **Comment / directive lines.** Lines beginning `!` are comments and are retained as
   `header.comments` (they routinely contain the only documentation the file has). Lines beginning
   `#` are treated the same.
3. **Header key-value lines** are any line before the first `*` row that splits on the first comma
   into a recognised key. Recognised keys, case-insensitive:
   `OUTPUT_FORMAT`, `VARIABLE_TYPES`, `NUMLINES`, `MPF_VERSION`, `SPCODE_SET`, `DATE_FORMAT`.
   Unrecognised header keys are retained verbatim in `header.extra` and emit `P0102` (info).
4. **The DCS header.** The column-name line is the last non-`*` line before the first `*` row that
   is *not* a recognised key — this is the robust rule, because different Prophet releases either
   name it `OUTPUT_FORMAT` on the same line or place the names on their own line. If both a named
   `OUTPUT_FORMAT` payload and a bare name line are present and they disagree, that is `P0103`
   (error) listing both.
5. **`VARIABLE_TYPES`** maps positionally onto the column names, *offset by the leading `*`
   column*. The type letters and their IR mapping:

   | Prophet | Meaning | IR `dtype` | IR `unit` (default) |
   |---|---|---|---|
   | `I` | integer / spcode | `i64` | `none` |
   | `N` | numeric | `f64` | `none` (see 4.1.7) |
   | `S` | string | `str` | `none` |
   | `T` | text/code | `str` | `none` |
   | `D` | date | `date` | `none` |
   | `B` | boolean flag | `bool` | `none` |

   Length mismatch between `VARIABLE_TYPES` and the name line is `P0104` (error) reporting both counts
   and the first name whose type is missing.
6. **Data rows** begin with `*`. Field count must equal the column count; a short row is `P0105`
   (error) with the row number and the raw line; a long row is `P0106`. `NUMLINES`, if present and
   inconsistent with the actual row count, is `P0107` (warning) — the data wins.
7. **Value parsing.** `N` fields: strip thousands separators, accept leading `+`, accept a trailing
   `%` (converts by /100 and emits `P0108` naming the affected column). Empty string → null, and a
   null in a `required` field is an error at load time, not import time. `D` fields: try, in order,
   `YYYYMMDD`, `DD/MM/YYYY`, `YYYY-MM-DD`, honouring `DATE_FORMAT` if the header set it; ambiguity
   between `DD/MM` and `MM/DD` that the data cannot resolve is `P0109` (error) — never guess a date
   order silently.
8. **Emitted artefacts.** The reader writes a Parquet modelpoint file and a `[[modelpoint_field]]`
   fragment. Names are lowercased and snake-cased (`SUM_ASSURED` → `sum_assured`), with the original
   preserved in `meta.source.column`. `unit` is left as `none` with a `W0102` lint per numeric
   column, because guessing `money` vs `count` from a column name is exactly the kind of silent
   inference this project refuses; the migration skill (§8) instructs the agent to fill units in as
   an explicit review step.
9. **Distinct-value report.** For every `S`/`T` column, the reader emits the distinct value set into
   the fragment as a proposed `[[enum]]`, capped at 64 values. `SEX` → `enum(Gender)` when the value
   set is exactly `{M,F}`; otherwise it stays `str` and emits a suggestion.

### 4.2 `.fac` table reader

Prophet factor/table files. Structure: a header naming the dimensions and their index ranges, then
either a flat list of values in row-major order or an explicitly indexed block.

```
! Mortality SA8990
TABLE_NAME, SA8990
DIMENSIONS, 3
DIM1, AGE, 18, 120
DIM2, SEX, 1, 2
DIM3, SMOKER, 1, 2
DATA
0.000412, 0.000387, 0.000501, 0.000466
...
```

**Parsing spec:**

1. Same encoding/line-ending/comment rules as §4.1.
2. `DIMENSIONS, n` then exactly `n` `DIMk, NAME, LO, HI` lines. Missing or inconsistent `n` is
   `P0201` (error). Dimension extents are inclusive.
3. **Value ordering after `DATA` is row-major with the *last* dimension varying fastest.** This is
   the Prophet convention and getting it wrong transposes a mortality table silently — so the reader
   also emits `P0202` (info) restating the ordering and the first four resolved keys with their
   values, which is printed in the terminal and included in the coverage report, so a human or an
   agent can eyeball the corner of the table against the source.
4. Value count must equal `∏(HIₖ − LOₖ + 1)` exactly. Any mismatch is `P0203` (error) reporting
   expected vs actual and the implied missing/extra count. There is no truncation and no padding.
5. Multi-value tables (`VALUES, qx, ix`) interleave per key; the reader splits them into named value
   columns.
6. **Output** is a long-format CSV — one column per key, one per value — plus a `[[table]]` fragment
   with `keys`, `values`, `on_missing = "error"`, `source`, and the computed `digest`. Key `policy`
   is proposed as `clamp` for the first numeric dimension whose values are contiguous integers
   (typically age) and `exact` otherwise; every proposed policy is emitted as a `P0204` info
   diagnostic so it is a reviewed decision, not a default that disappears.
7. Dimension names are mapped through the same snake-case rule; the mapping table
   (`AGE → age`, `SEX → gender`) is written to `migration/name_map.json` and is reused by the source
   importer so table lookups resolve.

### 4.3 `.rpt` results reader

The `.rpt` is the reconciliation target. Prophet results files come in two shapes and the reader
detects which:

- **Grouped/time-series form**: a header block (`RUN`, `PRODUCT`, `RUN_DATE`, `TIME_UNITS`), a
  variable-name line, then one row per period per group.
- **Per-model-point form**: an additional model-point key column, one row per (mp, period).

```
! Prophet results
RUN, TERM_BASE_2026Q2
PRODUCT, TERM_UK
RUN_DATE, 30/06/2026
TIME_UNITS, MONTHS
NUM_PERIODS, 480
SPCODE, POL_NUM, PERIOD, PREM_INC, DTH_CLAIM, EXPENSE, BEL_TOT
1, POL00042, 0, 540.00, 0.00, 45.00, 12844.51
```

**Parsing spec:**

1. Header parsed as §4.1 header rules; `RUN`, `PRODUCT`, `RUN_DATE`, `TIME_UNITS`, `NUM_PERIODS`,
   `VALUATION_DATE` are recognised and copied into the synthesised manifest (§6.3).
2. The period column is identified by name from `{PERIOD, T, TIME, MONTH, YEAR, DURATION}`;
   if absent, `P0301` (error) — a results file without a period axis cannot be diffed by timestep.
3. **Period base.** Prophet models are commonly 1-based in `t`. The reader does not guess: it reports
   the observed min and max period and requires `--period-base {0|1}` (or `period_base` in the
   mapping file) when the observed minimum is `1`, emitting `P0302` (error) otherwise. An off-by-one
   in `t` produces a diff at *every* timestep and is the single most expensive false alarm in the
   loop, so it is made an explicit, one-time, recorded decision.
4. **Time units.** `TIME_UNITS` must match the IR timeline `basis`; a mismatch is `P0303` (error)
   with the suggestion to either change the timeline or aggregate the Prophet output.
5. Each remaining numeric column becomes a component in the imported result set, named by the
   mapping file (§4.4), or by the raw Prophet name prefixed `prophet.` when unmapped.
6. Grouped files (no model point key) load with `mp_key = "<group>"` and the diff runs at group
   level; a `P0304` info records that model-point-level diffing is unavailable.
7. Output is a `results.parquet` in the §2 schema plus a `manifest.json` with
   `"source": {"system": "prophet", "file": …, "digest": …}` — so a Prophet run is a first-class run
   and `predictable diff run_prophet/ run/` is the same code path as diffing two predictable runs.

### 4.4 The mapping file

Component-name correspondence is data, not heuristics. `migration/mapping.toml`:

```toml
format = "pvf/1"
prophet_run = "TERM_BASE_2026Q2"
model_module = "term_assurance"
period_base = 1
mp_key = { prophet = "POL_NUM", predictable = "policy_number" }

[[component]]
prophet = "PREM_INC"
predictable = "premium_income"
sign = 1                      # 1 | -1, for convention flips
scale = 1.0                   # e.g. 1000.0 where Prophet reports in thousands
timing_shift = 0              # integer periods, for in-advance/in-arrears mismatch

[[component]]
prophet = "BEL_TOT"
predictable = "bel"

[[unmapped]]
prophet = "RESERVE_INT"
reason  = "intermediate; no predictable equivalent"
```

`sign`, `scale`, and `timing_shift` exist because these three are the entirety of the "it's the same
number in different clothes" space, and each one is a *declared, reviewable* claim rather than a
fudge applied inside a comparison. Any diff run where a mapping entry has a non-default `sign`,
`scale`, or `timing_shift` prints those entries in the header of the diff report.

---

## 5. Run diff

### 5.1 Command

```
predictable diff <run_a> <run_b> [--tolerance-profile reconcile] [--mapping migration/mapping.toml]
                 [--top 20] [--component bel] [--mp POL00042] [--json diff.json] [--fail-on any]
```

`run_a` and `run_b` are run directories; either may be a Prophet-imported run. Exit codes:
`0` all within tolerance, `1` divergence beyond tolerance, `2` structurally incomparable
(different `T`, disjoint modelpoint sets — reported, not diffed).

A differing **component set** is *not* by itself `exit 2`. When the two runs' `emit` settings or
`component_set_digest`s differ (IR §8.4.3), the diff reports `summary.emit_mismatch`, prints a banner
naming both settings, diffs the intersection, lists the one-sided components, and exits on the
intersection's tolerance outcome. `--require-same-emit` restores `exit 2` for that case. Diffing a
`--emit outputs` run against a `--emit all` run is a routine thing to want; it should not look like a
broken comparison.

### 5.2 The one-line signal

The headline format, quoted verbatim in the project thesis, is:

```
BEL diverges at t=7 in component expense: 10.00 vs 10.31
```

Generalised, one line per finding:

```
<output> diverges at t=<t> in component <component>: <a> vs <b>  (Δ <abs>, <rel>%)  [mp <key>]
```

The grammar matters: `<output>` is the *affected output* (from the IR impact set), `<component>` is
the *earliest upstream component that diverges*, and `t` is the *earliest `t` at which it does*.
That triple is the localisation, and computing it is the substance of the diff.

### 5.3 Algorithm

1. **Align.** Join on `(mp_key, component, t)` after applying the mapping file. Report set
   differences separately: `only_in_a`, `only_in_b`, both at component and modelpoint level.
2. **Compare** each cell under the tolerance policy (§5.6). Cells within tolerance are dropped.
3. **Earliest divergence per component.** For each component, find `t_first` = min `t` with a
   divergence, and record `(a, b, abs, rel)` there.
4. **Root-cause ranking.** Build the IR dependency graph. A diverging component whose *inputs at the
   relevant `t`* all agree within tolerance is a **root divergence**; one whose inputs also diverge is
   **inherited**. Rank findings: root divergences first, then by `|contribution to the movement of the
   largest affected Output|` — computed as the change in that output attributable to the component via
   the impact set, or by a one-at-a-time replay when the attribution is ambiguous.
5. **Attribute to a model change** where both runs are predictable runs: cross-reference
   `predictable diff modelA modelB` (IR §11.3). A root divergence in a component that the model diff
   marks as changed is reported as *explained*; one in an unchanged component is reported as
   *unexplained* and promoted to the top of the report, because unexplained divergence is where bugs
   live.
6. **Classify** each finding (§5.5).
7. **Emit** `diff.json` and the terminal rendering.

Complexity is O(cells) for steps 1–3 and O(edges × findings) for step 4; on a 10k-modelpoint,
480-period, 200-component run this is a few seconds and is the intended interactive loop speed.

### 5.4 `diff.json` schema

```jsonc
{
  "format": "pvf/1",
  "kind": "rundiff",
  "a": {"path": "run_prophet/", "manifest": "sha256:…", "system": "prophet",
        "run": "TERM_BASE_2026Q2"},
  "b": {"path": "run/", "manifest": "sha256:…", "system": "predictable",
        "engine_version": "0.4.1", "model_digest": "sha256:…"},
  "tolerance": {"profile": "reconcile", "abs": 0.005, "rel": 1e-6, "money_dp": 2},
  "mapping": {"file": "migration/mapping.toml", "digest": "sha256:…",
              "adjusted": [{"component":"premium_income","sign":1,"scale":1000.0}]},

  "summary": {
    "verdict": "diverged",                       // matched | diverged | incomparable
    "modelpoints": {"a": 10000, "b": 10000, "common": 10000, "only_a": 0, "only_b": 0},
    "components": {"common": 41, "only_a": 3, "only_b": 5, "unmapped_a": ["RESERVE_INT"]},
    "emit_mismatch": {"a": "outputs", "b": "all",
                      "a_component_set_digest": "sha256:19bd…",
                      "b_component_set_digest": "sha256:aa47…"},   // null when they agree

    "cells": {"compared": 19680000, "diverged": 4122, "max_abs": 31.44, "max_rel": 3.1e-2},
    "outputs": [
      {"component":"bel","a_total":128445100.0,"b_total":128449220.0,
       "abs":4120.0,"rel":3.2e-5,"within_tolerance":false}
    ],
    "root_divergences": 1,
    "first_divergence": {"component":"renewal_expenses","t":7,"mp_key":"POL00042"}
  },

  "findings": [
    {
      "id": "F001",
      "class": "root",                            // root | inherited | structural | tolerance-only
      "category": "value",                        // value | missing | extra | shape | dtype | nan
      "component": "term_assurance.renewal_expenses",
      "prophet_component": "EXPENSE",
      "affects_outputs": ["bel", "reserve", "net_cashflow"],
      "t_first": 7,
      "t_range": [7, 40],
      "n_modelpoints": 10000,
      "exemplar": {"mp_key": "POL00042", "mp_row": 41, "t": 7,
                   "a": 10.00, "b": 10.31, "abs": 0.31, "rel": 0.031},
      "worst": {"mp_key": "POL09912", "t": 39, "a": 41.02, "b": 44.87,
                "abs": 3.85, "rel": 0.0939},
      "contribution": {"output": "bel", "share_of_total_delta": 0.97},
      "explained_by_model_change": null,
      "message": "BEL diverges at t=7 in component renewal_expenses: 10.00 vs 10.31",
      "hypotheses": [
        {"code":"H0201","confidence":"high",
         "message":"b compounds expense_inflation from t=0 while a appears to compound from t=1.",
         "evidence":"b/a = 1.03^1 = 1.030 at every t ≥ 7 (constant ratio 1.0300 ± 1e-6).",
         "suggested_edit":{"file":"models/term_assurance/model.pir","line":132,
                           "byte_start":3410,"byte_end":3441,
                           "old":"compound(expense_inflation, t)",
                           "new":"compound(expense_inflation, t - 1)"}}
      ],
      "explain_command": "predictable explain-diff run_prophet/ run/ --component renewal_expenses --mp POL00042 --t 7"
    }
  ],

  "structural": {
    "only_in_a": [{"component":"RESERVE_INT","reason":"unmapped"}],
    "only_in_b": [{"component":"risk_adjustment","reason":"no prophet counterpart"}],
    "modelpoints_only_in_a": [], "modelpoints_only_in_b": []
  }
}
```

Field discipline: `findings` is sorted by `class` then `contribution.share_of_total_delta`
descending, and is truncated at `--top` with `"findings_truncated": {"kept": 20, "of": 137}`.
`exemplar` is chosen deterministically as the lowest `mp_row` exhibiting the divergence at `t_first`,
so re-running the diff on the same data yields a byte-identical report modulo timestamps.

### 5.5 Divergence classes and hypotheses

The `hypotheses` array is the design's central bet: the diff does not merely report, it *proposes*.
Hypotheses are generated by a fixed set of pattern detectors run over the per-`t` divergence vector
of a component. Each is cheap, each is falsifiable, and each carries the evidence that fired it.

| Code | Pattern detected | Test |
|---|---|---|
| `H0101` | Constant ratio | `b/a` constant across `t` to 1e-9 → scale factor; report the ratio and check it against 12, 1/12, 1000, `(1+r)`, `(1+r)^(1/12)` |
| `H0102` | Constant offset | `b−a` constant → additive term missing |
| `H0103` | Sign flip | `b ≈ −a` → convention mismatch; suggest `sign = -1` in mapping |
| `H0201` | Off-by-one in `t` | `b[t] ≈ a[t±1]` for all `t` → timing/`shift`; suggest `shift`, `retime`, or `period_base` |
| `H0202` | Timing basis | ratio ≈ `(1+i)^0.5` or `(1+i)` → `start`/`mid`/`end` mismatch in `npv`; name the timing tags of both |
| `H0301` | Divergence begins exactly at a table key boundary | → lookup `policy` mismatch (`clamp` vs `step`); cite the `N01xx` note from a trace |
| `H0302` | `a` is zero where `b` is not (or vice versa) from `t = k` onward | → indicator/term expiry off by one; name the `in_term`-like component |
| `H0303` | Divergence only for a subset of modelpoints sharing a field value | → segment-specific rate or missing enum branch; report the discriminating field and value |
| `H0401` | Rate conversion | ratio ≈ `r/12` ÷ `((1+r)^(1/12)−1)` → the Prophet `/12` idiom (IR §5) |
| `H0402` | Rounding | all `|Δ|` ≤ 0.5 × 10^−dp → the difference is rounding only; suggest a tolerance profile rather than a code change |
| `H0501` | NaN/Inf in one run | → trap; always severity-max, never a tolerance question |

Detectors run per component on the exemplar modelpoint first (cheap), and a hypothesis is only
emitted if it also holds on a random sample of 32 further diverging modelpoints — reported as
`"evidence_support": {"tested": 32, "held": 32}`. `confidence` is `high` when support is 32/32,
`medium` at ≥ 28/32, `low` otherwise, and low-confidence hypotheses are still emitted because an
LLM can cheaply falsify them and a silent omission cannot be falsified at all.

`suggested_edit` uses the same `{file, byte_start, byte_end, old, new}` shape as IR §7 suggestions, so
one code path in the agent applies edits from both the checker and the differ.

### 5.6 Tolerance and rounding policy

Comparison is defined once, here, and nowhere else.

**The predicate.** Two `f64` values `a` and `b` match iff:

```
|a − b| ≤ abs_tol   OR   |a − b| ≤ rel_tol × max(|a|, |b|)
```

Disjunctive, not conjunctive: `abs_tol` handles values near zero where relative tolerance is
meaningless, `rel_tol` handles large values where absolute tolerance is meaningless. `max(|a|,|b|)`
rather than `|a|` so the predicate is symmetric — `diff A B` and `diff B A` must report identically.

Special values: `NaN` never matches anything, including `NaN` (`H0501`). `+Inf` matches `+Inf` only.
`−0.0` matches `+0.0`. Integers, booleans, strings, dates and enums compare exactly; no tolerance
applies and a mismatch is always a finding.

**Profiles.** Named, versioned, and recorded in the diff manifest.

| Profile | `abs_tol` | `rel_tol` | Intended use |
|---|---|---|---|
| `exact` | 0 | 0 | Two runs of the same IR on the same engine version. Any difference is a bug. |
| `regression` | 1e-9 | 1e-12 | Same model, engine upgrade. Guards against reassociation drift. |
| `reconcile` (default for Prophet diffs) | 0.005 | 1e-6 | Money reconciliation to the half-cent. |
| `materiality` | 0.01 | 1e-4 | Sign-off level: "close enough to ship". |
| custom | user | user | `--abs 0.5 --rel 1e-5` |

Per-unit overrides, because a half-cent tolerance on a probability is nonsense:

```toml
[tolerance.reconcile]
abs = 0.005
rel = 1e-6
[tolerance.reconcile.by_unit]
prob   = { abs = 1e-9,  rel = 1e-9 }
factor = { abs = 1e-10, rel = 1e-10 }
count  = { abs = 1e-6,  rel = 1e-9 }
money  = { abs = 0.005, rel = 1e-6 }
```

Per-component overrides are permitted (`[tolerance.reconcile.by_component]`) but every one used is
printed in the diff header and listed in `diff.json` under `tolerance.overrides_applied` — a loosened
tolerance must never be invisible. `predictable diff --explain-tolerance` prints which rule matched
for any given cell.

**Rounding policy.** Distinct from tolerance and not a substitute for it:

1. The engine never rounds implicitly. Rounding is `round(x, dp)` in the IR, visible in the diff
   (IR §9.5). There is no global precision setting.
2. `round(x, dp)` is **round-half-away-from-zero** on the decimal representation, matching Prophet
   and actuarial convention, *not* IEEE round-half-even. This is a deliberate divergence from the
   `f64` default and is documented at the builtin. Implementation is `round_half_away(x * 10^dp) / 10^dp`
   with the scaling done in a way that avoids the `2.675 → 2.67` trap (compare against the shortest
   decimal representation, not the binary value).
3. Reporting rounds for *display only*, after comparison, never before.
4. When comparing against a Prophet `.rpt` whose values were themselves written at fixed precision,
   the reader records `report_precision` from the observed decimal places per column, and the diff
   automatically raises `abs_tol` for that component to `0.5 × 10^−report_precision`, emitting
   `H0402`-style note `"tolerance_raised_by_source_precision"`. This removes the most common class of
   phantom diff without the user ever loosening a tolerance by hand.

### 5.7 Terminal rendering

```
predictable diff run_prophet/ run/ --tolerance-profile reconcile

  a  prophet  TERM_BASE_2026Q2      run_prophet/          (30/06/2026)
  b  predictable 0.4.1              run/                  model 3f0a…  assumptions base
  tolerance  reconcile   abs 0.005  rel 1e-6              mapping migration/mapping.toml
  mapping adjustments    premium_income × 1000.0

  DIVERGED   4,122 of 19,680,000 cells   1 root divergence   3 outputs affected

  ▸ F001  root   renewal_expenses   affects bel, reserve, net_cashflow   97% of Δbel
      BEL diverges at t=7 in component renewal_expenses: 10.00 vs 10.31   (Δ 0.31, 3.1%)
      first at t=7, persists to t=40, all 10,000 modelpoints
      exemplar POL00042      worst POL09912 t=39  41.02 vs 44.87  (Δ 3.85, 9.4%)

      hypothesis H0201 (high, 32/32)
        b compounds expense_inflation from t=0 while a compounds from t=1.
        evidence: b/a = 1.0300 ± 1e-6 at every t ≥ 7.
        fix  model.pir:132
          -  compound(expense_inflation, t)
          +  compound(expense_inflation, t - 1)

      → predictable explain-diff run_prophet/ run/ -c renewal_expenses --mp POL00042 --t 7

  ▸ F002  inherited  bel  …
```

`--json` suppresses all of this and emits `diff.json` alone; the two must never both go to stdout.

---

## 6. Run manifests

### 6.1 Purpose

A result set without a manifest is not evidence. The manifest answers: *what exactly produced these
numbers, and can I get them again?* It is the join key for every comparison in this document.

### 6.2 Schema

`run/manifest.json`:

```jsonc
{
  "format": "pvf/1",
  "kind": "manifest",
  "manifest_digest": "sha256:3f0a…",       // computed over this doc with volatile fields excluded

  "run_id": "2026-08-17T09:14:22Z-3f0a",   // sortable, human-usable
  "system": "predictable",

  "versions": {
    "ir_version": "1.0",
    "engine_version": "0.4.1",
    "engine_git_sha": "a91c4de",
    "engine_build_profile": "release",
    "cli_version": "0.4.1",
    "dsl_version": "0.4.1"
  },

  "inputs": {
    "model": {
      "module": "term_assurance",
      "product": "TERM_UK",
      "digest": "sha256:9c11…",             // canonical text, all modules, path order (IR §9.6)
      "files": [
        {"path": "models/term_assurance/schema.pir", "digest": "sha256:71ab…"},
        {"path": "models/term_assurance/model.pir",  "digest": "sha256:2e04…"}
      ]
    },
    "assumptions": {"set": "base", "path": "assumptions/base.pir", "digest": "sha256:5d72…",
                    "values_digest": "sha256:8ab0…"},
    "modelpoints": {"path": "data/term.mpf.parquet", "digest": "sha256:c4f9…",
                    "rows": 10000, "key_field": "policy_number",
                    "source": {"system":"prophet","file":"data/TERM.MPF","digest":"sha256:aa10…"}},
    "tables": [
      {"name":"sa8990","path":"tables/sa8990.csv","resolver":"fs","digest":"sha256:9f2c…","rows":412,
       "declared_digest":"sha256:9f2c…","drift":false,
       "copy":"tables/sa8990.parquet","copy_digest":"sha256:b30d…"},
      {"name":"lapse_rates","path":"tables/lapses.csv","resolver":"fs","digest":"sha256:31aa…",
       "declared_digest":"sha256:31aa…","drift":false,
       "copy":"tables/lapse_rates.parquet","copy_digest":"sha256:2c81…"}
    ]
  },

  "timeline": {"basis":"annual","periods":40,"origin":"policy",
               "valuation_date":"2026-06-30","year_convention":"act/365"},

  "run_config": {
    "path": "runs/base.pir",
    "digest": "sha256:4be7…",             // run_digest — IR §8.4.2, excludes exec + out
    "emit": "all",
    "outputs": ["bel","reserve","net_cashflow","pv_premiums","pv_claims","pv_expenses"],
    "retain": "ring",
    "storage_precision": "f64",
    "on_trap": "abort",
    "max_errors": 100,
    "aggregations": ["by_product"],
    "solves": [],                          // typed by IR §8.4.4
    "exec": {"threads": 8, "chunk_size": 1024},   // excluded from run_digest
    "seed": null,
    "flags": {"allow_table_drift": false, "sum_kahan": false}
  },

  "lineage": {                             // IR §9.4.1
    "parent_run": null, "parent_manifest_digest": null,
    "group_id": null, "label": null, "varied": []
  },

  "environment_hash": "sha256:6b3e…",
  "environment": {
    "os": "macos", "os_version": "15.6", "arch": "aarch64",
    "cpu_features": ["neon"],
    "rustc": "1.83.0", "target_triple": "aarch64-apple-darwin",
    "float_settings": {"fma_contraction": false, "fast_math": false,
                       "reduction_order": "sequential_t"},
    "python": "3.12.7", "packages": {"predictable": "0.4.1", "pyarrow": "17.0.0"}
  },

  "results": {
    "path": "results.parquet", "digest": "sha256:e011…",
    "rows": 19680000, "components": 41,
    "component_set_digest": "sha256:aa47…",   // IR §8.4.3
    "aggregates": {"path": "aggregates.parquet", "digest": "sha256:77c2…"}
  },

  "execution": {
    "outcome": "completed",                 // completed | completed_with_traps | aborted | cancelled
    "started_at": "2026-08-17T09:14:22Z",
    "finished_at": "2026-08-17T09:14:29Z",
    "wall_ms": 6981,
    "modelpoints_projected": 10000,
    "modelpoints_trapped": 0,
    "traps": [],                            // IR §9.3.1 envelope, capped by --max-errors
    "warnings": [{"code":"W0102","count":3}]
  },

  "provenance": {
    "invocation": "predictable run models/term_assurance -a base -d data/term.mpf.parquet --emit all",
    "cwd_git": {"repo": "predictable-models", "sha": "e4c1907", "dirty": false},
    "user": null
  }
}
```

### 6.3 Rules

1. **`manifest_digest`** is SHA-256 over the canonical JSON of the manifest with
   `run_id`, `execution`, `results.digest`, `provenance.user`, and `manifest_digest` itself excluded.
   Two runs with the same `manifest_digest` **must** produce byte-identical `results.parquet`; CI
   asserts this across platforms in the golden corpus, and a violation is a P0 engine bug.
2. **`environment_hash`** is SHA-256 over the canonical JSON of `environment`. It is *not* part of
   `manifest_digest` — because a matching `manifest_digest` with a differing `environment_hash` is
   exactly the case the diff tool must call out first: *same inputs, different machine*. `predictable
   diff` prints `environment differs: aarch64-apple-darwin vs x86_64-unknown-linux-gnu` above the
   findings, so nobody spends an afternoon on a 1-ULP mystery.
3. **Ordering.** All arrays are sorted (files by path, tables by name, packages by name) and all keys
   appear in the order specified above, so manifests diff cleanly in git.
4. **`dirty: true`** in `cwd_git` is rendered as a warning banner in every report derived from the
   run. A dirty-tree run is not reproducible and the tooling says so rather than pretending.
5. **Prophet-imported runs** get a manifest with `"system": "prophet"`, populated `inputs.source`,
   and `versions`/`environment` reduced to what the `.rpt` header disclosed, with everything unknown
   explicitly `null` rather than omitted. Absent provenance is stated, never implied.
6. **`outcome` is load-bearing.** Any report derived from a run whose `outcome` is not `"completed"`
   carries a banner naming the outcome and the trapped/missing modelpoint count. `"aborted"` runs
   have a manifest and no `results.parquet`; `"cancelled"` runs have both, and their result set is
   explicitly partial (IR §9.3.1). A consumer that ignores `outcome` will eventually report on a
   truncated portfolio as though it were whole.
7. `predictable rerun run/manifest.json` re-executes from the manifest alone, verifying every digest
   before starting and refusing on any mismatch (`--allow-drift` to proceed, which stamps
   `"drift": true` into the new manifest permanently).

---

## 7. The verification CLI surface

The full loop, in the order an agent uses it:

```
predictable check <model>            --json      # IR §7 diagnostics
predictable run <model> -a <set> -d <mpf>        # → run/ with manifest + results
predictable prophet rpt <file.rpt> -o run_prophet/
predictable diff run_prophet/ run/ --json diff.json --tolerance-profile reconcile
predictable explain-diff run_prophet/ run/ -c <comp> --mp <key> --t <t> --json
predictable explain run/ -c <comp> --mp <key> --t <t> --json
predictable coverage migration/                  # prophet variables with no component
```

Every one of these accepts `--json` and writes a `pvf/1` document. Every one exits `0` on success,
`1` on a domain failure (diagnostics/divergence), `2` on a usage or structural failure. An agent can
therefore drive the whole loop on exit codes and JSON without parsing prose.

---

## 8. The published agent skill and `llms.txt`

### 8.1 Why this is in the verify spec

The migration agent is not a demo script; it is a *client of the verify layer*, and its instructions
are a normative statement of how that layer is meant to be used. If the skill needs to explain a
workaround, the verify layer has a defect.

### 8.2 `predictable-migration` — Claude Code agent skill

Published to the repo at `skills/predictable-migration/` and to the plugin marketplace.

```
skills/predictable-migration/
  SKILL.md                    # the loop; < 500 lines, loaded on trigger
  references/
    ir-cheatsheet.md          # condensed IR §2-§5: shapes, units, timing, grammar, builtins
    prophet-idioms.md         # Prophet construct → IR construct table
    diagnostics.md            # E/W/P/N/H code → meaning → standard fix
    verify-formats.md         # trace, rundiff, manifest schemas (this doc, condensed)
  scripts/
    loop.py                   # check → run → diff → report; one command, JSON out
```

`SKILL.md` frontmatter:

```yaml
name: predictable-migration
description: >
  Migrate an FIS Prophet library (.MPF, .FAC, .RPT, workspace source) into a predictable model
  and reconcile it to the Prophet run. Use when the user mentions Prophet, MPF, .fac, .rpt,
  actuarial model migration, or asks to reimplement/validate a life insurance projection model.
allowed-tools: Read, Write, Edit, Bash, Grep, Glob
```

The body is an explicit procedure, in this order:

1. **Inventory.** `predictable prophet import` over the workspace. Read `migration/coverage.json`.
   Never start writing `.pir` before the variable inventory exists.
2. **Schema first.** Emit `schema.pir`: modelpoint fields, enums, assumptions, tables. Run
   `predictable check` before writing a single formula. *Rationale stated in the skill:* schema
   errors surface as resolution errors in every subsequent formula and drown the signal.
3. **Units and timing are a decision, not a default.** For each numeric field and component, choose
   `unit` and `timing` explicitly. The skill contains the Prophet-timing → IR-timing table and the
   instruction that `/12` on an annual rate is an error, not a translation (IR §5).
4. **Translate leaves upward.** Decrements, then in-force, then cashflows, then discounting, then
   reserves — checking after each layer. Every component gets `meta.source` naming the Prophet
   variable and file:line (IR §11.1); the coverage report is the completion criterion.
5. **Run and diff.** `--emit all`, `--tolerance-profile reconcile`.
6. **Work the findings top-down.** Only `class = "root"` findings are actionable; inherited findings
   disappear when their root is fixed. Apply `suggested_edit` when confidence is `high`, verify with
   `explain-diff` when it is not. Re-run. **Never widen a tolerance to make a finding go away** — the
   skill states this as a hard prohibition and instructs the agent to surface the trade-off to the
   user instead.
7. **Stop condition.** `verdict: "matched"` at `reconcile`, coverage report showing no unmapped
   Prophet output variables, and `predictable check` clean of errors and of `W0102` unit lints.
8. **Hand back** a migration report: coverage, remaining differences with justification, and the
   `mapping.toml` with every `sign`/`scale`/`timing_shift` adjustment listed for human sign-off.

The skill's own success metric — quoted in it — is: *a Prophet library of ~50 variables reconciles to
`reconcile` tolerance in under an hour of agent time with no human edit to the generated `.pir`.*

### 8.3 `llms.txt`

Served at `predictable.dev/llms.txt`, and committed at the repo root. Follows the llms.txt
convention: H1, a blockquote summary, then link sections with one-line annotations.

```markdown
# predictable

> An open-source (MIT) actuarial modelling framework for life insurance cashflow projection.
> A Python authoring DSL compiles to a serialisable, git-diffable computation-graph IR that a Rust
> engine executes. Designed so an LLM can read a Prophet model and reimplement it in predictable
> in minutes, with a machine-readable verification loop: definition-time diagnostics, provenance
> traces, and a structured run-diff against Prophet .rpt output.

## Start here
- [IR specification](/docs/design/01-ir.md): the normative format an LLM should write. Shapes,
  units, timing, expression grammar, builtins. Read this before writing any model.
- [Verification loop](/docs/design/04-verify.md): explain() traces, Prophet readers, run-diff
  format, tolerance policy, run manifests. Read this before interpreting any diff output.
- [Quickstart](/docs/quickstart.md): a complete term assurance model in 60 lines.

## Writing models
- [IR cheatsheet](/docs/llm/ir-cheatsheet.md): the whole grammar and builtin set on one page.
- [Worked models](/docs/llm/models.md): term assurance, annuity, unit-linked, IFRS 17 GMM — full
  .pir sources, each one runnable and reconciled.
- [Diagnostics index](/docs/llm/diagnostics.md): every E/W/P/N/H code, its meaning, and its fix.

## Prophet migration
- [Migration guide](/docs/llm/prophet-migration.md): the loop, end to end.
- [Prophet idiom table](/docs/llm/prophet-idioms.md): Prophet construct → IR construct.
- [Agent skill](/skills/predictable-migration/SKILL.md): the packaged procedure.

## Reference
- [CLI](/docs/cli.md), [Python API](/docs/api.md), [Result & trace JSON schemas](/docs/schemas/)

## Optional
- [Architecture](/docs/design/00-architecture.md), [Benchmarks](/docs/benchmarks.md),
  [Contributing](/CONTRIBUTING.md)
```

`llms-full.txt` is generated: the IR spec, this spec, the cheatsheet, the diagnostics index, and the
worked models concatenated, so a single fetch primes a model completely. It is built in CI from the
same sources, and CI fails if any link in `llms.txt` 404s or if `llms-full.txt` exceeds 200k tokens.

Every diagnostic's `doc_url` (IR §7) points into `/docs/llm/diagnostics.md#<code>`, so an agent that
hits an unfamiliar code has a one-fetch path to its meaning and fix. That anchor discipline is
enforced in CI: every code emitted anywhere in the codebase must have a section in the index.

---

## 9. Testing the verify layer

The verify layer needs its own regression discipline, or it becomes the thing nobody trusts.

1. **Golden traces.** For the example corpus, `trace.text` and `trace.json` for a fixed set of
   (component, mp, t) are committed and diffed in CI. Deterministic rendering (§3.5) makes this
   viable.
2. **Mutation testing for the differ.** Take a reconciled model, apply a catalogue of seeded
   mutations (drop a `-1`, flip a timing tag, change a `clamp` to `exact`, insert a `/12`), run the
   diff, and assert that (a) the correct component is reported as the root divergence, (b) the
   correct `t_first` is found, and (c) the intended hypothesis code fires. **This is the primary
   quality metric for the whole project**: `root-cause hit rate` over the mutation catalogue, tracked
   in CI and published in the README. A differ that finds the right component 95% of the time is a
   different product from one that finds it 60% of the time.
3. **Reader fuzzing.** MPF/fac/rpt readers are fuzzed on truncation, encoding corruption, and row
   ragging; the invariant is *never panic, always a `P0xxx` diagnostic with a byte offset*.
4. **Round-trip corpus.** Prophet sample files (permissively-licensed / synthetic) with known-correct
   parsed forms, committed as fixtures.
5. **Cross-platform determinism.** The manifest rule in §6.3.1 is asserted on macOS/aarch64,
   Linux/x86-64, and wasm32 in CI.

---

## IR feedback

Things this layer needs that `01-ir.md` does not currently specify. None of these are divergences —
they are gaps I have designed around and would like the IR spec to close.

**Settled in IR 1.0** (raised here, now normative in `01-ir.md`):

- Component identity is `<module_path>.<name>`, dot-separated, globally unique, normative as the join
  key in results, traces, diffs and mapping files (IR §2.2).
- `stage: 1 | 2` is an explicit computed component field (IR §2.2), carried in the results schema.
- `round(x, dp)` is round-half-away-from-zero on the shortest decimal representation (IR §2.8); §5.6
  below is the implementation guidance for it, not the decision.
- `meta.display = { dp, scale }` exists, is non-semantic, and is excluded from `model_digest`
  (IR §11.1).
- IR §11 now cross-references this document as the normative home of the results schema, the trace
  schema, `RunDiffDoc` and the manifest.
- Enums are unordered and compare by equality only; declaration order fixes the dictionary encoding
  and nothing else (IR §2.9).

**Also settled (IR decision log Q1–Q15, `01-ir.md` §13):**

- **Traps** (Q7). `E0902` with a fixed JSON envelope (`trap` kind, component, `expr_path`, span,
  `mp_key`, `mp_row`, `t`, operands). `on_trap = "abort"` is the default: manifest written, no
  `results.parquet`, exit 2. `on_trap = "continue"` drops the modelpoint entirely — **no rows, not
  null rows**, correcting this layer's earlier assumption — and exits 1. `manifest.execution.outcome`
  distinguishes completed / completed_with_traps / aborted / cancelled (IR §9.3.1).
- **`[[solve]]` manifest contract** (Q8). `run_config.solves[]` is typed by IR §8.4.4: converged and
  not-converged counts, iteration stats, max residual and its modelpoint, `solves/<name>.parquet` for
  `scope = "per_mp"`, and the solved value also materialised as a `PerMP` component in the results.
- **`Agg` term provenance** (Q11). The recording evaluator **must** retain per-`t` `terms` for every
  `Agg` node, with the applied discount factor and `timing_used` for `npv`, summing exactly to the
  node's value; `--trace-max-terms` truncates loudly (IR §11.2). §3.2's `Agg.terms` is now required,
  not optional.
- **`emit` mismatch** (Q6). `emit` participates in `run_digest`, result sets carry
  `results.component_set_digest`, and `emit_mismatch` is a first-class condition in `diff.json`:
  compare the intersection, list the one-sided components, exit on the intersection's tolerance
  outcome, never `exit 2` unless `--require-same-emit` (IR §8.4.3).
- **Nulls** (Q3), **lineage** (Q12) and **table copies** (Q13) also land here: no runtime nulls, a
  `lineage` block on every manifest, and `inputs.tables[].copy` carrying table content with the run.

**Still open:** nothing. All questions raised by this layer are closed in `01-ir.md` §13.
