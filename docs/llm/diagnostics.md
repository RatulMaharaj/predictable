# Diagnostics catalogue

Every message predictable can produce — a checker error, a lint, a trace note, a Prophet reader
observation, a run-diff hypothesis — carries a **code**, and every code has an entry on this page.
That is enforced by a test in `predictable-diagnostics`: a code with no anchor here fails the build,
so the `doc_url` on any diagnostic you receive always resolves.

If you arrived here from a `doc_url`, jump straight to your code with the table of contents, or with
the anchor: `https://predictable.dev/llm/diagnostics/#E0201`.

## The shape of a diagnostic

Both renderings — terminal text and `--json` — come from one value:

```json
{
  "code": "E0201",
  "severity": "error",
  "message": "cyclic dependency in the same period",
  "spans": [
    {"file": "term.pir", "start": 41, "end": 44, "primary": true,
     "label": "reserve reads bel at time t"},
    {"file": "term.pir", "start": 90, "end": 97, "primary": false,
     "label": "bel reads reserve at time t"}
  ],
  "suggestions": [
    {"message": "read last period's value",
     "edits": [{"file": "term.pir", "start": 90, "end": 97, "replacement": "reserve[t-1]"}],
     "applicability": "maybe_incorrect"}
  ],
  "doc_url": "https://predictable.dev/llm/diagnostics/#E0201"
}
```

| Field | Meaning |
|---|---|
| `code` | The registry code. Stable: a code is never reused for a different rule. |
| `severity` | `error` (blocks the run), `warning` (a lint), `info` (a note or hypothesis). |
| `message` | One line, lower case, no trailing period. |
| `spans` | Byte ranges into named source files. Exactly one is normally `primary`. |
| `suggestions` | Named fixes, each a list of literal byte-range replacements. |
| `doc_url` | This page, anchored on the code. |
| `notes` | Optional prose paragraphs: why the rule exists. Omitted when empty. |

**Offsets are byte offsets**, not character or column positions, in spans *and* in edits. That is
what lets a tool apply a fix without re-deriving positions. Line and column are a rendering
convenience; add them with `Diagnostic::resolve_positions` if you want them in the JSON.

## Reading the terminal form

```text
error[E0201]: cyclic dependency in the same period
 --> term.pir:5:9
  |
5 | expr = "bel * 1.05"
  |         ^^^ reserve reads bel at time t
  |
  = note: Cycles across periods are fine; add a time lag.
help: read last period's value
  |
5 | expr = "bel[t-1] * 1.05"
  |            +++++
```

In colour mode the `error[E0201]` prefix is an OSC 8 hyperlink to this page. Plain mode — what you
get when output is piped, and what golden tests compare — is free of escape sequences, so rendered
diagnostics can be committed as fixtures.

## Applying a suggested edit

Suggestions are *literal*: replacement text plus a byte range, never prose describing a change. An
agent in the migration loop can apply them and re-check without a human in the middle.

```rust
use predictable_diagnostics::{apply_edits, from_json};

let diagnostics = from_json(&json)?;
let mut source = std::fs::read_to_string("term.pir")?;
for diag in &diagnostics {
    for suggestion in &diag.suggestions {
        if suggestion.applicability == predictable_diagnostics::Applicability::MachineApplicable {
            source = apply_edits(&source, &suggestion.edits)?;
        }
    }
}
```

Three rules make that safe:

1. **Apply one diagnostic's edits, then re-run the check.** Offsets are relative to the source the
   diagnostic was produced against; applying a second diagnostic's edits to already-patched text
   will land in the wrong place.
2. **Edits within one suggestion never overlap.** `apply_edits` sorts them and returns
   `EditError::Overlapping` rather than picking a winner, so a bad emitter fails loudly.
3. **Check `applicability` first.** `machine_applicable` is safe to apply unattended;
   `maybe_incorrect` and `has_placeholders` need a human, and `unspecified` is illustration only.

## Severities and exit codes

`predictable check` and `predictable run` map the batch onto a process exit code:

| Worst severity present | Exit code |
|---|---|
| none, or `info` only | 0 |
| `warning` | 1 |
| `error` | 2 |

A lint can be promoted to an error (`--deny W0103`) without changing its code, so a code always
means the same rule regardless of how strictly a project enforces it.

## Namespaces

The first two characters say which layer produced the diagnostic, which is the fastest way to know
which file to open.

