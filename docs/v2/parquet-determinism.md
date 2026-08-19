# Byte-deterministic result files

[Determinism and goldens](determinism.md) promises that the same model, assumptions, modelpoints
and tables produce the same *numbers* however the run was scheduled. This page is about the
stronger claim one layer out: the same run produces the same **file**.

`results.parquet` is hashed into `manifest.json` as `results.digest`, and that digest is what a
governance pack, a CI cache and the wasm determinism gate all compare. A digest that moves when
nothing about the model moved is a false alarm — and worse, a false alarm that costs an actuary an
afternoon proving no number changed.

Spec: [`04-verify.md` §2](../design/04-verify.md) (the results schema and the
`(chunk_idx, offset)` order), [`03-engine.md` §7](../design/03-engine.md) (determinism).

---

## 1. The promise

```bash
cd models/ifrs17_gmm
predictable run run.pir --out /tmp/a --chunk-size 1
predictable run run.pir --out /tmp/b --chunk-size 4096
cmp /tmp/a/results.parquet /tmp/b/results.parquet   # identical
```

and equivalently through the manifest, which is the form you would actually script:

```bash
jq -r .results.digest /tmp/a/manifest.json
jq -r .results.digest /tmp/b/manifest.json
```

The same holds across `--threads`, because the runner already buffers chunks back into
`chunk_idx` order before they reach the writer:

```bash
predictable run run.pir --out /tmp/t1 --threads 1 --chunk-size 4
predictable run run.pir --out /tmp/t4 --threads 4 --chunk-size 4
cmp /tmp/t1/results.parquet /tmp/t4/results.parquet
```

What is **not** promised: that two *different* `WriterOptions` produce the same bytes. Changing
`batch_rows` or the compression codec changes the physical layout on purpose. Nothing the
command line exposes changes them, so this is a library-level caveat, not a user-level one.

---

## 2. Why row order was not enough

The writer has always emitted rows in `(chunk_idx, offset)` order and enforced it rather than
trusting the runner. That fixes the *logical* content of the file. It does not fix the
*physical* encoding, and Parquet has plenty of that: row groups, data pages, dictionary pages,
per-page statistics.

Until this fix, the writer accumulated rows into Arrow builders and flushed "when at least
`batch_rows` rows are buffered", checked once per chunk. With `--chunk-size 4096` the check fired
after a chunk that had already overshot the threshold by thousands of rows, so batches — and
therefore row groups and pages — landed in different places than they did under
`--chunk-size 1`. Every number was identical; the byte offsets were not:

| Run | Rows | Batches handed to Parquet | `results.digest` |
|---|---|---|---|
| `--chunk-size 1` | 485 | 64, 64, …, 37 | `sha256:024c38…` |
| `--chunk-size 4096` (old) | 485 | 485 | `sha256:ce92a7…` |

This was carried through the Phase 1 gate as an `xfail` on `ifrs17_gmm`. It is now a passing test
on all five reference models.

---

## 3. What makes it hold

Two rules, both in `crates/predictable-io/src/outbound/writer.rs`.

**Chunk arrival never reaches Parquet.** Accepted rows land in a `pending: Vec<Row>` buffer and
are drained in *exact* `batch_rows` slices:

```rust
fn flush_full_batches(&mut self) -> Result<()> {
    let n = self.options.batch_rows;
    while self.pending.len() >= n {
        let rest = self.pending.split_off(n);
        let batch = std::mem::replace(&mut self.pending, rest);
        self.write_rows(&batch)?;   // always exactly `n` rows
    }
    Ok(())
}
```

`write_chunk` never writes a short batch; only `finish` does, and only once, for the tail. So the
sequence of batches Parquet sees is a function of the row count alone. A 485-row result set is
`64 × 7 + 37` whether it arrived as one chunk or 485.

**Every layout knob is a row count, not a byte budget.** A byte budget makes the layout depend on
how well the values happened to compress, which is stable in practice but not something to rest a
digest on. The writer properties pin the boundaries explicitly:

```rust
WriterProperties::builder()
    .set_created_by("predictable".to_string())          // no build-version drift
    .set_writer_version(WriterVersion::PARQUET_1_0)
    .set_max_row_group_size(self.batch_rows)            // one row group per batch
    .set_write_batch_size(1024)
    .set_data_page_row_count_limit(DATA_PAGE_ROWS)      // pages by rows…
    .set_data_page_size_limit(usize::MAX)               // …never by bytes
    .set_dictionary_page_size_limit(DICTIONARY_PAGE_BYTES)
```

`batch_rows = 0` is refused at construction (`WriterOptions.batch_rows must be at least 1`)
rather than silently making the batch boundary undefined.

Note that this is bounded buffering, not "write the whole thing at once": at most
`batch_rows - 1` rows plus one chunk are ever held. Streaming behaviour is unchanged.

---

## 4. The tests

Library level, `crates/predictable-io/tests/outbound_results_writer.rs`:

- `chunking_does_not_change_the_bytes` — 97 modelpoints (485 rows) partitioned six different
  ways (`1, 3, 7, 16, 97, 4096` modelpoints per chunk), under both `Uncompressed` and `Snappy`,
  asserting equal digests *and* equal file bytes.
- `row_groups_are_batch_rows_sized_regardless_of_chunking` — reads the Parquet footer back and
  asserts the row groups really are `[64, 64, 64, 64, 64, 64, 64, 37]` under both chunkings, so
  the determinism was not bought by degenerating into a single giant row group.
- `zero_batch_rows_is_refused`.

End to end, `models/tests/test_reference_models.py`:

- `test_chunk_size_does_not_change_the_result_bytes` — all five reference models, three chunk
  sizes (`1`, `7`, `4096`), comparing `results.digest` and the raw bytes.
- `test_thread_count_does_not_change_the_result_bytes` — the same claim across `--threads`.

```bash
cargo test -p predictable-io --test outbound_results_writer
pytest models/tests -q
```

---

## 5. Regenerating goldens after a layout change

Fixing the encoding moved `results_digest` for `term_monthly`, `savings_monthly` and
`ifrs17_gmm` — the three models large enough to exceed one batch. No value changed. When a
change is *supposed* to move a digest, regenerate rather than edit:

```bash
cargo build -p predictable-cli
python models/tools/regen.py           # all five models
python models/tools/regen.py ifrs17_gmm   # or just one
pytest models/tests -q
```

`regen.py` rebuilds the committed `.pir` from the DSL as well as the goldens, so a digest change
and a model change can never be committed as if they were the same thing.

---

## 6. Reproducible docs builds

The same argument applies to the documentation site, which `mkdocs build --strict` fails on a
broken link or an unreachable nav entry: an unpinned theme or handler upgrade can turn the build
red without a commit. The docs toolchain is therefore a pinned
[PEP 735](https://peps.python.org/pep-0735/) dependency group in `pyproject.toml`:

```toml
[dependency-groups]
docs = [
  "mkdocs==1.6.1",
  "mkdocs-material==9.6.14",
  "mkdocstrings==0.29.1",
  "mkdocstrings-python==1.16.10",
]
```

Build the site with the group, and `uv.lock` fixes the whole transitive set:

```bash
uv run --group docs mkdocs build --strict
uv run --group docs mkdocs serve      # while writing
```

Use this in CI in place of the ad-hoc `--with mkdocs-material --with mkdocstrings-python` form,
which resolves the latest release on every run.
