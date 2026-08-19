//! [`MpSchema`] — the load-time view of the IR modelpoint schema (`01-ir.md` §2.10).
//!
//! The schema is *taken from the IR*, never inferred from the data file. It is built once, before
//! any source is opened, and carries everything a reader needs to know before it touches row 1:
//! the column list (already pruned to what the model reads), each column's dtype, its typed
//! default, whether it needs a presence lane (§2.11), and — for error messages — which components
//! read it.

use std::collections::{BTreeMap, BTreeSet};

use predictable_ir::{Component, DType, EnumDecl, Expr, LitValue, ModelpointField, Module, Unit};

use crate::error::{IoError, Lint};

/// One column of the load plan.
#[derive(Debug, Clone, PartialEq)]
pub struct MpField {
    /// The field name, as declared in the IR and expected in the file.
    pub name: String,
    /// The declared dtype.
    pub dtype: DType,
    /// The declared unit (carried through for the results writer, T14).
    pub unit: Unit,
    /// `required = true` fields must be present and non-null in every row.
    pub required: bool,
    /// The results join key.
    pub key: bool,
    /// The typed value substituted for a missing or null cell (§2.11). `Some` iff `!required`.
    pub default: Option<DefaultValue>,
    /// True when the model calls `is_null`/`coalesce` on this field, so the loader must
    /// materialise a `bool` `PerMP` presence lane. Opt-in and visible, per §2.11.
    pub presence: bool,
    /// Components whose `expr` or `init` reads the field, in module declaration order.
    pub referenced_by: Vec<String>,
    /// Dictionary-encoded variants when `dtype = enum(...)`, in declaration order.
    pub variants: Vec<String>,
}

impl MpField {
    /// The enum name when this field is enum-typed.
    pub fn enum_name(&self) -> Option<&str> {
        match &self.dtype {
            DType::Enum(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// A default already decoded to the physical representation the lane uses.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    /// `f64` lane.
    F64(f64),
    /// `i64` lane.
    I64(i64),
    /// `bool` lane.
    Bool(bool),
    /// `date` lane: days since the Unix epoch.
    Date(i32),
    /// `str` lane.
    Str(String),
    /// `enum` lane: the dictionary code of the variant.
    Enum(u32),
}

/// The modelpoint load plan: an ordered, pruned, typed column list plus the enum dictionaries.
#[derive(Debug, Clone, PartialEq)]
pub struct MpSchema {
    fields: Vec<MpField>,
    index: BTreeMap<String, usize>,
    key: usize,
    pruned: Vec<String>,
}

impl MpSchema {
    /// Build the load plan for `module`, reading **every** declared field.
    ///
    /// Use [`MpSchema::pruned_for_module`] for the normal path: it drops the columns no component
    /// reads, which is what lets `ParquetSource` read 7 of an MPF's 60 columns.
    pub fn from_module(module: &Module) -> Result<MpSchema, IoError> {
        Self::build(module, false)
    }

    /// Build the load plan for `module`, keeping only the fields some component actually reads
    /// (plus the key field, which the results writer always needs).
    pub fn pruned_for_module(module: &Module) -> Result<MpSchema, IoError> {
        Self::build(module, true)
    }

    fn build(module: &Module, prune: bool) -> Result<MpSchema, IoError> {
        let enums: BTreeMap<&str, &EnumDecl> =
            module.enums.iter().map(|e| (e.name.as_str(), e)).collect();
        let readers = field_readers(module);
        let presence = presence_fields(module);

        let keys: Vec<String> = module
            .modelpoint_fields
            .iter()
            .filter(|f| f.key)
            .map(|f| f.name.clone())
            .collect();
        match keys.len() {
            0 => return Err(IoError::NoKeyField),
            1 => {}
            _ => return Err(IoError::MultipleKeyFields(keys)),
        }

        let mut fields = Vec::new();
        let mut pruned = Vec::new();
        for decl in &module.modelpoint_fields {
            let referenced_by = readers.get(&decl.name).cloned().unwrap_or_default();
            if prune && !decl.key && referenced_by.is_empty() {
                pruned.push(decl.name.clone());
                continue;
            }
            fields.push(mp_field(decl, referenced_by, &presence, &enums)?);
        }

        let index = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i))
            .collect();
        let key = fields
            .iter()
            .position(|f| f.key)
            .expect("key field is never pruned");
        Ok(MpSchema {
            fields,
            index,
            key,
            pruned,
        })
    }

    /// The columns to load, in IR declaration order.
    pub fn fields(&self) -> &[MpField] {
        &self.fields
    }

    /// The number of columns to load.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// True when there is nothing to load (never, in practice: the key field is always kept).
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The field named `name`, if it is part of the load plan.
    pub fn field(&self, name: &str) -> Option<&MpField> {
        self.index.get(name).map(|&i| &self.fields[i])
    }

    /// The position of `name` in the load plan.
    pub fn position(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    /// The `key = true` field.
    pub fn key_field(&self) -> &MpField {
        &self.fields[self.key]
    }

    /// The column names to request from the source — this *is* the projection pushed down into
    /// Parquet.
    pub fn column_names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_str()).collect()
    }

