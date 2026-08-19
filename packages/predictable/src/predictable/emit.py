"""Step 4 of `02-dsl.md` §8 — render a built model to canonical `.pir` text.

The rules are `01-ir.md` §4.1's, and they are the *formatter's* rules, not a second dialect:
fixed key order inside a `[[component]]`, declaration order everywhere else, normalised
expressions (already produced by :mod:`predictable.ir`), Ryū floats, LF, one trailing newline,
exactly one blank line before each section header, and arrays broken one element per line only
when they hold arrays or inline tables.

Nothing here re-derives an expression: `predictable.ir.to_pir` is a line-for-line port of
``predictable-fmt``'s ``expr_fmt.rs`` and the text it returns goes out unchanged. What this module
adds is the *file* layout, the split of one built product into the module files the engine reads,
and the table digests of §2.9 — which are SHA-256 over the bytes the resolver would return, not
over the path.

The output is verified, not assumed: :func:`emit` hands every file to the engine's own formatter
(``predictable_engine.format_pir``) and refuses to write text the formatter would rewrite. A
`.pir` that ``predictable fmt`` disagrees with is a `.pir` that ``predictable build --check``
would flag as drift the next day.
"""

from __future__ import annotations

import hashlib
import re
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Any

from .declarations import BuiltComponent, BuiltModel
from .ir import format_f64

__all__ = [
    "EmitOptions",
    "Emitted",
    "emit",
    "format_value",
    "module_file_names",
    "render_module",
    "render_product",
    "table_digest",
]

_DATE_KEYS = frozenset({"valuation_date"})
_IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


# --------------------------------------------------------------------------------------
# TOML value rendering (the half of canonical form that is not the expression printer)
# --------------------------------------------------------------------------------------


def _escape(s: str) -> str:
    out = []
    for ch in s:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        else:
            out.append(ch)
    return "".join(out)


def format_value(value: Any, *, raw: bool = False) -> str:
    """One TOML value in canonical form. ``raw`` writes a bare date literal, not a string."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return format_f64(value)
    if isinstance(value, date):
        return value.isoformat()
    if isinstance(value, str):
        if raw:
            return value
        return f'"{_escape(value)}"'
    if isinstance(value, dict):
        inner = ", ".join(f"{k} = {format_value(v)}" for k, v in value.items())
        return "{ " + inner + " }" if inner else "{}"
    if isinstance(value, (list, tuple)):
        return _array(list(value))
    raise TypeError(f"cannot render {value!r} as a .pir value")


def _array(items: Sequence[Any]) -> str:
    """§4.1's array rule: one line, unless >1 element and any element is itself structured."""
    structured = any(isinstance(i, (list, tuple, dict)) for i in items)
    if len(items) > 1 and structured:
        body = "".join(f"  {format_value(i)},\n" for i in items)
        return "[\n" + body + "]"
    return "[" + ", ".join(format_value(i) for i in items) + "]"


def _pairs(out: list[str], table: dict[str, Any]) -> None:
    for key, value in table.items():
        if value is None:
            continue
        out.append(f"{key} = {format_value(value, raw=key in _DATE_KEYS)}")


# --------------------------------------------------------------------------------------
# digests
# --------------------------------------------------------------------------------------


def table_digest(source: str, root: Path) -> str | None:
    """`01-ir.md` §2.9: the digest is over the *bytes*, so only a resolvable source has one.

    ``resource:`` sources are supplied by the host at run time and ``inline`` rows are digested
    by the engine over their canonical text, so neither is hashed here.
    """
    if source == "inline" or source.startswith("resource:"):
        return None
    path = root / source
    if not path.is_file():
        return None
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


# --------------------------------------------------------------------------------------
# file layout
# --------------------------------------------------------------------------------------


def module_file_names(module_paths: Sequence[str]) -> dict[str, str]:
    """Python module path → `.pir` module name.

    The leaf is used when it is unambiguous (``models.term.decrements`` → ``decrements``),
    because that is the name a human writes in ``imports``; the full dotted path with ``.``
    replaced by ``_`` is used when two Python modules share a leaf.
    """
    leaves: dict[str, list[str]] = {}
    for path in module_paths:
        leaves.setdefault(path.rsplit(".", 1)[-1], []).append(path)
    out: dict[str, str] = {}
    for path in module_paths:
        leaf = path.rsplit(".", 1)[-1]
        name = leaf if len(leaves[leaf]) == 1 else path.replace(".", "_")
        out[path] = _sanitise(name)
    return out


