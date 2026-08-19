"""`python -m bench` — run the gate, then the timings, then write the published files."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from . import report as report_mod
from .harness import BENCH_ROOT, ENGINES, run_all, run_gate
from .scenarios import SCENARIOS

RESULTS = BENCH_ROOT / "results"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="bench", description=__doc__)
    parser.add_argument("--scenario", action="append", choices=sorted(SCENARIOS), default=None)
    parser.add_argument("--engine", action="append", choices=ENGINES, default=None)
    parser.add_argument("--repeats", type=int, default=5, help="warm runs per measurement")
    parser.add_argument(
        "--threads", default="1", help="comma-separated thread counts for predictable"
    )
    parser.add_argument("--gate-only", action="store_true", help="run the gate and stop")
    parser.add_argument("--gate-size", action="append", type=int, default=None)
    parser.add_argument("--out", type=Path, default=RESULTS)
    parser.add_argument(
        "--no-docs",
        action="store_true",
        help="do not refresh the generated tables in docs/v2/benchmarks.md",
    )
    args = parser.parse_args(argv)

    scenarios = args.scenario or list(SCENARIOS)
    engines = args.engine or list(ENGINES)
    threads = [int(x) for x in args.threads.split(",")]

    if args.gate_only:
        results = run_gate(scenarios, args.gate_size)
        for r in results:
            worst = r.worst
            status = "pass" if r.passed else "FAIL"
            detail = r.error or (f"{worst.component} {worst.error:.2e}" if worst else "—")
            print(f"{status:5} {r.scenario:16} M={r.size:<8} {r.engine:12} {detail}")
        return 0 if all(r.passed for r in results) else 1

    report = run_all(
        scenarios,
        engines,
        repeats=args.repeats,
        thread_counts=threads,
        gate_sizes=args.gate_size,
    )
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "results.json").write_text(json.dumps(report.to_json(), indent=2) + "\n")
    (args.out / "results.md").write_text(report_mod.render(report))
    if not args.no_docs:
        from . import docs_update

        print(f"updated {docs_update.update(report)}")
    print(f"gate: {'passed' if report.gate_passed else 'FAILED'}")
    print(f"wrote {args.out / 'results.json'} and {args.out / 'results.md'}")
    return 0 if report.gate_passed else 1


if __name__ == "__main__":
    sys.exit(main())
