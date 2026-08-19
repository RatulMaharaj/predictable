# The Python API

`predictable_engine` is the compiled extension module that puts the Rust projection engine
behind four Python objects: `Program`, `Plan`, `Run`, and the `check()` function. It is what
the DSL builds on, and it is usable directly from a notebook.

The surface is deliberately small. Python authors and validates; the engine compiles and
executes. **No Python callback is ever invoked during a projection**, which is exactly why
the GIL can be released for the whole run.

```python
from predictable_engine import Program, check

program = Program.from_pir("model/")        # a directory, a file, or a list of files
diagnostics = check(program)                # list[dict] — the --json diagnostics
plan = program.plan()                       # raises CheckError if the model does not check
result = plan.run("modelpoints.csv")        # releases the GIL for the projection
table = result.to_arrow()                   # pyarrow.Table, zero copy
```

## Installing

The wheel is `abi3-py39`: one wheel per platform covers every Python ≥ 3.9.

```bash
maturin develop -m crates/predictable-py/Cargo.toml   # from a checkout
pip install predictable-engine                         # from PyPI
```

`pyarrow` is **optional**. Without it you still get results — through the Arrow C stream
capsule, which any Arrow-compatible library can read — you just do not get `to_arrow()`
and `to_pandas()`.

## `Program` — sources plus provenance

A program is a set of `.pir` files: modules, an optional `[run]` file, optional assumption
sets. Each file is classified by its own content, never by its name.

```python
Program.from_pir(paths, *, base_dir=None)     # paths: a file, a directory, or a list
Program.from_sources({"term.pir": text})      # in-memory — the DSL's emit path
```

| Member | What it is |
|---|---|
| `.files` | file names, in order |
| `.kinds` | `{name: "module" \| "run" \| "assumption_set" \| "product"}` |
| `.digest` | `model_digest` over the **canonical** text of every module |
| `.check()` | every diagnostic, as dicts; never raises |
| `.plan(*, retain_all=False, periods=None, optimise=True)` | a `Plan` |

`.digest` is content-addressed, not layout-addressed: reformatting a file does not change it,
and neither does the order you passed the files in.

```python
a = Program.from_sources({"a.pir": src_a, "b.pir": src_b})
b = Program.from_sources({"b.pir": src_b, "a.pir": src_a})
assert a.digest == b.digest
```

### Checking is separate from planning — on purpose

`check()` never raises. A model that does not compile is exactly the model you most need to
inspect, so reporting the breakage *is* the job:

```python
for d in check(program):
    print(d["code"], d["message"])
    for suggestion in d["suggestions"]:
        for edit in suggestion["edits"]:
            print("  fix:", edit["file"], edit["range"], "->", edit["replacement"])
```

Every diagnostic carries a literal byte-range edit rather than prose about a fix, so an agent
can apply it and re-check with no human in the middle.

`plan()`, by contrast, raises when the model does not check:

```python
try:
    plan = program.plan()
except CheckError as e:
    print(e.pretty)          # the rendered, snippet-annotated diagnostic
    codes = [d["code"] for d in e.diagnostics]
```

## `Plan` — checked, ordered, lowered

Planning does the expensive, deterministic work once: slot allocation, the evaluation order
and its `order_digest`, tape lowering. A plan is reusable, which is what makes a sensitivity
fan cheap.

| Member | What it is |
|---|---|
| `.digest` | `sha256(program_digest ‖ run config ‖ engine major)` |
| `.order_digest` | the hash of the evaluation order |
| `.periods` | `T`, from `[timeline].periods` |
| `.components` | the qualified ids the result set will carry |
| `.run(...)` | project — see below |

```python
plan.run(
    modelpoints=None,          # path | pyarrow.Table | RecordBatchReader | DataFrame | None
    *,
    assumptions=None,          # {name: float}, overriding the declared assumption set
    threads=None,              # None = the machine's pool, 1 = strictly serial
    chunk_size=None,           # defaults to [run.exec].chunk_size, then 1024
    progress=None,             # callable(done_chunks, total_chunks)
    allow_table_drift=False,
)
```

`modelpoints=None` reads `[run].modelpoints`, resolved relative to the program's base
directory. `.csv` and `.parquet` are read directly; anything exporting `__arrow_c_stream__`
is adopted without a copy; a pandas DataFrame is converted by `pyarrow` first — one copy, on
your side, visible in a profile.

Neither `threads` nor `chunk_size` can change a number:

```python
assert plan.run(chunk_size=1).to_arrow() == plan.run(chunk_size=1024).to_arrow()
assert plan.run(threads=1).to_arrow() == plan.run(threads=8).to_arrow()
```

## `Run` — a finished projection, as Arrow

Results are the long format: one row per `(modelpoint, component, t)`.

| Column | Type | Meaning |
|---|---|---|
| `mp_key` | `string` | the modelpoint's key field |
| `mp_row` | `uint32` | its row in the modelpoint file |
| `component` | `dictionary<int32, string>` | the qualified component id |
| `stage` | `int8` | `1` projection, `2` reduction |
| `t` | `int32` | `0..=T` for a series, `-1` for a `PerMP` or `Scalar` |
| `value` | `double` | `f64` components |
| `value_i` / `value_b` / `value_s` | `int64` / `bool` / `dictionary` | the other dtypes |

The long format is not a convenience. It is what makes two runs that emitted *different*
component sets still structurally comparable, and what lets series and per-modelpoint values
share one table without a ragged schema.

