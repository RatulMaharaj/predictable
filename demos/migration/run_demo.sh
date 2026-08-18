#!/usr/bin/env bash
# demos/migration/run_demo.sh — replay the whole Prophet migration, end to end.
#
# This script is the screencast. Every command in `README.md` is one of the commands below,
# in this order, and the assertions at the end are what makes the transcript a test rather
# than a story: iteration 1 must diverge, iteration 2 must reconcile at the `reconcile`
# profile with zero diverging cells and zero widened tolerances.
#
#   ./demos/migration/run_demo.sh          replay
#   PAUSE=1 ./demos/migration/run_demo.sh  replay with a beat between steps, for recording
#
set -euo pipefail

DEMO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DEMO/../.." && pwd)"
PAUSE="${PAUSE:-0}"

PREDICTABLE="${PREDICTABLE:-$ROOT/target/release/predictable}"
PYTHON="${PYTHON:-$ROOT/.venv/bin/python}"
[ -x "$PYTHON" ] || PYTHON="$(command -v python3)"
export PYTHONPATH="$ROOT/packages/predictable/src${PYTHONPATH:+:$PYTHONPATH}"

if [ ! -x "$PREDICTABLE" ]; then
  echo "building the CLI (one-off)..." >&2
  (cd "$ROOT" && cargo build --release -p predictable-cli >/dev/null)
fi

cd "$DEMO"

step() {
  printf '\n\033[1m── %s\033[0m\n' "$1"
  [ "$PAUSE" = "1" ] && sleep 1.2
  return 0
}
say() { printf '\033[2m$ %s\033[0m\n' "$*"; }
run() { say "$*"; "$@"; }

# ---------------------------------------------------------------------------------------
step "0.  The Prophet workspace we are migrating"
# ---------------------------------------------------------------------------------------
# Regenerated rather than committed-and-trusted, so the .rpt this demo reconciles to is
# always the output of the independent transcription in tools/make_prophet_run.py.
run "$PYTHON" tools/make_prophet_run.py
say "ls prophet/"
ls -1 prophet/

# ---------------------------------------------------------------------------------------
step "1.  Inventory, then the inputs (skill steps 1-2)"
# ---------------------------------------------------------------------------------------
run "$PYTHON" tools/import_prophet_inputs.py
say "cat migration/coverage.json"
"$PYTHON" -c "import json;d=json.load(open('migration/coverage.json'));\
print(' '.join(f\"{k}={len(v)}\" for k,v in d['variables'].items()))"

# ---------------------------------------------------------------------------------------
step "2.  Iteration 1 — build, check, run"
# ---------------------------------------------------------------------------------------
run "$PYTHON" -m predictable build steps/iteration1/model.py --out build_iter1
run "$PREDICTABLE" check build_iter1
run "$PREDICTABLE" run run_iter1.pir --retain-all

# ---------------------------------------------------------------------------------------
step "3.  Iteration 1 — diff against the Prophet run.  This MUST diverge."
# ---------------------------------------------------------------------------------------
say "predictable diff run prophet/TERM_BASE_2026Q2.rpt runs/iter1 --mapping migration/mapping.toml --tolerance-profile reconcile"
set +e
"$PREDICTABLE" diff run prophet/TERM_BASE_2026Q2.rpt runs/iter1 \
  --mapping migration/mapping.toml --tolerance-profile reconcile \
  --out-json migration/diff_iter1.json
ITER1_EXIT=$?
set -e

# ---------------------------------------------------------------------------------------
step "4.  Iteration 2 — the two roots fixed, rebuilt and re-run"
# ---------------------------------------------------------------------------------------
run "$PYTHON" -m predictable build model.py --out build
run "$PREDICTABLE" check build
run "$PREDICTABLE" run run.pir --retain-all

# ---------------------------------------------------------------------------------------
step "5.  Iteration 2 — diff.  This MUST reconcile."
# ---------------------------------------------------------------------------------------
say "predictable diff run prophet/TERM_BASE_2026Q2.rpt runs/base --mapping migration/mapping.toml --tolerance-profile reconcile"
set +e
"$PREDICTABLE" diff run prophet/TERM_BASE_2026Q2.rpt runs/base \
  --mapping migration/mapping.toml --tolerance-profile reconcile \
  --out-json migration/diff_final.json
FINAL_EXIT=$?
set -e

# ---------------------------------------------------------------------------------------
step "6.  What the numbers agree on, for one model point"
# ---------------------------------------------------------------------------------------
run "$PREDICTABLE" explain runs/base --component reserve --mp UK00001 --t 1 --depth 2

# ---------------------------------------------------------------------------------------
step "7.  Assertions"
# ---------------------------------------------------------------------------------------
"$PYTHON" - "$ITER1_EXIT" "$FINAL_EXIT" <<'PY'
import json, sys

iter1_exit, final_exit = int(sys.argv[1]), int(sys.argv[2])
one = json.load(open("migration/diff_iter1.json"))
two = json.load(open("migration/diff_final.json"))
ok = True


def check(label, cond, detail=""):
    global ok
    ok = ok and bool(cond)
    print(f"  {'PASS' if cond else 'FAIL'}  {label}{'  ' + detail if detail else ''}")


# --- iteration 1 is genuinely broken, and the differ says how -------------------------
check("iteration 1 exits 1 (diverged)", iter1_exit == 1, f"exit={iter1_exit}")
check("iteration 1 verdict is `diverged`", one["summary"]["verdict"] == "diverged")
roots = [f for f in one["findings"] if f["class"] == "root"]
check("iteration 1 has >= 2 root findings", len(roots) >= 2, f"{len(roots)} roots")
by_component = {f["component"].split(".")[-1] for f in roots}
check("`qx` is named as a root", "qx" in by_component)
check("`renewal_expenses` is named as a root", "renewal_expenses" in by_component)
qx = next(f for f in roots if f["component"].endswith(".qx"))
codes = [h["code"] for h in qx.get("hypotheses", [])]
check("a scale hypothesis (H0101) fires on `qx`", "H0101" in codes, ",".join(codes) or "none")
h = next(h for h in qx["hypotheses"] if h["code"] == "H0101")
check("it is high confidence", h["confidence"] == "high")
check("it carries a suggested edit", bool(h.get("suggested_edit")))

# --- iteration 2 reconciles, and nothing was loosened to get there --------------------
check("iteration 2 exits 0", final_exit == 0, f"exit={final_exit}")
check("iteration 2 verdict is `matched`", two["summary"]["verdict"] == "matched")
check("zero diverging cells", two["summary"]["cells"]["diverged"] == 0,
      f"{two['summary']['cells']['compared']:,} cells compared")
check("zero root divergences", two["summary"]["root_divergences"] == 0)
check("penny-equivalent (max |a-b| < 0.005)", two["summary"]["cells"]["max_abs"] < 0.005,
      f"max_abs={two['summary']['cells']['max_abs']:.3g}")
check("no tolerance was widened", two["tolerance"]["loosened"] == 0)
check("no mapping entry adjusts a number", two["mapping"]["adjusted"] == [])
check("every unmapped Prophet column has a stated reason",
      all(u.get("reason") for u in two["mapping"]["unmapped"]))
check("all 16 model points aligned", two["summary"]["modelpoints"]["common"] == 16)

print()
print("MIGRATION DEMO: " + ("OK" if ok else "FAILED"))
sys.exit(0 if ok else 1)
PY
