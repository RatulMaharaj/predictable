//! Regenerate the code table in the diagnostics catalogue.
//!
//! ```console
//! $ cargo run -p predictable-diagnostics --example generate_catalogue > /tmp/codes.md
//! ```
//!
//! The output is the section of `docs/llm/diagnostics.md` that starts at
//! `<!-- BEGIN GENERATED CODES -->`. The prose around it is written by hand; the
//! anchors and titles come from the registry so the docs cannot drift from the
//! code (`tests/registry.rs` asserts it).

use predictable_diagnostics::registry::{codes_in, Namespace};

fn main() {
    println!("<!-- BEGIN GENERATED CODES -->");
    for namespace in Namespace::all() {
        println!("\n## {}\n", namespace.title());
        for info in codes_in(*namespace) {
            println!("<a id=\"{}\"></a>", info.code);
            println!("### `{}` — {}\n", info.code, info.title);
            println!(
                "**Severity:** {} &nbsp;·&nbsp; **Specified in:** {}\n",
                info.severity.as_str(),
                info.spec
            );
        }
    }
    println!("<!-- END GENERATED CODES -->");
}
