# Reference models

Five worked models, each as **DSL source** and as **committed canonical `.pir`**, with
synthetic fixtures, run configuration and golden results. This is the corpus the rest of
the project is tested against: benchmarks (T34), model diff (T25), run diff (T26), the
visualisation screens and the launch demos all point here.

Actuarial documentation — product, basis, every formula, worked examples — is on the docs
site: **[`docs/v2/reference-models.md`](../docs/v2/reference-models.md)**. This file is the
operating manual.

| Model | Basis | Periods | Modelpoints | Components | What it exercises |
|---|---|---|---|---|---|
| `term_annual` | annual | 40 | 25 | 22 | the `01-ir.md` §6 model: lookups, recursion, `npv`, the `init` back-channel |
| `term_monthly` | monthly | 480 | 25 | 25 | the `t`-loop constant; annual → monthly rate conversion |
| `savings_monthly` | monthly | 360 | 20 | 49 | an account value, `Select`-heavy branching, four table lookups per period, curve discounting |
| `ifrs17_gmm` | monthly | 360 | 20 | 75 | the IFRS 17 GMM: CSM, RA, coverage units, LRC/LIC, **two substage levels** |
| `term_solve` | annual | 40 | 25 | 22 | `[[solve]]`: Brent on the premium to a zero BEL |

`term_solve` shares `term_annual`'s `model_digest` exactly — the two differ only in
`run.pir`.

## Layout

```text
models/<name>/
  model.py            the model, in the Python DSL          (source of truth)
  build/*.pir         the same model, canonical             (predictable build)
  base.pir            the assumption set
  run.pir             modelpoints, emit, aggregations, solves
  tables/*.csv        table fixtures                        (make_fixtures.py)
  data/modelpoints.csv                                      (make_fixtures.py)
  expected/           goldens                               (predictable run + goldens.py)
  runs/               scratch output — not committed
```

## Commands

```console
# prerequisites
$ cargo build -p predictable-cli

# regenerate fixtures, .pir builds, runs and goldens (all five, or one)
$ python models/tools/regen.py
$ python models/tools/regen.py ifrs17_gmm

# prove the committed corpus reproduces
$ pytest models/tests -q

# run one model by hand
$ cd models/term_annual && ../../target/debug/predictable run run.pir --out runs/base
```

`regen.py` invokes the real `predictable build` and `predictable run`, never a shortcut
through the library, so what is committed is what the shipped tools produce.

### A note on `PYTHONPATH`

The repository root still contains the v0 `predictable/` package, which shadows
`packages/predictable/src/predictable`. `regen.py` and the tests set `PYTHONPATH` and run
from `models/` to get the v2 DSL. Invoking `predictable build` by hand needs the same:

```console
$ cd models && PYTHONPATH=../packages/predictable/src python -m predictable build \
      term_annual/model.py --out term_annual/build
```

## One known limitation the corpus works around

**Enum-keyed tables.** `sex` is a `str` field in all five models, not an `enum(Sex)`. An
`enum` modelpoint field used as a *table key* does not resolve at runtime today: the lane
carries a schema variant index (`predictable-runner::chunk::lane_of`) where the compiled
table's dictionary index expects an engine dictionary code, so every lookup misses and the
run aborts with `E0902 lookup_miss`. `str` keys work correctly. When that seam is closed,
`sex` should become an enum in all five models and the goldens regenerated.

## Closed: chunk-size invariance of `results.parquet` bytes

This used to be a second limitation, carried as an `xfail`: values were identical under any
`--chunk-size` but the Parquet *encoding* was not, so `results_digest` moved on the larger
models. The writer now drains Arrow batches in exact `batch_rows` slices and pins every
layout knob to a row count, so the bytes are a function of the row sequence alone.
`test_chunk_size_does_not_change_the_result_bytes` asserts it strictly on all five models —
see [`docs/v2/parquet-determinism.md`](../docs/v2/parquet-determinism.md).
