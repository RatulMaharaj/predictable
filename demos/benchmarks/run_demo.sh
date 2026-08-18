#!/usr/bin/env bash
# demos/benchmarks/run_demo.sh — reproduce the published benchmark, one command.
#
# The order is the point: the correctness gate runs first and a scenario whose engines
# disagree is never timed. The script exits non-zero if the gate fails, whatever the
# timings would have said.
#
#   ./demos/benchmarks/run_demo.sh              gate + the published timing run
#   GATE_ONLY=1 ./demos/benchmarks/run_demo.sh  the gate alone (~1 minute; CI must not skip it)
#   REPEATS=5 THREADS=1,2,4,8 ./demos/benchmarks/run_demo.sh
#
set -euo pipefail

DEMO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DEMO/../.." && pwd)"
BENCH="$ROOT/benchmarks"
REPEATS="${REPEATS:-3}"
THREADS="${THREADS:-1,2,4,8}"
GATE_ONLY="${GATE_ONLY:-0}"

step() { printf '\n\033[1m── %s\033[0m\n' "$1"; }
say() { printf '\033[2m$ %s\033[0m\n' "$*"; }

# --------------------------------------------------------------------------------- setup
PY="$BENCH/.venv/bin/python"
if [ ! -x "$PY" ]; then
  step "0.  One-off setup — the four contenders need their own environment"
  say "uv venv $BENCH/.venv --python 3.12 && uv pip install cashflower lifelib modelx numpy pandas pyarrow pytest"
  (cd "$BENCH" && uv venv .venv --python 3.12 \
    && uv pip install --python .venv/bin/python cashflower lifelib modelx numpy pandas pyarrow pytest)
fi

if [ ! -x "$ROOT/target/release/predictable" ]; then
  step "0.  Building the CLI in release — a debug build measures the wrong thing"
  (cd "$ROOT" && cargo build --release -p predictable-cli >/dev/null)
fi

# ----------------------------------------------------------------------------- the gate
step "1.  The correctness gate.  Nothing is timed until every engine agrees."
say "python -m bench --gate-only"
(cd "$BENCH" && "$PY" -m bench --gate-only)

if [ "$GATE_ONLY" = "1" ]; then
  echo
  echo "BENCHMARK DEMO: gate OK (timings skipped: GATE_ONLY=1)"
  exit 0
fi

# --------------------------------------------------------------------------- the timings
step "2.  The published run"
say "python -m bench --repeats $REPEATS --threads $THREADS"
(cd "$BENCH" && "$PY" -m bench --repeats "$REPEATS" --threads "$THREADS" --no-docs)

# ------------------------------------------------------------------------------ assertions
step "3.  Assertions"
"$PY" - "$BENCH" <<'PY'
import json, sys
from pathlib import Path

bench = Path(sys.argv[1])
report = json.loads((bench / "results" / "results.json").read_text())
ok = True


def check(label, cond, detail=""):
    global ok
    ok = ok and bool(cond)
    print(f"  {'PASS' if cond else 'FAIL'}  {label}{'  ' + detail if detail else ''}")


gate = report["gate"]
check("the gate ran", len(gate) > 0, f"{len(gate)} engine x size comparisons")
check("every gate comparison passed", all(g["passed"] for g in gate),
      ", ".join(f"{g['scenario']}/{g['engine']}" for g in gate if not g["passed"]) or "none failed")
check("the gate covers all five scenarios",
      len({g["scenario"] for g in gate}) == 5)
check("the reference is NumPy, not predictable",
      all(g["engine"] != "numpy" for g in gate))
check("timings were recorded", len(report["timings"]) > 0,
      f"{len(report['timings'])} measurements")
check("every timed scenario passed the gate first",
      {m["scenario"] for m in report["timings"]} <= {g["scenario"] for g in gate if g["passed"]})
check("the environment is recorded with the numbers",
      all(k in report["environment"] for k in ("platform", "python")),
      report["environment"].get("platform", "?"))
check("results.md was written", (bench / "results" / "results.md").exists())

print()
print("BENCHMARK DEMO: " + ("OK" if ok else "FAILED"))
sys.exit(0 if ok else 1)
PY
