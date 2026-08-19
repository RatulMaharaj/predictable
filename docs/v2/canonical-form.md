# Canonical form and digests

Two `.pir` files that mean the same thing should *look* the same, and a model
that means something different should say so in one number. That is the job of
`predictable fmt` and of the digests computed over its output.

- **`predictable fmt`** rewrites a `.pir` file into its canonical form
  ([IR spec §4.1](../design/01-ir.md)). It is the arbiter of layout; CI enforces it.
- **`predictable digest`** hashes that canonical text. Because the input to
  every digest is canonical, reformatting a model cannot change its identity —
  and nothing else can fail to.

The implementation is the `predictable-fmt` crate; the command line is
`predictable-cli`.

---

## 1. Why formatting is a correctness feature

An actuarial model is reviewed as a diff. A formatter that leaves the author's
whitespace alone produces diffs full of lines that changed but do not matter,
and a reviewer who learns to skim those lines will eventually skim the one that
did. So canonical form is not a style preference: it is the reason a `.pir` diff
can be read as a *list of decisions that changed*.

The same argument fixes the digest rule. `model_digest` is taken over canonical
text, so:

```console
$ predictable fmt models/          # 4 files reformatted
$ predictable digest models/       # …the same digest as before
```

Reformat freely. Renaming a component, moving a decimal point or changing a
timing tag always moves the digest; reindenting never does.

---

## 2. The canonical form

### 2.1 The rules from the spec

1. **Key order inside a `[[component]]` is fixed**:
   `name, kind, dtype, shape, unit, timing, init, expr, doc, tags, output`.
   Absent keys are skipped, never emitted empty. Keys outside the list keep
   their relative order and follow the ones inside it.
2. **Declaration order is never touched.** Components stay in the order they
   were written — that order is authorial intent and it is the tiebreak in the
   evaluation order (§3.2) — and neither are top-level blocks reordered.
3. **Expressions are normalised** (§2.2 below).
4. **Floats round-trip via shortest representation** (Ryū). `1.050` becomes
   `1.05`; `0.10000000000000001` becomes `0.1`; `1.05` never becomes
   `1.0500000000000001`. A float never loses its point: `45.00` becomes `45.0`,
   not `45`. Exponent notation is used only where it is shorter — `1e-8`, but
   `1000000.0`.
5. **UTF-8, LF, one trailing newline, no trailing whitespace.**
6. **`fmt` is idempotent**: `fmt(fmt(x)) == fmt(x)`, proven over the whole
   conformance corpus and over machine-mangled variants of it.

### 2.2 Expressions

Formulas are re-printed from the parsed expression, not patched textually:

| Input | Canonical |
|---|---|
| `a*b   +c` | `a * b + c` |
| `sum( a ,b )` | `sum(a, b)` |
| `sa8990@( age ,gender )` | `sa8990@(age, gender)` |
| `x[ t - 1 ]` | `x[t-1]` |
| `(sum_assured)` | `sum_assured` |
| `a or (b and c)` | `a or b and c` |
| `retime(x, mid)` | `retime(x, mid)` |

Parentheses are removed when they say nothing, and kept when they say
something:

```toml
expr = "(a + b) * c"      # kept: they change the answer
expr = "a - (b - c)"      # kept: subtraction is not associative
expr = "a + (b + c)"      # kept: neither is floating-point addition
expr = "(a * b) + c"      # kept: §4.1 preserves mixed */+ as written
expr = "a * b + c"        # left alone: fmt never *adds* those parentheses
```

The last two lines are the one place where canonical form depends on what the
author wrote. Grouping that only restates the precedence of `*` over `+` is
preserved if it is there and never invented if it is not — because a reviewer
who put it there put it there on purpose, and a reviewer who did not should not
find it in the diff.

### 2.3 Layout

The spec fixes rules 1–6; the formatter fixes the rest, and the conformance
corpus pins it:

- one `key = value` per line — `;`-joined pairs are split onto their own lines;
- exactly one blank line before every `[section]` header, and **nowhere else**.
  A blank line is formatting, and formatting must be invisible to a digest,
  which it cannot be if `fmt` preserves it;
- comments are kept, at column 0, immediately above the item they precede.
  Inside a `[[component]]`, whose keys are reordered by rule 1, they are hoisted
  to just under the header — a comment cannot stay glued to a line that moves.
  Comments *are* content: unlike whitespace, editing one moves the digest;
- an array is written on one line unless it has more than one element and any
  element is itself an array or an inline table, in which case it goes one
  element per line, two-space indent, trailing comma:

```toml
keys = [
  { name = "age", dtype = "i64", policy = "clamp" },
  { name = "gender", dtype = "enum(Gender)", policy = "exact" },
]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
```

### 2.4 What `fmt` will not do

`fmt` refuses to rewrite a file that does not parse. It reports the syntax
errors and leaves the bytes on disk untouched: a formatter that guesses at a
half-written file is a formatter that eventually eats one.

---

## 3. Using the command

