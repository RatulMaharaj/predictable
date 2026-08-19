//! Why a run diff could not be produced.
//!
//! Every variant here is an *exit 2* condition: the diff never got as far as having an opinion
//! about numbers. Divergence is not an error — it is a result, and it lives in [`crate::RunDiff`].

/// A failure to load or compare two runs.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffError {
    /// The path is not a readable run directory.
    NotARunDir {
        /// The path as given.
        path: String,
        /// What was wrong with it.
        why: String,
    },
    /// The directory has a manifest but no results (an aborted run, IR §9.3.1).
    NoResults {
        /// The run directory.
        path: String,
        /// `manifest.execution.outcome`.
        outcome: String,
    },
    /// `results.parquet` could not be read, or is not in the §2 schema.
    Parquet {
        /// The file.
        path: String,
        /// The reader's complaint.
        why: String,
    },
    /// The mapping file could not be read or parsed.
    Mapping {
        /// The mapping file.
        path: String,
        /// What was wrong.
        why: String,
    },
    /// An unknown tolerance profile name.
    UnknownProfile {
        /// The name asked for.
        name: String,
        /// The names that exist.
        known: Vec<String>,
    },
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::NotARunDir { path, why } => write!(f, "{path}: not a run directory ({why})"),
            DiffError::NoResults { path, outcome } => write!(
                f,
                "{path}: run outcome is `{outcome}`, so there are no results to diff"
            ),
            DiffError::Parquet { path, why } => write!(f, "{path}: {why}"),
            DiffError::Mapping { path, why } => write!(f, "{path}: {why}"),
            DiffError::UnknownProfile { name, known } => write!(
                f,
                "unknown tolerance profile `{name}`; known profiles are {}",
                known.join(", ")
            ),
        }
    }
}

impl std::error::Error for DiffError {}
