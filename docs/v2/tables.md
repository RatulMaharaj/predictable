# Tables: loading, digests and lookups

A table in predictable is a **declared input** — typed, ordered keys, typed values, an explicit
miss policy, and a content digest. It is not a file path buried in a formula, and a lookup miss is
never a silent `NaN`.

This page describes what `predictable-tables` does with a `[[table]]` declaration: how the bytes are
found, how they are verified, and what structure the lookup compiles to.

Normative sources: [`01-ir.md` §2.9 / §2.9.1](../design/01-ir.md) and
[`03-engine.md` §6](../design/03-engine.md).

---

## 1. The declaration

```toml
[[table]]
name       = "mortality_sa8990"
keys       = [
  { name = "age",    dtype = "i64",          policy = "clamp" },
  { name = "gender", dtype = "enum(Gender)", policy = "exact" },
  { name = "smoker", dtype = "bool",         policy = "exact" },
]
values     = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source     = "tables/sa8990.csv"
digest     = "sha256:9f2c…e1"
```

Three fields decide everything downstream:

| Field | Decides |
|---|---|
| `keys[].policy` | what happens to a key value that is not exactly on the grid |
| `on_missing` | what the model sees when the key tuple has no row |
| `source` | which resolver fetches the bytes |

The declaration is also the **schema**: every column is typed before a byte is read, so a
malformed cell is reported as `row 2, column \`qx\`: \`not-a-number\` is not a valid f64`, not as a
downstream `NaN`.

---

## 2. Where the bytes come from

`source` is **always resolved relative to the directory of the `.pir` file that declares the
table** — never the process working directory, never a project root. A declaration therefore
relocates with its file.

The engine never opens a file itself. It calls a resolver:

```rust
pub trait TableResolver {
    fn resolve(&self, decl: &TableDecl, base: &Path) -> Result<TableBytes, ResolveError>;
}
```

| Resolver | Selected by | Behaviour |
|---|---|---|
| `FsResolver` | default, `file` scheme | reads `base.join(source)`, sandboxed to the project root |
| `MapResolver` | `resource:` scheme | a host-supplied name → bytes map: WASM, hosted runs, `predictable export` |
| `InlineResolver` | `source = "inline"` | reads the `rows` array in the declaration itself |

`SchemeResolver` dispatches between the three, which is what a runner installs when one model mixes
schemes.

The **project root** is the nearest ancestor directory containing `predictable.toml`, else the
module's own directory. A `source` that escapes it — or any absolute path — is
[`E0801`](../llm/diagnostics.md):

```text
error[E0801]: table `mortality` source `../../../secrets.csv` escapes the project root `/work/proj`
```

A `..` that stays inside the root is fine; the check is on where the path lands, not on how it is
spelled.

---

## 3. The digest is the table's identity

Whatever the resolver returns is hashed and compared with `digest` **before** compilation.

```rust
let table = predictable_tables::load(&decl, base, &resolver, &LoadOptions::default())?;
assert_eq!(table.digest, decl.digest.unwrap());
```

This is what makes swapping resolvers safe. A `resource:` run and a `file:` run of the same model
either agree on the bytes or are told they do not:

```text
error: table `mortality` content does not match its declared digest
  declared: sha256:9f2c…e1
  actual:   sha256:31aa…7d
  origin:   /work/proj/models/term/tables/sa8990.csv
  re-run with `--allow-table-drift` to accept the new content, or update `digest` in the declaration
```

`LoadOptions { allow_table_drift: true }` accepts the new content and records the fact on the
compiled table (`table.drifted`), so a manifest can say the run used content the model did not pin.
A run may override a table's `source`; it may never override its `digest`.

An **inline** table carries its rows in the module, and its digest is taken over the canonical text
of the `rows` array — so it is reproducible from the IR alone, with no filesystem involved:

```toml
[[table]]
name   = "lapse_rates"
keys   = [{ name = "policy_year", dtype = "i64", policy = "step" }]
values = [{ name = "lapse_pa", dtype = "f64" }]
source = "inline"
rows   = [
  [1, 0.12],
  [2, 0.08],
  [3, 0.05],
]
```

---

## 4. What a lookup compiles to

Compilation happens once per run. Afterwards a lookup is one index probe per key, one multiply-add
to compose the slots, and one load.

### 4.1 Per-key structures

Each key gets the structure its dtype and policy earn:

| Key | Structure | `KeyIndexKind` |
|---|---|---|
| `exact` on `i64`/`bool`, density > 0.5 | dense `Vec<u32>` indexed by `key - min` | `DenseInt` |
| `exact` on `i64`/`bool`, sparser | a perfect hash built at load | `PerfectHash` |
| `exact` on `str`/`date`/`enum(..)` | sorted dictionary + binary search | `Dictionary` |
| `clamp` / `step` | sorted domain + binary search | `SortedInt` / `SortedFloat` |
| `interpolate` | the same sorted domain plus a fused `lerp` | `SortedFloat` |

