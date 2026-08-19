//! Declaration builders shared by the table tests.

use predictable_ir::{
    DType, KeyPolicy, OnMissing, TableDecl, TableKey, TableSource, TableValue, Unit,
};
use predictable_tables::{TableBytes, TableFormat};

pub fn key(name: &str, dtype: DType, policy: KeyPolicy) -> TableKey {
    TableKey {
        name: name.into(),
        dtype,
        policy,
    }
}

pub fn value(name: &str, dtype: DType) -> TableValue {
    TableValue {
        name: name.into(),
        dtype,
        unit: Unit::default(),
    }
}

pub fn decl(name: &str, keys: Vec<TableKey>, values: Vec<TableValue>) -> TableDecl {
    TableDecl {
        name: name.into(),
        keys,
        values,
        on_missing: OnMissing::Error,
        source: TableSource::File(format!("tables/{name}.csv")),
        digest: None,
        rows: None,
        doc: None,
    }
}

pub fn csv(text: &str) -> TableBytes {
    TableBytes {
        bytes: text.as_bytes().to_vec(),
        origin: "test.csv".into(),
        format: TableFormat::Csv,
    }
}
