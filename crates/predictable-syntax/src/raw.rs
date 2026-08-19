//! The restricted TOML dialect of `.pir` files (`01-ir.md` §4), lexed with
//! `logos` and parsed by hand into a spanned, order-preserving tree.
//!
//! Differences from stock TOML, all deliberate:
//!
//! * `;` separates several key/value pairs on one physical line, which is how
//!   `01-ir.md` §6 writes `name = "x"; dtype = "i64"; required = true`.
//! * Dates are kept as text. The IR needs the literal a human wrote and the
//!   engine's own date type; parsing to a calendar type is not the parser's job.
//! * Bare keys are identifiers. `.pir` never uses numeric or quoted keys.
//!
//! The output of this module is a *raw* document. Nothing here knows what a
//! component is; [`crate::lower`] does that.

use logos::Logos;

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::expr::unescape;
use crate::source::{FileId, Span, Spanned};

#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r]+")]
#[logos(skip r"#[^\n]*")]
enum Tok {
    #[token("\n")]
    Newline,
    #[regex(r"[0-9]{4}-[0-9]{2}-[0-9]{2}([Tt ][0-9:.+\-Zz]+)?", priority = 5)]
    Date,
    #[regex(r"[A-Za-z_][A-Za-z0-9_-]*", priority = 3)]
    Ident,
    #[regex(
        r"[+-]?[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?|[+-]?[0-9]+[eE][+-]?[0-9]+",
        priority = 4
    )]
    Float,
    #[regex(r"[+-]?[0-9]+", priority = 3)]
    Int,
    #[regex(r#""([^"\\\n]|\\.)*""#)]
    Str,
    #[token("=")]
    Equals,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token(";")]
    Semi,
}

/// A raw TOML value, with the span of the syntax that produced it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// A date/date-time literal, kept verbatim.
    Date(String),
    Array(Vec<Spanned<Value>>),
    /// An inline table, `{ a = 1, b = 2 }`.
    Table(Table),
    /// A value that failed to parse; recorded so recovery can continue.
    Error,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Str(_) => "a string",
            Value::Int(_) => "an integer",
            Value::Float(_) => "a float",
            Value::Bool(_) => "a boolean",
            Value::Date(_) => "a date",
            Value::Array(_) => "an array",
            Value::Table(_) => "an inline table",
            Value::Error => "an invalid value",
        }
    }
}

/// An ordered set of key/value pairs. Order is preserved because declaration
/// order is authorial intent and is the tiebreak in `01-ir.md` §3.2.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub entries: Vec<(Spanned<String>, Spanned<Value>)>,
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&Spanned<Value>> {
        self.entries
            .iter()
            .find(|(k, _)| k.value == key)
            .map(|(_, v)| v)
    }

    pub fn key_span(&self, key: &str) -> Option<Span> {
        self.entries
            .iter()
            .find(|(k, _)| k.value == key)
            .map(|(k, _)| k.span)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.value.as_str())
    }
}

/// A `[header]` or `[[header]]` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Dotted header path, e.g. `["run", "exec"]`.
    pub path: Vec<String>,
    /// `true` for `[[array of tables]]`.
    pub array: bool,
    pub header_span: Span,
    pub table: Table,
}

impl Section {
    pub fn is(&self, name: &str) -> bool {
        self.path.len() == 1 && self.path[0] == name
    }

    pub fn dotted(&self) -> String {
        self.path.join(".")
    }
}

/// A whole `.pir` file, before it means anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawDocument {
    pub file: Option<FileId>,
    /// Key/value pairs before the first header.
    pub root: Table,
    pub sections: Vec<Section>,
}

impl RawDocument {
    /// All sections with a given single-segment header, in file order.
    pub fn sections_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections.iter().filter(move |s| s.is(name))
    }

    /// The first section with a given dotted header, if any.
    pub fn section(&self, dotted: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.dotted() == dotted)
    }
}

struct Parser<'a> {
    text: &'a str,
    toks: Vec<Tok>,
    ranges: Vec<(usize, usize)>,
    pos: usize,
    file: FileId,
}

/// Lex and parse a `.pir` file into a [`RawDocument`], reporting into `diags`.
///
/// Recovery: a malformed line is reported once and skipped to the next newline;
/// a malformed header is reported and the parser resynchronises on the next
/// header. One call therefore reports every broken line in the file.
pub fn parse_raw(text: &str, file: FileId, diags: &mut Diagnostics) -> RawDocument {
    let mut lexer = Tok::lexer(text);
    let mut toks = Vec::new();
    let mut ranges = Vec::new();
    while let Some(res) = lexer.next() {
        let r = lexer.span();
        match res {
            Ok(t) => {
                toks.push(t);
                ranges.push((r.start, r.end));
            }
            Err(_) => {
                let span = Span::new(file, r.start, r.end);
                let bad = &text[r.start..r.end];
                let (code, msg) = if bad.starts_with('"') {
                    ("E0002", "unterminated string".to_string())
                } else {
                    ("E0001", format!("unexpected character `{bad}`"))
                };
                diags.push(Diagnostic::error(code, msg).with_primary(span, "not valid here"));
            }
        }
    }
    let mut p = Parser {
        text,
        toks,
        ranges,
        pos: 0,
        file,
    };
    p.parse_document(diags)
}

