//! `WOW_DATA=` — set and empty — means **there is no install**, through the real env read.
//!
//! The ladder's own logic is unit-tested purely against `candidates_from`; this covers the wiring,
//! which is the half a pure test cannot see. It is its own file for the reason the sibling
//! `wow_data_env.rs` explains: setting `$WOW_DATA` is process-global, every test in the workspace
//! now resolves its install through that one read, and cargo gives each integration-test file its
//! own process — so one mutating test per file can only reach itself.
//!
//! What it protects: a dev build on a machine with an install cannot otherwise
//! reach the no-install boot path, so nothing exercised it and it rotted into a frame-one panic.
//! This spelling is what `scripts/gates.sh` runs the engine enforcer under.

#[test]
fn an_empty_override_means_no_install() {
    std::env::set_var("WOW_DATA", "");
    assert_eq!(
        benilla_formats::wow_data(),
        None,
        "`WOW_DATA=` must resolve to no install even where one exists"
    );
    assert!(
        benilla_formats::candidates().is_empty(),
        "`WOW_DATA=` must leave nothing on the ladder to report as looked-in"
    );

    std::env::remove_var("WOW_DATA");
}
