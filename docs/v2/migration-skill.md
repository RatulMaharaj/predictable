# The migration skill and `llms.txt`

Everything else in this section documents a command a human runs. This page documents the two
artefacts aimed at a *machine* reader: the packaged agent skill that performs a Prophet migration,
and the `llms.txt` pair that lets a model load the project's normative material in one fetch.

The design position is stated in [the verification spec](../design/04-verify.md) §8.1 and is worth
repeating, because it explains why the skill lives in this repository rather than in a blog post:

> The migration agent is not a demo script; it is a *client of the verify layer*, and its
> instructions are a normative statement of how that layer is meant to be used. If the skill needs
> to explain a workaround, the verify layer has a defect.

## The skill

```
skills/predictable-migration/
  SKILL.md                      the loop, as an explicit procedure
  references/
    ir-cheatsheet.md            shapes, units, timing, grammar, builtins — one page
    prophet-idioms.md           Prophet construct → IR construct, incl. the timing table
    diagnostics.md              the codes that fire in a migration, and the standard fix
    verify-formats.md           diff.json, traces, manifests — condensed
  scripts/
    loop.py                     check → run → diff → report; one command, JSON out
```

`SKILL.md` carries the Claude Code frontmatter (`name`, `description`, `allowed-tools`) and triggers
on Prophet, `.MPF`, `.fac`, `.rpt`, or any request to reimplement or validate a life insurance
projection model.

### The loop it encodes

1. **Inventory.** Read the workspace, produce `migration/coverage.json`. No `.pir` is written before
   the variable inventory exists.
2. **Schema first.** `schema.pir` — fields, enums, assumptions, tables — checked before a single
   formula is written. A wrong schema turns into a resolution error (`E0203`) in every formula that
   reads it, and ten formulas against a wrong schema produce forty errors of which one is real.
3. **Units and timing are a decision, not a default.** Every numeric field and component gets an
   explicit `unit` and `timing`. `annual_rate / 12` is an error, not a translation
   ([IR §5](../design/01-ir.md)); the honest spellings are `to_monthly(r)` or, when the source model
   really did divide, `nominal_to_periodic(r, 12)` — named so the choice appears in a diff.
4. **Translate leaves upward.** Decrements → in-force → cashflows → discounting → reserves, checking
   after each layer, with `meta.source` on every component naming the Prophet variable and its
   `file:line`. **Coverage of `meta.source` is the completion criterion for this step**, not a
   nicety: a component nobody can trace back to a Prophet variable is a formula the agent invented.
5. **Run and diff** with `--retain-all` and `--tolerance-profile reconcile`.
6. **Work the findings top-down.** Only `class = "root"` findings are actionable; inherited findings
   disappear when their root is fixed. Apply a `suggested_edit` at `high` confidence; otherwise
   [explain](explain.md) both sides before editing.
7. **Stop condition** — all four, or say plainly that you did not get there.
8. **Hand back** a report: coverage, remaining differences with justification, and `mapping.toml`
   with every `sign` / `scale` / `timing_shift` listed for human sign-off.

### The one hard prohibition

> **Never widen a tolerance to make a finding go away.**

A tolerance is a statement about which differences are immaterial. Loosening one to clear a finding
does not resolve the difference; it makes it invisible, and it does so in the artefact a valuation
committee will later be shown. The skill states this as a prohibition rather than a preference, and
`loop.py` enforces it rather than trusting it:

* `--abs` and `--rel` exist only to be refused — the script exits `3` with a structured refusal
  before anything is executed;
* every comparison is inspected for a loosened tolerance (`tolerance.loosened`, and any
  `overrides_applied` entry marked `looser`), and a run that loosened one is reported as `refused`
  even if the numbers matched.

The single legitimate case — `H0402`, where the entire difference is rounding — is a *named profile*
chosen deliberately and put to the user, not a bound nudged until the report goes quiet.

## `loop.py`

