# Demo 2 — the benchmark, gate first

```console
$ ./demos/benchmarks/run_demo.sh
...
BENCHMARK DEMO: OK
```

One command: set up the contenders' environment if it is missing, build the CLI in release,
run the **correctness gate**, and only then run the timings and assert that every timed
scenario cleared the gate first.

* Methodology, metric and caveats: [`docs/v2/benchmarks.md`](../../docs/v2/benchmarks.md)
* Why this demo exists: [`docs/v2/demo-benchmarks.md`](../../docs/v2/demo-benchmarks.md)
* Operating manual for the harness itself: [`benchmarks/README.md`](../../benchmarks/README.md)
* The published tables: `benchmarks/results/results.md` and `results.json`

---

## The order is the claim

```text
1. gate      four engines, five scenarios, every output x every model point x every period
2. timings   only for the scenarios the gate cleared
3. assert    every timed scenario passed the gate; the gate's reference is NumPy
```

A scenario whose engines disagree is **not timed at all**, and the report records the
reason in place of its numbers. The assertion that the gate's reference is NumPy rather
than predictable is in the script because a gate that cannot fail the engine it is
defending is decoration.

## Options

```console
$ GATE_ONLY=1 ./demos/benchmarks/run_demo.sh          # the gate alone, ~1 minute
$ REPEATS=5 THREADS=1,2,4,8 ./demos/benchmarks/run_demo.sh
```

`GATE_ONLY=1` is the mode CI should run on every commit. The full timing run takes hours,
writes multi-gigabyte result files at the larger sizes, and is a release activity.

## First-run setup

The three competing engines need their own environment; the script creates it with `uv` if
`benchmarks/.venv` is missing:

```console
$ cd benchmarks
$ uv venv .venv --python 3.12
$ uv pip install --python .venv/bin/python cashflower lifelib modelx numpy pandas pyarrow pytest
```

---

## The published run

Produced by exactly the command above. Environment, recorded with the numbers because a
benchmark figure without a machine attached is decoration:

| | |
|---|---|
| platform | macOS 26.6.1, arm64, 10 logical CPUs |
| python | 3.12.13 · numpy 2.0.1 |
| engines | predictable 0.0.1 · cashflower 0.10.8 · modelx 0.32.0 / lifelib 0.14.0 |
| settings | `--repeats 3 --threads 1,2,4,8` |
| recorded | 2026-08-17 |

Full tables: `benchmarks/results/results.md` (and `results.json`).

### 1. The gate — the part that has to come first

**31 comparisons, 5 scenarios, 4 engines, 0 failures.** Every output, for every model
point, for every period, against the NumPy reference at `1e-9` scale-relative.

| scenario | M | engine | cells compared | worst component | worst error |
|---|---:|---|---:|---|---:|
| `term_annual` | 1,000 | predictable | 293,000 | `bel` | **0.00e+00** |
| `term_monthly` | 1,000 | predictable | 3,373,000 | `bel` | **0.00e+00** |
| `savings_monthly` | 1,000 | predictable | 5,425,000 | `bel` | **0.00e+00** |
| `ifrs17_gmm` | 1,000 | predictable | 9,405,000 | `bel` | **0.00e+00** |
| `term_solve` | 1,000 | predictable | 294,000 | `bel` | 3.77e-10 |
| `term_annual` | 25 | cashflower | 7,325 | `reserve` | 1.08e-15 |
| `term_annual` | 25 | modelx | 7,325 | `reserve` | 1.08e-15 |
| `savings_monthly` | 25 | cashflower | 135,625 | `bel` | 6.88e-15 |
| `term_monthly` | 25 | modelx | 84,325 | `profit_margin` | 2.57e-15 |

Read the zeros literally: on the four non-solve scenarios predictable is **bit-identical**
to the NumPy reference at every size, not merely within tolerance. `term_solve` is the
exception at `~4e-10`, and that is the solver's tolerance rather than the engine's — Brent
and bisection converge on the same root from different directions, and
`models/term_solve/run.pir` asks for `1e-8`.

The other two engines land at `1e-15`, which is float addition order and nothing else.

### 2. Wall time — the headline sizes

Median of the warm runs, single-threaded. `mp-periods/s` = `M × (T+1) / median`.

