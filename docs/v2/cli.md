# The command line

`predictable` is one binary with a closed set of subcommands. It is the surface an
agent drives during a migration and the surface CI drives on every commit, so two
properties matter more than any individual command:

1. **Every subcommand accepts `--json`** and writes exactly one `pvf/1` document
   to stdout. `--out-json <path>` writes that same document to a file as well.
   Nothing is available only as prose.
2. **Exit codes mean one thing.**

| code | meaning | examples |
|---|---|---|
| `0` | success | a clean `check`, a completed `run` |
| `1` | a **domain** failure | lints, a trapped modelpoint, drifted inputs, a file that is not canonical under `fmt --check` |
| `2` | a **usage or structural** failure | an unknown flag, a missing file, checker errors, an unimplemented command |

The mapping from diagnostics to a code is `predictable_diagnostics::exit_code`,
not a rule the CLI re-derives, so the CLI, the Python API and the engine cannot
drift apart about what "failed" means.

```
predictable check   <path>...            # 01-ir.md §7 diagnostics
predictable fmt     [--check] <path>...  # canonical form, §4.1
predictable digest  [--kind k] <path>... # model / run / assumption / table digests
predictable build   <path>...            # check + plan + lower; digests, no data
predictable run     <run.pir>            # → run/ with manifest + results
predictable graph   <path>...            # the GraphDoc of 05-viz.md §2.1
predictable export  <run/> --out pack.html
predictable rerun   <run/manifest.json>  # verify every digest, then re-execute
predictable diff model <a> <b>           # structural model diff, IR §11.3
predictable explain <run/> --component c --mp k --t 3   # provenance trace, 04 §3
predictable diff <runA> <runB> | migrate # phase 2; they refuse, machine-readably
```

---

## A worked example

Everything below runs against three files. They are the CLI's own test fixtures
(`crates/predictable-cli/tests/fixtures/`), so the outputs are real.

=== "term.pir"

    ```toml
    format = "pir/1"
    module = "term"

    [timeline]
    basis = "annual"
    periods = 3
    origin = "policy"
    valuation_date = 2026-06-30

    [[modelpoint_field]]
    name = "policy_number"
    dtype = "str"
    required = true
    key = true

    # ... sum_assured, premium, q ...

    [[component]]
    name = "survivors"
    kind = "Derived"
    dtype = "f64"
    shape = "Series"
    unit = "count"
    timing = "start"
    init = "1.0"
    expr = "survivors[t-1] * (1 - q)"

    [[component]]
    name = "claims"
    kind = "Output"
    dtype = "f64"
    shape = "Series"
    unit = "money"
    timing = "end"
    expr = "survivors * q * sum_assured"

    [[component]]
    name = "bel"
    kind = "Output"
    dtype = "f64"
    shape = "PerMP"
    unit = "money"
    expr = "sum(net_cashflow)"
    ```

=== "mp.csv"

    ```csv
    policy_number,sum_assured,premium,q
    POL1,100000,900,0.01
    POL2,250000,1500,0.02
    POL3,50000,400,0.005
    POL4,75000,600,0.03
    ```

=== "run.pir"

    ```toml
    format = "pir/1"

    [run]
    product = "term"
    modelpoints = "mp.csv"
    out = "out"

    [run.exec]
    threads = 1
    chunk_size = 2
    ```

Paths inside a `[run]` file are resolved **relative to the run file's own
directory**, never to the working directory, so a run file is portable.

---

## `check` — the diagnostics pass

```console
$ predictable check term.pir
1 file(s) checked, no diagnostics
$ echo $?
0
```

An error renders with the snippet and the suggested edit:

```console
$ predictable check broken.pir
error[E1103]: unknown name `pols`
  --> broken.pir:11:9
   |
11 | expr = "pols * 0.01"
   |         ^^^^ not a component, assumption or modelpoint field
$ echo $?
2
```

Under `--json` the snippet is not repeated as prose — the document *is* the
output, and it carries the same diagnostics with byte-range suggested edits:

```console
$ predictable check broken.pir --json
{
  "format": "pvf/1",
  "kind": "check",
  "files": ["broken.pir"],
  "model_digest": "sha256:…",
  "summary": { "errors": 1, "warnings": 0, "info": 0, "help": 0 },
  "diagnostics": [ { "code": "E1103", "severity": "error", … } ],
  "exit_code": 2
}
```

A lint alone is exit `1`: the model still runs, and you were still told.

---

## `fmt` and `digest`

