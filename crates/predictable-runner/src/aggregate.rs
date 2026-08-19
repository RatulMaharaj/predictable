//! The `[[aggregation]]` stage (`01-ir.md` §8.3, ruling Q4).
//!
//! Aggregation happens strictly after all projections and cannot be expressed in an `Expr` —
//! which is what guarantees modelpoint independence, and therefore both the embarrassing
//! parallelism of §5.1 and bit-identical determinism.
//!
//! Every `op` is a **monoid** (or a pair of them, for `mean` / `weighted_mean`), so a chunk can
//! be folded into a [`Partial`] on whatever thread ran it. The runner then combines partials in
//! **chunk index order, never completion order**, and within a chunk `sum` accumulates
//! sequentially in `mp_row` order. That is the whole determinism argument, and it is why this
//! module never sorts by value and never uses a hash map's iteration order.
//!
//! Groupings do not nest: `group_by` is one ordered key tuple producing one flat row per distinct
//! tuple, rendered `k1=v1|k2=v2` by [`predictable_ir::run::Aggregation::group_key`]. A drill-down
//! tree is built by a UI from successive prefixes, which is why the order is preserved.

use std::collections::BTreeMap;

use predictable_engine::ChunkOutput;
use predictable_io::outbound::AggregateRow;
use predictable_ir::run::{Aggregation, AggregationOp, OverT};

use crate::chunk::{surviving_rows, Chunk, ChunkColumn};

use crate::error::RunError;

/// A group's accumulator. Every variant is a monoid with an identity, so an empty chunk
/// contributes nothing and combination is associative.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Acc {
    sum: f64,
    weight: f64,
    count: u64,
    min: f64,
    max: f64,
}

impl Acc {
    fn identity() -> Acc {
        Acc {
            sum: 0.0,
            weight: 0.0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    fn observe(&mut self, value: f64, weight: f64) {
        self.sum += value * weight;
        self.weight += weight;
        self.count += 1;
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
    }

    fn combine(&mut self, other: &Acc) {
        self.sum += other.sum;
        self.weight += other.weight;
        self.count += other.count;
        if other.min < self.min {
            self.min = other.min;
        }
        if other.max > self.max {
            self.max = other.max;
        }
    }

    fn value(&self, op: AggregationOp) -> f64 {
        match op {
            AggregationOp::Sum => self.sum,
            AggregationOp::Count => self.count as f64,
            AggregationOp::Min => self.min,
            AggregationOp::Max => self.max,
            AggregationOp::Mean | AggregationOp::WeightedMean => {
                if self.weight == 0.0 {
                    0.0
                } else {
                    self.sum / self.weight
                }
            }
        }
    }
}

/// One chunk's contribution to one aggregation: `(group_key, t) -> Acc`, in `BTreeMap` order so
/// that the fold below never depends on a hash seed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Partial {
    groups: BTreeMap<(String, i32), Acc>,
}

impl Partial {
    fn observe(&mut self, key: String, t: i32, value: f64, weight: f64) {
        self.groups
            .entry((key, t))
            .or_insert_with(Acc::identity)
            .observe(value, weight);
    }

    fn combine(&mut self, other: &Partial) {
        for (key, acc) in &other.groups {
            self.groups
                .entry(key.clone())
                .or_insert_with(Acc::identity)
                .combine(acc);
        }
    }

    /// Groups this partial saw. Exposed for the executor-equivalence tests.
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// True when nothing was observed.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// The declared aggregations of a run, folded over chunks.
#[derive(Debug, Clone)]
pub struct AggregationStage {
    specs: Vec<Aggregation>,
    partials: Vec<Partial>,
}

impl AggregationStage {
    /// Bind the `[[aggregation]]` blocks of a run file, in declaration order.
    pub fn new(specs: &[Aggregation]) -> AggregationStage {
        AggregationStage {
            partials: vec![Partial::default(); specs.len()],
            specs: specs.to_vec(),
        }
    }

