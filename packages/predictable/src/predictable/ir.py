"""IR expression trees as Python data (`01-ir.md` §2.6).

These dataclasses mirror ``predictable_ir::expr::Expr`` field for field. The JSON produced by
:func:`to_json` is exactly what serde emits for the Rust type — internally tagged on ``"node"``,
with the Rust variant names — so a traced body can be handed to the engine without a translation
layer, and a ``pir.json`` document round-trips through this module unchanged.

Nothing here computes: an ``Expr`` is data. The only behaviour is rendering, and
:func:`to_pir` mirrors ``predictable-fmt``'s canonical expression form (`01-ir.md` §4.1) so the
text this module writes is already the text ``predictable fmt`` would write.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Iterator

__all__ = [
    "Expr",
    "Lit",
    "Ref",
    "Lag",
    "At",
    "Unary",
    "Binary",
    "If",
    "Call",
    "Lookup",
    "Agg",
    "BUILTINS",
    "AGGREGATES",
    "TIMING_OPS",
    "format_f64",
    "to_json",
    "to_pir",
    "walk",
    "lit_of",
]

# --------------------------------------------------------------------------------------
# the closed builtin set (`01-ir.md` §2.8, mirrored from crates/predictable-ir/src/builtins.rs)
# --------------------------------------------------------------------------------------

BUILTINS: frozenset[str] = frozenset(
    {
        # aggregates
        "at", "count_while", "first", "last", "max_over", "min_over", "npv", "sum", "sum_kahan",
        # arithmetic
        "abs", "ceil", "clamp", "floor", "max", "min", "round", "sign",
        # exp / log
        "exp", "ln", "pow", "sqrt",
        # rates
        "annuity_factor", "compound", "i_from_v", "nominal_to_periodic", "to_annual",
        "to_monthly", "v_from_i",
        # timing
        "cum", "diff", "retime", "shift",
        # logic
        "and", "coalesce", "eq", "ge", "gt", "is_null", "le", "lt", "ne", "not", "or",
        # dates
        "add_months", "day", "month", "months_between", "year", "year_frac",
    }
)
"""Every legal ``Call`` function name in IR 1.0."""

AGGREGATES: frozenset[str] = frozenset(
    {"sum", "sum_kahan", "npv", "first", "last", "at", "max_over", "min_over", "count_while"}
)
"""The reductions over ``t``. A body containing one of these is stage 2 (`01-ir.md` §8.2)."""

TIMING_OPS: frozenset[str] = frozenset({"retime", "shift", "cum", "diff"})

# --------------------------------------------------------------------------------------
# nodes
# --------------------------------------------------------------------------------------


class Expr:
    """Base class of the expression tree. Subclasses are the variants of `01-ir.md` §2.6."""

    __slots__ = ()

    def children(self) -> list[tuple[str, "Expr"]]:
        """Immediate children paired with their ``ExprPath`` segment, in canonical order."""
        return []

    def to_json(self) -> dict[str, Any]:
        raise NotImplementedError

    def to_pir(self) -> str:
        """Canonical `.pir` expression text for this tree."""
        return _print(self, _CTX_TOP)

    def __str__(self) -> str:  # pragma: no cover - convenience
        return self.to_pir()


@dataclass(frozen=True)
class Lit(Expr):
    """A literal and the dtype that gives it meaning."""

    dtype: str
    value: bool | int | float | str

    def to_json(self) -> dict[str, Any]:
        return {"node": "Lit", "dtype": self.dtype, "value": self.value}


@dataclass(frozen=True)
class Ref(Expr):
    """``x`` — current period, current modelpoint."""

    name: str

    def to_json(self) -> dict[str, Any]:
        return {"node": "Ref", "name": self.name}


@dataclass(frozen=True)
class Lag(Expr):
    """``x[t-k]``, ``k >= 1``."""

    name: str
    k: int

    def to_json(self) -> dict[str, Any]:
        return {"node": "Lag", "name": self.name, "k": self.k}


@dataclass(frozen=True)
class At(Expr):
    """``x[k]``, absolute period index, ``k >= 0``."""

    name: str
    k: int

    def to_json(self) -> dict[str, Any]:
        return {"node": "At", "name": self.name, "k": self.k}


@dataclass(frozen=True)
class Unary(Expr):
    op: str  # "neg" | "not"
    operand: Expr

    def children(self) -> list[tuple[str, Expr]]:
        return [("operand", self.operand)]

    def to_json(self) -> dict[str, Any]:
        return {"node": "Unary", "op": self.op, "operand": self.operand.to_json()}


@dataclass(frozen=True)
class Binary(Expr):
    op: str  # add sub mul div pow eq ne lt le gt ge and or
    lhs: Expr
    rhs: Expr

    def children(self) -> list[tuple[str, Expr]]:
        return [("lhs", self.lhs), ("rhs", self.rhs)]

    def to_json(self) -> dict[str, Any]:
        return {
            "node": "Binary",
            "op": self.op,
            "lhs": self.lhs.to_json(),
            "rhs": self.rhs.to_json(),
        }


@dataclass(frozen=True)
class If(Expr):
    """A *value* conditional: both arms are typed, neither is control flow (`01-ir.md` §2.6)."""

    cond: Expr
    then: Expr
    otherwise: Expr

    def children(self) -> list[tuple[str, Expr]]:
        return [("cond", self.cond), ("then", self.then), ("else", self.otherwise)]

    def to_json(self) -> dict[str, Any]:
        return {
            "node": "If",
            "cond": self.cond.to_json(),
            "then": self.then.to_json(),
            "else": self.otherwise.to_json(),
        }


@dataclass(frozen=True)
class Call(Expr):
    """A builtin call. The set is closed; see :data:`BUILTINS`."""

    func: str
    args: tuple[Expr, ...]

    def children(self) -> list[tuple[str, Expr]]:
        return [(f"arg{i}", a) for i, a in enumerate(self.args)]

    def to_json(self) -> dict[str, Any]:
        return {"node": "Call", "fn": self.func, "args": [a.to_json() for a in self.args]}


@dataclass(frozen=True)
class Lookup(Expr):
    """``tbl@(k1, k2, ...)``."""

    table: str
    keys: tuple[Expr, ...]

    def children(self) -> list[tuple[str, Expr]]:
        return [(f"key{i}", k) for i, k in enumerate(self.keys)]

    def to_json(self) -> dict[str, Any]:
        return {"node": "Lookup", "table": self.table, "keys": [k.to_json() for k in self.keys]}


@dataclass(frozen=True)
class Agg(Expr):
    """A reduction over ``t``, with an optional predicate.

    The tracer emits aggregates as :class:`Call` nodes, because that is the spelling the `.pir`
    grammar has and the shape ``predictable-syntax`` parses back (`01-ir.md` §2.8). ``Agg`` is
    here so that a ``pir.json`` document written by the engine round-trips through this module.
    """

    op: str
    value: Expr
    pred: Expr | None = None

    def children(self) -> list[tuple[str, Expr]]:
        out: list[tuple[str, Expr]] = [("value", self.value)]
        if self.pred is not None:
            out.append(("pred", self.pred))
        return out

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {"node": "Agg", "op": self.op, "value": self.value.to_json()}
        if self.pred is not None:
            out["pred"] = self.pred.to_json()
        return out


# --------------------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------------------


def to_json(expr: Expr) -> dict[str, Any]:
    """The serde encoding of ``expr`` (internally tagged on ``"node"``)."""
    return expr.to_json()


def to_pir(expr: Expr) -> str:
    """Canonical `.pir` expression text for ``expr``."""
    return expr.to_pir()


def walk(expr: Expr) -> Iterator[Expr]:
    """Pre-order walk of every node in the tree."""
    yield expr
    for _seg, child in expr.children():
        yield from walk(child)


def lit_of(value: bool | int | float | str) -> Lit:
    """Lift a plain Python constant to the ``Lit`` node with the dtype the IR gives it."""
    if isinstance(value, bool):
        return Lit("bool", value)
    if isinstance(value, int):
        return Lit("i64", value)
    if isinstance(value, float):
        return Lit("f64", value)
    if isinstance(value, str):
        return Lit("str", value)
    raise TypeError(f"{value!r} has no IR literal form")


def format_f64(v: float) -> str:
    """Shortest round-tripping text of an ``f64``, matching ``predictable-fmt``'s Ryū output.

    A float never loses its float-ness (``45.0`` stays ``45.0``) and exponent notation is
    normalised to Ryū's spelling (``1e-8``, not Python's ``1e-08``).
    """
    if v != v or v in (float("inf"), float("-inf")):
        return repr(v)
    text = repr(float(v))
    if "e" in text:
        mantissa, _, exponent = text.partition("e")
        if mantissa.endswith(".0"):
            mantissa = mantissa[:-2]
        sign = "-" if exponent.startswith("-") else ""
        digits = exponent.lstrip("+-").lstrip("0") or "0"
        return f"{mantissa}e{sign}{digits}"
    if "." not in text:
        text += ".0"
    return text


# --------------------------------------------------------------------------------------
# canonical printing — mirrors crates/predictable-fmt/src/expr_fmt.rs
# --------------------------------------------------------------------------------------

_P_IF = 0
_P_OR = 1
_P_AND = 2
_P_CMP = 3
_P_SUM = 4
_P_PRODUCT = 5
_P_UNARY = 6
_P_POW = 7
_P_ATOM = 8

_BINOP_PREC = {
    "or": _P_OR,
    "and": _P_AND,
    "eq": _P_CMP,
    "ne": _P_CMP,
    "lt": _P_CMP,
    "le": _P_CMP,
    "gt": _P_CMP,
    "ge": _P_CMP,
    "add": _P_SUM,
    "sub": _P_SUM,
    "mul": _P_PRODUCT,
    "div": _P_PRODUCT,
    "pow": _P_POW,
}

_BINOP_SYMBOL = {
    "add": "+",
    "sub": "-",
    "mul": "*",
    "div": "/",
    "pow": "^",
    "eq": "==",
    "ne": "!=",
    "lt": "<",
    "le": "<=",
    "gt": ">",
    "ge": ">=",
    "and": "and",
    "or": "or",
}

_CTX_TOP = ("top", 0, False)
_CTX_UNARY = ("unary", 0, False)


def _prec(e: Expr) -> int:
    if isinstance(e, If):
        return _P_IF
    if isinstance(e, Binary):
        return _BINOP_PREC[e.op]
    if isinstance(e, Unary):
        return _P_UNARY
    return _P_ATOM


def _needs_parens(e: Expr, ctx: tuple[str, int, bool]) -> bool:
    kind, minimum, right = ctx
    p = _prec(e)
    if kind == "top":
        return False
    if kind == "unary":
        return p < _P_UNARY
    if p < minimum:
        return True
    if right and p == minimum:
        return True
    if minimum == _P_CMP and p == _P_CMP:
        return True
    return False


def _print(e: Expr, ctx: tuple[str, int, bool]) -> str:
    body = _print_bare(e)
    return f"({body})" if _needs_parens(e, ctx) else body


def _print_bare(e: Expr) -> str:
    if isinstance(e, Lit):
        return _print_lit(e)
    if isinstance(e, Ref):
        return e.name
    if isinstance(e, Lag):
        return f"{e.name}[t-{e.k}]"
    if isinstance(e, At):
        return f"{e.name}[{e.k}]"
    if isinstance(e, Unary):
        prefix = "-" if e.op == "neg" else "not "
        return prefix + _print(e.operand, _CTX_UNARY)
    if isinstance(e, Binary):
        p = _BINOP_PREC[e.op]
        # `^` is right-associative: the tight side is the left one.
        if e.op == "pow":
            lctx, rctx = ("operand", p, True), ("operand", p, False)
        else:
            lctx, rctx = ("operand", p, False), ("operand", p, True)
        return f"{_print(e.lhs, lctx)} {_BINOP_SYMBOL[e.op]} {_print(e.rhs, rctx)}"
    if isinstance(e, If):
        return (
            f"if {_print(e.cond, _CTX_TOP)} then {_print(e.then, _CTX_TOP)} "
            f"else {_print(e.otherwise, _CTX_TOP)}"
        )
    if isinstance(e, Call):
        return f"{e.func}({_print_list(e.func, e.args)})"
    if isinstance(e, Lookup):
        return f"{e.table}@({_print_list('', e.keys)})"
    if isinstance(e, Agg):
        args = [e.value] if e.pred is None else [e.value, e.pred]
        return f"{e.op}({_print_list(e.op, tuple(args))})"
    raise TypeError(f"not an Expr: {e!r}")  # pragma: no cover


def _print_list(func: str, args: tuple[Expr, ...]) -> str:
    parts = []
    for i, arg in enumerate(args):
        # `retime(x, mid)`: the timing tag is carried as a str literal and printed bare.
        if func == "retime" and i == 1 and isinstance(arg, Lit) and arg.dtype == "str":
            parts.append(str(arg.value))
        else:
            parts.append(_print(arg, _CTX_TOP))
    return ", ".join(parts)


def _print_lit(lit: Lit) -> str:
    if lit.dtype == "bool":
        return "true" if lit.value else "false"
    if lit.dtype == "i64":
        return str(lit.value)
    if lit.dtype == "f64":
        return format_f64(float(lit.value))
    escaped = str(lit.value).replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def from_json(doc: dict[str, Any]) -> Expr:
    """Rebuild an :class:`Expr` from its serde encoding. The inverse of :func:`to_json`."""
    node = doc["node"]
    if node == "Lit":
        return Lit(doc["dtype"], doc["value"])
    if node == "Ref":
        return Ref(doc["name"])
    if node == "Lag":
        return Lag(doc["name"], doc["k"])
    if node == "At":
        return At(doc["name"], doc["k"])
    if node == "Unary":
        return Unary(doc["op"], from_json(doc["operand"]))
    if node == "Binary":
        return Binary(doc["op"], from_json(doc["lhs"]), from_json(doc["rhs"]))
    if node == "If":
        return If(from_json(doc["cond"]), from_json(doc["then"]), from_json(doc["else"]))
    if node == "Call":
        return Call(doc["fn"], tuple(from_json(a) for a in doc["args"]))
    if node == "Lookup":
        return Lookup(doc["table"], tuple(from_json(k) for k in doc["keys"]))
    if node == "Agg":
        pred = doc.get("pred")
        return Agg(doc["op"], from_json(doc["value"]), from_json(pred) if pred else None)
    raise ValueError(f"unknown Expr node {node!r}")
