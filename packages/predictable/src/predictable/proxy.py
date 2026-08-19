"""``ExprProxy`` — the symbolic value a component body sees while it is traced.

A component body executes **exactly once**, at build time, with an ``ExprProxy`` in place of every
parameter. Operators on the proxy do not compute; they build IR ``Expr`` nodes (`02-dsl.md` §8
step 2). Local variables therefore inline for free: a name bound to a proxy is a name bound to a
subtree, and using it twice splices that subtree in twice, so

.. code-block:: python

    strain_per_death = sum_assured - reserve[t-1]
    return strain_per_death * deaths

traces to ``(sum_assured - reserve[t-1]) * deaths`` with no component in between. The IR graph
stays flat by construction (`01-ir.md` §12).

Everything the tracer cannot represent is refused here, at the operator, with a diagnostic that
names its replacement: ``__bool__`` is `E1201`, ``__iter__`` / ``__len__`` / ``__hash__`` are
`E1205`, an unknown attribute or call is `E1206`.
"""

from __future__ import annotations

import linecache
from typing import Any, Sequence

from . import errors
from .diagnostics import capture_span
from .ir import At, Binary, Call, Expr, Lag, Lit, Lookup, Ref, Unary, lit_of

__all__ = ["ExprProxy", "TableProxy", "TraceContext", "t", "lift", "as_expr", "is_traced"]


class TraceContext:
    """What the tracer knows while a single body is running.

    ``component`` is the name being defined — the thing an unlagged self-reference is measured
    against (`E1202`) — and ``root`` is ``"expr"`` or ``"init"``, which decides whether a stage-2
    value may be read at all (`E1207`).
    """

    __slots__ = ("component", "root", "stage2_names")

    def __init__(
        self,
        component: str | None = None,
        root: str = "expr",
        stage2_names: Sequence[str] = (),
    ) -> None:
        self.component = component
        self.root = root
        self.stage2_names = tuple(stage2_names)


_DEFAULT_CTX = TraceContext()


