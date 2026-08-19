//! The infix expression language of `01-ir.md` §4.2, its arena, and its parser.
//!
//! Expressions live inside `.pir` strings (`expr = "num_pols_if * qx"`). They are
//! parsed into an [`ExprArena`]: a flat `Vec` of nodes addressed by a `u32`
//! [`ExprId`], with spans held in a side table so the hot structural data stays
//! compact and so a formatting-only edit touches spans and nothing else.

use logos::Logos;

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::source::{FileId, Span};

// ---------------------------------------------------------------------------
// nodes
// ---------------------------------------------------------------------------

/// Index of a node in an [`ExprArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExprId(pub u32);

impl ExprId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A literal value. `Int`/`Float`/`Bool`/`Str` are unit-polymorphic (§2.4); the
/// checker unifies them with their context.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }
}

/// An expression node. Mirrors `01-ir.md` §2.6, except that aggregates are not a
/// distinct syntactic form — they are spelled as calls (`npv(x, disc)`), so they
/// parse as [`Expr::Call`] and are identified by [`is_agg_fn`].
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Lit),
    /// `x` — same period, same modelpoint.
    Ref(String),
    /// `x[t-k]`, `k >= 1`.
    Lag {
        name: String,
        k: u32,
    },
    /// `x[k]`, absolute index, `k >= 0`.
    At {
        name: String,
        k: u32,
    },
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Binary {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    If {
        cond: ExprId,
        then_: ExprId,
        else_: ExprId,
    },
    /// A builtin call (§2.8). The parser does not check the name or the arity.
    Call {
        func: String,
        args: Vec<ExprId>,
    },
    /// `tbl@(k1, k2)`.
    Lookup {
        table: String,
        keys: Vec<ExprId>,
    },
    /// A node that failed to parse. Present so that recovery can continue and a
    /// single expression can yield several diagnostics.
    Error,
}

/// The closed builtin set of IR 1.0 (§2.8).
pub const BUILTINS: &[&str] = &[
    "min",
    "max",
    "abs",
    "floor",
    "ceil",
    "round",
    "clamp",
    "sign",
    "exp",
    "ln",
    "pow",
    "sqrt",
    "to_monthly",
    "to_annual",
    "nominal_to_periodic",
    "v_from_i",
    "i_from_v",
    "annuity_factor",
    "compound",
    "shift",
    "retime",
    "cum",
    "diff",
    "and",
    "or",
    "not",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "is_null",
    "coalesce",
    "year",
    "month",
    "day",
    "add_months",
    "months_between",
    "year_frac",
    "sum",
    "sum_kahan",
    "npv",
    "first",
    "last",
    "at",
    "max_over",
    "min_over",
    "count_while",
];

/// The aggregate subset of [`BUILTINS`]: a component whose expression contains
/// one of these is stage-2 (§8.2).
pub const AGG_FNS: &[&str] = &[
    "sum",
    "sum_kahan",
    "npv",
    "first",
    "last",
    "at",
    "max_over",
    "min_over",
    "count_while",
];

/// The timing tags of §2.5, spelled bare in `retime(x, mid)`. They are the one
/// place the grammar has a keyword operand rather than a value, so the parser
/// rewrites them to a string literal instead of leaving a dangling `Ref`.
pub const TIMING_TAGS: &[&str] = &["start", "end", "mid", "point"];

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

pub fn is_agg_fn(name: &str) -> bool {
    AGG_FNS.contains(&name)
}

// ---------------------------------------------------------------------------
// arena
// ---------------------------------------------------------------------------

/// A flat store of expression nodes with a parallel span side table.
///
/// One arena serves a whole document: every component's `expr` and `init` are
/// allocated into it, and an `ExprId` is unique across the document.
#[derive(Debug, Clone, Default)]
pub struct ExprArena {
    nodes: Vec<Expr>,
    spans: Vec<Span>,
}

impl ExprArena {
    pub fn new() -> ExprArena {
        ExprArena::default()
    }

    pub fn alloc(&mut self, expr: Expr, span: Span) -> ExprId {
        self.nodes.push(expr);
        self.spans.push(span);
        ExprId(self.nodes.len() as u32 - 1)
    }

    /// Overwrite a node in place, keeping its span. Used by the parser to
    /// rewrite `retime(x, mid)`'s tag argument (see [`TIMING_TAGS`]).
    pub fn replace(&mut self, id: ExprId, expr: Expr) {
        self.nodes[id.index()] = expr;
    }

