//! `predictable diff model <A> <B>` — the structural model diff of
//! `01-ir.md` §11.3.
//!
//! The subject word is mandatory and is what keeps this command honest: `diff
//! model` compares two *models*, `diff run` (T26, `04-verify.md` §5) compares
//! two *runs*, and neither is inferred from what the paths happen to look like.
//! Guessing there would mean the same command sometimes compared formulas and
//! sometimes compared numbers.
//!
//! Each side is a path: a `.pir` file, or a directory that is expanded exactly
//! the way `check` and `build` expand one, so `diff model old/ new/` means the
//! same set of files those commands would have read.

use predictable_modeldiff::{diff, report, ModelSide};
use serde_json::json;

use crate::args::Args;
use crate::emit::{Emitter, DOMAIN, OK};
use crate::paths;

/// Flags this subcommand understands.
pub const FLAGS: &[&str] = &["json", "out-json", "fail-on-change"];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &["out-json"];

/// Run `diff model`.
pub fn run(args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    let (a, b) = match args.positional.as_slice() {
        [_subject, a, b] => (a, b),
        [_subject, ..] => return Err("diff model needs exactly two paths".into()),
        _ => return Err("diff model needs two paths".into()),
    };

    let side_a = ModelSide::load(a.clone(), &paths::collect_pir(std::slice::from_ref(a))?)?;
    let side_b = ModelSide::load(b.clone(), &paths::collect_pir(std::slice::from_ref(b))?)?;
    let d = diff(&side_a, &side_b);

    let mut doc = d.to_json();
    doc.as_object_mut()
        .expect("the diff is an object")
        .insert("status".into(), json!("ok"));
    emitter.document("modeldiff", doc)?;
    if !emitter.json {
        emitter.line(report::render_text(&d).trim_end());
    }

    // A difference is a result, not a failure — unless the caller asked for it
    // to be one, which is what a CI gate on "the model did not change" needs.
    if args.flag("fail-on-change") && !d.is_semantically_empty() {
        return Ok(DOMAIN);
    }
    Ok(OK)
}