| Prefix | Layer | Emitted by |
|---|---|---|
| `E0` | IR / checker errors | `predictable check`, parsing and the six checker passes |
| `W0` | IR / checker lints | the checker's lint pass, the planner |
| `E1` | Python DSL errors | `predictable build`, at trace time |
| `W1` | Python DSL lints | `predictable build` |
| `P0` | Prophet readers | `.MPF`, `.fac` and `.rpt` parsing during migration |
| `N0` | Trace notes | `explain()` — provenance, not problems |
| `H0` | Run-diff hypotheses | `predictable diff` — proposed causes of a divergence |

`N0xxx` and `H0xxx` are *not* faults. A trace note records that a lookup key was clamped or that an
`init` was used at `t = 0`; a hypothesis proposes why two runs disagree. Both are `info` and neither
affects an exit code.

## The catalogue

Entries below are generated from the registry in `predictable-diagnostics`, so the titles and codes
on this page and in the binary cannot drift apart.

<!-- BEGIN GENERATED CODES -->

## IR and checker errors (E0xxx)

<a id="E0000"></a>
### `E0000` — an error arrived without a registry code

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="E0001"></a>
### `E0001` — unexpected character

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0002"></a>
### `E0002` — unterminated string

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0003"></a>
### `E0003` — expected a key

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0004"></a>
### `E0004` — expected `=` after a key

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0005"></a>
### `E0005` — duplicate key

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0006"></a>
### `E0006` — unexpected trailing input on this line

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0007"></a>
### `E0007` — expected a `[section]` header

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0008"></a>
### `E0008` — expected a name in the section header

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0009"></a>
### `E0009` — unclosed section header

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0010"></a>
### `E0010` — integer literal does not fit in i64

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0011"></a>
### `E0011` — a bare word is not a value

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0012"></a>
### `E0012` — expected a value

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0013"></a>
### `E0013` — unclosed array

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0014"></a>
### `E0014` — unclosed inline table

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0020"></a>
### `E0020` — expected a value in an expression

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0021"></a>
### `E0021` — unexpected character in an expression

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0022"></a>
### `E0022` — trailing input after the expression

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0023"></a>
### `E0023` — expected `then` after the `if` condition

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0024"></a>
### `E0024` — expected `else`: both arms of an `if` are required

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0025"></a>
### `E0025` — comparisons do not chain

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0026"></a>
### `E0026` — unclosed `(`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0027"></a>
### `E0027` — unclosed argument list

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4.2

<a id="E0028"></a>
### `E0028` — expected `(` after the lookup sigil

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.9

<a id="E0029"></a>
### `E0029` — `x[t-0]` is not a lag

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.7

<a id="E0030"></a>
### `E0030` — forward references are not expressible

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.7

<a id="E0031"></a>
### `E0031` — an index must be `t`, `t-k` or a non-negative integer

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.7

<a id="E0032"></a>
### `E0032` — unclosed index

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.7

<a id="E0041"></a>
### `E0041` — a required key is missing

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2

<a id="E0042"></a>
### `E0042` — a key has the wrong type

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2

<a id="E0043"></a>
### `E0043` — a key's value is not one of the legal choices

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2

<a id="E0044"></a>
### `E0044` — unknown key for this block

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2

<a id="E0045"></a>
### `E0045` — unknown section

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0046"></a>
### `E0046` — `[run.exec]` without a `[run]` block

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4

<a id="E0047"></a>
### `E0047` — duplicate block

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §4

<a id="E0048"></a>
### `E0048` — a non-Series component has no timing

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.5

<a id="E0049"></a>
### `E0049` — a component has no `expr`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.2

<a id="E0050"></a>
### `E0050` — an optional modelpoint field has no `default`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.10

<a id="E0051"></a>
### `E0051` — an enum has no values

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.10

<a id="E0052"></a>
### `E0052` — `rows` is only legal on an inline table

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.9

<a id="E0053"></a>
### `E0053` — `periods` must not be negative

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §5

<a id="E0101"></a>
### `E0101` — a run config may not override a timeline field

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4.2

<a id="E0105"></a>
### `E0105` — module imported but not listed in `product.modules`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4.1

<a id="E0106"></a>
### `E0106` — `emit_list` names a component that does not resolve

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4.3

<a id="E0107"></a>
### `E0107` — `product.outputs` disagrees with the `Output` components

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4.1

<a id="E0108"></a>
### `E0108` — `storage_precision = "f32"` is not supported in IR 1.0

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4.5

<a id="E0201"></a>
### `E0201` — cyclic dependency in the same period

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §3.1

<a id="E0202"></a>
### `E0202` — cyclic dependency through `init`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §3.1

<a id="E0203"></a>
### `E0203` — cannot find a name in this model

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="E0204"></a>
### `E0204` — the same name is defined in two modules

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.2

<a id="E0205"></a>
### `E0205` — absolute period index beyond the projection horizon

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.7