    pub fn get(&self, id: ExprId) -> &Expr {
        &self.nodes[id.index()]
    }

    pub fn span(&self, id: ExprId) -> Span {
        self.spans[id.index()]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Direct children of a node, in `ExprPath` segment order.
    pub fn children(&self, id: ExprId) -> Vec<ExprId> {
        match self.get(id) {
            Expr::Unary { operand, .. } => vec![*operand],
            Expr::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
            Expr::If { cond, then_, else_ } => vec![*cond, *then_, *else_],
            Expr::Call { args, .. } => args.clone(),
            Expr::Lookup { keys, .. } => keys.clone(),
            _ => Vec::new(),
        }
    }

    /// The `ExprPath` segment naming child `i` of `id` (§3.0.1).
    fn segment(&self, id: ExprId, i: usize) -> String {
        match self.get(id) {
            Expr::Unary { .. } => "operand".to_string(),
            Expr::Binary { .. } => if i == 0 { "lhs" } else { "rhs" }.to_string(),
            Expr::If { .. } => ["cond", "then", "else"][i].to_string(),
            Expr::Call { .. } => format!("arg{i}"),
            Expr::Lookup { .. } => format!("key{i}"),
            _ => unreachable!("leaf node has no children"),
        }
    }

    /// Resolve an `ExprPath` (§3.0.1) against a root. `root_name` is `"expr"` or
    /// `"init"`; the path must start with it. Returns `None` for a stale path.
    pub fn resolve_path(&self, root: ExprId, root_name: &str, path: &str) -> Option<ExprId> {
        let mut segs = path.split('.');
        if segs.next()? != root_name {
            return None;
        }
        let mut cur = root;
        for seg in segs {
            let kids = self.children(cur);
            let i = (0..kids.len()).find(|&i| self.segment(cur, i) == seg)?;
            cur = kids[i];
        }
        Some(cur)
    }

    /// Every `Ref`/`Lag`/`At`/`Lookup` under `root`, each with the `ExprPath`
    /// that locates it — the edge list of `01-ir.md` §3, minus the semantics.
    pub fn references(&self, root: ExprId, root_name: &str) -> Vec<Reference> {
        let mut out = Vec::new();
        self.walk_refs(root, root_name.to_string(), &mut out);
        out
    }

    fn walk_refs(&self, id: ExprId, path: String, out: &mut Vec<Reference>) {
        match self.get(id) {
            Expr::Ref(name) => out.push(Reference {
                name: name.clone(),
                lag: Lag::Current,
                path: path.clone(),
                span: self.span(id),
            }),
            Expr::Lag { name, k } => out.push(Reference {
                name: name.clone(),
                lag: Lag::Back(*k),
                path: path.clone(),
                span: self.span(id),
            }),
            Expr::At { name, k } => out.push(Reference {
                name: name.clone(),
                lag: Lag::Absolute(*k),
                path: path.clone(),
                span: self.span(id),
            }),
            Expr::Lookup { table, .. } => out.push(Reference {
                name: table.clone(),
                lag: Lag::Table,
                path: path.clone(),
                span: self.span(id),
            }),
            _ => {}
        }
        for (i, child) in self.children(id).into_iter().enumerate() {
            let seg = self.segment(id, i);
            self.walk_refs(child, format!("{path}.{seg}"), out);
        }
    }

    /// `true` if the subtree contains an aggregate call — i.e. the owning
    /// component is stage-2 (§2.2).
    pub fn contains_agg(&self, root: ExprId) -> bool {
        if let Expr::Call { func, .. } = self.get(root) {
            if is_agg_fn(func) {
                return true;
            }
        }
        self.children(root)
            .into_iter()
            .any(|c| self.contains_agg(c))
    }

    /// `true` if the subtree contains an [`Expr::Error`] node.
    pub fn contains_error(&self, root: ExprId) -> bool {
        matches!(self.get(root), Expr::Error)
            || self
                .children(root)
                .into_iter()
                .any(|c| self.contains_error(c))
    }
}

/// How a reference reaches back in time. Mirrors the edge label of §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lag {
    /// `x` — lag 0.
    Current,
    /// `x[t-k]`.
    Back(u32),
    /// `x[k]` — a seed edge.
    Absolute(u32),
    /// A table dependency from a `Lookup`.
    Table,
}

