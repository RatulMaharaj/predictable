//! Pass 3: the cycle rules of `01-ir.md` §3.1.
//!
//! Two graphs, one rule each:
//!
//! * **`G₀`** — every edge with `lag = 0` and `stage = 1`. It must be acyclic
//!   (`E0201`). Lagged edges are unconstrained: last period is already computed
//!   and is just data, which is what makes `reserve[t] = reserve[t-1] * (1 + i)`
//!   a legal self-loop. `At(x, k)` is the subtlety — it is instantaneous *at the
//!   single period `t = k`*, so `x[0]` inside `x` is a seed and legal, while
//!   `x[5]` inside `x` is a cycle at `t = 5` and is not.
//! * **`G_init`** — `init` may read stage-2 values, which inverts the ordinary
//!   direction of dependence (§8.2). Chains are legal and are what the engine's
//!   substage levels implement; a cycle is `E0202`.

use std::collections::BTreeMap;

use predictable_diagnostics::Diagnostic;
use predictable_syntax::expr::{Expr, ExprArena, ExprId, Lag};
use predictable_syntax::source::{SourceMap, Span};
use predictable_syntax::PirDocument;

use crate::emit::{self, diagnostic};
use crate::world::World;

/// One dependency edge, `from` → `to`, with what it was read through.
#[derive(Debug, Clone)]
struct Edge {
    from: usize,
    to: usize,
    /// Span of the reference node that produced it.
    span: Span,
    /// `Some(k)` for an `At(x, k)` edge: instantaneous at that period only.
    at_period: Option<u32>,
    /// True when the edge came from an `init` expression.
    through_init: bool,
}

struct Nodes {
    /// Component name → node index, in `(module, declaration)` order.
    index: BTreeMap<String, usize>,
    names: Vec<String>,
    spans: Vec<Span>,
}

fn nodes(world: &World, docs: &[(usize, &PirDocument)]) -> Nodes {
    let mut index = BTreeMap::new();
    let mut names = Vec::new();
    let mut spans = Vec::new();
    for (_, doc) in docs {
        for c in &doc.components {
            if index.contains_key(&c.name.value) {
                continue;
            }
            index.insert(c.name.value.clone(), names.len());
            names.push(c.name.value.clone());
            spans.push(c.name.span);
        }
    }
    let _ = world;
    Nodes {
        index,
        names,
        spans,
    }
}

/// References under `root`, skipping the operands of aggregate calls: an `Agg`
/// is a whole-series edge and is stage 2, so it is not in `G₀` (§3, §8.2).
fn refs_stage1(arena: &ExprArena, root: ExprId, path: &str) -> Vec<predictable_syntax::Reference> {
    let mut out = Vec::new();
    walk(arena, root, path.to_string(), false, &mut out);
    out
}

/// Every reference under `root`, including those inside aggregates.
fn refs_all(arena: &ExprArena, root: ExprId, path: &str) -> Vec<predictable_syntax::Reference> {
    let mut out = Vec::new();
    walk(arena, root, path.to_string(), true, &mut out);
    out
}

fn walk(
    arena: &ExprArena,
    id: ExprId,
    path: String,
    into_agg: bool,
    out: &mut Vec<predictable_syntax::Reference>,
) {
    match arena.get(id) {
        Expr::Call { func, .. } if predictable_syntax::is_agg_fn(func) && !into_agg => return,
        Expr::Ref(name) => out.push(predictable_syntax::Reference {
            name: name.clone(),
            lag: Lag::Current,
            path: path.clone(),
            span: arena.span(id),
        }),
        Expr::Lag { name, k } => out.push(predictable_syntax::Reference {
            name: name.clone(),
            lag: Lag::Back(*k),
            path: path.clone(),
            span: arena.span(id),
        }),
        Expr::At { name, k } => out.push(predictable_syntax::Reference {
            name: name.clone(),
            lag: Lag::Absolute(*k),
            path: path.clone(),
            span: arena.span(id),
        }),
        Expr::Lookup { table, .. } => out.push(predictable_syntax::Reference {
            name: table.clone(),
            lag: Lag::Table,
            path: path.clone(),
            span: arena.span(id),
        }),
        _ => {}
    }
    for (i, child) in arena.children(id).into_iter().enumerate() {
        walk(arena, child, format!("{path}.{i}"), into_agg, out);
    }
}

