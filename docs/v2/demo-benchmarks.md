# Demo: reproducing the benchmarks

One command reproduces the published benchmark, in the order that matters: **the
correctness gate first, and nothing is timed until every engine agrees on every number.**

```console
$ ./demos/benchmarks/run_demo.sh
...
BENCHMARK DEMO: OK

$ GATE_ONLY=1 ./demos/benchmarks/run_demo.sh   # ~1 minute; the part CI must not skip
```

The methodology, the metric and the caveats are on [the benchmarks page](benchmarks.md);
the operating manual is
[`benchmarks/README.md`](https://github.com/RatulMaharaj/predictable/blob/main/benchmarks/README.md)
and the demo's own notes are
[`demos/benchmarks/README.md`](https://github.com/RatulMaharaj/predictable/blob/main/demos/benchmarks/README.md).
This page is why the demo exists.

---

## The claim

> The performance numbers are **gated on correctness**. A scenario whose engines disagree
> is not timed at all, and the report says so in place of its numbers.

Most benchmarks are a timing harness with a correctness check bolted on, if they have one.
This one is built the other way round: it is a correctness harness that also records
timings, and the timings are a side effect of a comparison that has already passed.

## What the demo runs

1. **The gate.** Four implementations of the same five models — predictable, cashflower,
   modelx/lifelib, and a hand-written NumPy reference — projected at `M ∈ {1, 25}`, and
   predictable additionally at `M = 1000` to cross its 1024-row chunk boundary. **Every
   output, for every model point, for every period** is compared, not a summary.
2. **Only then, the timings**, for the scenarios that cleared.
3. **Assertions**, including that every timed scenario passed the gate first, and that the
   reference the gate compares against is NumPy rather than predictable.

That last assertion is the load-bearing one. The gate has to be able to *fail* predictable,
and it cannot do that if predictable is the yardstick. The NumPy reference is in turn
pinned to the committed goldens under `models/*/expected/`, which it reproduces to the last
bit — so the chain terminates in something that was checked in and reviewed rather than in
an opinion formed at run time.

## What the gate output looks like

```text
pass  term_annual      M=1        cashflower   bel 5.09e-16
pass  term_annual      M=1        modelx       bel 0.00e+00
pass  term_annual      M=25       predictable  bel 0.00e+00
...
pass  ifrs17_gmm       M=1000     predictable  bel 0.00e+00
pass  term_solve       M=25       cashflower   bel 3.64e-12
```

Three things to read out of it:

* **`0.00e+00` for predictable** at every size on the four non-solve scenarios. Not "within
  tolerance" — bit-identical to the NumPy reference. `term_solve` is the exception at
  `~1e-10`, and that is the solver's own tolerance rather than the engine's: Brent and
  bisection converge on the same root from different directions.
* **`M = 1000` for predictable specifically.** The other engines project one policy at a
  time with no state shared between policies, so their answer for a policy cannot depend on
  how many others are in the file. predictable is chunked, so it can, and the extra size
  crosses the 1024-row boundary where it would show.
* **The error metric is scale-relative**, `max|a−b| / max(max|b|, floor)`. A per-cell
  relative gate sounds stricter and is useless here: `reserve` after run-off is a number
  like `1e-17`, and two engines differing in its last bit differ by 100% relatively while
  differing by `1e-17` on a component whose scale is thousands. The choice is defended and
  tested in [`benchmarks/tests/test_gate.py`](https://github.com/RatulMaharaj/predictable/blob/main/benchmarks/tests/test_gate.py),
  including that a `1e-8` relative difference *fails*.

## Reading the timings honestly

The published tables are in `benchmarks/results/results.md`, with the same content as
structured data in `results.json` and an environment block naming the machine, the Python
version and every engine's version. A benchmark number without a machine attached is
decoration.

Before quoting any of them, read [§5 of the benchmarks page](benchmarks.md#5-caveats-read-these-before-quoting-a-number).
The short version:

* **predictable pays for its output and the others do not.** It is timed through the CLI —
  parse, check, plan, build tables, load model points, project, and write
  `results.parquet` with every declared output for every model point for every period. The
  other three are timed in process, computing values and mostly discarding them. The
  difference favours the other three.
* **`term_solve` compares roots, not solvers.** Brent against bisection: same answer, far
  fewer model evaluations.
* **`ifrs17_gmm` has two implementations, not four.** Its structure — a whole-projection
  reduction feeding a forward recursion that is reduced again — is what a single-pass
  scalar framework is worst at expressing, and transcribing it faithfully into cashflower
  and modelx is a project rather than a benchmark entry. The gate still runs it against
  NumPy; the cross-engine columns are simply absent and the table says so.
* **One machine, one run.** The ratios are the durable part, and even those should be
  re-measured before being quoted.

## Setup

The three competing engines live in their own environment, which the script creates on
first run:

```console
$ cd benchmarks
$ uv venv .venv --python 3.12
$ uv pip install --python .venv/bin/python cashflower lifelib modelx numpy pandas pyarrow pytest
$ cargo build --release -p predictable-cli     # from the repository root
```

A debug build measures the wrong thing, so the demo builds `--release` if the binary is
missing rather than using whatever is there.
