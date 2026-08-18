# Getting started

Fifteen minutes, four commands: install the tools, author a model, run it, and diff two runs.
Every command below was executed against this repository as written.

!!! note "Two products share this repository"
    `predictable` v0 — the pure-Python cashflow library documented under **v0 (legacy)** — still
    ships and still works. Everything on this page is **v2**: a Rust projection engine executing a
    serializable IR, with Python as the authoring layer. The two do not share code.

---

## 1. Install

v2 is not on PyPI yet, so build it from a checkout.

```console
$ git clone https://github.com/RatulMaharaj/predictable
$ cd predictable
$ uv venv && uv pip install -e packages/predictable maturin
$ cargo build --release -p predictable-cli          # the `predictable` CLI
$ maturin develop -m crates/predictable-py/Cargo.toml --release   # the engine wheel
```

You need a Rust toolchain (1.75+) and Python 3.11+. Two artefacts come out:

| Artefact | What it is |
|---|---|
| `target/release/predictable` | the CLI — `check`, `build`, `run`, `explain`, `diff`, `serve`, `export` |
| `predictable_engine` (wheel) | the same engine, importable from Python |

Put the CLI on your path (`export PATH=$PWD/target/release:$PATH`) or call it by path, as the
examples below do.

---

## 2. Your first model

A v2 model is Python source that *emits* IR — it is traced once at build time, never called
during projection. The five reference models under [`models/`](reference-models.md) are the
worked examples; `term_annual` is the smallest complete one.

```python title="models/term_annual/model.py (excerpt)"
@series(timing=POINT, init=bel, output=True)
def reserve(
    reserve: Money,
    premium_income: Money,
    death_claims: Money,
    renewal_expenses: Money,
    valuation_rate: Rate.annual,
    policy_term: Years,
) -> Money:
    """Retrospective reserve roll-forward, seeded from the prospective `bel`."""
    return (
        (reserve[t - 1] + premium_income[t - 1] - death_claims[t - 1] - renewal_expenses[t - 1])
        * (1 + valuation_rate)
        * when(t <= policy_term, 1.0, 0.0)
    )
```

Dependencies are the parameter names, units and timing are declarative, and `reserve` refers to
its own previous period — the engine works out the schedule. Nothing here runs per modelpoint;
the decorator traces the function once.

Build it into canonical `.pir` files:

```console
$ cd models
$ PYTHONPATH=../packages/predictable/src python -m predictable build \
      term_annual/model.py --out term_annual/build
built 4 file(s): model.pir, product.pir, schema.pir, spans.json
```

!!! warning "PYTHONPATH at the repository root"
    The repo root still holds the v0 `predictable/` package, which shadows the v2 one. Build from
    `models/` with `PYTHONPATH=../packages/predictable/src`, as above. Outside this repository the
    installed package is unambiguous and no `PYTHONPATH` is needed.

`.pir` is the artefact you review and commit — text, canonically formatted, digest-stable.
See [IR concepts](ir-concepts.md) and [Parsing .pir files](pir-syntax.md).

Check it before you run it. The checker is where the error messages live:

```console
$ ../target/release/predictable check term_annual/build/*.pir term_annual/base.pir term_annual/run.pir
5 file(s) checked, no diagnostics
```

Exit 2 means errors, 1 means lints, 0 means clean. Every code is in the
[diagnostics catalogue](../llm/diagnostics.md).

---

## 3. Your first run

`run.pir` names the modelpoint file, what to emit, the aggregations and any solves.

```console
$ cd term_annual
$ ../../target/release/predictable run run.pir --out runs/base
run_id        2026-08-17T12:00:16Z-c2387e29
out           runs/base
outcome       Completed
modelpoints   25 projected, 0 trapped, of 25
results       7325 row(s), 13 component(s)
manifest      sha256:3c47ca68623539d71016669aa8aaeb71b1641c9b2a4644b8a1b77821d1c0c985
```

The run directory holds `results.parquet`, `aggregates.parquet`, the tables actually read, and
`manifest.json` with six reproducibility digests. Re-running the same inputs gives the same bytes
— see [Determinism and goldens](determinism.md).

