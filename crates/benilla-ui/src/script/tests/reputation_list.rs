//! The reputation list's folds, selection and header values as `ReputationInfo.cpp` holds them:
//! the fold mask `0x84a0a4` has five writers (the rebuild `0x4d5479`, a header registration
//! `0x4d5c34`, collapse and expand `0x4d6452`, `0x4d646b`, `0x4d647e`), the selection cell
//! `0xb73768` holds a faction id (`0x4d6bb0`), and `GetFactionInfo` answers by the entry's faction id
//! (`0x4d64f0`).

use mlua::Value;

use crate::script::{FactionEntry, ReputationState, UiScript};

fn entry(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        faction_id,
        rep_list_id,
        parent_id,
        name: name.into(),
        description: format!("About the {name}."),
        standing: 4000,
        standing_id: 5,
        bar_min: 3000,
        bar_max: 9000,
        visible: true,
        is_header: false,
        at_war: false,
        can_toggle_at_war: true,
        inactive: false,
    }
}

/// A header row as the wire delivers one: flag `0x08` without `VISIBLE`.
fn header(faction_id: u32, rep_list_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        visible: false,
        is_header: true,
        ..entry(faction_id, rep_list_id, 0, name)
    }
}

/// Rows: 1 Alliance, 2 Ironforge, 3 Stormwind, 4 Steamwheedle Cartel, 5 Booty Bay, 6 Other,
/// 7 Argent Dawn, 8 Bloodsail Buccaneers.
fn state() -> ReputationState {
    let mut alliance = header(469, 11, "Alliance");
    alliance.standing = 9500;
    alliance.standing_id = 6;
    alliance.bar_min = 9000;
    alliance.bar_max = 21000;
    ReputationState {
        entries: vec![
            entry(72, 19, 469, "Stormwind"),
            entry(529, 13, 0, "Argent Dawn"),
            alliance,
            entry(21, 1, 169, "Booty Bay"),
            entry(47, 20, 469, "Ironforge"),
            header(169, 10, "Steamwheedle Cartel"),
            entry(87, 0, 0, "Bloodsail Buccaneers"),
        ],
        watched: None,
    }
}

fn seated() -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_reputation(state());
    s
}

fn names(s: &UiScript) -> Vec<String> {
    s.eval::<Vec<String>>(
        "local t = {} for i = 1, GetNumFactions() do t[i] = (GetFactionInfo(i)) end return t",
    )
    .expect("names")
}

