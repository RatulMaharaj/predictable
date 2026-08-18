//! The resolved world: every name a `.pir` expression may bind to, with the
//! facts the later passes need about it.
//!
//! The world is built once, before pass 2, from the whole module set of a
//! product (`01-ir.md` §8.4.1). It is deliberately flat: unqualified names are
//! globally unique within a product (§2.2), so `loading` is either one thing or
//! an error (`E0204`), never a scope search.

use std::collections::BTreeMap;

use predictable_syntax::ast::{
    Component, DType, Kind, ModelpointField, Shape, TableDecl, Timing, Unit,
};
use predictable_syntax::source::Span;
use predictable_syntax::PirDocument;

/// Which declaration namespace a name lives in. Resolution suggestions are
/// restricted to the namespace of the *use site* (§7 pass 2), which is why this
/// is carried on every symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ns {
    Component,
    ModelpointField,
    Assumption,
    Table,
    Timeline,
}

impl Ns {
    pub fn describe(self) -> &'static str {
        match self {
            Ns::Component => "component",
            Ns::ModelpointField => "modelpoint field",
            Ns::Assumption => "assumption",
            Ns::Table => "table",
            Ns::Timeline => "timeline input",
        }
    }

    /// The description with its article, for prose: "an assumption named `x`".
    pub fn describe_with_article(self) -> &'static str {
        match self {
            Ns::Component => "a component",
            Ns::ModelpointField => "a modelpoint field",
            Ns::Assumption => "an assumption",
            Ns::Table => "a table",
            Ns::Timeline => "a timeline input",
        }
    }
}

/// One resolvable value name.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub ns: Ns,
    pub dtype: DType,
    pub shape: Shape,
    pub unit: Unit,
    pub timing: Option<Timing>,
    /// The component's `init` expression, if it declares one (§2.7).
    pub has_init: bool,
    /// An optional modelpoint field carries a presence bit (§2.11).
    pub optional_field: bool,
    /// Module the declaration lives in, and its index in the document list.
    pub module: String,
    pub doc_index: usize,
    /// Index of the declaration within its document, the §3.2 order tiebreak.
    pub decl_index: usize,
    /// Span of the declaration's `name` value.
    pub name_span: Span,
}

/// The timeline inputs of §5, always available and never redeclared.
///
/// Each is a `Series`: a timeline value varies with `t` but not with the
/// modelpoint, and §2.3 has no shape for that — the engine hoists it instead.
pub const TIMELINE_FIELDS: &[(&str, DType, Unit)] = &[
    ("t", DType::I64, Unit::None),
    ("period_start_date", DType::Date, Unit::None),
    ("period_end_date", DType::Date, Unit::None),
    ("year_frac", DType::F64, Unit::Years),
    ("month_of_year", DType::I64, Unit::None),
    ("policy_year", DType::I64, Unit::None),
    ("policy_month", DType::I64, Unit::None),
    ("is_anniversary", DType::Bool, Unit::None),
];

/// A component together with the document it was declared in.
#[derive(Debug, Clone, Copy)]
pub struct ComponentRef {
    pub doc_index: usize,
    pub index: usize,
}

/// Everything the checker knows about the model before it looks at a formula.
#[derive(Debug, Default)]
pub struct World {
    /// Value namespace: components, modelpoint fields, assumptions, timeline.
    pub values: BTreeMap<String, Symbol>,
    /// Table namespace, kept separate: `@` cannot be confused with a call (§4.2).
    pub tables: BTreeMap<String, Symbol>,
    /// Table declarations, by name.
    pub table_decls: BTreeMap<String, (usize, TableDecl)>,
    /// Declared enums, by name.
    pub enums: BTreeMap<String, Vec<String>>,
    /// Components in `(module order, declaration order)` — the §3.2 evaluation
    /// order tiebreak and the order every pass iterates in.
    pub components: Vec<ComponentRef>,
    /// `T` from `[timeline].periods`, the sole normative source (§8.4.2).
    pub periods: Option<i64>,
    /// `[timeline].basis`, for the §5 basis-conversion rule.
    pub basis_periods_per_year: Option<i64>,
    /// Names that were declared more than once across modules (`E0204`); the
    /// resolver still binds them so that one shadow does not cascade.
    pub shadowed: Vec<String>,
}

impl World {
    pub fn component(&self, name: &str) -> Option<&Symbol> {
        self.values.get(name).filter(|s| s.ns == Ns::Component)
    }

    pub fn lookup_value(&self, name: &str) -> Option<&Symbol> {
        self.values.get(name)
    }

    /// Candidate names for a "did you mean" suggestion at a value use site.
    pub fn value_names(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }

    pub fn table_names(&self) -> Vec<&str> {
        self.tables.keys().map(String::as_str).collect()
    }
}

