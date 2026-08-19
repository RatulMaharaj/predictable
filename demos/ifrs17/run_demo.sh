#!/usr/bin/env bash
# demos/ifrs17/run_demo.sh — the IFRS 17 GMM sample valuation, end to end.
#
#   generate a 10,000-contract portfolio
#   -> run the committed ifrs17_gmm reference model over it
#   -> build the CSM and RA roll-forwards and check five identities
#   -> export a single-file governance pack
#   -> explain one contract's CSM
#
# Assertions at the end; exits non-zero if any reconciliation fails.
#
#   ./demos/ifrs17/run_demo.sh
#   N=1000 ./demos/ifrs17/run_demo.sh     a smaller portfolio, for a quick pass
#
set -euo pipefail

DEMO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DEMO/../.." && pwd)"
N="${N:-10000}"

PREDICTABLE="${PREDICTABLE:-$ROOT/target/release/predictable}"
PYTHON="${PYTHON:-$ROOT/.venv/bin/python}"
[ -x "$PYTHON" ] || PYTHON="$(command -v python3)"

if [ ! -x "$PREDICTABLE" ]; then
  echo "building the CLI (one-off)..." >&2
  (cd "$ROOT" && cargo build --release -p predictable-cli >/dev/null)
fi

cd "$DEMO"
mkdir -p out

step() { printf '\n\033[1m── %s\033[0m\n' "$1"; }
say() { printf '\033[2m$ %s\033[0m\n' "$*"; }
run() { say "$*"; "$@"; }

step "1.  The portfolio"
run "$PYTHON" tools/make_portfolio.py --n "$N"

step "2.  The valuation — the committed reference model, unmodified"
run "$PREDICTABLE" check ../../models/ifrs17_gmm/build
run "$PREDICTABLE" run run.pir --retain-all

step "3.  Disclosure tables and reconciliations"
run "$PYTHON" tools/csm_rollforward.py

step "4.  The governance pack"
run "$PREDICTABLE" export runs/valuation --out out/governance-pack.html \
  --modelpoints sample:50 --with-traces csm_release

step "5.  One contract's CSM, traced"
run "$PREDICTABLE" explain runs/valuation --component csm --mp UL000001 --t 12 --depth 1 --width 96

step "6.  Assertions"
"$PYTHON" - <<'PY'
import json, os, sys
from pathlib import Path

ok = True


def check(label, cond, detail=""):
    global ok
    ok = ok and bool(cond)
    print(f"  {'PASS' if cond else 'FAIL'}  {label}{'  ' + detail if detail else ''}")


manifest = json.loads(Path("runs/valuation/manifest.json").read_text())
recs = json.loads(Path("out/reconciliations.json").read_text())
n = int(os.environ.get("N", "10000"))

ex = manifest["execution"]
check("the run completed", ex["outcome"] == "completed", ex["outcome"])
check(f"all {n:,} contracts projected", ex["modelpoints_projected"] == n,
      f"projected={ex['modelpoints_projected']:,}")
check("no traps", ex["modelpoints_trapped"] == 0)
check("47 components written", manifest["results"]["components"] == 47,
      f"{manifest['results']['rows']:,} rows")

for c in recs["checks"]:
    check(c["name"], c["ok"], f"residual={c['residual']:.6g}")

# The IFRS 17.47 dichotomy, contract by contract, is what the two aggregate identities
# above rest on: a contract may have a CSM or a loss component, never both.
t = recs["totals"]
check("portfolio CSM at issue is positive", t["model.csm_initial"] > 0,
      f"{t['model.csm_initial']:,.0f}")
check("some contracts are onerous", t["model.is_onerous"] > 0,
      f"{t['model.is_onerous']:,.0f} of {n:,}")
check("LRC at issue equals the loss component",
      abs(t["model.lrc_at_issue"] - t["model.loss_component_initial"]) < 1e-6)

for name in ("csm_rollforward.csv", "ra_rollforward.csv", "initial_recognition.csv",
             "summary.md", "governance-pack.html"):
    p = Path("out") / name
    check(f"{name} written", p.exists() and p.stat().st_size > 0,
          f"{p.stat().st_size:,} bytes" if p.exists() else "missing")

pack = Path("out/governance-pack.html").read_text(errors="ignore")
check("the pack is self-contained (no external http(s) references)",
      "http://" not in pack and "https://" not in pack.replace("https://www.w3.org", ""))
check("the pack carries this run's manifest digest",
      manifest["manifest_digest"][:24] in pack)

print()
print("IFRS 17 DEMO: " + ("OK" if ok else "FAILED"))
sys.exit(0 if ok else 1)
PY
