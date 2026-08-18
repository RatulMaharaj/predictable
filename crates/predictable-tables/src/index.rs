//! Per-key index structures (`03-engine.md` §6).
//!
//! Each declared key gets its own structure, chosen by the key's dtype and
//! `policy`, and each maps a runtime key value to a **domain slot** — the
//! position of that key value in the column's sorted unique domain. Composing
//! the slots into a row is [`crate::compile::TableIndex`]'s job.
//!
//! | Key | Structure |
//! |---|---|
//! | `exact` on `i64`/`bool`, density > 0.5 | dense `Vec<u32>` indexed by `key - min` |
//! | `exact` on `i64`/`bool`, sparser than that | a perfect hash built at load ([`Phf`]) |
//! | `exact` on `str`/`date`/`enum(..)` | sorted dictionary + binary search (enums are unordered, so equality only) |
//! | `clamp` / `step` | sorted domain + binary search; `step` returns the predecessor |
//! | `interpolate` | the same sorted domain plus the blend weight for a fused `lerp` |

use predictable_ir::KeyPolicy;

/// A key value supplied by a `Lookup` at runtime.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyArg<'a> {
    Int(i64),
    Float(f64),
    Bool(bool),
    /// `str`, `date` and `enum(..)` key values.
    Str(&'a str),
}

impl KeyArg<'_> {
    fn as_i64(&self) -> Option<i64> {
        match self {
            KeyArg::Int(i) => Some(*i),
            KeyArg::Bool(b) => Some(*b as i64),
            KeyArg::Float(x) if x.fract() == 0.0 && x.is_finite() => Some(*x as i64),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            KeyArg::Int(i) => Some(*i as f64),
            KeyArg::Bool(b) => Some(*b as i64 as f64),
            KeyArg::Float(x) => Some(*x),
            KeyArg::Str(_) => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            KeyArg::Str(s) => Some(s),
            _ => None,
        }
    }

    /// How the value appears in a miss message.
    pub fn render(&self) -> String {
        match self {
            KeyArg::Int(i) => i.to_string(),
            KeyArg::Float(x) => predictable_fmt::format_f64(*x),
            KeyArg::Bool(b) => b.to_string(),
            KeyArg::Str(s) => (*s).to_string(),
        }
    }
}

/// Where one key value landed in its domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyResolve {
    /// An exact domain slot.
    Slot(u32),
    /// Between two slots: `value = (1 - w) * lo + w * hi`.
    Blend { lo: u32, hi: u32, w: f64 },
    /// No slot. What happens next is `on_missing`'s business.
    Miss,
}

/// Which structure a key compiled to — surfaced so `explain()`, the docs and
/// the tests can assert the choice rather than infer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyIndexKind {
    DenseInt,
    PerfectHash,
    Dictionary,
    SortedInt,
    SortedFloat,
}

impl KeyIndexKind {
    pub fn label(self) -> &'static str {
        match self {
            KeyIndexKind::DenseInt => "dense vector",
            KeyIndexKind::PerfectHash => "perfect hash",
            KeyIndexKind::Dictionary => "sorted dictionary",
            KeyIndexKind::SortedInt => "sorted i64 + binary search",
            KeyIndexKind::SortedFloat => "sorted f64 + binary search",
        }
    }
}

/// The compiled index for one key column.
#[derive(Debug, Clone)]
pub enum KeyIndex {
    /// `exact`, integral, dense: one load and a bounds check.
    DenseInt {
        min: i64,
        /// `slots[k - min]` is the domain position, or `u32::MAX` for a hole.
        slots: Vec<u32>,
    },
    /// `exact`, integral, sparse: a perfect hash built at load.
    PerfectHash(Phf),
    /// `exact` on an unordered dtype: a sorted dictionary, compared by equality.
    Dictionary { domain: Vec<String> },
    /// `clamp` / `step` on an integral key. `as_f64` is the same domain
    /// pre-widened, so the search is one code path and no allocation.
    SortedInt {
        domain: Vec<i64>,
        as_f64: Vec<f64>,
        policy: KeyPolicy,
    },
    /// `clamp` / `step` / `interpolate` on a float key.
    SortedFloat { domain: Vec<f64>, policy: KeyPolicy },
}

