//! The long-format results schema and `results.schema.json` (`04-verify.md` §2).
//!
//! One row per `(modelpoint, component, t)`. It is the only shape that diffs cleanly when two
//! runs have different component sets, which is the whole reason the verification loop can say
//! *where* two runs part company rather than only *that* they do.
//!
//! ```text
//! mp_key    : string                     -- the modelpoint field marked key = true
//! mp_row    : uint32                     -- 0-based row index in the modelpoint file
//! component : dictionary<string>         -- qualified <module_path>.<name>
//! stage     : int8                       -- 1 | 2, copied from the IR
//! t         : int32                      -- -1 for Scalar and PerMP components
//! value     : double                     -- f64 components
//! value_i   : int64                      -- nullable, i64 components
//! value_b   : bool                       -- nullable
//! value_s   : dictionary<string>         -- nullable, str/enum/date(ISO) components
//! ```
//!
//! Exactly one `value*` column is non-null per row, and *which* one is decided by the
//! component's IR `dtype` — never by inspecting the value.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema};
use predictable_ir::model::{Component, Display};
use predictable_ir::types::{DType, Kind, Shape, Timing, Unit};
use serde::{Deserialize, Serialize};

use crate::outbound::error::{IoOutError, Result};

/// `format` of every JSON artefact in the verify layer (`04-verify.md` §1).
pub const FORMAT: &str = "pvf/1";

/// `kind` of `results.schema.json`.
pub const KIND_RESULTS_SCHEMA: &str = "results_schema";

/// The Arrow schema of `results.parquet`. Fixed; consumers may rely on field order.
pub fn results_arrow_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mp_key", DataType::Utf8, false),
        Field::new("mp_row", DataType::UInt32, false),
        Field::new(
            "component",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("stage", DataType::Int8, false),
        Field::new("t", DataType::Int32, false),
        Field::new("value", DataType::Float64, true),
        Field::new("value_i", DataType::Int64, true),
        Field::new("value_b", DataType::Boolean, true),
        Field::new(
            "value_s",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
    ]))
}

/// Which `value*` column a component's rows land in, decided by its `dtype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueColumn {
    /// `value : double` — f64 components. Q15: `double` unconditionally, there is no f32 path.
    F64,
    /// `value_i : int64`
    I64,
    /// `value_b : bool`
    Bool,
    /// `value_s : dictionary<string>` — `str`, `enum` and `date` (rendered ISO-8601).
    Str,
}

impl ValueColumn {
    pub fn of(dtype: &DType) -> ValueColumn {
        match dtype {
            DType::F64 => ValueColumn::F64,
            DType::I64 => ValueColumn::I64,
            DType::Bool => ValueColumn::Bool,
            DType::Str | DType::Date | DType::Enum(_) => ValueColumn::Str,
        }
    }

    pub fn column_name(self) -> &'static str {
        match self {
            ValueColumn::F64 => "value",
            ValueColumn::I64 => "value_i",
            ValueColumn::Bool => "value_b",
            ValueColumn::Str => "value_s",
        }
    }
}

/// One entry of `results.schema.json`'s component list — `{id, name, kind, dtype, shape, unit,
/// timing, stage, output, display}`, copied verbatim from the IR so that a consumer never needs
/// to parse `.pir` to interpret a result set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentDescriptor {
    /// The qualified `<module_path>.<name>` of IR §2.2. The join key everywhere downstream.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub dtype: DType,
    pub shape: Shape,
    pub unit: Unit,
    /// `null` on `Scalar`/`PerMP` — never omitted, because "this value has no timing" is
    /// information (Q14).
    pub timing: Option<Timing>,
    /// `1 | 2`, computed by the IR, never re-derived here.
    pub stage: i8,
    /// True iff `kind = "Output"`. A `--emit all` result set contains `Derived` rows too, and
    /// this flag is how a consumer tells the product's promised manifest from the rest.
    pub output: bool,
    pub display: Option<Display>,
}

impl ComponentDescriptor {
    /// Build a descriptor from an IR component and the module path that qualifies it.
    pub fn from_ir(module_path: &str, c: &Component) -> ComponentDescriptor {
        ComponentDescriptor {
            id: c.qualified_id(module_path),
            name: c.name.clone(),
            kind: c.kind,
            dtype: c.dtype.clone(),
            shape: c.shape,
            unit: c.unit.clone(),
            timing: c.timing,
            // `Stage` numbers itself 1|2 through its `u8` encoding; `as i8` on the enum would
            // silently give 0|1.
            stage: u8::from(c.stage()) as i8,
            output: c.kind.is_emitted(),
            display: c.meta.display.clone(),
        }
    }

    /// The `value*` column this component's rows occupy.
    pub fn value_column(&self) -> ValueColumn {
        ValueColumn::of(&self.dtype)
    }