`fmt` rewrites `.pir` files into the canonical form of `01-ir.md` §4.1 and is
idempotent. `--check` writes nothing and exits `1` if any file is not canonical —
the CI form. A path of `-` reads stdin and writes stdout.

```console
$ predictable fmt --check .
model.pir: not canonical
1 file(s) checked, 1 not canonical
```

`digest` prints the digests every artefact is pinned by. `--kind model` (the
default) canonicalises each file first and hashes them in path order, so
reformatting a model never changes its identity.

```console
$ predictable digest term.pir
sha256:6d5c…  model_digest
$ predictable digest --kind run run.pir
sha256:0a41…  run.pir (run_digest)
$ predictable digest --kind file mp.csv
sha256:9fb2…  mp.csv
```

`--kind` also takes `assumptions` and `table:<name>` (for an inlined table's
`rows`), and `--each` adds a per-file digest alongside the model digest.

---

## `build` — compile without data

`build` is everything that depends on the model and the plan options and on no
modelpoint at all: check, plan, lower to tape. It is what CI runs to prove a
model still compiles, and what pins the three digests before a run exists.

```console
$ predictable build term.pir
model_digest  sha256:6d5c…
plan_digest   sha256:1f0a…
order_digest  sha256:b731…
17 slot(s): 0 scalar, 5 per-mp, 12 series; 3 output(s) over T = 3
series buffer 0.000 MiB per chunk of 1024
```

- `--out plan.json` writes the whole plan as JSON.
- `--O0` turns off every optional rewrite (constant folding, hoisting). The
  `model_digest` is unchanged and the `plan_digest` is not — which is exactly
  what makes a bit-level difference bisectable.
- `--retain-all` forces every series to `Full` retention; the printed buffer size
  is the number that multiplies.

`build` refuses a model that does not check (`"status": "not_checked"`, exit `2`)
rather than half-planning it.

---

## `run` — the projection

```console
$ predictable run run.pir --out out
run_id        2026-08-17T09:14:22Z-b91d192b
out           out
outcome       Completed
modelpoints   4 projected, 0 trapped, of 4
results       36 row(s), 3 component(s)
manifest      sha256:7660…
```

The run directory holds `manifest.json`, `results.parquet`,
`results.schema.json`, plus `solves/` and `aggregates.parquet` when the run
declares them. The manifest re-derives its own digest, which is what `rerun`
checks before trusting it.

Overrides, all optional, all outside `run_digest` because none of them can change
a number: `--model`, `--assumptions`, `--modelpoints`, `--out`, `--threads`,
`--chunk-size`, `--run-id`, `--O0`, `--retain-all`, `--allow-table-drift`.

Exit codes follow the outcome: `0` completed, `1` completed with traps or
cancelled, `2` aborted (a trap under `on_trap = "abort"`, which writes a manifest
and **no** results, so a short result set can never look complete).

### What determinism looks like from here

```console
$ predictable run run.pir --out a --threads 1 --chunk-size 2 --json | jq -r .results.digest
sha256:7700f0ec…
$ predictable run run.pir --out b --threads 4 --chunk-size 2 --json | jq -r .results.digest
sha256:7700f0ec…
```

Thread count is a scheduling choice, never a numerical one: `results.parquet` is
byte-identical. Chunk size changes the file's row-group layout but not a single
number, so the result set — rows, components, `component_set_digest` — is
unchanged. Both are asserted in `crates/predictable-cli/tests/commands.rs`.

---

## `graph` — the model as a DAG

`graph` emits the `GraphDoc` of `05-viz.md` §2.1: nodes, edges and `layers`.

```console
$ predictable graph term.pir
17 node(s), 10 edge(s), 5 layer(s) in evaluation order
    0  policy_number, sum_assured, premium, q, t, period_start_date, …
    1  survivors
    2  claims, premium_income
    3  net_cashflow
    4  bel
```

The layers are bucketed out of the **planner's own ordering** — the same sequence
`order_digest` hashes and the tapes execute. The viz layer never computes a
topological order of its own, so the picture and the evaluation order cannot
disagree. The document repeats `order_digest`, which is how a rendered graph is
pinned to the plan it was drawn from.

Each node carries its kind, dtype, shape, unit, timing, stage, retention, the
expression text as written, and a span (`file`, `line`, `col`, byte range). Each
edge carries `lag` (`survivors[t-1]` is a lag-1 self edge), `stage` and `via` —
the `ExprPath` of the reference, e.g. `expr.lhs`.

---

## `export` — the governance pack

```console
$ predictable export out --out pack.html
pack.html  8645 byte(s), 1 module(s), manifest verified
```

