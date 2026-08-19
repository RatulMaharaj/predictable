//! The terminal rendering of §5.7.
//!
//! Deterministic to the byte for a given [`crate::RunDiff`]: no timestamps, no colour, no
//! locale-dependent formatting. `--json` suppresses this entirely — the two must never both go to
//! stdout, because a consumer that has to find the JSON inside a report is a consumer that will
//! eventually parse the report.

use crate::model::{Class, Verdict};
use crate::tolerance::round_half_away;
use crate::RunDiff;

/// Render the whole report.
pub fn render(diff: &RunDiff) -> String {
    let mut out = String::new();
    header(diff, &mut out);
    out.push('\n');
    verdict_line(diff, &mut out);
    if diff.verdict == Verdict::Incomparable {
        for why in &diff.incomparable {
            out.push_str(&format!("      {why}\n"));
        }
        return out;
    }
    for finding in &diff.findings {
        out.push('\n');
        render_finding(diff, finding, &mut out);
    }
    if let Some((kept, of)) = diff.findings_truncated {
        out.push_str(&format!(
            "\n  … {} more findings ({kept} of {of} shown; --top 0 for all)\n",
            of - kept
        ));
    }
    footer(diff, &mut out);
    out
}

fn header(diff: &RunDiff, out: &mut String) {
    out.push_str(&format!(
        "  a  {:<12} {:<22} {}\n",
        diff.a_system, diff.a_run, diff.a_path
    ));
    out.push_str(&format!(
        "  b  {:<12} {:<22} {}\n",
        diff.b_system, diff.b_run, diff.b_path
    ));
    let p = &diff.tolerances.profile;
    let mut line = format!(
        "  tolerance  {}   abs {}  rel {}",
        p.name, p.base.abs, p.base.rel
    );
    if let Some(m) = &diff.mapping {
        line.push_str(&format!("   mapping {}", m.file));
    }
    out.push_str(&line);
    out.push('\n');

    // §4.4: any mapping entry with a non-default sign/scale/timing_shift is printed in the header.
    if let Some(m) = &diff.mapping {
        for entry in m.adjusted() {
            let mut parts = Vec::new();
            if entry.sign != 1 {
                parts.push(format!("sign {}", entry.sign));
            }
            if entry.scale != 1.0 {
                parts.push(format!("× {}", entry.scale));
            }
            if entry.timing_shift != 0 {
                parts.push(format!("t {:+}", entry.timing_shift));
            }
            out.push_str(&format!(
                "  mapping adjustment     {} → {}  {}\n",
                entry.prophet,
                entry.predictable,
                parts.join("  ")
            ));
        }
    }
    // §5.6: every loosening is printed. Silence here would be the one failure mode that makes a
    // green diff meaningless.
    for o in diff.tolerances.loosenings() {
        out.push_str(&format!(
            "  tolerance loosened     {}  {}  abs {}  rel {}{}\n",
            o.component,
            o.rule.describe(),
            o.tol.abs,
            o.tol.rel,
            o.note
                .as_ref()
                .map(|n| format!("  ({n})"))
                .unwrap_or_default()
        ));
    }
    if let Some(e) = &diff.emit_mismatch {
        // Same `emit` setting on both sides but different component sets is the ordinary Prophet
        // case, and calling that an "emit mismatch" would send a reader looking at the wrong flag.
        let what = if e.a == e.b {
            format!("both `{}`, but the component sets differ", e.a)
        } else {
            format!("a `{}` vs b `{}`", e.a, e.b)
        };
        out.push_str(&format!(
            "  component sets         {what} — diffing the {} in common\n",
            diff.components.common
        ));
    }
    if !diff.graph_available {
        out.push_str(
            "  no model graph         root/inherited is ordered by earliest t, not by the IR\n",
        );
    }
}

fn verdict_line(diff: &RunDiff, out: &mut String) {
    out.push_str(&format!(
        "  {}   {} of {} cells   {} root divergence{}   {} output{} affected\n",
        diff.verdict.word(),
        thousands(diff.cells.diverged),
        thousands(diff.cells.compared),
        diff.root_divergences,
        plural(diff.root_divergences),
        diff.outputs.iter().filter(|o| !o.within_tolerance).count(),
        plural(diff.outputs.iter().filter(|o| !o.within_tolerance).count()),
    ));
}