Ages 20–120 are contiguous, so they become a dense vector. Sum-assured bands of
`1 000, 8 919, 16 838, …` are not, so they become a perfect hash — built deterministically, so the
same key set produces the same table on every platform, which is what bit-reproducibility requires.

```rust
let table = load_bytes(&decl, csv(ages_20_to_120), &LoadOptions::default())?;
assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::DenseInt]);
```

**Enums are unordered.** An `enum(..)` or `str` key supports `policy = "exact"` only; `clamp`,
`step` and `interpolate` on one are refused. So is `exact` on an `f64` key — float equality is not
an index, and the message says to use `clamp`, `step` or `interpolate` instead.

### 4.2 Composing the keys

Multi-key tables compile to a **row-major dense array over the cartesian product** whenever that
product is under 2²⁴. The common mortality case is `121 × 2 × 2 = 484` cells:

```rust
let stats = table.stats();
assert_eq!(stats.index, "cartesian dense array");
assert_eq!(stats.cells, 484);
assert_eq!(stats.fill, 1.0);   // a fully populated grid
```

A ragged grid keeps its holes, and a hole still misses — the dense array is a fast index, not an
extrapolation. Over the ceiling (say three keys of 300 distinct values each: 2.7 × 10⁷ cells for
300 rows), the compiler falls back to **sorted key tuples + binary search**, and `stats().index`
says so.

### 4.3 The policies, worked

Given `term,rate = (5, 0.02), (10, 0.03), (20, 0.04)`:

| `term` | `exact` | `clamp` | `step` | `interpolate` |
|---|---|---|---|---|
| 1 | miss | `0.02` | miss | `0.02` |
| 5 | `0.02` | `0.02` | `0.02` | `0.02` |
| 12 | miss | miss | `0.03` | `0.0314` |
| 99 | miss | `0.04` | `0.04` | `0.04` |

- `clamp` clamps to the key range and then matches exactly, so a value *inside* the range but off
  the grid still misses.
- `step` returns the last row at or below the key — the banded-table policy. Below the first band
  there is no predecessor, so that misses.
- `interpolate` blends linearly and is flat outside the range; it never misses. Only `f64` value
  columns can be interpolated, and at most one key may interpolate.

`on_missing = "interpolate(<key>)"` is the declaration-level way of saying "that key
interpolates", and is folded into the key's policy at compile time.

---

## 5. Misses

`on_missing = "error"` is the default, and silent `NaN` propagation is not available at any setting.

```rust
match table.lookup_f64(&[KeyArg::Int(41), KeyArg::Bool(true)], 0) {
    Outcome::Hit(qx)         => …,   // the table carried it
    Outcome::Substituted(qx) => …,   // on_missing = "default(<lit>)"
    Outcome::Trap(miss)      => …,   // on_missing = "error"
}
```

A trap renders as `E0902` with `trap = "lookup_miss"`, naming the table and the key values that
missed:

```text
error[E0902]: no row in table `qx` for (41, true)
  note: `on_missing = "error"` is the default; set `on_missing = "default(<lit>)"` on the
        table if a miss is expected
```

`on_missing = "default(<lit>)"` substitutes the literal *and* lights the table's **presence bit** —
the only thing `is_null(tbl@(k…))` can observe. `table.contains(keys)` is that bit:

```rust
assert!(table.has_presence_bit());
assert!(table.contains(&[KeyArg::Str("PILOT")]));
assert!(!table.contains(&[KeyArg::Str("CLERK")]));
```

The default is one literal for the whole table, so it must fit every value column's dtype.

---

## 6. Content errors

Everything the loader refuses, it refuses with the row and the column:

| Situation | Message |
|---|---|
| declared column absent | ``column `qx` is declared but the source has only [age, mortality]`` |
| unparseable cell | ``row 2, column `qx`: `not-a-number` is not a valid f64`` |
| repeated key tuple | ``the key tuple (40) appears on rows 1 and 3`` |
| no data rows | ``the source has no rows`` |

Extra columns in the source are ignored — a table file with a `note` column the model does not
declare still loads. CSV quoting, `#` comment lines and blank lines are handled; Parquet and Arrow
tables are read through `predictable-io`, and this crate says so rather than guessing.

---

## 7. API summary

```rust
use predictable_tables::{load, FsResolver, KeyArg, LoadOptions, Outcome};

let resolver = FsResolver::new();
let table = load(&decl, module_dir, &resolver, &LoadOptions::default())?;

let qx = table.lookup_f64(&[KeyArg::Int(65), KeyArg::Str("M"), KeyArg::Bool(true)], 0);
```

| Item | Purpose |
|---|---|
| `load` / `load_bytes` / `load_all` | resolve → verify → parse → compile |
| `validate(&decl)` | declaration-only checks, before any content is read |
| `verify_digest` | hash the resolved bytes and compare with `digest` |
| `CompiledTable::probe` | the index path alone, for a multi-value table |
| `lookup_f64` / `_i64` / `_bool` / `_str` | value fetch with `on_missing` applied |
| `CompiledTable::contains` | the presence bit `is_null` observes |
| `CompiledTable::stats` | which structures were chosen, for `explain()` and manifests |
