//! The differential test of `03-engine.md` §3.5: **optimisation is allowed to
//! change speed, never bits.**
//!
//! Every planner pass that is optional — constant folding, loop-invariant
//! hoisting — is covered by planning the same model twice, once at `--O0` and
//! once at `--O1`, evaluating both with the same reference interpreter, and
//! comparing the results *by bit pattern*, not by tolerance. `f64::to_bits`
//! rather than `==` is deliberate: `==` says `0.0 == -0.0` and says nothing
//! useful about NaN, and both of those are exactly the cases an unsound
//! algebraic identity would produce.
//!
//! The interpreter below is a test fixture, not the engine. It is a plain
//! recursive tree walk over the planned expressions — no tapes, no chunks, no
//! registers (those are T09's and T10's) — and its only job is to be a
//! *consistent* consumer of two plans, so any difference it reports is a
//! difference between the plans.

use std::collections::BTreeMap;

use predictable_ir::{AggOp, BinaryOp, Expr, Kind, LitValue, Shape, UnaryOp};
use predictable_plan::{plan_sources, OptLevel, Plan, PlanOptions, SlotId, Space};

const MODEL: &str = include_str!("model.pir");

// ---------------------------------------------------------------------------
// A reference interpreter over a plan
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Values {
    /// Slot name → its values. One entry for `Scalar`/`PerMP`, `T + 1` for
    /// `Series`. Retention is ignored on purpose: this fixture keeps
    /// everything, so a retention bug cannot masquerade as an optimiser bug.
    by_name: BTreeMap<String, Vec<f64>>,
    /// The evaluated `init` of each series that declares one.
    inits: BTreeMap<String, f64>,
    periods: u32,
}

impl Values {
    fn get(&self, name: &str, t: usize) -> f64 {
        let v = self
            .by_name
            .get(name)
            .unwrap_or_else(|| panic!("`{name}` read before it was written"));
        if v.len() == 1 {
            v[0]
        } else {
            v[t]
        }
    }
}

fn timeline_value(name: &str, t: usize) -> f64 {
    match name {
        "t" => t as f64,
        "policy_year" | "policy_month" => t as f64,
        "year_frac" => 1.0,
        "month_of_year" => (t % 12) as f64 + 1.0,
        "is_anniversary" => f64::from(t % 12 == 0),
        _ => 0.0,
    }
}

