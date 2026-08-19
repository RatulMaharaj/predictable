# Goldens

Generated on **aarch64 macOS** and asserted unchanged on every supported target — one golden per
case, not one per target, because byte-identical output *across* targets is the requirement.

* `<case>.golden` — a whole run of a `conformance/valid/**` case: digest chain, evaluation order,
  and every emitted value as its IEEE-754 bit pattern.
* `transcendentals.golden` — `exp`/`ln`/`pow`/`sqrt` pinned to the bit.
* `_skipped.txt` — corpus cases the harness cannot execute, and why. Also a golden: a case that
  silently stops running is the failure a determinism corpus is least able to notice.

Regenerate deliberately, in the same commit as the change that moved the numbers:

```bash
UPDATE_GOLDEN=1 cargo test -p predictable-determinism
```

See `docs/v2/determinism.md`.
