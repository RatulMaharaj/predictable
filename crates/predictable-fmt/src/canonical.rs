//! The canonical form of a `.pir` file — `01-ir.md` §4.1.
//!
//! The rules, in the spec's own numbering:
//!
//! 1. key order inside a `[[component]]` is fixed;
//! 2. components — and every other block — keep declaration order;
//! 3. expressions are normalised ([`crate::expr_fmt`]);
//! 4. floats round-trip via shortest representation ([`crate::float`]);
//! 5. UTF-8, LF, one trailing newline, no trailing whitespace;
//! 6. the result is a fixed point: `fmt(fmt(x)) == fmt(x)`.
//!
//! Layout rules that §4.1 leaves to the formatter, fixed here so that the
//! conformance corpus (whose inputs are authored canonical) is a fixed point:
//!
//! * one `key = value` per line, `;`-joined pairs split onto their own lines;
//! * exactly one blank line before every `[section]` header, and nowhere else —
//!   a blank line is formatting, and §9.3 rule 6 requires formatting to be
//!   invisible to a digest, which it cannot be if `fmt` preserves it;
//! * comment lines are preserved and printed immediately above the item they
//!   precede, at column 0. Inside a `[[component]]`, whose keys are reordered,
//!   they are hoisted to just below the header — a comment cannot stay attached
//!   to a line that moves;
//! * an array is written on one line unless it has more than one element and
//!   any element is itself an array or an inline table, in which case one
//!   element per line, two-space indent, trailing comma.

use predictable_syntax::raw::{Section, Table};
use predictable_syntax::{parse_raw, Diagnostic, Diagnostics, SourceMap, Spanned, Value};

use crate::expr_fmt::format_expression;
use crate::float::format_f64;

/// Intra-`[[component]]` key order — §4.1 rule 1. Keys not in this list keep
/// their relative order and follow the ones that are.
pub const COMPONENT_KEY_ORDER: &[&str] = &[
    "name", "kind", "dtype", "shape", "unit", "timing", "init", "expr", "doc", "tags", "output",
];

/// Keys whose value is an expression and is therefore normalised by rule 3.
const EXPR_KEYS: &[&str] = &["expr", "init"];

/// A file that could not be formatted because it does not parse. `fmt` never
/// rewrites a file it does not fully understand.
#[derive(Debug, Clone)]
pub struct FmtError {
    pub file: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl std::fmt::Display for FmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: cannot format a file with {} syntax error(s)",
            self.file,
            self.diagnostics.iter().filter(|d| d.is_error()).count()
        )?;
        for d in self.diagnostics.iter().filter(|d| d.is_error()) {
            write!(f, "\n  {} {}", d.code, d.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for FmtError {}

/// What to leave out of the emitted text. Used by `run_digest` (§8.4.2), which
/// hashes the canonical text of a run file *minus* its non-semantic fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter {
    /// Drop `[run.exec]`, `run.out` and `run.progress` — the fields that cannot
    /// change a number in `results.parquet`.
    pub drop_run_non_semantic: bool,
}

/// Canonicalise one `.pir` source text.
pub fn format_source(name: &str, text: &str) -> Result<String, FmtError> {
    format_source_with(name, text, Filter::default())
}

/// Canonicalise with a [`Filter`] applied.
pub fn format_source_with(name: &str, text: &str, filter: Filter) -> Result<String, FmtError> {
    // Rule 5 is a pure text rule and is applied before anything else: line
    // endings become LF and trailing whitespace goes. Doing it up front keeps
    // the rest of the formatter working on clean lines (and avoids handing the
    // lexer a date literal padded with spaces, which it mis-lexes).
    let text = &normalise_lines(text);
    let mut sources = SourceMap::new();
    let file = sources.add(name, text.to_string());
    let mut diags = Diagnostics::new();
    let doc = parse_raw(sources.text(file), file, &mut diags);
    if diags.has_errors() {
        return Err(FmtError {
            file: name.to_string(),
            diagnostics: diags.into_vec(),
        });
    }

    let mut w = Writer::new(text, filter);
    w.write_table_entries(&doc.root, false);
    for section in &doc.sections {
        if filter.drop_run_non_semantic && section.dotted() == "run.exec" {
            continue;
        }
        w.write_section(section);
    }
    if !w.errors.is_empty() {
        return Err(FmtError {
            file: name.to_string(),
            diagnostics: w.errors,
        });
    }
    Ok(w.finish())
}

/// Rule 5's text half: LF line endings, no trailing whitespace on any line,
/// exactly one trailing newline. Line *count* is preserved, so every span still
/// points at the line the author wrote it on.
fn normalise_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n') {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// `true` if `text` is already in canonical form.
pub fn is_canonical(name: &str, text: &str) -> Result<bool, FmtError> {
    Ok(format_source(name, text)? == text)
}

// ---------------------------------------------------------------------------
// writer
// ---------------------------------------------------------------------------

struct Writer<'a> {
    text: &'a str,
    /// Byte offset at which each line starts.
    line_starts: Vec<usize>,
    out: String,
    /// First line not yet considered for trivia.
    cursor: usize,
    /// Comments hoisted out of the component body currently being written.
    suppress_trivia: bool,
    filter: Filter,
    errors: Vec<Diagnostic>,
}

