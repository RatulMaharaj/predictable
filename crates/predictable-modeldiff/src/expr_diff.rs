//! The AST-level expression diff (`01-ir.md` §11.3).
//!
//! Two trees are walked in lockstep. A node whose *own* fields differ but whose
//! children line up is reported as a change **at that node**, and the walk
//! continues underneath it; a node whose structure differs is reported once, as
//! a replacement of the whole subtree, and the walk stops there.
//!
//! That distinction is the whole value of diffing the AST instead of the text.
//! `v^t` becoming `v^(t + 1)` is one `Replaced` at `expr.rhs` — a two-character
//! textual edit that a line diff buries and a reviewer misses. `a * b` becoming
//! `a / b` is one `Operator` change with the operands untouched, not a rewritten
//! formula. Every change is anchored to an [`ExprPath`] (§3.0.1), which is
//! stable under reformatting, so the anchors survive `predictable fmt`.

use predictable_ir::{Expr, ExprPath, PathRoot, PathSeg};
use serde::{Deserialize, Serialize};

use crate::render::{agg_name, render};

/// What changed at one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeChangeKind {
    /// The subtree was replaced by a structurally different one.
    Replaced,
    /// Same binary/unary node, different operator.
    Operator,
    /// A `Ref`/`Lag`/`At` now names a different component.
    Reference,
    /// Same component, different lag or absolute index.
    Offset,
    /// A literal value or its dtype changed.
    Literal,
    /// A call to a different builtin, operands unchanged.
    Function,
    /// A lookup against a different table, keys unchanged.
    Table,
    /// A different reduction over the same series, e.g. `sum` → `npv`.
    Aggregate,
    /// A predicate was added to or removed from an aggregate.
    Predicate,
    /// Same callable, different number of arguments or keys.
    Arity,
}

impl NodeChangeKind {
    /// A short human sentence, used by the terminal renderer.
    pub fn describe(self) -> &'static str {
        match self {
            NodeChangeKind::Replaced => "subtree replaced",
            NodeChangeKind::Operator => "operator changed",
            NodeChangeKind::Reference => "reference retargeted",
            NodeChangeKind::Offset => "time offset changed",
            NodeChangeKind::Literal => "literal changed",
            NodeChangeKind::Function => "function changed",
            NodeChangeKind::Table => "table changed",
            NodeChangeKind::Aggregate => "aggregate changed",
            NodeChangeKind::Predicate => "aggregate predicate changed",
            NodeChangeKind::Arity => "argument count changed",
        }
    }
}

/// One difference between two expression trees, anchored at an `ExprPath`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeChange {
    /// Where in the tree, e.g. `expr.lhs.arg0` (§3.0.1).
    pub path: ExprPath,
    /// What kind of difference this is.
    pub change: NodeChangeKind,
    /// The node as it was, rendered in canonical form.
    pub before: String,
    /// The node as it now is.
    pub after: String,
}

/// Diff two trees rooted at `root` (`expr` or `init`).
pub fn diff_tree(root: PathRoot, a: &Expr, b: &Expr) -> Vec<NodeChange> {
    let mut out = Vec::new();
    walk(&ExprPath::root(root), a, b, &mut out);
    out
}

fn push(out: &mut Vec<NodeChange>, path: &ExprPath, change: NodeChangeKind, a: &Expr, b: &Expr) {
    out.push(NodeChange {
        path: path.clone(),
        change,
        before: render(a),
        after: render(b),
    });
}

/// Recurse into the children the two nodes share, by path segment.
fn recurse(path: &ExprPath, a: &Expr, b: &Expr, out: &mut Vec<NodeChange>) {
    for ((seg, ca), (_, cb)) in a.children().into_iter().zip(b.children()) {
        walk(&path.child(seg), ca, cb, out);
    }
}

fn same_child_shape(a: &Expr, b: &Expr) -> bool {
    let (ka, kb) = (a.children(), b.children());
    ka.len() == kb.len() && ka.iter().zip(kb.iter()).all(|((sa, _), (sb, _))| sa == sb)
}

