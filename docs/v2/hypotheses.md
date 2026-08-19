# Hypotheses and the mutation harness

[`diff run`](run-diff.md) says *what* diverges: which component, at which `t`, on which modelpoint,
and whether the divergence starts there or was inherited. This page is about the next sentence —
**why it probably diverges**, and **the edit that would test the proposal**.

Every finding in `diff.json` carries a `hypotheses` array. Each entry is a pattern detector that
fired, the evidence that fired it, how far that evidence generalises across the population, and —
where the model source anchors one — a literal `{file, byte_start, byte_end, old, new}` edit.

Normative source: [`04-verify.md` §5.5 and §9.2](../design/04-verify.md).

## Why propose at all

A reconciliation loop is: read the diff, form a theory, edit, re-run, diff again. The theory step is
the slow one, and it is also the most mechanical: after the tenth migration, "the ratio is exactly
`1/12` on every period" means one thing and not many. Detecting that is cheap, saying it is cheap,
and being wrong is cheap too — the reader can falsify a bad proposal in a single re-run.

What is *not* cheap is a silent omission. A hypothesis nobody made cannot be falsified. So
low-confidence hypotheses are still emitted, with their support counts attached, rather than
suppressed for tidiness.

## The detectors

| Code | Fires when | Proposes |
|---|---|---|
| `H0101` | `b/a` is constant across `t` to 1e-9 | a scale factor; the ratio is checked against `12`, `1/12`, `1000`, `100`, `(1 + r)`, `(1 + r)^(1/12)` and `(1 + r)^0.5` for every assumption `r` in the model |
| `H0102` | `b − a` is constant | an additive term present on one side only |
| `H0103` | `b ≈ −a` | a sign convention; `sign = -1` in `mapping.toml` |
| `H0201` | `b[t] ≈ a[t±1]` everywhere compared | a timing shift; `timing_shift`, `retime` or `period_base` |
| `H0202` | the constant ratio is exactly `(1 + i)` or `(1 + i)^0.5` on an `npv` | a `start`/`mid`/`end` mismatch; names the timing tag of the discounted series |
| `H0301` | the divergence begins in the exact period a lookup key leaves the table's range, or lands on a row the table does not have | a `policy` mismatch, citing the `N0101`/`N0102` note the trace would show |
| `H0302` | one side is zero where the other is not, from `t = k` onward | an indicator or term expiry off by one, naming the input component whose own last non-zero `t` differs |
| `H0303` | only the modelpoints sharing one field value diverge | a segment-specific rate or a missing branch, naming the discriminating field and value |
| `H0401` | the ratio is `(r/12) ÷ ((1+r)^(1/12) − 1)` for an assumption `r` | the Prophet `/12` idiom — one side divides the annual rate, the other compounds it |
| `H0402` | every `|Δ|` is within `0.5 × 10⁻ᵈᵖ` and one side sits on that decimal grid | rounding: raise the tolerance, do not edit the model |
| `H0501` | a `NaN` or an infinity on either side | a trap. Never a tolerance question, and never suppressed by a profile |

Precedence is real, not cosmetic: when `H0401`, `H0202` or `H0103` explains a ratio, the generic
`H0101` stands down, because "the ratio is `1.0159…`" is a worse sentence than "one side divides the
annual rate by twelve".

## Worked example 1 — a stray `/12`

Two runs of [`models/term_annual`](reference-models.md), identical except that side b's
`renewal_expenses` acquired a `/ 12`:

```console
$ predictable diff run run_a run_b --top 1

  DIVERGED   1,575 of 7,325 cells   1 root divergence   6 outputs affected

  ▸ F001  root   renewal_expenses   affects bel, net_cashflow, pv_expenses, reserve   12% of Δreserve
      RESERVE diverges at t=0 in component renewal_expenses: 40.80 vs 3.40  (Δ 37.40, 91.7%)  [mp TA00001]
      first at t=0, persists to t=29, 25 of 25 modelpoints, 500 cells
      explained by a model change: formula
      H0101  (high confidence, held on 24/24)  b/a is constant at 0.083333333 across all 10 periods
             of `renewal_expenses` — that is 1/12: a scale factor, not a behavioural difference
          suggested edit  b/build/model.pir:2956..3062  `renewal_expenses` on side b is
                          0.0833… times side a's; * 12 restores it
          - "renewal_expense_pa / 12 * compound(expense_inflation, t) * expense_scale * num_pols_if * in_force_factor"
          + "(renewal_expense_pa / 12 * compound(expense_inflation, t) * expense_scale * num_pols_if * in_force_factor) * 12"
```

The same finding under `--json`:

