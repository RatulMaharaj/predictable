# `packages/predictable` — the v2 Python package

This is the **v2** `predictable` Python package: the authoring DSL that compiles
to the engine IR. As of task T19 it contains the **tracer** — `ExprProxy`, the
builtin set, the IR expression tree and the `E12xx` diagnostic catalogue. See
`docs/v2/dsl-tracer.md` for the user-facing guide.

```console
$ uv venv && uv pip install -e '.[dev]'
$ pytest -q
```

The distribution is named `predictable-v2` so that installing it never shadows
the v0 `predictable` on PyPI; the import name is already `predictable`, and the
rename happens at cutover.

## Why this exists now

The repository currently contains two generations of the project side by side:

| Path | Generation | Status |
| --- | --- | --- |
| `/predictable/` (repo root) | **v0** — pandas + pydantic v1 runtime | Reference only. Still importable, still tested by `/tests/`. Do not delete; do not extend. |
| `/packages/predictable/` | **v2** — DSL that compiles to the engine IR | Tracer implemented (T19); declarations are T20. |
| `/crates/predictable-engine/` | **v2** — Rust projection engine | Stub with passing tests. |
| `/crates/predictable-py/` | **v2** — PyO3 bindings (`predictable_engine` wheel) | Stub, built by maturin. |

Creating the v2 package directory in a fresh location means v0 keeps working
unchanged — same import path, same test suite, same docs build — while v2 is
built next to it rather than on top of it.

## Layout

```
packages/predictable/
├── pyproject.toml            # hatchling/maturin-consuming package: `pip install predictable`
├── src/predictable/
│   ├── __init__.py           # public surface
│   ├── ir.py                 # IR expression tree, mirroring predictable-ir's serde model
│   ├── proxy.py              # ExprProxy, t, TableProxy — the traced values  [T19]
│   ├── fn.py                 # the closed builtin set                        [T19]
│   ├── tracer.py             # trace() / trace_init(), Param, dependencies   [T19]
│   ├── errors.py             # the E12xx catalogue with worked messages      [T19]
│   ├── diagnostics.py        # {code, severity, message, spans, ...}         [T19]
│   ├── timing.py             # START / END / MID / POINT
│   ├── dsl/                  # declarations: @series, ModelPoint, table      [T20]
│   ├── interop/              # Prophet MPF / .fac / .rpt readers and the run-diff
│   ├── viz/                  # model.show() / results.show() local web UI
│   └── _engine.py            # thin wrapper over the `predictable_engine` wheel
└── tests/
```

The v2 package depends on the compiled `predictable_engine` wheel produced from
`crates/predictable-py`; no arbitrary Python callables cross that boundary — the
DSL only ever hands the engine a serialized IR document.

## Cutover

When v2 reaches feature parity for the launch demos, the root `/predictable/`
package is moved to `/legacy/predictable-v0/` (or dropped from the distribution
while remaining in git history) and this package takes over the `predictable`
name on PyPI with a major version bump. Until then, nothing here shadows the v0
import path.
