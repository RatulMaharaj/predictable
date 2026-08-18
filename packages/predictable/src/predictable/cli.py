"""``predictable build [--check]`` — the command of `02-dsl.md` §8.

Invoked as ``predictable-build`` (console script) or ``python -m predictable build``. It imports
the model's Python modules, traces them, emits canonical `.pir`, runs the engine's checker in
process and either writes ``build/`` or diffs against it.

Exit codes follow `04-verify.md` §7: ``0`` clean, ``1`` diagnostics or drift, ``2`` misuse.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from .builder import (
    SPAN_MAP_FILE,
    BuildResult,
    build_product,
    check_build,
    default_out_dir,
    import_models,
    write_build,
)
from .declarations import registry_scope
from .diagnostics import DslError, render

__all__ = ["build_command", "main"]

OK, FAILED, USAGE = 0, 1, 2


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="predictable build",
        description="Compile a Python model to canonical .pir and check it with the engine.",
    )
    parser.add_argument("paths", nargs="+", help="model .py files, or a directory of them")
    parser.add_argument("--out", default=None, help="build directory (default: <model dir>/build)")
    parser.add_argument("--product", default=None, help="product to build, if the model has several")
    parser.add_argument(
        "--check",
        action="store_true",
        help="do not write; fail if the committed build directory is out of date",
    )
    parser.add_argument("--json", action="store_true", help="machine-readable output on stdout")
    parser.add_argument(
        "--no-engine-check",
        action="store_true",
        help="emit only, skipping the engine's own checker (step 5 of 02-dsl.md §8)",
    )
    parser.add_argument(
        "--root",
        default=None,
        help="directory table `source` paths resolve against (default: the model directory)",
    )
    return parser


def build_command(argv: Sequence[str] | None = None, *, stdout=None, stderr=None) -> int:
    """Run one ``build``. Returns the exit code; never raises for a user error."""
    out_stream = stdout if stdout is not None else sys.stdout
    err_stream = stderr if stderr is not None else sys.stderr
    args = _parser().parse_args(list(argv) if argv is not None else None)

    paths = [Path(p) for p in args.paths]
    missing = [p for p in paths if not p.exists()]
    if missing:
        print(f"error: no such path: {missing[0]}", file=err_stream)
        return USAGE

    out_dir = Path(args.out) if args.out else default_out_dir(paths)
    root = Path(args.root) if args.root else (paths[0] if paths[0].is_dir() else paths[0].parent)

    try:
        # A command is a fresh world: only the modules named on this command line may declare
        # into it. Without the scope a long-lived process (a notebook, a test session) would
        # build whatever it had imported earlier as well.
        with registry_scope() as reg:
            import_models(paths)
            result = build_product(
                args.product, root=root, check=not args.no_engine_check, reg=reg
            )
    except DslError as exc:
        if args.json:
            print(json.dumps({"status": "error", "diagnostics": [exc.diagnostic.to_json()]}, indent=2), file=out_stream)
        else:
            print(render(exc.diagnostic), file=err_stream)
        return FAILED

    drift = check_build(result, out_dir) if args.check else []
    written: list[Path] = []
    if result.ok and not args.check:
        written = write_build(result, out_dir)

    if args.json:
        print(json.dumps(_json_report(result, out_dir, drift, written), indent=2), file=out_stream)
    else:
        _render_report(result, out_dir, drift, written, out_stream, err_stream)

    if not result.ok:
        return FAILED
    if args.check and drift:
        return FAILED
    return OK


def _json_report(result: BuildResult, out_dir: Path, drift, written) -> dict:
    return {
        "status": "ok" if result.ok and not drift else "failed",
        "out": str(out_dir),
        "files": sorted(result.artefacts()),
        "written": [str(p) for p in written],
        "checked": result.checked,
        "meta_in_pir": result.meta_in_pir,
        "table_digests": result.table_digests,
        "drift": [{"file": d.file, "kind": d.kind, "detail": d.describe()} for d in drift],
        "diagnostics": [d.to_json() for d in result.diagnostics],
        "pir_diagnostics": list(result.raw_diagnostics),
    }


def _render_report(result: BuildResult, out_dir: Path, drift, written, out_stream, err_stream) -> None:
    for diagnostic in result.diagnostics:
        print(render(diagnostic), file=err_stream)
        print(file=err_stream)

    if not result.checked:
        print(
            "note: the engine checker did not run (predictable_engine is not installed); "
            "only the DSL's own checks were applied.",
            file=err_stream,
        )
    if result.checked and not result.meta_in_pir:
        print(
            "note: the installed engine rejects `[component.meta]`; provenance was written to "
            f"{SPAN_MAP_FILE} only.",
            file=err_stream,
        )

    if drift:
        print(f"error: {out_dir} is out of date ({len(drift)} file(s)):", file=err_stream)
        for d in drift:
            print(f"  {d.describe()}", file=err_stream)
        print("  run `predictable build` to update it.", file=err_stream)
        return

    if not result.ok:
        errors = len(result.errors)
        print(f"error: build refused: {errors} error(s); nothing was written", file=err_stream)
        return

    if written:
        names = ", ".join(sorted(p.name for p in written))
        print(f"built {len(written)} file(s) into {out_dir}: {names}", file=out_stream)
    else:
        print(f"{out_dir} is up to date", file=out_stream)


def main(argv: Sequence[str] | None = None) -> int:
    """``python -m predictable <command>``. Only ``build`` exists today."""
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in {"-h", "--help"}:
        print(__doc__)
        return OK if argv else USAGE
    command, rest = argv[0], argv[1:]
    if command != "build":
        print(f"error: unknown command `{command}`; expected `build`", file=sys.stderr)
        return USAGE
    return build_command(rest)


def build_entrypoint() -> int:
    """Console-script entry: ``predictable-build <paths>``."""
    return build_command(sys.argv[1:])


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