<a id="E0301"></a>
### `E0301` — shape narrowing without an aggregate

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="E0305"></a>
### `E0305` — `clamp` / `step` / `interpolate` on an enum lookup key

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.9

<a id="E0402"></a>
### `E0402` — aggregation `group_by` key is a `Series`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.3

<a id="E0403"></a>
### `E0403` — aggregation `group_by` key is `f64`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.3

<a id="E0404"></a>
### `E0404` — `over_t` given for a `PerMP` measure

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.3

<a id="E0501"></a>
### `E0501` — incompatible units in an addition

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.4

<a id="E0502"></a>
### `E0502` — rates on different bases cannot be added

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.4

<a id="E0503"></a>
### `E0503` — `money * money` is not a unit

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.4

<a id="E0504"></a>
### `E0504` — wrong number of lookup keys

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.9

<a id="E0505"></a>
### `E0505` — lookup key dtype does not match the table

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.9

<a id="E0506"></a>
### `E0506` — `timing` on a component that is not a `Series`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.5

<a id="E0507"></a>
### `E0507` — a `Series` component with no `timing`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.5

<a id="E0508"></a>
### `E0508` — unknown timing tag

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.5

<a id="E0509"></a>
### `E0509` — a `Lag` on a dtype with no zero and no `init`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.7

<a id="E0602"></a>
### `E0602` — `is_null` / `coalesce` on a value that cannot be missing

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.11

<a id="E0801"></a>
### `E0801` — table `source` escapes the project root

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.9.1

<a id="E0901"></a>
### `E0901` — trace replay diverged from the recorded run

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="E0902"></a>
### `E0902` — arithmetic trap during projection

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §9.3.1

<a id="E0903"></a>
### `E0903` — solve did not converge

**Severity:** error &nbsp;·&nbsp; **Specified in:** 01-ir.md §8.4.4


## IR lints (W0xxx)

<a id="W0101"></a>
### `W0101` — component declared but never read

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="W0102"></a>
### `W0102` — `unit = "none"` in money arithmetic

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="W0103"></a>
### `W0103` — timing mismatch in a sum

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="W0104"></a>
### `W0104` — `Series` constant across modelpoints and time

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 01-ir.md §7

<a id="W0105"></a>
### `W0105` — timing operation applied to an untimed value

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 01-ir.md §2.5

<a id="W0110"></a>
### `W0110` — model exceeds three stage-2 substage levels

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 03-engine.md §5.3


## Python DSL errors (E1xxx)

<a id="E1101"></a>
### `E1101` — missing return annotation (no dtype or unit)

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1102"></a>
### `E1102` — `@series` without `timing`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1103"></a>
### `E1103` — unknown parameter name

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1104"></a>
### `E1104` — parameter annotation contradicts the declaration

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1105"></a>
### `E1105` — ambiguous name across namespaces

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1106"></a>
### `E1106` — table used but not in the signature (or vice versa)

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1201"></a>
### `E1201` — Python `if` / `and` / `or` / `not` on a traced value

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1202"></a>
### `E1202` — unlagged self-reference

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1203"></a>
### `E1203` — non-constant lag index

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1204"></a>
### `E1204` — forward time reference

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1205"></a>
### `E1205` — loop or comprehension over a traced value

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1206"></a>
### `E1206` — call to a non-builtin function on a traced value

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1207"></a>
### `E1207` — stage-2 value read in `expr` rather than `init`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1301"></a>
### `E1301` — modelpoint schema has zero or multiple `key()` fields

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1302"></a>
### `E1302` — modelpoint object accessed as a value

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1401"></a>
### `E1401` — redefinition of a timeline input

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1402"></a>
### `E1402` — arithmetic basis conversion on a `Rate`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1501"></a>
### `E1501` — shadowing an inherited component without `@override`

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1502"></a>
### `E1502` — `@override` changes shape, dtype, unit or timing

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1503"></a>
### `E1503` — unimplemented `@abstract` component in a product

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1504"></a>
### `E1504` — override of a `@final` component

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="E1601"></a>
### `E1601` — `product(outputs=...)` disagrees with the `output=True` components

**Severity:** error &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10


## Python DSL lints (W1xxx)

<a id="W1101"></a>
### `W1101` — component declared but never read and not `output=True`

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="W1102"></a>
### `W1102` — `Num` (unitless) in money arithmetic

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10

<a id="W1103"></a>
### `W1103` — timing mismatch in a sum

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 02-dsl.md §10


## Prophet reader diagnostics (P0xxx)

