"""The harness: run the gate, then — only if it passes — run the timings.

The ordering is the whole point of the exercise. `run_all` refuses to time a scenario
whose engines disagree, and says so in the output, because a fast wrong answer is not a
benchmark result.
"""

from __future__ import annotations

import contextlib
import io
import os
import platform
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

from . import gate, workdir
from .engines import numpy_ref, predictable_cli
from .outputs import Outputs
from .scenarios import GATE_SIZES, SCENARIOS, gate_sizes_for, sizes_for
from .timing import Timing, measure

BENCH_ROOT = Path(__file__).resolve().parents[1]
WORK = BENCH_ROOT / ".work"

#: The reference every other engine is compared against. NumPy, not predictable: the gate
#: must be able to fail predictable, so predictable cannot be the yardstick.
REFERENCE = "numpy"


def _load(name: str):
    if name == "numpy":
        return numpy_ref
    if name == "cashflower":
        from .engines import cashflower_impl

        return cashflower_impl
    if name == "modelx":
        from .engines import modelx_impl

        return modelx_impl
    raise KeyError(name)


ENGINES = ["predictable", "numpy", "cashflower", "modelx"]


@contextlib.contextmanager
def _quiet():
    """cashflower prints a progress bar to stdout; a benchmark log is not the place."""
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        yield


# -- running one (scenario, size, engine) -------------------------------------------------


def _run_engine(engine: str, scenario: str, size: int, *, collect_series: bool) -> Outputs:
    mps = workdir.modelpoints(scenario, size)
    if engine == "predictable":
        work = workdir.prepare(scenario, size, WORK)
        predictable_cli.execute(work)
        return predictable_cli.collect(work) if collect_series else Outputs(keys=[])
    module = _load(engine)
    with _quiet():
        return module.run(scenario, mps, collect_series=collect_series)


def _supported(engine: str, scenario: str) -> bool:
    if engine in ("predictable", "numpy"):
        return True
    return scenario in _load(engine).SUPPORTED


# -- the gate -----------------------------------------------------------------------------


def run_gate(scenarios: list[str], sizes: list[int] | None = None) -> list[gate.GateResult]:
    results: list[gate.GateResult] = []
    for scenario in scenarios:
        for size in sizes or GATE_SIZES:
            reference = None
            for engine in ENGINES:
                if engine == REFERENCE or not _supported(engine, scenario):
                    continue
                if size not in gate_sizes_for(engine, sizes):
                    continue
                if reference is None:
                    reference = _run_engine(REFERENCE, scenario, size, collect_series=True)
                try:
                    actual = _run_engine(engine, scenario, size, collect_series=True)
                except Exception as exc:  # noqa: BLE001 — recorded, not swallowed
                    r = gate.GateResult(scenario, size, engine, REFERENCE)
                    r.error = f"{type(exc).__name__}: {exc}"
                    results.append(r)
                    continue
                results.append(
                    gate.compare(scenario, size, engine, actual, reference, REFERENCE)
                )
    return results


# -- the timings --------------------------------------------------------------------------


def run_timings(
    scenarios: list[str], engines: list[str], repeats: int, thread_counts: list[int]
) -> list[Timing]:
    out: list[Timing] = []
    for scenario in scenarios:
        periods = SCENARIOS[scenario].periods
        for engine in engines:
            if not _supported(engine, scenario):
                continue
            engine_sizes = sizes_for(scenario, engine)
            for size in engine_sizes:
                # The scaling curve is taken at the largest size the scenario reaches:
                # thread scaling at M = 1 measures thread startup, not the engine.
                scaling = engine == "predictable" and size == engine_sizes[-1]
                threads_list = thread_counts if scaling else [1]
                for threads in threads_list:
                    if engine == "predictable":
                        work = workdir.prepare(scenario, size, WORK)

                        def call(work=work, threads=threads):
                            predictable_cli.execute(work, threads=threads)
                    else:
                        module = _load(engine)
                        mps = workdir.modelpoints(scenario, size)

                        def call(module=module, scenario=scenario, mps=mps):
                            with _quiet():
                                module.run(scenario, mps, collect_series=False)

                    warm = repeats if size <= 1_000 else max(1, repeats // 5)
                    try:
                        cold, runs, peak = measure(call, repeats=warm)
                    except Exception as exc:  # noqa: BLE001
                        # One broken configuration must not destroy an hour of measurement.
                        # It is recorded as a row with no timing and a reason attached.
                        out.append(
                            Timing(
                                scenario=scenario,
                                size=size,
                                engine=engine,
                                threads=threads,
                                periods=periods,
                                cold_s=float("nan"),
                                runs_s=[],
                                note=f"failed: {type(exc).__name__}: {exc}".split("\n")[0],
                            )
                        )
                        continue
                    finally:
                        if engine == "predictable":
                            shutil.rmtree(work / "runs", ignore_errors=True)
                    out.append(
                        Timing(
                            scenario=scenario,
                            size=size,
                            engine=engine,
                            threads=threads,
                            periods=periods,
                            cold_s=cold,
                            runs_s=runs,
                            peak_rss_mb=peak,
                            note=(
                                None
                                if len(runs) >= 3
                                else f"{len(runs)} warm run(s): a single run takes minutes here"
                            ),
                        )
                    )
    return out


# -- environment --------------------------------------------------------------------------


def environment() -> dict:
    import numpy

    env = {
        "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "platform": platform.platform(),
        "processor": platform.processor() or platform.machine(),
        "logical_cpus": os.cpu_count(),
        "python": sys.version.split()[0],
        "numpy": numpy.__version__,
    }
    try:
        env["predictable"] = predictable_cli.version()
    except Exception:  # noqa: BLE001
        env["predictable"] = "unavailable"
    for name in ("cashflower", "modelx"):
        try:
            env[name] = _load(name).version()
        except Exception as exc:  # noqa: BLE001
            env[name] = f"unavailable ({exc})"
    try:
        env["git_commit"] = (
            subprocess.run(
                ["git", "rev-parse", "--short", "HEAD"],
                cwd=BENCH_ROOT,
                capture_output=True,
                text=True,
            ).stdout.strip()
            or None
        )
    except Exception:  # noqa: BLE001
        env["git_commit"] = None
    return env


@dataclass
class Report:
    environment: dict
    gate: list[gate.GateResult] = field(default_factory=list)
    timings: list[Timing] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)

    @property
    def gate_passed(self) -> bool:
        return all(r.passed for r in self.gate)

    def to_json(self) -> dict:
        return {
            "environment": self.environment,
            "gate_passed": self.gate_passed,
            "gate": [r.to_json() for r in self.gate],
            "timings": [t.to_json() for t in self.timings],
            "skipped": self.skipped,
        }


def run_all(
    scenarios: list[str],
    engines: list[str],
    *,
    repeats: int,
    thread_counts: list[int],
    gate_sizes: list[int] | None = None,
) -> Report:
    """The published pipeline: gate first, and only then time what the gate cleared."""
    report = Report(environment=environment())
    report.gate = run_gate(scenarios, gate_sizes)
    failed = {r.scenario for r in report.gate if not r.passed}
    cleared = [s for s in scenarios if s not in failed]
    report.skipped = [
        f"{s}: not timed — the correctness gate failed" for s in scenarios if s in failed
    ]
    report.timings = run_timings(cleared, engines, repeats, thread_counts)
    return report
