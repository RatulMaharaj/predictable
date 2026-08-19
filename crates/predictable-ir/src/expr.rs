//! The expression tree (§2.6), its literals, and `ExprPath` (§3.0.1).
//!
//! An `Expr` is data. It has no user-defined functions, no loops, no mutable state and nothing
//! that can fail to terminate: evaluation is bounded by the size of the tree.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::IrError;
use crate::types::{DType, Timing};

/// A literal value. The companion `dtype` on [`Expr::Lit`] is what makes `"2026-06-30"` a
/// `date` rather than a `str`, and `1.0` an `f64` rather than an `i64`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LitValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Carries `str`, `date` and `enum(..)` values in their canonical text form.
    Text(String),
}

impl LitValue {
    /// True when this value can inhabit `dtype`.
    pub fn fits(&self, dtype: &DType) -> bool {
        matches!(
            (self, dtype),
            (LitValue::Bool(_), DType::Bool)
                | (LitValue::Int(_), DType::I64)
                | (LitValue::Float(_), DType::F64)
                | (LitValue::Int(_), DType::F64)
                | (LitValue::Text(_), DType::Date)
                | (LitValue::Text(_), DType::Str)
                | (LitValue::Text(_), DType::Enum(_))
        )
    }
}

impl fmt::Display for LitValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LitValue::Bool(b) => write!(f, "{b}"),
            LitValue::Int(i) => write!(f, "{i}"),
            // `{:?}` on f64 is Rust's shortest round-tripping representation and keeps the
            // `.0` on integral values, which is what §4.1's canonical form requires: `1.05`
            // never becomes `1.0500000000000001`, and `0.0` never becomes `0`.
            LitValue::Float(x) => write!(f, "{x:?}"),
            LitValue::Text(s) => write!(f, "{s}"),
        }
    }
}

/// Prefix operators. `not` is spelled as a word in the grammar (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Infix operators, including the logic connectives — there is no variadic logic node (§2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
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

impl BinaryOp {
    /// The infix spelling used by the canonical expression form (§4.1).
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Pow => "^",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
        }
    }
}

/// Reductions over `t` (§2.8 `aggregates`). Any of these makes its component stage 2 (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggOp {
    Sum,
    SumKahan,
    Npv,
    First,
    Last,
    At,
    MaxOver,
    MinOver,
    CountWhile,
}

/// A typed expression tree (§2.6).
///
/// The JSON encoding is internally tagged on `"node"`, so every node is self-describing and a
/// consumer that walks `pir.json` never needs the grammar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node")]
pub enum Expr {
    Lit {
        dtype: DType,
        value: LitValue,
    },
    /// `x` — current period, current modelpoint.
    Ref {
        name: String,
    },
    /// `x[t-k]`, `k >= 1`.
    Lag {
        name: String,
        k: u32,
    },
    /// `x[k]`, absolute period index, `k >= 0`.
    At {
        name: String,
        k: u32,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// A *value* conditional: both arms are typed, neither is control flow (§2.6).
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        #[serde(rename = "else")]
        otherwise: Box<Expr>,
    },
    /// A builtin call. The set is closed (§2.8); this crate does not police membership, the
    /// checker does — but [`crate::builtins`] is the list it checks against.
    Call {
        #[serde(rename = "fn")]
        func: String,
        args: Vec<Expr>,
    },
    /// `tbl@(k1, k2, ...)`.
    Lookup {
        table: String,
        keys: Vec<Expr>,
    },
    /// A reduction over `t`, with an optional predicate.
    Agg {
        op: AggOp,
        value: Box<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pred: Option<Box<Expr>>,
    },
}

impl Expr {
    /// Convenience constructor for a reference.
    pub fn r#ref(name: impl Into<String>) -> Expr {
        Expr::Ref { name: name.into() }
    }

    /// Convenience constructor for an `f64` literal.
    pub fn f64(x: f64) -> Expr {
        Expr::Lit {
            dtype: DType::F64,
            value: LitValue::Float(x),
        }
    }

    /// Convenience constructor for a binary node.
    pub fn binary(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    /// True when the tree contains an `Agg` node — the definition of stage 2 (§2.2, §8.2).
    pub fn contains_agg(&self) -> bool {
        if matches!(self, Expr::Agg { .. }) {
            return true;
        }
        self.children().iter().any(|(_, c)| c.contains_agg())
    }

    /// The immediate children of this node, each paired with the [`PathSeg`] that names it.
    /// Order is deterministic and is the order the canonical form writes them in.
    pub fn children(&self) -> Vec<(PathSeg, &Expr)> {
        match self {
            Expr::Lit { .. } | Expr::Ref { .. } | Expr::Lag { .. } | Expr::At { .. } => Vec::new(),
            Expr::Unary { operand, .. } => vec![(PathSeg::Operand, operand)],
            Expr::Binary { lhs, rhs, .. } => vec![(PathSeg::Lhs, lhs), (PathSeg::Rhs, rhs)],
            Expr::If {
                cond,
                then,
                otherwise,
            } => vec![
                (PathSeg::Cond, cond.as_ref()),
                (PathSeg::Then, then.as_ref()),
                (PathSeg::Else, otherwise.as_ref()),
            ],
            Expr::Call { args, .. } => args
                .iter()
                .enumerate()
                .map(|(i, a)| (PathSeg::Arg(i as u32), a))
                .collect(),
            Expr::Lookup { keys, .. } => keys
                .iter()
                .enumerate()
                .map(|(i, k)| (PathSeg::Key(i as u32), k))
                .collect(),
            Expr::Agg { value, pred, .. } => {
                let mut out = vec![(PathSeg::Value, value.as_ref())];
                if let Some(p) = pred {
                    out.push((PathSeg::Pred, p.as_ref()));
                }
                out
            }
        }
    }

    /// Resolve an [`ExprPath`] against this tree, which must be the tree named by the path's
    /// root. Returns `None` when the path does not exist — which is how a consumer detects a
    /// stale cached layout (§3.0.1).
    pub fn resolve(&self, path: &ExprPath) -> Option<&Expr> {
        let mut node = self;
        for seg in &path.segments {
            let children = node.children();
            let next = children.iter().find(|(s, _)| s == seg)?;
            node = next.1;
        }
        Some(node)
    }

    /// Every `Ref`/`Lag`/`At`/`Lookup` node in the tree with the path that locates it, in a
    /// deterministic pre-order walk. This is the raw material of the graph's edges (§3).
    pub fn reference_paths(&self, root: PathRoot) -> Vec<(ExprPath, &Expr)> {
        let mut out = Vec::new();
        self.walk_refs(&ExprPath::new(root, Vec::new()), &mut out);
        out
    }

    fn walk_refs<'a>(&'a self, at: &ExprPath, out: &mut Vec<(ExprPath, &'a Expr)>) {
        if matches!(
            self,
            Expr::Ref { .. } | Expr::Lag { .. } | Expr::At { .. } | Expr::Lookup { .. }
        ) {
            out.push((at.clone(), self));
        }
        for (seg, child) in self.children() {
            child.walk_refs(&at.child(seg), out);
        }
    }

    /// Number of nodes in the tree.
    pub fn size(&self) -> usize {
        1 + self.children().iter().map(|(_, c)| c.size()).sum::<usize>()
    }
}

