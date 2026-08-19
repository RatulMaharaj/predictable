# Loading modelpoints

`predictable-io` is the crate that reads modelpoint files. It has two halves — an **inbound** half
that turns a file into typed lanes for the kernel, and an **outbound** half that writes results —
and this page is about the inbound one.

Three properties are worth understanding before you point the engine at a file, because they are
what make a bad modelpoint file a good error message rather than a crash 40,000 rows in.

1. **The schema is the model's, not the file's.** `[[modelpoint_field]]` in your `.pir` is the
   contract; the file is checked against it.
2. **Only the columns your model reads are read.** A model touching 7 of an MPF's 60 columns reads
   7 columns off disk.
3. **Nulls are eliminated at the boundary.** Nothing downstream of the loader has a concept of
   missing data ([IR §2.11][q3], ruling Q3).

[q3]: ../design/01-ir.md

---

## The schema is part of the model

```toml
[[modelpoint_field]]
name     = "policy_id"
dtype    = "str"
key      = true
required = true

[[modelpoint_field]]
name     = "sum_assured"
dtype    = "f64"
unit     = "money"
required = true

[[modelpoint_field]]
name    = "smoker_loading"
dtype   = "f64"
default = 1.0          # optional fields MUST declare a default

[[modelpoint_field]]
name    = "gender"
dtype   = "enum(Gender)"
default = "female"
```

Exactly one field carries `key = true`: it is the join key results are written against. A field is
either `required = true` or carries a `default` — there is no third option, and a schema that
tries to have one is refused before any file is opened:

```
modelpoint field `smoker_loading` is optional but declares no `default`;
§2.11 requires one so that no lane can hold a sentinel
```

## Validation happens before row 1

Everything decidable from the file's header is decided from the file's header. A missing required
column names the file, the column, **and the components that read it**, so you know what breaks:

```
modelpoint file `portfolio.parquet` has no column `sum_assured`, which the model requires
  referenced by: claims, sum_at_risk
  help: add the column to the file, or declare `default = ...` on
        `[[modelpoint_field]] name = "sum_assured"` to make it optional
```

A column whose physical type cannot supply the declared dtype is the same kind of error, also
before the first row group is decoded:

```
modelpoint file `portfolio.parquet`: column `sum_assured` has type `Utf8`,
but the model declares `dtype = "f64"`
  help: cast the column in the source file, or change the declared dtype
```

Widening is allowed — an `Int64` column may fill an `f64` lane — but a string is never silently
parsed into a number, and a narrowing never happens at all.

A column the schema does not declare is **not** an error. §2.10 says it is accepted with a lint,
and it is:

```
modelpoint file `portfolio.parquet`: column `legacy_code` is not declared in the
modelpoint schema and is ignored
```

## Column pruning

`MpSchema` is built from the module, and it knows which fields are actually read, because it walks
every component's `expr` and `init` looking for references:

```rust
use predictable_io::{MpSchema, ParquetSource};

// Every declared field:            8 columns.
let full = MpSchema::from_module(&module)?;
// Only what the model reads:       3 columns (plus the key, always kept).
let plan = MpSchema::pruned_for_module(&module)?;
assert_eq!(plan.column_names(), ["policy_id", "sum_assured", "smoker_loading"]);

let source = ParquetSource::open(plan, "portfolio.parquet")?;
```

`ParquetSource` pushes that list down as a Parquet projection mask, so the five unread columns are
never decompressed. Pruned columns produce their own lint, distinct from the unknown-column one —
"declared but read by no component" is a fact about your model, not about your data.

## Nulls stop here

There is no null value at runtime. Every lane holds a defined value of its dtype for every `t`,
which is what keeps the kernel's inner loop a dense `f64` scan. So the loader eliminates
missingness as it reads:

| in the file | field is `required = true` | field has a `default` |
|---|---|---|
| a value | used | used, presence bit `true` |
| a null / empty cell | **error**, naming the row | the default, presence bit `false` |
| the column is absent entirely | **error**, before row 1 | the default for every row |

