//! `.pir` files → checked sources → a plan → tapes.
//!
//! Every command that needs more than text goes through here, so `check`, `build`,
//! `graph` and `run` cannot disagree about what "this model" means: the same file
//! expansion, the same canonicalisation for `model_digest`, the same checker, the
//! same planner options.

use std::path::PathBuf;

use predictable_check::{check, CheckResult, Input};
use predictable_fmt::digest;
use predictable_ir::Module;
use predictable_plan::{plan_sources, Plan, PlanOptions};
use predictable_tape::{lower_plan, TapeProgram};

use crate::paths;

/// The model's text, as read and as checked.
#[derive(Debug)]
pub struct Sources {
    /// Files in path order — the order `model_digest` hashes in.
    pub files: Vec<PathBuf>,
    /// `(display path, text)` for each file, in the same order.
    pub inputs: Vec<Input>,
}

impl Sources {
    /// Read and expand the paths given on the command line.
    pub fn load(argv_paths: &[String]) -> Result<Sources, String> {
        let files = paths::collect_pir(argv_paths)?;
        let mut inputs = Vec::with_capacity(files.len());
        for file in &files {
            inputs.push(Input::new(file.display().to_string(), paths::read(file)?));
        }
        Ok(Sources { files, inputs })
    }

    /// Run the checker.
    pub fn check(&self) -> CheckResult {
        check(&self.inputs)
    }

    /// `model_digest` over the canonical text of every file (`01-ir.md` §9.6).
    ///
    /// A file that does not parse has no canonical form, so the digest is
    /// `None` rather than a hash of something that is not the model.
    pub fn model_digest(&self) -> Option<String> {
        let mut canonical = Vec::with_capacity(self.inputs.len());
        for input in &self.inputs {
            let text = predictable_fmt::canonical::format_source(&input.name, &input.text).ok()?;
            canonical.push(digest::CanonicalFile {
                path: input.name.clone(),
                text,
            });
        }
        Some(digest::model_digest(&canonical))
    }

    /// Per-file digest of the canonical text, for `manifest.inputs.model.files`.
    pub fn file_digests(&self) -> Vec<(String, String)> {
        self.inputs
            .iter()
            .map(|i| {
                let text = predictable_fmt::canonical::format_source(&i.name, &i.text)
                    .unwrap_or_else(|_| i.text.clone());
                (i.name.clone(), digest::digest_bytes(text.as_bytes()))
            })
            .collect()
    }
}

/// A model compiled as far as the tapes.
#[derive(Debug)]
pub struct Compiled {
    /// The planner's output.
    pub plan: Plan,
    /// The lowered tapes.
    pub tapes: TapeProgram,
    /// The IR modules, in file order.
    pub modules: Vec<Module>,
    /// `model_digest`, or empty when a file did not canonicalise.
    pub model_digest: String,
}

/// Plan and lower already-checked sources.
///
/// The checker runs first and its errors are the caller's to report: the planner
/// refuses unchecked input by design (`PlanError::NotChecked`), and turning that
/// refusal into a second, differently worded diagnostic set is exactly the
/// divergence the split exists to prevent.
pub fn compile(sources: &Sources, options: &PlanOptions) -> Result<Compiled, String> {
    let mut options = options.clone();
    let model_digest = sources.model_digest().unwrap_or_default();
    if options.program_digest.is_empty() {
        options.program_digest = model_digest.clone();
    }
    let plan = plan_sources(&sources.inputs, &options).map_err(|e| e.to_string())?;
    let tapes = lower_plan(&plan).map_err(|e| format!("lowering: {e}"))?;
    let modules = predictable_plan::lower_modules(&sources.inputs);
    Ok(Compiled {
        plan,
        tapes,
        modules,
        model_digest,
    })
}