def _sanitise(name: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9_]", "_", name)
    return cleaned if _IDENT.match(cleaned) else f"m_{cleaned}"


@dataclass(frozen=True)
class EmitOptions:
    """What `predictable build` needs to know that the model itself does not carry."""

    #: Directory table `source` paths resolve against (`01-ir.md` §2.9.1, relative to the project).
    root: Path = field(default_factory=Path.cwd)
    #: Name of the schema module — timeline, enums, fields, assumptions and tables live there.
    schema_module: str = "schema"
    #: Write `[component.meta]` into the `.pir`. Probed against the installed checker by
    #: :mod:`predictable.builder`, because a checker that rejects the section would make every
    #: emitted module unbuildable.
    include_meta: bool = True


@dataclass(frozen=True)
class Emitted:
    """The whole build product: the files, their digests and the provenance side table."""

    files: dict[str, str]
    table_digests: dict[str, str | None]
    #: Python module path → `.pir` module name.
    module_names: dict[str, str]
    #: `.pir` file name → the module path it came from (``None`` for schema/product).
    file_modules: dict[str, str | None]

    @property
    def module_files(self) -> dict[str, str]:
        return {f: m for f, m in self.file_modules.items() if m is not None}


def emit(model: BuiltModel, options: EmitOptions | None = None) -> Emitted:
    """Render a built model to its canonical `.pir` files."""
    opts = options or EmitOptions()
    module_paths = list(dict.fromkeys(c.module for c in model.components))
    names = module_file_names(module_paths)

    digests = {t.name: table_digest(t.source, opts.root) for t in model.tables}

    files: dict[str, str] = {}
    file_modules: dict[str, str | None] = {}

    has_schema = bool(
        model.timeline or model.enums or model.fields or model.assumptions or model.tables
    )
    if has_schema:
        files[f"{opts.schema_module}.pir"] = render_schema(model, opts, digests)
        file_modules[f"{opts.schema_module}.pir"] = None

    owner = {c.name: names[c.module] for c in model.components}
    for path in module_paths:
        components = [c for c in model.components if c.module == path]
        name = names[path]
        imports = _imports_of(components, name, owner, opts, has_schema)
        files[f"{name}.pir"] = render_module(name, imports, components, opts)
        file_modules[f"{name}.pir"] = path

    if model.product is not None:
        modules = ([opts.schema_module] if has_schema else []) + [names[p] for p in module_paths]
        files["product.pir"] = render_product(model, modules)
        file_modules["product.pir"] = None

    return Emitted(
        files=files, table_digests=digests, module_names=names, file_modules=file_modules
    )


def _imports_of(
    components: Sequence[BuiltComponent],
    own_name: str,
    owner: dict[str, str],
    opts: EmitOptions,
    has_schema: bool,
) -> list[str]:
    """The closed import set: every other emitted module this one actually reads from.

    A dependency that no component declares is a modelpoint field, an assumption or a timeline
    input — all of which live in the schema module, so the schema is imported for those and for
    every table lookup.
    """
    needed: list[str] = []

    def add(name: str) -> None:
        if name != own_name and name not in needed:
            needed.append(name)

    for c in components:
        if c.tables and has_schema:
            add(opts.schema_module)
        for dep in c.dependencies:
            module = owner.get(dep)
            if module is None:
                if has_schema:
                    add(opts.schema_module)
            else:
                add(module)
    return needed


# --------------------------------------------------------------------------------------
# the files
# --------------------------------------------------------------------------------------


