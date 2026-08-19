//! Typed columns → a compiled lookup structure (`03-engine.md` §6).
//!
//! Compilation happens once per run, before projection starts. Afterwards a
//! lookup is: one [`KeyIndex::resolve`] per key, one multiply-add to compose the
//! slots into a row, and one load. Multi-key tables go to a **row-major dense
//! array over the cartesian product** whenever that product is under 2²⁴ — the
//! common mortality case (`age × gender × smoker = 121 × 2 × 2 = 484`) — and
//! fall back to a sorted tuple table with binary search when it is not.

use predictable_ir::{DType, KeyPolicy, LitValue, OnMissing, TableDecl};

use crate::error::TableError;
use crate::index::{KeyArg, KeyIndex, KeyIndexKind, KeyResolve, Phf};
use crate::parse::{KeyCell, RawTable, ValueColumn};

/// The cartesian-product ceiling of `03-engine.md` §6.
pub const CARTESIAN_LIMIT: usize = 1 << 24;

/// Sentinel for "no row at this cell".
const EMPTY: u32 = u32::MAX;

/// How the key tuple is turned into a row.
#[derive(Debug, Clone)]
pub enum TableIndex {
    /// Row-major dense array over the cartesian product of the key domains.
    Cartesian {
        /// Row-major strides, one per key.
        strides: Vec<usize>,
        /// `cells[Σ slot_i · stride_i]` is the row, or [`EMPTY`].
        cells: Vec<u32>,
    },
    /// Sorted key-slot tuples + binary search, for products at or over the limit.
    SortedTuples {
        arity: usize,
        /// `arity` slots per entry, in ascending lexicographic order.
        tuples: Vec<u32>,
        /// Parallel row numbers.
        rows: Vec<u32>,
    },
}

impl TableIndex {
    /// The name used in `explain()` and the docs.
    pub fn label(&self) -> &'static str {
        match self {
            TableIndex::Cartesian { .. } => "cartesian dense array",
            TableIndex::SortedTuples { .. } => "sorted tuples + binary search",
        }
    }

    /// Cells (dense) or entries (sparse) allocated.
    pub fn cells(&self) -> usize {
        match self {
            TableIndex::Cartesian { cells, .. } => cells.len(),
            TableIndex::SortedTuples { rows, .. } => rows.len(),
        }
    }

    fn get(&self, slots: &[u32]) -> Option<u32> {
        match self {
            TableIndex::Cartesian { strides, cells } => {
                let mut at = 0usize;
                for (s, stride) in slots.iter().zip(strides) {
                    at += *s as usize * stride;
                }
                match cells.get(at) {
                    Some(&r) if r != EMPTY => Some(r),
                    _ => None,
                }
            }
            TableIndex::SortedTuples {
                arity,
                tuples,
                rows,
            } => {
                let n = rows.len();
                let mut lo = 0usize;
                let mut hi = n;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let entry = &tuples[mid * arity..(mid + 1) * arity];
                    if entry < slots {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                if lo < n && &tuples[lo * arity..(lo + 1) * arity] == slots {
                    Some(rows[lo])
                } else {
                    None
                }
            }
        }
    }
}

/// Where a key tuple landed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowHit {
    /// One row.
    Row(u32),
    /// Two rows and a weight: `(1 - w) · lo + w · hi`.
    Blend { lo: u32, hi: u32, w: f64 },
    /// Nothing. `on_missing` decides what the caller sees.
    Missing,
}

/// The result of a lookup, with the `on_missing` decision already applied.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<T> {
    /// The table carried the value.
    Hit(T),
    /// The table missed and `on_missing = "default(…)"` supplied this.
    Substituted(T),
    /// The table missed and `on_missing = "error"`: the kernel must trap
    /// (`E0902`, `trap = "lookup_miss"`).
    Trap(Miss),
}

impl<T> Outcome<T> {
    /// The value, ignoring *how* it was obtained.
    pub fn value(self) -> Option<T> {
        match self {
            Outcome::Hit(v) | Outcome::Substituted(v) => Some(v),
            Outcome::Trap(_) => None,
        }
    }

    /// True when the value came from the table itself — the presence bit
    /// `is_null` observes (`01-ir.md` §2.11).
    pub fn is_hit(&self) -> bool {
        matches!(self, Outcome::Hit(_))
    }
}

