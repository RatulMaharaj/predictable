"""The declaration layer (`02-dsl.md` §2–§7): decorators, schema, tables, timeline, libraries.

The tracer (:mod:`predictable.tracer`) turns one function body into one IR expression. This module
is everything around it: what a component *is*, what its dependencies are, where its dtype and unit
come from, and how a product-team module specialises a company library without forking it.

Two rules shape the whole design.

**Dependencies are the parameter list** (§2.2). A component reads exactly what its signature names.
A free variable that resolves to another declaration is `E1103`, not a convenience — because the
dependency set is what the model explorer, `explain()` and the impact analysis all read, and a set
you have to parse a body to discover is a set no reviewer has.

**Nothing is inferred.** `timing` is required (`E1102`), the return annotation carries dtype and
unit (`E1101`), a redefinition of an inherited name needs `@override` (`E1501`), and an override
that changes shape/dtype/unit/timing is `E1502`. Every one of those is a decision an actuary has to
make anyway; the DSL only insists that it be written down.

Declaration order is preserved: every declaration takes a monotonically increasing
``declaration_index``, which is the IR's tiebreak for deterministic topological order
(`01-ir.md` §3.2), so source order in Python survives into evaluation order.

Emission of `.pir` text is task T21; :func:`build` stops at a fully resolved, fully traced model —
:class:`BuiltComponent` carries exactly the fields of `01-ir.md` §2.1 and nothing else.
"""

from __future__ import annotations

import inspect
import itertools
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable, Iterator, Sequence, get_type_hints

from . import decl_errors as derr
from .diagnostics import Diagnostic, Span
from .ir import Expr, Ref, lit_of
from .proxy import TableProxy
from .timing import Timing
from .tracer import Param, trace, trace_init
from .units import Unit, annotation_type

__all__ = [
    "Declaration",
    "ModelPointField",
    "Assumption",
    "TableDecl",
    "Key",
    "Value",
    "default",
    "Enum",
    "EnumDecl",
    "ModelPoint",
    "Timeline",
    "Product",
    "Library",
    "Registry",
    "BuiltComponent",
    "BuiltModel",
    "TIMELINE_INPUTS",
    "series",
    "per_mp",
    "scalar",
    "key",
    "assumption",
    "table",
    "timeline",
    "product",
    "module",
    "extends",
    "override",
    "abstract",
    "final",
    "build",
    "registry",
    "reset_registry",
    "registry_scope",
]


# --------------------------------------------------------------------------------------
# timeline inputs — always available, never redeclared (`01-ir.md` §5)
# --------------------------------------------------------------------------------------

TIMELINE_INPUTS: dict[str, tuple[str, str]] = {
    "t": ("i64", "none"),
    "policy_year": ("i64", "years"),
    "policy_month": ("i64", "months"),
    "period_start_date": ("date", "none"),
    "period_end_date": ("date", "none"),
    "year_frac": ("f64", "years"),
    "month_of_year": ("i64", "none"),
    "is_anniversary": ("bool", "none"),
}

_SHAPES = ("Scalar", "PerMP", "Series")
_MISSING = object()
_counter = itertools.count()


def _next_index() -> int:
    return next(_counter)


def _caller_module(depth: int = 2) -> str:
    """The ``__name__`` of the module that called us — a declaration's home."""
    frame = inspect.currentframe()
    for _ in range(depth):
        if frame is None:  # pragma: no cover - defensive
            return "__main__"
        frame = frame.f_back
    if frame is None:  # pragma: no cover - defensive
        return "__main__"
    return frame.f_globals.get("__name__", "__main__")


def _caller_span(depth: int = 2, needle: str | None = None, label: str | None = None) -> Span:
    frame = inspect.currentframe()
    for _ in range(depth):
        if frame is None:  # pragma: no cover - defensive
            return derr.span_at("<unknown>", 0)
        frame = frame.f_back
    if frame is None:  # pragma: no cover - defensive
        return derr.span_at("<unknown>", 0)
    return derr.span_at(frame.f_code.co_filename, frame.f_lineno, needle=needle, label=label)


# --------------------------------------------------------------------------------------
# declarations
# --------------------------------------------------------------------------------------


@dataclass
class Declaration:
    """A component declaration: the decorator's arguments plus the untraced body.

    The body is *not* traced here. Tracing needs every parameter resolved, and a parameter may
    name a component declared later in the file or in another module, so it happens in
    :func:`build` once collection is complete (`02-dsl.md` §8 steps 1–2).
    """

    name: str
    shape: str
    func: Callable[..., Any] | None
    module: str
    index: int
    timing: str | None = None
    output: bool = False
    unit: str | None = None
    dtype: str | None = None
    doc: str | None = None
    tags: tuple[str, ...] = ()
    init: Any = None
    is_abstract: bool = False
    is_final: bool = False
    override_of: "Declaration | None" = None
    inherited_from: str | None = None
    generated_from: dict[str, Any] | None = None
    authored_by: str = "dsl"

    @property
    def kind(self) -> str:
        return "Output" if self.output else "Derived"

    @property
    def contract(self) -> tuple[str, str | None, str | None, str | None]:
        """What an `@override` must preserve: shape, dtype, unit, timing (`02-dsl.md` §7.2)."""
        return (self.shape, self.dtype, self.unit, self.timing)

    @property
    def span(self) -> Span:
        return derr.span_of(self.func, needle=self.name)

    def __repr__(self) -> str:
        return f"<{self.shape} {self.module}.{self.name}>"


@dataclass(frozen=True)
class ModelPointField:
    name: str
    dtype: str
    unit: str
    required: bool
    key: bool = False
    default: Any = None
    enum: str | None = None
    schema: str = ""
    span: Span | None = None


