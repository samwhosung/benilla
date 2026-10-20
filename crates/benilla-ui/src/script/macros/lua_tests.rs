//! The macro verbs as an addon meets them: how the table is ordered and addressed, what each verb
//! answers, which limits it keeps and what a delete clears. Every case goes through Lua.

use crate::script::tick::heard;
use crate::script::UiScript;

/// A VM whose icon chooser lists two icons, so `CreateMacro`'s icon number resolves.
fn script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_macro_icons(vec![
        "Interface\\Icons\\Ability_Ambush".into(),
        "Interface\\Icons\\Ability_BackStab".into(),
    ]);
    s
}

fn make(s: &UiScript, name: &str, per_character: bool) -> i64 {
    s.eval::<i64>(&format!(
        r#"return CreateMacro("{name}", 1, "", nil, {})"#,
        if per_character { "1" } else { "nil" }
    ))
    .unwrap()
}

/// The names `GetMacroInfo` gives for indices `from..=to`, `-` where it gives nil.
fn names(s: &UiScript, from: i64, to: i64) -> Vec<String> {
    s.eval(&format!(
        "local t = {{}} for i = {from}, {to} do table.insert(t, GetMacroInfo(i) or '-') end return t"
    ))
    .unwrap()
}

/// `0x4f0d30` sorts each scope by name without case, and `CreateMacro` answers the index the new
/// macro lands on once the table is rebuilt (`0x4f16ba`): the character scope sorts on its own and
/// starts at 19.
#[test]
fn a_scope_sits_by_name_and_a_create_answers_where_it_landed() {
    let s = script();
    assert_eq!(make(&s, "Charlie", false), 1);
    assert_eq!(
        make(&s, "alpha", false),
        1,
        "ahead of Charlie, whatever its case"
    );
    assert_eq!(make(&s, "Bravo", false), 2);
    assert_eq!(names(&s, 1, 4), ["alpha", "Bravo", "Charlie", "-"]);
    assert_eq!(
        s.eval::<i64>(r#"return GetMacroIndexByName("charlie")"#)
            .unwrap(),
        3
    );

    assert_eq!(make(&s, "zed", true), 19);
    assert_eq!(make(&s, "Yak", true), 19);
    assert_eq!(names(&s, 18, 21), ["-", "Yak", "zed", "-"]);
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (3, 2)
    );
}

/// `SStrCmpI` is `_strnicmp` (`0x414310`): it folds `A` to `Z` to lower case, so `[` (0x5b) sorts
/// below every letter and `_` below `a`.
#[test]
fn the_compare_folds_to_lower_case() {
    let s = script();
    make(&s, "Zed", false);
    make(&s, "[bracket", false);
    make(&s, "ax", false);
    make(&s, "_x", false);
    assert_eq!(names(&s, 1, 4), ["[bracket", "_x", "ax", "Zed"]);
}

