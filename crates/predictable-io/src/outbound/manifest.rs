//! `run/manifest.json` — the `pvf/1` run manifest of `04-verify.md` §6.
//!
//! > A result set without a manifest is not evidence.
//!
//! Two rules drive the whole module:
//!
//! 1. **Key order is the spec's order.** Every struct below declares its fields in the order
//!    §6.2 prints them, and serialisation goes straight from the struct (never through
//!    `serde_json::Value`, which would sort keys), so a manifest diffs cleanly in git.
//! 2. **`manifest_digest` is over the reproducible part.** It excludes `run_id`, `execution`,
//!    `results.digest`, `provenance.user` and itself, because two runs with the same
//!    `manifest_digest` **must** produce a byte-identical `results.parquet` (§6.3 rule 1).
//!    `environment_hash` is deliberately *outside* the digest: same digest, different
//!    environment is precisely the case the diff tool must call out first (§6.3 rule 2).

use std::collections::BTreeMap;

use predictable_ir::run::{Emit, OnTrap, Retain, StoragePrecision};
use serde::{Deserialize, Serialize};

use crate::outbound::error::{IoOutError, Result};
use crate::outbound::hash::digest_bytes;
use crate::outbound::schema::FORMAT;

/// `kind` of `manifest.json`.
pub const KIND_MANIFEST: &str = "manifest";

