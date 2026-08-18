//! Scalar vocabulary of the IR: `DType`, `Unit`, `Shape`, `Timing`, `Kind`.
//!
//! Normative source: `docs/design/01-ir.md` §2.2–§2.5.
//!
//! `DType` and `Unit` have parameterised forms (`enum(Gender)`, `rate(annual)`), so they
//! serialise as strings and are parsed with [`std::str::FromStr`]. Everything else is a plain
//! string enum whose spelling is fixed by the spec.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::IrError;

/// Shape lattice (§2.3). `Scalar ⊑ PerMP ⊑ Series`; widening is implicit, narrowing never is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Shape {
    Scalar,
    #[serde(rename = "PerMP")]
    PerMp,
    Series,
}

impl Shape {
    /// The broadcast join `⊔` of §2.3. Total, commutative, associative.
    pub fn join(self, other: Shape) -> Shape {
        self.max(other)
    }

    /// True when going from `self` to `to` would be a narrowing (never implicit).
    pub fn is_narrowing_to(self, to: Shape) -> bool {
        to < self
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Shape::Scalar => "Scalar",
            Shape::PerMp => "PerMP",
            Shape::Series => "Series",
        })
    }
}

/// When within period `t` a `Series` value is realised (§2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    Start,
    End,
    Mid,
    Point,
}

impl Timing {
    /// The discount exponent `npv` applies for this timing (§2.5).
    pub fn discount_exponent(self, t: u32) -> f64 {
        match self {
            Timing::Start | Timing::Point => t as f64,
            Timing::End => t as f64 + 1.0,
            Timing::Mid => t as f64 + 0.5,
        }
    }
}

impl fmt::Display for Timing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Timing::Start => "start",
            Timing::End => "end",
            Timing::Mid => "mid",
            Timing::Point => "point",
        })
    }
}

/// Component kind (§2.2). There is deliberately no `Abstract` and no `Aggregate` (Q10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    #[serde(rename = "Input.Modelpoint")]
    InputModelpoint,
    #[serde(rename = "Input.Assumption")]
    InputAssumption,
    #[serde(rename = "Input.Table")]
    InputTable,
    #[serde(rename = "Input.Timeline")]
    InputTimeline,
    Derived,
    Output,
}

impl Kind {
    /// Inputs carry no `expr`.
    pub fn is_input(self) -> bool {
        matches!(
            self,
            Kind::InputModelpoint | Kind::InputAssumption | Kind::InputTable | Kind::InputTimeline
        )
    }

    /// `Output` is `Derived` plus an emission flag — emission is a property of the model (Q2).
    pub fn is_emitted(self) -> bool {
        matches!(self, Kind::Output)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::InputModelpoint => "Input.Modelpoint",
            Kind::InputAssumption => "Input.Assumption",
            Kind::InputTable => "Input.Table",
            Kind::InputTimeline => "Input.Timeline",
            Kind::Derived => "Derived",
            Kind::Output => "Output",
        })
    }
}

/// Value type (§2.4). `f64` is the only float; there is no `f32` and no decimal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DType {
    F64,
    I64,
    Bool,
    Date,
    Str,
    /// `enum(<EnumName>)`, referring to an `[[enum]]` declaration.
    Enum(String),
}

impl DType {
    /// The zero used for a pre-origin `Lag` with no `init` (§2.7). `date`/`str`/`enum` have none.
    pub fn has_zero(&self) -> bool {
        matches!(self, DType::F64 | DType::I64 | DType::Bool)
    }
}

impl fmt::Display for DType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DType::F64 => f.write_str("f64"),
            DType::I64 => f.write_str("i64"),
            DType::Bool => f.write_str("bool"),
            DType::Date => f.write_str("date"),
            DType::Str => f.write_str("str"),
            DType::Enum(name) => write!(f, "enum({name})"),
        }
    }
}

impl FromStr for DType {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "f64" => DType::F64,
            "i64" => DType::I64,
            "bool" => DType::Bool,
            "date" => DType::Date,
            "str" => DType::Str,
            other => {
                let name = parenthesised(other, "enum")
                    .ok_or_else(|| IrError::dtype(other))?
                    .trim();
                if name.is_empty() {
                    return Err(IrError::dtype(other));
                }
                DType::Enum(name.to_string())
            }
        })
    }
}

/// The compounding basis of a `rate` unit (§2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateBasis {
    Annual,
    Monthly,
    Period,
}

impl fmt::Display for RateBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RateBasis::Annual => "annual",
            RateBasis::Monthly => "monthly",
            RateBasis::Period => "period",
        })
    }
}

impl FromStr for RateBasis {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "annual" => RateBasis::Annual,
            "monthly" => RateBasis::Monthly,
            "period" => RateBasis::Period,
            other => return Err(IrError::unit(&format!("rate({other})"))),
        })
    }
}

/// Dimensional tag (§2.4). Checked, never converted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Unit {
    #[default]
    None,
    Money,
    Rate(RateBasis),
    Prob,
    Count,
    Years,
    Months,
    Factor,
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unit::None => f.write_str("none"),
            Unit::Money => f.write_str("money"),
            Unit::Rate(b) => write!(f, "rate({b})"),
            Unit::Prob => f.write_str("prob"),
            Unit::Count => f.write_str("count"),
            Unit::Years => f.write_str("years"),
            Unit::Months => f.write_str("months"),
            Unit::Factor => f.write_str("factor"),
        }
    }
}

impl FromStr for Unit {
    type Err = IrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "none" => Unit::None,
            "money" => Unit::Money,
            "prob" => Unit::Prob,
            "count" => Unit::Count,
            "years" => Unit::Years,
            "months" => Unit::Months,
            "factor" => Unit::Factor,
            other => {
                let basis = parenthesised(other, "rate").ok_or_else(|| IrError::unit(other))?;
                Unit::Rate(basis.trim().parse()?)
            }
        })
    }
}

/// Extract `x` from `head(x)`, or `None` when `s` is not of that form.
fn parenthesised<'a>(s: &'a str, head: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(head)?;
    let rest = rest.strip_prefix('(')?;
    rest.strip_suffix(')')
}

macro_rules! string_serde {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                raw.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

string_serde!(DType);
string_serde!(Unit);
string_serde!(RateBasis);