/// Build the world from the parsed module documents.
///
/// `docs` is the module set in `product.modules` order when a product file is
/// present, else file order; that order is what makes `E0204`'s "already
/// defined in module `a`" deterministic.
pub fn build(docs: &[(usize, &PirDocument)]) -> World {
    let mut world = World::default();

    for (name, dtype, unit) in TIMELINE_FIELDS {
        world.values.insert(
            (*name).to_string(),
            Symbol {
                name: (*name).to_string(),
                ns: Ns::Timeline,
                dtype: dtype.clone(),
                shape: Shape::Series,
                unit: unit.clone(),
                timing: Some(Timing::Start),
                has_init: false,
                optional_field: false,
                module: "<timeline>".to_string(),
                doc_index: usize::MAX,
                decl_index: 0,
                name_span: Span::new(predictable_syntax::source::FileId(0), 0, 0),
            },
        );
    }

    for (doc_index, doc) in docs {
        let module = doc
            .module
            .as_ref()
            .map(|m| m.value.clone())
            .unwrap_or_default();
        if let Some(tl) = &doc.timeline {
            world.periods.get_or_insert(tl.periods);
            world.basis_periods_per_year.get_or_insert(match tl.basis {
                predictable_syntax::ast::TimelineBasis::Monthly => 12,
                predictable_syntax::ast::TimelineBasis::Quarterly => 4,
                predictable_syntax::ast::TimelineBasis::Annual => 1,
            });
        }
        for e in &doc.enums {
            world.enums.insert(e.name.value.clone(), e.values.clone());
        }
        for (i, f) in doc.modelpoint_fields.iter().enumerate() {
            insert(&mut world, field_symbol(f, &module, *doc_index, i));
        }
        for (i, a) in doc.assumptions.iter().enumerate() {
            insert(
                &mut world,
                Symbol {
                    name: a.name.value.clone(),
                    ns: Ns::Assumption,
                    dtype: a.dtype.clone(),
                    shape: a.shape,
                    unit: a.unit.clone(),
                    timing: None,
                    has_init: false,
                    optional_field: false,
                    module: module.clone(),
                    doc_index: *doc_index,
                    decl_index: i,
                    name_span: a.name.span,
                },
            );
        }
        for (i, c) in doc.components.iter().enumerate() {
            insert(&mut world, component_symbol(c, &module, *doc_index, i));
            world.components.push(ComponentRef {
                doc_index: *doc_index,
                index: i,
            });
        }
        for (i, t) in doc.tables.iter().enumerate() {
            let sym = Symbol {
                name: t.name.value.clone(),
                ns: Ns::Table,
                dtype: t
                    .values
                    .first()
                    .map(|v| v.dtype.clone())
                    .unwrap_or(DType::F64),
                shape: Shape::Scalar,
                unit: t
                    .values
                    .first()
                    .map(|v| v.unit.clone())
                    .unwrap_or(Unit::None),
                timing: None,
                has_init: false,
                optional_field: false,
                module: module.clone(),
                doc_index: *doc_index,
                decl_index: i,
                name_span: t.name.span,
            };
            world.tables.insert(t.name.value.clone(), sym);
            world
                .table_decls
                .insert(t.name.value.clone(), (*doc_index, t.clone()));
        }
    }
    world
}

fn field_symbol(f: &ModelpointField, module: &str, doc_index: usize, i: usize) -> Symbol {
    Symbol {
        name: f.name.value.clone(),
        ns: Ns::ModelpointField,
        dtype: f.dtype.clone(),
        shape: Shape::PerMP,
        unit: f.unit.clone(),
        timing: None,
        has_init: false,
        optional_field: !f.required,
        module: module.to_string(),
        doc_index,
        decl_index: i,
        name_span: f.name.span,
    }
}

fn component_symbol(c: &Component, module: &str, doc_index: usize, i: usize) -> Symbol {
    Symbol {
        name: c.name.value.clone(),
        ns: if c.kind == Kind::InputTimeline {
            Ns::Timeline
        } else {
            Ns::Component
        },
        dtype: c.dtype.clone(),
        shape: c.shape,
        unit: c.unit.clone(),
        timing: c.timing,
        has_init: c.init.is_some(),
        optional_field: false,
        module: module.to_string(),
        doc_index,
        decl_index: i,
        name_span: c.name.span,
    }
}

/// First declaration wins; a second one is recorded as a shadow (`E0204`) but
/// still resolves, so a single shadow does not turn into a cascade of
/// "cannot find" errors at every use site.
fn insert(world: &mut World, sym: Symbol) {
    match world.values.get(&sym.name) {
        Some(existing) if existing.ns == Ns::Timeline && existing.doc_index == usize::MAX => {
            // A model may not redeclare a timeline input; recorded as a shadow.
            world.shadowed.push(sym.name.clone());
        }
        Some(_) => world.shadowed.push(sym.name.clone()),
        None => {
            world.values.insert(sym.name.clone(), sym);
        }
    }
}

/// Levenshtein distance, for namespace-restricted "did you mean" suggestions.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The nearest candidate within an edit distance that scales with the length of
/// the typo: one edit for a short name, up to three for a long one.
pub fn nearest<'a>(needle: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let budget = (needle.chars().count() / 4).clamp(1, 3);
    candidates
        .into_iter()
        .map(|c| (levenshtein(needle, c), c))
        .filter(|(d, _)| *d <= budget)
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)))
        .map(|(_, c)| c)
}