/// Fields excluded from `manifest_digest` (§6.3 rule 1), stated once so the rule has exactly one
/// implementation.
pub const MANIFEST_DIGEST_EXCLUDED: &[&str] = &[
    "manifest_digest",
    "run_id",
    "execution",
    "results.digest",
    "provenance.user",
    // §6.3 rule 2: the environment travels in `environment_hash` instead, so that "same inputs,
    // different machine" is a *comparison the diff can make* rather than two unrelated digests.
    "environment",
    "environment_hash",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Versions {
    pub ir_version: String,
    pub engine_version: String,
    pub engine_git_sha: Option<String>,
    pub engine_build_profile: String,
    pub cli_version: Option<String>,
    pub dsl_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRef {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInputs {
    pub module: String,
    pub product: Option<String>,
    /// `model_digest` — canonical text of all modules in path order (IR §9.6).
    pub digest: String,
    /// Sorted by path (§6.3 rule 3).
    pub files: Vec<FileRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssumptionInputs {
    pub set: String,
    pub path: String,
    pub digest: String,
    pub values_digest: Option<String>,
}

/// Where a modelpoint file came from before it was a modelpoint file — for a Prophet migration,
/// the `.MPF` and its digest. Absent provenance is stated, never implied (§6.3 rule 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub system: String,
    pub file: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelpointInputs {
    pub path: String,
    pub digest: String,
    pub rows: u64,
    pub key_field: String,
    pub source: Option<SourceRef>,
}

/// A table as it was actually read, plus its copy inside the run directory (Q13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableInput {
    pub name: String,
    pub path: String,
    /// `fs` | `resource` | `inline`.
    pub resolver: String,
    /// Digest of the bytes the resolver returned — the table's identity (IR §2.9.1).
    pub digest: String,
    /// What the model declared. Unequal to `digest` means drift.
    pub declared_digest: Option<String>,
    pub drift: bool,
    /// `run/tables/<name>.parquet`, or `null` under `--no-table-copy`, in which case a consumer
    /// must say "table content unavailable in this run" rather than render a partial row.
    pub copy: Option<String>,
    pub copy_digest: Option<String>,
    pub rows: Option<u64>,
}

impl TableInput {
    /// A table read from the filesystem with no copy taken (`--no-table-copy`).
    pub fn new(
        name: impl Into<String>,
        path: impl Into<String>,
        digest: impl Into<String>,
    ) -> TableInput {
        let digest = digest.into();
        TableInput {
            name: name.into(),
            path: path.into(),
            resolver: "fs".to_string(),
            declared_digest: Some(digest.clone()),
            digest,
            drift: false,
            copy: None,
            copy_digest: None,
            rows: None,
        }
    }

    /// Record the copy this run wrote. `copy_digest` is over the copy's bytes, not the source's.
    pub fn with_copy(
        mut self,
        copy: impl Into<String>,
        copy_digest: impl Into<String>,
        rows: u64,
    ) -> TableInput {
        self.copy = Some(copy.into());
        self.copy_digest = Some(copy_digest.into());
        self.rows = Some(rows);
        self
    }

    /// True when the bytes read differ from the digest the model declared.
    pub fn recompute_drift(&mut self) {
        self.drift = matches!(&self.declared_digest, Some(d) if *d != self.digest);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inputs {
    pub model: ModelInputs,
    pub assumptions: Option<AssumptionInputs>,
    pub modelpoints: ModelpointInputs,
    /// Sorted by name (§6.3 rule 3).
    pub tables: Vec<TableInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineInfo {
    pub basis: String,
    pub periods: u32,
    pub origin: String,
    pub valuation_date: Option<String>,
    pub year_convention: Option<String>,
}

/// The solve outcome block typed by IR §8.4.4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolveOutcome {
    pub name: String,
    pub target: String,
    pub to: f64,
    pub vary: String,
    pub scope: String,
    pub method: String,
    pub tolerance: f64,
    pub max_iter: u32,
    pub bracket: Option<[f64; 2]>,
    pub converged: u64,
    pub not_converged: u64,
    pub iterations: IterationStats,
    pub residual: ResidualStats,
    pub per_mp: Option<FileRef>,
    pub on_not_converged: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IterationStats {
    pub min: u32,
    pub max: u32,
    pub mean: f64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResidualStats {
    pub max_abs: f64,
    pub argmax_mp: Option<String>,
}

/// Non-semantic execution settings. Excluded from `run_digest` (IR §8.4.2) but recorded, because
/// "which machine, how many threads" is evidence even when it cannot change a number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecInfo {
    pub threads: u32,
    pub chunk_size: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunConfigInfo {
    pub path: String,
    /// `run_digest` — IR §8.4.2, excludes `exec` and `out`.
    pub digest: String,
    pub emit: Emit,
    pub outputs: Vec<String>,
    pub retain: Retain,
    /// Q15: `"f64"` only; `"f32"` is `E0108` and [`Manifest::build`] refuses it.
    pub storage_precision: StoragePrecision,
    pub on_trap: OnTrap,
    pub max_errors: u32,
    pub aggregations: Vec<String>,
    pub solves: Vec<SolveOutcome>,
    pub exec: ExecInfo,
    pub seed: Option<u64>,
    pub flags: BTreeMap<String, bool>,
}

/// A single varied input of a sensitivity child run (Q12). `from`/`to` are canonical text form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariedInput {
    /// Dotted path into the run's inputs: `assumptions.<name>`, `run.tables.<name>`,
    /// `run.<field>`, `modelpoints`.
    pub path: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

/// `lineage` (Q12, IR §9.4.1). A sensitivity fan is N runs that share a `group_id`; it is never
/// inferred from filenames.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Lineage {
    pub parent_run: Option<String>,
    pub parent_manifest_digest: Option<String>,
    pub group_id: Option<String>,
    pub label: Option<String>,
    pub varied: Vec<VariedInput>,
}

impl Lineage {
    /// A base run: no parent, no group, nothing varied.
    pub fn base() -> Lineage {
        Lineage::default()
    }

    /// A child of `parent_run`, in fan `group_id`.
    pub fn child(
        parent_run: impl Into<String>,
        parent_manifest_digest: impl Into<String>,
        group_id: impl Into<String>,
        label: impl Into<String>,
        varied: Vec<VariedInput>,
    ) -> Lineage {
        Lineage {
            parent_run: Some(parent_run.into()),
            parent_manifest_digest: Some(parent_manifest_digest.into()),
            group_id: Some(group_id.into()),
            label: Some(label.into()),
            varied,
        }
    }

    /// True for a run that has a parent. A fan member with no `group_id` is legal but lonely.
    pub fn is_child(&self) -> bool {
        self.parent_run.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloatSettings {
    pub fma_contraction: bool,
    pub fast_math: bool,
    pub reduction_order: String,
}

impl Default for FloatSettings {
    fn default() -> Self {
        FloatSettings {
            fma_contraction: false,
            fast_math: false,
            reduction_order: "sequential_t".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub os: String,
    pub os_version: Option<String>,
    pub arch: String,
    pub cpu_features: Vec<String>,
    pub rustc: Option<String>,
    pub target_triple: String,
    pub float_settings: FloatSettings,
    pub python: Option<String>,
    /// Sorted by name — a `BTreeMap` is the sort.
    pub packages: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregatesRef {
    pub path: String,
    pub digest: String,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultsRef {
    pub path: String,
    /// Excluded from `manifest_digest`: it is an *outcome*, and the digest covers *inputs*.
    pub digest: String,
    pub rows: u64,
    pub components: usize,
    pub component_set_digest: String,
    pub aggregates: Option<AggregatesRef>,
}

/// `execution.outcome` (IR §9.3.1). Load-bearing: a consumer that ignores it will eventually
/// report on a truncated portfolio as though it were whole (§6.3 rule 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    CompletedWithTraps,
    Aborted,
    Cancelled,
}

impl Outcome {
    /// Everything except `Completed` demands a banner on every derived report.
    pub fn needs_banner(self) -> bool {
        !matches!(self, Outcome::Completed)
    }

    /// An aborted run has a manifest and no `results.parquet` (§6.3 rule 6).
    pub fn has_results(self) -> bool {
        !matches!(self, Outcome::Aborted)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarningCount {
    pub code: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Execution {
    pub outcome: Outcome,
    pub started_at: String,
    pub finished_at: String,
    pub wall_ms: u64,
    pub modelpoints_projected: u64,
    pub modelpoints_trapped: u64,
    /// The `E0902` envelopes of IR §9.3.1, capped by `--max-errors`.
    pub traps: Vec<serde_json::Value>,
    pub warnings: Vec<WarningCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitInfo {
    pub repo: Option<String>,
    pub sha: Option<String>,
    /// `true` is rendered as a warning banner in every derived report (§6.3 rule 4).
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub invocation: String,
    pub cwd_git: Option<GitInfo>,
    /// Excluded from `manifest_digest`.
    pub user: Option<String>,
}

/// The manifest itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub kind: String,
    /// Filled in by [`Manifest::build`]; empty until then.
    pub manifest_digest: String,
    pub run_id: String,
    /// `"predictable"`, or `"prophet"` for an imported run (§6.3 rule 5).
    pub system: String,
    pub versions: Versions,
    pub inputs: Inputs,
    pub timeline: TimelineInfo,
    pub run_config: RunConfigInfo,
    pub lineage: Lineage,
    pub environment_hash: String,
    pub environment: Environment,
    pub results: ResultsRef,
    pub execution: Execution,
    pub provenance: Provenance,
}

/// The subset of `results` that enters `manifest_digest` — everything but `digest`.
#[derive(Serialize)]
struct ResultsDigestView<'a> {
    path: &'a str,
    rows: u64,
    components: usize,
    component_set_digest: &'a str,
    aggregates: &'a Option<AggregatesRef>,
}

/// The subset of `provenance` that enters `manifest_digest` — everything but `user`.
#[derive(Serialize)]
struct ProvenanceDigestView<'a> {
    invocation: &'a str,
    cwd_git: &'a Option<GitInfo>,
}

/// The digest-participating projection of a manifest, in spec key order.
#[derive(Serialize)]
struct ManifestDigestView<'a> {
    format: &'a str,
    kind: &'a str,
    system: &'a str,
    versions: &'a Versions,
    inputs: &'a Inputs,
    timeline: &'a TimelineInfo,
    run_config: &'a RunConfigInfo,
    lineage: &'a Lineage,
    results: ResultsDigestView<'a>,
    provenance: ProvenanceDigestView<'a>,
}

impl Environment {
    /// What this build can honestly say about the machine it is running on. Fields it cannot
    /// know (`python`, package versions) are left for the caller: absent provenance is stated,
    /// never invented.
    pub fn detect() -> Environment {
        Environment {
            os: std::env::consts::OS.to_string(),
            os_version: None,
            arch: std::env::consts::ARCH.to_string(),
            cpu_features: Vec::new(),
            rustc: None,
            target_triple: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
            float_settings: FloatSettings::default(),
            python: None,
            packages: BTreeMap::new(),
        }
    }
}

impl Manifest {
    /// An unstamped manifest: `lineage` is a base run, `environment` is this machine, and both
    /// digests are empty until [`Manifest::build`] fills them in.
    #[allow(clippy::too_many_arguments)]
    pub fn draft(
        run_id: impl Into<String>,
        versions: Versions,
        inputs: Inputs,
        timeline: TimelineInfo,
        run_config: RunConfigInfo,
        results: ResultsRef,
        execution: Execution,
        provenance: Provenance,
    ) -> Manifest {
        Manifest {
            format: FORMAT.to_string(),
            kind: KIND_MANIFEST.to_string(),
            manifest_digest: String::new(),
            run_id: run_id.into(),
            system: "predictable".to_string(),
            versions,
            inputs,
            timeline,
            run_config,
            lineage: Lineage::base(),
            environment_hash: String::new(),
            environment: Environment::detect(),
            results,
            execution,
            provenance,
        }
    }

    /// Normalise ordering, reject unsupported storage precision, then stamp `environment_hash`
    /// and `manifest_digest`. Call this exactly once, after every field is populated; the two
    /// digests are only meaningful on a finished manifest.
    pub fn build(mut self) -> Result<Manifest> {
        if !self.run_config.storage_precision.is_supported_in_1_0() {
            return Err(IoOutError::UnsupportedStoragePrecision);
        }
        self.format = FORMAT.to_string();
        self.kind = KIND_MANIFEST.to_string();
        self.normalise();
        self.environment_hash =
            digest_bytes(crate::outbound::canonical_json(&self.environment)?.as_bytes());
        self.manifest_digest = String::new();
        self.manifest_digest = self.compute_digest()?;
        Ok(self)
    }

    /// §6.3 rule 3: files by path, tables by name, packages by name (the `BTreeMap` sorts
    /// itself), warnings by code.
    fn normalise(&mut self) {
        self.inputs.model.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.inputs.tables.sort_by(|a, b| a.name.cmp(&b.name));
        self.execution.warnings.sort_by(|a, b| a.code.cmp(&b.code));
    }

    /// `manifest_digest` over the canonical JSON of the digest view (§6.3 rule 1).
    pub fn compute_digest(&self) -> Result<String> {
        let view = ManifestDigestView {
            format: &self.format,
            kind: &self.kind,
            system: &self.system,
            versions: &self.versions,
            inputs: &self.inputs,
            timeline: &self.timeline,
            run_config: &self.run_config,
            lineage: &self.lineage,
            results: ResultsDigestView {
                path: &self.results.path,
                rows: self.results.rows,
                components: self.results.components,
                component_set_digest: &self.results.component_set_digest,
                aggregates: &self.results.aggregates,
            },
            provenance: ProvenanceDigestView {
                invocation: &self.provenance.invocation,
                cwd_git: &self.provenance.cwd_git,
            },
        };
        Ok(digest_bytes(
            crate::outbound::canonical_json(&view)?.as_bytes(),
        ))
    }

    /// Re-derive `manifest_digest` and compare — what `predictable rerun` does before it trusts a
    /// manifest, and what CI asserts across platforms.
    pub fn verify_digest(&self) -> Result<bool> {
        Ok(self.compute_digest()? == self.manifest_digest)
    }

    /// Canonical JSON text of the whole manifest: spec key order, two-space indent, LF, one
    /// trailing newline.
    pub fn to_canonical_json(&self) -> Result<String> {
        crate::outbound::canonical_json(self)
    }

    pub fn from_json(text: &str) -> Result<Manifest> {
        Ok(serde_json::from_str(text)?)
    }

    /// `varied` derived by diffing two manifests, so a hand-run child's lineage is never the only
    /// record of what changed (IR §9.4.1). Compares the digest-bearing inputs and the run digest.
    pub fn derive_varied(parent: &Manifest, child: &Manifest) -> Vec<VariedInput> {
        let mut out = Vec::new();
        let s = |v: &str| serde_json::Value::String(v.to_string());
        if parent.inputs.model.digest != child.inputs.model.digest {
            out.push(VariedInput {
                path: "model".to_string(),
                from: s(&parent.inputs.model.digest),
                to: s(&child.inputs.model.digest),
            });
        }
        match (&parent.inputs.assumptions, &child.inputs.assumptions) {
            (Some(p), Some(c)) if p.digest != c.digest => out.push(VariedInput {
                path: format!("assumptions.{}", c.set),
                from: s(&p.digest),
                to: s(&c.digest),
            }),
            _ => {}
        }
        if parent.inputs.modelpoints.digest != child.inputs.modelpoints.digest {
            out.push(VariedInput {
                path: "modelpoints".to_string(),
                from: s(&parent.inputs.modelpoints.digest),
                to: s(&child.inputs.modelpoints.digest),
            });
        }
        for c in &child.inputs.tables {
            if let Some(p) = parent.inputs.tables.iter().find(|t| t.name == c.name) {
                if p.digest != c.digest {
                    out.push(VariedInput {
                        path: format!("run.tables.{}", c.name),
                        from: s(&p.digest),
                        to: s(&c.digest),
                    });
                }
            }
        }
        if parent.run_config.digest != child.run_config.digest {
            out.push(VariedInput {
                path: "run".to_string(),
                from: s(&parent.run_config.digest),
                to: s(&child.run_config.digest),
            });
        }
        out
    }
}