/// `SMSG_SET_FACTION_STANDING` (`0x4d5760`) writes none of the mask's writers, so a standing
/// change keeps every fold, the Inactive header's included; a push that lists a header key not
/// registered before registers it, and the registration sets the mask to all ones (`0x4d5c34`).
#[test]
fn a_standing_push_keeps_the_folds_and_a_new_header_opens_every_one() {
    let mut s = seated();
    s.run("CollapseFactionHeader(1)").unwrap();
    assert_eq!(names(&s).len(), 6, "Alliance folded");
    let mut ticked = state();
    ticked.entries[0].standing = 12345;
    s.set_reputation(ticked.clone());
    assert_eq!(
        names(&s).len(),
        6,
        "a standing change keeps Alliance folded"
    );

    // A row moved to Inactive and its header opened, then a standing change: still open.
    ticked.entries[1].inactive = true; // Argent Dawn
    s.set_reputation(ticked.clone());
    let n = names(&s).len();
    s.run(&format!("ExpandFactionHeader({n})")).unwrap();
    assert_eq!(names(&s).last().unwrap(), "Argent Dawn", "Inactive opened");
    ticked.entries[0].standing = 12346;
    s.set_reputation(ticked.clone());
    assert_eq!(
        names(&s).last().unwrap(),
        "Argent Dawn",
        "Inactive stays open across a standing change"
    );

    // A new team key (Horde Forces, 892) registers a header: every header opens.
    s.run("CollapseFactionHeader(1)").unwrap();
    ticked
        .entries
        .push(entry(889, 21, 892, "Warsong Outriders"));
    ticked.entries.push(header(892, 22, "Horde Forces"));
    s.set_reputation(ticked);
    let all = names(&s);
    assert!(
        all.iter().any(|n| n == "Ironforge") && all.iter().any(|n| n == "Warsong Outriders"),
        "the registration opened Alliance again: {all:?}"
    );
    assert_eq!(all.last().unwrap(), "Argent Dawn", "and Inactive with it");
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// `SetSelectedFaction` stores the faction id of entry `index - 1` (`0x4d6bb0` → `0x4d5fa0`),
/// header or not, and `GetSelectedFaction` finds that id among the visible entries (`0x4d6c07` to
/// `0x4d6c30`). An index past the entries asks for faction 0 (`0x4d5fa0`), the Other header's.
#[test]
fn set_selected_faction_selects_a_header_by_its_faction_id() {
    let s = seated();
    s.run("SetSelectedFaction(4)").unwrap(); // Steamwheedle Cartel
    assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 4);
    // Folding Alliance moves the header up two rows; the selection follows it.
    s.run("CollapseFactionHeader(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 2);
    s.run("ExpandFactionHeader(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 4);
    // Index 0 and an index past the list store faction 0: the Other header, row 6.
    s.run("SetSelectedFaction(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 6);
    s.run("SetSelectedFaction(3) SetSelectedFaction(99)")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 6);
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// A header whose faction has a `Faction.dbc` row (`0x4d654f`) answers its own eleven values, is
/// header 1 and its fold (`0x4d6633` to `0x4d66b4`); a header with no row answers its text, an
/// empty description, standing id 1 and three zeros (`0x4d678d` to `0x4d67d1`); an index past
/// the list asks for faction 0 and answers as the Other header.
#[test]
fn get_faction_info_on_a_header_answers_its_own_values() {
    let s = seated();
    let got = s
        .eval::<(
            String,
            String,
            i64,
            i64,
            i64,
            i64,
            Value,
            Value,
            i64,
            Value,
            Value,
        )>("return GetFactionInfo(1)")
        .unwrap();
    assert_eq!(got.0, "Alliance");
    assert_eq!(got.1, "About the Alliance.");
    assert_eq!((got.2, got.3, got.4, got.5), (6, 9000, 21000, 9500));
    assert!(got.6.is_nil(), "not at war");
    assert_eq!(got.7, Value::Integer(1), "can toggle");
    assert_eq!(got.8, 1, "is header");
    assert!(got.9.is_nil() && got.10.is_nil(), "open, not watched");

    let (name, desc, standing_id, lo, hi, value, war, toggle, is_header) = s
        .eval::<(String, String, i64, i64, i64, i64, Value, Value, i64)>("return GetFactionInfo(6)")
        .unwrap();
    assert_eq!((name.as_str(), desc.as_str()), ("Other", ""));
    assert_eq!((standing_id, lo, hi, value), (1, 0, 0, 0));
    assert!(war.is_nil() && toggle.is_nil());
    assert_eq!(is_header, 1);

    assert_eq!(
        s.eval::<(String, i64)>("local n,_,_,_,_,_,_,_,h = GetFactionInfo(99) return n,h")
            .unwrap(),
        ("Other".into(), 1),
        "past the list: faction 0, the Other header"
    );
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// `SetFactionInactive` and `SetFactionActive` test `is-number` and raise their own usage lines
/// (`0x84a2dc` at `0x4d69c1`, `0x84a300` at `0x4d6a11`); a negative index is a number.
#[test]
fn the_inactive_verbs_raise_their_usage_lines() {
    let s = seated();
    for (verb, usage) in [
        ("SetFactionInactive", "Usage: SetFactionInactive(index)"),
        ("SetFactionActive", "Usage: SetFactionActive(index)"),
    ] {
        for arg in ["", "nil", "{}", "'x'"] {
            let call = format!("{verb}({arg})");
            let err = s.run(&call).expect_err(&call).to_string();
            assert!(err.contains(usage), "{call}: {err}");
        }
        s.run(&format!("{verb}(-1)"))
            .expect("a negative index is a number");
    }
}
