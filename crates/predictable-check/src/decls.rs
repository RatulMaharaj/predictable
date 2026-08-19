//! Declaration-level checks that need the *raw* key/value spans: timing tags
//! (§2.5) and table `source` sandboxing (§2.9.1).
//!
//! These read `predictable_syntax::raw` rather than the typed AST because the
//! diagnostic has to caret the offending **value** — `timing = "average"` points
//! at `average`, not at the component — and the typed AST has already thrown the
//! bad value away.

use std::path::{Component as PathComponent, Path};

use predictable_diagnostics::Diagnostic;
use predictable_syntax::raw::{RawDocument, Value};
use predictable_syntax::source::{SourceMap, Span};

use crate::emit::{self, diagnostic, inner};

const TIMING_TAGS: &[&str] = &["start", "end", "mid", "point"];

/// Spans the parser may also have reported on; the checker owns the pass-5
/// reading of them, so its own report supersedes the parser's.
#[derive(Debug, Default)]
pub struct Superseded {
    pub spans: Vec<Span>,
}

impl Superseded {
    pub fn covers(&self, span: Span) -> bool {
        self.spans
            .iter()
            .any(|s| s.file == span.file && span.start >= s.start && span.end <= s.end)
    }
}

/// §2.5: a `Series` carries a timing tag, nothing else does, and the tag is one
/// of four words.
pub fn timing(
    map: &SourceMap,
    raw: &RawDocument,
    out: &mut Vec<Diagnostic>,
    superseded: &mut Superseded,
) {
    for section in raw.sections_named("component") {
        let table = &section.table;
        let shape = match table.get("shape").map(|v| &v.value) {
            Some(Value::Str(s)) => s.clone(),
            _ => continue,
        };
        let raw_name_span = table
            .get("name")
            .map(|v| v.span)
            .unwrap_or(section.header_span);
        let name_span = inner(raw_name_span);
        let name = match table.get("name").map(|v| &v.value) {
            Some(Value::Str(s)) => s.clone(),
            _ => "this component".to_string(),
        };
        let timing = table.get("timing");

        match (shape.as_str(), timing) {
            ("Series", None) => {
                superseded.spans.push(raw_name_span);
                out.push(
                    diagnostic("E0507", "a `Series` component must declare a `timing`")
                        .span(emit::primary(
                            map,
                            name_span,
                            format!("`{name}` is a Series, so it needs one of: start, end, mid, point"),
                        ))
                        .note(
                            "Timing is not decoration: `npv` uses it to pick the discount exponent \
                             (start → v^t, end → v^(t+1), mid → v^(t+0.5), point → v^t), which is \
                             where the classic off-by-one-period error lives (01-ir.md §2.5).",
                        )
                        .suggestion(emit::maybe(emit::replace(
                            map,
                            name_span,
                            format!("{name}\"\ntiming = \"end"),
                            "tag it as an in-arrears flow",
                        ))),
                );
            }
            ("Series", Some(value)) => {
                if let Value::Str(tag) = &value.value {
                    if !TIMING_TAGS.contains(&tag.as_str()) {
                        let span = inner(value.span);
                        superseded.spans.push(value.span);
                        out.push(
                            diagnostic("E0508", format!("unknown timing tag `{tag}`"))
                                .span(emit::primary(
                                    map,
                                    span,
                                    "timing is one of: start, end, mid, point",
                                ))
                                .note(
                                    "`mid` is the uniform / mid-period approximation; a bespoke \
                                     timing is expressed by retiming the flow, not by inventing a \
                                     tag (01-ir.md §2.5).",
                                )
                                .suggestion(emit::maybe(emit::replace(
                                    map,
                                    span,
                                    "mid",
                                    "did you mean `mid`?",
                                ))),
                        );
                    }
                }
            }
            (_, Some(value)) => {
                let span = inner(value.span);
                superseded.spans.push(value.span);
                if let Some(key) = section.table.key_span("timing") {
                    superseded.spans.push(key);
                }
                out.push(
                    diagnostic(
                        "E0506",
                        "`timing` is only meaningful for shape = \"Series\"",
                    )
                    .span(emit::primary(
                        map,
                        span,
                        format!("`{name}` is a {shape}, and a {shape} has no timing"),
                    ))
                    .note(
                        "A Scalar or PerMP value is not realised within a period, so there is \
                         nothing for a timing tag to say about it (01-ir.md §2.5).",
                    )
                    .suggestion(emit::delete(
                        map,
                        Span::new(
                            value.span.file,
                            emit::line_range(map, value.span).start,
                            emit::line_range(map, value.span).end,
                        ),
                        "remove the `timing` key",
                    )),
                );
            }
            _ => {}
        }
    }
}

/// §2.9.1: `source` resolves relative to the declaring `.pir`, sandboxed to the
/// project root. An absolute path, or one that climbs out of the root, is
/// `E0801` — a `.pir` file may not read arbitrary bytes off the machine that
/// checks it.
pub fn table_sources(map: &SourceMap, raw: &RawDocument, out: &mut Vec<Diagnostic>) {
    for section in raw.sections_named("table") {
        let Some(value) = section.table.get("source") else {
            continue;
        };
        let Value::Str(source) = &value.value else {
            continue;
        };
        if source == "inline" || source.contains(':') {
            // `inline` and `resource:` never touch the filesystem.
            continue;
        }
        let span = inner(value.span);
        let declaring_dir = Path::new(map.name(value.span.file))
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let absolute = Path::new(source).is_absolute();
        if !absolute && !escapes(&declaring_dir, source) {
            continue;
        }
        let name = match section.table.get("name").map(|v| &v.value) {
            Some(Value::Str(s)) => s.clone(),
            _ => "this table".to_string(),
        };
        out.push(
            diagnostic("E0801", "table source escapes the project root")
                .span(emit::primary(
                    map,
                    span,
                    if absolute {
                        "an absolute path is never resolvable on another machine"
                    } else {
                        "this climbs out of the directory the model lives in"
                    },
                ))
                .note(
                    "`source` is resolved relative to the directory of the declaring .pir file and \
                     is sandboxed to the project root — the nearest ancestor with a \
                     predictable.toml, else the module's own directory (01-ir.md §2.9.1).",
                )
                .suggestion(emit::maybe(emit::replace(
                    map,
                    span,
                    format!("tables/{name}.csv"),
                    "move the file under the model and reference it relatively",
                )))
                .suggestion(emit::maybe(emit::replace(
                    map,
                    span,
                    format!("resource:{name}"),
                    "or have the host supply the bytes through a MapResolver",
                ))),
        );
    }
}

/// Does `source`, resolved against `base`, leave `base`'s root?
fn escapes(base: &Path, source: &str) -> bool {
    let mut depth: i32 = base.components().count() as i32;
    for component in Path::new(source).components() {
        match component {
            PathComponent::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            PathComponent::CurDir => {}
            PathComponent::RootDir | PathComponent::Prefix(_) => return true,
            PathComponent::Normal(_) => depth += 1,
        }
    }
    false
}

/// The span of a key's *value* inside the `[[<section>]]` block whose `name` is
/// `name` — the span a suggestion must replace to change, say, `unit = "none"`
/// into `unit = "money"` without leaving the old key behind.
pub fn value_span(raw: &RawDocument, section: &str, name: &str, key: &str) -> Option<Span> {
    raw.sections_named(section)
        .find(|s| matches!(s.table.get("name").map(|v| &v.value), Some(Value::Str(n)) if n == name))
        .and_then(|s| s.table.get(key))
        .map(|v| inner(v.span))
}
