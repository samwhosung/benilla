//! **The Lua index space is not the registry** (decision 2175) — a different order, over a
//! different set, that does not exist until the server answers.
//!
//! `GetNumAddOns` and the *index* form of every AddOn verb address a flat array the client
//! rebuilds in exactly one place: the tail of `AddOn_ReadAddonInfoReply 0x51da70`
//! (`[0x51dc30, 0x51dcdf)`), driven by `SMSG_ADDON_INFO`. It is `## Title`-sorted with
//! `SStrCmpI`, filtered by `[rec+0x29]`, and empty before the reply — three properties benilla had
//! none of, because it indexed the registry list directly (the binary's own `qsort 0x73f727` and
//! comparator `0x51deb0`).

use crate::script::{AddOnInfo, UiScript};

/// Three addons whose FOLDER order and TITLE order are deliberately different permutations, so a
/// test cannot pass by accident on a registry that happens to be sorted.
///
/// Folder order (registration): `Zulu`, `Alpha`, `Mike`.
/// Title order: `Zulu` (`"aardvark"`), `Mike` (`"Middle"`), `Alpha` (`"zebra"`).
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

/// The names `GetAddOnInfo(i)` answers, in index order.
fn by_index(s: &UiScript) -> Vec<String> {
    s.eval::<Vec<String>>(
        "local t = {} for i = 1, GetNumAddOns() do t[i] = (GetAddOnInfo(i)) end return t",
    )
    .unwrap()
}

/// **The index space is `## Title`-sorted, case-insensitively — not folder order.**
///
/// Comparator `0x51deb0` resolves both sides through `AddOn_GetTitle 0x51df20` and compares with
/// `SStrCmpI` (`0x64a4c0` → `_strnicmp 0x414310`, which folds `'A'..'Z'` by `+0x20` on both
/// operands — ASCII only, which is what `to_ascii_lowercase` is).
///
/// `"Middle"` is the case control: a byte-wise sort would put every capital before every
/// lowercase and answer `Middle, aardvark, zebra`.
#[test]
fn the_index_space_is_title_sorted_case_insensitively_and_not_registry_order() {
    let mut s = seeded();
    s.note_addon_info_reply(&[]);
    assert_eq!(by_index(&s), vec!["Zulu", "Mike", "Alpha"]);
}

/// **A record with no `## Title` sorts under its FOLDER name**, which is the comparator's
/// fallback: `AddOn_GetTitle` misses (`0x51e046` returns 0) and `0x51ded0`/`0x51ded6` substitute
/// the name the array holds.
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
            // No title at all — sorts as "bravo", between nothing and "zzz".
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

/// **There is no index space until `SMSG_ADDON_INFO` arrives** — `[0xbe1b90]` is zeroed by the
/// registry reset `0x51fad1` and written nowhere but the reply's own rebuild.
///
/// The registry is fully populated here; only the array is absent. The raise is the same one an
/// out-of-range index takes, because it *is* the same unsigned bound against a count of zero.
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
    // The registry is there all along — the NAME form never went through the array, it probes the
    // master registry hash (`0x51df20`'s own lookup), so it answers for an addon the index space
    // cannot reach.
    assert_eq!(
        s.eval::<String>("return (GetAddOnInfo('Alpha'))").ok(),
        Some("Alpha".into())
    );
}

/// **A record the server hid is gone from the index space and still answers by name.**
///
/// `status = 2` sets `[rec+0x29] = 1` (`0x51db84`) and the rebuild's `0x51dc4f`/`0x51dc54` drops
/// it. On a stock install that is all twelve `Blizzard_*` addons, which is why the reference's own
/// AddOns list shows the player's addons and none of Blizzard's — and it is why the name form has
/// to keep working, since `LoadAddOn("Blizzard_TalentUI")` is how the talent window opens.
#[test]
fn a_hidden_addon_leaves_the_index_space_but_still_answers_by_name() {
    let mut s = seeded();
    s.note_addon_info_reply(&["mike".into()]); // case-insensitive, as every name compare here is
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(2));
    assert_eq!(by_index(&s), vec!["Zulu", "Alpha"]);
    assert_eq!(
        s.eval::<String>("return (GetAddOnInfo('Mike'))").ok(),
        Some("Mike".into())
    );
}

/// **An empty reply is not the same as no reply.** A server that hides nothing still brings the
/// array into existence; only silence leaves it at zero.
#[test]
fn a_reply_that_hides_nothing_still_builds_the_array() {
    let mut s = seeded();
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(0));
    s.note_addon_info_reply(&[]);
    assert_eq!(s.eval::<i64>("return GetNumAddOns()").ok(), Some(3));
}
