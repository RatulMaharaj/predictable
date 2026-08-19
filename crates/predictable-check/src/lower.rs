//! Lowering a parsed document to the `predictable-ir` data model — and, with
//! it, the one structural rewrite the parser deliberately leaves undone.
//!
//! The grammar of §4.2 has no aggregate form: `npv(x, disc)` and `sum(flow)`
//! parse as ordinary calls, because syntactically that is all they are. The IR
//! does have one ([`predictable_ir::Expr::Agg`]), because semantically an
//! aggregate is *not* a call: it reduces a completed series to a `PerMP`, which
//! is what makes its component stage 2 (§2.2, §8.2). Deciding that is a
//! checker's job, not a parser's, so `Call → Agg` happens here.
//!
//! Two spellings collapse in the same step: `at(x, k)` and `x[k]` are the same
//! operation (§2.8) and both lower to [`predictable_ir::Expr::At`].
//!
//! ```
//! use predictable_check::lower;
//! use predictable_syntax::{parse, SourceMap};
//!
//! let mut sources = SourceMap::new();
//! let parsed = parse(&mut sources, "m.pir", r#"
//! format = "pir/1"
//! module = "m"
//!
//! [[component]]
//! name = "pv"
//! kind = "Output"
//! dtype = "f64"
//! shape = "PerMP"
//! unit = "money"
//! expr = "npv(flow, disc)"
//! "#);
//! let component = parsed.document.component("pv").unwrap();
//! let ir = lower::expr(&parsed.document.arena, component.expr.unwrap()).unwrap();
//! assert!(matches!(ir, predictable_ir::Expr::Agg { op: predictable_ir::AggOp::Npv, .. }));
//! ```

use predictable_ir::{AggOp, BinaryOp, Expr as IrExpr, LitValue, UnaryOp as IrUnary};
use predictable_syntax::ast as syn;
use predictable_syntax::expr::{BinOp, Expr, ExprArena, ExprId, Lit, UnaryOp};
use predictable_syntax::PirDocument;

/// The aggregate call names of §2.8, mapped to their IR node.
fn agg_op(name: &str) -> Option<AggOp> {
    Some(match name {
        "sum" => AggOp::Sum,
        "sum_kahan" => AggOp::SumKahan,
        "npv" => AggOp::Npv,
        "first" => AggOp::First,
        "last" => AggOp::Last,
        "at" => AggOp::At,
        "max_over" => AggOp::MaxOver,
        "min_over" => AggOp::MinOver,
        "count_while" => AggOp::CountWhile,
        _ => return None,
    })
}

