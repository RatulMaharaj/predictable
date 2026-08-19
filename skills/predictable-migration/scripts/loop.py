#!/usr/bin/env python3
"""One command for the migration loop of `04-verify.md` §8.2: check -> run -> diff -> report.

The point of this script is not convenience. It is that the loop has a *machine-readable
stop condition*, so an agent iterating on a migration cannot mistake "the diff printed
fewer lines" for "the model reconciles". Everything the skill's step 7 requires is
computed here and reported as booleans:

    verdict == "matched"          the run diff agrees at the named tolerance profile
    coverage.complete             every component carries `meta.source` (IR §11.1)
    check.clean                   no errors and no `W0102` unit lints

and the loop is done only when all three hold.

It also enforces the one hard prohibition the skill states: **a tolerance is never
widened to make a finding go away.** `--abs` / `--rel` are accepted only so they can be
refused with an explanation, and a mapping file that loosens a tolerance is reported as a
violation rather than silently honoured.

Usage
-----
    python loop.py --model good/build --run good/run.pir --baseline prophet_run/ \
                   [--out .migration] [--tolerance-profile reconcile] \
                   [--mapping migration/mapping.toml] [--cwd DIR] [--report report.json]

Exit codes
----------
    0  done: matched, covered, clean
    1  the loop is not finished: divergences remain (the report names the roots)
    2  blocked: checker errors, a failed run, or an incomparable pair
    3  refused: the invocation would have widened a tolerance
"""

from __future__ import annotations

import argparse
import json

import subprocess
import sys
import tomllib
from pathlib import Path

SCHEMA = "migration-loop/1"
PROFILES = ("exact", "regression", "reconcile", "materiality")

EXIT_DONE = 0
EXIT_UNFINISHED = 1
EXIT_BLOCKED = 2
EXIT_REFUSED = 3

PROHIBITION = (
    "widening a tolerance to silence a finding is prohibited: it converts an unexplained "
    "difference into an invisible one. Fix the model, or surface the trade-off to the user "
    "and let a human accept the difference explicitly."
)


# --------------------------------------------------------------------------- helpers


def run_cli(binary: str, args: list[str], cwd: str) -> tuple[int, dict | None, str]:
    """Invoke the CLI, asking for its `pvf/1` document. Returns (exit, doc, stderr)."""
    proc = subprocess.run(
        [binary, *args, "--json"],
        cwd=cwd,
        capture_output=True,
        text=True,
    )
    doc = None
    if proc.stdout.strip():
        try:
            doc = json.loads(proc.stdout)
        except json.JSONDecodeError:
            doc = None
    return proc.returncode, doc, proc.stderr.strip()


def pir_files(cwd: str, model_paths: list[str], run_file: str) -> list[str]:
    """Every `.pir` file the model is made of, relative to `cwd`, sorted."""
    base = Path(cwd)
    found: set[str] = set()
    for raw in [*model_paths, run_file]:
        path = base / raw
        if path.is_dir():
            for child in path.rglob("*.pir"):
                found.add(str(child.relative_to(base)))
        elif path.suffix == ".pir":
            found.add(raw)
    return sorted(found)


def source_coverage(cwd: str, files: list[str]) -> dict:
    """`meta.source` coverage over every component in the model (IR §11.1).

    This is the completion criterion of the skill's step 4: a component with no
    `meta.source` is a formula nobody can trace back to a Prophet variable, whatever the
    numbers say.
    """
    covered: list[str] = []
    missing: list[str] = []
    unreadable: list[str] = []
    for rel in files:
        path = Path(cwd) / rel
        try:
            doc = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError):
            unreadable.append(rel)
            continue
        module = doc.get("module") or path.stem
        for comp in doc.get("component", []):
            name = f"{module}.{comp.get('name', '?')}"
            source = (comp.get("meta") or {}).get("source")
            if isinstance(source, dict) and (source.get("variable") or source.get("file")):
                covered.append(name)
            else:
                missing.append(name)
    total = len(covered) + len(missing)
    return {
        "components": total,
        "with_source": len(covered),
        "missing_source": sorted(missing),
        "unreadable_files": unreadable,
        "fraction": (len(covered) / total) if total else 0.0,
        "complete": total > 0 and not missing,
    }


def check_summary(doc: dict | None, exit_code: int) -> dict:
    diagnostics = (doc or {}).get("diagnostics", []) or []
    summary = (doc or {}).get("summary", {}) or {}
    unit_lints = [d for d in diagnostics if d.get("code") == "W0102"]
    errors = int(summary.get("errors", 0))
    return {
        "exit_code": exit_code,
        "errors": errors,
        "warnings": int(summary.get("warnings", 0)),
        "unit_lints": [
            {"code": d.get("code"), "message": d.get("message"), "doc_url": d.get("doc_url")}
            for d in unit_lints
        ],
        # Step 7's wording: clean of errors *and* of W0102 unit lints. A unit lint is a
        # units-and-timing decision that was never made, which is exactly what the
        # migration is supposed to have decided.
        "clean": errors == 0 and not unit_lints,
        "codes": sorted({d.get("code") for d in diagnostics if d.get("code")}),
    }


