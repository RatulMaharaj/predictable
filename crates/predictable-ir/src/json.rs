//! The lossless `pir.json` encoding (§4).
//!
//! `pir.json` is *generated*, never authored: the `.pir` text is the source of truth. It exists
//! so tooling that should not embed a parser — a browser, a notebook, another language — can
//! read a model. The encoding is a mechanical transcription: array-of-table names become array
//! field names, `[timeline]` becomes an object, expressions become node-tagged trees, and every
//! value round-trips.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::IrError;
use crate::model::{AssumptionSet, Module};
use crate::run::{ProductFile, RunFile};
use crate::FORMAT;

/// Any of the four `.pir` file kinds (§6, §8.4).
///
/// The kind is decided by which top-level key is present — `module`, `product`, `run`,
/// `assumption_set` — exactly as it is in the TOML.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PirFile {
    Module(Box<Module>),
    Product(Box<ProductFile>),
    Run(Box<RunFile>),
    AssumptionSet(Box<AssumptionSet>),
}

impl PirFile {
    /// The `format` string this file declares.
    pub fn format(&self) -> &str {
        match self {
            PirFile::Module(m) => &m.format,
            PirFile::Product(p) => &p.format,
            PirFile::Run(r) => &r.format,
            PirFile::AssumptionSet(a) => &a.format,
        }
    }

    /// A short name for the file kind, used in diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            PirFile::Module(_) => "module",
            PirFile::Product(_) => "product",
            PirFile::Run(_) => "run",
            PirFile::AssumptionSet(_) => "assumption set",
        }
    }

    /// Reject a document whose `format` this build does not read (§10).
    pub fn check_format(&self) -> Result<(), IrError> {
        if self.format() == FORMAT {
            Ok(())
        } else {
            Err(IrError::Format {
                found: self.format().to_string(),
                expected: FORMAT.to_string(),
            })
        }
    }

    /// Decode a `pir.json` document, dispatching on its top-level keys.
    pub fn from_json(text: &str) -> Result<PirFile, IrError> {
        let value: Value = serde_json::from_str(text)?;
        PirFile::from_value(value)
    }

    /// Decode from an already-parsed JSON value.
    pub fn from_value(value: Value) -> Result<PirFile, IrError> {
        let obj = value.as_object().ok_or(IrError::UnknownFileKind)?;
        let file = if obj.contains_key("product") {
            PirFile::Product(Box::new(ProductFile::deserialize(&value)?))
        } else if obj.contains_key("run") {
            PirFile::Run(Box::new(RunFile::deserialize(&value)?))
        } else if obj.contains_key("assumption_set") {
            PirFile::AssumptionSet(Box::new(AssumptionSet::deserialize(&value)?))
        } else if obj.contains_key("module") {
            PirFile::Module(Box::new(Module::deserialize(&value)?))
        } else {
            return Err(IrError::UnknownFileKind);
        };
        Ok(file)
    }

    /// Encode as compact `pir.json`.
    pub fn to_json(&self) -> Result<String, IrError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Encode as indented `pir.json` with a trailing newline — the form written to disk.
    pub fn to_json_pretty(&self) -> Result<String, IrError> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }
}

impl From<Module> for PirFile {
    fn from(m: Module) -> Self {
        PirFile::Module(Box::new(m))
    }
}

impl From<ProductFile> for PirFile {
    fn from(p: ProductFile) -> Self {
        PirFile::Product(Box::new(p))
    }
}

impl From<RunFile> for PirFile {
    fn from(r: RunFile) -> Self {
        PirFile::Run(Box::new(r))
    }
}

impl From<AssumptionSet> for PirFile {
    fn from(a: AssumptionSet) -> Self {
        PirFile::AssumptionSet(Box::new(a))
    }
}