impl<'a> Writer<'a> {
    fn new(text: &'a str, filter: Filter) -> Writer<'a> {
        let mut line_starts = vec![0usize];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Writer {
            text,
            line_starts,
            out: String::with_capacity(text.len() + 64),
            cursor: 0,
            suppress_trivia: false,
            filter,
            errors: Vec::new(),
        }
    }

    fn finish(mut self) -> String {
        // Rule 5: exactly one trailing newline, and no blank line at the end.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn line_of(&self, offset: u32) -> usize {
        match self.line_starts.binary_search(&(offset as usize)) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    fn line_text(&self, line: usize) -> &'a str {
        let start = self.line_starts[line];
        let end = self
            .line_starts
            .get(line + 1)
            .map(|&e| e - 1)
            .unwrap_or(self.text.len());
        self.text[start..end.max(start)].trim_end_matches('\r')
    }

    /// Comment lines in `[from, to)`, and whether any blank line separated them
    /// from the previous item.
    fn scan(&self, from: usize, to: usize) -> (bool, Vec<String>) {
        let mut blank = false;
        let mut comments = Vec::new();
        for line in from..to.min(self.line_starts.len()) {
            let t = self.line_text(line).trim();
            if t.is_empty() {
                blank = true;
            } else if let Some(rest) = t.strip_prefix('#') {
                comments.push(format!("#{}", rest.trim_end()));
            }
        }
        (blank, comments)
    }

    fn push_line(&mut self, s: &str) {
        self.out.push_str(s.trim_end());
        self.out.push('\n');
    }

    fn push_blank(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    // -- sections ----------------------------------------------------------

    fn write_section(&mut self, section: &Section) {
        let header_line = self.line_of(section.header_span.start);
        let (_, comments) = self.scan(self.cursor, header_line);
        let interior = {
            let end = self.line_of(section.header_span.end);
            self.scan(header_line + 1, end + 1).1
        };
        self.push_blank();
        for c in comments.iter().chain(interior.iter()) {
            let c = c.clone();
            self.push_line(&c);
        }
        let brackets = if section.array {
            ("[[", "]]")
        } else {
            ("[", "]")
        };
        let header = format!("{}{}{}", brackets.0, section.path.join("."), brackets.1);
        self.push_line(&header);
        self.cursor = self.line_of(section.header_span.end) + 1;

        let is_component = section.array && section.path == ["component"];
        if is_component {
            // Rule 1 reorders the keys, so comments cannot stay attached to a
            // key line; hoist them all to just under the header.
            let end = section
                .table
                .entries
                .iter()
                .map(|(k, v)| self.line_of(k.span.end.max(v.span.end)))
                .max()
                .unwrap_or(self.cursor);
            let (_, body_comments) = self.scan(self.cursor, end + 1);
            for c in body_comments {
                self.push_line(&c);
            }
            self.suppress_trivia = true;
            self.write_table_entries(&section.table, true);
            self.suppress_trivia = false;
            self.cursor = end + 1;
        } else {
            self.write_table_entries(&section.table, false);
        }
    }

    fn write_table_entries(&mut self, table: &Table, component: bool) {
        let mut order: Vec<usize> = (0..table.entries.len()).collect();
        if component {
            order.sort_by_key(|&i| {
                let key = table.entries[i].0.value.as_str();
                let rank = COMPONENT_KEY_ORDER
                    .iter()
                    .position(|k| *k == key)
                    .unwrap_or(COMPONENT_KEY_ORDER.len() + i);
                (rank, i)
            });
        }
        for i in order {
            let (key, value) = &table.entries[i];
            if self.filter.drop_run_non_semantic && matches!(key.value.as_str(), "out" | "progress")
            {
                continue;
            }
            self.write_entry(key, value, component);
        }
    }

    fn write_entry(&mut self, key: &Spanned<String>, value: &Spanned<Value>, component: bool) {
        let start = self.line_of(key.span.start);
        let end = self.line_of(value.span.end);
        if !self.suppress_trivia {
            let (_blank, comments) = self.scan(self.cursor, start);
            // Comments physically inside a multi-line value move above it.
            let interior = self.scan(start + 1, end + 1).1;
            for c in comments.into_iter().chain(interior) {
                self.push_line(&c);
            }
        }
        let is_expr = component && EXPR_KEYS.contains(&key.value.as_str());
        let rendered = self.render_value(&value.value, is_expr, 0);
        let line = format!("{} = {}", key.value, rendered);
        for l in line.split('\n') {
            self.push_line(l);
        }
        if !self.suppress_trivia {
            self.cursor = end + 1;
        }
    }

    // -- values ------------------------------------------------------------

    fn render_value(&mut self, value: &Value, is_expr: bool, indent: usize) -> String {
        match value {
            Value::Str(s) => {
                let body = if is_expr {
                    let (normalised, diags) = format_expression(s);
                    self.errors
                        .extend(diags.into_iter().filter(|d| d.is_error()));
                    normalised
                } else {
                    s.clone()
                };
                let mut out = String::with_capacity(body.len() + 2);
                out.push('"');
                crate::escape_into(&body, &mut out);
                out.push('"');
                out
            }
            Value::Int(v) => v.to_string(),
            Value::Float(v) => format_f64(*v),
            Value::Bool(v) => v.to_string(),
            Value::Date(d) => d.clone(),
            Value::Table(t) => {
                if t.entries.is_empty() {
                    return "{}".to_string();
                }
                let parts: Vec<String> = t
                    .entries
                    .iter()
                    .map(|(k, v)| {
                        let rendered = self.render_value(&v.value, false, indent);
                        format!("{} = {}", k.value, rendered)
                    })
                    .collect();
                format!("{{ {} }}", parts.join(", "))
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return "[]".to_string();
                }
                let multiline = items.len() > 1
                    && items
                        .iter()
                        .any(|i| matches!(i.value, Value::Array(_) | Value::Table(_)));
                let rendered: Vec<String> = items
                    .iter()
                    .map(|i| self.render_value(&i.value, false, indent + 1))
                    .collect();
                if multiline {
                    let pad = "  ".repeat(indent + 1);
                    let close = "  ".repeat(indent);
                    let body: String = rendered.iter().map(|r| format!("{pad}{r},\n")).collect();
                    format!("[\n{body}{close}]")
                } else {
                    format!("[{}]", rendered.join(", "))
                }
            }
            // Unreachable: a document with a parse error never reaches here.
            Value::Error => "<error>".to_string(),
        }
    }
}
