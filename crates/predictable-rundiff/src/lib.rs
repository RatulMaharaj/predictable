//! # `predictable-rundiff` — where two runs part company
//!
//! `04-verify.md` §5. Two run directories go in; one `pvf/1` `rundiff` document comes out, saying
//! *which output* diverges, *in which component*, *at which `t`*, on *which modelpoint*, whether
//! the divergence starts there or was inherited from upstream, and how much of the headline
//! output's movement it accounts for.
//!
//! ## Machine-first
//!
//! The JSON is the product and the terminal rendering is a projection of it. Nothing is printed
//! that is not also a field, because the loop this design rests on is an agent reading
//! `diff.json`, proposing an edit, re-running, and diffing again. A fact that exists only in prose
//! is a fact the loop cannot use.
//!
//! ## Either side may be a Prophet run
//!
//! `predictable-prophet`'s importer writes a real run directory — same `results.parquet`, same
//! `results.schema.json`, same `manifest.json` with `system = "prophet"`. So there is exactly one
//! reader here and exactly one comparison path; `diff run_prophet/ run/` is not a special case.
//! What *is* Prophet-specific is declared, not inferred: component correspondence comes from
//! `migration/mapping.toml` ([`Mapping`]), and the reader's own fixed precision raises the
//! tolerance for the components it wrote ([`tolerance`]).
//!
//! ## The five decisions, and where each lives
//!
//! | Decision | Module | Spec |
//! |---|---|---|
//! | What lines up with what | [`compare`] (alignment), [`mapping`] | §5.3 step 1, §4.4 |
//! | When two numbers are the same | [`tolerance`] — and nowhere else | §5.6 |
//! | Where it first goes wrong | [`compare`] | §5.3 step 3 |
//! | Whose fault it is | [`graph`] + [`compare`] | §5.3 steps 4–5 |
//! | How it is said | [`model`] (JSON), [`render`] (terminal) | §5.4, §5.7 |
//!
//! ```no_run
//! use predictable_rundiff::{diff_runs, DiffOptions, RunSide};
//!
//! let a = RunSide::load("a", "run_prophet/")?;
//! let b = RunSide::load("b", "run/")?;
//! let diff = diff_runs(&a, &b, &DiffOptions::default());
//! println!("{}", serde_json::to_string_pretty(&diff.to_json())?);
//! std::process::exit(diff.verdict.exit_code());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod compare;
pub mod error;
pub mod graph;
pub mod hypothesis;
pub mod mapping;
pub mod model;
pub mod mutation;
pub mod render;
pub mod run_side;
pub mod suggest;
pub mod tolerance;

use std::path::PathBuf;

use predictable_modeldiff::ModelSide;
use sha2::{Digest, Sha256};

pub use compare::diff_runs;
pub use error::DiffError;
pub use hypothesis::{Confidence, Context, Hypothesis, Support};
pub use mapping::Mapping;
pub use model::{
    Category, Cell, CellCounts, Class, Contribution, EmitMismatch, Explained, Finding,
    FirstDivergence, OneSided, OutputTotal, SetCounts, Structural, Verdict,
};
pub use run_side::{RunSide, Value};
pub use suggest::{SourceIndex, SuggestedEdit};
pub use tolerance::{Profile, Tol, ToleranceReport};

/// `format` of the document this crate writes.
pub const FORMAT: &str = "pvf/1";
/// `kind` of the document this crate writes.
pub const KIND: &str = "rundiff";

/// Everything the diff needs beyond the two runs themselves.
#[derive(Debug, Clone)]
pub struct DiffOptions {
    /// The tolerance profile in force.
    pub profile: Profile,
    /// `migration/mapping.toml`, when one was given.
    pub mapping: Option<Mapping>,
    /// `--top`: keep this many findings. `0` keeps all.
    pub top: usize,
    /// `--component`: report only this component (qualified or bare).
    pub filter_component: Option<String>,
    /// `--mp`: compare only this modelpoint.
    pub filter_mp: Option<String>,
    /// `--require-same-emit`: restore `exit 2` for a component-set difference (Q6).
    pub require_same_emit: bool,
    /// Raise the tolerance to the non-predictable side's own reporting precision (§5.6 rule 4).
    pub use_source_precision: bool,
    /// `a`'s model, for the dependency graph and the model-diff attribution.
    pub model_a: Option<ModelSide>,
    /// `b`'s model.
    pub model_b: Option<ModelSide>,
    /// `b`'s model source files, indexed by span, so a hypothesis can carry a literal edit
    /// (§5.5). Empty is not a failure: the detectors then propose without a `suggested_edit`.
    pub source_b: SourceIndex,
}

impl Default for DiffOptions {
    fn default() -> DiffOptions {
        DiffOptions {
            // `reconcile` is §5.6's default for a Prophet diff, and the only profile whose
            // per-unit table is right for money out of the box.
            profile: Profile::builtin("reconcile").expect("reconcile is a builtin"),
            mapping: None,
            top: 20,
            filter_component: None,
            filter_mp: None,
            require_same_emit: false,
            use_source_precision: true,
            model_a: None,
            model_b: None,
            source_b: SourceIndex::empty(),
        }
    }
}