class ExprProxy:
    """A symbolic model value carrying the IR subtree built so far."""

    __slots__ = ("_expr", "_name", "_shape", "_unit", "_stage2", "_ctx", "_offset")

    def __init__(
        self,
        expr: Expr,
        *,
        name: str | None = None,
        shape: str = "Series",
        unit: str | None = None,
        stage2: bool = False,
        ctx: TraceContext | None = None,
        offset: int | None = None,
    ) -> None:
        object.__setattr__(self, "_expr", expr)
        object.__setattr__(self, "_name", name)
        object.__setattr__(self, "_shape", shape)
        object.__setattr__(self, "_unit", unit)
        object.__setattr__(self, "_stage2", stage2)
        object.__setattr__(self, "_ctx", ctx or _DEFAULT_CTX)
        # Set only on values derived purely from `t` — `t`, `t-1`, `t+2` — so that `x[t-1]`
        # can read the offset back out while `t - 1` still works as an ordinary value.
        object.__setattr__(self, "_offset", offset)

    # -- introspection ---------------------------------------------------------------

    @property
    def name(self) -> str | None:
        """The component this proxy started as, or ``None`` once it is a computed subtree."""
        return self._name

    @property
    def shape(self) -> str:
        return self._shape

    @property
    def unit(self) -> str | None:
        return self._unit

    def to_expr(self) -> Expr:
        """The IR subtree, after the checks that reading this value triggers."""
        if self._name is not None and isinstance(self._expr, Ref):
            if self._ctx.component == self._name:
                errors.raise_e1202_self_reference(self._name)
            if self._stage2 and self._ctx.root == "expr":
                errors.raise_e1207_stage2_in_expr(self._name)
        return self._expr

    def _derive(self, expr: Expr, *, unit: str | None = None, stage2: bool = False) -> "ExprProxy":
        return ExprProxy(
            expr,
            shape=self._shape,
            unit=unit,
            stage2=stage2 or self._stage2,
            ctx=self._ctx,
        )

    # -- arithmetic ------------------------------------------------------------------

    def _binary(self, op: str, other: Any, *, reflected: bool = False) -> "ExprProxy":
        lhs, rhs = (other, self) if reflected else (self, other)
        node = Binary(op, _operand(lhs, self._ctx), _operand(rhs, self._ctx))
        unit = self._unit if op in ("add", "sub") else None
        return self._derive(node, unit=unit, stage2=_stage2_of(lhs) or _stage2_of(rhs))

    def __add__(self, other: Any) -> "ExprProxy":
        return self._binary("add", other)

    def __radd__(self, other: Any) -> "ExprProxy":
        return self._binary("add", other, reflected=True)

    def __sub__(self, other: Any) -> "ExprProxy":
        return self._binary("sub", other)

    def __rsub__(self, other: Any) -> "ExprProxy":
        return self._binary("sub", other, reflected=True)

    def __mul__(self, other: Any) -> "ExprProxy":
        return self._binary("mul", other)

    def __rmul__(self, other: Any) -> "ExprProxy":
        return self._binary("mul", other, reflected=True)

    def __truediv__(self, other: Any) -> "ExprProxy":
        # §5: dividing a rate to change its basis is `E1402`, not arithmetic.
        if (
            self._unit is not None
            and self._unit.startswith("rate(")
            and isinstance(other, (int, float))
            and not isinstance(other, bool)
            and other not in (0, 1)
        ):
            basis = self._unit[len("rate(") : -1]
            errors.raise_e1402_basis_conversion(self._name or "this rate", basis, other)
        return self._binary("div", other)

    def __rtruediv__(self, other: Any) -> "ExprProxy":
        return self._binary("div", other, reflected=True)

    def __pow__(self, other: Any) -> "ExprProxy":
        return self._binary("pow", other)

    def __rpow__(self, other: Any) -> "ExprProxy":
        return self._binary("pow", other, reflected=True)

    def __neg__(self) -> "ExprProxy":
        return self._derive(Unary("neg", self.to_expr()))

    def __pos__(self) -> "ExprProxy":
        return self

    def __abs__(self) -> "ExprProxy":
        return self._derive(Call("abs", (self.to_expr(),)))

    def __round__(self, ndigits: int = 0) -> "ExprProxy":
        return self._derive(Call("round", (self.to_expr(), lit_of(int(ndigits)))))

    # -- comparison ------------------------------------------------------------------

    def __eq__(self, other: Any) -> "ExprProxy":  # type: ignore[override]
        return self._binary("eq", other)

    def __ne__(self, other: Any) -> "ExprProxy":  # type: ignore[override]
        return self._binary("ne", other)

    def __lt__(self, other: Any) -> "ExprProxy":
        return self._binary("lt", other)

    def __le__(self, other: Any) -> "ExprProxy":
        return self._binary("le", other)

    def __gt__(self, other: Any) -> "ExprProxy":
        return self._binary("gt", other)

    def __ge__(self, other: Any) -> "ExprProxy":
        return self._binary("ge", other)

    # -- time indexing ---------------------------------------------------------------

    def __getitem__(self, key: Any) -> "ExprProxy":
        if self._name is None or not isinstance(self._expr, Ref):
            errors.raise_e1205_iteration(
                _describe(self._expr), operation="index a computed expression rather than"
            )
        name = self._name
        if isinstance(key, ExprProxy) and key._offset is not None:
            offset = key._offset
            if offset > 0:
                errors.raise_e1204_forward_reference(name, f"t+{offset}")
            if offset == 0:
                return ExprProxy(Ref(name), name=name, shape=self._shape, unit=self._unit,
                                 stage2=self._stage2, ctx=self._ctx)
            return self._derive(Lag(name, -offset), unit=self._unit)
        if isinstance(key, ExprProxy):
            errors.raise_e1203_non_constant_lag(name, _describe(key._expr))
        if isinstance(key, bool) or not isinstance(key, int):
            errors.raise_e1203_non_constant_lag(name, repr(key))
        if key < 0:
            errors.raise_e1203_non_constant_lag(name, str(key))
        return self._derive(At(name, int(key)), unit=self._unit)

    # -- refusals --------------------------------------------------------------------

    def __bool__(self) -> bool:
        errors.raise_e1201_truth_test(
            _describe(self._expr), self._shape, construct=_construct_at_fault()
        )
        raise AssertionError("unreachable")  # pragma: no cover

    def __iter__(self):
        errors.raise_e1205_iteration(_describe(self._expr))
        raise AssertionError("unreachable")  # pragma: no cover

    def __len__(self) -> int:
        errors.raise_e1205_iteration(_describe(self._expr), operation="take the length of")
        raise AssertionError("unreachable")  # pragma: no cover

    def __contains__(self, item: Any) -> bool:
        errors.raise_e1205_iteration(_describe(self._expr), operation="test membership in")
        raise AssertionError("unreachable")  # pragma: no cover

    def __hash__(self) -> int:  # type: ignore[override]
        errors.raise_e1205_iteration(
            _describe(self._expr), operation="use as a dict key or set member"
        )
        raise AssertionError("unreachable")  # pragma: no cover

    def __int__(self) -> int:
        errors.raise_e1206_non_builtin(_describe(self._expr), "int")
        raise AssertionError("unreachable")  # pragma: no cover

    def __float__(self) -> float:
        errors.raise_e1206_non_builtin(_describe(self._expr), "float")
        raise AssertionError("unreachable")  # pragma: no cover

    def __index__(self) -> int:
        errors.raise_e1206_non_builtin(_describe(self._expr), "int")
        raise AssertionError("unreachable")  # pragma: no cover

    def __array__(self, *_args: Any, **_kwargs: Any) -> Any:
        # numpy asks for this before any ufunc; refusing here is what turns `np.exp(x)` into
        # "use predictable.fn.exp" instead of an array of proxies.
        errors.raise_e1206_non_builtin(
            _describe(self._expr), "numpy", suggestion=f"exp({_describe(self._expr)})"
        )
        raise AssertionError("unreachable")  # pragma: no cover

    __array_ufunc__ = None

    def __call__(self, *_args: Any, **_kwargs: Any) -> Any:
        errors.raise_e1206_non_builtin(_describe(self._expr), f"{_describe(self._expr)}(...)")
        raise AssertionError("unreachable")  # pragma: no cover

    def __getattr__(self, attribute: str) -> Any:
        if attribute.startswith("_"):
            raise AttributeError(attribute)
        errors.raise_e1206_non_builtin(
            _describe(self._expr), f"{_describe(self._expr)}.{attribute}"
        )
        raise AssertionError("unreachable")  # pragma: no cover

    def __setattr__(self, attribute: str, value: Any) -> None:
        errors.raise_e1206_non_builtin(
            _describe(self._expr), f"{_describe(self._expr)}.{attribute} = ..."
        )

    def __repr__(self) -> str:
        return f"<ExprProxy {self._expr.to_pir()}>"