/// Everything a trap report needs about a miss (`01-ir.md` §9.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    pub table: String,
    /// The key values as supplied, in key order.
    pub keys: Vec<String>,
}

impl std::fmt::Display for Miss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no row in table `{}` for ({})",
            self.table,
            self.keys.join(", ")
        )
    }
}

/// What the compiler chose, for `explain()`, the manifest and the docs.
#[derive(Debug, Clone)]
pub struct TableStats {
    pub rows: usize,
    pub key_kinds: Vec<KeyIndexKind>,
    pub index: &'static str,
    pub cells: usize,
    /// `cells / rows` for a dense index: 1.0 is a fully populated grid.
    pub fill: f64,
}

/// A table, loaded, digest-checked and compiled. Immutable during projection.
#[derive(Debug, Clone)]
pub struct CompiledTable {
    name: String,
    key_names: Vec<String>,
    value_names: Vec<String>,
    keys: Vec<KeyIndex>,
    index: TableIndex,
    values: Vec<ValueColumn>,
    rows: usize,
    on_missing: OnMissing,
    default_f64: Option<f64>,
    /// The digest of the bytes actually compiled — the table's identity (§2.9.1).
    pub digest: String,
    /// Where the bytes came from: a path, a `resource:` name, or `inline`.
    pub origin: String,
    /// True when the compiled content differs from the declared `digest` and the
    /// run allowed the drift.
    pub drifted: bool,
}

impl CompiledTable {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn arity(&self) -> usize {
        self.keys.len()
    }

    pub fn value_names(&self) -> &[String] {
        &self.value_names
    }

    pub fn key_names(&self) -> &[String] {
        &self.key_names
    }

    pub fn key_index(&self, i: usize) -> &KeyIndex {
        &self.keys[i]
    }

    pub fn index(&self) -> &TableIndex {
        &self.index
    }

    /// The position of a named value column, for a `Lookup` that names one.
    pub fn value_position(&self, name: &str) -> Option<usize> {
        self.value_names.iter().position(|v| v == name)
    }

    /// True when this table carries a presence bit `is_null` may observe (§2.11).
    pub fn has_presence_bit(&self) -> bool {
        matches!(self.on_missing, OnMissing::Default(_))
    }

    pub fn stats(&self) -> TableStats {
        let cells = self.index.cells();
        TableStats {
            rows: self.rows,
            key_kinds: self.keys.iter().map(KeyIndex::kind).collect(),
            index: self.index.label(),
            cells,
            fill: if cells == 0 {
                0.0
            } else {
                self.rows as f64 / cells as f64
            },
        }
    }