impl DiffOptions {
    /// Load each side's model from the paths its manifest recorded, when they still exist.
    ///
    /// Without this the diff still works; it simply cannot classify root versus inherited from the
    /// IR, and says so in `class_basis` rather than pretending.
    pub fn with_models_from(mut self, a: &RunSide, b: &RunSide) -> DiffOptions {
        self.model_a = compare::load_model(&run_side::model_files(a), "a");
        let b_files = run_side::model_files(b);
        self.model_b = compare::load_model(&b_files, "b");
        self.source_b = SourceIndex::load(&b_files);
        self
    }

    /// Load models from explicit paths.
    pub fn with_model_paths(mut self, a: Option<&[PathBuf]>, b: Option<&[PathBuf]>) -> DiffOptions {
        if let Some(files) = a {
            self.model_a = compare::load_model(files, "a");
        }
        if let Some(files) = b {
            self.model_b = compare::load_model(files, "b");
            self.source_b = SourceIndex::load(files);
        }
        self
    }
}

/// The whole diff.
#[derive(Debug, Clone)]
pub struct RunDiff {
    /// `a`'s directory.
    pub a_path: String,
    /// `b`'s directory.
    pub b_path: String,
    /// `a`'s `run_id`.
    pub a_run: String,
    /// `b`'s `run_id`.
    pub b_run: String,
    /// `a`'s system (`predictable` | `prophet`).
    pub a_system: String,
    /// `b`'s system.
    pub b_system: String,
    /// `a`'s manifest digest.
    pub a_manifest_digest: String,
    /// `b`'s manifest digest.
    pub b_manifest_digest: String,
    /// `a`'s engine version.
    pub a_engine_version: String,
    /// `b`'s engine version.
    pub b_engine_version: String,
    /// `a`'s model digest.
    pub a_model_digest: String,
    /// `b`'s model digest.
    pub b_model_digest: String,
    /// The mapping in force.
    pub mapping: Option<Mapping>,
    /// The verdict, and the exit code.
    pub verdict: Verdict,
    /// Why the runs are incomparable, when they are. Empty otherwise.
    pub incomparable: Vec<String>,
    /// The emit/component-set difference, when there is one (Q6).
    pub emit_mismatch: Option<EmitMismatch>,
    /// Component set counts.
    pub components: SetCounts,
    /// Modelpoint set counts.
    pub modelpoints: SetCounts,
    /// Cell counts.
    pub cells: CellCounts,
    /// Per-output totals.
    pub outputs: Vec<OutputTotal>,
    /// The findings, ranked and truncated.
    pub findings: Vec<Finding>,
    /// `(kept, of)` when `--top` truncated the list.
    pub findings_truncated: Option<(usize, usize)>,
    /// One-sided components and modelpoints.
    pub structural: Structural,
    /// Where the run first parts company.
    pub first_divergence: Option<FirstDivergence>,
    /// How many findings are root divergences.
    pub root_divergences: usize,
    /// The tolerance in force, and every override that fired.
    pub tolerances: ToleranceReport,
    /// Whether both models were available, so `explained_by_model_change` means anything.
    pub model_attribution_available: bool,
    /// Whether an IR dependency graph was available for root/inherited classification.
    pub graph_available: bool,
}

impl RunDiff {
    /// The `pvf/1` `rundiff` document of §5.4.
    pub fn to_json(&self) -> serde_json::Value {
        let mut doc = serde_json::json!({
            "format": FORMAT,
            "kind": KIND,
            "a": {
                "path": self.a_path,
                "run": self.a_run,
                "system": self.a_system,
                "manifest": self.a_manifest_digest,
                "engine_version": self.a_engine_version,
                "model_digest": self.a_model_digest,
            },
            "b": {
                "path": self.b_path,
                "run": self.b_run,
                "system": self.b_system,
                "manifest": self.b_manifest_digest,
                "engine_version": self.b_engine_version,
                "model_digest": self.b_model_digest,
            },
            "tolerance": self.tolerances.to_json(),
            "mapping": self.mapping.as_ref().map(|m| serde_json::json!({
                "file": m.file,
                "digest": m.digest,
                "adjusted": m.adjusted(),
                "unmapped": m.unmapped,
            })),
            "summary": {
                "verdict": self.verdict,
                "incomparable": self.incomparable,
                "modelpoints": self.modelpoints,
                "components": self.components,
                "emit_mismatch": self.emit_mismatch,
                "cells": self.cells,
                "outputs": self.outputs,
                "root_divergences": self.root_divergences,
                "first_divergence": self.first_divergence,
                "model_attribution_available": self.model_attribution_available,
                "graph_available": self.graph_available,
            },
            "findings": self.findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
            "structural": self.structural,
        });
        if let Some((kept, of)) = self.findings_truncated {
            doc.as_object_mut()
                .expect("the document is an object")
                .insert(
                    "findings_truncated".into(),
                    serde_json::json!({"kept": kept, "of": of}),
                );
        }
        doc
    }

    /// The one-line signal of §5.2 for the top finding, when there is one.
    pub fn headline(&self) -> Option<&str> {
        self.findings
            .iter()
            .find(|f| f.class != Class::ToleranceOnly)
            .map(|f| f.message.as_str())
    }
}

/// SHA-256 of some bytes, hex, lowercase. The mapping file's identity.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
