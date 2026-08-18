//! Expression normalisation — rule 3 of `01-ir.md` §4.1.
//!
//! > Expressions are normalised: single spaces around binary operators, no
//! > redundant parentheses *except* those that disambiguate mixed `*`/`+`
//! > precedence, which are preserved as written.
//!
//! The expression is parsed by `predictable-syntax` (the same parser the engine
//! uses) and printed back from the arena. The arena does not record grouping —
//! `(a)` and `a` are the same node — so the one place where the canonical form
//! depends on what the author wrote, mixed `*`/`+`, is recovered from the source
//! text through the node's span: a `*`/`/` node directly under a `+`/`-` node
//! keeps its parentheses if, and only if, they are there in the input.

use predictable_syntax::expr::{parse_expr, BinOp, Expr, ExprArena, ExprId, Lit, UnaryOp};
use predictable_syntax::{Diagnostic, Diagnostics, FileId, SourceMap};

use crate::float::format_f64;

/// Normalise one expression string. Returns the input unchanged if it does not
/// parse — `fmt` never rewrites what it does not understand — together with the
/// diagnostics, so a caller can refuse to write the file.
pub fn format_expression(text: &str) -> (String, Vec<Diagnostic>) {
    let mut sources = SourceMap::new();
    let file = sources.add("<expr>", text.to_string());
    let mut arena = ExprArena::new();
    let mut diags = Diagnostics::new();
    let root = parse_expr(text, file, 0, &mut arena, &mut diags);
    if diags.has_errors() {
        return (text.to_string(), diags.into_vec());
    }
    let printer = Printer {
        src: text,
        arena: &arena,
        file,
    };
    let mut out = String::with_capacity(text.len());
    printer.print(root, Ctx::Top, &mut out);
    (out, diags.into_vec())
}

// ---------------------------------------------------------------------------
// precedence
// ---------------------------------------------------------------------------

/// Binding power, loosest first. Mirrors the grammar of §4.2 exactly.
const P_IF: u8 = 0;
const P_OR: u8 = 1;
const P_AND: u8 = 2;
const P_CMP: u8 = 3;
const P_SUM: u8 = 4;
const P_PRODUCT: u8 = 5;
const P_UNARY: u8 = 6;
const P_POW: u8 = 7;
const P_ATOM: u8 = 8;

fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => P_OR,
        BinOp::And => P_AND,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => P_CMP,
        BinOp::Add | BinOp::Sub => P_SUM,
        BinOp::Mul | BinOp::Div => P_PRODUCT,
        BinOp::Pow => P_POW,
    }
}

fn prec(arena: &ExprArena, id: ExprId) -> u8 {
    match arena.get(id) {
        Expr::If { .. } => P_IF,
        Expr::Binary { op, .. } => binop_prec(*op),
        Expr::Unary { .. } => P_UNARY,
        _ => P_ATOM,
    }
}

/// Where a node is being printed, which is what decides its parentheses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// The root, a call argument or a lookup key: never needs parentheses.
    Top,
    /// Operand of a binary operator: `(min, is_right_operand)`.
    Operand(u8, bool),
    /// Operand of `-` / `not`.
    Unary,
}

struct Printer<'a> {
    src: &'a str,
    arena: &'a ExprArena,
    file: FileId,
}

impl<'a> Printer<'a> {
    /// Was this node wrapped in parentheses in the source text?
    ///
    /// Spans exclude the grouping parentheses (the parser returns the inner
    /// node), so the test is "the first non-space character before the node is
    /// `(` and the first after it is `)`". It is only ever asked of a `*`/`/`
    /// node directly under `+`/`-`, which is the only case §4.1 preserves.
    fn parenthesised_in_source(&self, id: ExprId) -> bool {
        let span = self.arena.span(id);
        if span.file != self.file {
            return false;
        }
        let bytes = self.src.as_bytes();
        let before = bytes[..span.start as usize]
            .iter()
            .rposition(|b| !b.is_ascii_whitespace());
        let after = bytes[span.end as usize..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| span.end as usize + i);
        matches!(before.map(|i| bytes[i]), Some(b'('))
            && matches!(after.map(|i| bytes[i]), Some(b')'))
    }