/// Lower one expression. Returns `None` if the tree contains a parse error
/// node — a broken expression has no IR form, and inventing one would hide the
/// diagnostic that produced it.
pub fn expr(arena: &ExprArena, id: ExprId) -> Option<IrExpr> {
    Some(match arena.get(id) {
        Expr::Error => return None,
        Expr::Lit(lit) => match lit {
            Lit::Int(i) => IrExpr::Lit {
                dtype: predictable_ir::DType::I64,
                value: LitValue::Int(*i),
            },
            Lit::Float(x) => IrExpr::Lit {
                dtype: predictable_ir::DType::F64,
                value: LitValue::Float(*x),
            },
            Lit::Bool(b) => IrExpr::Lit {
                dtype: predictable_ir::DType::Bool,
                value: LitValue::Bool(*b),
            },
            Lit::Str(s) => IrExpr::Lit {
                dtype: predictable_ir::DType::Str,
                value: LitValue::Text(s.clone()),
            },
        },
        Expr::Ref(name) => IrExpr::Ref { name: name.clone() },
        Expr::Lag { name, k } => IrExpr::Lag {
            name: name.clone(),
            k: *k,
        },
        Expr::At { name, k } => IrExpr::At {
            name: name.clone(),
            k: *k,
        },
        Expr::Unary { op, operand } => IrExpr::Unary {
            op: match op {
                UnaryOp::Neg => IrUnary::Neg,
                UnaryOp::Not => IrUnary::Not,
            },
            operand: Box::new(expr(arena, *operand)?),
        },
        Expr::Binary { op, lhs, rhs } => IrExpr::Binary {
            op: binary_op(*op),
            lhs: Box::new(expr(arena, *lhs)?),
            rhs: Box::new(expr(arena, *rhs)?),
        },
        Expr::If { cond, then_, else_ } => IrExpr::If {
            cond: Box::new(expr(arena, *cond)?),
            then: Box::new(expr(arena, *then_)?),
            otherwise: Box::new(expr(arena, *else_)?),
        },
        Expr::Lookup { table, keys } => IrExpr::Lookup {
            table: table.clone(),
            keys: keys
                .iter()
                .map(|k| expr(arena, *k))
                .collect::<Option<_>>()?,
        },
        Expr::Call { func, args } => match agg_op(func) {
            // `at(x, k)` with a literal index is the same node as `x[k]`.
            Some(AggOp::At) => match (
                args.first().map(|a| arena.get(*a)),
                args.get(1).map(|a| arena.get(*a)),
            ) {
                (Some(Expr::Ref(name)), Some(Expr::Lit(Lit::Int(k)))) if *k >= 0 => IrExpr::At {
                    name: name.clone(),
                    k: *k as u32,
                },
                _ => aggregate(arena, AggOp::At, args)?,
            },
            Some(op) => aggregate(arena, op, args)?,
            None => IrExpr::Call {
                func: func.clone(),
                args: args
                    .iter()
                    .map(|a| expr(arena, *a))
                    .collect::<Option<_>>()?,
            },
        },
    })
}

/// `Agg(op, value, second)`.
///
/// The IR gives an `Agg` one optional second operand (§2.6). For a predicated
/// reduction — `sum(x, cond)`, `count_while(cond)` — that operand is the
/// predicate; for `npv(x, disc)` it is the discount-factor series. Both are
/// reached by the `pred` segment of an `ExprPath` (§3.0.1), which is why the
/// path grammar names only one.
fn aggregate(arena: &ExprArena, op: AggOp, args: &[ExprId]) -> Option<IrExpr> {
    let value = Box::new(expr(arena, *args.first()?)?);
    let second = match args.get(1) {
        Some(a) => Some(Box::new(expr(arena, *a)?)),
        None => None,
    };
    Some(IrExpr::Agg {
        op,
        value,
        pred: second,
    })
}

fn binary_op(op: BinOp) -> BinaryOp {
    match op {
        BinOp::Add => BinaryOp::Add,
        BinOp::Sub => BinaryOp::Sub,
        BinOp::Mul => BinaryOp::Mul,
        BinOp::Div => BinaryOp::Div,
        BinOp::Pow => BinaryOp::Pow,
        BinOp::Eq => BinaryOp::Eq,
        BinOp::Ne => BinaryOp::Ne,
        BinOp::Lt => BinaryOp::Lt,
        BinOp::Le => BinaryOp::Le,
        BinOp::Gt => BinaryOp::Gt,
        BinOp::Ge => BinaryOp::Ge,
        BinOp::And => BinaryOp::And,
        BinOp::Or => BinaryOp::Or,
    }
}

// ---------------------------------------------------------------------------
// declarations
// ---------------------------------------------------------------------------