```console
$ python skills/predictable-migration/scripts/loop.py \
      --model build --model base.pir --run run.pir \
      --baseline prophet_run/ --tolerance-profile reconcile
```

It runs `check` → `run --retain-all` → `diff run --json` and prints one report. `--retain-all` is
not optional: which component the diff calls the root is partly a fact about what you asked the
engine to write down, and a component that was never emitted cannot be named.

The report's value is its **stop condition**, which is what keeps an iterating agent honest:

```json
{
  "verdict": "diverged",
  "stop_condition": {
    "matched": false,
    "coverage_complete": false,
    "check_clean": true,
    "tolerance_not_widened": true
  },
  "done": false,
  "roots": [ { "id": "F001", "component": "model.premium_rate", "class_basis": "ir_graph", "…": "…" } ],
  "next_actions": ["F001 model.premium_rate: apply the H0101 suggested edit at …"]
}
```

`done` is the conjunction of all four booleans, and the exit code follows it:

| Exit | Meaning |
|---|---|
| `0` | done: matched, covered, clean, nothing widened |
| `1` | not finished — the report names the root findings and the next action for each |
| `2` | blocked — checker errors, a failed run, or an incomparable pair |
| `3` | refused — the invocation, or the comparison, would have widened a tolerance |

Note what `done = false` above is saying: the numbers can match while the migration is *not*
finished, because nothing carried `meta.source`. That is deliberate. A reconciliation nobody can
trace back to the source library is a coincidence you have not yet explained.

`loop.py` is tested against the seeded off-by-one from
[the verification walkthrough](verification-walkthrough.md) — the same scenario, run through the
script exactly as the skill instructs an agent to run it — asserting that it localises to
`premium_rate` rather than to the six outputs that visibly moved, that both hypotheses arrive with
their evidence, and that a tolerance override is refused.

## `llms.txt` and `llms-full.txt`

[`llms.txt`](https://github.com/RatulMaharaj/predictable/blob/main/llms.txt) at the repository root
follows the convention: an H1, a blockquote summary, then annotated link sections — start here,
writing models, Prophet migration, running and checking, reference.

`llms-full.txt` is generated by `tools/gen_llms_full.py`: the IR spec, the verification spec, the
cheatsheet, the diagnostics index, the skill and its references, the walkthrough and the worked
model sources, concatenated so that one fetch primes a model completely. It currently runs at
roughly 58k tokens against a 200k budget; the generator fails rather than emitting an oversized
file, so the fix is to drop a source, never to raise the budget.

Both are covered by a test in `tests/test_llms_txt.py`:

* every link in `llms.txt` resolves to a file that exists in the repository;
* the entries an agent must read first are present;
* `llms-full.txt` is regenerable and identical to the committed copy, and inside its budget.

## The anchor discipline

Every diagnostic carries a `doc_url` of the form
`https://predictable.dev/llm/diagnostics/#<code>`, which resolves to a section of the
[diagnostics catalogue](../llm/diagnostics.md). That is what makes an unfamiliar code a one-fetch
problem for an agent instead of a dead end, so it is enforced from three directions:

1. every **registered** code has an anchor in the catalogue, and every anchor is a registered code;
2. every code **emitted anywhere in the codebase** — found by scanning every `src/` file in the
   workspace for code-shaped string literals — is registered *and* anchored. This is the check that
   caught the `.pir` syntax codes `E0000`–`E0053`, which were live in the parser but had never been
   registered;
3. anchors are unique, so no `doc_url` is ambiguous.

The catalogue's code sections are generated from the registry
(`cargo run -p predictable-diagnostics --example generate_catalogue`), so a title cannot drift
between the code and the documentation without failing a test.

## See also

* [The verification loop end to end](verification-walkthrough.md) — the scenario `loop.py` is tested on
* [Reading Prophet files](prophet-readers.md) — the readers behind step 1
* [Diffing two runs](run-diff.md) — tolerance profiles, `mapping.toml`, exit codes
* [Diagnostics catalogue](../llm/diagnostics.md) — every code, its meaning and its fix
