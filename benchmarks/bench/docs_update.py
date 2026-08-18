"""Fill the generated sections of `docs/v2/benchmarks.md` from a finished report.

The prose on that page is written by hand; the three tables that quote numbers are not,
because a hand-copied number goes stale silently. Each generated block sits between an
HTML comment marker and its `<!-- /… -->` closer, and this module rewrites what is between
them and nothing else.
"""

from __future__ import annotations

import re
from pathlib import Path

from .harness import BENCH_ROOT, Report
from .report import _fmt_rate, _fmt_size, _fmt_time

DOCS_PAGE = BENCH_ROOT.parent / "docs" / "v2" / "benchmarks.md"

#: `03-engine.md` §11.5, verbatim. `size` is the size the target was stated at; where this
#: machine cannot reach it, the row says so rather than silently comparing a smaller run.
TARGETS = [
    ("term_annual", 1_000_000, 1, 300e6, "≥ 300 M mp-periods/s"),
    ("term_annual", 1_000_000, 8, 2e9, "≥ 2 G mp-periods/s"),
    ("ifrs17_gmm", 1_000_000, 1, 20e6, "≥ 20 M mp-periods/s"),
    ("ifrs17_gmm", 1_000_000, 8, 140e6, "≥ 140 M mp-periods/s"),
]


def _block(name: str, body: str) -> str:
    return f"<!-- {name} -->\n\n{body}\n\n<!-- /{name} -->"


def _replace(text: str, name: str, body: str) -> str:
    pattern = re.compile(
        rf"<!-- {name} -->(?:.*?<!-- /{name} -->)?", re.DOTALL
    )
    if not pattern.search(text):
        raise KeyError(f"marker {name} not found in {DOCS_PAGE}")
    return pattern.sub(lambda _: _block(name, body), text, count=1)


def _gate_summary(report: Report) -> str:
    total = len(report.gate)
    failed = [r for r in report.gate if not r.passed]
    cells = sum(sum(c.cells for c in r.components) for r in report.gate)
    worst = max(
        (c for r in report.gate for c in r.components), key=lambda c: c.error, default=None
    )
    lines = [
        f"- **{total} comparisons**, {cells:,} cells, tolerance `1e-9`.",
        f"- **{len(failed)} failed.**",
    ]
    if worst:
        lines.append(
            f"- Largest disagreement anywhere: `{worst.component}` at "
            f"`{worst.error:.2e}` — {'within' if worst.error <= 1e-9 else 'outside'} tolerance."
        )
    if failed:
        lines.append("")
        lines.append("| scenario | M | engine | detail |")
        lines.append("|---|---:|---|---|")
        for r in failed:
            detail = r.error or ", ".join(c.component for c in r.components if not c.passed)
            lines.append(f"| `{r.scenario}` | {_fmt_size(r.size)} | {r.engine} | {detail} |")
    else:
        lines.append("")
        lines.append(
            "Every engine reproduced every output of every scenario it implements, at every "
            "gate size, to better than `1e-9`. The timings below were recorded after that."
        )
    return "\n".join(lines)


def _throughput(report: Report) -> str:
    rows = [t for t in report.timings if t.threads == 1 and t.ok]
    by_scenario: dict[str, list] = {}
    for t in rows:
        by_scenario.setdefault(t.scenario, []).append(t)
    lines = [
        "Single-threaded, one row per size that predictable ran. A dash means the engine "
        "was not run at that size — see the caps in §3.",
        "",
        "| scenario | M | predictable | NumPy | cashflower | modelx |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for scenario, timings in sorted(by_scenario.items()):
        sizes = sorted({t.size for t in timings})
        for size in sizes:
            at = {t.engine: t for t in timings if t.size == size}
            if "predictable" not in at:
                continue
            cells = [
                _fmt_rate(at[e].mp_periods_per_s) + "/s" if e in at else "—"
                for e in ("predictable", "numpy", "cashflower", "modelx")
            ]
            lines.append(
                f"| `{scenario}` | {_fmt_size(size)} | " + " | ".join(cells) + " |"
            )
    lines += [
        "",
        "Wall time, cold and warm, min–max spread and peak RSS for every row are in "
        "`benchmarks/results/results.md`.",
    ]
    return "\n".join(lines)


def _targets(report: Report) -> str:
    # The §11.5 targets are predictable's, so only predictable's rows are eligible.
    mine = [t for t in report.timings if t.engine == "predictable" and t.ok]
    index = {(t.scenario, t.size, t.threads): t for t in mine}
    lines = [
        "| scenario | M | threads | target | achieved | verdict |",
        "|---|---:|---:|---|---:|---|",
    ]
    for scenario, size, threads, target, label in TARGETS:
        t = index.get((scenario, size, threads))
        if t is None:
            largest = max(
                (x for x in mine if x.scenario == scenario and x.threads == threads),
                key=lambda x: x.size,
                default=None,
            )
            got = (
                f"not run at this size (largest run: {_fmt_size(largest.size)} at "
                f"{_fmt_rate(largest.mp_periods_per_s)}/s)"
                if largest
                else "not run"
            )
            lines.append(
                f"| `{scenario}` | {_fmt_size(size)} | {threads} | {label} | — | {got} |"
            )
            continue
        rate = t.mp_periods_per_s
        verdict = "**met**" if rate >= target else f"**missed** ({rate / target:.0%} of target)"
        lines.append(
            f"| `{scenario}` | {_fmt_size(size)} | {threads} | {label} | "
            f"{_fmt_rate(rate)}/s | {verdict} |"
        )

    one_mp = [t for t in mine if t.size == 1]
    if one_mp:
        worst = max(one_mp, key=lambda t: t.median_s)
        target_met = worst.median_s < 0.050
        lines.append(
            f"| any | 1 | 1 | < 50 ms end to end | {_fmt_time(worst.median_s)} | "
            f"{'**met**' if target_met else '**missed**'} (worst: `{worst.scenario}`) |"
        )
    lines += [
        "",
        "The peak-RSS target of §11.5 (under 2 GB at `M = 1e7` with 8 threads) is **not "
        "tested here**: `M = 1e7` is not run on this machine. It stays open.",
    ]
    return "\n".join(lines)


def update(report: Report, page: Path | None = None) -> Path:
    page = page or DOCS_PAGE
    text = page.read_text()
    text = _replace(text, "GATE-SUMMARY", _gate_summary(report))
    text = _replace(text, "THROUGHPUT", _throughput(report))
    text = _replace(text, "TARGETS", _targets(report))
    page.write_text(text)
    return page
