"""Wall time and peak RSS, measured the way `03-engine.md` §11.4 asks for.

- **median of `repeats`**, with min and max reported. Not the mean: one scheduler hiccup
  moves a mean and does not move a median, and a benchmark that publishes only a mean is
  publishing the hiccup.
- **cold and warm**. The first measured run of a (scenario, size, engine) is reported
  separately as the cold one; the median is taken over the runs after it.
- **peak RSS**, sampled at 10 ms over the whole process tree: the harness plus any child.
  For predictable, most of that is the CLI subprocess, and the harness's own share is a
  fixed ~100 MB of interpreter and imports. For the in-process engines it is the harness
  *including* the engine, so the same ~100 MB floor is in the number. Read the RSS column
  as an upper bound with a common, constant offset — not as an engine's own footprint.
- **modelpoint-periods per second**: `M × (T + 1) / s`, the size-independent number.
"""

from __future__ import annotations

import statistics
import threading
import time
from dataclasses import dataclass, field

import psutil


@dataclass
class Timing:
    scenario: str
    size: int
    engine: str
    threads: int
    periods: int
    cold_s: float
    runs_s: list[float] = field(default_factory=list)
    peak_rss_mb: float = 0.0
    note: str | None = None

    @property
    def ok(self) -> bool:
        """False for a configuration that raised. Such a row keeps its place in the
        report — with its reason in `note` — rather than vanishing from the table."""
        return bool(self.runs_s)

    @property
    def median_s(self) -> float:
        return statistics.median(self.runs_s) if self.runs_s else self.cold_s

    @property
    def mp_periods_per_s(self) -> float:
        return self.size * (self.periods + 1) / self.median_s

    def to_json(self) -> dict:
        """A failed row serialises its timings as `null`, never as `NaN`.

        `NaN` is not valid JSON, and a consumer that silently accepts it will happily
        plot it. `null` forces the question.
        """
        head = {
            "scenario": self.scenario,
            "size": self.size,
            "engine": self.engine,
            "threads": self.threads,
            "periods": self.periods,
            "ok": self.ok,
            "note": self.note,
        }
        if not self.ok:
            return {
                **head,
                "cold_s": None,
                "median_s": None,
                "min_s": None,
                "max_s": None,
                "repeats": 0,
                "mp_periods_per_s": None,
                "peak_rss_mb": None,
            }
        return {
            **head,
            "cold_s": self.cold_s,
            "median_s": self.median_s,
            "min_s": min(self.runs_s),
            "max_s": max(self.runs_s),
            "repeats": len(self.runs_s),
            "mp_periods_per_s": self.mp_periods_per_s,
            "peak_rss_mb": self.peak_rss_mb,
        }


class RssSampler:
    """Sample a process tree's RSS every 10 ms for the duration of a `with` block."""

    def __init__(self, interval: float = 0.01):
        self.interval = interval
        self.peak = 0.0
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def _sample(self):
        me = psutil.Process()
        while not self._stop.is_set():
            total = me.memory_info().rss
            for child in me.children(recursive=True):
                try:
                    total += child.memory_info().rss
                except psutil.Error:
                    pass
            self.peak = max(self.peak, total / (1024 * 1024))
            self._stop.wait(self.interval)

    def __enter__(self):
        self._thread = threading.Thread(target=self._sample, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, *exc):
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=1.0)
        return False


def measure(fn, *, repeats: int) -> tuple[float, list[float], float]:
    """Run `fn` once cold, then `repeats` times warm. Returns (cold, warm times, peak MB)."""
    sampler = RssSampler()
    with sampler:
        t0 = time.perf_counter()
        fn()
        cold = time.perf_counter() - t0
        runs = []
        for _ in range(repeats):
            t0 = time.perf_counter()
            fn()
            runs.append(time.perf_counter() - t0)
    return cold, runs, sampler.peak