/// Lower a whole module document to [`predictable_ir::Module`].
///
/// Only call this on a document that checked clean: lowering is a translation,
/// not a validation, and it drops anything it cannot represent.
pub fn module(doc: &PirDocument) -> predictable_ir::Module {
    let mut out = predictable_ir::Module::new(
        doc.module
            .as_ref()
            .map(|m| m.value.clone())
            .unwrap_or_default(),
    );
    out.imports = doc.imports.clone();
    out.timeline = doc.timeline.as_ref().map(|t| predictable_ir::Timeline {
        basis: match t.basis {
            syn::TimelineBasis::Monthly => predictable_ir::Basis::Monthly,
            syn::TimelineBasis::Quarterly => predictable_ir::Basis::Quarterly,
            syn::TimelineBasis::Annual => predictable_ir::Basis::Annual,
        },
        periods: t.periods.max(0) as u32,
        origin: match t.origin {
            syn::Origin::Policy => predictable_ir::Origin::Policy,
            syn::Origin::Valuation => predictable_ir::Origin::Valuation,
        },
        valuation_date: t.valuation_date.clone().unwrap_or_default(),
        year_convention: t
            .year_convention
            .clone()
            .unwrap_or_else(|| "act/365".to_string()),
    });
    out.enums = doc
        .enums
        .iter()
        .map(|e| predictable_ir::EnumDecl {
            name: e.name.value.clone(),
            values: e.values.clone(),
        })
        .collect();
    out.modelpoint_fields = doc
        .modelpoint_fields
        .iter()
        .map(|f| predictable_ir::ModelpointField {
            name: f.name.value.clone(),
            dtype: dtype(&f.dtype),
            unit: unit(&f.unit),
            required: f.required,
            key: f.key,
            default_value: f.default.as_ref().and_then(value_literal),
            doc: f.doc.clone(),
        })
        .collect();
    out.assumptions = doc
        .assumptions
        .iter()
        .map(|a| predictable_ir::AssumptionDecl {
            name: a.name.value.clone(),
            dtype: dtype(&a.dtype),
            unit: unit(&a.unit),
            shape: shape(a.shape),
            doc: a.doc.clone(),
        })
        .collect();
    out.tables = doc.tables.iter().map(table).collect();
    out.components = doc.components.iter().map(|c| component(doc, c)).collect();
    out
}

/// Lower one component, aggregates and all.
pub fn component(doc: &PirDocument, c: &syn::Component) -> predictable_ir::Component {
    predictable_ir::Component {
        name: c.name.value.clone(),
        kind: kind(c.kind),
        dtype: dtype(&c.dtype),
        shape: shape(c.shape),
        unit: unit(&c.unit),
        timing: c.timing.map(timing),
        init: c.init.and_then(|id| expr(&doc.arena, id)),
        expr: c.expr.and_then(|id| expr(&doc.arena, id)),
        doc: c.doc.clone(),
        tags: c.tags.clone(),
        meta: predictable_ir::Meta::default(),
    }
}

fn table(t: &syn::TableDecl) -> predictable_ir::TableDecl {
    predictable_ir::TableDecl {
        name: t.name.value.clone(),
        keys: t
            .keys
            .iter()
            .map(|k| predictable_ir::TableKey {
                name: k.name.clone(),
                dtype: dtype(&k.dtype),
                policy: match k.policy {
                    syn::KeyPolicy::Exact => predictable_ir::KeyPolicy::Exact,
                    syn::KeyPolicy::Clamp => predictable_ir::KeyPolicy::Clamp,
                    syn::KeyPolicy::Step => predictable_ir::KeyPolicy::Step,
                    syn::KeyPolicy::Interpolate => predictable_ir::KeyPolicy::Interpolate,
                },
            })
            .collect(),
        values: t
            .values
            .iter()
            .map(|v| predictable_ir::TableValue {
                name: v.name.clone(),
                dtype: dtype(&v.dtype),
                unit: unit(&v.unit),
            })
            .collect(),
        on_missing: match &t.on_missing {
            syn::OnMissing::Error => predictable_ir::OnMissing::Error,
            syn::OnMissing::Default(text) => predictable_ir::OnMissing::Default(literal(text)),
            syn::OnMissing::Interpolate(key) => predictable_ir::OnMissing::Interpolate(key.clone()),
        },
        source: t
            .source
            .parse()
            .unwrap_or(predictable_ir::TableSource::Inline),
        digest: t.digest.clone(),
        rows: if t.rows.is_empty() {
            None
        } else {
            Some(
                t.rows
                    .iter()
                    .map(|row| row.iter().filter_map(value_literal).collect())
                    .collect(),
            )
        },
        doc: None,
    }
}