/// The root of an [`ExprPath`]: a component has two expression trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathRoot {
    Expr,
    Init,
}

impl fmt::Display for PathRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PathRoot::Expr => "expr",
            PathRoot::Init => "init",
        })
    }
}

/// One segment of an [`ExprPath`] — always the *field name* of the parent node (§3.0.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathSeg {
    Lhs,
    Rhs,
    Operand,
    Cond,
    Then,
    Else,
    Arg(u32),
    Key(u32),
    Value,
    Pred,
}

impl fmt::Display for PathSeg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSeg::Lhs => f.write_str("lhs"),
            PathSeg::Rhs => f.write_str("rhs"),
            PathSeg::Operand => f.write_str("operand"),
            PathSeg::Cond => f.write_str("cond"),
            PathSeg::Then => f.write_str("then"),
            PathSeg::Else => f.write_str("else"),
            PathSeg::Arg(i) => write!(f, "arg{i}"),
            PathSeg::Key(i) => write!(f, "key{i}"),
            PathSeg::Value => f.write_str("value"),
            PathSeg::Pred => f.write_str("pred"),
        }
    }
}

impl FromStr for PathSeg {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "lhs" => PathSeg::Lhs,
            "rhs" => PathSeg::Rhs,
            "operand" => PathSeg::Operand,
            "cond" => PathSeg::Cond,
            "then" => PathSeg::Then,
            "else" => PathSeg::Else,
            "value" => PathSeg::Value,
            "pred" => PathSeg::Pred,
            other => {
                let parse_idx = |rest: &str| rest.parse::<u32>().ok();
                if let Some(rest) = other.strip_prefix("arg") {
                    PathSeg::Arg(
                        parse_idx(rest)
                            .ok_or_else(|| IrError::ExprPathSegment(other.to_string()))?,
                    )
                } else if let Some(rest) = other.strip_prefix("key") {
                    PathSeg::Key(
                        parse_idx(rest)
                            .ok_or_else(|| IrError::ExprPathSegment(other.to_string()))?,
                    )
                } else {
                    return Err(IrError::ExprPathSegment(other.to_string()));
                }
            }
        })
    }
}

/// A dotted, deterministic path from a component's `expr` or `init` root to a node (§3.0.1).
///
/// `ExprPath` is byte-range independent by design: spans move when a file is reformatted,
/// `ExprPath` does not. It is the `via` label on a graph edge and the `expr_path` field of a
/// trap report.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExprPath {
    root: PathRoot,
    segments: Vec<PathSeg>,
}

impl ExprPath {
    pub fn new(root: PathRoot, segments: Vec<PathSeg>) -> Self {
        ExprPath { root, segments }
    }

    /// The empty path — "the whole expression is the reference" (§3.0.1).
    pub fn root(root: PathRoot) -> Self {
        ExprPath::new(root, Vec::new())
    }

    /// Extend this path by one segment.
    pub fn child(&self, seg: PathSeg) -> Self {
        let mut segments = self.segments.clone();
        segments.push(seg);
        ExprPath::new(self.root, segments)
    }

    pub fn root_kind(&self) -> PathRoot {
        self.root
    }

    pub fn segments(&self) -> &[PathSeg] {
        &self.segments
    }
}

impl fmt::Display for ExprPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.root)?;
        for seg in &self.segments {
            write!(f, ".{seg}")?;
        }
        Ok(())
    }
}

impl FromStr for ExprPath {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.');
        let root = match parts.next() {
            Some("expr") => PathRoot::Expr,
            Some("init") => PathRoot::Init,
            other => return Err(IrError::ExprPathRoot(other.unwrap_or("").to_string())),
        };
        let segments = parts.map(PathSeg::from_str).collect::<Result<_, _>>()?;
        Ok(ExprPath::new(root, segments))
    }
}

impl Serialize for ExprPath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ExprPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// The timing argument of `retime(x, timing)` is a keyword, not a string literal, so it is
/// carried as a `Lit` of dtype `str` whose text parses as a [`Timing`]. This helper is the one
/// place that knows it.
pub fn timing_arg(t: Timing) -> Expr {
    Expr::Lit {
        dtype: DType::Str,
        value: LitValue::Text(t.to_string()),
    }
}