```json
{
  "code": "H0101",
  "confidence": "high",
  "evidence": {
    "ratio": 0.08333333333333334,
    "matches": { "name": "1/12", "value": 0.08333333333333333 },
    "t_tested": 10
  },
  "evidence_support": { "tested": 24, "held": 24 },
  "message": "b/a is constant at 0.083333333 across all 10 periods of `renewal_expenses` — that is 1/12: …",
  "suggested_edit": {
    "file": "b/build/model.pir",
    "byte_start": 2956,
    "byte_end": 3062,
    "old": "\"renewal_expense_pa / 12 * compound(expense_inflation, t) * …\"",
    "new": "\"(renewal_expense_pa / 12 * compound(expense_inflation, t) * …) * 12\"",
    "message": "`renewal_expenses` on side b is 0.0833… times side a's; * 12 restores it"
  }
}
```

## Worked example 2 — the Prophet `/12` idiom

[`models/term_monthly`](reference-models.md) converts the annual valuation rate by compounding.
Side b converts it by dividing — spelled `nominal_to_periodic(valuation_rate, 12)`, because a bare
`/ 12` on a `rate(annual)` is [`E1402`](../llm/diagnostics.md) precisely so that this choice shows up
in a diff.

Run both sides with `emit = "all"` (`predictable run … --retain-all`), which is the migration mode:
with only the outputs emitted, the earliest root the result set *can* name is a `pv_*` aggregate,
and the mistake is four components upstream of it.

```console
$ predictable diff run run_a run_b --top 1 --tolerance-profile regression

  DIVERGED   18,175 of 180,625 cells   1 root divergence   6 outputs affected

  ▸ F001  root   monthly_valuation_rate   affects bel, profit_margin, pv_claims, pv_expenses, …
      RESERVE diverges at t=-1 in component monthly_valuation_rate: 0.00 vs 0.00  (Δ 0.00, 1.6%)
      explained by a model change: formula
      H0401  (high confidence, held on 24/24)  b/a is 1.015942028 =
             (valuation_rate/12) / ((1+valuation_rate)^(1/12) - 1): the Prophet `/12` idiom —
             one side divides the annual rate by twelve where the other compounds it
          suggested edit  b/build/model.pir:1458..1499  compound `valuation_rate` to the monthly
                          basis, the way side a does
          - "nominal_to_periodic(valuation_rate, 12)"
          + "(pow(1 + valuation_rate, 0.08333333333333333) - 1)"
```

`monthly_valuation_rate` is a `PerMP` value: it has one cell per modelpoint and no series at all. Its
divergence vector is therefore taken *across the population* rather than across `t`, which is why a
constant-ratio claim about it means anything.

## Confidence and support

A detector runs on the exemplar modelpoint first, because that is cheap. If it fires, it is re-run on
a sample of up to **32 further diverging modelpoints**, and support counts the ones that produced the
*same* claim — not merely "fired again". A ratio of `1/12` here and `0.97` there are two claims, and
counting them as one would inflate confidence exactly where it should collapse.

| `evidence_support` | `confidence` |
|---|---|
| held on every modelpoint tested | `high` |
| held on ≥ 28 of 32 (or the same proportion of a smaller sample) | `medium` |
| fewer | `low`, still emitted |

A claim that fails on **more than half** the sample is dropped: at that point it is a fact about the
exemplar, and asserting it of the component would be false rather than uncertain.

The sample is deterministic — the diverging modelpoints in `mp_row` order, evenly spaced. "Random"
in the spec means "not cherry-picked"; a seeded shuffle would make two runs of the same diff report
different support, which the determinism rule does not allow.

## Suggested edits

`suggested_edit` is the same shape the checker emits (IR §7), so **one code path in an agent applies
edits from both**. Two guarantees make it safe to apply mechanically:

1. **`old` is the bytes that are there.** Every edit is built by reading `text[byte_start..byte_end]`
   from the file on disk, and `still_applies()` re-checks it before an edit is applied. An edit
   proposed against a file that has since changed fails loudly instead of corrupting it.
2. **The anchor comes from the parser.** Spans are taken from the `.pir` parser, so `expr = "a * b"`
   is anchored at its own value token — not at the first place that text happens to appear, which
   might be a `doc` string three components later.

Edits land in **side b's** source (the predictable side of a reconciliation; side a is often a
Prophet run with no `.pir` at all) and are written to make b reproduce a. Not every hypothesis
proposes one: `H0402` deliberately proposes a tolerance rather than an edit, and `H0103` and `H0201`
propose `mapping.toml` settings, which are a declaration about the *comparison*, not about the model.