@dataclass(frozen=True)
class Assumption:
    name: str
    dtype: str
    unit: str
    shape: str = "Scalar"
    default: Any = None
    doc: str | None = None
    index: int = 0
    span: Span | None = None


@dataclass(frozen=True)
class EnumDecl:
    name: str
    values: tuple[str, ...]


@dataclass(frozen=True)
class Key:
    """One table key column: its name, its dtype, and its out-of-range policy (`01-ir.md` §2.9)."""

    name: str
    dtype: Any
    policy: str = "exact"


@dataclass(frozen=True)
class Value:
    name: str
    dtype: Any


class _Default:
    """``on_missing=default(0.0)`` — the IR spelling is ``default(<lit>)``."""

    __slots__ = ("value",)

    def __init__(self, value: Any) -> None:
        self.value = value

    def __str__(self) -> str:
        return f"default({lit_of(self.value).to_pir()})"


def default(value: Any) -> _Default:
    """The `on_missing` policy that substitutes a literal instead of failing."""
    return _Default(value)


class TableDecl(TableProxy):
    """A declared table. It is a :class:`~predictable.proxy.TableProxy`, so calling it is a lookup.

    Passed into a body through the ``tbl=tbl`` default-argument idiom, which keeps the
    "dependencies are parameters" rule intact for tables too and makes a table swap a
    signature-level change (`02-dsl.md` §6).
    """

    __slots__ = ("keys", "value_specs", "source", "on_missing", "module", "index", "decl_span")

    def __init__(
        self,
        name: str,
        keys: Sequence[Key],
        values: Sequence[Value],
        source: str,
        on_missing: Any,
        module: str,
        index: int,
        span: Span | None = None,
    ) -> None:
        super().__init__(name, tuple(v.name for v in values))
        self.keys = tuple(keys)
        self.value_specs = tuple(values)
        self.source = source
        self.on_missing = on_missing
        self.module = module
        self.index = index
        self.decl_span = span

    def to_ir(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "keys": [
                {"name": k.name, "dtype": _dtype_of(k.dtype), "policy": k.policy} for k in self.keys
            ],
            "values": [
                {"name": v.name, "dtype": _dtype_of(v.dtype), "unit": _unit_of(v.dtype)}
                for v in self.value_specs
            ],
            "on_missing": str(self.on_missing),
            "source": self.source,
        }


@dataclass(frozen=True)
class Timeline:
    basis: str
    periods: int
    origin: str
    valuation_date: Any = None
    year_convention: str = "act/365"
    module: str = ""

    def to_ir(self) -> dict[str, Any]:
        out = {
            "basis": self.basis,
            "periods": self.periods,
            "origin": self.origin,
            "year_convention": self.year_convention,
        }
        if self.valuation_date is not None:
            out["valuation_date"] = str(self.valuation_date)
        return out


@dataclass(frozen=True)
class Product:
    name: str
    modules: tuple[str, ...]
    outputs: tuple[str, ...]
    key_field: str | None = None
    assumptions: str | None = None
    doc: str | None = None
    span: Span | None = None

    def to_ir(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "name": self.name,
            "modules": list(self.modules),
            "outputs": list(self.outputs),
        }
        if self.key_field:
            out["key_field"] = self.key_field
        if self.assumptions:
            out["assumptions"] = self.assumptions
        if self.doc:
            out["doc"] = self.doc
        return out


@dataclass(frozen=True)
class Library:
    """A distributable set of modules (`02-dsl.md` §7.1), pinned by semver and recorded in a run."""

    name: str
    version: str
    modules: tuple[str, ...]

    def __init__(self, *, name: str, version: str, modules: Sequence[str]) -> None:
        object.__setattr__(self, "name", name)
        object.__setattr__(self, "version", version)
        object.__setattr__(self, "modules", tuple(modules))


# --------------------------------------------------------------------------------------
# the registry
# --------------------------------------------------------------------------------------


@dataclass
class ModuleRegistry:
    path: str
    components: dict[str, Declaration] = field(default_factory=dict)
    inherited: dict[str, tuple[Declaration, str]] = field(default_factory=dict)
    extends: list[str] = field(default_factory=list)


class Registry:
    """Every declaration made in this process, in declaration order.

    A single global instance is the normal case — declarations are made by importing modules, and a
    process imports a model once. Tests use :func:`registry_scope` for isolation.
    """

    def __init__(self) -> None:
        self.modules: dict[str, ModuleRegistry] = {}
        self.fields: dict[str, ModelPointField] = {}
        self.assumptions: dict[str, Assumption] = {}
        self.tables: dict[str, TableDecl] = {}
        self.enums: dict[str, EnumDecl] = {}
        self.timeline: Timeline | None = None
        self.products: dict[str, Product] = {}
        self.aliases: dict[str, str] = {}

    # -- module bookkeeping ---------------------------------------------------------

    def module(self, path: str) -> ModuleRegistry:
        reg = self.modules.get(path)
        if reg is None:
            reg = ModuleRegistry(path)
            self.modules[path] = reg
        return reg

    def add_component(self, decl: Declaration) -> None:
        if decl.name in TIMELINE_INPUTS:
            derr.raise_e1401_timeline_redefinition(decl.name, decl.span)
        self.module(decl.module).components[decl.name] = decl

    def components_of(self, path: str) -> dict[str, Declaration]:
        """A module's visible components: its own, plus inherited names it has not overridden."""
        reg = self.module(path)
        out: dict[str, Declaration] = {}
        for name, (decl, _source) in reg.inherited.items():
            out[name] = decl
        out.update(reg.components)
        return out

    def all_component_names(self) -> list[str]:
        return sorted({name for m in self.modules.values() for name in m.components})

    def resolved_module(self, path: str) -> str:
        return self.aliases.get(path, path)


_REGISTRY = Registry()


