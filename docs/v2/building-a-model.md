# Building a model: `predictable build`

[Declaring a model](dsl-declarations.md) gets you Python that has never run. `predictable build`
runs it once, against symbolic values, and turns what comes out into files: canonical `.pir`
modules, table digests, a Python ↔ `.pir` span map, and a full pass of the engine's own checker —
whose diagnostics are printed against your `.py`, not against the generated TOML.

This is step 4 to 6 of [`02-dsl.md` §8](../design/02-dsl.md). Steps 1 to 3 — collect, trace,
resolve — belong to the [tracer](dsl-tracer.md) and the [declaration layer](dsl-declarations.md).

```
predictable-build models/term_annual/ [--check] [--out build/] [--json]
python -m predictable build models/term_annual/model.py
```

!!! note "Two commands called `build`"
    The Rust CLI's [`predictable build`](cli.md) compiles `.pir` files into a plan. This one
    compiles *Python* into those `.pir` files, and is installed as `predictable-build` so the two
    can live on one `PATH`. In-process, it is `predictable.build_product`.

---

## What one build produces

Given `models/term_annual/model.py`, a build writes four files:

```
build/
├── schema.pir     # timeline, enums, modelpoint fields, assumptions, tables (+ digests)
├── model.pir      # one file per Python module, components in declaration order
├── product.pir    # the product manifest: modules, outputs, key field
└── spans.json     # the Python ↔ .pir span map
```

The split is not cosmetic. `schema.pir` holds everything a *second* product could reuse, each
Python module becomes exactly one IR module — so `module_path` and therefore evaluation order
follow your own file layout — and `product.pir` names the closed module set that
[`01-ir.md` §8.4.1](../design/01-ir.md) requires.

### The model

```python title="models/term_annual/model.py"
@series(timing=START, init=1.0)
def num_pols_if(num_pols_if: Count, qx: Prob, in_term: Flag) -> Count:
    """Survivorship, self-referential at lag 1."""
    return num_pols_if[t - 1] * (1 - qx[t - 1]) * in_term
```

### The build

```toml title="build/model.pir"
format = "pir/1"
module = "model"
imports = ["schema"]

[[component]]
name = "num_pols_if"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1 - qx[t-1]) * in_term"
doc = "Survivorship, self-referential at lag 1."
```

The text is in [canonical form](canonical-form.md) as emitted — fixed key order, normalised
expression spacing, Ryū floats, LF, one blank line before each section. The build does not write
text `predictable fmt` would rewrite, and the test suite asserts the emitter lands on the
formatter's output directly rather than relying on a post-pass.

---

## Table digests

A table's identity is *the bytes*, not the path ([`01-ir.md` §2.9](../design/01-ir.md)). Every
table whose `source` resolves under the build root is hashed and the digest goes into the module:

```toml title="build/schema.pir"
[[table]]
name = "mortality"
keys = [{ name = "age", dtype = "i64", policy = "clamp" }]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "tables/mortality.csv"
digest = "sha256:f7a571513c88186e30dd65dde3ff79a2f6b9fceef7746e0a61783cccb319bd87"
```

Change one rate in the CSV and the digest changes, so the table change appears in the git diff of
the build directory — which is the point: a table edit is as reviewable as a formula edit. Sources
the build cannot read get *no* digest rather than a wrong one: `inline` rows are digested by the
engine over their canonical text, and `resource:` sources are supplied by the host at run time.

`--root` chooses the directory `source` paths resolve against; it defaults to the model directory.

---

## Diagnostics come back in Python

This is the part [`02-dsl.md` §8](../design/02-dsl.md) says must not be cut. After emitting, the
build hands the text to the engine's checker **in process** through the
[Python API](python-api.md) — the same checker the Rust CLI runs, never a Python re-implementation
— and re-anchors every diagnostic through the span map before printing it.

A cycle is the case worth showing, because no single function body contains the fault:

```python title="models/broken/cycle.py"
@series(timing=END)
def a(b: Money, premium: Money) -> Money:
    return b + premium


@series(timing=END, output=True)
def b(a: Money) -> Money:
    return a * 2.0
```

```console
$ predictable-build models/broken/cycle.py
error[E0201]: cyclic dependency in the same period

   ┌─ models/broken/cycle.py:23:12
 23 │     return b + premium
    │            ^ `a` reads `b` at time t

   ┌─ models/broken/cycle.py:28:12
 28 │     return a * 2.0
    │            - `b` reads `a` at time t

  `b` depends on `a`, which depends on `b` — with no time lag between them, so neither can be
  computed first.

  help: read last period's `b` instead
  help: or seed the recursion with an `init`

  see https://predictable.dev/llm/diagnostics/#E0201

error: build refused: 1 error(s); nothing was written
```