    /// `t` is `-1` for `Scalar` and `PerMP`, and `>= 0` for `Series` (`04-verify.md` §2).
    pub fn check_t(&self, t: i32) -> Result<()> {
        let ok = match self.shape {
            Shape::Series => t >= 0,
            Shape::Scalar | Shape::PerMp => t == -1,
        };
        if ok {
            return Ok(());
        }
        Err(IoOutError::BadT {
            component: self.id.clone(),
            shape: self.shape.to_string(),
            expected: match self.shape {
                Shape::Series => ">= 0",
                _ => "-1",
            },
            got: t,
        })
    }
}

/// One column of the long-format table, as restated in `results.schema.json` so that a consumer
/// reading only JSON knows the physical layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    /// Arrow type spelling, exactly as `04-verify.md` §2 writes it.
    pub r#type: String,
    pub nullable: bool,
    pub doc: String,
}

fn column(name: &str, ty: &str, nullable: bool, doc: &str) -> ColumnSpec {
    ColumnSpec {
        name: name.to_string(),
        r#type: ty.to_string(),
        nullable,
        doc: doc.to_string(),
    }
}

/// The column list of §2, in physical order.
pub fn columns() -> Vec<ColumnSpec> {
    vec![
        column(
            "mp_key",
            "string",
            false,
            "the modelpoint field marked key = true",
        ),
        column(
            "mp_row",
            "uint32",
            false,
            "0-based row index in the modelpoint file (stable ordering)",
        ),
        column(
            "component",
            "dictionary<string>",
            false,
            "qualified <module_path>.<name>",
        ),
        column("stage", "int8", false, "1 | 2, copied from the IR"),
        column("t", "int32", false, "-1 for Scalar and PerMP components"),
        column("value", "double", true, "f64 components"),
        column("value_i", "int64", true, "i64 components"),
        column("value_b", "bool", true, "bool components"),
        column(
            "value_s",
            "dictionary<string>",
            true,
            "str/enum/date(ISO) components",
        ),
    ]
}

/// `run/results.schema.json` — the component list of the result set plus the physical column
/// layout and `component_set_digest` (IR §8.4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultsSchemaDoc {
    pub format: String,
    pub kind: String,
    /// `emit` mode this set was produced under: `outputs` | `all` | `list`.
    pub emit: String,
    /// SHA-256 over the sorted qualified ids — two sets with equal digests are directly
    /// comparable (IR §8.4.3).
    pub component_set_digest: String,
    pub columns: Vec<ColumnSpec>,
    /// Sorted by `id`, so the document diffs cleanly in git.
    pub components: Vec<ComponentDescriptor>,
    /// Q15: `"f64"` always. The key exists so a consumer never has to assume.
    pub storage_precision: String,

    #[serde(skip)]
    index: HashMap<String, usize>,
}

impl ResultsSchemaDoc {
    /// Build the document from the emitted components. `emit` is the run's `emit` mode.
    pub fn new(emit: &str, mut components: Vec<ComponentDescriptor>) -> Result<ResultsSchemaDoc> {
        components.sort_by(|a, b| a.id.cmp(&b.id));
        let mut index = HashMap::with_capacity(components.len());
        for (i, c) in components.iter().enumerate() {
            if index.insert(c.id.clone(), i).is_some() {
                return Err(IoOutError::DuplicateComponent(c.id.clone()));
            }
        }
        let ids: Vec<&str> = components.iter().map(|c| c.id.as_str()).collect();
        Ok(ResultsSchemaDoc {
            format: FORMAT.to_string(),
            kind: KIND_RESULTS_SCHEMA.to_string(),
            emit: emit.to_string(),
            component_set_digest: predictable_fmt::digest::component_set_digest(&ids),
            columns: columns(),
            components,
            storage_precision: "f64".to_string(),
            index,
        })
    }

    /// Look a component up by qualified id.
    pub fn get(&self, id: &str) -> Option<&ComponentDescriptor> {
        self.index.get(id).map(|i| &self.components[*i])
    }

    /// Look a component up, or fail with the error the writer reports.
    pub fn require(&self, id: &str) -> Result<&ComponentDescriptor> {
        self.get(id)
            .ok_or_else(|| IoOutError::UnknownComponent(id.to_string()))
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Canonical JSON text: keys in the order declared above, two-space indent, LF, one
    /// trailing newline — so the artefact is itself diffable in git (`04-verify.md` §1).
    pub fn to_canonical_json(&self) -> Result<String> {
        crate::outbound::canonical_json(self)
    }

    /// Re-attach the lookup index after deserialisation.
    pub fn reindex(mut self) -> Result<ResultsSchemaDoc> {
        let components = std::mem::take(&mut self.components);
        let mut rebuilt = ResultsSchemaDoc::new(&self.emit, components)?;
        rebuilt.component_set_digest = self.component_set_digest;
        Ok(rebuilt)
    }
}
