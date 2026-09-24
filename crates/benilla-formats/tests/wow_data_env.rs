//! `$WOW_DATA` leads the install ladder of [`benilla_formats::wow_data`]. Setting it is
//! process-global and every test resolves its install through it, so this one test has a file, and
//! so a process, of its own.

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
