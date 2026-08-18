//! # `predictable-determinism` — the harness that keeps results reproducible
//!
//! `03-engine.md` §7 makes six promises about a run. This crate is the machinery that tests
//! them, and nothing else: it computes no numbers of its own, it owns no engine behaviour, and
//! it depends on every other crate precisely so it can call them the way a real run does.
//!
//! | §7 clause | Enforced by | Test file |
//! |---|---|---|
//! | 1. Lane independence — `run(C=1) ≡ run(C=1024)` | [`project`] at two chunk sizes | `tests/chunk_invariance.rs` |
//! | 1. Schedule independence — `threads=1 ≡ threads=8` | [`Executors`] | `tests/chunk_invariance.rs` |
//! | 2. Reductions are sequential | adversarial `sum`/`npv` inputs | `tests/float_contract.rs` |
//! | 3. No FMA contraction | `a*b + c` against the split-product reference | `tests/float_contract.rs` |
//! | 6. Digest chain | digests in every golden header | `tests/golden.rs` |
//! | 7. Golden corpus | [`render_golden`] over [`cases`] | `tests/golden.rs` |
//!
//! ## What a golden is
//!
//! A golden file is the **byte-exact** rendering of a whole run: the digest chain, the plan's
//! evaluation order, and every emitted value written as its IEEE-754 bit pattern
//! ([`bits`]) beside its decimal form. Bits, not decimals, are the assertion — a golden that
//! compared `0.1 + 0.2` printed to 15 places would pass on a platform that got the last ULP
//! wrong, which is the exact failure the corpus exists to catch.
//!
//! Goldens are generated from the conformance corpus (`conformance/valid/**`), which was
//! authored from `01-ir.md` before any of this existed. Regenerate with:
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo test -p predictable-determinism
//! ```
//!
//! and expect the diff to be reviewed: a changed golden is a changed result.
//!
//! ## Cross-platform
//!
//! The committed goldens were generated on `aarch64-apple-darwin` and are asserted on every
//! platform the test runs on. There is deliberately **one** golden per case, not one per
//! target: §7 requires byte-identical output on `x86_64-linux`, `aarch64-macos` and
//! `wasm32-wasi`, so a per-target golden would encode the very divergence the corpus forbids.
//! CI adds a platform by running this same test there. [`host_target`] is recorded in each
//! golden header as provenance, and is *not* compared.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod corpus;
pub mod gen;
pub mod golden;
pub mod render;

pub use corpus::{cases, load, project, Case, Executors, Loaded, RunOutput};
pub use golden::{assert_golden, golden_dir, update_requested};
pub use render::{bits, host_target, render_golden};