| scenario | M | predictable | cashflower | modelx | NumPy |
|---|---:|---:|---:|---:|---:|
| `term_annual` (40p) | 1,000 | **68 ms** | failed | 4.25 s | 3.8 ms |
| `term_annual` | 1,000,000 | **67.1 s** | — | — | 2.63 s |
| `term_monthly` (480p) | 1,000 | **872 ms** | 2.72 s | — | 50.8 ms |
| `term_monthly` | 100,000 | **92.1 s** | — | — | 4.72 s |
| `savings_monthly` (360p) | 1,000 | **1.54 s** | 3.82 s | — | 52.0 ms |
| `savings_monthly` | 100,000 | **153.5 s** | — | — | 4.38 s |
| `ifrs17_gmm` (360p) | 10,000 | **30.3 s** | n/i | n/i | 537 ms |
| `term_solve` (40p) | 100 | **17.5 ms** | 2.55 s | 20.74 s | 84.8 ms |

Throughput, the size-independent number: **0.5–0.7 M model-point-periods per second** on a
single thread across the four projection scenarios, essentially flat from `M = 1,000` to
`M = 1,000,000` — 602 k/s at 1,000 and 611 k/s at a million on `term_annual`. Flat is the
result worth having: it means the chunk pipeline is not degrading with portfolio size.

Ratios against the scalar frameworks: **2.5–3.2× cashflower** and **30–62× modelx** where
both engines reach the size, rising to **145× cashflower** and **1,183× modelx** on
`term_solve` — where, as the caveats say, the comparison is Brent against bisection and is
measuring the solver as much as the engine.

### 3. The three things this table says that are not flattering

**NumPy is faster, by 4× to 25×, and the table says so.** predictable is timed through the
CLI, so its number includes parse, check, plan, table build, model point load, projection
*and writing `results.parquet` with every declared output for every model point for every
period*. At `M = 1,000,000` on `term_annual` that is 533 million result rows written to
disk; the NumPy reference computes the same values in memory and discards them. These are
not the same amount of work, and the difference is entirely in NumPy's favour. The number
to compare against NumPy's is not on this table, because the harness does not measure
predictable with output suppressed — and until it does, the honest statement is "NumPy is
faster at this, for reasons that include a large amount of I/O predictable is doing and it
is not".

**Thread scaling is poor: 1.13× at 8 threads.** `term_annual` at `M = 1,000,000` goes
67.1 s → 64.4 s → 60.0 s → 59.3 s for 1, 2, 4 and 8 threads. That is not a projection that
fails to parallelise — it is a run that is dominated by the Parquet write, which is
sequential. `term_solve` at `M = 10,000`, whose output is small, scales 763 ms → 597 ms
(1.28×) over the same range, which is better and still not linear. Thread scaling at these
sizes is measuring the writer.

**Peak RSS is high and is reported as an upper bound.** It is sampled over the whole process
tree at 10 ms and includes a ~100 MB interpreter floor plus the harness's own comparison
buffers, so a 1,686 MB figure at `M = 1` is the harness, not the engine. It should be read
as an upper bound with a constant offset rather than as a footprint.

### 4. What did not run, and why

* **`cashflower` fails on `term_annual` at `M ≥ 100`** with `KeyError: 'TA00026'` — the
  transcription indexes a fixture that only exists in the 25-policy lattice. It is recorded
  as a failure in the "Not run" section of `results.md` rather than dropped, because a
  missing row and a failing row are different facts.
* **`ifrs17_gmm` has no cashflower or modelx implementation.** Its shape — a
  whole-projection reduction feeding a forward recursion that is reduced again — is what a
  single-pass scalar framework is worst at expressing. The gate still runs it against NumPy.
* **`M = 10,000,000` is not run here at all.** It is the CI-runner size of
  `03-engine.md` §11.3, and until it has run there this repository publishes no number for
  it.
* **Per-engine size caps** (`bench/scenarios.py`) stop a scalar Python engine being asked
  for a multi-day run. Each cap is printed next to the numbers rather than appearing as a
  silently absent row.



## Notes

* The demo does **not** rewrite `docs/v2/benchmarks.md` (it passes `--no-docs`). Refreshing
  the generated tables on that page is `python -m bench` without the flag, and is a
  deliberate act rather than a side effect of running a demo.
* The gate is fast because it runs at `M ∈ {1, 25}` plus `M = 1000` for predictable alone.
  The other three engines project one policy at a time with no state shared between
  policies, so their answer for a policy cannot depend on how many others are in the file;
  predictable is chunked, so it can, which is why it gets the extra size.
* `benchmarks/tests/` is the harness's own regression suite, including the tests that prove
  the gate metric rejects a `1e-8` relative difference:

  ```console
  $ benchmarks/.venv/bin/python -m pytest benchmarks/tests -q
  ```