def registry() -> Registry:
    """The active registry."""
    return _REGISTRY


def reset_registry() -> None:
    """Drop every declaration. Used by tests and by a rebuild in a long-lived process."""
    global _REGISTRY
    _REGISTRY = Registry()


@contextmanager
def registry_scope() -> Iterator[Registry]:
    """A fresh registry for the duration of the block, restored afterwards."""
    global _REGISTRY
    previous = _REGISTRY
    _REGISTRY = Registry()
    try:
        yield _REGISTRY
    finally:
        _REGISTRY = previous


def module(name: str) -> None:
    """Give the calling Python module an explicit IR module path (`01-ir.md` §2.2)."""
    _REGISTRY.aliases[_caller_module()] = name


# --------------------------------------------------------------------------------------
# §3 — modelpoint schema and enums
# --------------------------------------------------------------------------------------


class _EnumMeta(type):
    def __new__(mcls, name, bases, namespace, **kwargs):
        cls = super().__new__(mcls, name, bases, namespace)
        if not bases:
            return cls
        values = tuple(v for k, v in namespace.items() if not k.startswith("_") and isinstance(v, str))
        members = {}
        for member_name, value in list(namespace.items()):
            if member_name.startswith("_") or not isinstance(value, str):
                continue
            member = _EnumMember(name, member_name, value)
            setattr(cls, member_name, member)
            members[member_name] = member
        cls._predictable_enum = name
        cls._members = members
        _REGISTRY.enums[name] = EnumDecl(name, values)
        return cls


class _EnumMember:
    """One enum value. Carries its IR spelling so ``gender == Gender.F`` traces to a `str` literal."""

    __slots__ = ("enum", "name", "_predictable_tag")

    def __init__(self, enum: str, name: str, value: str) -> None:
        self.enum = enum
        self.name = name
        self._predictable_tag = value

    @property
    def value(self) -> str:
        return self._predictable_tag

    def __eq__(self, other: object) -> bool:
        if isinstance(other, _EnumMember):
            return (self.enum, self._predictable_tag) == (other.enum, other._predictable_tag)
        return NotImplemented

    def __hash__(self) -> int:
        return hash((self.enum, self._predictable_tag))

    def __repr__(self) -> str:
        return f"<{self.enum}.{self.name}>"


class Enum(metaclass=_EnumMeta):
    """A model enum (`01-ir.md` §2.9). Unordered — members compare by equality, never by ordinal.

    .. code-block:: python

        class Gender(Enum):
            M = "M"
            F = "F"
    """


class _Key:
    """The sentinel returned by :func:`key`, marking the modelpoint identity column."""

    __slots__ = ()


def key() -> Any:
    """Mark the modelpoint's identity column. Exactly one per schema (`E1301`)."""
    return _Key()


class _ModelPointMeta(type):
    def __new__(mcls, name, bases, namespace, **kwargs):
        cls = super().__new__(mcls, name, bases, namespace)
        if not bases:
            return cls
        annotations = _hints_of_class(cls, namespace)
        span = _caller_span(depth=2, needle=name)
        keys: list[str] = []
        fields: list[ModelPointField] = []
        for fname, annotation in annotations.items():
            resolved = annotation_type(annotation)
            if resolved is None:
                dtype, unit = "f64", "none"
            else:
                dtype, unit = resolved
            raw_default = namespace.get(fname, _MISSING)
            is_key = isinstance(raw_default, _Key)
            if is_key:
                keys.append(fname)
            required = is_key or raw_default is _MISSING
            fields.append(
                ModelPointField(
                    name=fname,
                    dtype=dtype,
                    unit=unit,
                    required=required,
                    key=is_key,
                    default=None if required else raw_default,
                    enum=dtype[5:-1] if dtype.startswith("enum(") else None,
                    schema=name,
                    span=span,
                )
            )
        if len(keys) != 1:
            derr.raise_e1301_key_fields(name, keys, span.file, span.start_pos.line if span.start_pos else 0)
        for f in fields:
            if f.name in TIMELINE_INPUTS:
                derr.raise_e1401_timeline_redefinition(f.name, span)
            _REGISTRY.fields[f.name] = f
        cls._predictable_fields = tuple(fields)
        return cls

    def __getattr__(cls, attribute: str) -> Any:
        if attribute.startswith("_"):
            raise AttributeError(attribute)
        from . import errors

        errors.raise_e1302_modelpoint_object(cls.__name__, attribute)
        raise AssertionError("unreachable")  # pragma: no cover


class ModelPoint(metaclass=_ModelPointMeta):
    """A modelpoint schema (`02-dsl.md` §3). A schema only: never instantiated, never a value.

    Components take individual fields as parameters; touching the class as if it were a record is
    `E1302`.
    """


def _hints_of_class(cls: type, namespace: dict[str, Any]) -> dict[str, Any]:
    try:
        return get_type_hints(cls, include_extras=True)
    except Exception:  # pragma: no cover - unresolvable annotation, fall back to raw
        return dict(namespace.get("__annotations__", {}))


# --------------------------------------------------------------------------------------
# §4 — assumptions
# --------------------------------------------------------------------------------------


