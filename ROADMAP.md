# predictable — roadmap

**What this is.** An open-source (MIT) actuarial modelling framework for life
insurance cashflow projection — a credible alternative to FIS Prophet and
Moody's AXIS. A Rust projection engine executes a serializable, git-diffable
computation-graph IR; a Python DSL is the authoring layer that compiles to that
IR. No arbitrary Python callables cross the engine boundary.

**The thesis.** An AI-first model layer: an LLM must be able to read a Prophet
model and reimplement it in predictable in minutes to hours. The verification
loop is the product — definition-time validation with Rust/Elm-grade,
LLM-actionable error messages, `explain()` provenance traces, and a structured
Prophet `.rpt` run-diff that proves the reimplementation matches.

Design specs: [`docs/design/`](docs/design/README.md).
Build backlog and dependency graph: [`docs/design/00-backlog.md`](docs/design/00-backlog.md).

---

## Status — 2026-08-17

**Phases 0–3 are complete.** The full pipeline exists end to end: DSL → `.pir` → check → plan →
tape → run → results → `explain()` → model diff → run diff with ranked hypotheses, plus the viz
server with all three screens, the wasm engine and single-file governance packs, Prophet readers,
the migration skill, the correctness-gated benchmark suite and three self-asserting demos.

Release gate: **1,523 tests, 0 failures** (785 Rust, 481 Python across four suites, 257 vitest),
wasm determinism gate green, all three demos exit 0, `mkdocs build --strict` green.

What Phase 3 did *not* ship: PyPI wheels, the migration screencast, and retiring v0 to `legacy/`.
Those and the rest of the honest gap list are in
[`RELEASE_NOTES_v2.md`](RELEASE_NOTES_v2.md).

Per-task detail: [`docs/design/00-backlog.md`](docs/design/00-backlog.md) §2a–§2d.

---

## Phase 0 — Foundation ✅ complete

Make the repo a place where v2 can be built without disturbing v0.

- Monorepo skeleton: `crates/predictable-engine` (Rust workspace),
  `crates/predictable-py` (PyO3/maturin), `packages/predictable` (v2 Python
  package, placeholder), `docs/design/`, this roadmap.
- v0 (`/predictable`, `/tests`, `/examples`) stays untouched and working —
  reference only.
- Design docs 01–05 written and agreed: IR schema, DSL surface, engine
  execution model, verification loop, visualization.
- CI: `cargo test` + `cargo clippy` + `cargo fmt --check` alongside the existing
  Python job.

**Exit:** `cargo test` green; the IR schema is written down concretely enough
that the engine and the DSL can be built against it independently.

## Phase 1 — Engine + DSL ✅ complete

The vertical slice: author a model in Python, run it in Rust, get numbers.

- IR v1 implemented on both sides (Rust `serde` types, Python dataclasses),
  with a shared round-trip conformance corpus.
- Engine: graph validation, cycle detection, topological schedule, column-at-a-
  time evaluation over the `(modelpoint x period)` grid, table lookups
  (mortality/lapse), monthly and annual time axes.
- DSL: `Model`, components, indicators, table references, dependency capture,
  compile-to-IR.
- `predictable_engine` wheel via maturin; `predictable.run(model, modelpoints)`
  returns results.
- Reference model: a term assurance product end to end — premiums, claims,
  expenses, discount factors, reserves.

**Exit:** term assurance projects correctly from Python source through the Rust
engine, on a real modelpoint file.

## Phase 2 — Interop + verification ✅ complete

Turn "it runs" into "it is provably the same model".

- Prophet interop: MPF model point reader, `.fac` table reader, `.rpt` results
  reader.
- Structured run-diff: compare a predictable run against a Prophet `.rpt` per
  variable, per period, per modelpoint, with tolerance policy and a ranked list
  of first divergences.
- Diagnostics pass: every definition-time error carries the offending node, the
  expected shape, and a suggested fix — written to be actionable by an LLM as
  well as a human.
- `explain(variable, modelpoint, period)` — full provenance trace of how a
  number was produced.