def tolerance_integrity(diff: dict | None) -> dict:
    """Did anything in this comparison loosen a tolerance? (The hard prohibition.)"""
    tolerance = (diff or {}).get("tolerance") or {}
    overrides = tolerance.get("overrides_applied") or []
    looser = [o for o in overrides if o.get("looser")]
    loosened = int(tolerance.get("loosened", 0) or 0)
    return {
        "profile": tolerance.get("profile"),
        "abs": tolerance.get("abs"),
        "rel": tolerance.get("rel"),
        "loosened": loosened,
        "looser_overrides": [o.get("component") for o in looser],
        "ok": loosened == 0 and not looser,
        "rule": PROHIBITION,
    }


def summarise_findings(diff: dict | None) -> tuple[list[dict], int]:
    findings = (diff or {}).get("findings", []) or []
    roots = []
    for f in findings:
        if f.get("class") != "root":
            continue
        roots.append(
            {
                "id": f.get("id"),
                "component": f.get("component"),
                "class_basis": f.get("class_basis"),
                "category": f.get("category"),
                "affects_outputs": f.get("affects_outputs", []),
                "explained_by_model_change": f.get("explained_by_model_change"),
                "exemplar": f.get("exemplar"),
                "explain_command": f.get("explain_command"),
                "hypotheses": [
                    {
                        "code": h.get("code"),
                        "confidence": h.get("confidence"),
                        "message": h.get("message"),
                        "evidence_support": h.get("evidence_support"),
                        "suggested_edit": h.get("suggested_edit"),
                    }
                    for h in (f.get("hypotheses") or [])
                ],
            }
        )
    inherited = sum(1 for f in findings if f.get("class") != "root")
    return roots, inherited


def next_actions(roots: list[dict], coverage: dict, check: dict) -> list[str]:
    actions: list[str] = []
    for root in roots:
        high = [h for h in root["hypotheses"] if h.get("confidence") == "high" and h.get("suggested_edit")]
        if high:
            edit = high[0]["suggested_edit"]
            actions.append(
                f"{root['id']} {root['component']}: apply the {high[0]['code']} suggested edit at "
                f"{edit.get('file')}:{edit.get('byte_start')}..{edit.get('byte_end')}, then re-run the loop"
            )
        elif root.get("explain_command"):
            actions.append(
                f"{root['id']} {root['component']}: confidence is not high — "
                f"run `{root['explain_command']}` on both sides before editing"
            )
    if not actions and not coverage["complete"]:
        missing = coverage["missing_source"]
        actions.append(
            f"numbers agree, but {len(missing)} component(s) still have no `meta.source`: "
            + ", ".join(missing[:5])
            + (" ..." if len(missing) > 5 else "")
        )
    if not check["clean"]:
        actions.append("resolve the checker errors and `W0102` unit lints before trusting the diff")
    return actions


