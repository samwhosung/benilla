//! Shared helpers for the typed DBC loaders. A DBC carries no column types, so each loader supplies
//! a [`Schema`] whose field count must match the file header, and reads fields by index.

use std::collections::HashMap;
use std::io::Cursor;

use anyhow::{anyhow, Context, Result};
use benilla_dbc::{DbcParser, FieldType, Record, RecordSet, Schema, SchemaField, StringRef, Value};

use crate::Chain;

/// Parse a DBC with `schema`; `what` names the file in errors.
pub(crate) fn parse(bytes: &[u8], schema: Schema, what: &str) -> Result<RecordSet> {
    let parser = DbcParser::parse(&mut Cursor::new(bytes))
        .map_err(|e| anyhow!("parsing {what} header: {e}"))?;
    let parser = parser
        .with_schema(schema)
        .map_err(|e| anyhow!("applying {what} schema (field-count mismatch?): {e}"))?;
    parser
        .parse_records()
        .map_err(|e| anyhow!("parsing {what} records: {e}"))
}

/// An unsigned field, from either int variant: the schema tag only picks the decoding.
pub(crate) fn u32_at(r: &Record, i: usize) -> Option<u32> {
    match r.get_value(i)? {
        Value::UInt32(v) => Some(*v),
        Value::Int32(v) => Some(*v as u32),
        _ => None,
    }
}

pub(crate) fn f32_at(r: &Record, i: usize) -> Option<f32> {
    match r.get_value(i)? {
        Value::Float32(v) => Some(*v),
        _ => None,
    }
}

/// A signed field; either int variant reads, bit for bit.
pub(crate) fn i32_at(r: &Record, i: usize) -> Option<i32> {
    match r.get_value(i)? {
        Value::Int32(v) => Some(*v),
        Value::UInt32(v) => Some(*v as i32),
        _ => None,
    }
}

/// A string field from the string block; an empty string reads as `None`.
pub(crate) fn str_at(rs: &RecordSet, r: &Record, i: usize) -> Option<String> {
    match r.get_value(i)? {
        Value::StringRef(StringRef(off)) => {
            let s = rs.get_string(StringRef(*off)).ok()?;
            (!s.is_empty()).then(|| s.to_string())
        }
        _ => None,
    }
}

/// `SpellIcon.dbc`: id → extensionless texture path (`Interface\Icons\…`).
pub(crate) fn load_spell_icon_map(chain: &mut Chain) -> Result<HashMap<u32, String>> {
    let bytes = chain
        .read_file("DBFilesClient\\SpellIcon.dbc")
        .context("reading SpellIcon.dbc")?;
    let mut schema = Schema::new("SpellIcon");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    schema.add_field(SchemaField::new("TextureFilename", FieldType::String));
    let set = parse(&bytes, schema, "SpellIcon.dbc")?;
    let mut icons = HashMap::new();
    for r in set.records() {
        if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&set, r, 1)) {
            icons.insert(id, path);
        }
    }
    Ok(icons)
}

/// An id → name schema of `cols` four-byte columns (a string column is one offset); the other
/// locale slots and the locale flag mask stay anonymous `cN` columns.
pub(crate) fn id_name_schema(what: &str, name_col: usize, cols: usize) -> Schema {
    let mut s = Schema::new(what);
    for i in 0..cols {
        if i == name_col {
            s.add_field(SchemaField::new("Name", FieldType::String));
        } else {
            s.add_field(SchemaField::new(format!("c{i}"), FieldType::UInt32));
        }
    }
    s
}

/// Read an id → name DBC into a map, skipping rows whose name is empty (a hole, not a name).
pub(crate) fn load_id_name_table(
    chain: &mut Chain,
    file: &str,
    name_col: usize,
    cols: usize,
    what: &str,
) -> Result<HashMap<u32, String>> {
    let bytes = chain
        .read_file(file)
        .with_context(|| format!("reading {file}"))?;
    let rs = parse(&bytes, id_name_schema(what, name_col, cols), what)?;
    let mut out = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&rs, r, name_col)) {
            out.insert(id, name);
        }
    }
    Ok(out)
}