fn walk(path: &ExprPath, a: &Expr, b: &Expr, out: &mut Vec<NodeChange>) {
    if a == b {
        return;
    }
    match (a, b) {
        (
            Expr::Lit {
                dtype: da,
                value: va,
            },
            Expr::Lit {
                dtype: db,
                value: vb,
            },
        ) if da != db || va != vb => push(out, path, NodeChangeKind::Literal, a, b),

        (Expr::Ref { name: x }, Expr::Ref { name: y }) if x != y => {
            push(out, path, NodeChangeKind::Reference, a, b)
        }

        (Expr::Lag { name: x, k: ka }, Expr::Lag { name: y, k: kb }) => {
            let kind = if x == y {
                debug_assert_ne!(ka, kb);
                NodeChangeKind::Offset
            } else {
                NodeChangeKind::Reference
            };
            push(out, path, kind, a, b);
        }
        (Expr::At { name: x, k: ka }, Expr::At { name: y, k: kb }) => {
            let kind = if x == y {
                debug_assert_ne!(ka, kb);
                NodeChangeKind::Offset
            } else {
                NodeChangeKind::Reference
            };
            push(out, path, kind, a, b);
        }
        // `x` ↔ `x[t-1]` on the same component is a timing edit, not a rewrite:
        // it is the single most common cause of a one-period shift.
        (Expr::Ref { name: x }, Expr::Lag { name: y, .. })
        | (Expr::Lag { name: x, .. }, Expr::Ref { name: y })
        | (Expr::Ref { name: x }, Expr::At { name: y, .. })
        | (Expr::At { name: x, .. }, Expr::Ref { name: y })
        | (Expr::Lag { name: x, .. }, Expr::At { name: y, .. })
        | (Expr::At { name: x, .. }, Expr::Lag { name: y, .. })
            if x == y =>
        {
            push(out, path, NodeChangeKind::Offset, a, b)
        }

        (Expr::Unary { op: oa, .. }, Expr::Unary { op: ob, .. }) => {
            if oa != ob {
                push(out, path, NodeChangeKind::Operator, a, b);
            }
            recurse(path, a, b, out);
        }
        (Expr::Binary { op: oa, .. }, Expr::Binary { op: ob, .. }) => {
            if oa != ob {
                push(out, path, NodeChangeKind::Operator, a, b);
            }
            recurse(path, a, b, out);
        }
        (Expr::If { .. }, Expr::If { .. }) => recurse(path, a, b, out),

        (Expr::Call { func: fa, args: aa }, Expr::Call { func: fb, args: ab }) => {
            if aa.len() != ab.len() {
                push(out, path, NodeChangeKind::Arity, a, b);
                return;
            }
            if fa != fb {
                push(out, path, NodeChangeKind::Function, a, b);
            }
            recurse(path, a, b, out);
        }
        (
            Expr::Lookup {
                table: ta,
                keys: ka,
            },
            Expr::Lookup {
                table: tb,
                keys: kb,
            },
        ) => {
            if ka.len() != kb.len() {
                push(out, path, NodeChangeKind::Arity, a, b);
                return;
            }
            if ta != tb {
                push(out, path, NodeChangeKind::Table, a, b);
            }
            recurse(path, a, b, out);
        }
        (
            Expr::Agg {
                op: oa, pred: pa, ..
            },
            Expr::Agg {
                op: ob, pred: pb, ..
            },
        ) => {
            if pa.is_some() != pb.is_some() {
                push(out, path, NodeChangeKind::Predicate, a, b);
            }
            if oa != ob {
                push(out, path, NodeChangeKind::Aggregate, a, b);
            }
            if !same_child_shape(a, b) {
                // A predicate appeared or vanished; the `value` child still
                // lines up, so compare that and stop.
                walk(&path.child(PathSeg::Value), value_of(a), value_of(b), out);
                return;
            }
            recurse(path, a, b, out);
        }

        _ => push(out, path, NodeChangeKind::Replaced, a, b),
    }
}

