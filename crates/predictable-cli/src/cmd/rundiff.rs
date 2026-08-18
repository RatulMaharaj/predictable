//! `predictable diff run <A> <B>` — the run diff of `04-verify.md` §5.
//!
//! The subject word is what keeps `diff` honest: `diff model` compares two *models*, `diff run`
//! compares two *runs*, and neither is inferred from what the paths happen to look like.
//!
//! Either side may be a Prophet run: a directory the importer wrote, or the `.rpt` itself, which
//! this command imports into a scratch run directory first. After that there is one comparison
//! path, because a Prophet run is a run.
//!
//! Under `--json` the `pvf/1` `rundiff` document is the *only* thing on stdout. The two renderings
//! must never both go there: a consumer that has to find the JSON inside a report is a consumer
//! that will eventually parse the report.

use std::path::{Path, PathBuf};

use predictable_rundiff::render;
use predictable_rundiff::tolerance::{Profile, Tol};
use predictable_rundiff::{diff_runs, DiffOptions, Mapping, RunSide, Verdict};

use crate::args::Args;
use crate::emit::{Emitter, DOMAIN, OK, USAGE};

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &[
    "json",
    "out-json",
    "tolerance-profile",
    "abs",
    "rel",
    "mapping",
    "top",
    "component",
    "mp",
    "fail-on",
    "require-same-emit",
    "explain-tolerance",
    "no-source-precision",
    "model-a",
    "model-b",
    "period-base",
];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &[
    "out-json",
    "tolerance-profile",
    "abs",
    "rel",
    "mapping",
    "top",
    "component",
    "mp",
    "fail-on",
    "model-a",
    "model-b",
    "period-base",
];

/// Run `diff run`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let (a_path, b_path) = match args.positional.as_slice() {
        [_subject, a, b] => (a, b),
        [_subject, ..] => return Err("diff run needs exactly two run directories".into()),
        _ => return Err("diff run needs two run directories".into()),
    };

    let mapping = match args.opt("mapping") {
        Some(path) => Some(Mapping::load(path).map_err(|e| e.to_string())?),
        None => None,
    };
    let period_base = match args.opt("period-base") {
        Some(v) => Some(
            v.parse::<i64>()
                .map_err(|_| format!("`--period-base` wants 0 or 1, got `{v}`"))?,
        ),
        None => mapping.as_ref().and_then(|m| m.period_base).map(i64::from),
    };

    let a = load_side("a", a_path, period_base, mapping.as_ref())?;
    let b = load_side("b", b_path, period_base, mapping.as_ref())?;

    let mut opts = DiffOptions {
        profile: profile(args)?,
        mapping,
        top: args.opt_u32("top")?.unwrap_or(20) as usize,
        filter_component: args.opt("component").map(str::to_string),
        filter_mp: args.opt("mp").map(str::to_string),
        require_same_emit: args.flag("require-same-emit"),
        use_source_precision: !args.flag("no-source-precision"),
        model_a: None,
        model_b: None,
        source_b: Default::default(),
    };
    // Explicit model paths win; otherwise each side's own manifest is asked where its model is.
    // A model that cannot be found is not an error — it costs the IR-based classification and the
    // model-diff attribution, and the report says which of those it lost.
    opts = opts.with_models_from(&a, &b);
    let explicit_a = args.opt("model-a").map(collect).transpose()?;
    let explicit_b = args.opt("model-b").map(collect).transpose()?;
    opts = opts.with_model_paths(explicit_a.as_deref(), explicit_b.as_deref());

    let diff = diff_runs(&a, &b, &opts);

    let mut doc = diff.to_json();
    doc.as_object_mut()
        .expect("the diff is an object")
        .remove("format");
    doc.as_object_mut()
        .expect("the diff is an object")
        .remove("kind");
    emitter.document("rundiff", doc)?;
    if !emitter.json {
        emitter.line(render::render(&diff).trim_end());
        if args.flag("explain-tolerance") {
            emitter.line("");
            emitter.line("  tolerance by component");
            for (component, resolved) in &diff.tolerances.resolutions {
                emitter.line(format!(
                    "    {component:<40} abs {:<12} rel {:<12} {}",
                    resolved.tol.abs,
                    resolved.tol.rel,
                    resolved.rule.describe()
                ));
            }
        }
    }

    exit_code(&diff, args.opt("fail-on").unwrap_or("any"))
}