    /// Resolve a key tuple to a row (or a blend, or nothing). This is the whole
    /// index path; the value fetch is separate so a multi-value table probes once.
    pub fn probe(&self, keys: &[KeyArg<'_>]) -> RowHit {
        if keys.len() != self.keys.len() {
            return RowHit::Missing;
        }
        let mut slots: Vec<u32> = Vec::with_capacity(keys.len());
        let mut blend: Option<(usize, u32, u32, f64)> = None;
        for (i, (index, arg)) in self.keys.iter().zip(keys).enumerate() {
            match index.resolve(*arg) {
                KeyResolve::Slot(s) => slots.push(s),
                KeyResolve::Blend { lo, hi, w } => {
                    blend = Some((i, lo, hi, w));
                    slots.push(lo);
                }
                KeyResolve::Miss => return RowHit::Missing,
            }
        }
        match blend {
            None => match self.index.get(&slots) {
                Some(r) => RowHit::Row(r),
                None => RowHit::Missing,
            },
            Some((i, lo, hi, w)) => {
                slots[i] = lo;
                let Some(a) = self.index.get(&slots) else {
                    return RowHit::Missing;
                };
                slots[i] = hi;
                let Some(b) = self.index.get(&slots) else {
                    return RowHit::Missing;
                };
                RowHit::Blend { lo: a, hi: b, w }
            }
        }
    }

    /// True when the key tuple is present — the presence bit of §2.11, which is
    /// only observable on a table with `on_missing = "default(…)"`.
    pub fn contains(&self, keys: &[KeyArg<'_>]) -> bool {
        !matches!(self.probe(keys), RowHit::Missing)
    }

    fn miss(&self, keys: &[KeyArg<'_>]) -> Miss {
        Miss {
            table: self.name.clone(),
            keys: keys.iter().map(KeyArg::render).collect(),
        }
    }

    /// Look up an `f64` value column. Interpolating keys blend here, with the
    /// fused `lerp` of §6: `lo + w · (hi - lo)`.
    pub fn lookup_f64(&self, keys: &[KeyArg<'_>], value: usize) -> Outcome<f64> {
        let ValueColumn::F64(col) = &self.values[value] else {
            return Outcome::Trap(self.miss(keys));
        };
        match self.probe(keys) {
            RowHit::Row(r) => Outcome::Hit(col[r as usize]),
            RowHit::Blend { lo, hi, w } => {
                let (a, b) = (col[lo as usize], col[hi as usize]);
                Outcome::Hit(a + w * (b - a))
            }
            RowHit::Missing => match self.default_f64 {
                Some(d) => Outcome::Substituted(d),
                None => Outcome::Trap(self.miss(keys)),
            },
        }
    }

    /// Look up an `i64` value column. A blend cannot produce an integer, so an
    /// interpolating table with integer values is rejected at compile time.
    pub fn lookup_i64(&self, keys: &[KeyArg<'_>], value: usize) -> Outcome<i64> {
        let ValueColumn::I64(col) = &self.values[value] else {
            return Outcome::Trap(self.miss(keys));
        };
        match self.probe(keys) {
            RowHit::Row(r) | RowHit::Blend { lo: r, .. } => Outcome::Hit(col[r as usize]),
            RowHit::Missing => match &self.on_missing {
                OnMissing::Default(LitValue::Int(i)) => Outcome::Substituted(*i),
                _ => Outcome::Trap(self.miss(keys)),
            },
        }
    }

    /// Look up a `bool` value column.
    pub fn lookup_bool(&self, keys: &[KeyArg<'_>], value: usize) -> Outcome<bool> {
        let ValueColumn::Bool(col) = &self.values[value] else {
            return Outcome::Trap(self.miss(keys));
        };
        match self.probe(keys) {
            RowHit::Row(r) | RowHit::Blend { lo: r, .. } => Outcome::Hit(col[r as usize]),
            RowHit::Missing => match &self.on_missing {
                OnMissing::Default(LitValue::Bool(b)) => Outcome::Substituted(*b),
                _ => Outcome::Trap(self.miss(keys)),
            },
        }
    }

    /// Look up a `str` / `date` / `enum(..)` value column.
    pub fn lookup_str(&self, keys: &[KeyArg<'_>], value: usize) -> Outcome<&str> {
        let ValueColumn::Text(col) = &self.values[value] else {
            return Outcome::Trap(self.miss(keys));
        };
        match self.probe(keys) {
            RowHit::Row(r) | RowHit::Blend { lo: r, .. } => Outcome::Hit(col[r as usize].as_str()),
            RowHit::Missing => match &self.on_missing {
                OnMissing::Default(LitValue::Text(s)) => Outcome::Substituted(s.as_str()),
                _ => Outcome::Trap(self.miss(keys)),
            },
        }
    }
}

/// Compile a parsed table against its declaration.
pub fn compile(
    decl: &TableDecl,
    raw: RawTable,
    digest: String,
    origin: String,
    drifted: bool,
) -> Result<CompiledTable, TableError> {
    if raw.rows == 0 {
        return Err(TableError::Empty {
            table: decl.name.clone(),
        });
    }

    let policies = validate(decl)?;

    // 1. Domains and per-key indexes.
    let mut key_indexes = Vec::with_capacity(decl.keys.len());
    let mut slot_of_row: Vec<Vec<u32>> = Vec::with_capacity(decl.keys.len());
    for (i, key) in decl.keys.iter().enumerate() {
        let column = &raw.keys[i];
        let mut domain: Vec<KeyCell> = column.clone();
        domain.sort_by(KeyCell::cmp_key);
        domain.dedup_by(|a, b| a.cmp_key(b) == std::cmp::Ordering::Equal);

        let slots: Vec<u32> = column
            .iter()
            .map(|c| {
                domain
                    .binary_search_by(|d| d.cmp_key(c))
                    .expect("every cell is in its own domain") as u32
            })
            .collect();

        key_indexes.push(build_key_index(&domain, policies[i], &key.dtype));
        slot_of_row.push(slots);
    }

    // 2. Compose. Row-major cartesian while the product fits, sorted tuples after.
    let cardinalities: Vec<usize> = key_indexes.iter().map(KeyIndex::cardinality).collect();
    let product = cardinalities
        .iter()
        .try_fold(1usize, |acc, &c| acc.checked_mul(c));

    let index = match product {
        Some(p) if p < CARTESIAN_LIMIT => {
            let mut strides = vec![1usize; cardinalities.len()];
            for i in (0..cardinalities.len().saturating_sub(1)).rev() {
                strides[i] = strides[i + 1] * cardinalities[i + 1];
            }
            let mut cells = vec![EMPTY; p];
            // `r` indexes every key's slot column at once, so an iterator over
            // one of them would not be simpler.
            #[allow(clippy::needless_range_loop)]
            for r in 0..raw.rows {
                let at: usize = (0..key_indexes.len())
                    .map(|k| slot_of_row[k][r] as usize * strides[k])
                    .sum();
                if cells[at] != EMPTY {
                    return Err(duplicate(decl, &raw, r, cells[at] as usize));
                }
                cells[at] = r as u32;
            }
            TableIndex::Cartesian { strides, cells }
        }
        _ => {
            let arity = key_indexes.len();
            let mut entries: Vec<(Vec<u32>, u32)> = (0..raw.rows)
                .map(|r| {
                    (
                        (0..arity).map(|k| slot_of_row[k][r]).collect::<Vec<u32>>(),
                        r as u32,
                    )
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            for w in entries.windows(2) {
                if w[0].0 == w[1].0 {
                    return Err(duplicate(decl, &raw, w[1].1 as usize, w[0].1 as usize));
                }
            }
            let mut tuples = Vec::with_capacity(entries.len() * arity);
            let mut rows = Vec::with_capacity(entries.len());
            for (tuple, row) in entries {
                tuples.extend_from_slice(&tuple);
                rows.push(row);
            }
            TableIndex::SortedTuples {
                arity,
                tuples,
                rows,
            }
        }
    };

    let default_f64 = match &decl.on_missing {
        OnMissing::Default(LitValue::Float(x)) => Some(*x),
        OnMissing::Default(LitValue::Int(i)) => Some(*i as f64),
        _ => None,
    };

    Ok(CompiledTable {
        name: decl.name.clone(),
        key_names: decl.keys.iter().map(|k| k.name.clone()).collect(),
        value_names: decl.values.iter().map(|v| v.name.clone()).collect(),
        keys: key_indexes,
        index,
        values: raw.values,
        rows: raw.rows,
        on_missing: decl.on_missing.clone(),
        default_f64,
        digest,
        origin,
        drifted,
    })
}

fn duplicate(decl: &TableDecl, raw: &RawTable, second: usize, first: usize) -> TableError {
    let tuple = raw
        .keys
        .iter()
        .map(|col| col[second].render())
        .collect::<Vec<_>>()
        .join(", ");
    TableError::DuplicateKey {
        table: decl.name.clone(),
        tuple,
        first: first + 1,
        second: second + 1,
    }
}

/// Check the declaration alone — before a byte of content is read, so a
/// `policy` that cannot work is reported as a declaration problem rather than
/// as a cell that failed to parse. Returns the effective per-key policies.
pub fn validate(decl: &TableDecl) -> Result<Vec<KeyPolicy>, TableError> {
    let policies = effective_policies(decl)?;
    validate_values(decl, &policies)?;
    Ok(policies)
}

/// The policy each key is actually compiled with.
///
/// `on_missing = "interpolate(k)"` is the declaration-level way of saying "key
/// `k` interpolates", so it is folded into that key's policy here and the rest
/// of the compiler has one notion of interpolation, not two.
fn effective_policies(decl: &TableDecl) -> Result<Vec<KeyPolicy>, TableError> {
    let mut policies: Vec<KeyPolicy> = decl.keys.iter().map(|k| k.policy).collect();

    if let OnMissing::Interpolate(name) = &decl.on_missing {
        let at = decl
            .keys
            .iter()
            .position(|k| &k.name == name)
            .ok_or_else(|| TableError::UnknownInterpolationKey {
                table: decl.name.clone(),
                key: name.clone(),
            })?;
        policies[at] = KeyPolicy::Interpolate;
    }

    let mut interpolating: Vec<&str> = Vec::new();
    for (key, policy) in decl.keys.iter().zip(&policies) {
        let ordered = matches!(key.dtype, DType::F64 | DType::I64);
        if !ordered && *policy != KeyPolicy::Exact {
            return Err(TableError::UnorderedKeyPolicy {
                table: decl.name.clone(),
                key: key.name.clone(),
                dtype: key.dtype.to_string(),
                policy: policy_name(*policy),
            });
        }
        if key.dtype == DType::F64 && *policy == KeyPolicy::Exact {
            return Err(TableError::FloatExactKey {
                table: decl.name.clone(),
                key: key.name.clone(),
            });
        }
        if *policy == KeyPolicy::Interpolate {
            interpolating.push(&key.name);
        }
    }
    if interpolating.len() > 1 {
        return Err(TableError::MultipleInterpolatedKeys {
            table: decl.name.clone(),
            keys: interpolating
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(" and "),
        });
    }
    Ok(policies)
}

fn validate_values(decl: &TableDecl, policies: &[KeyPolicy]) -> Result<(), TableError> {
    if let OnMissing::Default(lit) = &decl.on_missing {
        for value in &decl.values {
            if !lit.fits(&value.dtype) {
                return Err(TableError::DefaultDtype {
                    table: decl.name.clone(),
                    value: lit.to_string(),
                    column: value.name.clone(),
                    dtype: value.dtype.to_string(),
                });
            }
        }
    }
    if let Some(at) = policies.iter().position(|p| *p == KeyPolicy::Interpolate) {
        for value in &decl.values {
            if value.dtype != DType::F64 {
                return Err(TableError::NonNumericInterpolation {
                    table: decl.name.clone(),
                    key: decl.keys[at].name.clone(),
                    column: value.name.clone(),
                    dtype: value.dtype.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn policy_name(p: KeyPolicy) -> &'static str {
    match p {
        KeyPolicy::Exact => "exact",
        KeyPolicy::Clamp => "clamp",
        KeyPolicy::Step => "step",
        KeyPolicy::Interpolate => "interpolate",
    }
}

/// Pick the structure for one key: the table in [`crate::index`].
fn build_key_index(domain: &[KeyCell], policy: KeyPolicy, dtype: &DType) -> KeyIndex {
    let integral: Option<Vec<i64>> = domain
        .iter()
        .map(|c| match c {
            KeyCell::Int(i) => Some(*i),
            _ => None,
        })
        .collect();

    match (policy, integral) {
        (KeyPolicy::Exact, Some(ints)) => {
            let (min, max) = (ints[0], ints[ints.len() - 1]);
            let span = (max - min).unsigned_abs() as u128 + 1;
            let dense = span <= CARTESIAN_LIMIT as u128 && ints.len() as f64 / span as f64 > 0.5;
            if dense {
                let mut slots = vec![EMPTY; span as usize];
                for (position, &k) in ints.iter().enumerate() {
                    slots[(k - min) as usize] = position as u32;
                }
                KeyIndex::DenseInt { min, slots }
            } else {
                let entries: Vec<(i64, u32)> = ints
                    .iter()
                    .enumerate()
                    .map(|(position, &k)| (k, position as u32))
                    .collect();
                KeyIndex::PerfectHash(Phf::build(&entries))
            }
        }
        (KeyPolicy::Exact, None) => KeyIndex::Dictionary {
            domain: domain
                .iter()
                .map(|c| match c {
                    KeyCell::Text(s) => s.clone(),
                    other => other.render(),
                })
                .collect(),
        },
        (policy, Some(ints)) if *dtype != DType::F64 => KeyIndex::SortedInt {
            as_f64: ints.iter().map(|&i| i as f64).collect(),
            domain: ints,
            policy,
        },
        (policy, _) => KeyIndex::SortedFloat {
            domain: domain
                .iter()
                .map(|c| match c {
                    KeyCell::Float(x) => *x,
                    KeyCell::Int(i) => *i as f64,
                    KeyCell::Text(_) => f64::NAN,
                })
                .collect(),
            policy,
        },
    }
}