fn value_literal(v: &predictable_syntax::raw::Value) -> Option<LitValue> {
    use predictable_syntax::raw::Value;
    Some(match v {
        Value::Str(s) => LitValue::Text(s.clone()),
        Value::Int(i) => LitValue::Int(*i),
        Value::Float(x) => LitValue::Float(*x),
        Value::Bool(b) => LitValue::Bool(*b),
        Value::Date(d) => LitValue::Text(d.clone()),
        _ => return None,
    })
}

fn literal(text: &str) -> LitValue {
    if text == "true" || text == "false" {
        LitValue::Bool(text == "true")
    } else if let Ok(i) = text.parse::<i64>() {
        LitValue::Int(i)
    } else if let Ok(x) = text.parse::<f64>() {
        LitValue::Float(x)
    } else {
        LitValue::Text(text.trim_matches('"').to_string())
    }
}

fn kind(k: syn::Kind) -> predictable_ir::Kind {
    match k {
        syn::Kind::InputModelpoint => predictable_ir::Kind::InputModelpoint,
        syn::Kind::InputAssumption => predictable_ir::Kind::InputAssumption,
        syn::Kind::InputTable => predictable_ir::Kind::InputTable,
        syn::Kind::InputTimeline => predictable_ir::Kind::InputTimeline,
        syn::Kind::Derived => predictable_ir::Kind::Derived,
        syn::Kind::Output => predictable_ir::Kind::Output,
    }
}

fn shape(s: syn::Shape) -> predictable_ir::Shape {
    match s {
        syn::Shape::Scalar => predictable_ir::Shape::Scalar,
        syn::Shape::PerMP => predictable_ir::Shape::PerMp,
        syn::Shape::Series => predictable_ir::Shape::Series,
    }
}

fn timing(t: syn::Timing) -> predictable_ir::Timing {
    match t {
        syn::Timing::Start => predictable_ir::Timing::Start,
        syn::Timing::End => predictable_ir::Timing::End,
        syn::Timing::Mid => predictable_ir::Timing::Mid,
        syn::Timing::Point => predictable_ir::Timing::Point,
    }
}

fn dtype(d: &syn::DType) -> predictable_ir::DType {
    match d {
        syn::DType::F64 => predictable_ir::DType::F64,
        syn::DType::I64 => predictable_ir::DType::I64,
        syn::DType::Bool => predictable_ir::DType::Bool,
        syn::DType::Date => predictable_ir::DType::Date,
        syn::DType::Str => predictable_ir::DType::Str,
        syn::DType::Enum(name) => predictable_ir::DType::Enum(name.clone()),
    }
}

fn unit(u: &syn::Unit) -> predictable_ir::Unit {
    match u {
        syn::Unit::None => predictable_ir::Unit::None,
        syn::Unit::Money => predictable_ir::Unit::Money,
        syn::Unit::Prob => predictable_ir::Unit::Prob,
        syn::Unit::Count => predictable_ir::Unit::Count,
        syn::Unit::Years => predictable_ir::Unit::Years,
        syn::Unit::Months => predictable_ir::Unit::Months,
        syn::Unit::Factor => predictable_ir::Unit::Factor,
        syn::Unit::Rate(b) => predictable_ir::Unit::Rate(match b {
            syn::RateBasis::Annual => predictable_ir::RateBasis::Annual,
            syn::RateBasis::Monthly => predictable_ir::RateBasis::Monthly,
            syn::RateBasis::Period => predictable_ir::RateBasis::Period,
        }),
    }
}