/// Exit `0` all within tolerance, `1` divergence beyond tolerance, `2` structurally incomparable.
///
/// `--fail-on` narrows what counts as a failure without ever hiding it from the report: the
/// findings are identical, only the code changes.
fn exit_code(diff: &predictable_rundiff::RunDiff, fail_on: &str) -> Result<i32, String> {
    if diff.verdict == Verdict::Incomparable {
        return Ok(USAGE);
    }
    match fail_on {
        "any" => Ok(diff.verdict.exit_code()),
        "root" => Ok(if diff.root_divergences > 0 {
            DOMAIN
        } else {
            OK
        }),
        "never" => Ok(OK),
        other => Err(format!(
            "`--fail-on` wants any | root | never, got `{other}`"
        )),
    }
}

fn profile(args: &Args) -> Result<Profile, String> {
    let abs = parse_f64(args, "abs")?;
    let rel = parse_f64(args, "rel")?;
    let mut profile = match args.opt("tolerance-profile") {
        Some(name) => Profile::builtin(name).map_err(|e| e.to_string())?,
        // §5.6: `reconcile` is the default for a Prophet diff, and the only profile whose
        // per-unit table is right for money without the user writing one.
        None => Profile::builtin("reconcile").map_err(|e| e.to_string())?,
    };
    if abs.is_some() || rel.is_some() {
        // An explicit `--abs`/`--rel` is the user overruling the profile, so the profile's
        // per-unit table goes with it. Leaving `money = 0.005` in place would silently ignore a
        // `--abs 1e12` on every money component, which is exactly the kind of invisible tolerance
        // decision §5.6 forbids.
        profile = Profile {
            name: format!("{} (custom)", profile.name),
            base: Tol::new(
                abs.unwrap_or(profile.base.abs),
                rel.unwrap_or(profile.base.rel),
            ),
            money_dp: profile.money_dp,
            by_unit: Default::default(),
            by_component: Default::default(),
        };
    }
    Ok(profile)
}

fn parse_f64(args: &Args, name: &str) -> Result<Option<f64>, String> {
    match args.opt(name) {
        None => Ok(None),
        Some(v) => v
            .parse::<f64>()
            .map(Some)
            .map_err(|_| format!("`--{name}` wants a number, got `{v}`")),
    }
}

fn collect(path: &str) -> Result<Vec<PathBuf>, String> {
    crate::paths::collect_pir(std::slice::from_ref(&path.to_string()))
}

/// Load one side: a run directory, or a Prophet `.rpt` imported into a scratch run directory.
fn load_side(
    label: &str,
    path: &str,
    period_base: Option<i64>,
    mapping: Option<&Mapping>,
) -> Result<RunSide, String> {
    let p = Path::new(path);
    if p.is_file() {
        let is_rpt = p
            .extension()
            .map(|e| e.eq_ignore_ascii_case("rpt"))
            .unwrap_or(false);
        if !is_rpt {
            return Err(format!(
                "{path}: a run diff takes run directories, or a Prophet `.rpt`"
            ));
        }
        return import_rpt(label, p, period_base, mapping);
    }
    RunSide::load(label, p).map_err(|e| e.to_string())
}

fn import_rpt(
    label: &str,
    path: &Path,
    period_base: Option<i64>,
    mapping: Option<&Mapping>,
) -> Result<RunSide, String> {
    use predictable_prophet::rpt::{read_rpt, RptOptions};
    use predictable_prophet::run_dir::{write_run_dir, ImportOptions};

    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let options = RptOptions {
        period_base,
        timeline_basis: None,
        mp_key_column: mapping
            .and_then(|m| m.mp_key.as_ref())
            .map(|k| k.prophet.clone()),
        component_names: Default::default(),
    };
    let read = read_rpt(&bytes, &path.display().to_string(), &options);
    if read.has_errors() {
        let first = read
            .diagnostics
            .iter()
            .find(|d| format!("{:?}", d.severity).contains("Error"))
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "the .rpt could not be read".to_string());
        return Err(format!("{}: {first}", path.display()));
    }
    // The scratch directory is derived from the file's own path, so two runs of the same command
    // reuse it and a `.rpt`-vs-run diff is as repeatable as a directory-vs-directory one.
    let dir = std::env::temp_dir().join(format!(
        "predictable-rpt-{}-{}",
        label,
        &predictable_rundiff::sha256_hex(&bytes)[..16]
    ));
    let _ = std::fs::remove_dir_all(&dir);
    write_run_dir(
        &read.value,
        &dir,
        &bytes,
        &ImportOptions {
            module: mapping.and_then(|m| m.model_module.clone()),
            source_path: Some(path.display().to_string()),
            invocation: Some("predictable diff run".to_string()),
        },
    )
    .map_err(|e| e.to_string())?;
    RunSide::load(label, &dir).map_err(|e| e.to_string())
}
