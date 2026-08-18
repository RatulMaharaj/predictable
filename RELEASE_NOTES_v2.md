# predictable v2 — release notes

**Date:** 2026-08-17 · **Status:** feature-complete against specs 01–05, pre-release (not on PyPI)

v2 is a ground-up rewrite. A Rust projection engine executes a serializable, git-diffable
computation-graph IR (`.pir`); a Python DSL is the authoring layer that compiles to that IR. No
arbitrary Python callable crosses the engine boundary, which is what makes a model an artefact you
can digest, diff, reproduce and explain. v0 (the pandas-based Python library) is untouched and
still installs from PyPI; the two share no code.

---

## What exists

### The pipeline

DSL source → `.pir` (canonical, digest-stable) → check → plan → tape lowering → kernel → run
directory (Parquet results, aggregates, table copies, manifest with six reproducibility digests).

| Piece | State |
|---|---|
| `.pir` syntax, canonical form, digests | complete; conformance corpus with valid/invalid cases |
| Checker + diagnostics | complete; every `E`/`W` code has a catalogue anchor and, where meaningful, a suggested edit |
| Planner, tape lowering, kernel | complete; two substage levels, `npv`/aggregates, `init` back-channel, recursion, table lookups |
| Runner | complete; chunking, threads, Brent solves, aggregations, trap policy, cancellation |
| Determinism | `run(threads=1) ≡ run(threads=64)`, `chunk=1 ≡ chunk=1024`, bit for bit; byte-stable Parquet |
| Python DSL | complete; `@series`/`@per_mp`/`product`, units, timing, declarative dependencies |
| CLI | `check fmt digest build run graph serve export rerun diff explain` |
| Python API | `predictable_engine` wheel via maturin; results, `explain()` |
| Prophet interop | `.MPF`, `.fac`, `.rpt` readers with byte-offset spans and `P` diagnostics; a `.rpt` imports as a first-class run directory |
| `explain()` | any cell replayed to a provenance tree down to assumptions, table cells (with digests) and modelpoint fields |
| Model diff | AST-level, `ExprPath`-anchored, rename-aware, with a transitive impact set |
| Run diff | root vs inherited classification from the IR graph, tolerance profiles, mapping files, contribution ranking, machine-first `diff.json` |
| Hypotheses | `H0101`–`H0501` detectors with support/confidence and applicable source edits; mutation harness scores **14/14 root-cause hits**, intended hypothesis fired 11/11 |
| Visualisation | model explorer, modelpoint drill-down, run-diff screen; tokenised localhost server; Arrow IPC over HTTP |
| WASM + packs | engine on `wasm32`, 882,650 B raw / 315,234 B gzipped (1.5 MB budget); single-file self-contained governance packs, optionally carrying the engine for in-browser `explain()` |
| Agent surface | `skills/predictable-migration/`, `llms.txt`, generated `llms-full.txt`, diagnostics index |
| Reference models | five: `term_annual`, `term_monthly`, `savings_monthly`, `ifrs17_gmm`, `term_solve`, each with committed `.pir` and goldens |
| Benchmarks | four implementations per scenario, correctness-gated before any timing is published |

### Test totals — release gate, 2026-08-17

| Suite | Command | Result |
|---|---|---|
| Rust workspace | `cargo test --workspace` | **785 passed**, 0 failed |
| PyO3 bindings build | `cargo test -p predictable-py` (with `PYO3_PYTHON`) | builds and links; 0 native tests (the suite is `crates/predictable-py/tests`, in Python) |
| Python — root | `pytest tests` | **62 passed** |
| Python — v2 package | `pytest packages/predictable/tests` | **278 passed** |
| Python — bindings | `pytest crates/predictable-py/tests` | **47 passed** |
| Python — reference models | `pytest models/tests` | **94 passed** |
| Frontend | `npm test` (vitest, 24 files) | **257 passed** |
| **Total** | | **1,523 tests, 0 failures** |

Gates beyond the unit suites:

- **wasm determinism gate** — every conformance case rendered to IEEE-754 bit patterns natively and
  again inside wasmtime (WASI p1, no preopens); byte-equal. Size budget checked in the same test.
