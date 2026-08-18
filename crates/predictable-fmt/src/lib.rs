//! `predictable-fmt` — the canonical form of a `.pir` file, and the digests
//! computed over it.
//!
//! Two things live here, and they are one thing:
//!
//! * [`format_source`] implements `predictable fmt` (`01-ir.md` §4.1). It is the
//!   arbiter of what a `.pir` file looks like, and it is a fixed point:
//!   `fmt(fmt(x)) == fmt(x)` for every file that parses.
//! * [`digest`] hashes that canonical text (`01-ir.md` §9.3 rule 6). Because the
//!   input to every digest is canonical, a whitespace change, a reordered
//!   component key or `1.050` written for `1.05` cannot change a model's
//!   identity — and nothing else can fail to.
//!
//! ```
//! use predictable_fmt::{format_source, digest::model_digest_of_sources};
//!
//! let source = r#"
//! format = "pir/1"
//! module = "m"
//!
//! [[component]]
//! expr = "( a  *  b ) + 1.050"
//! name = "x"
//! kind = "Derived"
//! dtype = "f64"
//! shape = "PerMP"
//! unit = "money"
//! "#;
//!
//! let canonical = format_source("m.pir", source).unwrap();
//! assert!(canonical.contains("expr = \"(a * b) + 1.05\""));
//! assert!(canonical.starts_with("format = \"pir/1\"\n"));
//!
//! // Reformatting is not a model change.
//! let a = model_digest_of_sources([("m.pir", source)]).unwrap();
//! let b = model_digest_of_sources([("m.pir", canonical.as_str())]).unwrap();
//! assert_eq!(a, b);
//! ```

pub mod canonical;
pub mod digest;
pub mod expr_fmt;
pub mod float;

pub use canonical::{format_source, format_source_with, is_canonical, Filter, FmtError};
pub use expr_fmt::format_expression;
pub use float::format_f64;

/// Escape a string into a TOML basic string body — rule 5's UTF-8 half: every
/// character is written literally except the four the grammar cannot carry.
pub(crate) fn escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
}