def assumption(
    annotation: Any,
    *,
    shape: str = "Scalar",
    default: Any = None,
    doc: str | None = None,
) -> Assumption:
    """Declare an assumption: a run input whose *values* live in an assumption set, not in Python.

    .. code-block:: python

        valuation_rate = assumption(Rate.annual)
        mortality_loading = assumption(Factor, default=1.0)

    ``shape`` is ``"Scalar"`` (the default), ``"PerMP"`` for a per-policy input or ``"Series"`` for
    a term structure. Values are separate because a scenario run varies them thousands of times
    while the model is fixed (`02-dsl.md` §9).
    """
    resolved = annotation_type(annotation)
    if resolved is None:
        raise TypeError(
            "assumption() needs a unit-carrying annotation, e.g. Rate.annual, Money, Factor"
        )
    dtype, unit = resolved
    if shape not in _SHAPES:
        raise ValueError(f"shape must be one of {_SHAPES}, got {shape!r}")
    span = _caller_span(depth=2)
    name = _binding_name(span)
    decl = Assumption(
        name=name or "<unnamed>",
        dtype=dtype,
        unit=unit,
        shape=shape,
        default=default,
        doc=doc,
        index=_next_index(),
        span=span,
    )
    if decl.name in TIMELINE_INPUTS:
        derr.raise_e1401_timeline_redefinition(decl.name, span)
    _REGISTRY.assumptions[decl.name] = decl
    return decl


def _binding_name(span: Span) -> str | None:
    """The name on the left of ``=`` on the declaring line — an assumption names itself."""
    text = span.text
    if "=" not in text:
        return None
    lhs = text.split("=", 1)[0].strip()
    return lhs if lhs.isidentifier() else None


# --------------------------------------------------------------------------------------
# §5 — timeline
# --------------------------------------------------------------------------------------


def timeline(
    *,
    basis: str,
    periods: int,
    origin: str = "policy",
    valuation_date: Any = None,
    year_convention: str = "act/365",
) -> Timeline:
    """Declare the projection timeline. One call per model (`01-ir.md` §5)."""
    if basis not in ("annual", "monthly", "quarterly"):
        raise ValueError(f"basis must be annual, monthly or quarterly, got {basis!r}")
    if origin not in ("policy", "valuation"):
        raise ValueError(f"origin must be policy or valuation, got {origin!r}")
    if periods <= 0:
        raise ValueError("periods must be positive")
    decl = Timeline(
        basis=basis,
        periods=periods,
        origin=origin,
        valuation_date=valuation_date,
        year_convention=year_convention,
        module=_caller_module(),
    )
    _REGISTRY.timeline = decl
    return decl


# --------------------------------------------------------------------------------------
# §6 — tables
# --------------------------------------------------------------------------------------


def table(
    *,
    source: str,
    keys: Sequence[Key],
    values: Sequence[Value],
    on_missing: Any = "error",
    name: str | None = None,
) -> TableDecl:
    """Declare a table input. ``source`` resolves relative to the declaring module (`01-ir.md` §2.9.1)."""
    span = _caller_span(depth=2)
    table_name = name or _binding_name(span) or "<unnamed>"
    for k in keys:
        if _dtype_of(k.dtype).startswith("enum(") and k.policy != "exact":
            raise ValueError(
                f"enum key `{k.name}` supports policy='exact' only — enums are unordered "
                "(`01-ir.md` §2.9); clamp/step/interpolate on an enum key is E0305"
            )
    decl = TableDecl(
        name=table_name,
        keys=keys,
        values=values,
        source=source,
        on_missing=on_missing,
        module=_caller_module(),
        index=_next_index(),
        span=span,
    )
    _REGISTRY.tables[table_name] = decl
    return decl


def _dtype_of(annotation: Any) -> str:
    resolved = annotation_type(annotation)
    return resolved[0] if resolved else "f64"


def _unit_of(annotation: Any) -> str:
    resolved = annotation_type(annotation)
    return resolved[1] if resolved else "none"


# --------------------------------------------------------------------------------------
# §2.1 — the three shape decorators
# --------------------------------------------------------------------------------------


def _declare(
    shape: str,
    *,
    timing: Any = None,
    init: Any = None,
    output: bool = False,
    unit: Any = None,
    dtype: str | None = None,
    doc: str | None = None,
    tags: Sequence[str] = (),
    name_suffix: str | None = None,
    module_path: str | None = None,
) -> Callable[[Callable[..., Any]], Declaration]:
    timing_tag: str | None
    if shape == "Series":
        if timing is None:
            derr.raise_e1102_missing_timing()
        timing_tag = timing.tag if isinstance(timing, Timing) else str(timing)
    else:
        if timing is not None:
            raise TypeError(f"@{'per_mp' if shape == 'PerMP' else 'scalar'} has no timing")
        timing_tag = None
    home = module_path or _caller_module(depth=3)

    def decorate(func: Callable[..., Any]) -> Declaration:
        name = func.__name__
        generated = None
        if name_suffix is not None:
            name = f"{name}_{name_suffix}"
            code = func.__code__
            generated = {
                "file": code.co_filename,
                "line": code.co_firstlineno,
                "iteration": str(name_suffix),
            }
        hints = _hints_of_func(func)
        return_annotation = hints.get("return")
        resolved = annotation_type(return_annotation)
        if resolved is None and (dtype is None or unit is None):
            derr.raise_e1101_missing_return_annotation(name, func)
        base_dtype, base_unit = resolved if resolved else (dtype or "f64", "none")
        decl = Declaration(
            name=name,
            shape=shape,
            func=func,
            module=home,
            index=_next_index(),
            timing=timing_tag,
            output=output,
            unit=str(unit) if unit is not None else base_unit,
            dtype=dtype or base_dtype,
            doc=doc if doc is not None else _docstring(func),
            tags=tuple(tags),
            init=init,
            generated_from=generated,
        )
        _REGISTRY.add_component(decl)
        return decl

    return decorate


def series(
    *,
    timing: Timing | None = None,
    init: Any = None,
    output: bool = False,
    unit: Unit | None = None,
    dtype: str | None = None,
    doc: str | None = None,
    tags: Sequence[str] = (),
    name_suffix: str | None = None,
) -> Callable[[Callable[..., Any]], Declaration]:
    """A value per modelpoint per period. ``timing`` is required — there is no default (§2.5)."""
    return _declare(
        "Series",
        timing=timing,
        init=init,
        output=output,
        unit=unit,
        dtype=dtype,
        doc=doc,
        tags=tags,
        name_suffix=name_suffix,
        module_path=_caller_module(),
    )