/// One name reference inside an expression, with its `ExprPath`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub name: String,
    pub lag: Lag,
    pub path: String,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// lexer
// ---------------------------------------------------------------------------

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum Tok {
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", priority = 3)]
    Ident,
    #[regex(r"[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?|[0-9]+[eE][+-]?[0-9]+")]
    Float,
    #[regex(r"[0-9]+")]
    Int,
    #[regex(r#""([^"\\]|\\.)*""#)]
    Str,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("^")]
    Caret,
    #[token("==")]
    EqEq,
    #[token("!=")]
    BangEq,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token("@")]
    At,
}

fn describe(tok: Option<&Tok>) -> &'static str {
    match tok {
        None => "end of expression",
        Some(Tok::Ident) => "a name",
        Some(Tok::Float) | Some(Tok::Int) => "a number",
        Some(Tok::Str) => "a string",
        Some(Tok::LParen) => "`(`",
        Some(Tok::RParen) => "`)`",
        Some(Tok::LBracket) => "`[`",
        Some(Tok::RBracket) => "`]`",
        Some(Tok::Comma) => "`,`",
        Some(Tok::At) => "`@`",
        Some(_) => "an operator",
    }
}

struct Lexed {
    toks: Vec<Tok>,
    /// Byte range of each token, relative to the expression text.
    ranges: Vec<(usize, usize)>,
}

// ---------------------------------------------------------------------------
// parser
// ---------------------------------------------------------------------------

/// Recursive-descent parser for one expression string.
///
/// `base` is the absolute byte offset of the expression text within its file, so
/// every span the parser produces points into the original `.pir` file rather
/// than into the extracted string.
pub struct ExprParser<'a> {
    text: &'a str,
    lexed: Lexed,
    pos: usize,
    file: FileId,
    base: u32,
    /// Set once a syntax error has been reported for this expression, so that
    /// cascading failures do not each produce a diagnostic.
    poisoned: bool,
}

/// Parse one expression into `arena`, reporting into `diags`.
///
/// Always returns an [`ExprId`]: on failure the node is [`Expr::Error`], which is
/// what lets a document with several broken formulas report all of them.
pub fn parse_expr(
    text: &str,
    file: FileId,
    base: u32,
    arena: &mut ExprArena,
    diags: &mut Diagnostics,
) -> ExprId {
    let mut lexer = Tok::lexer(text);
    let mut lexed = Lexed {
        toks: Vec::new(),
        ranges: Vec::new(),
    };
    let mut bad: Vec<(usize, usize)> = Vec::new();
    while let Some(res) = lexer.next() {
        let r = lexer.span();
        match res {
            Ok(t) => {
                lexed.toks.push(t);
                lexed.ranges.push((r.start, r.end));
            }
            Err(_) => bad.push((r.start, r.end)),
        }
    }
    let mut p = ExprParser {
        text,
        lexed,
        pos: 0,
        file,
        base,
        poisoned: false,
    };
    for (s, e) in bad {
        let span = p.span_of(s, e);
        diags.push(
            Diagnostic::error("E0021", format!("unexpected character `{}`", &text[s..e]))
                .with_primary(span, "not valid in an expression"),
        );
        p.poisoned = true;
    }
    let id = p.parse_expr(arena, diags);
    if p.pos < p.lexed.toks.len() {
        let span = p.current_span();
        p.error(
            diags,
            "E0022",
            "trailing input after the expression",
            span,
            "unexpected here",
        );
    }
    id
}

impl<'a> ExprParser<'a> {
    fn span_of(&self, start: usize, end: usize) -> Span {
        Span {
            file: self.file,
            start: self.base + start as u32,
            end: self.base + end as u32,
        }
    }

    fn peek(&self) -> Option<&Tok> {
        self.lexed.toks.get(self.pos)
    }

