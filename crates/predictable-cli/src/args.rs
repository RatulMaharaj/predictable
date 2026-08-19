//! A very small argument parser.
//!
//! The CLI deliberately has no third-party argument crate. The surface is fixed
//! by `04-verify.md` §7 and `05-viz.md` §1.4 — a closed set of subcommands, each
//! with a closed set of flags — so the parser's whole job is to refuse anything
//! outside that set with exit code 2 rather than to be extensible.
//!
//! Two forms are accepted for a value option: `--out dir` and `--out=dir`. A
//! bare `--flag` is a boolean. Anything else is a positional. An unknown flag is
//! an error, never a positional: a typo must not silently become a file name.

use std::collections::{BTreeMap, BTreeSet};

/// Parsed arguments for one subcommand.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Args {
    /// Positional arguments, in order.
    pub positional: Vec<String>,
    flags: BTreeSet<String>,
    opts: BTreeMap<String, String>,
}

impl Args {
    /// Parse `argv` for a subcommand whose value-taking options are `takes_value`.
    ///
    /// Names are given without the leading dashes.
    pub fn parse(argv: &[String], takes_value: &[&str]) -> Result<Args, String> {
        let mut out = Args::default();
        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if let Some(rest) = a.strip_prefix("--") {
                let (name, inline) = match rest.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (rest, None),
                };
                if takes_value.contains(&name) {
                    let value = match inline {
                        Some(v) => v,
                        None => {
                            i += 1;
                            argv.get(i)
                                .cloned()
                                .ok_or_else(|| format!("`--{name}` needs a value"))?
                        }
                    };
                    out.opts.insert(name.to_string(), value);
                } else if inline.is_some() {
                    return Err(format!("`--{name}` does not take a value"));
                } else {
                    out.flags.insert(name.to_string());
                }
            } else {
                out.positional.push(a.clone());
            }
            i += 1;
        }
        Ok(out)
    }

    /// Whether a boolean flag was given.
    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    /// The value of an option, if given.
    pub fn opt(&self, name: &str) -> Option<&str> {
        self.opts.get(name).map(String::as_str)
    }

    /// An option parsed as a `u32`.
    pub fn opt_u32(&self, name: &str) -> Result<Option<u32>, String> {
        match self.opt(name) {
            None => Ok(None),
            Some(v) => v
                .parse::<u32>()
                .map(Some)
                .map_err(|_| format!("`--{name}` wants a whole number, got `{v}`")),
        }
    }

    /// Reject any flag or option outside `allowed` — the check that makes a typo
    /// a usage error instead of a silent default.
    pub fn reject_unknown(&self, allowed: &[&str]) -> Result<(), String> {
        for name in self.flags.iter().chain(self.opts.keys()) {
            if !allowed.contains(&name.as_str()) {
                return Err(format!("unknown flag `--{name}`"));
            }
        }
        Ok(())
    }

    /// The single positional this subcommand requires.
    pub fn one_positional(&self, what: &str) -> Result<&str, String> {
        match self.positional.len() {
            1 => Ok(&self.positional[0]),
            0 => Err(format!("expected {what}")),
            n => Err(format!("expected one {what}, got {n}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn both_value_forms_parse_the_same() {
        let a = Args::parse(&argv(&["--out", "dir", "x.pir"]), &["out"]).unwrap();
        let b = Args::parse(&argv(&["--out=dir", "x.pir"]), &["out"]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.opt("out"), Some("dir"));
        assert_eq!(a.positional, vec!["x.pir".to_string()]);
    }

    #[test]
    fn a_missing_value_is_an_error_not_a_default() {
        let e = Args::parse(&argv(&["--out"]), &["out"]).unwrap_err();
        assert!(e.contains("needs a value"), "{e}");
    }

    #[test]
    fn a_typo_is_never_silently_a_positional() {
        let args = Args::parse(&argv(&["--jsonn", "m.pir"]), &[]).unwrap();
        assert_eq!(args.positional, vec!["m.pir".to_string()]);
        let e = args.reject_unknown(&["json"]).unwrap_err();
        assert_eq!(e, "unknown flag `--jsonn`");
    }

    #[test]
    fn a_value_on_a_boolean_flag_is_refused() {
        let e = Args::parse(&argv(&["--json=yes"]), &[]).unwrap_err();
        assert!(e.contains("does not take a value"), "{e}");
    }

    #[test]
    fn numeric_options_are_validated() {
        let args = Args::parse(&argv(&["--threads", "four"]), &["threads"]).unwrap();
        assert!(args.opt_u32("threads").is_err());
        let args = Args::parse(&argv(&["--threads", "4"]), &["threads"]).unwrap();
        assert_eq!(args.opt_u32("threads").unwrap(), Some(4));
    }
}
