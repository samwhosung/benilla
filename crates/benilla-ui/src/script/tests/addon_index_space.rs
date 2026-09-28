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

/// A player's addon `Probe` and a hidden chain addon `Blizzard_AuctionUI`, LoadOnDemand, whose one
/// file sets `AuctionLoaded`; the reply hides the Blizzard one, as a vmangos realm does.
fn with_hidden_blizzard(blizzard_enabled: bool) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_addon_chain_reader(Box::new(|path| {
        (path == "Interface/AddOns/Blizzard_AuctionUI/Blizzard_AuctionUI.lua")
            .then(|| b"AuctionLoaded = 1".to_vec())
    }));
    s.register_addons(
        vec![
            AddOnInfo {
                name: "Blizzard_AuctionUI".into(),
                interface: 11200,
                load_on_demand: true,
                chain: true,
                files: vec!["Blizzard_AuctionUI.lua".into()],
                enabled: blizzard_enabled,
                ..Default::default()
            },
            AddOnInfo {
                name: "Probe".into(),
                interface: 11200,
                enabled: true,
                ..Default::default()
            },
        ],
        None,
        None,
        None,
    );
    s.note_addon_info_reply(&["blizzard_auctionui".into()]);
    s
}

/// `GetAddOnInfo(name)`'s `enabled` slot, 1 or nil.
fn enabled(s: &UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!(
        "local _, _, _, on = GetAddOnInfo('{name}') return on ~= nil"
    ))
    .unwrap()
}

/// `DisableAllAddOns` (`0x48e7f0`) loops `0x51df00` up to `0x51def0`, the array a hidden record
/// never enters (`0x51dc4f`), so an addon manager's disable-all leaves the auction house loadable.
#[test]
fn disable_all_leaves_a_hidden_addon_enabled_and_loadable() {
    let s = with_hidden_blizzard(true);
    s.run("DisableAllAddOns()").unwrap();
    assert!(!enabled(&s, "Probe"), "the visible addon is disabled");
    assert!(
        enabled(&s, "Blizzard_AuctionUI"),
        "the hidden one is untouched"
    );
    assert_eq!(
        s.eval::<(Option<i64>, Option<String>)>("return LoadAddOn('Blizzard_AuctionUI')")
            .unwrap(),
        (Some(1), None)
    );
    assert_eq!(s.eval::<i64>("return AuctionLoaded").ok(), Some(1));
}

/// `EnableAllAddOns` (`0x48e720`) walks the same array, so a hidden addon keeps its own row, and
/// `AddOn_CanLoad` check 3 (`0x51e81d`) still reads that row: no check of the gate reads
/// `[rec+0x29]`, so a hidden addon disabled by name stays `DISABLED`.
#[test]
fn enable_all_touches_the_visible_addons_only() {
    let s = with_hidden_blizzard(false);
    s.run("DisableAddOn('Probe') EnableAllAddOns()").unwrap();
    assert!(enabled(&s, "Probe"), "the visible addon is enabled again");
    assert!(
        !enabled(&s, "Blizzard_AuctionUI"),
        "the hidden one is untouched"
    );
    assert_eq!(
        s.eval::<(Option<i64>, Option<String>)>("return LoadAddOn('Blizzard_AuctionUI')")
            .unwrap(),
        (None, Some("DISABLED".into()))
    );
}

/// Before `SMSG_ADDON_INFO` the count `[0xbe1b90]` is 0, so both loops run no iteration.
#[test]
fn the_all_verbs_do_nothing_before_the_server_answers() {
    let s = seeded();
    s.run("DisableAllAddOns()").unwrap();
    assert!(enabled(&s, "Alpha") && enabled(&s, "Mike") && enabled(&s, "Zulu"));
}

/// What a disable-all leaves for the shutdown writer (`0x51ef20`): the setter's one new row per
/// visible addon, and none for the hidden one, whose row it never set.
#[test]
fn disable_all_writes_rows_for_the_visible_addons_only() {
    let s = with_hidden_blizzard(true);
    assert_eq!(
        s.take_addon_enable_rows(),
        None,
        "nothing set, nothing to write"
    );
    s.run("DisableAllAddOns()").unwrap();
    assert_eq!(
        s.take_addon_enable_rows(),
        Some(vec![("Probe".to_string(), false)])
    );
}