/// Build `G₀`: lag-0, stage-1 edges only.
fn instantaneous(nodes: &Nodes, docs: &[(usize, &PirDocument)]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (_, doc) in docs {
        for c in &doc.components {
            let Some(&to) = nodes.index.get(&c.name.value) else {
                continue;
            };
            let Some(expr) = c.expr else { continue };
            for r in refs_stage1(&doc.arena, expr, "expr") {
                let Some(&from) = nodes.index.get(&r.name) else {
                    continue;
                };
                match r.lag {
                    Lag::Current => edges.push(Edge {
                        from,
                        to,
                        span: r.span,
                        at_period: None,
                        through_init: false,
                    }),
                    // `x[0]` is the seed period, and a component's own `init`
                    // covers it: not an instantaneous edge.
                    Lag::Absolute(0) => {}
                    Lag::Absolute(k) => edges.push(Edge {
                        from,
                        to,
                        span: r.span,
                        at_period: Some(k),
                        through_init: false,
                    }),
                    Lag::Back(_) | Lag::Table => {}
                }
            }
        }
    }
    edges
}

/// Build `G_init`: `init` edges plus the whole-series edges an `Agg` creates.
fn init_graph(nodes: &Nodes, docs: &[(usize, &PirDocument)]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (_, doc) in docs {
        for c in &doc.components {
            let Some(&to) = nodes.index.get(&c.name.value) else {
                continue;
            };
            if let Some(init) = c.init {
                for r in refs_all(&doc.arena, init, "init") {
                    if let Some(&from) = nodes.index.get(&r.name) {
                        edges.push(Edge {
                            from,
                            to,
                            span: r.span,
                            at_period: None,
                            through_init: true,
                        });
                    }
                }
            }
            if let Some(expr) = c.expr {
                for r in refs_all(&doc.arena, expr, "expr") {
                    // Lagged reads inside `expr` are already-computed data and
                    // never participate in an `init` chain.
                    if !matches!(r.lag, Lag::Current) && !inside_agg(&doc.arena, expr, &r) {
                        continue;
                    }
                    if let Some(&from) = nodes.index.get(&r.name) {
                        edges.push(Edge {
                            from,
                            to,
                            span: r.span,
                            at_period: None,
                            through_init: false,
                        });
                    }
                }
            }
        }
    }
    edges
}

fn inside_agg(arena: &ExprArena, root: ExprId, r: &predictable_syntax::Reference) -> bool {
    !refs_stage1(arena, root, "expr")
        .iter()
        .any(|s| s.span == r.span)
}

/// Run both cycle checks.
pub fn check(
    map: &SourceMap,
    world: &World,
    docs: &[(usize, &PirDocument)],
    out: &mut Vec<Diagnostic>,
) {
    let nodes = nodes(world, docs);
    let g0 = instantaneous(&nodes, docs);
    let mut reported: Vec<Vec<usize>> = Vec::new();

    for cycle in cycles(nodes.names.len(), &g0) {
        if seen(&mut reported, &cycle.0) {
            continue;
        }
        out.push(same_period(map, &nodes, &cycle.1));
    }

    let ginit = init_graph(&nodes, docs);
    let mut reported_init: Vec<Vec<usize>> = Vec::new();
    for cycle in cycles(nodes.names.len(), &ginit) {
        if !cycle.1.iter().any(|e| e.through_init) {
            continue;
        }
        if seen(&mut reported_init, &cycle.0) {
            continue;
        }
        out.push(through_init(map, &nodes, &cycle.1));
    }
}

fn seen(reported: &mut Vec<Vec<usize>>, members: &[usize]) -> bool {
    let mut key = members.to_vec();
    key.sort_unstable();
    key.dedup();
    if reported.contains(&key) {
        return true;
    }
    reported.push(key);
    false
}

/// Every simple cycle reachable by DFS, as `(member nodes, edges in order)`.
///
/// Deterministic: nodes are visited in declaration order and edges in the order
/// they were built, which is declaration order too — the §3.2 tiebreak applied
/// to diagnostics rather than to evaluation.
fn cycles(n: usize, edges: &[Edge]) -> Vec<(Vec<usize>, Vec<Edge>)> {
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        adjacency[e.from].push(i);
    }

    let mut found = Vec::new();
    let mut colour = vec![0u8; n]; // 0 white, 1 grey, 2 black
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (node, edge index that entered it)

    for start in 0..n {
        if colour[start] != 0 {
            continue;
        }
        stack.push((start, usize::MAX));
        dfs(
            start,
            &adjacency,
            edges,
            &mut colour,
            &mut stack,
            &mut found,
        );
        stack.pop();
    }
    found
}