Two Python bodies, two underlined identifiers, and the `.pir` never mentioned. The engine reported
byte ranges in `model.pir`; the span map turned them back into the names you typed. A build that
produces an error writes nothing at all — a half-written build directory is worse than none.

The `.pir`-anchored originals are not thrown away. `--json` emits both streams:

```json
{
  "status": "failed",
  "diagnostics":     [{ "code": "E0201", "spans": [{ "file": ".../cycle.py", "..." : "..." }] }],
  "pir_diagnostics": [{ "code": "E0201", "spans": [{ "file": "model.pir",    "..." : "..." }] }]
}
```

---

## The span map

`spans.json` is the `ExprId → origin_span` side table
[`01-ir.md` §11.1](../design/01-ir.md) requires alongside the modules. Per component it records
where the component came from, where it landed, and the span of every name that appears in its
signature or body — dependencies, tables and the annotations alongside them:

```json
{
  "name": "qx",
  "pir_file": "model.pir",
  "pir_line": 24,
  "pir_end_line": 33,
  "origin_span": { "file": "model.py", "line": 68, "col_start": 4, "col_end": 6 },
  "refs": {
    "age":               { "file": "model.py", "line": 70, "col_start": 21, "col_end": 24 },
    "mortality_loading": { "file": "model.py", "line": 70, "col_start": 28, "col_end": 45 }
  }
}
```

Paths are relative to the build root, so the file is identical on every machine and `--check` never
reports drift that is really just somebody else's home directory. Spans come from the function's
own AST, so they point at the identifier — column range included — and not merely at the line.

Anything downstream that needs "where in Python did this come from" reads this file:
`explain()`, the model explorer's inspector, and the run diff's attribution panel.

!!! warning "`meta.origin_span` in the `.pir` itself"
    §11.1 also puts `origin_span` in each component's `[component.meta]`. The build probes the
    installed engine once and writes the section only if the checker accepts it; today's checker
    rejects `[component.meta]` as an unknown section, so provenance is carried in `spans.json`
    alone and the build says so. When the checker learns the section, the same build starts
    emitting it with no change here. `--json` reports which happened as `meta_in_pir`.

---

## `--check`: the build directory is a committed artefact

Commit `build/`. It is the thing reviewers read, the thing `predictable diff` compares, and the
thing CI can prove is current:

```console
$ predictable-build models/term_annual/ --check --out models/term_annual/build
models/term_annual/build is up to date
```

```console
$ predictable-build models/term_annual/ --check --out models/term_annual/build
error: models/term_annual/build is out of date (1 file(s)):
  model.pir: line 47: committed 'unit = "years"', built 'unit = "months"'
  run `predictable build` to update it.
```

`--check` writes nothing and exits `1` on drift, `0` when current. `spans.json` is compared too: a
function that moved down the file changes provenance even when it changes no number, and a
provenance table that silently rots is worse than no provenance at all.

Exit codes are the project's usual ones: `0` clean, `1` diagnostics or drift, `2` misuse.

---

## From Python

The command is a thin shell over one function, so a notebook or a test can do the same thing:

```python
from predictable import build_product, write_build, check_build, import_models, registry_scope

with registry_scope():
    import_models(["models/term_annual/model.py"])
    result = build_product(root="models/term_annual")

result.ok                      # no errors from the DSL or the engine
result.files["model.pir"]      # canonical text, in memory
result.table_digests           # {"mortality": "sha256:…"}
result.span_map                # the Python ↔ .pir map, before it is serialised
print(result.render())         # every diagnostic, rendered against Python

write_build(result, "models/term_annual/build")     # or:
check_build(result, "models/term_annual/build")     # [] when the committed build is current
```

`registry_scope()` matters in a long-lived process: declarations register on import, so without a
scope a second build would also contain whatever the first one imported. The command line does this
for you — a command is a fresh world.

---

## What the build refuses to do

* **Compute.** No modelpoint is read and no number is produced; see [running a model](running-a-model.md).
* **Guess a digest.** A table it cannot read is emitted without one.
* **Write a partial build.** Errors mean nothing is written.
* **Report against generated text when it knows better.** A diagnostic falls back to the `.pir`
  span only when no Python span exists for that location, and says so in a note when it does.