class LookupProxy(ExprProxy):
    """The result of a table call. ``sa8990(age, gender, smoker).qx`` selects a value column."""

    __slots__ = ("_table", "_keys", "_values")

    def __init__(self, table: str, keys: tuple[Expr, ...], values: Sequence[str], ctx: TraceContext):
        # A single-value table allows the bare form — `sa8990@(age, gender, smoker)`; a
        # multi-value one must name its column, and does so until `.qx` selects it.
        node = Lookup(table, keys)
        super().__init__(node, shape="Series", ctx=ctx)
        object.__setattr__(self, "_table", table)
        object.__setattr__(self, "_keys", keys)
        object.__setattr__(self, "_values", tuple(values))

    def __getattr__(self, attribute: str) -> Any:
        if attribute.startswith("_"):
            raise AttributeError(attribute)
        if attribute in self._values:
            return ExprProxy(Lookup(f"{self._table}.{attribute}", self._keys), ctx=self._ctx)
        errors.raise_e1206_non_builtin(
            self._table,
            f"{self._table}(...).{attribute}",
            suggestion=(
                f"{self._table}(...).{self._values[0]}" if self._values else None
            ),
        )
        raise AssertionError("unreachable")  # pragma: no cover


class TableProxy:
    """A declared table, bound into a body through the ``tbl=tbl`` default-argument idiom.

    Calling it is the lookup; the IR's ``@`` sigil is emitted by the compiler (`02-dsl.md` §6).
    """

    __slots__ = ("name", "values", "_ctx")

    def __init__(self, name: str, values: Sequence[str] = ("value",), ctx: TraceContext | None = None):
        self.name = name
        self.values = tuple(values)
        self._ctx = ctx or _DEFAULT_CTX

    def bind(self, ctx: TraceContext) -> "TableProxy":
        return TableProxy(self.name, self.values, ctx)

    def __call__(self, *keys: Any) -> LookupProxy:
        return LookupProxy(
            self.name, tuple(_operand(k, self._ctx) for k in keys), self.values, self._ctx
        )

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"<TableProxy {self.name}>"