def per_mp(
    *,
    output: bool = False,
    unit: Unit | None = None,
    dtype: str | None = None,
    doc: str | None = None,
    tags: Sequence[str] = (),
    name_suffix: str | None = None,
) -> Callable[[Callable[..., Any]], Declaration]:
    """One value per modelpoint. Reaching it from a `Series` needs an explicit aggregate."""
    return _declare(
        "PerMP",
        output=output,
        unit=unit,
        dtype=dtype,
        doc=doc,
        tags=tags,
        name_suffix=name_suffix,
        module_path=_caller_module(),
    )


def scalar(
    *,
    output: bool = False,
    unit: Unit | None = None,
    dtype: str | None = None,
    doc: str | None = None,
    tags: Sequence[str] = (),
    name_suffix: str | None = None,
) -> Callable[[Callable[..., Any]], Declaration]:
    """One value for the whole run."""
    return _declare(
        "Scalar",
        output=output,
        unit=unit,
        dtype=dtype,
        doc=doc,
        tags=tags,
        name_suffix=name_suffix,
        module_path=_caller_module(),
    )


def _docstring(func: Callable[..., Any]) -> str | None:
    """The docstring as `doc`: first line only when a blank line follows, else the whole thing."""
    raw = inspect.getdoc(func)
    if not raw:
        return None
    parts = raw.split("\n\n", 1)
    return parts[0].strip() if len(parts) > 1 else raw.strip()


def _hints_of_func(func: Callable[..., Any]) -> dict[str, Any]:
    try:
        return get_type_hints(func, include_extras=True)
    except Exception:  # pragma: no cover - unresolvable annotation
        return dict(getattr(func, "__annotations__", {}))


# --------------------------------------------------------------------------------------
# §7.2 — the library layer
# --------------------------------------------------------------------------------------


def extends(source: Any) -> None:
    """Inherit every component of ``source`` into the calling module.

    Copies, not links: the built IR is fully self-contained, so the engine never resolves
    inheritance and `explain()` never shows a virtual dispatch (`02-dsl.md` §7.2).
    """
    here = _caller_module()
    reg = _REGISTRY.module(here)
    for path in _module_paths(source):
        reg.extends.append(path)
        for name, decl in _REGISTRY.components_of(path).items():
            reg.inherited[name] = (decl, path)


def _module_paths(source: Any) -> list[str]:
    if isinstance(source, Library):
        return list(source.modules)
    if isinstance(source, str):
        return [source]
    name = getattr(source, "__name__", None)
    if name is None:
        raise TypeError(f"extends() takes a module, a Library or a module path, got {source!r}")
    return [name]


def override(base: Declaration) -> Callable[[Declaration], Declaration]:
    """Replace an inherited component. The contract — shape, dtype, unit, timing — must match."""
    if not isinstance(base, Declaration):
        raise TypeError("@override takes the component it replaces, e.g. @override(acme.qx_loading)")

    def decorate(decl: Declaration) -> Declaration:
        if base.is_final:
            derr.raise_e1504_final_override(base.name, decl.span, base.module)
        differences = [
            (label, str(b), str(n))
            for label, b, n in zip(
                ("shape", "dtype", "unit", "timing"), base.contract, decl.contract
            )
            if b != n
        ]
        if differences:
            derr.raise_e1502_override_signature(base.name, decl.span, differences)
        decl.override_of = base
        if f"override:{base.module}.{base.name}" not in decl.tags:
            decl.tags = decl.tags + (f"override:{base.module}.{base.name}",)
        return decl

    return decorate


def abstract(decl: Declaration) -> Declaration:
    """Declare a component with a signature and no body. A product must override it (`E1203`)."""
    decl.is_abstract = True
    return decl


def final(decl: Declaration) -> Declaration:
    """Forbid override. Reserved for regulatory or group-standard components (`E1504`)."""
    decl.is_final = True
    return decl


# --------------------------------------------------------------------------------------
# §11 — the product entry point
# --------------------------------------------------------------------------------------


def product(
    name: str,
    *,
    modules: Sequence[str],
    outputs: Sequence[str],
    key_field: str | None = None,
    assumptions: str | None = None,
    doc: str | None = None,
) -> Product:
    """Declare the product entry point: which modules constitute the model, and what it promises.

    ``outputs`` is a checked manifest, not a selector — it must equal the set of components
    carrying ``output=True``, or `E1601` (`01-ir.md` §13 Q2).
    """
    decl = Product(
        name=name,
        modules=tuple(modules),
        outputs=tuple(outputs),
        key_field=key_field,
        assumptions=assumptions,
        doc=doc,
        span=_caller_span(depth=2, needle="product"),
    )
    _REGISTRY.products[name] = decl
    return decl


# --------------------------------------------------------------------------------------
# build — resolve, trace, check
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class BuiltComponent:
    """One resolved, traced component — exactly the fields of `01-ir.md` §2.1."""

    name: str
    kind: str
    dtype: str
    shape: str
    unit: str
    timing: str | None
    expr: Expr | None
    init: Expr | None
    module: str
    index: int
    dependencies: tuple[str, ...]
    tables: tuple[str, ...]
    stage: int
    doc: str | None = None
    tags: tuple[str, ...] = ()
    meta: dict[str, Any] = field(default_factory=dict)

    @property
    def qualified_name(self) -> str:
        return f"{self.module}.{self.name}"

    def to_ir(self) -> dict[str, Any]:
        """The component as IR data, in `01-ir.md` §2.1 key order. Rendering to text is T21."""
        out: dict[str, Any] = {
            "name": self.name,
            "kind": self.kind,
            "dtype": self.dtype,
            "shape": self.shape,
            "unit": self.unit,
        }
        if self.timing is not None:
            out["timing"] = self.timing
        if self.expr is not None:
            out["expr"] = self.expr.to_pir()
        if self.init is not None:
            out["init"] = self.init.to_pir()
        meta = dict(self.meta)
        if self.doc:
            meta["doc"] = self.doc
        if self.tags:
            meta["tags"] = list(self.tags)
        if meta:
            out["meta"] = meta
        return out


