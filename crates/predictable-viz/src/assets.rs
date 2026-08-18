//! The embedded SPA shell.
//!
//! The server embeds the built frontend at compile time so that a `predictable`
//! binary is the whole product — no asset directory to lose, no path to
//! configure, and a governance pack that inlines the same bytes (§1.4).
//!
//! `frontend/dist/index.html` is checked in. Today it is a placeholder shell
//! that proves the transport end to end: it reads the token from the URL
//! fragment, calls `/api/capabilities` and `/api/graph`, and prints the
//! planner's layers. T29/T30 replace it by running `npm run build` in
//! `crates/predictable-viz/frontend`, which writes the real bundle to the same
//! path and needs no change here.

/// The `index.html` served at `/`.
pub const SHELL_HTML: &str = include_str!("../frontend/dist/index.html");
