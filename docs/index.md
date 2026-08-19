# Predictable

**An open-source actuarial modelling framework for life insurance cashflow projection.** A Rust
projection engine executes a serializable, git-diffable computation-graph IR; a Python DSL is the
authoring layer that compiles to that IR. No arbitrary Python callables cross the engine boundary,
so a model is an artefact you can review, digest, diff and reproduce.

[Get started →](v2/getting-started.md){ .md-button .md-button--primary }
[Read the design specs →](design/README.md){ .md-button }

!!! warning "Pre-release"
    v2 is feature-complete against its specs and heavily tested, but it has not been used in
    anger outside this repository and is not yet on PyPI. See
    [`RELEASE_NOTES_v2.md`](https://github.com/RatulMaharaj/predictable/blob/main/RELEASE_NOTES_v2.md)
    for what exists and what is missing.

---

## The thesis

An LLM should be able to read a Prophet model and reimplement it in predictable in minutes to
hours — and then **prove** the reimplementation right. The verification loop is the product:

<div class="grid cards" markdown>

- **Definition-time validation**

    A checker with Rust/Elm-grade, machine-actionable diagnostics. Errors carry a code, a span,
    the rule and a suggested edit. [Checking a model](v2/checking.md)

- **`explain()` provenance**

    Any cell, replayed as a tree down to assumptions, table cells (with digests) and modelpoint
    fields. No number is unexplained. [Explaining a number](v2/explain.md)

- **Structured run diff**

    Two runs — or a run against a Prophet `.rpt` — reconciled into *root* divergences and
    inherited noise, with ranked hypotheses about the cause. [Diffing two runs](v2/run-diff.md)

- **Byte-level determinism**

    Same inputs, same bytes, on any thread count or chunk size — checked natively and again
    inside wasm. [Determinism and goldens](v2/determinism.md)

</div>

## What it looks like

```console
$ predictable run run.pir --out runs/base
modelpoints   25 projected, 0 trapped, of 25
results       7325 row(s), 13 component(s)
manifest      sha256:3c47ca68623539d71016669aa8aaeb71b1641c9b2a4644b8a1b77821d1c0c985

$ predictable diff run runs/base runs/candidate --tolerance-profile regression
  DIVERGED   650 of 7,325 cells   3 root divergences   6 outputs affected
  ▸ F001  root   pv_claims   affects bel, profit_margin, pv_claims, reserve   31% of Δreserve
```

## Three demos, three claims

Each is a script that asserts its own result and fails loudly if the claim stops being true.

| | claim |
|---|---|
| [Migrating a Prophet library](v2/demo-migration.md) | an agent migrates a Prophet library and proves it right — reconciled to the penny, no tolerance widened |
| [Reproducing the benchmarks](v2/demo-benchmarks.md) | performance numbers are gated on correctness: nothing is timed until every engine agrees |
| [An IFRS 17 sample valuation](v2/ifrs17-valuation.md) | a GMM valuation over 10,000 contracts, reconciled, traceable to one cell |

## Where things are

- **[Getting started](v2/getting-started.md)** — install, first model, first run, first diff.
- **Concepts** — the IR, the DSL, the engine, verification, visualisation, as a manual.
- **Reference** — the CLI, the Python API, run results, diagnostics.
- **Design specs** — [`docs/design/`](design/README.md) is normative; the manual pages follow it.
- **v0 (legacy)** — the original pure-Python library. Still installable via `pip install
  predictable`, documented under [Installation](installation.md) and [Tutorial](tutorial.md), and
  unrelated to v2's code.

Licensed MIT. Source at [RatulMaharaj/predictable](https://github.com/RatulMaharaj/predictable).