@dataclass(frozen=True)
class BuiltModel:
    """A whole product, resolved and traced: what `predictable build` (T21) renders to `.pir`."""

    components: tuple[BuiltComponent, ...]
    timeline: Timeline | None
    enums: tuple[EnumDecl, ...]
    fields: tuple[ModelPointField, ...]
    assumptions: tuple[Assumption, ...]
    tables: tuple[TableDecl, ...]
    product: Product | None
    warnings: tuple[Diagnostic, ...] = ()

    def component(self, name: str) -> BuiltComponent:
        for c in self.components:
            if c.name == name:
                return c
        raise KeyError(name)

    @property
    def outputs(self) -> tuple[str, ...]:
        return tuple(c.name for c in self.components if c.kind == "Output")

    def to_ir(self) -> dict[str, Any]:
        out: dict[str, Any] = {"format": "pir/1"}
        if self.timeline is not None:
            out["timeline"] = self.timeline.to_ir()
        if self.enums:
            out["enum"] = [{"name": e.name, "values": list(e.values)} for e in self.enums]
        if self.fields:
            out["modelpoint_field"] = [
                {
                    k: v
                    for k, v in (
                        ("name", f.name),
                        ("dtype", f.dtype),
                        ("unit", f.unit if f.unit != "none" else None),
                        ("required", f.required),
                        ("key", True if f.key else None),
                        ("default", f.default if not f.required else None),
                    )
                    if v is not None
                }
                for f in self.fields
            ]
        if self.assumptions:
            out["assumption"] = [
                {"name": a.name, "dtype": a.dtype, "unit": a.unit, "shape": a.shape}
                for a in self.assumptions
            ]
        if self.tables:
            out["table"] = [t.to_ir() for t in self.tables]
        out["component"] = [c.to_ir() for c in self.components]
        if self.product is not None:
            out["product"] = self.product.to_ir()
        return out


_FN_NAMES: frozenset[str] = frozenset()


def _fn_names() -> frozenset[str]:
    global _FN_NAMES
    if not _FN_NAMES:
        from . import fn

        _FN_NAMES = frozenset(dir(fn)) | {"t", "when"}
    return _FN_NAMES


def build(product_name: str | Product | None = None, *, reg: Registry | None = None) -> BuiltModel:
    """Resolve every declaration, trace every body, and run the declaration-level checks.

    This is steps 1–3 of `02-dsl.md` §8 — collect, trace, resolve-and-check. Emission (step 4) and
    the engine's own checker (step 5) are task T21; what comes back here is a fully resolved model
    with no unresolved names, no untraced bodies and no undischarged abstracts.
    """
    r = reg or _REGISTRY
    prod = product_name if isinstance(product_name, Product) else None
    if prod is None and product_name is not None:
        prod = r.products[product_name]
    if prod is None and len(r.products) == 1:
        prod = next(iter(r.products.values()))

    module_order = _module_order(r, prod)
    declarations = _collect(r, module_order)
    _check_abstracts(r, prod, declarations, module_order)

    concrete = [d for d in declarations if not d.is_abstract]
    stage2 = _stage2_map(r, concrete, module_order)
    built = [_build_one(r, d, module_order, stage2) for d in concrete]

    if prod is not None:
        _check_output_manifest(prod, built)

    return BuiltModel(
        components=tuple(built),
        timeline=r.timeline,
        enums=tuple(r.enums.values()),
        fields=tuple(r.fields.values()),
        assumptions=tuple(sorted(r.assumptions.values(), key=lambda a: a.index)),
        tables=tuple(sorted(r.tables.values(), key=lambda t: t.index)),
        product=prod,
        warnings=tuple(_lints(built, concrete)),
    )


def _module_order(r: Registry, prod: Product | None) -> list[str]:
    if prod is not None:
        return [p for p in prod.modules]
    return sorted(
        r.modules,
        key=lambda p: min((d.index for d in r.modules[p].components.values()), default=1 << 30),
    )


def _collect(r: Registry, module_order: Sequence[str]) -> list[Declaration]:
    """Every visible component across the build, in declaration order, with `E1501` enforced."""
    out: list[Declaration] = []
    seen: dict[str, Declaration] = {}
    # A base component that some module overrides is *replaced*, not duplicated: the built IR is
    # self-contained and contains exactly one declaration per name (`02-dsl.md` §7.2).
    replaced = {
        id(decl.override_of)
        for path in module_order
        for decl in r.module(path).components.values()
        if decl.override_of is not None
    }
    for path in module_order:
        reg = r.module(path)
        for name, decl in reg.components.items():
            inherited = reg.inherited.get(name)
            if inherited is not None and decl.override_of is None:
                derr.raise_e1501_shadows_inherited(name, decl.span, inherited[1])
            if decl.override_of is not None and decl.override_of.is_final:
                derr.raise_e1504_final_override(name, decl.span, decl.override_of.module)
        for name, decl in r.components_of(path).items():
            if id(decl) in replaced:
                continue
            if name in seen and seen[name] is not decl:
                # An inherited component reached through two modules is the same declaration
                # object; two *different* declarations of one name is a cross-module clash.
                if seen[name].module != decl.module:
                    derr.raise_e1105_ambiguous_name(
                        name, name, decl.func or seen[name].func, [seen[name].module, decl.module]
                    )
            if name not in seen:
                seen[name] = decl
                out.append(decl)
    out.sort(key=lambda d: d.index)
    return out