/// `qsort` (`0x73f727`) is not stable. A shortsort of `b, a, B` moves the first `b` behind the
/// `B` that only ties it, so the pair ends in the reverse of the order it was made in.
#[test]
fn names_that_tie_sit_where_the_crt_sort_leaves_them() {
    let s = script();
    make(&s, "b", false);
    make(&s, "a", false);
    make(&s, "B", false);
    assert_eq!(names(&s, 1, 3), ["a", "B", "b"]);
    // The first match in list order, which is the order they were made in.
    assert_eq!(
        s.eval::<i64>(r#"return GetMacroIndexByName("B")"#).unwrap(),
        3,
        "the older `b` is the first match, and sits third"
    );
}

/// `EditMacro` rebuilds the table (`0x4f1322`) and answers the macro's index in it (`0x4f19c4`).
#[test]
fn an_edit_re_sorts_and_answers_the_new_index() {
    let s = script();
    make(&s, "a", false);
    make(&s, "b", false);
    make(&s, "c", false);
    assert_eq!(s.eval::<i64>(r#"return EditMacro(1, "z")"#).unwrap(), 3);
    assert_eq!(names(&s, 1, 3), ["b", "c", "z"]);
    assert_eq!(
        s.eval::<i64>(r#"return EditMacro(2, nil, nil, "/say hi")"#)
            .unwrap(),
        2,
        "a body save moves nothing"
    );
}

/// An action slot holds the macro's id (`macroId | 0x40000000`, `0x4e62e0`), so a macro made or
/// deleted ahead of it moves its index and leaves its button alone.
#[test]
fn a_bar_slot_keeps_its_macro_when_the_indices_move() {
    let s = script();
    make(&s, "b", false);
    make(&s, "d", false);
    s.run("PickupMacro(2) PlaceAction(5)").unwrap();
    let text = |s: &UiScript| s.eval::<Option<String>>("return GetActionText(5)").unwrap();
    assert_eq!(text(&s).as_deref(), Some("d"));

    make(&s, "a", false);
    assert_eq!(text(&s).as_deref(), Some("d"), "d moved to index 3");
    s.run("DeleteMacro(1)").unwrap();
    assert_eq!(text(&s).as_deref(), Some("d"), "d moved to index 2");
    assert_eq!(names(&s, 1, 2), ["b", "d"]);
}

/// A macro on the cursor is the id too: it is placed whole after the table re-sorts under it.
#[test]
fn a_held_macro_survives_the_table_moving() {
    let s = script();
    make(&s, "b", false);
    make(&s, "d", false);
    s.run("PickupMacro(2)").unwrap();
    s.run("DeleteMacro(1)").unwrap();
    s.run("PlaceAction(5)").unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return GetActionText(5)")
            .unwrap()
            .as_deref(),
        Some("d")
    );
}

/// `DeleteMacro` clears every slot holding the id (`0x4f1449` into `0x4e5d10`), lowest first, each
/// sent as 0 and signalled; the slots of other macros are left.
#[test]
fn a_delete_clears_each_slot_that_holds_the_macro_and_no_other() {
    let mut s = script();
    make(&s, "b", false);
    make(&s, "d", false);
    s.run(
        "PickupMacro(2) PlaceAction(9) PickupMacro(2) PlaceAction(5) PickupMacro(1) PlaceAction(7)",
    )
    .unwrap();
    s.take_action_sets();
    heard::listen(&s, &["ACTIONBAR_SLOT_CHANGED"], Some("arg1"));

    s.run("DeleteMacro(2)").unwrap();
    assert_eq!(
        s.eval::<(Option<i64>, Option<i64>, Option<i64>)>(
            "return HasAction(5), HasAction(9), HasAction(7)"
        )
        .unwrap(),
        (None, None, Some(1))
    );
    assert_eq!(s.take_action_sets(), [(5, 0), (9, 0)]);
    assert_eq!(
        heard::take(&s),
        ["ACTIONBAR_SLOT_CHANGED 5", "ACTIONBAR_SLOT_CHANGED 9"]
    );
}

/// `SStrCopy` into the record's 0x40-byte name (`0x4f11fc`, `0x4f12e1`) keeps 63 bytes and does not
/// trim; the body keeps 256 characters (`0x4f1580`). The stock edit boxes stop shorter on their own.
#[test]
fn the_verbs_keep_a_63_byte_name_and_a_256_character_body() {
    let s = script();
    s.run(&format!(
        r#"CreateMacro("{}", 1, "{}")"#,
        "N".repeat(100),
        "b".repeat(300)
    ))
    .unwrap();
    let (name, body) = s
        .eval::<(String, String)>("local n, _, b = GetMacroInfo(1) return n, b")
        .unwrap();
    assert_eq!((name.len(), body.len()), (63, 256));

    s.run(&format!(
        r#"EditMacro(1, "{}", nil, "{}")"#,
        "M".repeat(70),
        "c".repeat(257)
    ))
    .unwrap();
    let (name, body) = s
        .eval::<(String, String)>("local n, _, b = GetMacroInfo(1) return n, b")
        .unwrap();
    assert_eq!((name.len(), body.len()), (63, 256));

    // A cut inside a character stops at its boundary: 62 of the 63 bytes of 40 two-byte characters
    // (the deviation of `cut_name`).
    s.run(&format!(r#"EditMacro(1, "{}")"#, "\u{e9}".repeat(40)))
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GetMacroInfo(1)").unwrap(),
        "\u{e9}".repeat(31)
    );

    // Not trimmed, and an empty name is a name: the worker copies whatever string it is given.
    s.run(r#"EditMacro(1, "  padded  ")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return GetMacroInfo(1)").unwrap(),
        "  padded  "
    );
    s.run(r#"EditMacro(1, "")"#).unwrap();
    assert_eq!(s.eval::<String>("return GetMacroInfo(1)").unwrap(), "");
}

/// A body is copied through 0x400 bytes (`0x4f1556`) and then counted to 256 characters. 255
/// four-byte characters and two more fit the 1023 bytes, so the 256th character is kept and the
/// 257th cut; 300 four-byte characters do not, and the 256th, cut in its fourth byte, is dropped
/// whole, because the decoder returns -1 at the NUL inside it (`0x41ab69` into `0x41aada`).
#[test]
fn a_body_is_cut_to_1023_bytes_before_it_is_counted() {
    let s = script();
    let kept = |body: String| {
        s.run(&format!(r#"CreateMacro("m", 1, "{body}")"#)).unwrap();
        let kept = s
            .eval::<String>("local _, _, b = GetMacroInfo(1) return b")
            .unwrap();
        s.run("DeleteMacro(1)").unwrap();
        kept
    };
    let emoji = "\u{1F600}";
    let fits = kept(format!("{}ab", emoji.repeat(255)));
    assert_eq!(fits, format!("{}a", emoji.repeat(255)));
    assert_eq!(kept(emoji.repeat(300)), emoji.repeat(255));
}

/// `EditMacro` raises its usage text unless argument 1 is a number or a numeric string
/// (`0x4f18be`-`0x4f18c7`), and answers a number whichever index it is given.
#[test]
fn edit_macro_raises_its_usage_error_for_a_first_argument_that_is_no_number() {
    let s = script();
    make(&s, "m", false);
    for call in [
        "EditMacro()",
        "EditMacro(nil)",
        "EditMacro('x', 'n')",
        "EditMacro({})",
        "EditMacro(true)",
    ] {
        let err = s.run(call).expect_err(call).to_string();
        assert!(
            err.contains("Usage: EditMacro(index, name, icon, body, local)"),
            "{call}: {err}"
        );
    }
    // A numeric string is its number (`0x6f34d0`).
    assert_eq!(s.eval::<i64>(r#"return EditMacro("1", "n")"#).unwrap(), 1);
    assert_eq!(s.eval::<String>("return GetMacroInfo(1)").unwrap(), "n");
}

/// An index that names no macro has id 0, and `0x4f0ec0(0)` finds the first empty entry: the next
/// free index of the account scope, else of the character scope, else -1 plus one (`0x4f19c4`).
/// Nothing is created or changed.
#[test]
fn edit_macro_answers_the_first_empty_index_for_an_index_with_no_macro() {
    let s = script();
    make(&s, "a", false);
    make(&s, "b", false);
    for index in [3, 18, 30, 36, 37, 0, -1] {
        assert_eq!(
            s.eval::<i64>(&format!(r#"return EditMacro({index}, "x")"#))
                .unwrap(),
            3,
            "EditMacro({index})"
        );
    }
    assert_eq!(names(&s, 1, 3), ["a", "b", "-"]);
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (2, 0)
    );

    // With the account scope full, the next free place is the character scope's.
    for i in 3..=18 {
        make(&s, &format!("m{i:02}"), false);
    }
    make(&s, "c", true);
    assert_eq!(s.eval::<i64>(r#"return EditMacro(30, "x")"#).unwrap(), 20);
    // Both full: no empty entry, and -1 plus one.
    for i in 2..=18 {
        make(&s, &format!("c{i:02}"), true);
    }
    assert_eq!(s.eval::<i64>(r#"return EditMacro(40, "x")"#).unwrap(), 0);
}

/// The worker always stores argument 5 as the local flag (`0x4f12dc`), `false` when absent.
#[test]
fn edit_macro_replaces_the_local_flag_with_its_fifth_argument() {
    let s = script();
    s.run(r#"CreateMacro("m", 1, "", 1)"#).unwrap();
    let local = |s: &UiScript| {
        s.eval::<Option<i64>>("local _, _, _, l = GetMacroInfo(1) return l")
            .unwrap()
    };
    assert_eq!(local(&s), Some(1));
    s.run(r#"EditMacro(1, nil, nil, "x")"#).unwrap();
    assert_eq!(local(&s), None);
    s.run(r#"EditMacro(1, nil, nil, nil, 1)"#).unwrap();
    assert_eq!(local(&s), Some(1));
}

/// `GetMacroIndexByName` takes a string or a number (`0x4f1b3c`), else its usage text (`0x84ccb4`);
/// an empty name matches nothing (`0x4f0ee8`), even a macro whose name was edited empty.
#[test]
fn get_macro_index_by_name_takes_its_argument_as_the_reference_does() {
    let s = script();
    make(&s, "12", false);
    make(&s, "x", false);
    assert_eq!(s.eval::<i64>("return GetMacroIndexByName(12)").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>(r#"return GetMacroIndexByName("X")"#).unwrap(),
        2
    );
    s.run(r#"EditMacro(2, "")"#).unwrap();
    assert_eq!(
        s.eval::<i64>(r#"return GetMacroIndexByName("")"#).unwrap(),
        0
    );
    for call in [
        "GetMacroIndexByName()",
        "GetMacroIndexByName(nil)",
        "GetMacroIndexByName({})",
    ] {
        let err = s.run(call).expect_err(call).to_string();
        assert!(
            err.contains("Usage: GetMacroIndexByName(name)"),
            "{call}: {err}"
        );
    }
}