def render_schema(model: BuiltModel, opts: EmitOptions, digests: dict[str, str | None]) -> str:
    out: list[str] = ['format = "pir/1"', f'module = "{opts.schema_module}"']

    if model.timeline is not None:
        timeline = model.timeline.to_ir()
        out += ["", "[timeline]"]
        # §2.4's own key order, which `to_ir` does not promise.
        _pairs(
            out,
            {
                k: timeline.get(k)
                for k in ("basis", "periods", "origin", "valuation_date", "year_convention")
            },
        )

    for enum in model.enums:
        out += ["", "[[enum]]", f'name = "{enum.name}"', f"values = {format_value(list(enum.values))}"]

    for f in model.fields:
        out += ["", "[[modelpoint_field]]"]
        _pairs(
            out,
            {
                "name": f.name,
                "dtype": f.dtype,
                "unit": f.unit if f.unit and f.unit != "none" else None,
                "required": f.required,
                "key": True if f.key else None,
                "default": f.default if not f.required else None,
            },
        )

    for a in model.assumptions:
        out += ["", "[[assumption]]"]
        _pairs(out, {"name": a.name, "dtype": a.dtype, "unit": a.unit, "shape": a.shape})

    for t in model.tables:
        ir = t.to_ir()
        out += ["", "[[table]]"]
        _pairs(
            out,
            {
                "name": ir["name"],
                "keys": ir["keys"],
                "values": [{k: v for k, v in val.items() if v} for val in ir["values"]],
                "on_missing": ir["on_missing"],
                "source": ir["source"],
                "digest": digests.get(t.name),
            },
        )

    return _finish(out)


#: Intra-``[[component]]`` key order — `01-ir.md` §4.1 rule 1, mirrored from
#: ``predictable-fmt``'s ``COMPONENT_KEY_ORDER``.
COMPONENT_KEY_ORDER = ("name", "kind", "dtype", "shape", "unit", "timing", "init", "expr", "doc", "tags")


def render_module(
    name: str,
    imports: Sequence[str],
    components: Sequence[BuiltComponent],
    opts: EmitOptions,
) -> str:
    out: list[str] = ['format = "pir/1"', f'module = "{name}"']
    if imports:
        out.append(f"imports = {format_value(list(imports))}")

    for c in components:
        out += ["", "[[component]]"]
        _pairs(out, component_table(c))
        if opts.include_meta:
            meta = meta_table(c, opts.root)
            if meta:
                out += ["", "[component.meta]"]
                _pairs(out, meta)

    return _finish(out)


def component_table(c: BuiltComponent) -> dict[str, Any]:
    """The component's top-level keys, in canonical order."""
    table: dict[str, Any] = {
        "name": c.name,
        "kind": c.kind,
        "dtype": c.dtype,
        "shape": c.shape,
        "unit": c.unit,
        "timing": c.timing if c.shape == "Series" else None,
        "init": c.init.to_pir() if c.init is not None else None,
        "expr": c.expr.to_pir() if c.expr is not None else None,
        "doc": c.doc or None,
        "tags": list(c.tags) or None,
    }
    return {k: table[k] for k in COMPONENT_KEY_ORDER if table.get(k) is not None}


def meta_table(c: BuiltComponent, root: Path | None = None) -> dict[str, Any]:
    """`01-ir.md` §11.1's `meta`, restricted to the keys the DSL actually knows.

    `origin_span.file` is written relative to the build root: an absolute path would make the
    emitted `.pir` differ between machines, and a `.pir` is a committed artefact.
    """
    from .spanmap import relative_file

    meta = dict(c.meta)
    out: dict[str, Any] = {}
    for key in ("id", "authored_by", "origin_span", "overrides", "generated_from"):
        value = meta.get(key)
        if value is None:
            continue
        if key == "origin_span" and isinstance(value, dict):
            value = {**value, "file": relative_file(value["file"], root)}
        out[key] = value
    return out


def render_product(model: BuiltModel, modules: Sequence[str]) -> str:
    prod = model.product
    assert prod is not None
    out: list[str] = ['format = "pir/1"', "", "[product]"]
    _pairs(
        out,
        {
            "name": prod.name,
            "modules": list(modules),
            "outputs": [c.name for c in model.components if c.kind == "Output"],
            "key_field": prod.key_field or None,
            "assumptions": prod.assumptions or None,
            "doc": prod.doc or None,
        },
    )
    return _finish(out)


def _finish(lines: Iterable[str]) -> str:
    return "\n".join(line.rstrip() for line in lines) + "\n"
