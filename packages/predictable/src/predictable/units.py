"""Unit and dtype annotations (`01-ir.md` §2.4, `02-dsl.md` §2.1).

A component's **return annotation carries its dtype and its unit**, and there is no inference: a
missing annotation is `E1101`. The annotations are ordinary :data:`typing.Annotated` aliases, so
they type-check in an editor and still carry the IR tag the compiler needs:

.. code-block:: python

    Money is Annotated[float, Unit.money]        # dtype f64, unit money
    Years is Annotated[int,   Unit.years]        # dtype i64, unit years
    Rate.annual is Annotated[float, Unit.rate("annual")]

Nothing here converts. Units are a shallow dimensional tag the checker compares; the conversions
that actuaries get wrong (`r / 12`) are functions in :mod:`predictable.fn`, never arithmetic.
"""

from __future__ import annotations

import datetime as _dt
from typing import Annotated, Any, get_args, get_origin

__all__ = [
    "Unit",
    "DType",
    "Money",
    "Prob",
    "Count",
    "Rate",
    "Years",
    "Months",
    "Factor",
    "Flag",
    "Num",
    "Str",
    "Date",
    "annotation_type",
    "UNITS",
]


class Unit:
    """A unit tag. ``Unit.money``, ``Unit.rate("monthly")`` — the IR spelling is ``str(unit)``."""

    __slots__ = ("tag",)

    def __init__(self, tag: str) -> None:
        self.tag = tag

    def __str__(self) -> str:
        return self.tag

    def __repr__(self) -> str:
        return f"Unit({self.tag!r})"

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Unit) and other.tag == self.tag

    def __hash__(self) -> int:
        return hash(self.tag)

    # the closed set of `01-ir.md` §2.4
    none: "Unit"
    money: "Unit"
    prob: "Unit"
    count: "Unit"
    years: "Unit"
    months: "Unit"
    factor: "Unit"

    @staticmethod
    def rate(basis: str) -> "Unit":
        """``rate(annual)`` / ``rate(monthly)`` / ``rate(period)`` — the three rate bases."""
        if basis not in ("annual", "monthly", "period"):
            raise ValueError(f"unknown rate basis {basis!r}; expected annual, monthly or period")
        return Unit(f"rate({basis})")


Unit.none = Unit("none")
Unit.money = Unit("money")
Unit.prob = Unit("prob")
Unit.count = Unit("count")
Unit.years = Unit("years")
Unit.months = Unit("months")
Unit.factor = Unit("factor")

UNITS: tuple[str, ...] = (
    "none",
    "money",
    "prob",
    "count",
    "years",
    "months",
    "factor",
    "rate(annual)",
    "rate(monthly)",
    "rate(period)",
)


class DType:
    """The IR dtype names. ``enum(Gender)`` is produced by :func:`enum_dtype`."""

    f64 = "f64"
    i64 = "i64"
    bool = "bool"
    date = "date"
    str = "str"


Money = Annotated[float, Unit.money]
Prob = Annotated[float, Unit.prob]
Count = Annotated[float, Unit.count]
Years = Annotated[int, Unit.years]
Months = Annotated[int, Unit.months]
Factor = Annotated[float, Unit.factor]
Flag = Annotated[bool, Unit.none]
Num = Annotated[float, Unit.none]
Str = Annotated[str, Unit.none]
Date = Annotated[_dt.date, Unit.none]


class _Rate:
    """``Rate.annual``, ``Rate.monthly``, ``Rate.period`` — a rate is never basis-less."""

    annual = Annotated[float, Unit.rate("annual")]
    monthly = Annotated[float, Unit.rate("monthly")]
    period = Annotated[float, Unit.rate("period")]

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return "<Rate: use Rate.annual / Rate.monthly / Rate.period>"


Rate = _Rate()

_PRIMITIVES: dict[Any, str] = {
    float: DType.f64,
    int: DType.i64,
    bool: DType.bool,
    str: DType.str,
    _dt.date: DType.date,
}


def annotation_type(annotation: Any) -> tuple[str, str] | None:
    """``(dtype, unit)`` for an annotation, or ``None`` when it carries neither.

    Recognised: the aliases above, the bare primitives (``bool``, ``str``, ``int``, ``float``,
    ``date``) which carry dtype and unit ``none``, and an :class:`~predictable.enums.Enum`
    subclass, which is ``enum(<Name>)``.
    """
    if annotation is None or annotation is Ellipsis:
        return None
    if get_origin(annotation) is Annotated:
        args = get_args(annotation)
        base, tags = args[0], args[1:]
        dtype = _PRIMITIVES.get(base)
        if dtype is None:
            return None
        unit = next((str(tag) for tag in tags if isinstance(tag, Unit)), "none")
        return dtype, unit
    if isinstance(annotation, type):
        enum_name = getattr(annotation, "_predictable_enum", None)
        if enum_name is not None:
            return f"enum({enum_name})", "none"
        # bool before int: `bool` is a subclass of `int` and would otherwise become i64.
        if annotation is bool:
            return DType.bool, "none"
        dtype = _PRIMITIVES.get(annotation)
        if dtype is not None:
            return dtype, "none"
    return None