impl KeyIndex {
    pub fn kind(&self) -> KeyIndexKind {
        match self {
            KeyIndex::DenseInt { .. } => KeyIndexKind::DenseInt,
            KeyIndex::PerfectHash(_) => KeyIndexKind::PerfectHash,
            KeyIndex::Dictionary { .. } => KeyIndexKind::Dictionary,
            KeyIndex::SortedInt { .. } => KeyIndexKind::SortedInt,
            KeyIndex::SortedFloat { .. } => KeyIndexKind::SortedFloat,
        }
    }

    /// The number of distinct values this key takes — one factor of the
    /// cartesian product.
    pub fn cardinality(&self) -> usize {
        match self {
            KeyIndex::DenseInt { slots, .. } => slots.iter().filter(|&&s| s != u32::MAX).count(),
            KeyIndex::PerfectHash(p) => p.len(),
            KeyIndex::Dictionary { domain } => domain.len(),
            KeyIndex::SortedInt { domain, .. } => domain.len(),
            KeyIndex::SortedFloat { domain, .. } => domain.len(),
        }
    }

    /// True when this key can produce a [`KeyResolve::Blend`].
    pub fn interpolates(&self) -> bool {
        matches!(
            self,
            KeyIndex::SortedFloat {
                policy: KeyPolicy::Interpolate,
                ..
            } | KeyIndex::SortedInt {
                policy: KeyPolicy::Interpolate,
                ..
            }
        )
    }

    /// Map a runtime key value to its domain slot.
    pub fn resolve(&self, arg: KeyArg<'_>) -> KeyResolve {
        match self {
            KeyIndex::DenseInt { min, slots } => {
                let Some(k) = arg.as_i64() else {
                    return KeyResolve::Miss;
                };
                let Some(off) = k.checked_sub(*min) else {
                    return KeyResolve::Miss;
                };
                match usize::try_from(off).ok().and_then(|o| slots.get(o)) {
                    Some(&s) if s != u32::MAX => KeyResolve::Slot(s),
                    _ => KeyResolve::Miss,
                }
            }
            KeyIndex::PerfectHash(phf) => match arg.as_i64().and_then(|k| phf.get(k)) {
                Some(s) => KeyResolve::Slot(s),
                None => KeyResolve::Miss,
            },
            KeyIndex::Dictionary { domain } => match arg.as_str() {
                Some(s) => match domain.binary_search_by(|d| d.as_str().cmp(s)) {
                    Ok(i) => KeyResolve::Slot(i as u32),
                    Err(_) => KeyResolve::Miss,
                },
                None => KeyResolve::Miss,
            },
            KeyIndex::SortedInt { as_f64, policy, .. } => resolve_sorted(as_f64, *policy, arg),
            KeyIndex::SortedFloat { domain, policy } => resolve_sorted(domain, *policy, arg),
        }
    }
}