```python
result = plan.run("modelpoints.csv")

result.outcome            # "completed" | "completed_with_traps" | "cancelled"
result.exit_code          # 0 | 1
result.rows               # rows in the long format
result.modelpoints        # modelpoints that produced rows
result.modelpoints_trapped
result.trap_count
result.components
result.traps              # the E0902 envelopes
result.aggregates         # [[aggregation]] rows
result.solves             # one outcome block per [[solve]]

result.to_arrow()         # pyarrow.Table — zero copy
result.to_pandas()        # via Arrow; one copy, on pandas' side
result.__arrow_c_stream__()   # the raw capsule, no pyarrow needed
```

### Reading one component

```python
import pyarrow.compute as pc

table = result.to_arrow()
bel = table.filter(pc.equal(table["component"], "term.bel"))
print(dict(zip(bel["mp_key"].to_pylist(), bel["value"].to_pylist())))
```

## Zero copy, in both directions

Results cross the boundary through the **Arrow C Data Interface**: the batch this process
allocated is handed over as an `arrow_array_stream` PyCapsule and `pyarrow` adopts the same
buffers, with ownership transferred through the release callback. Nothing is serialised and
nothing is copied.

```python
import pyarrow as pa

table = pa.table(result)              # equivalent to result.to_arrow()
```

Inbound modelpoints take the same path in reverse:

```python
mps = pa.table({
    "policy_number": ["POL1", "POL2"],
    "sum_assured": [100_000.0, 250_000.0],
    "premium": [900.0, 1_500.0],
    "q": [0.01, 0.02],
})
result = plan.run(mps)
```

The results schema is fixed and cannot be renegotiated: passing a `requested_schema` raises,
because quietly casting a result set would make two runs incomparable.

## The GIL, progress and `Ctrl-C`

`plan.run()` wraps the entire projection in `Python::allow_threads`. Other Python threads
keep running while it projects:

```python
import threading, time

ticks = [0]
stop = threading.Event()
threading.Thread(target=lambda: [ (ticks.__setitem__(0, ticks[0] + 1), time.sleep(0.001))
                                  for _ in iter(lambda: not stop.is_set(), False) ]).start()
result = plan.run("big.csv")
stop.set()
assert ticks[0] > 0          # the interpreter was never blocked
```

The progress callback is invoked **between chunk groups**, with the GIL retaken, at most once
every 100 ms, and it receives counts only — it cannot influence a result:

```python
plan.run("big.csv", progress=lambda done, total: print(f"{done}/{total} chunks"))
```

A callback that raises, and a `Ctrl-C`, both mean the same thing: stop. The runner finishes
the chunk in flight, sets the cancel flag and raises `KeyboardInterrupt` — no `SIGKILL`, and
no truncated result set that looks complete.

## Errors are rendered diagnostics

Every failure is a subclass of `PredictableError` carrying `.diagnostics` (the JSON of the
diagnostics catalogue) and `.pretty` (the rendered terminal form).

```text
PredictableError
├── ParseError    the .pir text does not parse
├── CheckError    it parses but does not check
├── DataError     modelpoints, tables or run configuration are unusable
└── TrapError     the projection trapped under on_trap = "abort"
```

```python
from predictable_engine import CheckError, DataError, TrapError

try:
    result = plan.run("modelpoints.csv")
except TrapError as e:
    first = e.diagnostics[0]
    print(first["code"], first["mp_key"], first["component"], first["t"])
    print(first["operands"])     # the real operand values, captured on replay
```

Under `on_trap = "abort"` (the default) a trap raises `TrapError` and there is **no** result
set — a short result set that looked complete would be the worse outcome. Under
`on_trap = "continue"` the trapping modelpoint is dropped entirely, `outcome` becomes
`completed_with_traps` and `exit_code` becomes `1`:

```python
result = plan.run("modelpoints.csv")      # run file says on_trap = "continue"
assert result.outcome == "completed_with_traps"
assert result.modelpoints_trapped == 1
assert result.traps[0]["code"] == "E0902"
```

## A worked example, end to end

```python
from predictable_engine import Program

TERM = """
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

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "q"
dtype = "f64"
unit = "prob"
required = true

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
expr = "sum(claims)"
"""

program = Program.from_sources({"term.pir": TERM})
assert [d for d in program.check() if d["severity"] == "error"] == []

plan = program.plan()
result = plan.run("modelpoints.csv")

table = result.to_arrow()
print(result.outcome, result.modelpoints, "modelpoints,", result.rows, "rows")
```

## Solves and aggregations

A `[[solve]]` in the run file runs *before* the final projection, so the numbers you get back
are the solved ones, and the solved value appears in the result set as an ordinary `PerMP`
component — a downstream join never has to special-case a solve.

```toml
[[solve]]
name = "breakeven_premium"
target = "bel"
to = 0.0
vary = "premium"
scope = "per_mp"
bracket = [0.0, 100000.0]
```

```python
result = plan.run("modelpoints.csv")
outcome = result.solves[0]
print(outcome["converged"], "converged,", outcome["not_converged"], "did not")

rows = result.to_arrow().to_pylist()
solved = {r["mp_key"]: r["value"] for r in rows if r["component"] == "breakeven_premium"}
```

`[[aggregation]]` rows arrive on `result.aggregates`, folded in chunk index order — so the
chunk size makes no difference to a single aggregate:

```python
{row["group_key"]: row["value"] for row in result.aggregates}
# {'product_code=TERM_UK': -3.0e5, 'product_code=TERM_IE': -1.1e5}
```

## Building the wheel

```bash
export PYO3_PYTHON=$PWD/.venv/bin/python
maturin build --release -m crates/predictable-py/Cargo.toml
pytest crates/predictable-py/tests
```

`abi3-py39` means the wheel is built once per platform, not once per Python version, and the
Rust toolchain is pinned — cross-platform bit-identity is only testable against a fixed
compiler.
