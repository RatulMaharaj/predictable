"""The tracer: run a component body once, with proxies, and keep the expression it built.

This is step 2 of `02-dsl.md` §8. It is the whole runtime of the DSL — there is no interpreter,
no ``eval`` and no per-modelpoint callback. A body executes exactly once, ever.

.. code-block:: python

    def net_death_strain(sum_assured, reserve, deaths):
        strain_per_death = sum_assured - reserve[t - 1]
        return strain_per_death * deaths

    trace(net_death_strain, [Param("sum_assured"), Param("reserve"), Param("deaths")]).pir
    # '(sum_assured - reserve[t-1]) * deaths'

Local variables inline because a name bound to a proxy is a name bound to a subtree. Nothing is
promoted to a component behind the author's back (`02-dsl.md` §2.4).
"""

from __future__ import annotations

import inspect
from dataclasses import dataclass
from typing import Any, Callable, Iterable, Mapping

from . import errors
from .ir import AGGREGATES, Agg, At, Call, Expr, Lag, Lookup, Ref, lit_of, walk
from .proxy import ExprProxy, TableProxy, TraceContext

__all__ = ["Param", "TraceResult", "trace", "trace_init", "dependencies", "is_stage2"]


@dataclass(frozen=True)
class Param:
    """What the tracer knows about one parameter before the body runs.

    ``shape``/``unit`` come from the parameter's resolved declaration; ``stage2`` marks a value
    whose own expression contains an aggregate, which is what `E1207` is measured against.
    """

    name: str
    shape: str = "Series"
    unit: str | None = None
    stage2: bool = False
    table: TableProxy | None = None


@dataclass(frozen=True)
class TraceResult:
    """The output of one trace: an IR expression, plus what it turned out to depend on."""

    expr: Expr
    dependencies: tuple[str, ...]
    tables: tuple[str, ...]
    stage2: bool

    @property
    def pir(self) -> str:
        """Canonical `.pir` text of the expression — what ``expr = "..."`` would contain."""
        return self.expr.to_pir()

    def to_json(self) -> dict[str, Any]:
        return self.expr.to_json()


def _bind(func: Callable[..., Any], params: Mapping[str, Param], ctx: TraceContext) -> dict[str, Any]:
    signature = inspect.signature(func)
    kwargs: dict[str, Any] = {}
    for name, parameter in signature.parameters.items():
        default = parameter.default
        if isinstance(default, TableProxy):
            kwargs[name] = default.bind(ctx)
            continue
        spec = params.get(name, Param(name))
        if spec.table is not None:
            kwargs[name] = spec.table.bind(ctx)
            continue
        if default is not inspect.Parameter.empty and not isinstance(default, ExprProxy):
            # A plain-Python default (`lo=lo` from a generated declaration) is a build-time
            # constant and stays one: it is folded into the expression as a literal.
            kwargs[name] = default
            continue
        kwargs[name] = ExprProxy(
            Ref(spec.name),
            name=spec.name,
            shape=spec.shape,
            unit=spec.unit,
            stage2=spec.stage2,
            ctx=ctx,
        )
    return kwargs


def _normalise(params: Mapping[str, Param] | Iterable[Param] | None) -> dict[str, Param]:
    if params is None:
        return {}
    if isinstance(params, Mapping):
        return dict(params)
    return {p.name: p for p in params}


def trace(
    func: Callable[..., Any],
    params: Mapping[str, Param] | Iterable[Param] | None = None,
    *,
    component: str | None = None,
    root: str = "expr",
) -> TraceResult:
    """Run ``func`` once with proxies and return the IR expression it built.

    ``component`` is the name being defined; passing it is what lets the tracer raise `E1202`
    on an unlagged self-reference. ``root`` is ``"expr"`` or ``"init"`` and decides whether a
    stage-2 parameter may be read (`E1207`).
    """
    if component is None:
        component = getattr(func, "__name__", None)
    specs = _normalise(params)
    ctx = TraceContext(component=component, root=root)
    result = func(**_bind(func, specs, ctx))
    expr = _result_expr(result, component)
    return TraceResult(
        expr=expr,
        dependencies=dependencies(expr),
        tables=_tables(expr),
        stage2=is_stage2(expr),
    )


def trace_init(
    func: Callable[..., Any],
    params: Mapping[str, Param] | Iterable[Param] | None = None,
    *,
    component: str | None = None,
) -> TraceResult:
    """Trace an ``init=`` lambda. Identical to :func:`trace` but in the ``init`` root.

    ``init`` is evaluated in stage 2, so a stage-2 value is legal here and only here — this is
    the IR's single backward channel (`01-ir.md` §8.2).
    """
    return trace(func, params, component=component, root="init")


def _result_expr(result: Any, component: str | None) -> Expr:
    if isinstance(result, ExprProxy):
        return result.to_expr()
    if isinstance(result, Expr):
        return result
    if isinstance(result, (bool, int, float, str)):
        return lit_of(result)
    if result is None:
        errors.raise_e1201_no_return(component or "this component")
    if isinstance(result, (list, tuple, dict, set)):
        errors.raise_e1205_iteration(
            f"the {type(result).__name__} returned by {component}",
            operation="return a container from",
        )
    errors.raise_e1206_non_builtin(
        component or "this component",
        f"a {type(result).__name__} returned from a component body",
    )
    raise AssertionError("unreachable")  # pragma: no cover


def dependencies(expr: Expr) -> tuple[str, ...]:
    """Every component name the expression reads, in first-appearance order.

    This is the DSL's view of the graph edges of `01-ir.md` §3 — and it is exactly the set the
    checker will rebuild from the emitted `.pir`, so a mismatch is a bug in one of the two.
    """
    seen: list[str] = []
    for node in walk(expr):
        if isinstance(node, (Ref, Lag, At)) and node.name not in seen:
            seen.append(node.name)
    return tuple(seen)


def _tables(expr: Expr) -> tuple[str, ...]:
    seen: list[str] = []
    for node in walk(expr):
        if isinstance(node, Lookup):
            name = node.table.split(".", 1)[0]
            if name not in seen:
                seen.append(name)
    return tuple(seen)


def is_stage2(expr: Expr) -> bool:
    """True when the expression reduces over ``t`` — the definition of stage 2 (§8.2)."""
    return any(
        isinstance(node, Agg) or (isinstance(node, Call) and node.func in AGGREGATES)
        for node in walk(expr)
    )