<a id="P0101"></a>
### `P0101` — file is not valid UTF-8; decoded as Windows-1252

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0102"></a>
### `P0102` — unrecognised header key retained verbatim

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0103"></a>
### `P0103` — `OUTPUT_FORMAT` disagrees with the name line

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0104"></a>
### `P0104` — `VARIABLE_TYPES` length differs from the name line

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0105"></a>
### `P0105` — short data row

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0106"></a>
### `P0106` — long data row

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0107"></a>
### `P0107` — `NUMLINES` disagrees with the actual row count

**Severity:** warning &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0108"></a>
### `P0108` — percentage column converted by dividing by 100

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0109"></a>
### `P0109` — ambiguous date format (`DD/MM` vs `MM/DD`)

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0110"></a>
### `P0110` — no column-name line, or no data rows, in the Prophet file

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0111"></a>
### `P0111` — value does not parse as its declared Prophet type

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0201"></a>
### `P0201` — `.fac` dimension declaration is malformed

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0202"></a>
### `P0202` — `.fac` key ordering resolved

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0203"></a>
### `P0203` — `.fac` value count does not match the dimension extents

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0204"></a>
### `P0204` — lookup policy proposed for a `.fac` dimension

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0301"></a>
### `P0301` — `.rpt` has no period axis

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0302"></a>
### `P0302` — `.rpt` period base could not be aligned to the IR timeline

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0303"></a>
### `P0303` — `TIME_UNITS` does not match the IR timeline basis

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0304"></a>
### `P0304` — `.rpt` is aggregate-level; modelpoint diffing unavailable

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §4

<a id="P0305"></a>
### `P0305` — `.rpt` model point key column is ambiguous

**Severity:** error &nbsp;·&nbsp; **Specified in:** 04-verify.md §4


## Trace notes (N0xxx)

<a id="N0101"></a>
### `N0101` — lookup key clamped

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0102"></a>
### `N0102` — lookup key stepped (banded table, exact key absent)

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0103"></a>
### `N0103` — lookup key interpolated between two rows

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0104"></a>
### `N0104` — lookup hit `on_missing = default(...)`

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0201"></a>
### `N0201` — `retime` applied — timing cast, value unchanged

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0202"></a>
### `N0202` — timing-mismatched addition inside this expression

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0301"></a>
### `N0301` — `pre_origin_default` used (no `init` declared)

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0302"></a>
### `N0302` — `init` used at `t = 0`

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0401"></a>
### `N0401` — value is exactly zero because a factor in the product chain is zero

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0402"></a>
### `N0402` — money value magnitude outside 1e-12 .. 1e12 (probable scaling error)

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0403"></a>
### `N0403` — denominator within 1e-12 of zero (near-trap)

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4

<a id="N0501"></a>
### `N0501` — branch of an `If` never taken for any `t`

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §3.4


## Run-diff hypotheses (H0xxx)

<a id="H0101"></a>
### `H0101` — constant ratio — a scale factor is missing

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0102"></a>
### `H0102` — constant offset — an additive term is missing

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0103"></a>
### `H0103` — sign flip — sign convention mismatch

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0201"></a>
### `H0201` — off-by-one in `t` — timing or `shift` mismatch

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0202"></a>
### `H0202` — timing basis mismatch in an `npv`

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0301"></a>
### `H0301` — divergence begins at a table key boundary — lookup policy mismatch

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0302"></a>
### `H0302` — one side is zero from `t = k` onward — indicator or term expiry off by one

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0303"></a>
### `H0303` — divergence confined to modelpoints sharing a field value

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0401"></a>
### `H0401` — rate conversion — the Prophet `/12` idiom

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0402"></a>
### `H0402` — rounding only — raise the tolerance rather than change the model

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<a id="H0501"></a>
### `H0501` — NaN or Inf on one side — a trap, never a tolerance question

**Severity:** info &nbsp;·&nbsp; **Specified in:** 04-verify.md §5.5

<!-- END GENERATED CODES -->

## Adding a code

1. Add an entry to `REGISTRY` in `crates/predictable-diagnostics/src/registry.rs`, keeping the array
   sorted by code — `lookup` binary-searches it and a test asserts the ordering.
2. Regenerate the catalogue section:

   ```console
   $ cargo run -p predictable-diagnostics --example generate_catalogue
   ```

   and paste the output over everything between the `BEGIN`/`END GENERATED CODES` markers above.
3. Write the prose for the new entry: what triggers it, why the rule exists, and the fix.
4. `cargo test -p predictable-diagnostics` — the registry tests fail if a code has no anchor here, if
   this page documents a code that is not registered, or if a title has drifted.

Codes are never reused. Retiring a rule means leaving its entry in place marked as retired, so an
old log line or a committed `--json` fixture still resolves to an explanation.
