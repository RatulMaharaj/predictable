"""Timing tags (`01-ir.md` §2.5). Four of them, and no default — see `02-dsl.md` §2.5."""

from __future__ import annotations

__all__ = ["Timing", "START", "END", "MID", "POINT"]


class Timing:
    """A timing tag. Carries its IR spelling so ``retime(x, MID)`` needs no lookup table."""

    __slots__ = ("_predictable_tag", "gloss")

    def __init__(self, tag: str, gloss: str) -> None:
        object.__setattr__(self, "_predictable_tag", tag)
        object.__setattr__(self, "gloss", gloss)

    @property
    def tag(self) -> str:
        return self._predictable_tag

    def __str__(self) -> str:
        return self._predictable_tag

    def __repr__(self) -> str:
        return f"<Timing {self._predictable_tag}: {self.gloss}>"


START = Timing("start", "in advance — the flow happens at the start of period t")
END = Timing("end", "in arrears — the flow happens at the end of period t")
MID = Timing("mid", "mid-period approximation — the flow happens halfway through period t")
POINT = Timing("point", "a stock, not a flow — a balance observed at a point in time")