    fn slice(&self, n: usize) -> &'a str {
        let (s, e) = self.lexed.ranges[n];
        &self.text[s..e]
    }

    fn current_text(&self) -> &'a str {
        self.slice(self.pos.min(self.lexed.ranges.len().saturating_sub(1)))
    }

    fn current_span(&self) -> Span {
        match self.lexed.ranges.get(self.pos) {
            Some(&(s, e)) => self.span_of(s, e),
            None => {
                let end = self.text.len();
                self.span_of(end.saturating_sub(1), end)
            }
        }
    }

    fn prev_span(&self) -> Span {
        let i = self.pos.saturating_sub(1);
        match self.lexed.ranges.get(i) {
            Some(&(s, e)) => self.span_of(s, e),
            None => self.current_span(),
        }
    }

    fn eat(&mut self, tok: Tok) -> bool {
        if self.peek() == Some(&tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Is the current token the keyword `kw`?
    fn at_kw(&self, kw: &str) -> bool {
        self.peek() == Some(&Tok::Ident) && self.slice(self.pos) == kw
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn error(
        &mut self,
        diags: &mut Diagnostics,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        if !self.poisoned {
            diags.push(Diagnostic::error(code, msg).with_primary(span, label));
            self.poisoned = true;
        }
    }

    fn error_node(&mut self, arena: &mut ExprArena) -> ExprId {
        let span = self.current_span();
        arena.alloc(Expr::Error, span)
    }

    // expr := ternary
    fn parse_expr(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        self.parse_ternary(arena, diags)
    }

    // ternary := "if" expr "then" expr "else" expr | orexpr
    fn parse_ternary(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        if self.at_kw("if") {
            let start = self.current_span();
            self.pos += 1;
            let cond = self.parse_expr(arena, diags);
            if !self.eat_kw("then") {
                let span = self.current_span();
                self.error(
                    diags,
                    "E0023",
                    "expected `then` after the `if` condition",
                    span,
                    "expected `then`",
                );
            }
            let then_ = self.parse_expr(arena, diags);
            if !self.eat_kw("else") {
                let span = self.current_span();
                self.error(
                    diags,
                    "E0024",
                    "expected `else`: `if` is a value conditional and both arms are required",
                    span,
                    "expected `else`",
                );
            }
            let else_ = self.parse_expr(arena, diags);
            let span = start.to(arena.span(else_));
            return arena.alloc(Expr::If { cond, then_, else_ }, span);
        }
        self.parse_or(arena, diags)
    }

    fn parse_or(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let mut lhs = self.parse_and(arena, diags);
        while self.eat_kw("or") {
            let rhs = self.parse_and(arena, diags);
            let span = arena.span(lhs).to(arena.span(rhs));
            lhs = arena.alloc(
                Expr::Binary {
                    op: BinOp::Or,
                    lhs,
                    rhs,
                },
                span,
            );
        }
        lhs
    }

    fn parse_and(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let mut lhs = self.parse_cmp(arena, diags);
        while self.eat_kw("and") {
            let rhs = self.parse_cmp(arena, diags);
            let span = arena.span(lhs).to(arena.span(rhs));
            lhs = arena.alloc(
                Expr::Binary {
                    op: BinOp::And,
                    lhs,
                    rhs,
                },
                span,
            );
        }
        lhs
    }

    // cmp := sum (op sum)?    -- non-associative, exactly the grammar
    fn parse_cmp(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let lhs = self.parse_sum(arena, diags);
        let op = match self.peek() {
            Some(Tok::EqEq) => BinOp::Eq,
            Some(Tok::BangEq) => BinOp::Ne,
            Some(Tok::Lt) => BinOp::Lt,
            Some(Tok::Le) => BinOp::Le,
            Some(Tok::Gt) => BinOp::Gt,
            Some(Tok::Ge) => BinOp::Ge,
            _ => return lhs,
        };
        self.pos += 1;
        let rhs = self.parse_sum(arena, diags);
        let span = arena.span(lhs).to(arena.span(rhs));
        let id = arena.alloc(Expr::Binary { op, lhs, rhs }, span);
        // `a < b < c` is not in the grammar; say so rather than mis-associating.
        if matches!(
            self.peek(),
            Some(Tok::EqEq)
                | Some(Tok::BangEq)
                | Some(Tok::Lt)
                | Some(Tok::Le)
                | Some(Tok::Gt)
                | Some(Tok::Ge)
        ) {
            let span = self.current_span();
            self.error(
                diags,
                "E0025",
                "comparisons do not chain",
                span,
                "a second comparison at the same level",
            );
        }
        id
    }

    fn parse_sum(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let mut lhs = self.parse_product(arena, diags);
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_product(arena, diags);
            let span = arena.span(lhs).to(arena.span(rhs));
            lhs = arena.alloc(Expr::Binary { op, lhs, rhs }, span);
        }
        lhs
    }

    fn parse_product(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let mut lhs = self.parse_unary(arena, diags);
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_unary(arena, diags);
            let span = arena.span(lhs).to(arena.span(rhs));
            lhs = arena.alloc(Expr::Binary { op, lhs, rhs }, span);
        }
        lhs
    }

    fn parse_unary(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        if self.peek() == Some(&Tok::Minus) {
            let start = self.current_span();
            self.pos += 1;
            let operand = self.parse_unary(arena, diags);
            let span = start.to(arena.span(operand));
            return arena.alloc(
                Expr::Unary {
                    op: UnaryOp::Neg,
                    operand,
                },
                span,
            );
        }
        if self.at_kw("not") {
            let start = self.current_span();
            self.pos += 1;
            let operand = self.parse_unary(arena, diags);
            let span = start.to(arena.span(operand));
            return arena.alloc(
                Expr::Unary {
                    op: UnaryOp::Not,
                    operand,
                },
                span,
            );
        }
        self.parse_power(arena, diags)
    }

    // power := atom ("^" unary)?   -- right-associative through `unary`
    fn parse_power(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let lhs = self.parse_atom(arena, diags);
        if self.eat(Tok::Caret) {
            let rhs = self.parse_unary(arena, diags);
            let span = arena.span(lhs).to(arena.span(rhs));
            return arena.alloc(
                Expr::Binary {
                    op: BinOp::Pow,
                    lhs,
                    rhs,
                },
                span,
            );
        }
        lhs
    }

    fn parse_atom(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let span = self.current_span();
        match self.peek().cloned() {
            Some(Tok::Int) => {
                let text = self.slice(self.pos);
                self.pos += 1;
                let v = text.parse::<i64>().unwrap_or(0);
                arena.alloc(Expr::Lit(Lit::Int(v)), span)
            }
            Some(Tok::Float) => {
                let text = self.slice(self.pos);
                self.pos += 1;
                let v = text.parse::<f64>().unwrap_or(0.0);
                arena.alloc(Expr::Lit(Lit::Float(v)), span)
            }
            Some(Tok::Str) => {
                let text = self.slice(self.pos);
                self.pos += 1;
                let inner = unescape(&text[1..text.len() - 1]);
                arena.alloc(Expr::Lit(Lit::Str(inner)), span)
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.parse_expr(arena, diags);
                if !self.eat(Tok::RParen) {
                    let s = self.current_span();
                    self.error(diags, "E0026", "unclosed `(`", s, "expected `)` here");
                }
                inner
            }
            Some(Tok::Ident) => self.parse_ident_atom(arena, diags),
            other => {
                let found = describe(other.as_ref());
                self.error(
                    diags,
                    "E0020",
                    format!("expected a value, found {found}"),
                    span,
                    "a name, a number, a call or `(` was expected here",
                );
                // Do not consume: the caller's loop terminates on the same token.
                arena.alloc(Expr::Error, span)
            }
        }
    }

    /// `ref`, `call` or `lookup`, all of which start with an identifier.
    fn parse_ident_atom(&mut self, arena: &mut ExprArena, diags: &mut Diagnostics) -> ExprId {
        let start = self.current_span();
        let name = self.slice(self.pos).to_string();
        if name == "true" || name == "false" {
            self.pos += 1;
            return arena.alloc(Expr::Lit(Lit::Bool(name == "true")), start);
        }
        self.pos += 1;

        // call := IDENT "(" args? ")"
        if self.peek() == Some(&Tok::LParen) {
            self.pos += 1;
            let mut args = Vec::new();
            if self.peek() != Some(&Tok::RParen) {
                loop {
                    args.push(self.parse_expr(arena, diags));
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
            }
            if !self.eat(Tok::RParen) {
                let s = self.current_span();
                self.error(
                    diags,
                    "E0027",
                    format!("unclosed argument list for `{name}`"),
                    s,
                    "expected `)` or `,`",
                );
            }
            if name == "retime" && args.len() == 2 {
                if let Expr::Ref(tag) = arena.get(args[1]) {
                    if TIMING_TAGS.contains(&tag.as_str()) {
                        let tag = tag.clone();
                        arena.replace(args[1], Expr::Lit(Lit::Str(tag)));
                    }
                }
            }
            let span = start.to(self.prev_span());
            return arena.alloc(Expr::Call { func: name, args }, span);
        }

        // lookup := IDENT "@" "(" keys ")"
        if self.peek() == Some(&Tok::At) {
            self.pos += 1;
            if !self.eat(Tok::LParen) {
                let s = self.current_span();
                self.error(
                    diags,
                    "E0028",
                    format!("expected `(` after the lookup sigil on `{name}`"),
                    s,
                    "a table lookup is written `tbl@(key, ...)`",
                );
                return self.error_node(arena);
            }
            let mut keys = Vec::new();
            if self.peek() != Some(&Tok::RParen) {
                loop {
                    keys.push(self.parse_expr(arena, diags));
                    if !self.eat(Tok::Comma) {
                        break;
                    }
                }
            }
            if !self.eat(Tok::RParen) {
                let s = self.current_span();
                self.error(
                    diags,
                    "E0027",
                    format!("unclosed key list for `{name}@(...)`"),
                    s,
                    "expected `)` or `,`",
                );
            }
            let span = start.to(self.prev_span());
            return arena.alloc(Expr::Lookup { table: name, keys }, span);
        }

        // ref := IDENT index?
        if self.peek() == Some(&Tok::LBracket) {
            return self.parse_index(name, start, arena, diags);
        }
        arena.alloc(Expr::Ref(name), start)
    }

    // index := "[" ("t" ("-" INT)? | INT) "]"
    fn parse_index(
        &mut self,
        name: String,
        start: Span,
        arena: &mut ExprArena,
        diags: &mut Diagnostics,
    ) -> ExprId {
        self.pos += 1; // `[`
        let index_span = self.current_span();
        let node = if self.at_kw("t") {
            self.pos += 1;
            if self.eat(Tok::Minus) {
                let k = self.parse_index_int(arena, diags);
                if k == 0 {
                    let s = self.prev_span();
                    self.error(
                        diags,
                        "E0029",
                        "`x[t-0]` is not a lag",
                        s,
                        "write `x` instead",
                    );
                }
                Expr::Lag { name, k }
            } else if self.peek() == Some(&Tok::Plus) {
                self.pos += 1;
                let _ = self.parse_index_int(arena, diags);
                diags.push(
                    Diagnostic::error("E0030", "forward references are not expressible")
                        .with_primary(index_span, "`x[t+k]` looks into the future")
                        .with_secondary(
                            start,
                            "a projection is a single forward pass; express this with an aggregate over the completed series",
                        ),
                );
                self.poisoned = true;
                Expr::Error
            } else {
                Expr::Ref(name)
            }
        } else if self.peek() == Some(&Tok::Int) {
            let k = self.parse_index_int(arena, diags);
            Expr::At { name, k }
        } else if self.at_kw("T") {
            // `x[T]` is the other spelling of a forward reference: the horizon
            // is not addressable from inside the projection (01-ir.md §2.7).
            self.pos += 1;
            diags.push(
                Diagnostic::error("E0030", "forward references are not expressible")
                    .with_primary(index_span, "`T` is the end of the projection, which has not happened yet")
                    .with_secondary(
                        start,
                        "a projection is a single forward pass; express this with an aggregate over the completed series",
                    )
                    .with_suggestion(index_span, "t", "read the current period"),
            );
            self.poisoned = true;
            Expr::Error
        } else {
            let s = self.current_span();
            self.error(
                diags,
                "E0031",
                "an index must be `t`, `t-k` or a non-negative integer",
                s,
                "not a legal index",
            );
            Expr::Error
        };
        if !self.eat(Tok::RBracket) {
            let s = self.current_span();
            self.error(diags, "E0032", "unclosed index", s, "expected `]`");
        }
        let span = start.to(self.prev_span());
        arena.alloc(node, span)
    }

    fn parse_index_int(&mut self, _arena: &mut ExprArena, diags: &mut Diagnostics) -> u32 {
        if self.peek() == Some(&Tok::Int) {
            let text = self.slice(self.pos);
            self.pos += 1;
            text.parse::<u32>().unwrap_or(0)
        } else {
            let s = self.current_span();
            let found = describe(self.peek());
            self.error(
                diags,
                "E0031",
                format!("expected an integer in the index, found {found}"),
                s,
                "expected an integer",
            );
            let _ = self.current_text();
            0
        }
    }
}

/// Unescape a TOML basic-string body.
pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