/// Branchless-ish binary search over a sorted domain, then the policy.
fn resolve_sorted(domain: &[f64], policy: KeyPolicy, arg: KeyArg<'_>) -> KeyResolve {
    let Some(x) = arg.as_f64() else {
        return KeyResolve::Miss;
    };
    if domain.is_empty() || x.is_nan() {
        return KeyResolve::Miss;
    }
    let last = domain.len() - 1;
    // `partition_point` is the standard branch-predictable binary search: the
    // number of elements strictly less than or equal to `x`.
    let le = domain.partition_point(|&d| d <= x);

    match policy {
        KeyPolicy::Exact => {
            if le > 0 && domain[le - 1] == x {
                KeyResolve::Slot((le - 1) as u32)
            } else {
                KeyResolve::Miss
            }
        }
        KeyPolicy::Clamp => {
            if x <= domain[0] {
                return KeyResolve::Slot(0);
            }
            if x >= domain[last] {
                return KeyResolve::Slot(last as u32);
            }
            if domain[le - 1] == x {
                KeyResolve::Slot((le - 1) as u32)
            } else {
                KeyResolve::Miss
            }
        }
        KeyPolicy::Step => {
            if le == 0 {
                // Below the first band: there is no predecessor.
                KeyResolve::Miss
            } else {
                KeyResolve::Slot((le - 1) as u32)
            }
        }
        KeyPolicy::Interpolate => {
            if x <= domain[0] {
                return KeyResolve::Slot(0);
            }
            if x >= domain[last] {
                return KeyResolve::Slot(last as u32);
            }
            let lo = le - 1;
            if domain[lo] == x {
                return KeyResolve::Slot(lo as u32);
            }
            let hi = lo + 1;
            let span = domain[hi] - domain[lo];
            let w = if span == 0.0 {
                0.0
            } else {
                (x - domain[lo]) / span
            };
            KeyResolve::Blend {
                lo: lo as u32,
                hi: hi as u32,
                w,
            }
        }
    }
}

/// A minimal perfect hash over `i64` keys, built by compress-hash-displace.
///
/// Deterministic by construction: bucket order breaks ties on the key value, so
/// the same key set always produces the same table on every platform — which is
/// what `03-engine.md` §7 requires of anything a run's numbers depend on.
#[derive(Debug, Clone)]
pub struct Phf {
    /// Per-bucket displacement seeds.
    disp: Vec<u64>,
    /// Slot → key, `None` where the slot is unused.
    keys: Vec<i64>,
    occupied: Vec<bool>,
    /// Slot → domain position.
    slots: Vec<u32>,
    len: usize,
}

#[inline]
fn mix(mut z: u64) -> u64 {
    // splitmix64's finaliser: cheap, well-distributed, and fixed forever.
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[inline]
fn hash(seed: u64, key: i64) -> u64 {
    mix((key as u64) ^ mix(seed))
}

impl Phf {
    /// Build over `(key, domain position)` pairs. Keys must be distinct.
    pub fn build(entries: &[(i64, u32)]) -> Phf {
        let n = entries.len();
        let buckets = (n / 4).max(1);
        let mut size = (n * 5 / 4).max(1);

        loop {
            if let Some(phf) = Self::try_build(entries, buckets, size) {
                return phf;
            }
            size += size / 2 + 1;
        }
    }

    fn try_build(entries: &[(i64, u32)], buckets: usize, size: usize) -> Option<Phf> {
        let mut by_bucket: Vec<Vec<(i64, u32)>> = vec![Vec::new(); buckets];
        for &(k, v) in entries {
            by_bucket[(hash(0, k) % buckets as u64) as usize].push((k, v));
        }
        // Largest bucket first; ties by the smallest key, so the order is a
        // function of the key set alone.
        let mut order: Vec<usize> = (0..buckets).collect();
        order.sort_by(|&a, &b| {
            by_bucket[b]
                .len()
                .cmp(&by_bucket[a].len())
                .then_with(|| bucket_min(&by_bucket[a]).cmp(&bucket_min(&by_bucket[b])))
        });

        let mut disp = vec![0u64; buckets];
        let mut keys = vec![0i64; size];
        let mut occupied = vec![false; size];
        let mut slots = vec![u32::MAX; size];
        let mut scratch: Vec<usize> = Vec::new();

        for b in order {
            if by_bucket[b].is_empty() {
                continue;
            }
            let mut d = 1u64;
            'seed: loop {
                if d > 1_000_000 {
                    return None;
                }
                scratch.clear();
                for &(k, _) in &by_bucket[b] {
                    let slot = (hash(d, k) % size as u64) as usize;
                    if occupied[slot] || scratch.contains(&slot) {
                        d += 1;
                        continue 'seed;
                    }
                    scratch.push(slot);
                }
                for (&(k, v), &slot) in by_bucket[b].iter().zip(scratch.iter()) {
                    occupied[slot] = true;
                    keys[slot] = k;
                    slots[slot] = v;
                }
                disp[b] = d;
                break;
            }
        }

        Some(Phf {
            disp,
            keys,
            occupied,
            slots,
            len: entries.len(),
        })
    }

    /// The domain position of `key`, if it is in the table.
    #[inline]
    pub fn get(&self, key: i64) -> Option<u32> {
        let b = (hash(0, key) % self.disp.len() as u64) as usize;
        let d = self.disp[b];
        if d == 0 {
            return None;
        }
        let slot = (hash(d, key) % self.keys.len() as u64) as usize;
        if self.occupied[slot] && self.keys[slot] == key {
            Some(self.slots[slot])
        } else {
            None
        }
    }

    /// Number of keys.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Slots allocated — the load factor's denominator.
    pub fn capacity(&self) -> usize {
        self.keys.len()
    }
}