```console
$ predictable fmt models/                 # rewrite every .pir under models/
models/term_assurance/model.pir: formatted

$ predictable fmt --check models/         # CI mode: exit 1 if anything is off
models/term_assurance/model.pir: not canonical

$ predictable fmt --stdout model.pir      # print, do not write
$ cat model.pir | predictable fmt -       # or read stdin
```

`--check` never writes. Wire it into CI as the guard that keeps every diff
readable:

```yaml
- run: predictable fmt --check models/
```

---

## 4. Digests

Every digest is SHA-256, rendered `sha256:` followed by 64 lowercase hex
digits, and every model-level digest is taken over **canonical text**.

| Digest | Over what | Spec |
|---|---|---|
| `model_digest` | canonical text of every module *and* the product file, in path order | §9.3 rule 6 |
| `run_digest` | canonical text of the `[run]` file minus `[run.exec]`, `out`, `progress` | §8.4.2 |
| `assumption_digest` | canonical text of the assumption-set `.pir` | §9.4 |
| table `digest` | the **bytes** the resolver returned (a CSV is not `.pir` text) | §2.9 |
| inline table digest | canonical text of the table's `rows` array | §2.9.1 |
| `component_set_digest` | the sorted, newline-terminated list of emitted component ids | §8.4.3 |

### 4.1 Framing

A digest over several files must not depend on where one file ends and the next
begins. Files are therefore fed to the hash length-prefixed, in ascending byte
order of their path:

```text
<path>\n<byte length of the canonical text>\n<canonical text>
```

Two different file sets can never produce the same byte stream, so
`{a.pir: "one", b.pir: "two"}` and `{a.pir: "onetwo"}` have different digests,
and so do two identical files under different names.

### 4.2 `run_digest` and the fields it ignores

A field participates in `run_digest` **iff changing it can change a number in
`results.parquet`**. `threads`, `chunk_size` and `progress` cannot — a run is
bit-identical at any thread count — and `out` is where results are written, not
what they are. Everything else is in: `product`, `assumptions`, `modelpoints`,
`emit`, `emit_list`, `retain`, `storage_precision`, `on_trap`, `max_errors`,
`allow_table_drift`, `sum_kahan`, `[run.tables]`, every `[[solve]]` and every
`[[aggregation]]`.

### 4.3 On the command line

```console
$ predictable digest models/                       # one model_digest over the set
sha256:6f1c…  model_digest

$ predictable digest --each models/                # plus one line per file
sha256:9ab3…  models/term_assurance/model.pir
sha256:1d40…  models/term_assurance/schema.pir
sha256:6f1c…  model_digest

$ predictable digest --kind run runs/base.pir      # run_digest
$ predictable digest --kind assumptions base.pir   # assumption_digest
$ predictable digest --kind file tables/sa8990.csv # raw bytes, as tables are hashed
$ predictable digest --kind table:lapse_rates model.pir   # an inlined table's rows
```

A run is reproducible from six digests — `model_digest`, `assumption_digest`,
`modelpoint_digest`, the table digests, `run_digest` and the engine version —
and the run manifest carries all six.

---

## 5. From Rust

```rust
use predictable_fmt::{format_source, is_canonical};
use predictable_fmt::digest::{model_digest_of_sources, run_digest, component_set_digest};

let canonical = format_source("model.pir", source)?;      // Err = it does not parse
assert!(is_canonical("model.pir", &canonical)?);

let digest = model_digest_of_sources([
    ("schema.pir", schema_source),
    ("model.pir", model_source),
    ("product.pir", product_source),
])?;

let run = run_digest("run.pir", run_source)?;
let emitted = component_set_digest(&["term_assurance.bel", "term_assurance.reserve"]);
```

`format_source` returns an `FmtError` carrying the parser's diagnostics; there
is no lossy mode.

---

## 6. Corpus reconciliation

The conformance corpus is written from the spec before the tools exist, so when
a tool and the corpus disagree the default assumption is that the tool is wrong.
Building `fmt` turned up three disagreements. Two were corpus bugs and were
fixed in the corpus, with the citation recorded here:

| File | Was | Now | Why |
|---|---|---|---|
| `valid/v04-expressions/model.pir` | `not (flow > 0.0) or (flow >= 0.0 and flow <= amount)` | `… or flow >= 0.0 and flow <= amount` | §4.1 rule 3 exempts mixed `*`/`+` only; `and` under `or` is a redundant parenthesis |
| `invalid/e21-cross-module-shadow/b.pir` | `expr = "1.10"` | `expr = "1.1"` | §4.1 rule 4, the same rule the corpus pins as `1.050` → `1.05` |
| `valid/v0{1,2,8}-*/base.pir` | a blank line between the header keys and the assumption values | no blank line | §9.3 rule 6: formatting-only changes must not move a digest, which requires blank lines to be canonicalised away |

The third is a layout rule the spec leaves open, decided against the corpus's
authored layout and in favour of the digest guarantee; §2.3 above states it.
