//! The expression grammar of `01-ir.md` §4.2: precedence, associativity, time
//! indexing, lookups, and what the parser refuses.

use predictable_syntax::{
    expr::{Expr, Lag, Lit, UnaryOp},
    parse_expression, ExprArena, ExprId, SourceMap,
};

fn parse(src: &str) -> (ExprArena, ExprId) {
    let mut sources = SourceMap::new();
    let (arena, id, diags) = parse_expression(&mut sources, "expr", src);
    assert!(
        diags.is_empty(),
        "`{src}` should parse cleanly: {:?}",
        diags.codes()
    );
    (arena, id)
}

/// A parenthesised, fully-associated rendering of the tree — precedence made
/// visible, so the assertions below read as the shape they assert.
fn render(arena: &ExprArena, id: ExprId) -> String {
    match arena.get(id) {
        Expr::Lit(Lit::Int(i)) => i.to_string(),
        Expr::Lit(Lit::Float(f)) => format!("{f:?}"),
        Expr::Lit(Lit::Bool(b)) => b.to_string(),
        Expr::Lit(Lit::Str(s)) => format!("\"{s}\""),
        Expr::Ref(n) => n.clone(),
        Expr::Lag { name, k } => format!("{name}[t-{k}]"),
        Expr::At { name, k } => format!("{name}[{k}]"),
        Expr::Unary { op, operand } => {
            let op = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "not ",
            };
            format!("({op}{})", render(arena, *operand))
        }
        Expr::Binary { op, lhs, rhs } => {
            format!(
                "({} {} {})",
                render(arena, *lhs),
                op.as_str(),
                render(arena, *rhs)
            )
        }
        Expr::If { cond, then_, else_ } => format!(
            "(if {} then {} else {})",
            render(arena, *cond),
            render(arena, *then_),
            render(arena, *else_)
        ),
        Expr::Call { func, args } => {
            let args: Vec<String> = args.iter().map(|a| render(arena, *a)).collect();
            format!("{func}({})", args.join(", "))
        }
        Expr::Lookup { table, keys } => {
            let keys: Vec<String> = keys.iter().map(|k| render(arena, *k)).collect();
            format!("{table}@({})", keys.join(", "))
        }
        Expr::Error => "<error>".to_string(),
    }
}

fn shape(src: &str) -> String {
    let (arena, id) = parse(src);
    render(&arena, id)
}

#[test]
fn precedence_follows_the_grammar() {
    assert_eq!(shape("a + b * c"), "(a + (b * c))");
    assert_eq!(shape("(a + b) * c"), "((a + b) * c)");
    assert_eq!(
        shape("a - b - c"),
        "((a - b) - c)",
        "sum is left-associative"
    );
    assert_eq!(
        shape("a / b / c"),
        "((a / b) / c)",
        "product is left-associative"
    );
    assert_eq!(
        shape("a ^ b ^ c"),
        "(a ^ (b ^ c))",
        "power is right-associative"
    );
    assert_eq!(
        shape("-a ^ b"),
        "(-(a ^ b))",
        "unary binds looser than power"
    );
    assert_eq!(shape("a * -b"), "(a * (-b))");
    assert_eq!(
        shape("a < b and c > d"),
        "((a < b) and (c > d))",
        "cmp binds tighter than and"
    );
    assert_eq!(
        shape("a and b or c"),
        "((a and b) or c)",
        "and binds tighter than or"
    );
    assert_eq!(shape("not a and b"), "((not a) and b)");
    assert_eq!(shape("1 - prob"), "(1 - prob)");
}

#[test]
fn the_ternary_is_a_value_conditional() {
    assert_eq!(
        shape("if in_term then 1.0 else 0.0"),
        "(if in_term then 1.0 else 0.0)"
    );
    // `if` is the loosest form: the arms swallow whole expressions.
    assert_eq!(
        shape("if x == 0 then 0.0 else 1.0 / x"),
        "(if (x == 0) then 0.0 else (1.0 / x))"
    );
    // Nesting through the else-arm is how a chain is written.
    assert_eq!(
        shape("if a then 1 else if b then 2 else 3"),
        "(if a then 1 else (if b then 2 else 3))"
    );
}

#[test]
fn time_indexing_covers_ref_lag_and_at() {
    let (arena, id) = parse("x");
    assert_eq!(arena.get(id), &Expr::Ref("x".into()));

    let (arena, id) = parse("x[t]");
    assert_eq!(
        arena.get(id),
        &Expr::Ref("x".into()),
        "`x[t]` is the same node as `x`"
    );

    let (arena, id) = parse("x[t-1]");
    assert_eq!(
        arena.get(id),
        &Expr::Lag {
            name: "x".into(),
            k: 1
        }
    );

    let (arena, id) = parse("x[t-12]");
    assert_eq!(
        arena.get(id),
        &Expr::Lag {
            name: "x".into(),
            k: 12
        }
    );

    let (arena, id) = parse("x[0]");
    assert_eq!(
        arena.get(id),
        &Expr::At {
            name: "x".into(),
            k: 0
        }
    );

    let (arena, id) = parse("x[12]");
    assert_eq!(
        arena.get(id),
        &Expr::At {
            name: "x".into(),
            k: 12
        }
    );
}

#[test]
fn forward_references_are_rejected_with_the_reason() {
    let mut sources = SourceMap::new();
    let (_, _, diags) = parse_expression(&mut sources, "expr", "x[t+1] + 1");
    assert_eq!(diags.codes(), vec!["E0030"]);
    let d = diags.iter().next().unwrap();
    assert!(d.message.contains("forward"));
    assert!(d
        .labels
        .iter()
        .any(|l| l.message.contains("single forward pass")));
}