    fn needs_parens(&self, id: ExprId, ctx: Ctx) -> bool {
        let p = prec(self.arena, id);
        match ctx {
            Ctx::Top => false,
            Ctx::Unary => p < P_UNARY,
            Ctx::Operand(min, right) => {
                if p < min {
                    return true;
                }
                // Left-associative operators need parentheses on the right at
                // equal precedence (`a - (b - c)`), and comparisons never chain.
                if right && p == min {
                    return true;
                }
                if min == P_CMP && p == P_CMP {
                    return true;
                }
                false
            }
        }
    }

    fn print(&self, id: ExprId, ctx: Ctx, out: &mut String) {
        let mut paren = self.needs_parens(id, ctx);
        // §4.1 rule 3: parentheses disambiguating mixed `*`/`+` are preserved
        // as written — kept when present, never invented.
        if !paren
            && matches!(ctx, Ctx::Operand(P_SUM, _))
            && matches!(
                self.arena.get(id),
                Expr::Binary {
                    op: BinOp::Mul | BinOp::Div,
                    ..
                }
            )
            && self.parenthesised_in_source(id)
        {
            paren = true;
        }
        if paren {
            out.push('(');
        }
        self.print_bare(id, out);
        if paren {
            out.push(')');
        }
    }

    fn print_bare(&self, id: ExprId, out: &mut String) {
        match self.arena.get(id) {
            Expr::Lit(lit) => print_lit(lit, out),
            Expr::Ref(name) => out.push_str(name),
            Expr::Lag { name, k } => {
                out.push_str(name);
                out.push_str("[t-");
                out.push_str(&k.to_string());
                out.push(']');
            }
            Expr::At { name, k } => {
                out.push_str(name);
                out.push('[');
                out.push_str(&k.to_string());
                out.push(']');
            }
            Expr::Unary { op, operand } => {
                out.push_str(match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "not ",
                });
                self.print(*operand, Ctx::Unary, out);
            }
            Expr::Binary { op, lhs, rhs } => {
                let p = binop_prec(*op);
                // `^` is right-associative: the tight side is the left one.
                let (lctx, rctx) = if *op == BinOp::Pow {
                    (Ctx::Operand(p, true), Ctx::Operand(p, false))
                } else {
                    (Ctx::Operand(p, false), Ctx::Operand(p, true))
                };
                self.print(*lhs, lctx, out);
                out.push(' ');
                out.push_str(op.as_str());
                out.push(' ');
                self.print(*rhs, rctx, out);
            }
            Expr::If { cond, then_, else_ } => {
                out.push_str("if ");
                self.print(*cond, Ctx::Top, out);
                out.push_str(" then ");
                self.print(*then_, Ctx::Top, out);
                out.push_str(" else ");
                self.print(*else_, Ctx::Top, out);
            }
            Expr::Call { func, args } => {
                out.push_str(func);
                out.push('(');
                self.print_list(func, args, out);
                out.push(')');
            }
            Expr::Lookup { table, keys } => {
                out.push_str(table);
                out.push_str("@(");
                self.print_list("", keys, out);
                out.push(')');
            }
            // Unreachable: a document with an `Error` node is never formatted.
            Expr::Error => out.push_str("<error>"),
        }
    }

    fn print_list(&self, func: &str, args: &[ExprId], out: &mut String) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            // `retime(x, mid)`: the parser rewrites the bare timing tag to a
            // string literal, so print it back bare.
            if func == "retime" && i == 1 {
                if let Expr::Lit(Lit::Str(tag)) = self.arena.get(*arg) {
                    out.push_str(tag);
                    continue;
                }
            }
            self.print(*arg, Ctx::Top, out);
        }
    }
}

fn print_lit(lit: &Lit, out: &mut String) {
    match lit {
        Lit::Int(v) => out.push_str(&v.to_string()),
        Lit::Float(v) => out.push_str(&format_f64(*v)),
        Lit::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        Lit::Str(s) => {
            out.push('"');
            crate::escape_into(s, out);
            out.push('"');
        }
    }
}