When the model source is unavailable — no `.pir` beside the run, or a file whose digest no longer
matches the one that ran — the hypothesis is still made, and simply carries no edit. A missing anchor
costs a suggestion; it never costs a hypothesis.

## The mutation harness

[`04-verify.md` §9.2](../design/04-verify.md) makes one number the primary quality metric for the
whole project:

> Take a reconciled model, apply a catalogue of seeded mutations, run the diff, and assert that (a)
> the correct component is reported as the root divergence, (b) the correct `t_first` is found, and
> (c) the intended hypothesis code fires. **This is the primary quality metric for the whole
> project**: `root-cause hit rate`.

The harness lives in `predictable_rundiff::mutation`. Each catalogue entry is a literal edit to a
copy of a committed reference model. Both sides are then run by the same CLI a user runs, and the
two run directories are diffed:

```console
$ cargo run -p predictable-rundiff --example mutation_report
$ cargo test -p predictable-rundiff --test mutation_harness
```

Two decisions keep the measurement honest:

- **Two copies, not one edited in place.** The diff verifies each side's recorded model digest before
  it classifies from it, so side a's `.pir` has to still be side a's `.pir`.
- **Both sides run with `emit = "all"`.** With only outputs emitted, "which component is the root" is
  always answered "the earliest output downstream of the mistake" — a fact about the emit setting
  rather than about the differ.

The catalogue is deliberately boring. Every entry is a mistake somebody has actually made: a dropped
lag, an annual rate divided by twelve, a term test that includes its last anniversary, a report
written to two decimal places, a table whose middle rows are missing.

### Published hit rate

Regenerated by `cargo run -p predictable-rundiff --example mutation_report`, and asserted by the test
of the same name, so the table cannot drift from the tests.

| mutation | kind | root component | `t_first` | hypothesis |
|---|---|---|---|---|
| `TA-lag-dropped` | lag | yes | yes | — |
| `TA-scale-twelfth` | scale | yes | yes | yes |
| `TA-offset-added` | offset | yes | yes | yes |
| `TA-sign-flipped` | sign | yes | yes | yes |
| `TA-timing-flipped` | timing | yes | yes | yes |
| `TA-term-off-by-one` | expiry | yes | yes | yes |
| `TA-rounded-premium` | rounding | yes | yes | yes |
| `TA-table-truncated` | table policy | yes | yes | yes |
| `TM-prophet-twelfth-rate` | rate conversion | yes | yes | yes |
| `TM-prophet-twelfth-inflation` | rate conversion | yes | yes | yes |
| `TM-term-off-by-one` | expiry | yes | yes | yes |
| `TM-lag-dropped` | lag | yes | yes | — |
| `SM-maturity-off-by-one` | expiry | yes | yes | — |
| `SM-amc-twelfth` | scale | yes | yes | yes |

**Root-cause hit rate: 14/14 = 100%.** Intended hypothesis fired on 11/11.

| mistake | found |
|---|---|
| lag | 2/2 |
| timing | 1/1 |
| scale | 2/2 |
| offset | 1/1 |
| sign | 1/1 |
| rate conversion | 2/2 |
| table policy | 1/1 |
| expiry | 3/3 |
| rounding | 1/1 |

### What the number does and does not mean

14 out of 14 on a 14-entry catalogue over three reference models is a statement about *this*
catalogue: single mutations, one at a time, on models the project wrote. It is a regression gate, not
a claim about migrations in general — two interacting mistakes on somebody else's model is a harder
problem, and the honest way to keep the number meaningful is to grow the catalogue until it stops
being 100%. Every entry that is added stays added.

The three `—` rows are mutations for which the catalogue names no expected hypothesis: a dropped lag
produces a divergence whose shape is neither a constant ratio nor a constant offset, and inventing a
code so the column would read `yes` is exactly the kind of thing this table exists to prevent.

## Reading it from Rust

```rust
use predictable_rundiff::{diff_runs, DiffOptions, RunSide};

let a = RunSide::load("a", "run_a")?;
let b = RunSide::load("b", "run_b")?;
let opts = DiffOptions::default().with_models_from(&a, &b);
let diff = diff_runs(&a, &b, &opts);

for finding in &diff.findings {
    for h in &finding.hypotheses {
        println!("{} {} ({})", finding.component, h.code, h.confidence.word());
        if let Some(edit) = &h.suggested_edit {
            let text = std::fs::read_to_string(&edit.file)?;
            if let Some(fixed) = edit.apply(&text) {
                std::fs::write(&edit.file, fixed)?;
            }
        }
    }
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

`with_models_from` is what loads the source index: without a model on side b there are no anchors,
and therefore no edits.
