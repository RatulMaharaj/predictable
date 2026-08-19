# Reading `.pir` files: the syntax crate

`predictable-syntax` is the front end of the IR. It turns the bytes of a `.pir`
file into a typed document plus a list of diagnostics, and nothing else: it
decides what a file *says*, never whether what it says makes sense.

It is pass 1 of the six checker passes in the [IR spec](../design/01-ir.md#7-type-checking).
Name resolution, cycle detection, shape and unit checking and the lints all run
*over* its output.

If you are an actuary writing models, you will meet this crate through the error
messages it produces. If you are building a tool — an editor plugin, a migration
script, a diff viewer — this is the crate you depend on, because it is the only
thing that has to understand `.pir` text.

## What a `.pir` file is

A restricted TOML dialect, one file per module, one `[[component]]` table per
component, with formulas written in a small infix language inside strings:

```toml
format = "pir/1"
module = "term_assurance"

[[component]]
name  = "num_pols_if"
kind  = "Derived"
dtype = "f64"
shape = "Series"
unit  = "count"
timing = "start"
init  = "1.0"
expr  = "num_pols_if[t-1] * (1 - qx[t-1]) * (1 - wx[t-1])"
doc   = "Survivorship. Self-referential with lag 1 — legal."
```

Three deliberate departures from stock TOML:

| Departure | Why |
|---|---|
| `;` separates several pairs on one line — `name = "x"; dtype = "i64"` | It is how the schema blocks in the spec are written, and it keeps a field declaration to one readable line. |
| Dates are kept as the text you wrote | The parser does not own a calendar type; the engine does. `valuation_date = 2026-06-30` round-trips exactly. |
| Bare keys are identifiers | `.pir` never needs numeric or quoted keys, and the restriction makes the lexer unambiguous. |

## Parsing a file

```rust
use predictable_syntax::{parse, SourceMap};

let mut sources = SourceMap::new();
let parsed = parse(&mut sources, "model.pir", std::fs::read_to_string("model.pir")?);

for d in parsed.diagnostics.iter() {
    let loc = sources.location(d.primary_span().unwrap());
    println!("{}: {} at {}:{}:{}", d.code, d.message, loc.file, loc.line, loc.col);
}

for component in &parsed.document.components {
    println!("{} : {} {}", component.name.value, component.shape, component.dtype);
}
```

`parse` never fails and never panics. A file with errors still returns a
document containing everything that did parse — which is what lets a tool show
you your model *and* your mistakes at the same time.

### What you get back

`PirDocument` holds the typed contents of one file, in declaration order
(declaration order is authorial intent, and it is the tiebreak the engine uses
when ordering evaluation):

- `components`, each with `kind`, `dtype`, `shape`, `unit`, `timing`, `init`,
  `expr`, `doc` and `tags`
- `modelpoint_fields`, `assumptions`, `tables`, `enums`, `timeline`
- `product`, `run`, `solves`, `aggregations` for the non-module file kinds
- `assumption_values` for an assumption set
- `arena` — every expression in the file

`document.kind()` tells you which of the four file kinds you are holding
(`Module`, `AssumptionSet`, `Product`, `Run`), decided by content, never by
filename.

## Expressions and the arena

Formulas are parsed into an `ExprArena`: a flat vector of nodes addressed by a
`u32` `ExprId`, with spans in a side table. One arena serves the whole file, so
an `ExprId` is unique across the document and cheap to store, copy and compare.

```rust
let deaths = parsed.document.component("deaths").unwrap();
let arena = &parsed.document.arena;

for r in arena.references(deaths.expr.unwrap(), "expr") {
    println!("{} at {} ({:?})", r.name, r.path, r.lag);
}
// num_pols_if at expr.lhs (Current)
// qx at expr.rhs (Current)
```

The grammar is exactly the one in the IR spec:

```
expr    := ternary
ternary := "if" expr "then" expr "else" expr | orexpr
orexpr  := andexpr ("or" andexpr)*
andexpr := cmp ("and" cmp)*
cmp     := sum (("==" | "!=" | "<" | "<=" | ">" | ">=") sum)?
sum     := product (("+" | "-") product)*
product := unary (("*" | "/") unary)*
unary   := ("-" | "not") unary | power
power   := atom ("^" unary)?
atom    := literal | call | lookup | ref | "(" expr ")"
ref     := IDENT index?
index   := "[" ("t" ("-" INT)? | INT) "]"
call    := IDENT "(" (expr ("," expr)*)? ")"
lookup  := IDENT "@" "(" expr ("," expr)* ")"
```

Things worth knowing when you write a formula:

- **`x`, `x[t]`, `x[t-1]`, `x[0]` are the four time references.** `x` and `x[t]`
  are the same node. `x[t-k]` is a lag, `x[k]` is an absolute period.
- **`x[t+1]` is refused**, with the reason: a projection is a single forward
  pass. If you need the future, aggregate over the completed series.
- **`^` is right-associative and binds tighter than unary minus**, so `-a ^ b`
  is `-(a ^ b)`. Comparisons do not chain: `a < b < c` is an error, not a
  mis-association.
- **`if` is a value conditional**, not control flow; both arms are required and
  both are typed.
- **`tbl@(k1, k2)` is a table lookup**, deliberately spelled differently from a
  function call, because table names and builtin names live in different
  namespaces.
- **`retime(x, mid)`'s second argument is a timing tag**, not a component. The
  parser rewrites it to a literal so that nothing downstream goes looking for a
  component called `mid`.

### `ExprPath`

Every node has a deterministic, dotted path from its expression root —
`expr.lhs.arg0.key1` is "the second lookup key of the first argument of the call
on the left of the top-level operator". Paths are what traces, graph layouts and
run-diffs use to point at part of a formula, because unlike byte spans they do
not move when a file is reformatted.

```rust
let node = arena.resolve_path(root, "expr", "expr.lhs.lhs").unwrap();
```

`resolve_path` returning `None` is how a consumer detects that a cached path is
stale against the current model.

### Stage

`component.stage(&arena)` returns `2` if the formula contains an aggregate
(`sum`, `npv`, `last`, `count_while`, …) and `1` otherwise — the two-stage
evaluation of the IR spec, computed rather than declared, so it cannot disagree
with the formula.

## Diagnostics

Every problem comes back as `{code, severity, message, labels, suggestions,
doc_url}` — the same shape the project-wide diagnostics crate renders and emits
as JSON. Labels carry byte spans; suggestions carry literal replacement text
over a byte range, so an agent (or an editor's quick-fix) can apply them
mechanically and re-check.

Syntax codes owned by this crate:

| Range | Meaning | Examples |
|---|---|---|
| `E0001`–`E0014` | lexical and TOML-structural | unterminated string, missing `=`, duplicate key, unclosed array |
| `E0020`–`E0032` | expression grammar | missing operand, unclosed `(`, chained comparison, `x[t+1]` |
| `E0041`–`E0053` | block schema | missing required key, unknown key, illegal `kind`/`shape`/`unit` value |

Three codes fixed by the IR spec are also emitted here, because they are
decidable without resolving any name: `E0101` (a run trying to set a timeline
field), `E0108` (`storage_precision` other than `"f64"`) and `E0305` (an ordered
key policy on an unordered enum key).

Misspellings come with a fix attached:

```
error[E0044]: unknown key `timming`
  --> model.pir:24:1
   |
24 | timming = "start"
   | ^^^^^^^ not a key of this block
   = help: did you mean `timing`?
```

## Error recovery

Parsing does not stop at the first error, because a checker that reports one
problem per run is a checker you run twenty times. The recovery rules:

- a malformed line is reported **once** and skipped to the next newline;
- a malformed section header resynchronises on the next header;
- a malformed formula yields an error node, and the rest of the document still
  lowers.

So a file with a broken expression in one component, a misspelled `kind` in
another and a typo'd key in a third reports three diagnostics, and the fourth,
correct component is parsed as normal.

## The source map

Spans are byte ranges, and a `SourceMap` is the only thing that can turn one
back into a file, a line and a column:

```rust
let loc = sources.location(span);      // { file, line, col }, 1-based
let text = sources.snippet(span);      // the bytes the span covers
let line = sources.line_text(span);    // the whole line, for a caret rendering
```

Byte ranges rather than char offsets is a deliberate choice: it is what a
mechanical suggested-edit applier needs, and it is what terminal renderers
consume.

## Scope

What this crate does **not** do, by design:

- resolve names, or tell you that `qx` is undefined
- detect cycles, check shapes, check units, or run any lint
- read files from disk, resolve table sources, or compute digests
- format a file back out (that is `predictable fmt`'s job, and it consumes this
  crate's output)

Those all belong to later passes, and keeping them out is what makes this crate
usable from an editor, a browser and a migration script alike.
