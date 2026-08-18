#!/usr/bin/env python3
"""Self-check for the conformance corpus.

This does NOT parse .pir files — no parser exists yet, and the corpus deliberately
predates one. It checks that the corpus obeys its own contract (README.md), so that
a case cannot silently rot into something no runner can consume:

  1. every case directory has a case.toml with the required keys
  2. every case is registered in index.toml, and nothing is registered twice
  3. every file named by `entry`, `product`, `run` exists
  4. every expected.diag line parses, and its file/line/column exist in the case
  5. the column points at a plausible span start (inside the quotes of a value,
     or the first character of the offending list element)
  6. diagnostic codes are either normative codes from 01-ir.md or family wildcards
  7. kind = "invalid" cases expect at least one error, kind = "valid" cases none
  8. every .pir and .expected file obeys the canonical text rules that do not need
     a parser: UTF-8, LF, one trailing newline, no trailing whitespace
  9. index.toml's `codes` for a case equals the codes its expectation files declare

Run: python3 conformance/check_corpus.py
Exit 0 on success; on failure it prints one line per problem and exits 1.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent

NORMATIVE = {
    "E0101", "E0105", "E0106", "E0107", "E0108",
    "E0201", "E0202", "E0301", "E0305",
    "E0402", "E0403", "E0404", "E0602", "E0801",
    "E0902", "E0903", "E1203", "E1402",
    "W0101", "W0102", "W0103", "W0104", "W0105",
    # Assigned by the diagnostics registry when the checker (T06) landed; the
    # behaviour each one names was already fixed by 01-ir.md, and each replaced
    # a family wildcard in the same commit (README §3.1).
    "E0030",                                    # illegal index form (grammar)
    "E0203", "E0204", "E0205",                  # resolve, shadowing, horizon
    "E0501", "E0502", "E0503",                  # unit arithmetic
    "E0504", "E0505",                           # lookup key arity and dtype
    "E0506", "E0507", "E0508", "E0509",         # timing tags, zeroless lag
}
FAMILIES = {"E00xx", "E01xx", "E02xx", "E03xx", "E04xx", "E05xx",
            "E06xx", "E08xx", "E09xx", "E12xx", "E14xx"}

DIAG_RE = re.compile(
    r"^(?P<sev>error|warning)\s+(?P<code>[EW]\d{4}|[EW]\d\dxx)\s+"
    r"(?P<file>[\w./-]+?)(?::(?P<line>\d+)(?::(?P<col>\d+))?)?\s+(?P<msg>\S.*)$"
)

problems: list[str] = []


def fail(where: Path | str, msg: str) -> None:
    problems.append(f"{where}: {msg}")


def toml_str(text: str, key: str) -> str | None:
    m = re.search(rf'^{key}\s*=\s*"([^"]*)"', text, re.M)
    return m.group(1) if m else None


def toml_list(text: str, key: str) -> list[str] | None:
    m = re.search(rf"^{key}\s*=\s*\[(.*?)\]", text, re.M | re.S)
    if not m:
        return None
    return re.findall(r'"([^"]*)"', m.group(1))


def toml_bool(text: str, key: str) -> bool | None:
    m = re.search(rf"^{key}\s*=\s*(true|false)", text, re.M)
    return m.group(1) == "true" if m else None


def check_text_rules(path: Path) -> None:
    raw = path.read_bytes()
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        fail(path, "not valid UTF-8")
        return
    if b"\r" in raw:
        fail(path, "contains CR; line endings must be LF")
    if not text.endswith("\n"):
        fail(path, "missing the single trailing newline")
    if text.endswith("\n\n"):
        fail(path, "more than one trailing newline")
    for i, line in enumerate(text.split("\n")[:-1], start=1):
        if line != line.rstrip():
            fail(path, f"trailing whitespace on line {i}")


def read_expected_diag(case_dir: Path) -> list[tuple[str, str, str, int | None, int | None]]:
    """-> [(severity, code, file, line, col)]"""
    path = case_dir / "expected.diag"
    out = []
    if not path.exists():
        return out
    prev_key = None
    for n, raw in enumerate(path.read_text().splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        m = DIAG_RE.match(line)
        if not m:
            fail(path, f"line {n} does not match the expected.diag grammar: {line!r}")
            continue
        f = m.group("file")
        ln = int(m.group("line")) if m.group("line") else None
        col = int(m.group("col")) if m.group("col") else None
        code = m.group("code")

        if code not in NORMATIVE and code not in FAMILIES:
            fail(path, f"line {n}: unknown code {code}")
        if code in FAMILIES and code.replace("xx", "") in {c[:3] for c in NORMATIVE}:
            pass  # families overlap normative prefixes by design

        target = case_dir / f
        if not target.exists():
            fail(path, f"line {n}: span file {f} does not exist in the case")
        elif ln is not None:
            src = target.read_text().splitlines()
            if ln > len(src):
                fail(path, f"line {n}: span line {ln} is past the end of {f}")
            elif col is not None:
                text = src[ln - 1]
                if col > len(text):
                    fail(path, f"line {n}: span column {col} is past the end of {f}:{ln}")
                elif text[col - 1] in '"= ':
                    fail(path, f"line {n}: span column {col} points at {text[col-1]!r}, "
                               f"not at the first character of a value ({f}:{ln})")
        key = (f, ln or 0, col or 0, code)
        if prev_key is not None and key < prev_key:
            fail(path, f"line {n}: diagnostics must be sorted by (file, line, col, code)")
        prev_key = key
        out.append((m.group("sev"), code, f, ln, col))
    return out


def main() -> int:
    index = (ROOT / "index.toml").read_text()
    registered = {}
    for block in index.split("[[case]]")[1:]:
        name = toml_str(block, "name")
        registered[name] = set(toml_list(block, "codes") or [])
    if len(registered) != index.count("[[case]]"):
        fail("index.toml", "duplicate case names")

    seen = set()
    for kind in ("valid", "invalid"):
        for case_dir in sorted((ROOT / kind).iterdir()):
            if not case_dir.is_dir():
                continue
            name = case_dir.name
            seen.add(name)
            rel = case_dir.relative_to(ROOT)
            meta_path = case_dir / "case.toml"
            if not meta_path.exists():
                fail(rel, "no case.toml")
                continue
            meta = meta_path.read_text()

            for key in ("name", "kind", "title"):
                if toml_str(meta, key) is None:
                    fail(rel, f"case.toml is missing `{key}`")
            if toml_str(meta, "name") != name:
                fail(rel, "case.toml `name` does not match the directory name")
            if toml_str(meta, "kind") != kind:
                fail(rel, f"case.toml `kind` should be {kind!r}")
            if not toml_list(meta, "spec"):
                fail(rel, "case.toml has no `spec` citation")
            entry = toml_list(meta, "entry") or []
            if not entry:
                fail(rel, "case.toml has no `entry` files")
            for f in entry:
                if not (case_dir / f).exists():
                    fail(rel, f"entry file {f} does not exist")
            for key in ("product", "run"):
                f = toml_str(meta, key)
                if f and not (case_dir / f).exists():
                    fail(rel, f"`{key}` names {f}, which does not exist")

            for f in list(case_dir.rglob("*.pir")) + list(case_dir.rglob("*.expected")):
                check_text_rules(f)

            diags = read_expected_diag(case_dir)
            codes = {c for _, c, _, _, _ in diags}
            errors = [d for d in diags if d[0] == "error"]
            has_trap = (case_dir / "expected.trap.json").exists()
            if has_trap:
                codes.add("E0902")

            if kind == "invalid" and not errors and not has_trap:
                fail(rel, "an invalid case must expect at least one error")
            if kind == "valid" and errors:
                fail(rel, "a valid case must expect no errors")
            if toml_bool(meta, "runtime") and not (case_dir / "data").exists():
                fail(rel, "runtime = true but there is no data/ directory")

            if name not in registered:
                fail(rel, "not registered in index.toml")
            elif registered[name] != codes:
                fail(rel, f"index.toml codes {sorted(registered[name])} != "
                          f"expectation files {sorted(codes)}")

    for name in registered:
        if name not in seen:
            fail("index.toml", f"registered case {name} has no directory")

    for p in problems:
        print(p)
    print(f"{len(seen)} cases checked, {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