fn value_of(e: &Expr) -> &Expr {
    match e {
        Expr::Agg { value, .. } => value,
        other => other,
    }
}

/// A one-line summary of an aggregate change, for the terminal renderer.
pub fn agg_label(op: predictable_ir::AggOp) -> &'static str {
    agg_name(op)
}

#[cfg(test)]
mod tests {
    use super::*;
    use predictable_ir::{AggOp, BinaryOp, Expr};

    fn diff(a: &Expr, b: &Expr) -> Vec<NodeChange> {
        diff_tree(PathRoot::Expr, a, b)
    }

    #[test]
    fn identical_trees_produce_nothing() {
        let e = Expr::binary(BinaryOp::Add, Expr::r#ref("a"), Expr::f64(1.0));
        assert!(diff(&e, &e).is_empty());
    }

    #[test]
    fn operator_change_keeps_the_operands() {
        let a = Expr::binary(BinaryOp::Mul, Expr::r#ref("a"), Expr::r#ref("b"));
        let b = Expr::binary(BinaryOp::Div, Expr::r#ref("a"), Expr::r#ref("b"));
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Operator);
        assert_eq!(d[0].path.to_string(), "expr");
        assert_eq!(d[0].before, "a * b");
        assert_eq!(d[0].after, "a / b");
    }

    #[test]
    fn a_lag_edit_is_an_offset_not_a_rewrite() {
        let a = Expr::binary(
            BinaryOp::Add,
            Expr::r#ref("x"),
            Expr::Lag {
                name: "r".into(),
                k: 1,
            },
        );
        let b = Expr::binary(
            BinaryOp::Add,
            Expr::r#ref("x"),
            Expr::Lag {
                name: "r".into(),
                k: 2,
            },
        );
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Offset);
        assert_eq!(d[0].path.to_string(), "expr.rhs");
    }

    #[test]
    fn a_deep_exponent_change_is_one_localised_replacement() {
        // v^t  →  v^(t + 1): the classic discounting bug.
        let a = Expr::binary(BinaryOp::Pow, Expr::r#ref("v"), Expr::r#ref("t"));
        let b = Expr::binary(
            BinaryOp::Pow,
            Expr::r#ref("v"),
            Expr::binary(
                BinaryOp::Add,
                Expr::r#ref("t"),
                Expr::Lit {
                    dtype: predictable_ir::DType::I64,
                    value: predictable_ir::LitValue::Int(1),
                },
            ),
        );
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Replaced);
        assert_eq!(d[0].path.to_string(), "expr.rhs");
        assert_eq!(d[0].before, "t");
        assert_eq!(d[0].after, "t + 1");
    }

    #[test]
    fn aggregate_and_predicate_changes_are_distinguished() {
        let a = Expr::Agg {
            op: AggOp::Sum,
            value: Box::new(Expr::r#ref("cf")),
            pred: None,
        };
        let b = Expr::Agg {
            op: AggOp::Npv,
            value: Box::new(Expr::r#ref("cf")),
            pred: None,
        };
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Aggregate);

        let c = Expr::Agg {
            op: AggOp::Sum,
            value: Box::new(Expr::r#ref("cf")),
            pred: Some(Box::new(Expr::r#ref("alive"))),
        };
        let d = diff(&a, &c);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Predicate);
    }

    #[test]
    fn a_retargeted_lookup_keeps_its_keys() {
        let keys = vec![Expr::r#ref("age"), Expr::r#ref("gender")];
        let a = Expr::Lookup {
            table: "sa8990".into(),
            keys: keys.clone(),
        };
        let b = Expr::Lookup {
            table: "amc00".into(),
            keys,
        };
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Table);
    }

    #[test]
    fn changed_arity_stops_the_walk() {
        let a = Expr::Call {
            func: "max".into(),
            args: vec![Expr::r#ref("a"), Expr::r#ref("b")],
        };
        let b = Expr::Call {
            func: "max".into(),
            args: vec![Expr::r#ref("a")],
        };
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].change, NodeChangeKind::Arity);
    }
}
