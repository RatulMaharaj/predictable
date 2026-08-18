//! Errors and lints raised while loading modelpoints.
//!
//! The shape of these errors is deliberate: `01-ir.md` §2.10 requires that a modelpoint file
//! mismatch is a *definition-time-shaped* failure — it names the file, the column, and the
//! components that reference it — rather than a `KeyError` on row 40,000. Every variant that can
//! be detected from the header alone is detected before row 1 is decoded
//! ([`crate::inbound::validate`]).

use predictable_ir::DType;
use thiserror::Error;

/// A modelpoint load failure.
#[derive(Debug, Error)]
pub enum IoError {
    /// A required column of the IR modelpoint schema is absent from the file.
    #[error(
        "modelpoint file `{file}` has no column `{column}`, which the model requires\n  \
         referenced by: {}\n  help: add the column to the file, or declare `default = ...` on \
         `[[modelpoint_field]] name = \"{column}\"` to make it optional",
        fmt_refs(referenced_by)
    )]
    MissingColumn {
        /// The modelpoint file.
        file: String,
        /// The missing column.
        column: String,
        /// The components whose expressions read the field, in declaration order.
        referenced_by: Vec<String>,
    },

    /// The file's column type cannot supply the IR dtype.
    #[error(
        "modelpoint file `{file}`: column `{column}` has type `{found}`, but the model declares \
         `dtype = \"{expected}\"`\n  help: cast the column in the source file, or change the \
         declared dtype"
    )]
    TypeMismatch {
        /// The modelpoint file.
        file: String,
        /// The offending column.
        column: String,
        /// The declared IR dtype.
        expected: DType,
        /// The physical type found in the file.
        found: String,
    },

    /// The same column name appears twice in the file.
    #[error("modelpoint file `{file}`: column `{column}` appears more than once")]
    DuplicateColumn {
        /// The modelpoint file.
        file: String,
        /// The duplicated column.
        column: String,
    },

    /// A required field held a null. Required fields have no default, so there is nothing to
    /// eliminate the missingness with (§2.11).
    #[error(
        "modelpoint file `{file}`, row {row}: column `{column}` is null, but the field is \
         `required = true`\n  help: fill the cell, or make the field optional by giving it a \
         `default`"
    )]
    NullInRequired {
        /// The modelpoint file.
        file: String,
        /// The offending column.
        column: String,
        /// 1-based row number within the data (header excluded).
        row: u64,
    },

    /// An optional field was declared without a `default`, so missingness cannot be eliminated.
    /// The checker rejects this too; the loader refuses to guess.
    #[error(
        "modelpoint field `{column}` is optional but declares no `default`; §2.11 requires one so \
         that no lane can hold a sentinel"
    )]
    OptionalWithoutDefault {
        /// The offending field.
        column: String,
    },

    /// A cell could not be decoded as the declared dtype.
    #[error(
        "modelpoint file `{file}`, row {row}: cannot read `{text}` in column `{column}` as \
         `{expected}`"
    )]
    BadValue {
        /// The modelpoint file.
        file: String,
        /// The offending column.
        column: String,
        /// 1-based row number within the data.
        row: u64,
        /// The raw text.
        text: String,
        /// The declared dtype.
        expected: DType,
    },

    /// An enum-typed cell held a variant the `[[enum]]` declaration does not list.
    #[error(
        "modelpoint file `{file}`, row {row}: column `{column}` holds `{value}`, which is not a \
         variant of `enum({enum_name})`\n  variants: {}",
        expected.join(", ")
    )]
    UnknownVariant {
        /// The modelpoint file.
        file: String,
        /// The offending column.
        column: String,
        /// 1-based row number within the data.
        row: u64,
        /// The value read.
        value: String,
        /// The enum's name.
        enum_name: String,
        /// The declared variants.
        expected: Vec<String>,
    },

    /// A field is `dtype = "enum(X)"` but no `[[enum]] name = "X"` is declared in the module.
    #[error(
        "modelpoint field `{column}` is `enum({enum_name})`, but no such `[[enum]]` is declared"
    )]
    UnknownEnum {
        /// The offending field.
        column: String,
        /// The undeclared enum.
        enum_name: String,
    },

    /// The schema declares no `key = true` field, so results could not be joined.
    #[error(
        "the modelpoint schema declares no `key = true` field; results have nothing to join on"
    )]
    NoKeyField,

    /// The schema declares more than one `key = true` field.
    #[error("the modelpoint schema declares more than one `key = true` field: {}", .0.join(", "))]
    MultipleKeyFields(Vec<String>),

    /// Failure reported by the underlying reader (Parquet, CSV, Arrow).
    #[error("modelpoint file `{file}`: {message}")]
    Backend {
        /// The modelpoint file.
        file: String,
        /// The backend's own message.
        message: String,
    },

    /// Filesystem failure.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn fmt_refs(refs: &[String]) -> String {
    if refs.is_empty() {
        "(no component reads it; it is declared in the schema)".to_string()
    } else {
        refs.join(", ")
    }
}

/// A non-fatal observation about the modelpoint file.
///
/// §2.10: "a modelpoint file with an unknown extra column is accepted with a lint".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lint {
    /// The file carries a column the IR schema does not declare. It is ignored.
    UnknownColumn {
        /// The modelpoint file.
        file: String,
        /// The undeclared column.
        column: String,
    },
    /// A declared field is not read by any component and was pruned from the read.
    PrunedColumn {
        /// The modelpoint file.
        file: String,
        /// The pruned column.
        column: String,
    },
}

impl std::fmt::Display for Lint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lint::UnknownColumn { file, column } => write!(
                f,
                "modelpoint file `{file}`: column `{column}` is not declared in the modelpoint \
                 schema and is ignored"
            ),
            Lint::PrunedColumn { file, column } => write!(
                f,
                "modelpoint file `{file}`: column `{column}` is declared but read by no \
                 component; it is not loaded"
            ),
        }
    }
}