    /// True when `name` is a declared modelpoint field that pruning dropped from this plan. Such
    /// a column in the file is not "unknown" — it is simply not read.
    pub fn is_pruned(&self, name: &str) -> bool {
        self.pruned.iter().any(|p| p == name)
    }

    /// Fields that were dropped by pruning, for the `W`-style lint the caller may surface.
    pub fn pruned_lints(&self, file: &str) -> Vec<Lint> {
        self.pruned
            .iter()
            .map(|column| Lint::PrunedColumn {
                file: file.to_string(),
                column: column.clone(),
            })
            .collect()
    }
}

fn mp_field(
    decl: &ModelpointField,
    referenced_by: Vec<String>,
    presence: &BTreeSet<String>,
    enums: &BTreeMap<&str, &EnumDecl>,
) -> Result<MpField, IoError> {
    let variants = match &decl.dtype {
        DType::Enum(name) => {
            let decl_enum = enums
                .get(name.as_str())
                .ok_or_else(|| IoError::UnknownEnum {
                    column: decl.name.clone(),
                    enum_name: name.clone(),
                })?;
            decl_enum.values.clone()
        }
        _ => Vec::new(),
    };

    let default = if decl.required {
        None
    } else {
        let lit = decl
            .default_value
            .as_ref()
            .ok_or_else(|| IoError::OptionalWithoutDefault {
                column: decl.name.clone(),
            })?;
        Some(decode_default(&decl.name, &decl.dtype, lit, &variants)?)
    };

    Ok(MpField {
        name: decl.name.clone(),
        dtype: decl.dtype.clone(),
        unit: decl.unit.clone(),
        required: decl.required,
        key: decl.key,
        default,
        presence: presence.contains(&decl.name),
        referenced_by,
        variants,
    })
}

fn decode_default(
    column: &str,
    dtype: &DType,
    lit: &LitValue,
    variants: &[String],
) -> Result<DefaultValue, IoError> {
    let bad = |text: String| IoError::BadValue {
        file: "<IR default>".to_string(),
        column: column.to_string(),
        row: 0,
        text,
        expected: dtype.clone(),
    };
    Ok(match (dtype, lit) {
        (DType::F64, LitValue::Float(x)) => DefaultValue::F64(*x),
        (DType::F64, LitValue::Int(i)) => DefaultValue::F64(*i as f64),
        (DType::I64, LitValue::Int(i)) => DefaultValue::I64(*i),
        (DType::Bool, LitValue::Bool(b)) => DefaultValue::Bool(*b),
        (DType::Date, LitValue::Text(s)) => {
            DefaultValue::Date(parse_date(s).ok_or_else(|| bad(s.clone()))?)
        }
        (DType::Str, LitValue::Text(s)) => DefaultValue::Str(s.clone()),
        (DType::Enum(name), LitValue::Text(s)) => {
            let code =
                variants
                    .iter()
                    .position(|v| v == s)
                    .ok_or_else(|| IoError::UnknownVariant {
                        file: "<IR default>".to_string(),
                        column: column.to_string(),
                        row: 0,
                        value: s.clone(),
                        enum_name: name.clone(),
                        expected: variants.to_vec(),
                    })?;
            DefaultValue::Enum(code as u32)
        }
        (_, other) => {
            return Err(bad(match other {
                LitValue::Bool(b) => b.to_string(),
                LitValue::Int(i) => i.to_string(),
                LitValue::Float(x) => x.to_string(),
                LitValue::Text(s) => s.clone(),
            }))
        }
    })
}

