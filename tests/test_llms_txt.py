"""`llms.txt`, `llms-full.txt` and the diagnostics index — the CI link check of 04-verify.md §8.3.

An index that points at a file which does not exist is worse than no index: an agent that follows
a dead link has no way to tell "this page is missing" from "I asked for the wrong thing". So every
link in `llms.txt` is resolved against the repository here, `llms-full.txt` is regenerated and
compared, and the anchor discipline that makes every diagnostic's `doc_url` resolvable is asserted
from the documentation side as well as from the registry side.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent
LLMS = REPO / "llms.txt"
LLMS_FULL = REPO / "llms-full.txt"
CATALOGUE = REPO / "docs" / "llm" / "diagnostics.md"
GENERATOR = REPO / "tools" / "gen_llms_full.py"

LINK = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")


def links() -> list[tuple[str, str]]:
    return LINK.findall(LLMS.read_text(encoding="utf-8"))


def test_llms_txt_follows_the_convention():
    text = LLMS.read_text(encoding="utf-8")
    lines = text.splitlines()
    assert lines[0] == "# predictable"
    # An H1, then a blockquote summary, then link sections.
    summary = [line for line in lines[1:8] if line.startswith(">")]
    assert len(summary) >= 3, "llms.txt needs a blockquote summary under the H1"
    assert text.count("\n## ") >= 4, "llms.txt needs link sections"


def test_every_link_in_llms_txt_resolves():
    missing = []
    for label, target in links():
        assert target.startswith("/"), f"{label}: links must be repo-absolute, got {target!r}"
        if not (REPO / target.lstrip("/")).exists():
            missing.append(f"{label} -> {target}")
    assert not missing, f"llms.txt points at files that do not exist: {missing}"


def test_llms_txt_covers_the_things_an_agent_must_read_first():
    targets = {target for _, target in links()}
    for required in (
        "/docs/design/01-ir.md",
        "/docs/design/04-verify.md",
        "/docs/llm/diagnostics.md",
        "/skills/predictable-migration/SKILL.md",
        "/skills/predictable-migration/scripts/loop.py",
    ):
        assert required in targets, f"llms.txt does not link {required}"


def test_llms_full_is_current_and_within_budget():
    done = subprocess.run(
        [sys.executable, str(GENERATOR), "--check"], capture_output=True, text=True, cwd=REPO
    )
    assert done.returncode == 0, done.stdout + done.stderr


def test_llms_full_contains_the_normative_specs():
    text = LLMS_FULL.read_text(encoding="utf-8")
    assert "Source: `docs/design/01-ir.md`" in text
    assert "Source: `docs/design/04-verify.md`" in text
    assert "Source: `docs/llm/diagnostics.md`" in text
    # The worked model is included as source, fenced so it is unambiguous.
    assert "Source: `models/term_annual/build/model.pir`" in text
    assert "```toml" in text


def test_generator_refuses_to_exceed_the_token_budget(monkeypatch):
    sys.path.insert(0, str(GENERATOR.parent))
    import gen_llms_full  # noqa: PLC0415

    monkeypatch.setattr(gen_llms_full, "TOKEN_BUDGET", 10)
    assert gen_llms_full.main(["--stdout"]) == 1


@pytest.mark.parametrize("code", ["E0201", "W0102", "P0302", "N0302", "H0201"])
def test_diagnostics_index_anchors_every_namespace(code):
    """The `doc_url` contract: `.../llm/diagnostics/#<code>` must land on a section."""
    catalogue = CATALOGUE.read_text(encoding="utf-8")
    assert f'<a id="{code}"></a>' in catalogue
    assert f"### `{code}` —" in catalogue


def test_every_anchor_in_the_catalogue_is_unique():
    catalogue = CATALOGUE.read_text(encoding="utf-8")
    anchors = re.findall(r'<a id="([^"]+)"></a>', catalogue)
    duplicates = {a for a in anchors if anchors.count(a) > 1}
    assert not duplicates, f"duplicate anchors would make a doc_url ambiguous: {duplicates}"
    assert len(anchors) > 100, "the catalogue looks truncated"
