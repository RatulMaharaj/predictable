//! The human rendering of a trace (`04-verify.md` §3.5).
//!
//! This module is a **pure projection**: it reads a [`Trace`] and nothing else.
//! No fact appears here that is absent from the JSON, and the same tree always
//! renders identically — which is what lets a rendered trace be committed as a
//! golden test.
//!
//! Formatting rules, from §3.5: values right-aligned and thousands-separated,
//! 2 dp for `money`, 6 significant figures for `prob` and `factor`, the unit and
//! timing after the value, and the span at the right margin. Colour is never the
//! sole carrier of meaning, so there is none here at all.

use crate::explain::{Node, NodeKind, Trace};

/// Default terminal width; `--width` overrides it.
pub const DEFAULT_WIDTH: usize = 100;

/// Render a trace at the default width.
pub fn render(trace: &Trace) -> String {
    render_with_width(trace, DEFAULT_WIDTH)
}

/// Render a trace at a given width.
pub fn render_with_width(trace: &Trace, width: usize) -> String {
    let width = width.max(48);
    let mut out = String::new();
    let root = &trace.root;

    // The header line: what was asked for, for whom, and what it came to.
    let head = format!(
        "{}{}  {}  = {}",
        root.id.clone().unwrap_or_default(),
        root.t.map(|t| format!("[t={t}]")).unwrap_or_default(),
        root.modelpoint
            .as_ref()
            .map(|m| m.key.clone())
            .unwrap_or_default(),
        value_text(root.value, root.unit.as_deref()),
    );
    let mut tail = String::new();
    if let Some(u) = &root.unit {
        tail.push_str(u);
    }
    if let Some(t) = &root.timing {
        tail.push(' ');
        tail.push_str(t);
    }
    let span = root
        .span
        .as_ref()
        .map(|s| {
            let file = s.file.rsplit('/').next().unwrap_or(&s.file);
            format!("{file}:{}", s.line)
        })
        .unwrap_or_default();
    out.push_str(&pad_line(&format!("{head}  {tail}"), &span, width));
    out.push('\n');
    if let Some(expr) = &root.expr {
        out.push_str(&format!("│ {expr}\n│\n"));
    }

    for (i, child) in root.children.iter().enumerate() {
        let last = i + 1 == root.children.len();
        node_lines(child, "", last, width, &mut out);
    }

    if !trace.notes.is_empty() {
        out.push_str("│\nnotes:\n");
        for note in &trace.notes {
            let t = note.t.map(|t| format!("t={t}  ")).unwrap_or_default();
            out.push_str(&format!(
                "  {}  {}  {}{}\n",
                note.code, note.component, t, note.message
            ));
        }
    }
    if trace.truncated.elided_nodes > 0 {
        out.push_str(&format!(
            "\n{} node(s) elided at depth {}; re-run with --depth -1 for the whole tree\n",
            trace.truncated.elided_nodes, trace.truncated.depth
        ));
    }
    out
}

fn node_lines(node: &Node, prefix: &str, last: bool, width: usize, out: &mut String) {
    let branch = if last { "└─ " } else { "├─ " };
    let left = format!("{prefix}{branch}{}", label(node));
    let right = right_margin(node);
    out.push_str(&pad_line(&left, &right, width));
    out.push('\n');

    let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
    for (i, child) in node.children.iter().enumerate() {
        node_lines(
            child,
            &child_prefix,
            i + 1 == node.children.len(),
            width,
            out,
        );
    }
    // An `Agg` prints its first and last contributions: the discount exponent is
    // the bug, and it has to be visible without opening the JSON.
    if node.node == NodeKind::Agg && !node.terms.is_empty() {
        let shown: Vec<usize> = if node.terms.len() <= 6 {
            (0..node.terms.len()).collect()
        } else {
            let n = node.terms.len();
            vec![0, 1, 2, n - 3, n - 2, n - 1]
        };
        let mut previous: Option<usize> = None;
        for i in shown {
            if previous.is_some_and(|p| i > p + 1) {
                out.push_str(&format!("{child_prefix}   ...\n"));
            }
            let term = &node.terms[i];
            let disc = term
                .disc
                .map(|d| format!("  disc {}", six_sf(d)))
                .unwrap_or_default();
            let excluded = if term.included { "" } else { "  (excluded)" };
            out.push_str(&format!(
                "{child_prefix}   t={:<4} {:>16}{disc}  → {}{excluded}\n",
                term.t,
                thousands(term.value, 2),
                thousands(term.contribution, 2),
            ));
            previous = Some(i);
        }
        if node.terms_truncated {
            out.push_str(&format!(
                "{child_prefix}   ({} term(s) in total; --trace-max-terms truncated the middle)\n",
                node.term_count.unwrap_or(node.terms.len())
            ));
        }
    }
}

