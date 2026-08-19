//! The terminal rendering of a [`ModelDiff`].
//!
//! Prose is never the only place a fact appears — everything here is also in
//! the JSON document — but the prose is what a reviewer reads, so it is ordered
//! the way a review goes: what changed, then what that reaches.

use crate::{ChangeClass, EntryStatus, ModelDiff};

fn classes(list: &[ChangeClass]) -> String {
    list.iter()
        .map(|c| {
            serde_json::to_value(c)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn status(s: &EntryStatus) -> &'static str {
    match s {
        EntryStatus::Added => "added",
        EntryStatus::Removed => "removed",
        EntryStatus::Changed => "changed",
    }
}

/// Render a diff for a terminal.
pub fn render_text(d: &ModelDiff) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} → {}\n", d.a, d.b));
    if d.is_empty() {
        out.push_str("no structural differences\n");
        return out;
    }

    for r in &d.renamed {
        out.push_str(&format!(
            "  ~ renamed {} → {} (by {})\n",
            r.from, r.to, r.evidence
        ));
    }
    for c in &d.added {
        out.push_str(&format!("  + {} [{}]\n", c.name, c.kind));
        if let Some(e) = &c.expr {
            out.push_str(&format!("      expr = {e}\n"));
        }
    }
    for c in &d.removed {
        out.push_str(&format!("  - {} [{}]\n", c.name, c.kind));
    }
    for c in &d.changed {
        out.push_str(&format!("  * {} [{}]\n", c.name, classes(&c.classes)));
        for f in &c.fields {
            out.push_str(&format!("      {}: {} → {}\n", f.field, f.before, f.after));
        }
        for n in c.expr_nodes.iter().chain(&c.init_nodes) {
            out.push_str(&format!(
                "      {} {}: {} → {}\n",
                n.path,
                n.change.describe(),
                n.before,
                n.after
            ));
        }
    }
    for t in &d.tables {
        out.push_str(&format!("  T {} table {}\n", status(&t.status), t.name));
        for f in &t.fields {
            out.push_str(&format!("      {}: {} → {}\n", f.field, f.before, f.after));
        }
        if let Some(rows) = &t.rows {
            out.push_str(&format!(
                "      rows: +{} -{} ~{}\n",
                rows.added.len(),
                rows.removed.len(),
                rows.changed.len()
            ));
            for r in rows.changed.iter().take(5) {
                out.push_str(&format!(
                    "        [{}] {} → {}\n",
                    r.key.join(", "),
                    r.before.join(", "),
                    r.after.join(", ")
                ));
            }
        }
    }
    for a in &d.assumptions {
        out.push_str(&format!("  A {} assumption {}", status(&a.status), a.name));
        match (&a.value_before, &a.value_after) {
            (Some(x), Some(y)) if x != y => out.push_str(&format!(": {x} → {y}")),
            (None, Some(y)) => out.push_str(&format!(": {y}")),
            (Some(x), None) => out.push_str(&format!(": was {x}")),
            _ => {}
        }
        out.push('\n');
        for f in &a.fields {
            out.push_str(&format!("      {}: {} → {}\n", f.field, f.before, f.after));
        }
    }

    let s = &d.summary;
    let change_count = s.added + s.removed + s.changed + s.tables + s.assumptions;
    out.push_str(&format!(
        "\n{change_count} change(s) affect {} downstream component(s); {} output(s) affected",
        s.components_impacted, s.outputs_affected
    ));
    if !d.impact.outputs_affected.is_empty() {
        out.push_str(&format!(": {}", d.impact.outputs_affected.join(", ")));
    }
    out.push('\n');
    if !d.unparsed.is_empty() {
        out.push_str(&format!(
            "warning: {} file(s) did not parse and were skipped: {}\n",
            d.unparsed.len(),
            d.unparsed.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{diff, ModelSide};
    use predictable_ir::{BinaryOp, Component, DType, Expr, Module, Shape};

    fn side(label: &str, op: BinaryOp) -> ModelSide {
        let mut m = Module::new("m");
        m.components.push(Component::derived(
            "x",
            DType::F64,
            Shape::Series,
            Expr::binary(op, Expr::r#ref("a"), Expr::r#ref("b")),
        ));
        let mut s = ModelSide::new(label);
        s.modules.push(m);
        s
    }

    #[test]
    fn identical_models_say_so() {
        let text = render_text(&diff(&side("a", BinaryOp::Mul), &side("b", BinaryOp::Mul)));
        assert!(text.contains("no structural differences"), "{text}");
    }

    #[test]
    fn a_formula_change_prints_its_path_and_both_sides() {
        let text = render_text(&diff(&side("a", BinaryOp::Mul), &side("b", BinaryOp::Div)));
        assert!(text.contains("* x [formula]"), "{text}");
        assert!(
            text.contains("expr operator changed: a * b → a / b"),
            "{text}"
        );
        assert!(
            text.contains("1 change(s) affect 0 downstream component(s)"),
            "{text}"
        );
    }
}