- **`mkdocs build --strict`** — green.
- **Three demo scripts**, each asserting its own claim and exiting non-zero if it stops holding:
  `demos/migration` (Prophet migration reconciled to the penny, no tolerance widened, 16/16
  modelpoints, max |Δ| = 0), `demos/benchmarks` (31 engine × size comparisons in the correctness gate,
  all passing, before any of the 84 timing measurements is recorded), `demos/ifrs17` (10,000 contracts, 47 components, 101,270,000 rows, all
  GMM identities closing to ~1e-8, self-contained 11.5 MB governance pack).

---

## Known gaps and deviations — honestly

**Distribution is not done.**

1. **Not on PyPI.** No wheels are published; installation is from a checkout with a Rust toolchain.
   The docs' Getting Started page says so.
2. **v0 has not been retired to `legacy/`.** Both packages are importable as `predictable`, which
   is why the four Python suites cannot be collected in one `pytest` invocation and why building a
   model inside this repository needs `PYTHONPATH=../packages/predictable/src`. This is the single
   most confusing thing about the repository today.
3. **No migration screencast.** Demo 1 is the reproducible substitute; the video was a Phase 3
   deliverable and was not made.

**Engine and modelling limitations.**

4. **Enum-keyed tables do not resolve at runtime.** An `enum` modelpoint field used as a table key
   carries a schema variant index where the compiled table expects an engine dictionary code, so
   every lookup misses and the run aborts with `E0902 lookup_miss`. All five reference models use
   `str` for `sex` as a workaround.
5. **`f32` output storage is deferred to IR 1.1.** `run.storage_precision` accepts `"f64"` only;
   `"f32"` is `E0108`.
6. **A locally installed `predictable_engine` wheel can be older than the CLI.** `predictable build`
   currently warns `the installed engine rejects [component.meta]; provenance was written to
   spans.json only` when the venv's wheel predates the CLI binary. Rebuild with
   `maturin develop -m crates/predictable-py/Cargo.toml --release` to clear it. Nothing numeric
   depends on it, but the warning is noise a first-time user should not see.

**Verification caveats.**

7. **`emit = "outputs"` costs one step of localisation.** "Which component is the root" is partly a
   fact about what the run was asked to write down. Diff runs should use `emit = "all"`.
8. **Hypotheses are heuristics with stated support, not proofs.** Confidence is scaled to the
   modelpoint sample the detector actually held on; a claim failing on more than half the sample is
   dropped rather than downgraded. The 14/14 mutation score is against *our* catalogue of 14 seeded
   mutations — it is a regression floor, not an estimate of field performance.
9. **The Prophet side is tested against synthetic fixtures**, not against a commercial Prophet
   installation. The readers are total functions with fuzzed span invariants, but no real-world
   library of any size has been migrated outside demo 1's committed workspace.

**Performance.**

10. **predictable is not fastest at every size.** Against `cashflower` and `modelx`/`lifelib` on
    identical models, it wins by wide margins at scale (tens of × at M ≥ 100) and can *lose* at
    M = 1, where process and planning overhead dominates the projection. The published table in
    `benchmarks/results/results.md` shows both, and the correctness gate runs first every time.
11. **Benchmarks are single-machine.** The numbers carry the machine's stamp (Apple silicon, 10
    logical CPUs) and nothing else has been measured.

**Docs and API stability.**

12. **No stability guarantee.** The IR is versioned (`pir/1`) and the conformance corpus pins it,
    but the DSL surface and the CLI flags may still move before a 1.0.
13. **Screenshots in the viz docs are generated** by Playwright against the real server, so they
    track the UI — but the viz docs assume a local checkout, since there is no hosted demo.

---

## Upgrading from v0

There is no upgrade path and none is planned. v0 models are Python objects evaluated in pandas; v2
models are declarations compiled to an IR. A v0 model is rewritten, not converted — and the
migration skill was built for Prophet, not for v0.

---

## Where to start

- [Getting started](docs/v2/getting-started.md) — install, first model, first run, first diff.
- [The verification loop end to end](docs/v2/verification-walkthrough.md) — the walkthrough that
  seeds a real bug and localises it.
- [`demos/`](demos/README.md) — three scripts, three claims.
- [`docs/design/`](docs/design/README.md) — the normative specs.