/// The left-hand label of one node — its variant, spelled the way an actuary
/// reads it rather than the way the JSON tags it.
fn label(node: &Node) -> String {
    let name = node
        .reference
        .clone()
        .or(node.id.clone())
        .unwrap_or_default();
    // A `str`/`enum` lane is a dictionary code (`03-engine.md` §4.2), and the
    // code is not what the model author wrote, so the text wins where there is one.
    let value = match &node.text {
        Some(text) => format!("\"{text}\""),
        None => value_text(node.value, node.unit.as_deref()),
    };
    match node.node {
        NodeKind::Component => format!(
            "{name}{}  {value}",
            node.t.map(|t| format!("[t={t}]")).unwrap_or_default()
        ),
        NodeKind::Binary => format!("{}  {value}", node.op.clone().unwrap_or_default()),
        NodeKind::Unary => format!("{}  {value}", node.op.clone().unwrap_or_default()),
        NodeKind::Ref => format!("{name}{}  {value}", opt_t(node)),
        NodeKind::Lag => format!(
            "{name}[t-{}]{}  {value}",
            node.lag.unwrap_or(0),
            opt_t(node)
        ),
        NodeKind::At => format!("{name}[{}]  {value}", node.at.unwrap_or(0)),
        NodeKind::Input => {
            let kind = node.kind.clone().unwrap_or_default().to_lowercase();
            let set = node
                .source
                .as_ref()
                .and_then(|s| s.assumption_set.clone())
                .map(|s| format!("[{s}]"))
                .unwrap_or_default();
            format!("{name}  {kind}{set}  {value}")
        }
        NodeKind::Lit => value,
        NodeKind::If => format!("if → {}  {value}", node.taken.clone().unwrap_or_default()),
        NodeKind::Lookup => {
            let keys = node
                .keys
                .iter()
                .map(|k| {
                    if k.fired {
                        format!("{}={} → {}", k.name, k.requested, k.resolved)
                    } else {
                        format!("{}={}", k.name, k.requested)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}@({keys})  {value}",
                node.table.clone().unwrap_or_default()
            )
        }
        NodeKind::Call => format!("{}(…)  {value}", node.func.clone().unwrap_or_default()),
        NodeKind::Agg => format!(
            "{}({name})  {} term(s)  {value}",
            node.op.clone().or(node.func.clone()).unwrap_or_default(),
            node.term_count.unwrap_or(node.terms.len())
        ),
    }
}

/// The right margin: unit, timing, resolution, note code and span — in that
/// order, so the column reads as one thing per position.
fn right_margin(node: &Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(u) = &node.unit {
        if u != "none" {
            parts.push(u.clone());
        }
    }
    if let Some(t) = &node.timing {
        parts.push(t.clone());
    }
    if let Some(r) = &node.resolution {
        if r != "computed" {
            parts.push(format!("({r})"));
        }
    }
    if let Some(n) = &node.note {
        parts.push(n.clone());
    }
    if let Some(e) = node.elided {
        parts.push(format!("+{e} elided"));
    }
    if let Some(s) = &node.span {
        let file = s.file.rsplit('/').next().unwrap_or(&s.file);
        parts.push(format!("{file}:{}", s.line));
    }
    parts.join("  ")
}

/// `  t=4` for a series read, nothing at all for a value that has no period —
/// a `t` on a `PerMP` would invite a reader to think it varies.
fn opt_t(node: &Node) -> String {
    node.t.map(|t| format!("  t={t}")).unwrap_or_default()
}

fn pad_line(left: &str, right: &str, width: usize) -> String {
    if right.is_empty() {
        return left.to_string();
    }
    let used = left.chars().count() + right.chars().count();
    if used + 2 > width {
        format!("{left}  {right}")
    } else {
        format!("{left}{}{right}", " ".repeat(width - used))
    }
}

/// `04-verify.md` §3.5: 2 dp for money, 6 significant figures for probabilities
/// and factors, plain otherwise.
fn value_text(value: f64, unit: Option<&str>) -> String {
    match unit.unwrap_or("") {
        "money" => thousands(value, 2),
        "prob" | "factor" | "rate" => six_sf(value),
        u if u.starts_with("rate(") => six_sf(value),
        _ => thousands(value, 4)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string(),
    }
}

fn six_sf(value: f64) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let text = format!("{value:.6e}");
    // Keep the readable form when the magnitude is ordinary; fall back to
    // scientific only where a fixed rendering would be all zeros or unreadable.
    let magnitude = value.abs();
    if (1e-4..1e7).contains(&magnitude) {
        let digits = (6 - (magnitude.log10().floor() as i32 + 1)).clamp(0, 12) as usize;
        format!("{value:.digits$}")
    } else {
        text
    }
}

/// Thousands separators, fixed decimals, and no locale anywhere: the rendering
/// is a golden test, so it may not depend on the machine it runs on.
fn thousands(value: f64, dp: usize) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    let text = format!("{:.dp$}", value.abs());
    let (int_part, frac) = match text.split_once('.') {
        Some((i, f)) => (i.to_string(), format!(".{f}")),
        None => (text, String::new()),
    };
    let mut grouped = String::new();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let sign = if value.is_sign_negative() && value != 0.0 {
        "-"
    } else {
        ""
    };
    format!("{sign}{grouped}{frac}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_are_grouped_and_signed() {
        assert_eq!(thousands(1284.5, 2), "1,284.50");
        assert_eq!(thousands(-1_000_000.0, 2), "-1,000,000.00");
        assert_eq!(thousands(0.0, 2), "0.00");
    }

    #[test]
    fn six_significant_figures_for_probabilities() {
        assert_eq!(six_sf(0.00214), "0.00214000");
        assert_eq!(six_sf(1.035), "1.03500");
    }

    #[test]
    fn padding_never_loses_the_right_margin() {
        let line = pad_line("a", "model.pir:1", 20);
        assert!(line.ends_with("model.pir:1"));
        assert_eq!(line.chars().count(), 20);
        let tight = pad_line(&"x".repeat(30), "model.pir:1", 20);
        assert!(tight.ends_with("  model.pir:1"));
    }
}
