//! The commands whose implementations are later phases: `diff` and `migrate`.
//!
//! `explain` graduated out of here in T23 and lives in [`crate::cmd::explain`].
//!
//! They are present, documented and *machine-readable in their refusal*. An agent
//! driving the loop of `04-verify.md` §7 on exit codes must be able to tell "this
//! build cannot do that yet" from "your model is wrong", and a command that simply
//! did not exist would be indistinguishable from a typo. So each stub exits 2 (a
//! structural failure, not a domain one) and, under `--json`, says which task owns
//! the real implementation.

use serde_json::json;

use crate::args::Args;
use crate::emit::{Emitter, USAGE};

/// Flags every stub accepts, so a caller's real invocation is parsed rather than
/// rejected for a flag the finished command will understand.
pub const FLAGS: &[&str] = &[
    "json",
    "out-json",
    "out",
    "tolerance-profile",
    "mp",
    "t",
    "c",
    "component",
    "format",
    "from",
    "to",
];
/// Options that take a value.
pub const VALUE_FLAGS: &[&str] = &[
    "out-json",
    "out",
    "tolerance-profile",
    "mp",
    "t",
    "c",
    "component",
    "format",
    "from",
    "to",
];

/// What a stub says about itself.
pub struct Stub {
    /// Subcommand name.
    pub name: &'static str,
    /// Backlog task that owns the implementation.
    pub owner: &'static str,
    /// Spec section that specifies it.
    pub spec: &'static str,
    /// One line of what it will do.
    pub summary: &'static str,
}

/// `predictable diff` — `04-verify.md` §5.
pub const DIFF: Stub = Stub {
    name: "diff",
    // Both diffs exist; what does not exist is a `diff` that guesses which one you meant. The
    // subject word is mandatory, so `diff A B` lands here — a refusal a machine can read.
    owner: "T26 — say `predictable diff run A B` (numbers) or `diff model A B` (formulas)",
    spec: "04-verify.md §5",
    summary: "diff needs a subject word: `run` or `model`",
};

/// `predictable migrate` — `04-verify.md` §8.2.
pub const MIGRATE: Stub = Stub {
    name: "migrate",
    owner: "T24 (readers) / T33 (skill)",
    spec: "04-verify.md §4, §8.2",
    summary: "drive the Prophet → predictable migration loop",
};

/// Report a stub.
pub fn run(stub: &Stub, args: &Args, emitter: &mut Emitter) -> Result<i32, String> {
    args.reject_unknown(FLAGS)?;
    emitter.document(
        stub.name,
        json!({
            "status": "unimplemented",
            "command": stub.name,
            "summary": stub.summary,
            "spec": stub.spec,
            "implemented_by": stub.owner,
            "exit_code": USAGE,
        }),
    )?;
    emitter.note(format!(
        "`predictable {}` is not implemented in this build: {} ({}, {}).",
        stub.name, stub.summary, stub.spec, stub.owner
    ));
    Ok(USAGE)
}