impl<'a> Parser<'a> {
    fn span(&self, i: usize) -> Span {
        match self.ranges.get(i) {
            Some(&(s, e)) => Span::new(self.file, s, e),
            None => {
                let end = self.text.len();
                Span::new(self.file, end.saturating_sub(1), end)
            }
        }
    }

    fn cur_span(&self) -> Span {
        self.span(self.pos)
    }

    fn prev_span(&self) -> Span {
        self.span(self.pos.saturating_sub(1))
    }

    fn peek(&self) -> Option<Tok> {
        self.toks.get(self.pos).copied()
    }

    fn slice(&self, i: usize) -> &'a str {
        let (s, e) = self.ranges[i];
        &self.text[s..e]
    }

    fn cur_text(&self) -> &'a str {
        self.slice(self.pos)
    }

    fn eat(&mut self, tok: Tok) -> bool {
        if self.peek() == Some(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(Tok::Newline) {
            self.pos += 1;
        }
    }

    /// Recovery: drop everything up to and including the next newline.
    fn skip_line(&mut self) {
        while let Some(t) = self.peek() {
            self.pos += 1;
            if t == Tok::Newline {
                return;
            }
        }
    }

    fn parse_document(&mut self, diags: &mut Diagnostics) -> RawDocument {
        let mut doc = RawDocument {
            file: Some(self.file),
            ..Default::default()
        };
        self.skip_newlines();
        // Root key/value pairs, up to the first header.
        self.parse_entries(&mut doc.root, diags);
        while self.peek().is_some() {
            match self.parse_section(diags) {
                Some(section) => doc.sections.push(section),
                None => self.skip_line(),
            }
            self.skip_newlines();
        }
        doc
    }

    /// Key/value pairs until a header or EOF.
    fn parse_entries(&mut self, table: &mut Table, diags: &mut Diagnostics) {
        loop {
            self.skip_newlines();
            match self.peek() {
                None | Some(Tok::LBracket) => return,
                Some(Tok::Ident) => {}
                Some(_) => {
                    let span = self.cur_span();
                    diags.push(
                        Diagnostic::error("E0003", "expected a key")
                            .with_primary(span, "a `key = value` line was expected here"),
                    );
                    self.skip_line();
                    continue;
                }
            }
            // one or more `key = value` pairs, `;`-separated, on this line
            let mut line_broken = false;
            loop {
                if self.peek() != Some(Tok::Ident) {
                    break;
                }
                let key_span = self.cur_span();
                let key = self.cur_text().to_string();
                self.pos += 1;
                if !self.eat(Tok::Equals) {
                    let span = self.cur_span();
                    diags.push(
                        Diagnostic::error("E0004", format!("expected `=` after key `{key}`"))
                            .with_primary(span, "expected `=`")
                            .with_suggestion(span.at_start(), "= ", "insert the assignment"),
                    );
                    self.skip_line();
                    line_broken = true;
                    break;
                }
                let value = self.parse_value(diags);
                if let Some(prev) = table.key_span(&key) {
                    diags.push(
                        Diagnostic::error("E0005", format!("duplicate key `{key}`"))
                            .with_primary(key_span, "redefined here")
                            .with_secondary(prev, "first defined here"),
                    );
                }
                table.entries.push((Spanned::new(key, key_span), value));
                if !self.eat(Tok::Semi) {
                    break;
                }
            }
            if line_broken {
                continue;
            }
            // the line must end here
            match self.peek() {
                None => return,
                Some(Tok::Newline) => {
                    self.pos += 1;
                }
                Some(_) => {
                    let span = self.cur_span();
                    diags.push(
                        Diagnostic::error("E0006", "unexpected trailing input on this line")
                            .with_primary(span, "expected a newline or `;` here"),
                    );
                    self.skip_line();
                }
            }
        }
    }

    fn parse_section(&mut self, diags: &mut Diagnostics) -> Option<Section> {
        let start = self.cur_span();
        if !self.eat(Tok::LBracket) {
            let span = self.cur_span();
            diags.push(
                Diagnostic::error("E0007", "expected a `[section]` header")
                    .with_primary(span, "not a header"),
            );
            return None;
        }
        let array = self.eat(Tok::LBracket);
        let mut path = Vec::new();
        loop {
            if self.peek() != Some(Tok::Ident) {
                let span = self.cur_span();
                diags.push(
                    Diagnostic::error("E0008", "expected a name in the section header")
                        .with_primary(span, "expected a name"),
                );
                return None;
            }
            path.push(self.cur_text().to_string());
            self.pos += 1;
            if !self.eat(Tok::Dot) {
                break;
            }
        }
        if !self.eat(Tok::RBracket) || (array && !self.eat(Tok::RBracket)) {
            let span = self.cur_span();
            let close = if array { "]]" } else { "]" };
            diags.push(
                Diagnostic::error(
                    "E0009",
                    format!("unclosed section header, expected `{close}`"),
                )
                .with_primary(span, "expected the closing bracket")
                .with_suggestion(span.at_start(), close, "close the header"),
            );
            return None;
        }
        let header_span = start.to(self.prev_span());
        if !matches!(self.peek(), None | Some(Tok::Newline)) {
            let span = self.cur_span();
            diags.push(
                Diagnostic::error("E0006", "unexpected input after a section header")
                    .with_primary(span, "a header must be alone on its line"),
            );
            self.skip_line();
        }
        let mut table = Table::default();
        self.parse_entries(&mut table, diags);
        Some(Section {
            path,
            array,
            header_span,
            table,
        })
    }

    fn parse_value(&mut self, diags: &mut Diagnostics) -> Spanned<Value> {
        let span = self.cur_span();
        match self.peek() {
            Some(Tok::Str) => {
                let text = self.cur_text();
                self.pos += 1;
                Spanned::new(Value::Str(unescape(&text[1..text.len() - 1])), span)
            }
            Some(Tok::Int) => {
                let text = self.cur_text();
                self.pos += 1;
                match text.parse::<i64>() {
                    Ok(v) => Spanned::new(Value::Int(v), span),
                    Err(_) => {
                        diags.push(
                            Diagnostic::error(
                                "E0010",
                                format!("integer `{text}` does not fit in i64"),
                            )
                            .with_primary(span, "out of range"),
                        );
                        Spanned::new(Value::Error, span)
                    }
                }
            }
            Some(Tok::Float) => {
                let text = self.cur_text();
                self.pos += 1;
                Spanned::new(Value::Float(text.parse::<f64>().unwrap_or(f64::NAN)), span)
            }
            Some(Tok::Date) => {
                let text = self.cur_text().to_string();
                self.pos += 1;
                Spanned::new(Value::Date(text), span)
            }
            Some(Tok::Ident) => {
                let text = self.cur_text();
                self.pos += 1;
                match text {
                    "true" => Spanned::new(Value::Bool(true), span),
                    "false" => Spanned::new(Value::Bool(false), span),
                    other => {
                        diags.push(
                            Diagnostic::error("E0011", format!("`{other}` is not a value"))
                                .with_primary(span, "bare words are not values")
                                .with_suggestion(
                                    span,
                                    format!("\"{other}\""),
                                    "quote it if a string was meant",
                                ),
                        );
                        Spanned::new(Value::Error, span)
                    }
                }
            }
            Some(Tok::LBracket) => self.parse_array(diags),
            Some(Tok::LBrace) => self.parse_inline_table(diags),
            _ => {
                diags.push(
                    Diagnostic::error("E0012", "expected a value")
                        .with_primary(span, "a value was expected after `=`"),
                );
                Spanned::new(Value::Error, span)
            }
        }
    }

    fn parse_array(&mut self, diags: &mut Diagnostics) -> Spanned<Value> {
        let start = self.cur_span();
        self.pos += 1; // `[`
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == Some(Tok::RBracket) || self.peek().is_none() {
                break;
            }
            items.push(self.parse_value(diags));
            self.skip_newlines();
            if !self.eat(Tok::Comma) {
                self.skip_newlines();
                break;
            }
        }
        if !self.eat(Tok::RBracket) {
            let span = self.cur_span();
            diags.push(
                Diagnostic::error("E0013", "unclosed array")
                    .with_primary(span, "expected `]` or `,`"),
            );
            return Spanned::new(Value::Error, start.to(span));
        }
        Spanned::new(Value::Array(items), start.to(self.prev_span()))
    }

    fn parse_inline_table(&mut self, diags: &mut Diagnostics) -> Spanned<Value> {
        let start = self.cur_span();
        self.pos += 1; // `{`
        let mut table = Table::default();
        loop {
            self.skip_newlines();
            if self.peek() == Some(Tok::RBrace) || self.peek().is_none() {
                break;
            }
            if self.peek() != Some(Tok::Ident) {
                let span = self.cur_span();
                diags.push(
                    Diagnostic::error("E0003", "expected a key in the inline table")
                        .with_primary(span, "expected a key"),
                );
                break;
            }
            let key_span = self.cur_span();
            let key = self.cur_text().to_string();
            self.pos += 1;
            if !self.eat(Tok::Equals) {
                let span = self.cur_span();
                diags.push(
                    Diagnostic::error("E0004", format!("expected `=` after key `{key}`"))
                        .with_primary(span, "expected `=`"),
                );
                break;
            }
            let value = self.parse_value(diags);
            table.entries.push((Spanned::new(key, key_span), value));
            self.skip_newlines();
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        self.skip_newlines();
        if !self.eat(Tok::RBrace) {
            let span = self.cur_span();
            diags.push(
                Diagnostic::error("E0014", "unclosed inline table")
                    .with_primary(span, "expected `}` or `,`"),
            );
            return Spanned::new(Value::Error, start.to(span));
        }
        Spanned::new(Value::Table(table), start.to(self.prev_span()))
    }
}