/// `YYYY-MM-DD` → days since the Unix epoch, proleptic Gregorian. Small and exact; the crate
/// deliberately does not pull in a calendar dependency for one format.
pub fn parse_date(s: &str) -> Option<i32> {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y: i32 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe as i32 - 719_468)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Every modelpoint field read by some component, mapped to the readers' names.
fn field_readers(module: &Module) -> BTreeMap<String, Vec<String>> {
    let declared: BTreeSet<&str> = module
        .modelpoint_fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for component in &module.components {
        let mut seen = BTreeSet::new();
        for expr in component_exprs(component) {
            collect_refs(expr, &mut seen);
        }
        for name in seen {
            if declared.contains(name.as_str()) {
                out.entry(name).or_default().push(component.name.clone());
            }
        }
    }
    out
}

/// Fields on which the model calls `is_null` / `coalesce` — the only fields that get a presence
/// lane (§2.11).
fn presence_fields(module: &Module) -> BTreeSet<String> {
    let declared: BTreeSet<&str> = module
        .modelpoint_fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let mut out = BTreeSet::new();
    for component in &module.components {
        for expr in component_exprs(component) {
            collect_presence(expr, &declared, &mut out);
        }
    }
    out
}

fn component_exprs(component: &Component) -> impl Iterator<Item = &Expr> {
    component.expr.iter().chain(component.init.iter())
}

fn collect_refs(expr: &Expr, out: &mut BTreeSet<String>) {
    match expr {
        Expr::Ref { name } | Expr::Lag { name, .. } | Expr::At { name, .. } => {
            out.insert(name.clone());
        }
        _ => {}
    }
    for (_, child) in expr.children() {
        collect_refs(child, out);
    }
}

fn collect_presence(expr: &Expr, declared: &BTreeSet<&str>, out: &mut BTreeSet<String>) {
    if let Expr::Call { func, args } = expr {
        let observed: &[Expr] = match func.as_str() {
            "is_null" => args.get(..1).unwrap_or(&[]),
            // `coalesce(x, y)` is sugar for `if is_null(x) then y else x` (§2.11).
            "coalesce" => args.get(..1).unwrap_or(&[]),
            _ => &[],
        };
        for arg in observed {
            if let Expr::Ref { name } | Expr::Lag { name, .. } | Expr::At { name, .. } = arg {
                if declared.contains(name.as_str()) {
                    out.insert(name.clone());
                }
            }
        }
    }
    for (_, child) in expr.children() {
        collect_presence(child, declared, out);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_date;

    #[test]
    fn dates_map_to_days_since_the_epoch() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("1969-12-31"), Some(-1));
        assert_eq!(parse_date("2000-03-01"), Some(11017));
        assert_eq!(parse_date("1982-03-04"), Some(4445));
        // 2000 is a leap year, 1900 is not.
        assert_eq!(
            parse_date("2000-02-29").map(|d| d + 1),
            parse_date("2000-03-01")
        );
    }

    #[test]
    fn malformed_or_impossible_dates_are_rejected() {
        for bad in [
            "",
            "1982-3-4",
            "1982/03/04",
            "1982-13-01",
            "1982-02-30",
            "abcd-ef-gh",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }
}
