//! # `predictable-wasm` — the engine in the page
//!
//! `03-engine.md` §9 and `05-viz.md` §1.5. The browser build runs the *same*
//! `predictable-engine` code as the CLI: same planner, same tapes, same kernel,
//! same `explain()` replay. Nothing here computes a number of its own. What it
//! does is remove every host dependency the native pipeline has:
//!
//! * **no filesystem** — sources, tables and modelpoints arrive as text through
//!   [`Inputs`], and tables resolve through [`session::PackResolver`] rather
//!   than `FsResolver`, digest check included;
//! * **no threads** — one lane at a time, `SerialExecutor` semantics by
//!   construction. §7's lane independence is what makes that safe: a chunk of 1
//!   and a chunk of 1024 must agree, so the browser may use either;
//! * **no clock, no `std::time`, no rayon, no `predictable-io`** — the Arrow and
//!   Parquet stacks stay out of the wasm dependency graph, which is most of why
//!   the artefact fits §9's 1.5 MB gzipped budget.
//!
//! ## The surface
//!
//! §9 lists four calls: `check`, `plan`, `run`, `explain`. They are exposed
//! twice, from one implementation:
//!
//! * as ordinary Rust ([`api::call`], taking and returning a JSON string), which
//!   is what the native tests and the WASI determinism gate use;
//! * as a three-function C ABI ([`abi`]) on `wasm32`, which is what the browser
//!   uses.
//!
//! ### Why a C ABI and not `wasm-bindgen`
//!
//! A governance pack is **one file** (`05-viz.md` §1.4). `wasm-bindgen` emits a
//! second artefact — a JS glue module produced by `wasm-bindgen-cli` — and
//! inlining generated glue into the pack means the pack's contents depend on a
//! tool version that the auditor cannot re-derive from this repository. Three
//! exported functions (`pv_alloc`, `pv_free`, `pv_call`) over a length-prefixed
//! UTF-8 buffer need about thirty lines of loader JS, which *is* checked in
//! (`crates/predictable-viz/frontend/src/wasm/engine.ts`), and the `.wasm` is
//! then the only binary the pack embeds. The exposed calls are exactly §9's.
//!
//! ## Determinism
//!
//! §9 claims wasm results are bit-identical to native. `tests/wasm_gate.rs`
//! checks it rather than assuming it: every conformance case is rendered to
//! IEEE-754 bit patterns by [`render::render_case`], natively and again under
//! `wasmtime` on `wasm32-wasip1`, and the two byte strings must be equal. That
//! is T16's contract extended to a third target, using T16's corpus.

#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod abi;
pub mod api;
pub mod csv;
pub mod render;
pub mod session;

pub use api::call;
pub use session::{Inputs, Session, SessionError, SourceFile, TableBytes};

/// Version of the wasm engine build, reported by `{"op":"version"}`.
///
/// It is the crate version, which tracks the engine's: a pack states which
/// engine traced its numbers, and a pack whose embedded engine disagreed with
/// the manifest is a finding, not a detail.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