```
modelpoint file `portfolio.csv`, row 2: column `sum_assured` is null,
but the field is `required = true`
  help: fill the cell, or make the field optional by giving it a `default`
```

### Presence lanes are opt-in

`is_null(f)` and `coalesce(f, y)` do not observe a runtime null — they observe a **presence bit**,
and one exists only where the model asks for it. The loader materialises a `bool` lane beside the
value lane for exactly those optional fields the model calls `is_null`/`coalesce` on, and for no
others. The cost is opt-in and visible in `MpField::presence`.

```rust
let plan = MpSchema::pruned_for_module(&module)?;
assert!( plan.field("smoker_loading").unwrap().presence);  // `coalesce(smoker_loading, 1.0)`
assert!(!plan.field("sum_assured").unwrap().presence);     // nobody asked
```

At a load of

```csv
policy_id,sum_assured,smoker_loading
P1,100000,1.5
P2,50000,
P3,25000,NA
```

the `smoker_loading` value lane is `[1.5, 1.0, 1.0]` and its presence lane is
`[true, false, false]`. `NA`, `N/A`, `null`, `NaN`, `none` and the empty cell are all nulls in CSV,
case-insensitively.

## Chunks are deterministic

```rust
use predictable_io::{ChunkColumns, ModelpointSource};

let mut chunk = ChunkColumns::for_schema(source.schema());
while source.next_chunk(1024, &mut chunk)? > 0 {
    // chunk.chunk_idx(), chunk.first_row() — the writer's ordering key
    let sums = chunk.column("sum_assured").unwrap().as_f64().unwrap();
}
```

`next_chunk(n, ..)` yields exactly `n` rows until the file runs out. The file's own physical
batching is invisible: Parquet row-group boundaries and Arrow batch boundaries are re-cut, so chunk
boundaries are a function of `C` and the row count only. That is half of why `run(C = 1)` and
`run(C = 1024)` are bit-identical; lane independence is the other half.

`chunk_idx` and `first_row` travel with the chunk so the results writer can emit in
`(chunk_idx, offset)` order regardless of which chunk finished first.

The buffer is reused across chunks — `next_chunk` clears it rather than reallocating — so a long
run does not touch the allocator per chunk.

## The three sources

| type | when |
|---|---|
| `ParquetSource` | the default. Column pruning is a real projection pushed into the reader. |
| `CsvSource` | text files. The IR schema drives the parse; there is no type inference. |
| `ArrowSource` | a `RecordBatchReader` handed over in-process, e.g. from Python. |

All three implement one trait:

```rust
pub trait ModelpointSource: Send {
    fn schema(&self) -> &MpSchema;
    fn next_chunk(&mut self, n: usize, out: &mut ChunkColumns) -> Result<usize, IoError>;
    fn lints(&self) -> &[Lint];
}
```

and all three agree row for row on the same data — a test asserts it.

## Lane representations

| dtype | lane |
|---|---|
| `f64` | `f64` |
| `i64` | `i64` |
| `bool` | `bool` |
| `date` | `i32`, days since the Unix epoch |
| `str` | `String` |
| `enum(X)` | `u32` dictionary code, indexing `[[enum]] values` in declaration order |

An enum cell whose text is not a declared variant is an error that lists the variants:

```
modelpoint file `portfolio.csv`, row 1: column `gender` holds `unspecified`,
which is not a variant of `enum(Gender)`
  variants: male, female
```

Enums compare by equality alone, never by ordinal — the dictionary code is a storage detail, and
reordering an `[[enum]]` changes the codes, not the model's meaning.

## What is not here yet

The outbound half — the long-format results schema, the Parquet/Arrow writer and `manifest.json` —
is a stub module owned by a later task. The seam is deliberate: `predictable_io::inbound` and
`predictable_io::outbound` share only the error type, and neither reaches into the other.
