"""predictable v2 — the authoring DSL.

This package is an **authoring layer, not a runtime** (`02-dsl.md`). Its job is to turn readable
Python into IR expression trees, and to produce every diagnostic it possibly can while doing so.
No number is ever computed here: a component body runs once, at build time, against symbolic
proxies, and what comes out is data.

What exists today is the tracer (task T19):

* :class:`~predictable.proxy.ExprProxy` and ``t`` — the symbolic values a body sees.
* :mod:`predictable.fn` — the closed builtin set, each function building one IR node.
* :mod:`predictable.ir` — the IR expression tree, mirroring ``predictable-ir``'s serde model.
* :func:`~predictable.tracer.trace` — run a body once, get an ``Expr``.
* :mod:`predictable.errors` — the ``E12xx`` catalogue, with worked, actionable messages.

`predictable build` (task T21) turns all of that into files:

* :mod:`predictable.emit` — canonical `.pir` text, the schema/module/product split, table digests.
* :mod:`predictable.spanmap` — the Python ↔ `.pir` span map of `01-ir.md` §11.1, both directions.
* :mod:`predictable.builder` — emit, run the engine's checker in process, re-render its
  diagnostics against Python, then write ``build/`` or diff against it.
* :mod:`predictable.cli` — ``predictable-build`` / ``python -m predictable build``.

The declaration layer (task T20) sits on top of the tracer, unchanged:

* :mod:`predictable.declarations` — ``@series``/``@per_mp``/``@scalar``, ``ModelPoint``, ``Enum``,
  ``assumption``, ``table``, ``timeline``, ``product``, and the library layer (``extends``,
  ``@override``, ``@abstract``, ``@final``, ``name_suffix``).
* :mod:`predictable.units` — the annotations that carry dtype and unit.
* :func:`~predictable.declarations.build` — resolve every name, trace every body, run the
  declaration-level checks, and hand a fully resolved model to the emitter (T21).
"""

from .declarations import (
    Assumption,
    BuiltComponent,
    BuiltModel,
    Declaration,
    Enum,
    Key,
    Library,
    ModelPoint,
    ModelPointField,
    Product,
    Registry,
    TableDecl,
    Timeline,
    Value,
    abstract,
    assumption,
    build,
    default,
    extends,
    final,
    key,
    module,
    override,
    per_mp,
    product,
    registry,
    registry_scope,
    reset_registry,
    scalar,
    series,
    table,
    timeline,
)
from .builder import (
    BuildResult,
    build_product,
    check_build,
    engine_available,
    engine_check,
    import_models,
    write_build,
)
from .cli import build_command
from .diagnostics import Diagnostic, DslError, Edit, Span, Suggestion
from .emit import EmitOptions, Emitted, emit
from .ir import AGGREGATES, BUILTINS, Expr, to_json, to_pir
from .spanmap import ComponentSpans, PySpan, SpanIndex, SpanMap, build_span_map
from .proxy import ExprProxy, TableProxy, TraceContext, is_traced, t
from .timing import END, MID, POINT, START, Timing
from .tracer import Param, TraceResult, dependencies, is_stage2, trace, trace_init

__version__ = "0.1.0"

__all__ = [
    "Diagnostic",
    "DslError",
    "Edit",
    "Span",
    "Suggestion",
    "Expr",
    "BUILTINS",
    "AGGREGATES",
    "to_json",
    "to_pir",
    "ExprProxy",
    "TableProxy",
    "TraceContext",
    "is_traced",
    "t",
    "Timing",
    "START",
    "END",
    "MID",
    "POINT",
    "Param",
    "TraceResult",
    "trace",
    "trace_init",
    "dependencies",
    "is_stage2",
    # declarations (T20)
    "series",
    "per_mp",
    "scalar",
    "Declaration",
    "ModelPoint",
    "ModelPointField",
    "Enum",
    "key",
    "assumption",
    "Assumption",
    "table",
    "TableDecl",
    "Key",
    "Value",
    "default",
    "timeline",
    "Timeline",
    "product",
    "Product",
    "Library",
    "module",
    "extends",
    "override",
    "abstract",
    "final",
    "build",
    "BuiltComponent",
    "BuiltModel",
    "Registry",
    "registry",
    "reset_registry",
    "registry_scope",
    # build (T21)
    "emit",
    "EmitOptions",
    "Emitted",
    "build_product",
    "BuildResult",
    "write_build",
    "check_build",
    "engine_check",
    "engine_available",
    "import_models",
    "build_command",
    "build_span_map",
    "SpanMap",
    "SpanIndex",
    "ComponentSpans",
    "PySpan",
    "__version__",
]