fn bucket_min(bucket: &[(i64, u32)]) -> i64 {
    bucket.iter().map(|&(k, _)| k).min().unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: i64) -> Vec<(i64, u32)> {
        // Hostile spread: negatives, a huge stride, and both extremes.
        let mut v: Vec<i64> = (0..n).map(|i| (i - n / 2) * 1_000_003).collect();
        v.push(i64::MIN);
        v.push(i64::MAX);
        v.sort_unstable();
        v.dedup();
        v.iter()
            .enumerate()
            .map(|(position, &k)| (k, position as u32))
            .collect()
    }

    #[test]
    fn the_perfect_hash_finds_every_key_and_no_others() {
        let entries = keys(10_000);
        let phf = Phf::build(&entries);
        assert_eq!(phf.len(), entries.len());
        for &(k, position) in &entries {
            assert_eq!(phf.get(k), Some(position), "key {k}");
        }
        // Values one away from a real key are not real keys.
        for &(k, _) in entries.iter().take(500) {
            if entries.binary_search_by_key(&(k + 1), |e| e.0).is_err() {
                assert_eq!(phf.get(k + 1), None, "key {}", k + 1);
            }
        }
    }

    #[test]
    fn the_perfect_hash_is_a_function_of_the_key_set_alone() {
        let entries = keys(2_000);
        let a = Phf::build(&entries);
        let mut shuffled = entries.clone();
        shuffled.reverse();
        // The build sorts its buckets deterministically, so input order cannot
        // change the table — which is what §7's bit-reproducibility needs.
        let b = Phf::build(&shuffled);
        assert_eq!(a.capacity(), b.capacity());
        for &(k, _) in &entries {
            assert_eq!(a.get(k), b.get(k));
        }
    }

    #[test]
    fn an_empty_and_a_single_key_table_both_work() {
        let phf = Phf::build(&[]);
        assert!(phf.is_empty());
        assert_eq!(phf.get(0), None);

        let phf = Phf::build(&[(42, 0)]);
        assert_eq!(phf.get(42), Some(0));
        assert_eq!(phf.get(43), None);
    }

    #[test]
    fn step_and_clamp_agree_on_the_grid_and_differ_off_it() {
        let domain = [0.0, 10.0, 20.0];
        assert_eq!(
            resolve_sorted(&domain, KeyPolicy::Step, KeyArg::Float(15.0)),
            KeyResolve::Slot(1)
        );
        assert_eq!(
            resolve_sorted(&domain, KeyPolicy::Clamp, KeyArg::Float(15.0)),
            KeyResolve::Miss
        );
        assert_eq!(
            resolve_sorted(&domain, KeyPolicy::Clamp, KeyArg::Float(-1.0)),
            KeyResolve::Slot(0)
        );
        // NaN never resolves, at any policy.
        for policy in [
            KeyPolicy::Exact,
            KeyPolicy::Clamp,
            KeyPolicy::Step,
            KeyPolicy::Interpolate,
        ] {
            assert_eq!(
                resolve_sorted(&domain, policy, KeyArg::Float(f64::NAN)),
                KeyResolve::Miss
            );
        }
    }
}