# --------------------------------------------------------------------------- the loop


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run one iteration of the predictable migration loop.")
    parser.add_argument("--model", action="append", required=True, help="model directory or .pir file (repeatable)")
    parser.add_argument("--run", required=True, help="the run .pir to project")
    parser.add_argument("--baseline", required=True, help="the run directory or .rpt to reconcile against")
    parser.add_argument("--baseline-model", default=None, help="model directory behind the baseline, for attribution")
    parser.add_argument("--out", default=".migration/candidate", help="run directory to write")
    parser.add_argument("--tolerance-profile", default="reconcile", choices=PROFILES)
    parser.add_argument("--mapping", default=None, help="migration/mapping.toml")
    parser.add_argument("--bin", default="predictable", help="the predictable binary")
    parser.add_argument("--cwd", default=".", help="working directory every path is relative to")
    parser.add_argument("--report", default=None, help="also write the report JSON here")
    parser.add_argument("--abs", dest="abs_", default=None, help=argparse.SUPPRESS)
    parser.add_argument("--rel", default=None, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)

    def emit(report: dict, code: int) -> int:
        text = json.dumps(report, indent=2, sort_keys=True)
        if args.report:
            Path(args.cwd, args.report).write_text(text + "\n", encoding="utf-8")
        print(text)
        return code

    # Step 0 — the prohibition, before anything is executed.
    if args.abs_ is not None or args.rel is not None:
        return emit(
            {
                "schema": SCHEMA,
                "verdict": "refused",
                "reason": "tolerance_override_requested",
                "message": PROHIBITION,
                "remedy": (
                    "re-run with a named --tolerance-profile and fix the root findings, or ask the "
                    "user to accept the difference in writing"
                ),
                "done": False,
            },
            EXIT_REFUSED,
        )

    files = pir_files(args.cwd, args.model, args.run)
    if not files:
        return emit(
            {"schema": SCHEMA, "verdict": "blocked", "reason": "no .pir files found", "done": False},
            EXIT_BLOCKED,
        )

    steps: list[dict] = []

    # Step 1 — check. Schema and formula errors first; a diff on an unchecked model is noise.
    code, doc, err = run_cli(args.bin, ["check", *files], args.cwd)
    check = check_summary(doc, code)
    steps.append({"step": "check", "command": f"{args.bin} check {' '.join(files)} --json", **check, "stderr": err})
    coverage = source_coverage(args.cwd, files)
    if check["errors"]:
        return emit(
            {
                "schema": SCHEMA,
                "verdict": "blocked",
                "reason": "checker errors",
                "steps": steps,
                "coverage": coverage,
                "diagnostics": (doc or {}).get("diagnostics", []),
                "next_actions": ["fix the checker errors above; every `doc_url` resolves to the catalogue"],
                "done": False,
            },
            EXIT_BLOCKED,
        )

    # Step 2 — run. Always `--retain-all`: "which component is the root" is partly a fact
    # about what you asked the engine to write down (04-verify.md §5, walkthrough §3).
    code, doc, err = run_cli(args.bin, ["run", args.run, "--out", args.out, "--retain-all"], args.cwd)
    steps.append(
        {
            "step": "run",
            "command": f"{args.bin} run {args.run} --out {args.out} --retain-all --json",
            "exit_code": code,
            "outcome": (doc or {}).get("outcome"),
            "run_id": (doc or {}).get("run_id"),
            "manifest_digest": (doc or {}).get("manifest_digest"),
            "traps": (doc or {}).get("traps", []),
            "stderr": err,
        }
    )
    if code != 0:
        return emit(
            {
                "schema": SCHEMA,
                "verdict": "blocked",
                "reason": "the run did not complete",
                "steps": steps,
                "coverage": coverage,
                "next_actions": ["resolve the run failure or traps above"],
                "done": False,
            },
            EXIT_BLOCKED,
        )

    # Step 3 — diff against the baseline.
    diff_args = ["diff", "run", args.baseline, args.out, "--tolerance-profile", args.tolerance_profile]
    model_dirs = [m for m in args.model if (Path(args.cwd) / m).is_dir()]
    if args.baseline_model:
        diff_args += ["--model-a", args.baseline_model]
    if model_dirs:
        diff_args += ["--model-b", model_dirs[0]]
    if args.mapping:
        diff_args += ["--mapping", args.mapping]
    code, diff, err = run_cli(args.bin, diff_args, args.cwd)
    summary = (diff or {}).get("summary", {}) or {}
    steps.append(
        {
            "step": "diff",
            "command": f"{args.bin} {' '.join(diff_args)} --json",
            "exit_code": code,
            "verdict": summary.get("verdict"),
            "cells": summary.get("cells"),
            "root_divergences": summary.get("root_divergences"),
            "graph_available": summary.get("graph_available"),
            "stderr": err,
        }
    )
    if diff is None or code == 2:
        return emit(
            {
                "schema": SCHEMA,
                "verdict": "blocked",
                "reason": "the two runs are incomparable",
                "steps": steps,
                "coverage": coverage,
                "incomparable": summary.get("incomparable", []),
                "next_actions": ["reconcile the two sides' shape (components, modelpoints, emit) before diffing"],
                "done": False,
            },
            EXIT_BLOCKED,
        )

    tolerance = tolerance_integrity(diff)
    roots, inherited = summarise_findings(diff)
    verdict = summary.get("verdict", "unknown")

    report = {
        "schema": SCHEMA,
        "verdict": verdict,
        "steps": steps,
        "coverage": coverage,
        "check": check,
        "tolerance": tolerance,
        "roots": roots,
        "inherited_findings": inherited,
        "stop_condition": {
            "matched": verdict == "matched",
            "coverage_complete": coverage["complete"],
            "check_clean": check["clean"],
            "tolerance_not_widened": tolerance["ok"],
        },
        "prohibition": PROHIBITION,
    }
    report["done"] = all(report["stop_condition"].values())
    report["next_actions"] = next_actions(roots, coverage, check)

    if not tolerance["ok"]:
        report["verdict"] = "refused"
        report["reason"] = "the comparison loosened a tolerance"
        report["done"] = False
        return emit(report, EXIT_REFUSED)
    if report["done"]:
        return emit(report, EXIT_DONE)
    return emit(report, EXIT_UNFINISHED)


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