- Visualization: `model.show()` dependency-graph explorer, `results.show()`
  waterfalls, run diffs and drill-down.

**Exit:** an LLM, given a Prophet model and its `.rpt`, can iterate to a
matching predictable model using only the errors and the diff.

## Phase 3 — v1 launch (three demos) ✅ complete, except distribution

Ship it, with proof rather than claims.

- **AI migration screencast** — a real Prophet model reimplemented in
  predictable, live, driven by the verification loop.
- **Speed benchmark** — reproducible comparison against cashflower and lifelib
  on identical models and modelpoint volumes.
- **Sample IFRS 17 valuation** — building blocks (BEL, risk adjustment, CSM)
  built in the DSL, results explorable in the viz layer.
- Supporting work: docs site for v2, `pip install predictable` on PyPI with
  prebuilt wheels, standalone CLI, WASM build for in-browser demos, v0 retired
  to legacy.

**Exit:** a Prophet-shop actuary can install it, migrate a model, and see the
diff go green without help.

## Phase 4 — Monetize the wrapper ⏳ not started

The core stays MIT and complete. The commercial layer is what a regulated team
needs *around* the core, never a crippled engine.

- Hosted runs and scaling, model registry with versioned lineage.
- Governance: sign-off workflow, audit trails, reviewer-facing model diffs.
- Team collaboration, SSO, permissions.
- Migration services and support contracts for Prophet shops moving in bulk.

**Exit:** revenue from teams, without any capability moving out of the
open-source engine.

---

## Repository layout

```
predictable/
├── Cargo.toml                    # Rust workspace root
├── crates/                       # target shape, per docs/design/03-engine.md §1
│   ├── predictable-syntax/       # .pir lexer, parser, formatter, spans, diagnostics
│   ├── predictable-ir/           # IR model, resolver, checker, digests, JSON encoding
│   ├── predictable-engine/       # planner + runtime (no io, no threads, no Python)
│   ├── predictable-prophet/      # MPF / .fac / .rpt readers
│   ├── predictable-io/           # modelpoint readers, table loaders, result writers
│   ├── predictable-runner/       # chunking, rayon, solve loop, aggregation, manifest
│   ├── predictable-cli/          # `predictable` binary
│   ├── predictable-py/           # PyO3 bindings -> `predictable_engine` wheel
│   └── predictable-wasm/         # wasm-bindgen surface for the browser UI
├── packages/
│   └── predictable/              # v2 Python package (placeholder)
├── viz/                          # predictable-viz SPA (React + TS)
├── skills/predictable-migration/ # published agent skill (04-verify §8)
├── docs/design/                  # 00-backlog, 01-ir, 02-dsl, 03-engine, 04-verify, 05-viz
├── ROADMAP.md
│
├── predictable/                  # v0 (pandas/pydantic v1) — reference only
├── tests/  examples/  docs/      # v0 — untouched
└── pyproject.toml                # v0 packaging — untouched
```

## Building the Rust side

```sh
cargo test                        # workspace tests (engine)
cargo clippy --all-targets
maturin develop -m crates/predictable-py/Cargo.toml   # build + install the wheel
```

`cargo test` at the workspace root builds only `predictable-engine`: the
workspace sets `default-members` accordingly, because `predictable-py` links
against libpython and is meant to be built through maturin, which supplies the
interpreter and enables the crate's `extension-module` feature. Build the
bindings explicitly with `cargo check -p predictable-py` when a development
Python (with shared libs) is on PATH, or via `maturin`.

**Toolchain status:** `cargo test --workspace` runs 785 passing tests. Note that
`cargo` may not be on the default `PATH` in non-login shells; it lives at
`~/.cargo/bin/cargo` via rustup, and `predictable-py` needs `PYO3_PYTHON` pointed
at a development interpreter (`.venv/bin/python` in this repository).

The four Python suites must be invoked separately — the v0 `predictable/` package at
the repo root shadows `packages/predictable/src/predictable`:

```sh
pytest tests -q
pytest packages/predictable/tests -q
pytest crates/predictable-py/tests -q
pytest models/tests -q
```