def _check_abstracts(
    r: Registry, prod: Product | None, declarations: Sequence[Declaration], module_order: Sequence[str]
) -> None:
    overridden = {
        d.override_of.name for d in declarations if d.override_of is not None
    }
    missing = []
    for decl in declarations:
        if not decl.is_abstract or decl.name in overridden:
            continue
        signature = _signature_text(decl)
        decorator = _decorator_text(decl)
        where = f"{decl.module}:{decl.func.__code__.co_firstlineno if decl.func else 0}"
        missing.append((signature, decorator, where))
    if missing:
        span = (
            prod.span
            if prod is not None and prod.span is not None
            else (declarations[0].span if declarations else derr.span_at("<unknown>", 0))
        )
        derr.raise_e1203_unimplemented_abstract(prod.name if prod else "<unnamed>", missing, span)


def _signature_text(decl: Declaration) -> str:
    if decl.func is None:  # pragma: no cover - defensive
        return decl.name
    hints = _hints_of_func(decl.func)
    params = []
    for name in inspect.signature(decl.func).parameters:
        annotation = hints.get(name)
        resolved = annotation_type(annotation)
        params.append(f"{name}: {resolved[1] if resolved else '?'}")
    return f"{decl.name}({', '.join(params)}) -> {decl.unit}"


def _decorator_text(decl: Declaration) -> str:
    if decl.shape == "Series":
        return f"@series(timing={decl.timing.upper() if decl.timing else '?'})"
    return "@per_mp()" if decl.shape == "PerMP" else "@scalar()"


def _stage2_map(
    r: Registry, declarations: Sequence[Declaration], module_order: Sequence[str]
) -> dict[str, bool]:
    """Which components carry a stage-2 (`Agg`-derived) value.

    A component's own *stage* is local — 2 iff its expression contains an `Agg` (`01-ir.md` §2.2) —
    but `E1207` is about reachability: a value that only exists once the projection has finished
    stays stage-2 however many `PerMP` components it passes through. So this is the local property
    closed over the dependency graph, computed from a first trace in which nothing is marked, so
    that pass cannot raise `E1207` itself.
    """
    own: dict[str, bool] = {}
    deps: dict[str, tuple[str, ...]] = {}
    for decl in declarations:
        params = _resolve_params(r, decl, module_order, {})
        result = trace(decl.func, params, component=decl.name)
        own[decl.name] = result.stage2
        deps[decl.name] = result.dependencies
    changed = True
    while changed:
        changed = False
        for name, flag in own.items():
            if not flag and any(own.get(d, False) for d in deps[name]):
                own[name] = True
                changed = True
    return own


def _build_one(
    r: Registry,
    decl: Declaration,
    module_order: Sequence[str],
    stage2: dict[str, bool],
) -> BuiltComponent:
    params = _resolve_params(r, decl, module_order, stage2)
    result = trace(decl.func, params, component=decl.name)
    _check_tables(decl, params, result.tables)

    init_expr: Expr | None = None
    init_deps: tuple[str, ...] = ()
    if decl.init is not None:
        init_expr, init_deps = _resolve_init(r, decl, module_order, stage2)

    meta: dict[str, Any] = {"authored_by": decl.authored_by}
    code = decl.func.__code__ if decl.func else None
    if code is not None:
        meta["origin_span"] = {
            "file": code.co_filename,
            "line": code.co_firstlineno,
            "col_start": 0,
            "col_end": 0,
        }
    if decl.override_of is not None:
        meta["overrides"] = {
            "library": decl.override_of.module,
            "component": decl.override_of.name,
        }
    if decl.generated_from is not None:
        meta["generated_from"] = decl.generated_from

    deps = tuple(dict.fromkeys(result.dependencies + init_deps))
    return BuiltComponent(
        name=decl.name,
        kind=decl.kind,
        dtype=decl.dtype or "f64",
        shape=decl.shape,
        unit=decl.unit or "none",
        timing=decl.timing,
        expr=result.expr,
        init=init_expr,
        module=decl.module,
        index=decl.index,
        dependencies=deps,
        tables=result.tables,
        stage=2 if result.stage2 else 1,
        doc=decl.doc,
        tags=decl.tags,
        meta=meta,
    )


def _resolve_init(
    r: Registry, decl: Declaration, module_order: Sequence[str], stage2: dict[str, bool]
) -> tuple[Expr, tuple[str, ...]]:
    init = decl.init
    if isinstance(init, Declaration):
        return Ref(init.name), (init.name,)
    if isinstance(init, (bool, int, float, str)):
        return lit_of(init), ()
    if callable(init):
        lam = Declaration(
            name=decl.name,
            shape=decl.shape,
            func=init,
            module=decl.module,
            index=decl.index,
            timing=decl.timing,
            unit=decl.unit,
            dtype=decl.dtype,
        )
        params = _resolve_params(r, lam, module_order, stage2, in_init=True)
        result = trace_init(init, params, component=decl.name)
        return result.expr, result.dependencies
    raise TypeError(
        f"init= takes a literal, a component reference or a lambda, got {type(init).__name__}"
    )


# --------------------------------------------------------------------------------------
# name resolution (§2.2)
# --------------------------------------------------------------------------------------


