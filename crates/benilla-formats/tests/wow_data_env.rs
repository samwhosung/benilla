//! `$WOW_DATA` still wins — the one env read in [`benilla_formats::wow_data`], covered.
//!
//! **Why this is an integration test and not a unit test**: setting `$WOW_DATA` is
//! process-global, and since the sweep every test in the workspace resolves its install through
//! that one read. A unit test that mutates it poisons whatever else is running in the same test
//! binary, and the victim moves around with the thread scheduling — which is exactly what happened
//! before this was pulled out. Cargo gives each integration-test file its own process, and this
//! file holds one test, so the mutation can only reach itself.
//!
//! The ladder itself is unit-tested purely, against `candidates_from`.

#[test]
fn the_override_is_read_and_wins() {
    let tmp = std::env::temp_dir().join(format!("benilla-wdenv-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    std::env::set_var("WOW_DATA", &tmp);
    assert_eq!(
        benilla_formats::wow_data(),
        Some(tmp.clone()),
        "$WOW_DATA is set to a real directory and must be the answer"
    );
    assert_eq!(
        benilla_formats::candidates().first(),
        Some(&tmp),
        "$WOW_DATA must lead the ladder"
    );

    std::env::remove_var("WOW_DATA");
    assert_ne!(
        benilla_formats::wow_data(),
        Some(tmp.clone()),
        "unset, the override must stop being consulted"
    );
    std::fs::remove_dir_all(&tmp).ok();
}
