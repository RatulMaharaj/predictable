//! # `predictable-prophet` — read-only Prophet interop
//!
//! Three readers for the three Prophet artefacts a migration has to consume
//! (`04-verify.md` §4): model point files (`.MPF`), factor tables (`.fac`) and
//! results files (`.rpt`). predictable never *writes* Prophet formats.
//!
//! Every reader is the same shape — a pure function
//! `bytes -> (value, [Diagnostic])`:
//!
//! ```text
//! read_mpf(bytes, file, &MpfOptions) -> Read<MpfFile>
//! read_fac(bytes, file)              -> Read<FacTable>
//! read_rpt(bytes, file, &RptOptions) -> Read<RptResults>
//! ```
//!
//! No IO, no globals, no panics, and no `Err`: a file that cannot be interpreted
//! yields an empty value and at least one `Severity::Error` diagnostic. The
//! diagnostics carry `P0xxx` codes and **byte offsets into the original file**,
//! so a migrating agent can point a human at the exact line — which is why the
//! scanners work on raw bytes and decode only field slices (Windows-1252 decoding
//! is one byte per character, so an offset never moves).
//!
//! ## The three refusals
//!
//! Most of the value here is in what the readers decline to do, because each of
//! these guesses is silent, plausible, and wrong often enough to cost a week:
//!
//! 1. **Date order.** `01/02/2024` is ambiguous; when neither `DATE_FORMAT` nor
//!    the data resolves it, the `.MPF` reader emits `P0109` instead of picking.
//! 2. **Table orientation and size.** A `.fac` is read with the *last* dimension
//!    varying fastest, restated as `P0202` with a corner of the table to eyeball,
//!    and its value count must equal `∏(HIₖ − LOₖ + 1)` exactly (`P0203`) — no
//!    padding, no truncation.
//! 3. **Period base.** A 1-based `.rpt` without an explicit `--period-base` is
//!    `P0302`. An off-by-one in `t` diffs at *every* timestep.
//!
//! Units are the fourth: numeric `.MPF` columns import as `unit = "none"` with a
//! `W0102` lint each, because guessing `money` from a column name is exactly the
//! inference this project exists to refuse.
//!
//! ## Reading a model point file
//!
//! ```
//! use predictable_prophet::{read_mpf, Cell, MpfOptions};
//!
//! let src = b"! term assurance\n\
//!             VARIABLE_TYPES,I,S,N\n\
//!             NUMLINES,1\n\
//!             SPCODE,POL_NUM,SUM_ASSURED\n\
//!             *,1,POL00042,100000\n";
//! let read = read_mpf(src, "data/term.mpf", &MpfOptions::default());
//!
//! assert_eq!(read.value.columns.len(), 3);
//! assert_eq!(read.value.cell(0, "pol_num"), Some(&Cell::Str("POL00042".into())));
//! // The one lint is the refused unit inference on `SUM_ASSURED`.
//! assert_eq!(read.codes(), ["W0102"]);
//! assert!(!read.has_errors());
//! ```
//!
//! ## Reading a factor table
//!
//! ```
//! use predictable_prophet::read_fac;
//!
//! let src = b"TABLE_NAME, SA8990\n\
//!             DIMENSIONS, 2\n\
//!             DIM1, AGE, 40, 41\n\
//!             DIM2, SMOKER, 0, 1\n\
//!             DATA\n\
//!             0.001, 0.003, 0.0012, 0.0035\n";
//! let read = read_fac(src, "tables/sa8990.fac");
//!
//! // Last dimension fastest: cell 1 is (age = 40, smoker = 1).
//! assert_eq!(read.value.key_at(1), vec![40, 1]);
//! assert_eq!(read.value.values_at(1), &[0.003]);
//! assert!(!read.has_errors());
//! ```

#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod common;
pub mod fac;
pub mod mpf;
pub mod rpt;
pub mod text;

#[cfg(feature = "run-dir")]
pub mod run_dir;

pub use common::{Cell, Header, HeaderEntry, Read};
pub use fac::{read_fac, FacDim, FacTable, ProposedPolicy};
pub use mpf::{read_mpf, DateOrder, MpfColumn, MpfFile, MpfOptions};
pub use rpt::{read_rpt, RptComponent, RptLevel, RptOptions, RptResults, RptRow};
pub use text::{Encoding, Field, Line};

#[cfg(feature = "run-dir")]
pub use run_dir::{write_run_dir, ImportError, ImportOptions};

/// Lower-case hex SHA-256 — the digest form every `pvf/1` artefact uses (without
/// the `sha256:` prefix, which the caller adds).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Render an `f64` in the shortest form that round-trips, with a non-finite value
/// rendered as the empty field rather than `NaN` — a CSV consumer must not read
/// "not a number" as a number.
pub fn format_f64(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        String::new()
    }
}