    /// The component ids an aggregation *computes over* — measures, weights and filters. The
    /// runner adds these to the emission set, because a number nobody emitted cannot be summed
    /// (`01-ir.md` §8.3 requires each of them to be a named component, not an expression).
    pub fn required_components(&self) -> Vec<String> {
        let mut out = Vec::new();
        for spec in &self.specs {
            out.push(spec.measure.clone());
            out.extend(spec.weight.iter().cloned());
            out.extend(spec.filter.iter().cloned());
        }
        out.sort();
        out.dedup();
        out
    }

    /// The `group_by` keys, which are resolved from the chunk's own columns where they are
    /// modelpoint fields and only need emitting when they are derived.
    pub fn group_keys(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .specs
            .iter()
            .flat_map(|s| s.group_by.iter().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// True when the run declared no aggregations — then no `aggregates.parquet` is written.
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Fold one chunk's output into the partials. **Must be called in chunk index order.**
    ///
    /// The chunk itself is passed alongside its output because group keys are read from the
    /// *source* text where they are modelpoint fields: a `str` lane inside the kernel is a
    /// dictionary code private to the engine that ran it (`03-engine.md` §4.2), and a group key
    /// must be the canonical text form of §4.1, equal across workers and across runs.
    pub fn absorb(&mut self, chunk: &Chunk, out: &ChunkOutput) -> Result<(), RunError> {
        for (i, spec) in self.specs.iter().enumerate() {
            let partial = fold_chunk(spec, chunk, out)?;
            self.partials[i].combine(&partial);
        }
        Ok(())
    }

    /// The rows of `aggregates.parquet`: aggregation declaration order, then group key, then
    /// ascending `t` — which is exactly `BTreeMap` order over `(group_key, t)`.
    pub fn finish(&self) -> Vec<AggregateRow> {
        let mut rows = Vec::new();
        for (spec, partial) in self.specs.iter().zip(&self.partials) {
            for ((group_key, t), acc) in &partial.groups {
                rows.push(AggregateRow {
                    aggregation: spec.name.clone(),
                    group_key: group_key.clone(),
                    measure: spec.measure.clone(),
                    t: *t,
                    value: acc.value(spec.op),
                });
            }
        }
        rows
    }

    /// The value of one group of one aggregation — what a `scope = "portfolio"` solve targets.
    pub fn value_of(&self, aggregation: &str, group_key: Option<&str>) -> Option<f64> {
        let i = self.specs.iter().position(|s| s.name == aggregation)?;
        let spec = &self.specs[i];
        let mut total = Acc::identity();
        let mut found = false;
        for ((key, _), acc) in &self.partials[i].groups {
            if group_key.is_some_and(|want| want != key) {
                continue;
            }
            total.combine(acc);
            found = true;
        }
        found.then(|| total.value(spec.op))
    }
}

/// Fold one chunk into one aggregation's partial, sequentially in `mp_row` order.
fn fold_chunk(spec: &Aggregation, chunk: &Chunk, out: &ChunkOutput) -> Result<Partial, RunError> {
    let mut partial = Partial::default();
    let measure = column(out, &spec.measure)?;
    let weight = match &spec.weight {
        Some(w) => Some(column(out, w)?),
        None => None,
    };
    if spec.op == AggregationOp::WeightedMean && weight.is_none() {
        return Err(RunError::AggregationWeightRequired(spec.name.clone()));
    }
    let filter = match &spec.filter {
        Some(f) => Some(column(out, f)?),
        None => None,
    };
    let keys: Vec<KeySource<'_>> = spec
        .group_by
        .iter()
        .map(|k| key_source(chunk, out, k))
        .collect::<Result<_, _>>()?;
    // A trapped modelpoint contributes to no group (§9.3.1), so the surviving lanes of `out`
    // are a *subsequence* of the chunk's. Walk both with one cursor rather than assuming.
    let rows = surviving_rows(chunk, out);

    let series_measure = measure.stride > 1;
    if series_measure && spec.over_t.is_none() {
        // `over_t` defaults to `each` for a series measure (§8.3).
    }
    let over_t = spec.over_t.unwrap_or(OverT::Each);

    for (lane, row) in rows.iter().copied().enumerate().take(measure.lanes()) {
        if filter.is_some_and(|f| f.lane(lane)[0] == 0.0) {
            continue;
        }
        let w = match (spec.op, weight) {
            (AggregationOp::WeightedMean, Some(col)) => col.lane(lane)[0],
            (AggregationOp::Mean, _) => 1.0,
            _ => 1.0,
        };
        let group_key =
            spec.group_key(&keys.iter().map(|k| k.render(row, lane)).collect::<Vec<_>>());
        let values = measure.lane(lane);
        match (series_measure, over_t) {
            (false, _) => partial.observe(group_key, -1, values[0], w),
            (true, OverT::Each) => {
                for (t, v) in values.iter().enumerate() {
                    partial.observe(group_key.clone(), t as i32, *v, w);
                }
            }
            (true, OverT::Total) => {
                // Sequential left-to-right in `t`, exactly as `01-ir.md` §9.2 requires of every
                // reduction, so the total is the same number the kernel's own `sum` would give.
                let mut total = 0.0;
                for v in values {
                    total += *v;
                }
                partial.observe(group_key, -1, total, w);
            }
        }
    }
    Ok(partial)
}

/// Where one group key's values come from: the chunk's own column (text preserved) or, for a
/// derived component, the emitted output lane.
#[derive(Debug)]
enum KeySource<'a> {
    Text(&'a [String]),
    Num(&'a [f64]),
    Emitted(&'a predictable_engine::OutputColumn),
}

impl KeySource<'_> {
    fn render(&self, row: usize, lane: usize) -> String {
        match self {
            KeySource::Text(v) => v[row].clone(),
            KeySource::Num(v) => render(v[row]),
            KeySource::Emitted(c) => render(c.lane(lane)[0]),
        }
    }
}

fn key_source<'a>(
    chunk: &'a Chunk,
    out: &'a ChunkOutput,
    name: &str,
) -> Result<KeySource<'a>, RunError> {
    let bare = name.rsplit('.').next().unwrap_or(name);
    if let Some(col) = chunk.columns.get(name).or_else(|| chunk.columns.get(bare)) {
        return Ok(match col {
            ChunkColumn::Text(v) => KeySource::Text(v),
            ChunkColumn::Num(v) => KeySource::Num(v),
        });
    }
    Ok(KeySource::Emitted(column(out, name)?))
}

fn column<'a>(
    out: &'a ChunkOutput,
    name: &str,
) -> Result<&'a predictable_engine::OutputColumn, RunError> {
    out.column(name)
        .ok_or_else(|| RunError::AggregationInputNotEmitted(name.to_string()))
}

/// The canonical text form of a group key value (`01-ir.md` §4.1, §8.3).
///
/// Group keys are `i64`, `bool`, `str`, `date` or `enum` (an `f64` key is `E0403`), so every
/// value in a lane is an integral `f64`: a code, a flag or a whole number. Rendering it as an
/// integer is what makes `group_key` a stable join key rather than a float's shortest repr.
fn render(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulators_are_monoids() {
        let mut a = Acc::identity();
        let mut b = Acc::identity();
        a.observe(1.0, 1.0);
        a.observe(2.0, 1.0);
        b.observe(4.0, 1.0);
        let mut ab = a;
        ab.combine(&b);
        assert_eq!(ab.value(AggregationOp::Sum), 7.0);
        assert_eq!(ab.value(AggregationOp::Count), 3.0);
        assert_eq!(ab.value(AggregationOp::Min), 1.0);
        assert_eq!(ab.value(AggregationOp::Max), 4.0);
        // Identity.
        let mut with_id = ab;
        with_id.combine(&Acc::identity());
        assert_eq!(with_id, ab);
    }

    #[test]
    fn keys_render_in_canonical_integer_form() {
        assert_eq!(render(2026.0), "2026");
        assert_eq!(render(0.0), "0");
        assert_eq!(render(1.0), "1");
    }
}
