# demos/

Three demos, three scripts, three claims. Each script runs end to end, asserts its own
result, and exits non-zero if the claim stops being true — so a demo that has quietly
rotted fails rather than misleads.

| | claim | run it | write-up |
|---|---|---|---|
| **1. migration** | an agent can migrate a Prophet library and *prove* it right — reconciled to the penny, no tolerance widened | `./demos/migration/run_demo.sh` | [README](migration/README.md) · [docs](../docs/v2/demo-migration.md) |
| **2. benchmarks** | the performance numbers are gated on correctness: nothing is timed until every engine agrees | `./demos/benchmarks/run_demo.sh` | [README](benchmarks/README.md) · [docs](../docs/v2/demo-benchmarks.md) |
| **3. ifrs17** | a real IFRS 17 GMM valuation over 10,000 contracts, reconciled, traceable to one cell | `./demos/ifrs17/run_demo.sh` | [README](ifrs17/README.md) · [docs](../docs/v2/ifrs17-valuation.md) |

## Before you start

```console
$ cargo build --release -p predictable-cli
```

The scripts build the CLI themselves if it is missing, and they use `.venv/bin/python` at
the repository root when it exists. Override either:

```console
$ PREDICTABLE=/path/to/predictable PYTHON=/path/to/python ./demos/migration/run_demo.sh
```

Demo 2 additionally needs `benchmarks/.venv`, which holds the three competing engines; the
script creates it with `uv` on first run.

## What the scripts generate

Everything under `demos/*/runs/`, `demos/*/out/` and `demos/migration/build*/` is generated
and git-ignored. The inputs — the Prophet workspace, the models, the mapping file, the
portfolio generator — are committed.
