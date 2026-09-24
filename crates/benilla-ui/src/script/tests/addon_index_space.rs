//! The addon index space that `GetNumAddOns` and every verb's index form address: an array, not
//! the registry, rebuilt only by `SMSG_ADDON_INFO` (`AddOn_ReadAddonInfoReply 0x51da70`,
//! `[0x51dc30, 0x51dcdf)`). It is `## Title`-sorted with `SStrCmpI` (`qsort 0x73f727`, comparator
//! `0x51deb0`), filtered by `[rec+0x29]`, and empty before the reply.

use crate::script::{AddOnInfo, UiScript};

/// Folder order `Zulu`, `Alpha`, `Mike`; title order `Zulu`, `Mike`, `Alpha`.
fn seeded() -> UiScript {
    let mut s = UiScript::new().unwrap();
    let row = |name: &str, title: Option<&str>| AddOnInfo {
        name: name.into(),
        title: title.map(str::to_owned),
        interface: 11200,
        enabled: true,
        ..Default::default()
    };
    s.register_addons(
        vec![
            row("Zulu", Some("aardvark")),
            row("Alpha", Some("zebra")),
            row("Mike", Some("Middle")),
        ],
        None,
        None,
        None,
    );
    s
}

fn by_index(s: &UiScript) -> Vec<String> {
    s.eval::<Vec<String>>(
        "local t = {} for i = 1, GetNumAddOns() do t[i] = (GetAddOnInfo(i)) end return t",
    )
    .unwrap()
}

/// Comparator `0x51deb0` compares `AddOn_GetTitle` (`0x51df20`) with `SStrCmpI` (`0x64a4c0`,
/// `_strnicmp 0x414310`), folding ASCII only; a byte-wise sort would put `"Middle"` first.
#[test]
fn the_index_space_is_title_sorted_case_insensitively_and_not_registry_order() {
    let mut s = seeded();
    s.note_addon_info_reply(&[]);
    assert_eq!(by_index(&s), vec!["Zulu", "Mike", "Alpha"]);
}

/// The comparator's fallback: `AddOn_GetTitle` misses (`0x51e046`) and `0x51ded0`/`0x51ded6`
/// substitute the folder name.
#[test]
fn a_titleless_addon_sorts_under_its_folder_name() {
    let mut s = UiScript::new().unwrap();
    s.register_addons(
        vec![
            AddOnInfo {
                name: "Yankee".into(),
                title: Some("zzz".into()),
                interface: 11200,
                enabled: true,
                ..Default::default()
            },
            AddOnInfo {
                name: "bravo".into(),
                interface: 11200,
                enabled: true,
                ..Default::default()
            },
        ],
        None,
        None,
        None,
    );
    s.note_addon_info_reply(&[]);
    assert_eq!(by_index(&s), vec!["bravo", "Yankee"]);
}

/// The count `[0xbe1b90]` is zeroed by the registry reset (`0x51fad1`) and written only by the
/// reply's rebuild, so before it an index raises the ordinary range error against 0.
#[test]
fn there_is_no_index_space_until_the_server_answers() {
    let s = seeded();
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(0));
    let raised = s
        .eval::<String>(
            "local ok, e = pcall(function() GetAddOnInfo(1) end) \
             if ok then return \"<no raise>\" end return tostring(e)",
        )
        .unwrap();
    assert!(
        raised.contains("AddOn index must be in the range of 1 to 0"),
        "{raised}"
    );
    // The name form probes the registry hash (`0x51df20`), not the array.
    assert_eq!(
        s.eval::<String>("return (GetAddOnInfo('Alpha'))").ok(),
        Some("Alpha".into())
    );
}

/// `status = 2` sets `[rec+0x29]` (`0x51db84`) and the rebuild drops it (`0x51dc4f`/`0x51dc54`);
/// a stock install hides the twelve `Blizzard_*` addons this way, and they still load by name.
#[test]
fn a_hidden_addon_leaves_the_index_space_but_still_answers_by_name() {
    let mut s = seeded();
    s.note_addon_info_reply(&["mike".into()]); // names match case-insensitively
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(2));
    assert_eq!(by_index(&s), vec!["Zulu", "Alpha"]);
    assert_eq!(
        s.eval::<String>("return (GetAddOnInfo('Mike'))").ok(),
        Some("Mike".into())
    );
}

#[test]
fn a_reply_that_hides_nothing_still_builds_the_array() {
    let mut s = seeded();
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(0));
    s.note_addon_info_reply(&[]);
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(3));
}