fn dfs(
    node: usize,
    adjacency: &[Vec<usize>],
    edges: &[Edge],
    colour: &mut Vec<u8>,
    stack: &mut Vec<(usize, usize)>,
    found: &mut Vec<(Vec<usize>, Vec<Edge>)>,
) {
    colour[node] = 1;
    for &ei in &adjacency[node] {
        let edge = &edges[ei];
        let next = edge.to;
        if colour[next] == 1 || next == node {
            // Back edge: the cycle is the stack from `next` onwards.
            // The cycle is the tail of the DFS stack from `next` onwards,
            // closed by this edge.
            let mut members = vec![next];
            let mut cycle_edges = Vec::new();
            let at = stack
                .iter()
                .position(|(n, _)| *n == next)
                .unwrap_or(stack.len().saturating_sub(1));
            for (n, e) in stack.iter().skip(at + 1) {
                members.push(*n);
                cycle_edges.push(edges[*e].clone());
            }
            cycle_edges.push(edge.clone());
            if !members.contains(&node) {
                members.push(node);
            }
            found.push((members, cycle_edges));
            continue;
        }
        if colour[next] == 0 {
            stack.push((next, ei));
            dfs(next, adjacency, edges, colour, stack, found);
            stack.pop();
        }
    }
    colour[node] = 2;
}

/// The `E0201` message of §3.1, verbatim in structure: who depends on whom, why
/// neither can go first, and the lag that fixes it.
fn same_period(map: &SourceMap, nodes: &Nodes, cycle: &[Edge]) -> Diagnostic {
    let edge = primary_edge(nodes, cycle);
    let from = &nodes.names[edge.from];
    let to = &nodes.names[edge.to];
    let chain: Vec<&str> = cycle.iter().map(|e| nodes.names[e.to].as_str()).collect();

    let mut d = diagnostic("E0201", "cyclic dependency in the same period")
        .span(emit::primary(
            map,
            edge.span,
            match edge.at_period {
                Some(k) => format!("`{to}` reads `{from}` at time t = {k}"),
                None => format!("`{to}` reads `{from}` at time t"),
            },
        ))
        .note(format!(
            "{} — with no time lag between them, so neither can be computed first.",
            describe(&chain)
        ))
        .note(
            "Cycles across periods are fine. If this really is simultaneous, it cannot be \
             expressed in a projection IR — solve it outside the loop and feed the result in as \
             an assumption.",
        );
    for other in cycle.iter().filter(|e| e.span != edge.span) {
        d = d.span(emit::secondary(
            map,
            other.span,
            format!(
                "`{}` reads `{}` at time t",
                nodes.names[other.to], nodes.names[other.from]
            ),
        ));
    }
    let text = crate::infer::snippet(map, edge.span);
    d.suggestion(emit::maybe(emit::replace(
        map,
        edge.span,
        match edge.at_period {
            Some(_) => format!("{from}[t-1]"),
            None => format!("{text}[t-1]"),
        },
        format!("read last period's `{from}` instead"),
    )))
    .suggestion(emit::maybe(emit::replace(
        map,
        emit::inner(nodes.spans[edge.to]),
        format!("{to}\"\ninit = \"<seed value at t = 0>"),
        "or seed the recursion with an `init`",
    )))
}

fn through_init(map: &SourceMap, nodes: &Nodes, cycle: &[Edge]) -> Diagnostic {
    let edge = cycle
        .iter()
        .filter(|e| e.through_init)
        .min_by_key(|e| e.to)
        .cloned()
        .unwrap_or_else(|| cycle[0].clone());
    let chain: Vec<&str> = cycle.iter().map(|e| nodes.names[e.to].as_str()).collect();

    diagnostic("E0202", "cyclic dependency through `init`")
        .span(emit::primary(
            map,
            edge.span,
            format!(
                "`{}` is seeded from `{}`",
                nodes.names[edge.to], nodes.names[edge.from]
            ),
        ))
        .note(format!(
            "{} — and an `init` that depends on its own stage-2 result cannot be evaluated in \
             either order.",
            describe(&chain)
        ))
        .note(
            "`init` may read a stage-2 aggregate (01-ir.md §8.2) — that is the one legal backward \
             channel — but the chain of such reads must be acyclic.",
        )
        .suggestion(emit::maybe(emit::replace(
            map,
            edge.span,
            "0.0",
            "seed this one with a constant and break the chain",
        )))
}

/// The edge to caret: the one leaving the earliest-declared member of the cycle.
fn primary_edge(nodes: &Nodes, cycle: &[Edge]) -> Edge {
    let _ = nodes;
    cycle
        .iter()
        .min_by_key(|e| (e.to, e.from))
        .cloned()
        .unwrap_or_else(|| cycle[0].clone())
}

fn describe(chain: &[&str]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for name in chain {
        if !unique.contains(name) {
            unique.push(name);
        }
    }
    if unique.len() == 1 {
        return format!("`{}` depends on itself", unique[0]);
    }
    let mut text = format!("`{}` depends on ", unique[0]);
    text.push_str(
        &unique[1..]
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", which depends on "),
    );
    text.push_str(&format!(", which depends on `{}`", unique[0]));
    text
}