One self-contained file: no CDN, no fonts fetched, no network at all, and a print
stylesheet for `Ctrl-P`. The manifest is a **visible header** — all six digests,
versions, timeline, outcome — because the audience is an auditor opening the file
in three years, not a developer hovering for a tooltip.

`--format json` writes the same content as a `pvf/1` `pack` document.

This build embeds the manifest, the full model text and the results schema. It
does **not** embed the result columns (base64 Arrow IPC) or the WASM engine —
those are T32 — and the page says so in a banner rather than quietly omitting
them.

---

## `rerun` — reproduce, or refuse

```console
$ predictable rerun out/manifest.json --verify-only
manifest_digest verified
      ok  model        term.pir
      ok  modelpoints  mp.csv
```

Every input digest is verified before anything starts. Model files are compared
against their **canonical** text, so re-formatting a module is not drift;
changing a number in it is.

```console
$ predictable rerun out/manifest.json --verify-only
   DRIFT  modelpoints  mp.csv
2 input(s) drifted since the run; refusing. Pass --allow-drift to proceed anyway.
$ echo $?
1
```

An edited manifest — one that no longer re-derives its own digest — is exit `2`,
not `1`: it is not a disagreement about numbers, it is a broken artefact.

Without `--verify-only`, verification is followed by a real re-execution through
the same path a fresh `run` takes: the manifest names the `[run]` file, and that
file remains the source of truth for what a run *is*. Nothing is reconstructed
from the manifest.

!!! note "Run `rerun` from where the run was launched"
    The manifest records input paths as the run saw them. If they were relative,
    `rerun` must be invoked from the same working directory.

---

## `diff model`

`predictable diff model A B` compares two models structurally: renames, change
classification and the transitive impact set. It has its own page —
[Diffing two models](model-diff.md).

```console
$ predictable diff model ./a ./b
./a → ./b
  * qx [formula]
      expr.rhs subtree replaced: mortality_loading → 1.1

1 change(s) affect 14 downstream component(s); 12 output(s) affected: bel, …
```

A difference exits `0`; `--fail-on-change` turns semantic change into exit `1`.

---

## `explain <run/> --component c --mp k [--t N]`

Replay one modelpoint and print the provenance tree behind one cell
(`04-verify.md` §3). The full page is [Explaining a number](explain.md).

```console
$ cd models/term_annual
$ predictable explain runs/base --component bel --mp TA00001
model.bel  TA00001  = -223.20  money
│ pv_claims + pv_expenses + initial_expense - pv_premiums
│
└─ -  -223.202
   ├─ …
```

`--json` writes the trace document of §3.2 — `{"format": "pvf/1", "kind":
"trace", …}` — and the terminal rendering is a pure projection of it, so the two
can never disagree. `--depth N` (`-1` for the leaves), `--expand a,b`,
`--values-only`, `--trace-max-terms N` and `--width N` shape what is retained,
never what is computed.

Exit `0`. Exit `1` only for `E0901` — the replay did not reproduce the run, which
is an engine bug and is reported as one, with the trace still emitted so the
divergence can be read. Exit `2` for a usage failure: an unknown component, an
unknown modelpoint, a `t` on a `PerMP` value or a missing `t` on a series.

---

## `diff <runA> <runB>`, `migrate`

Present, documented, and machine-readable in their refusal:

```console
$ predictable migrate --json
{
  "format": "pvf/1",
  "kind": "migrate",
  "status": "unimplemented",
  "command": "migrate",
  "summary": "drive the Prophet → predictable migration loop",
  "spec": "04-verify.md §4, §8.2",
  "implemented_by": "T24 (readers) / T33 (skill)",
  "exit_code": 2
}
```

An agent must be able to tell "this build cannot do that yet" from "you made a
typo"; a command that simply did not exist would be indistinguishable from the
latter. A typo, by contrast, produces no document at all.

---

## Notes and current limits

- **Modelpoint formats.** `run` reads `.csv` and `.parquet`. Arrow IPC files are
  supported by `predictable-io` but not yet wired to a file extension here.
- **Table content copies.** Table digests and the drift flag travel into the
  manifest; the table *content* copy (`run/tables/*.parquet`, ruling Q13) is not
  taken by this build, so `copy` is stamped `null` and a consumer must say "table
  content unavailable in this run".
- **`manifest_digest` and `[run.exec]`.** `run_config.exec` participates in
  `manifest_digest`, so recording "this ran on four threads" changes the manifest
  even though it cannot change a number. `results.digest` is the byte-equality
  claim to compare across schedules.
