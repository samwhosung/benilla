//! The glue screens' Lua surface (`reference/1.12-glue-surface.tsv`) held to its source, the shapes
//! table: every row of a registrar table the glue builder installs is there, with the same binding
//! and arity, and nothing else is.

use std::collections::{BTreeMap, BTreeSet};

/// A vendored `reference/` table's data rows, split on tabs, the header row dropped.
fn reference_rows(file: &str, header: &str) -> Vec<Vec<String>> {
    let path = format!("{}/../../reference/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect::<Vec<_>>())
        .filter(|f| f[0] != header)
        .collect()
}

/// One row of the glue surface table.
pub(super) struct GlueRow {
    pub scope: String,
    pub name: String,
    pub fn_va: String,
    pub table_va: String,
    pub arity: String,
}

/// `reference/1.12-glue-surface.tsv`.
pub(super) fn glue_surface() -> Vec<GlueRow> {
    let rows: Vec<GlueRow> = reference_rows("1.12-glue-surface.tsv", "scope")
        .into_iter()
        .map(|f| GlueRow {
            scope: f[0].clone(),
            name: f[2].clone(),
            fn_va: f[3].clone(),
            table_va: f[4].clone(),
            arity: f[5].clone(),
        })
        .collect();
    assert!(
        rows.len() > 150,
        "the glue surface table looks truncated ({} rows) — regenerate it with \
         scripts/gen-reference-glue-surface.py",
        rows.len()
    );
    rows
}

/// The tables the glue state installs, as the glue registrar `0x46abb0` and the Lua init
/// `0x7039e0` loop them: each `cmp` bound over the 8-byte pair stride.
const GLUE_TABLES: [(&str, usize); 8] = [
    ("0x811e28", 36), // the base array, 0x7039e0
    ("0x8503f4", 6),  // 0x51bb20
    ("0x872e74", 4),  // 0x706350
    ("0x8373b8", 60), // 0x46ddf0
    ("0x838578", 11), // 0x4736c0
    ("0x8380f8", 24), // 0x471a20
    ("0x837ea0", 10), // 0x46f480
    ("0x835a50", 4),  // 0x4587f0
];

/// Each registrar table's rows are exactly the shapes table's rows for that table, binding and
/// arity alike; a compat alias names a library binding the shapes table has.
#[test]
fn the_glue_surface_matches_the_shapes_table() {
    // (table_va, name) -> (fn, arity): a name is not a key in the shapes table.
    let shapes: BTreeMap<(String, String), (String, String)> =
        reference_rows("1.12-shapes.tsv", "name")
            .into_iter()
            .map(|f| ((f[3].clone(), f[0].clone()), (f[1].clone(), f[7].clone())))
            .collect();
    let surface = glue_surface();

    for (table_va, rows) in GLUE_TABLES {
        let in_shapes: BTreeSet<&str> = shapes
            .keys()
            .filter(|(t, _)| t == table_va)
            .map(|(_, n)| n.as_str())
            .collect();
        let in_surface: BTreeSet<&str> = surface
            .iter()
            .filter(|r| r.table_va == table_va)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(
            in_shapes.len(),
            rows,
            "the shapes table's {table_va} has {} rows",
            in_shapes.len()
        );
        assert_eq!(
            in_surface, in_shapes,
            "table {table_va}: the glue surface and the shapes table differ"
        );
    }
    for r in &surface {
        if r.fn_va == "lua" {
            continue;
        }
        if r.table_va == "-" {
            // `__pow`, set by `luaopen_math` outside any table.
            assert_eq!(r.name, "__pow", "{} has no table", r.name);
            continue;
        }
        let shape = shapes
            .iter()
            .find(|((t, _), (f, _))| *t == r.table_va && *f == r.fn_va)
            .map(|(_, s)| s);
        assert_eq!(
            shape.map(|(f, a)| (f.as_str(), a.as_str())),
            Some((r.fn_va.as_str(), r.arity.as_str())),
            "{}: no shapes row in table {} with that binding and arity",
            r.name,
            r.table_va
        );
    }
    let names: BTreeSet<&str> = surface.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names.len(), surface.len(), "a name appears twice");
    // 36 base-array rows, the compat chunk's 39 functions and `__pow`; 6 + 4 + 4 shared; the four
    // glue tables' 105.
    for (scope, n) in [("base", 76), ("shared", 14), ("glue", 105)] {
        assert_eq!(
            surface.iter().filter(|r| r.scope == scope).count(),
            n,
            "{scope} rows"
        );
    }
}