class TimeIndex(ExprProxy):
    """``t`` — the period index, usable both as a value and as the base of ``x[t-1]``."""

    __slots__ = ()

    def __init__(self, ctx: TraceContext | None = None) -> None:
        super().__init__(Ref("t"), name=None, shape="Series", ctx=ctx, offset=0)

    def _shifted(self, offset: int) -> ExprProxy:
        if offset == 0:
            return self
        op, k = ("sub", -offset) if offset < 0 else ("add", offset)
        return ExprProxy(
            Binary(op, Ref("t"), lit_of(k)), shape="Series", ctx=self._ctx, offset=offset
        )

    def __sub__(self, other: Any) -> ExprProxy:
        if isinstance(other, int) and not isinstance(other, bool):
            return self._shifted(-other)
        return super().__sub__(other)

    def __add__(self, other: Any) -> ExprProxy:
        if isinstance(other, int) and not isinstance(other, bool):
            return self._shifted(other)
        return super().__add__(other)


t = TimeIndex()
"""The period index. ``x[t-1]`` lags, ``x[0]`` indexes absolutely, ``t`` alone is a value."""


# --------------------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------------------


def is_traced(value: Any) -> bool:
    """True when ``value`` is a symbolic model value rather than a build-time constant."""
    return isinstance(value, ExprProxy)


def as_expr(value: Any) -> Expr:
    """The IR subtree of a traced value or of a build-time constant."""
    return _operand(value, _DEFAULT_CTX)


def lift(value: Any, ctx: TraceContext | None = None) -> ExprProxy:
    """Wrap a constant as a proxy so builtins can treat every argument uniformly."""
    if isinstance(value, ExprProxy):
        return value
    return ExprProxy(lit_of(value), ctx=ctx)


def _operand(value: Any, ctx: TraceContext) -> Expr:
    if isinstance(value, ExprProxy):
        return value.to_expr()
    if isinstance(value, Expr):
        return value
    if isinstance(value, (bool, int, float, str)):
        return lit_of(value)
    if isinstance(value, TableProxy):
        errors.raise_e1206_non_builtin(
            value.name, f"{value.name} used as a value", suggestion=f"{value.name}(<keys>)"
        )
    if hasattr(value, "_predictable_tag"):
        # A timing tag (`MID`) or enum member, which carry their own IR spelling.
        return lit_of(value._predictable_tag)
    errors.raise_e1206_non_builtin(
        type(value).__name__, f"a {type(value).__name__} in a model expression"
    )
    raise AssertionError("unreachable")  # pragma: no cover


def _stage2_of(value: Any) -> bool:
    return isinstance(value, ExprProxy) and value._stage2


def _describe(expr: Expr) -> str:
    """A short name for a node, used to make a diagnostic point at something recognisable."""
    if isinstance(expr, (Ref, Lag, At)):
        return expr.name
    if isinstance(expr, Lookup):
        return expr.table
    if isinstance(expr, Call):
        return expr.func
    return expr.to_pir()


_CONSTRUCTS = (" and ", " or ", "not ", "while ", "if ", "assert ")


def _construct_at_fault() -> str:
    """Which Python construct truth-tested the value, read back off the user's own line."""
    span = capture_span()
    line = span.text or linecache.getline(span.file, span.start_pos.line if span.start_pos else 0)
    stripped = line.strip()
    for construct in _CONSTRUCTS:
        if construct in f" {stripped} ":
            return construct.strip()
    return "bool()"