fn render_finding(diff: &RunDiff, f: &crate::Finding, out: &mut String) {
    let share = f
        .contribution
        .as_ref()
        .map(|c| {
            format!(
                "   {:.0}% of Δ{}",
                c.share_of_total_delta * 100.0,
                bare(&c.output)
            )
        })
        .unwrap_or_default();
    let affects = if f.affects_outputs.is_empty() {
        String::new()
    } else {
        format!(
            "   affects {}",
            f.affects_outputs
                .iter()
                .map(|s| bare(s))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    out.push_str(&format!(
        "  ▸ {}  {}   {}{}{}\n",
        f.id,
        f.class.word(),
        bare(&f.component),
        affects,
        share
    ));
    out.push_str(&format!("      {}\n", f.message));
    if f.class == Class::ToleranceOnly || f.class == Class::Structural {
        return;
    }
    out.push_str(&format!(
        "      first at t={}, persists to t={}, {} of {} modelpoints, {} cells\n",
        f.t_range[0],
        f.t_range[1],
        f.n_modelpoints,
        diff.modelpoints.common,
        thousands(f.n_cells as u64),
    ));
    let w = &f.worst;
    out.push_str(&format!(
        "      exemplar {}      worst {} t={}  {} vs {}  (Δ {}, {:.1}%)\n",
        f.exemplar.mp_key,
        w.mp_key,
        w.t,
        money(w.a.as_ref().and_then(crate::Value::as_f64)),
        money(w.b.as_ref().and_then(crate::Value::as_f64)),
        money(Some(w.abs)),
        w.rel * 100.0
    ));
    match &f.explained_by_model_change {
        Some(e) if e.changed => out.push_str(&format!(
            "      explained by a model change: {}\n",
            e.what.join(", ")
        )),
        Some(_) => {
            out.push_str("      UNEXPLAINED: the model diff reports no change in this component\n")
        }
        None => {}
    }
    for h in &f.hypotheses {
        out.push_str(&format!(
            "      {}  ({} confidence, held on {}/{})  {}\n",
            h.code,
            h.confidence.word(),
            h.evidence_support.held,
            h.evidence_support.tested,
            h.message,
        ));
        if let Some(edit) = &h.suggested_edit {
            out.push_str(&format!(
                "          suggested edit  {}:{}..{}  {}\n",
                edit.file, edit.byte_start, edit.byte_end, edit.message
            ));
            out.push_str(&format!(
                "          - {}\n          + {}\n",
                edit.old, edit.new
            ));
        }
    }
    if !f.explain_command.is_empty() {
        out.push_str(&format!("      → {}\n", f.explain_command));
    }
}

fn footer(diff: &RunDiff, out: &mut String) {
    if !diff.structural.only_in_a.is_empty() || !diff.structural.only_in_b.is_empty() {
        out.push('\n');
        for one in &diff.structural.only_in_a {
            out.push_str(&format!(
                "  only in a   {}  ({})\n",
                one.component, one.reason
            ));
        }
        for one in &diff.structural.only_in_b {
            out.push_str(&format!(
                "  only in b   {}  ({})\n",
                one.component, one.reason
            ));
        }
    }
    let mp = &diff.structural;
    if !mp.modelpoints_only_in_a.is_empty() || !mp.modelpoints_only_in_b.is_empty() {
        out.push_str(&format!(
            "\n  modelpoints  {} only in a, {} only in b\n",
            mp.modelpoints_only_in_a.len(),
            mp.modelpoints_only_in_b.len()
        ));
    }
}

fn bare(id: &str) -> String {
    id.rsplit_once('.')
        .map(|(_, t)| t)
        .unwrap_or(id)
        .to_string()
}

fn money(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{:.2}", round_half_away(x, 2)),
        Some(x) => format!("{x}"),
        None => "<absent>".to_string(),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `1234567 → 1,234,567`. Grouping is fixed, not locale-dependent: the report must be
/// byte-identical wherever it runs.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups_from_the_right() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(19680000), "19,680,000");
    }

    #[test]
    fn money_rounds_half_away_for_display_only() {
        assert_eq!(money(Some(2.675)), "2.68");
        assert_eq!(money(None), "<absent>");
    }
}