#[test]
fn calls_lookups_and_literals() {
    assert_eq!(
        shape("sa8990@(age, gender, smoker)"),
        "sa8990@(age, gender, smoker)"
    );
    assert_eq!(
        shape("npv(premium_income, disc_factor)"),
        "npv(premium_income, disc_factor)"
    );
    assert_eq!(
        shape("compound(expense_inflation, t)"),
        "compound(expense_inflation, t)"
    );
    assert_eq!(shape("round(x, 2)"), "round(x, 2)");
    assert_eq!(shape("clamp(x, 0.0, 1.0)"), "clamp(x, 0.0, 1.0)");
    assert_eq!(shape("t"), "t", "`t` is an ordinary timeline input");
    assert_eq!(shape("gender == \"M\""), "(gender == \"M\")");
    assert_eq!(shape("smoker == true"), "(smoker == true)");
    // A lookup key may itself be an expression.
    assert_eq!(shape("tbl@(age + 1, gender)"), "tbl@((age + 1), gender)");
    // A call with no arguments is legal syntax.
    assert_eq!(shape("f()"), "f()");
}

#[test]
fn retime_tags_are_not_component_references() {
    // §2.5: `retime(x, mid)`'s second operand is a timing tag, not a name — the
    // resolver must not be handed a phantom component called `mid`.
    let (arena, id) = parse("retime(premium_income, mid)");
    let refs = arena.references(id, "expr");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].name, "premium_income");
    match arena.get(id) {
        Expr::Call { args, .. } => {
            assert_eq!(arena.get(args[1]), &Expr::Lit(Lit::Str("mid".into())))
        }
        other => panic!("unexpected: {other:?}"),
    }
    // A component genuinely called `mid` elsewhere is still a reference.
    let (arena, id) = parse("mid * 2");
    assert_eq!(arena.references(id, "expr")[0].name, "mid");
}

#[test]
fn references_carry_lag_and_expr_path() {
    let (arena, id) = parse("(reserve[t-1] + premium_income[t-1]) * (1 + valuation_rate)");
    let refs = arena.references(id, "expr");
    let got: Vec<(&str, Lag, &str)> = refs
        .iter()
        .map(|r| (r.name.as_str(), r.lag, r.path.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("reserve", Lag::Back(1), "expr.lhs.lhs"),
            ("premium_income", Lag::Back(1), "expr.lhs.rhs"),
            ("valuation_rate", Lag::Current, "expr.rhs.rhs"),
        ],
        "the four-edge example of §3.0.1"
    );
    // Every path round-trips back to the node it names.
    for r in &refs {
        let node = arena
            .resolve_path(id, "expr", &r.path)
            .expect("path resolves");
        assert_eq!(arena.span(node), r.span);
    }
}

#[test]
fn lookups_and_calls_produce_key_and_arg_paths() {
    let (arena, id) = parse("max(sa8990@(age, gender), floor_rate)");
    let refs = arena.references(id, "expr");
    let paths: Vec<&str> = refs.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["expr.arg0", "expr.arg0.key0", "expr.arg0.key1", "expr.arg1"]
    );
    assert_eq!(refs[0].lag, Lag::Table, "a lookup depends on its table");
    assert_eq!(refs[0].name, "sa8990");
}

#[test]
fn aggregates_are_detected_for_the_stage_computation() {
    let (arena, id) = parse("npv(premium_income, disc_factor)");
    assert!(arena.contains_agg(id));
    let (arena, id) = parse("pv_claims + pv_expenses - pv_premiums");
    assert!(!arena.contains_agg(id));
    // Nested one level down still makes the component stage-2.
    let (arena, id) = parse("1.0 + sum(x) * 2.0");
    assert!(arena.contains_agg(id));
}

#[test]
fn spans_point_at_the_original_bytes() {
    let src = "num_pols_if * qx";
    let mut sources = SourceMap::new();
    let (arena, id, diags) = parse_expression(&mut sources, "expr", src);
    assert!(diags.is_empty());
    let refs = arena.references(id, "expr");
    assert_eq!(sources.snippet(refs[0].span), "num_pols_if");
    assert_eq!(sources.snippet(refs[1].span), "qx");
    assert_eq!(sources.snippet(arena.span(id)), "num_pols_if * qx");
}

#[test]
fn syntax_errors_are_reported_with_a_code_and_a_span() {
    let cases: &[(&str, &str)] = &[
        ("a +", "E0020"),
        ("a * * b", "E0020"),
        ("(a + b", "E0026"),
        ("npv(a, b", "E0027"),
        ("if a then 1", "E0024"),
        ("if a 1 else 2", "E0023"),
        ("a < b < c", "E0025"),
        ("x[t-0]", "E0029"),
        ("x[t+1]", "E0030"),
        ("x[y]", "E0031"),
        ("x[1", "E0032"),
        ("tbl@age", "E0028"),
        ("a $ b", "E0021"),
        ("a b", "E0022"),
    ];
    for (src, code) in cases {
        let mut sources = SourceMap::new();
        let (_, _, diags) = parse_expression(&mut sources, "expr", *src);
        assert!(
            diags.codes().contains(code),
            "`{src}` should report {code}, got {:?}",
            diags.codes()
        );
        for d in diags.iter() {
            assert!(
                d.primary_span().is_some(),
                "`{src}`: {} has no span",
                d.code
            );
            assert!(d.doc_url.ends_with(d.code));
        }
    }
}

#[test]
fn a_failed_expression_still_yields_a_node() {
    // Recovery contract: the caller always gets an id, and can ask whether the
    // tree is usable rather than having to thread an Option through lowering.
    let mut sources = SourceMap::new();
    let (arena, id, diags) = parse_expression(&mut sources, "expr", "a + ");
    assert!(diags.has_errors());
    assert!(arena.contains_error(id));
}