Ask why a number is what it is:

```console
$ ../../target/release/predictable explain runs/base \
      --component model.reserve --mp TA00001 --t 5 --depth 2
model.reserve[t=5]  TA00001  = 69.15  money point
│ (reserve[t-1] + premium_income[t-1] - death_claims[t-1] - renewal_expenses[t-1]) * (1 + valuation_rate) * if(t <= policy_term, 1.0, 0.0)
│
└─ *  69.1464
   ├─ *  69.1464
   │  ├─ -  66.8081
   │  │  ├─ -  97.4442
   │  │  │  ├─ +  122.1787
   │  │  │  │  ├─ model.reserve[t-1]  t=4  20.39                    money  point
   │  │  │  │  └─ model.premium_income[t-1]  t=4  …                 money  start
   │  │  │  └─ model.death_claims[t-1]  t=4  …                      money  end
   │  │  └─ model.renewal_expenses[t-1]  t=4  …                     money  start
   │  └─ +  1.035
   │     ├─ 1
   │     └─ valuation_rate  assumption[base]  0.0350000             rate(annual)
```

Every leaf is an input with its provenance: an assumption set, a table cell with its digest, a
modelpoint field. Full grammar in [Explaining a number](explain.md).

---

## 4. Your first diff

The verification loop is the product. Change one assumption and ask what moved:

```console
$ sed 's/^valuation_rate = 0.035/valuation_rate = 0.030/' base.pir > /tmp/base_lowrate.pir
$ ../../target/release/predictable run run.pir --assumptions /tmp/base_lowrate.pir --out runs/lowrate
$ ../../target/release/predictable diff run runs/base runs/lowrate --tolerance-profile regression
  tolerance  regression   abs 0.000000001  rel 0.000000000001

  DIVERGED   650 of 7,325 cells   3 root divergences   6 outputs affected

  ▸ F001  root   pv_claims   affects bel, profit_margin, pv_claims, reserve   31% of Δreserve
      RESERVE diverges at t=-1 in component pv_claims: 217.49 vs 222.86  (Δ 5.36, 2.4%)  [mp TA00001]
      first at t=-1, persists to t=-1, 25 of 25 modelpoints, 25 cells
      exemplar TA00001      worst TA00024 t=-1  6265.86 vs 6826.61  (Δ 560.75, 8.2%)
      → predictable explain runs/lowrate --component pv_claims --mp TA00001 --t -1

  ▸ F004  inherited   reserve   affects reserve   100% of Δreserve
      …
```

The three **root** findings are where the change entered; the rest are inherited downstream. That
separation — and the `explain` command printed under each finding — is what makes a reconciliation
finite rather than a spreadsheet hunt. Exit code is 0 within tolerance, 1 diverged, 2 incomparable,
so this belongs in CI.

The same command diffs a predictable run against a **Prophet `.rpt`** with
`--mapping migration/mapping.toml`, which is how a migration is proved. Read
[Diffing two runs](run-diff.md), then the
[verification walkthrough](verification-walkthrough.md) end to end.

---

## 5. Look at it

```console
$ ../../target/release/predictable serve build/*.pir --out-json /tmp/serve.json
```

That prints (and writes) a tokenised `127.0.0.1` URL for the Model Explorer — the dependency
graph, the modelpoint drill-down and the run-diff screen. To hand a reviewer something offline,
`predictable export runs/base --out pack.html` writes a single self-contained HTML file with the
results, the traces and — with `--engine wasm` — the engine itself.

---

## Where to go next

- **Concepts** — [IR concepts](ir-concepts.md), [Declaring a model](dsl-declarations.md),
  [Substage levels and terms](substage-levels.md)
- **Verification** — [Diffing two runs](run-diff.md),
  [Hypotheses and mutation testing](hypotheses.md)
- **Demos** — [migrating a Prophet library](demo-migration.md),
  [an IFRS 17 valuation](ifrs17-valuation.md)
- **Specs** — [`docs/design/`](../design/README.md) is normative; the pages above are the manual.
