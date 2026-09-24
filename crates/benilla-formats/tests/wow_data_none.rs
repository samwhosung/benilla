//! `WOW_DATA=`, set and empty, means there is no install even where one exists: the spelling
//! `scripts/gates.sh` runs the engine enforcer under to reach the no-install boot path. Setting it
//! is process-global, so this test has a file, and so a process, of its own.

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
