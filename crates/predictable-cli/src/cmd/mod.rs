//! One module per subcommand. Each exports `FLAGS` (everything it accepts),
//! `VALUE_FLAGS` (the subset that takes a value) and `run`.

pub mod build;
pub mod check;
pub mod diff;
pub mod digest;
pub mod explain;
pub mod export;
pub mod fmt;
pub mod graph;
pub mod rerun;
pub mod run;
pub mod rundiff;
pub mod serve;
pub mod stub;