def _resolve_params(
    r: Registry,
    decl: Declaration,
    module_order: Sequence[str],
    stage2: dict[str, bool],
    *,
    in_init: bool = False,
) -> dict[str, Param]:
    func = decl.func
    assert func is not None
    signature = inspect.signature(func)
    hints = _hints_of_func(func)
    params: dict[str, Param] = {}
    table_names: list[str] = []
    for name, parameter in signature.parameters.items():
        if isinstance(parameter.default, TableDecl):
            table_names.append(parameter.default.name)
            continue
        if parameter.default is not inspect.Parameter.empty:
            # A plain-Python default is a build-time constant (`02-dsl.md` §7.3) and is folded in.
            continue
        target, where = _resolve_name(r, decl, name, module_order)
        if target is None:
            derr.raise_e1103_unknown_parameter(
                decl.name,
                name,
                func,
                components=sorted(_visible_components(r, decl.module, module_order)),
                fields=sorted(r.fields),
                assumptions=sorted(r.assumptions),
                tables=sorted(r.tables),
                timeline=sorted(TIMELINE_INPUTS),
            )
        annotated = annotation_type(hints.get(name))
        declared = (_dtype_of_target(target), _unit_of_target(target))
        if annotated is not None and annotated != declared:
            derr.raise_e1104_annotation_conflict(
                decl.name,
                name,
                func,
                annotated=annotated,
                declared=declared,
                declared_at=_span_of_target(target),
            )
        params[name] = Param(
            name=name,
            shape=_shape_of_target(target),
            unit=declared[1],
            # A stage-2 value read by a `Series` body is read *at time t*, which is `E1207`.
            # Read by a `PerMP` or `Scalar` body it is legal: that body is stage 2 as well
            # (`01-ir.md` §8.2), which is exactly how `bel` sums three `npv`s.
            stage2=stage2.get(name, False) and decl.shape == "Series",
        )
    _check_free_variables(r, decl, set(signature.parameters), set(table_names), module_order)
    return params


def _resolve_name(
    r: Registry, decl: Declaration, name: str, module_order: Sequence[str]
) -> tuple[Any, str]:
    """First match wins across the namespaces of §2.2; ties within a namespace are `E1105`."""
    own = r.components_of(decl.module).get(name)
    if own is not None:
        return own, "component"
    matches = [
        r.module(path).components[name]
        for path in module_order
        if path != decl.module and name in r.module(path).components
    ]
    unique = {id(m): m for m in matches}
    if len(unique) > 1:
        derr.raise_e1105_ambiguous_name(
            decl.name, name, decl.func, [m.module for m in unique.values()]
        )
    if matches:
        return matches[0], "component"
    if name in r.fields:
        return r.fields[name], "modelpoint field"
    if name in r.assumptions:
        return r.assumptions[name], "assumption"
    if name in TIMELINE_INPUTS:
        return TIMELINE_INPUTS[name], "timeline input"
    if name in r.tables:
        return r.tables[name], "table"
    return None, ""


def _visible_components(r: Registry, home: str, module_order: Sequence[str]) -> Iterable[str]:
    names = set(r.components_of(home))
    for path in module_order:
        names.update(r.module(path).components)
    return names


def _shape_of_target(target: Any) -> str:
    if isinstance(target, Declaration):
        return target.shape
    if isinstance(target, ModelPointField):
        return "PerMP"
    if isinstance(target, Assumption):
        return target.shape
    if isinstance(target, tuple):  # a timeline input
        return "Series"
    return "Series"  # pragma: no cover - defensive


def _dtype_of_target(target: Any) -> str:
    if isinstance(target, tuple):
        return target[0]
    return getattr(target, "dtype", "f64") or "f64"


def _unit_of_target(target: Any) -> str:
    if isinstance(target, tuple):
        return target[1]
    return getattr(target, "unit", "none") or "none"


def _span_of_target(target: Any) -> Span | None:
    if isinstance(target, Declaration):
        return target.span
    return getattr(target, "span", None)


def _check_free_variables(
    r: Registry,
    decl: Declaration,
    parameters: set[str],
    tables_in_signature: set[str],
    module_order: Sequence[str],
) -> None:
    """A body that reads a declaration it did not declare as a parameter is `E1103` (§2.2)."""
    func = decl.func
    assert func is not None
    reachable = set(_visible_components(r, decl.module, module_order)) | set(r.fields) | set(
        r.assumptions
    )
    reachable -= set(TIMELINE_INPUTS)
    reachable -= _fn_names()
    used = set(func.__code__.co_names) | set(func.__code__.co_freevars)
    for name in sorted(used & reachable):
        if name in parameters or name in tables_in_signature or name == decl.name:
            continue
        derr.raise_e1103_free_variable(decl.name, name, func)


def _check_tables(decl: Declaration, params: dict[str, Param], used: Sequence[str]) -> None:
    """`E1106`: the tables in the signature and the tables the body looks up must be the same set."""
    func = decl.func
    assert func is not None
    declared = {
        p.default.name
        for p in inspect.signature(func).parameters.values()
        if isinstance(p.default, TableDecl)
    }
    for name in sorted(set(used) - declared):
        derr.raise_e1106_table_signature(decl.name, name, func, unused=False)
    for name in sorted(declared - set(used)):
        derr.raise_e1106_table_signature(decl.name, name, func, unused=True)


def _check_output_manifest(prod: Product, built: Sequence[BuiltComponent]) -> None:
    actual = {c.name for c in built if c.kind == "Output"}
    listed = set(prod.outputs)
    if actual != listed:
        span = prod.span or derr.span_at("<unknown>", 0)
        derr.raise_e1601_output_manifest(prod.name, listed - actual, actual - listed, span)


def _lints(built: Sequence[BuiltComponent], decls: Sequence[Declaration]) -> list[Diagnostic]:
    """`W1101`: declared, never read, not an output. A warning — it never stops a build."""
    spans = {d.name: d.span for d in decls}
    read = {name for c in built for name in c.dependencies}
    out = []
    for c in built:
        if c.kind != "Output" and c.name not in read:
            out.append(derr.warn_w1101_unused(c.name, spans.get(c.name) or derr.span_at("<unknown>", 0)))
    return out