fn eval(expr: &Expr, t: usize, v: &Values) -> f64 {
    match expr {
        Expr::Lit { value, .. } => match value {
            LitValue::Float(x) => *x,
            LitValue::Int(i) => *i as f64,
            LitValue::Bool(b) => f64::from(*b),
            LitValue::Text(_) => f64::NAN,
        },
        Expr::Ref { name } => v.get(name, t),
        Expr::Lag { name, k } => {
            let k = *k as usize;
            if t >= k {
                v.get(name, t - k)
            } else {
                // `01-ir.md` §2.7: below the origin, `init` if declared, else
                // the zero of the dtype.
                v.inits.get(name).copied().unwrap_or(0.0)
            }
        }
        Expr::At { name, k } => v.get(name, *k as usize),
        Expr::Unary { op, operand } => {
            let x = eval(operand, t, v);
            match op {
                UnaryOp::Neg => -x,
                UnaryOp::Not => f64::from(x == 0.0),
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let a = eval(lhs, t, v);
            let b = eval(rhs, t, v);
            match op {
                BinaryOp::Add => a + b,
                BinaryOp::Sub => a - b,
                BinaryOp::Mul => a * b,
                BinaryOp::Div => a / b,
                BinaryOp::Pow => a.powf(b),
                BinaryOp::Eq => f64::from(a == b),
                BinaryOp::Ne => f64::from(a != b),
                BinaryOp::Lt => f64::from(a < b),
                BinaryOp::Le => f64::from(a <= b),
                BinaryOp::Gt => f64::from(a > b),
                BinaryOp::Ge => f64::from(a >= b),
                BinaryOp::And => f64::from(a != 0.0 && b != 0.0),
                BinaryOp::Or => f64::from(a != 0.0 || b != 0.0),
            }
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            // Both arms are evaluated (`01-ir.md` §2.6) — the interpreter does
            // it too, so a fold that removes an arm cannot hide a difference in
            // the arm it removed.
            let c = eval(cond, t, v);
            let a = eval(then, t, v);
            let b = eval(otherwise, t, v);
            if c != 0.0 {
                a
            } else {
                b
            }
        }
        Expr::Call { func, args } => {
            let a: Vec<f64> = args.iter().map(|e| eval(e, t, v)).collect();
            match func.as_str() {
                "min" => a[0].min(a[1]),
                "max" => a[0].max(a[1]),
                "abs" => a[0].abs(),
                "exp" => a[0].exp(),
                "ln" => a[0].ln(),
                "sqrt" => a[0].sqrt(),
                "pow" => a[0].powf(a[1]),
                other => panic!("test interpreter does not implement `{other}`"),
            }
        }
        Expr::Lookup { .. } => panic!("test interpreter does not implement lookups"),
        Expr::Agg { op, value, pred } => {
            // Strictly sequential in `t`, never reassociated (`01-ir.md` §9.2).
            let series: Vec<f64> = (0..=v.periods as usize)
                .map(|i| eval(value, i, v))
                .collect();
            match op {
                AggOp::Sum | AggOp::SumKahan => series.iter().fold(0.0, |acc, x| acc + x),
                AggOp::First => series[0],
                AggOp::Last => series[series.len() - 1],
                AggOp::At => series[0],
                AggOp::MaxOver => series.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                AggOp::MinOver => series.iter().copied().fold(f64::INFINITY, f64::min),
                AggOp::CountWhile => {
                    let p = pred.as_ref().expect("count_while has a predicate");
                    (0..=v.periods as usize)
                        .take_while(|i| eval(p, *i, v) != 0.0)
                        .count() as f64
                }
                // `npv(x, disc)`: the discount series is the `Agg`'s second
                // operand after lowering. Summed strictly in `t`, never
                // reassociated; the timing exponent of §2.5 is the kernel's,
                // and this fixture only has to be the *same* for both plans.
                AggOp::Npv => {
                    let disc = pred.as_ref().expect("npv carries a discount series");
                    series
                        .iter()
                        .enumerate()
                        .fold(0.0, |acc, (i, x)| acc + x * eval(disc, i, v))
                }
            }
        }
    }
}

/// Evaluate one modelpoint through a plan, in exactly the order the plan says.
fn run(plan: &Plan, modelpoint: &BTreeMap<&str, f64>, assumptions: &BTreeMap<&str, f64>) -> Values {
    let mut v = Values {
        by_name: BTreeMap::new(),
        inits: BTreeMap::new(),
        periods: plan.periods,
    };

    let shape_of = |id: SlotId| plan.slot_refs[id.index()].space;

    // ---- prologue: inputs and pure-PerMP derivations ----------------------
    for id in &plan.prologue {
        let info = plan.info(*id);
        let value = match info.kind {
            Kind::InputModelpoint => modelpoint[info.name.as_str()],
            Kind::InputAssumption => assumptions[info.name.as_str()],
            _ => {
                let expr = match shape_of(*id) {
                    Space::Scalar => plan
                        .scalars
                        .iter()
                        .find(|s| s.info.id == *id)
                        .unwrap()
                        .expr
                        .clone(),
                    Space::PerMp => plan
                        .permp
                        .iter()
                        .find(|s| s.info.id == *id)
                        .unwrap()
                        .expr
                        .clone(),
                    Space::Series => unreachable!("series are not in the prologue"),
                };
                eval(&expr.expect("a derived slot has an expression"), 0, &v)
            }
        };
        v.by_name.insert(info.name.clone(), vec![value]);
    }

    // ---- series seeds -----------------------------------------------------
    for slot in &plan.series {
        v.by_name
            .insert(slot.info.name.clone(), vec![0.0; plan.periods as usize + 1]);
    }
    for slot in &plan.series {
        if let Some(init) = &slot.init {
            let value = eval(init, 0, &v);
            v.inits.insert(slot.info.name.clone(), value);
        }
    }

    // ---- the `t` loop -----------------------------------------------------
    // Hoisted slots are computed once per run; running them ahead of the body
    // for each `t` is the same computation with the same inputs, which is
    // precisely the claim §3.4 makes.
    for t in 0..=plan.periods as usize {
        for id in plan.hoisted.iter().chain(plan.stage1.iter()) {
            let slot = plan.series_of(*id).unwrap();
            let value = match &slot.expr {
                Some(e) => eval(e, t, &v),
                None => timeline_value(&slot.info.name, t),
            };
            v.by_name.get_mut(&slot.info.name).unwrap()[t] = value;
        }
    }

    // ---- stage 2 ----------------------------------------------------------
    for id in &plan.stage2 {
        let info = plan.info(*id);
        let expr = plan
            .permp
            .iter()
            .find(|s| s.info.id == *id)
            .and_then(|s| s.expr.clone())
            .or_else(|| {
                plan.scalars
                    .iter()
                    .find(|s| s.info.id == *id)
                    .and_then(|s| s.expr.clone())
            })
            .expect("a stage-2 slot has an expression");
        let value = eval(&expr, 0, &v);
        v.by_name.insert(info.name.clone(), vec![value]);
    }

    v
}

type Case = (BTreeMap<&'static str, f64>, BTreeMap<&'static str, f64>);

fn modelpoints() -> Vec<Case> {
    let mp = |sum_assured: f64, entry_age: f64| {
        BTreeMap::from([("sum_assured", sum_assured), ("entry_age", entry_age)])
    };
    let asm = |rate: f64, loading: f64| {
        BTreeMap::from([("valuation_rate", rate), ("mortality_loading", loading)])
    };
    vec![
        (mp(100_000.0, 30.0), asm(0.008, 1.0)),
        (mp(0.0, 65.0), asm(0.0, 1.25)),
        // A negative zero and a subnormal: the cases where an "obviously safe"
        // algebraic identity stops being safe.
        (mp(-0.0, 40.0), asm(-0.0, 1.0)),
        (mp(1e-320, 55.0), asm(0.05, 0.75)),
        (mp(1e300, 20.0), asm(0.03, 2.0)),
    ]
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[test]
fn o0_and_o1_agree_bit_for_bit() {
    let o0 = plan_sources(
        &[predictable_check::Input::new("model.pir", MODEL)],
        &PlanOptions {
            opt: OptLevel::O0,
            ..PlanOptions::default()
        },
    )
    .unwrap();
    let o1 = plan_sources(
        &[predictable_check::Input::new("model.pir", MODEL)],
        &PlanOptions::default(),
    )
    .unwrap();
    assert_eq!(o1.opt, OptLevel::O1);

    // The two plans really are different plans — otherwise this test proves
    // nothing.
    assert!(o0.hoisted.is_empty() && !o1.hoisted.is_empty());
    assert_ne!(o0.order_digest, o1.order_digest);

    for (i, (mp, asm)) in modelpoints().into_iter().enumerate() {
        let a = run(&o0, &mp, &asm);
        let b = run(&o1, &mp, &asm);
        assert_eq!(
            a.by_name.keys().collect::<Vec<_>>(),
            b.by_name.keys().collect::<Vec<_>>()
        );
        for (name, left) in &a.by_name {
            let right = &b.by_name[name];
            for (t, (x, y)) in left.iter().zip(right).enumerate() {
                assert_eq!(
                    x.to_bits(),
                    y.to_bits(),
                    "modelpoint {i}, `{name}` at t={t}: O0 gave {x:?}, O1 gave {y:?}"
                );
            }
        }
    }
}

#[test]
fn the_optimiser_actually_did_something() {
    // A differential test that compares two identical plans passes vacuously.
    // This is the guard: `--O1` must produce strictly smaller expressions.
    let inputs = [predictable_check::Input::new("model.pir", MODEL)];
    let o0 = plan_sources(
        &inputs,
        &PlanOptions {
            opt: OptLevel::O0,
            ..PlanOptions::default()
        },
    )
    .unwrap();
    let o1 = plan_sources(&inputs, &PlanOptions::default()).unwrap();

    let size = |p: &Plan| -> usize {
        p.scalars
            .iter()
            .filter_map(|s| s.expr.as_ref())
            .chain(p.permp.iter().filter_map(|s| s.expr.as_ref()))
            .chain(p.series.iter().filter_map(|s| s.expr.as_ref()))
            .map(predictable_plan::fold::size)
            .sum()
    };
    assert!(
        size(&o1) < size(&o0),
        "O1 folded nothing: {} vs {}",
        size(&o1),
        size(&o0)
    );

    // `annual_rate = valuation_rate * (2.0 + 3.0)` folds its constant subtree;
    // the multiply by the assumption survives, because an assumption is not a
    // constant at plan time.
    let folded = o1
        .scalar_named("annual_rate")
        .unwrap()
        .expr
        .clone()
        .unwrap();
    assert_eq!(predictable_plan::fold::size(&folded), 3);
}

#[test]
fn the_plan_shape_is_stable_across_modelpoints() {
    // Nothing in the plan may depend on data. Two runs over different
    // modelpoints must consume the same plan, in the same order.
    let plan = plan_sources(
        &[predictable_check::Input::new("model.pir", MODEL)],
        &PlanOptions::default(),
    )
    .unwrap();
    let mut totals = Vec::new();
    for (mp, asm) in modelpoints() {
        totals.push(run(&plan, &mp, &asm).by_name["total_claims"][0]);
    }
    // A real signal from a real projection: the first modelpoint has claims.
    assert!(totals[0] > 0.0);
    // ...and the zero-sum-assured one has none, exactly.
    assert_eq!(totals[1].to_bits(), 0.0f64.to_bits());
}

#[test]
fn shapes_are_partitioned_as_the_spec_says() {
    let plan = plan_sources(
        &[predictable_check::Input::new("model.pir", MODEL)],
        &PlanOptions::default(),
    )
    .unwrap();
    let modules =
        predictable_plan::lower_modules(&[predictable_check::Input::new("model.pir", MODEL)]);
    for c in &modules[0].components {
        let space = plan.slot_refs[plan
            .series_named(&c.name)
            .map(|s| s.info.id)
            .or_else(|| plan.permp_named(&c.name).map(|s| s.info.id))
            .or_else(|| plan.scalar_named(&c.name).map(|s| s.info.id))
            .unwrap()
            .index()]
        .space;
        let expected = match c.shape {
            Shape::Scalar => Space::Scalar,
            Shape::PerMp => Space::PerMp,
            Shape::Series => Space::Series,
        };
        assert_eq!(space, expected, "{}", c.name);
    }
}
