//! The shipped **Honor tab** (`Interface\\FrameXML\\HonorFrame.xml`, the reference's own) driven
//! end-to-end, engine-only (no Bevy) — the per-window test module the skills/reputation files
//! establish (decision 1512).
//!
//! What it pins is the PAINT law, the half `benilla-ui`'s own `script::pvp::tests` structurally
//! cannot reach: that module drives the thirteen globals and asserts their tuples, and stops at the
//! seam. Everything below is about what those tuples become on screen — which figure lands in which
//! of the thirteen row slots, what the rank block does with a nil title, which badge file the
//! arithmetic names, and the weekly/session repaint split.
//!
//! Two of these are the arc's own traps, and they are why this file exists rather than trusting the
//! engine tests:
//!
//! - **The badge is the VISUAL rank and the title is the INTERNAL one.** They differ by four, both
//!   are numbers, and both come out of one call. A pane that indexed the badge by the internal rank
//!   would put a Sergeant's art on a Knight-Captain and no tuple assertion would notice.
//! - **The weekly sections repaint on world entry only.** A pane that repainted everything on every
//!   kill would look identical on screen for a whole session and differ only in what it costs —
//!   which is to say, it would never be caught by looking.

use benilla_ui::script::{HonorState, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// The fixture snapshot — a rank-12 Alliance character with a figure in every bucket, all thirteen
/// distinct so a swapped pair of rows cannot pass. Deliberately the same numbers as the protocol
/// crate's 50-byte `inspect_honor_stats_golden`, so a failure here can be read straight against the
/// bytes one layer down.
fn state() -> HonorState {
    HonorState {
        session_hk: 17,
        session_dk: 2,
        yesterday_hk: 41,
        yesterday_dk: 0,
        yesterday_honor: 640,
        this_week_hk: 123,
        this_week_honor: 1_250,
        last_week_hk: 420,
        last_week_dk: 0,
        last_week_honor: 8_431,
        last_week_standing: 57,
        lifetime_hk: 3_907,
        lifetime_dk: 12,
        // Two DIFFERENT ranks on purpose: the current one the badge and title draw, and the
        // higher lifetime best the "Highest Rank" row shows. A pane that read one for the other
        // passes every same-value fixture.
        rank: 12,
        highest_rank: 14,
        // Three quarters through the rank (the binding's constant × 191).
        rank_bar: 191,
    }
}

/// The eight rank titles this fixture can ask for, defined as the install's `GlobalStrings.lua`
/// defines them — verbatim (`GlobalStrings.lua:2795`, `:3075`, `:3150-3161`, `:3297`).
///
/// **The inspect page's, since 1751.** The character page loads the player's own file as its first
/// [`super::test_ui::CHARACTER_UI`] entry, because stock `PaperDollFrame_OnLoad` sets seven labels
/// from it; [`shown_inspect_honor_page`] loads no such thing, so these are what its titles read.
/// [`the_real_global_strings_name_the_rank`] is the same assertion against the player's actual
/// file, on both sides.
const RANK_GLOBALS: &str = r#"
    NONE = "None"
    RANK = "Rank"
    PLAYER_LEVEL = "Level %d %s %s"
    PVP_RANK_12_1 = "Knight-Captain"
    PVP_RANK_12_0 = "Legionnaire"
    PVP_RANK_14_1 = "Lieutenant Commander"
    PVP_RANK_14_0 = "Champion"
"#;

/// The character window, whose fifth tab this page is. Every file it needs — including
/// `Interface\\FrameXML\\HonorFrame.xml` itself, which the shared list carries because
/// `PaperDollFrame_SetLevel` writes `HonorLevelText` on every show
/// (`PaperDollFrame.lua:103`, and `_SetGuild` writes `HonorGuildText` at `:123`) — is
/// [`super::test_ui::CHARACTER_UI`]; this page adds nothing of its own. It was a hand-copied list until the window became the reference's (decision 1751), which
/// is when the list stopped being short enough for a copy to stay honest.
fn load_page(s: &UiScript) {
    for file in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(s, file);
    }
}

/// An Alliance, male, level-60 player — the two facts the rank title's GlobalString key is built
/// from (side → the team digit, sex → the `_FEMALE` twin), plus the level line's input.
///
/// **Both halves of the race and class pairs, since 1751.** `UnitRace`/`UnitClass` answer
/// `(localized, file)` or `nil, nil` — the binding `zip`s the two, so a snapshot carrying only the
/// localized name reports the unit as raceless. Stock `PaperDollFrame_SetLevel` writes
/// `HonorLevelText` off `format(TEXT(PLAYER_LEVEL), UnitLevel, UnitRace, UnitClass)` — unguarded,
/// on every show of this page (`PaperDollFrame.lua:100-104`) — so a half-filled snapshot raises
/// there. Ours never called it from the honor tab, which is why the gap sat unseen.
fn alliance_player() -> UnitState {
    UnitState {
        exists: true,
        level: 60,
        sex: 2,
        faction_group: Some("Alliance".into()),
        // The rank title's team digit is `pvp_team` (`0x5efe00`: race → ChrRaces →
        // FactionTemplate), NOT `faction_group` (`UnitFactionGroup`: the live template). Human is
        // race 1 → Alliance → 1. Both are seated because a real Alliance player has both; the GM
        // test below is what drives them apart.
        pvp_team: crate::ui_unit::race_pvp_team(1),
        race: Some("Human".into()),
        race_file: Some("Human".into()),
        class: Some("Warrior".into()),
        class_file: Some("WARRIOR".into()),
        pvp_rank: 12,
        ..UnitState::default()
    }
}

/// The Honor page open on its tab with [`state`] pushed, as world entry leaves it (`weekly` true,
/// so every section has been painted at least once).
fn shown_honor_page() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_page(&s);
    s.set_unit("player", Some(alliance_player()));
    s.set_honor(Some(state()));
    s.run(r#"ToggleCharacter("HonorFrame")"#).unwrap();
    s.run("HonorFrame_Update(1)").unwrap();
    s.resolve();
    s
}

fn text(s: &mut UiScript, frame: &str) -> String {
    s.eval::<String>(&format!("return {frame}:GetText()"))
        .unwrap_or_else(|e| panic!("{frame}:GetText(): {e}"))
}

/// Every figure the snapshot carries lands in its own row. Thirteen distinct numbers against
/// thirteen slots: any transposition — the two lifetime totals, this week against last week, the
/// standing against a kill count — fails here and only here.
#[test]
fn every_figure_lands_in_its_own_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    for (frame, want) in [
        ("HonorFrameCurrentHKValue", "17"),
        ("HonorFrameCurrentDKValue", "2"),
        ("HonorFrameYesterdayHKValue", "41"),
        ("HonorFrameYesterdayContributionValue", "640"),
        ("HonorFrameThisWeekHKValue", "123"),
        ("HonorFrameThisWeekContributionValue", "1250"),
        ("HonorFrameLastWeekHKValue", "420"),
        ("HonorFrameLastWeekContributionValue", "8431"),
        ("HonorFrameLastWeekStandingValue", "57"),
        ("HonorFrameLifeTimeHKValue", "3907"),
        ("HonorFrameLifeTimeDKValue", "12"),
        // The HIGHEST rank's title, not the current one — rank 14, not rank 12.
        ("HonorFrameLifeTimeRankValue", "Lieutenant Commander"),
    ] {
        assert_eq!(text(&mut s, frame), want, "{frame}");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The rank block: the title is keyed by the INTERNAL rank and the badge by the VISUAL one, which
/// differ by four. This is the arc's central conflation trap (decision 1512) and the assertion that
/// makes it impossible to ship.
#[test]
fn the_title_is_the_internal_rank_and_the_badge_is_the_visual_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    assert_eq!(text(&mut s, "HonorFrameCurrentPVPTitle"), "Knight-Captain");
    // Internal 12 → visual 8. The line reads "(Rank 8)", never "(Rank 12)".
    assert_eq!(text(&mut s, "HonorFrameCurrentPVPRank"), "(Rank 8)");
    assert!(
        s.eval::<bool>("return HonorFramePvPIcon:IsShown()")
            .unwrap(),
        "a ranked character shows a badge"
    );
    assert_eq!(
        s.eval::<String>("return HonorFramePvPIcon:GetTexture()")
            .unwrap(),
        "Interface\\PvPRankBadges\\PvPRank08",
        "the badge file is the VISUAL rank, zero-padded"
    );
}

/// Rank 0 — every character who has never taken an honorable kill. The title's GlobalString does
/// not exist (there is no `PVP_RANK_0_*`), the binding answers nil, and the pane says NONE with no
/// badge. The nil is the mechanism, so this is the case that proves the pane never invents a title.
#[test]
fn an_unranked_character_reads_none_and_shows_no_badge() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    s.set_honor(Some(HonorState {
        rank: 0,
        highest_rank: 0,
        ..state()
    }));
    s.set_unit(
        "player",
        Some(UnitState {
            pvp_rank: 0,
            ..alliance_player()
        }),
    );
    s.run("HonorFrame_Update(1)").unwrap();
    s.resolve();
    assert_eq!(text(&mut s, "HonorFrameCurrentPVPTitle"), "None");
    assert_eq!(text(&mut s, "HonorFrameCurrentPVPRank"), "(Rank 0)");
    assert_eq!(text(&mut s, "HonorFrameLifeTimeRankValue"), "None");
    assert!(
        !s.eval::<bool>("return HonorFramePvPIcon:IsShown()")
            .unwrap(),
        "rank 0 has no badge to show"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Report B378 — a GM-flagged Grand Marshal's Highest Rank read `None`.** The regression test
/// for decision 2227, and the one that holds the two sides apart.
///
/// vmangos forces a GM to faction template 35, whose group mask is 0, so `UnitFactionGroup`
/// genuinely answers nil — that half is faithful and the PvP icon is *supposed* to vanish with it.
/// The rank title is not on that wire at all: `0x5efe00` reads the unit's RACE through
/// `ChrRaces`/`FactionTemplate` (`[obj+0x110]+0x78`), and a GM's race does not change, which is why
/// the reporter's own 1.12 client on the same server read "Grand Marshal" from the same byte. We
/// had the title keyed off the faction group, so both rows went to NONE together.
///
/// The control is the pair: the two rank rows must name the rank **and** the bar must still take
/// the sideless arm's Horde red — a fix that "restored the side" would pass the first half and
/// silently break the second. Skips without client data.
#[test]
fn a_gm_flagged_player_keeps_his_rank_title_and_loses_only_the_faction_group() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    s.set_honor(Some(HonorState {
        // Grand Marshal — internal 18, the top of the Alliance list.
        rank: 18,
        highest_rank: 18,
        ..state()
    }));
    s.set_unit(
        "player",
        Some(UnitState {
            // `.gm on`: template 35, group mask 0 → no side.
            faction_group: None,
            faction_group_localized: None,
            pvp_rank: 18,
            ..alliance_player()
        }),
    );
    s.run("HonorFrame_Update(1)").unwrap();
    s.resolve();
    assert_eq!(
        text(&mut s, "HonorFrameLifeTimeRankValue"),
        "Grand Marshal",
        "the Highest Rank row — the reported symptom"
    );
    assert_eq!(
        text(&mut s, "HonorFrameCurrentPVPTitle"),
        "Grand Marshal",
        "and the title under the badge, which reads the same key"
    );
    assert!(
        s.eval::<bool>(r#"return UnitFactionGroup("player") == nil"#)
            .unwrap(),
        "the side itself is still gone — that half of GM mode is the reference's"
    );
    // The control: the bar's colour is `UnitFactionGroup`'s, so a sideless player takes the
    // `else` arm (Horde red, `HonorFrame.lua:68`) and must KEEP taking it.
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return HonorFrameProgressBar:GetStatusBarColor()")
        .unwrap();
    assert!(
        r > g && r > b,
        "a sideless player keeps the else-arm's red bar, got ({r}, {g}, {b})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The bar takes the 0..1 fraction straight, and wears the player's own faction colour. Alliance
/// navy here; the `else` arm is Horde red for everyone else, including a unit whose side has not
/// resolved — a bar with no colour at all would read as a broken pane.
#[test]
fn the_bar_takes_the_fraction_and_the_faction_colour() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = shown_honor_page();
    // Against the binding's own arithmetic, not against `191/255`. The client MULTIPLIES by the
    // f32 nearest 1/255 (`0x3B808081`, `0x51aace`) rather than
    // dividing, and the two answers differ in the eighth decimal — a tolerance loose enough to
    // accept both would stop pinning the correction the moment it was made.
    let want = 191.0 * f64::from(f32::from_bits(0x3B80_8081));
    let binding = s.eval::<f64>("return GetPVPRankProgress()").unwrap();
    assert!(
        (binding - want).abs() < 1e-12,
        "the binding answers 191 × the reference's own constant = {want}, got {binding}"
    );
    // The BAR is checked at `f32`, and that is not a slackened assertion — a `StatusBar`'s value is
    // a C `float` in the real client and an `f32` in ours, so the round trip through the widget is
    // *supposed* to lose the tail. Asserting the exact `f64` here would be asserting that our
    // status bars are wider than the reference's.
    let value = s
        .eval::<f64>("return HonorFrameProgressBar:GetValue()")
        .unwrap();
    assert_eq!(
        value as f32, want as f32,
        "the bar holds the binding's answer at widget precision"
    );
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return HonorFrameProgressBar:GetStatusBarColor()")
        .unwrap();
    assert!(
        (r - 0.05).abs() < 1e-6 && (g - 0.15).abs() < 1e-6 && (b - 0.36).abs() < 1e-6,
        "Alliance navy, got ({r}, {g}, {b})"
    );
}

/// The repaint split the reference wrote and we kept: a kill moves the session and rank blocks, and
/// leaves the three weekly sections alone until world entry says otherwise.
///
/// Driven through the real `OnEvent` rather than by calling `HonorFrame_Update` directly, because
/// the flag's *derivation from the event name* is the half that can be got wrong.
#[test]
fn a_kill_repaints_the_session_but_not_the_week() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    // Everything moves in the snapshot — including a weekly figure, which is the bait.
    s.set_honor(Some(HonorState {
        session_hk: 18,
        this_week_hk: 124,
        ..state()
    }));
    s.fire_event("PLAYER_PVP_KILLS_CHANGED", vec![]);
    s.resolve();
    assert_eq!(
        text(&mut s, "HonorFrameCurrentHKValue"),
        "18",
        "the session block follows a kill"
    );
    assert_eq!(
        text(&mut s, "HonorFrameThisWeekHKValue"),
        "123",
        "the weekly block does NOT — it moves at the server's weekly maintenance"
    );

    // World entry is the flag that says "repaint those too".
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    assert_eq!(text(&mut s, "HonorFrameThisWeekHKValue"), "124");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The rank title against the **real shipped strings**, end to end — the leg the hand-set
/// [`RANK_GLOBALS`] above cannot reach, and the one that proves the key we build
/// (`PVP_RANK_<internal>_<team>`) is the key the player's own file actually defines.
///
/// The two sides matter: rank 12 is "Knight-Captain" to the Alliance and "Legionnaire" to the
/// Horde, so a hardcoded team digit passes one and fails the other. Skips without client data.
#[test]
fn the_real_global_strings_name_the_rank() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    load_page(&s);
    s.set_honor(Some(state()));

    for (team, want) in [(1i8, "Knight-Captain"), (0, "Legionnaire")] {
        s.set_unit(
            "player",
            Some(UnitState {
                pvp_team: team,
                ..alliance_player()
            }),
        );
        s.run(r#"ToggleCharacter("HonorFrame")"#).ok();
        s.run("HonorFrame_Update(1)").unwrap();
        s.resolve();
        assert_eq!(
            text(&mut s, "HonorFrameCurrentPVPTitle"),
            want,
            "rank 12 to team {team}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The inspect window's honor page ─────────────────────────────────────────────────────────────
//
// Same twelve rows, same two painters, a different data source: the character page reads a
// descriptor block we hold, this one reads a reply that had to be asked for. The tests below are
// about that difference — the twelve-value tuple landing in the right slots, and the ask/hold latch.

use benilla_ui::script::InspectHonorData;
use std::collections::HashMap;

/// A live, inspectable unit at squared distance `d2` (see `inspect_tests`' twin).
fn reach(d2: f64) -> benilla_ui::script::UnitReach {
    benilla_ui::script::UnitReach {
        dist_sq: d2,
        inspectable: true,
    }
}

/// The reply for the inspected player, carrying the same figures as [`state`] so a row that took
/// its number from the wrong pane's feed is visible as a wrong *value*, not just a wrong layout.
fn inspect_reply() -> InspectHonorData {
    let s = state();
    InspectHonorData {
        guid: 0x0000_0001_0000_2AB3,
        session_hk: s.session_hk,
        session_dk: s.session_dk,
        yesterday_hk: s.yesterday_hk,
        yesterday_honor: s.yesterday_honor,
        this_week_hk: s.this_week_hk,
        this_week_honor: s.this_week_honor,
        last_week_hk: s.last_week_hk,
        last_week_honor: s.last_week_honor,
        last_week_standing: s.last_week_standing,
        lifetime_hk: s.lifetime_hk,
        lifetime_dk: s.lifetime_dk,
        // The reply's own `highestRank` — `GetInspectHonorData`'s twelfth return, which the
        // reference destructures as `lifetimeRank`.
        highest_rank: s.highest_rank,
        rank_bar: s.rank_bar,
    }
}

/// The inspect window open on its Honor tab, over a rank-12 Alliance target with [`inspect_reply`]
/// held.
fn shown_inspect_honor_page() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(RANK_GLOBALS).unwrap();
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        // `TEXT`, the stock level line's formatter.
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "ScrollTemplates.xml", // our scroll kit + the placeholder icon
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The inspect window's four tabs (`Blizzard_InspectUI`) inherit it (1993).
        r"Interface\FrameXML\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        // `InspectUnit`'s home since 1832.
        "Interface\\FrameXML\\GameTooltip.xml",
        // Before the inspect addon — its honor page inherits this file's row templates and calls
        // its two shared painters, and `inherits=` resolves at load (the manifest's own order).
        // The stock inspect slot buttons inherit this (1832).
        "Interface\\FrameXML\\ItemButtonTemplate.xml",
        "Interface\\FrameXML\\HonorFrame.xml",
    ] {
        load_xml(&s, file);
    }
    // The window is a LoadOnDemand addon, reached the way the app reaches it: seated off the
    // chain as a registry row (1957) and loaded by the reference's own `InspectFrame_LoadUI`
    // (UIParent.xml; 1967).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_InspectUI");
    s.run("InspectFrame_LoadUI()").unwrap();
    s.set_unit("target", Some(alliance_player()));
    // The PLAYER snapshot is required even though this page is about somebody else, and the reason
    // is the reference's own asymmetry: `BenillaHonorPane_SetRank` passes the inspected token for
    // the BAR COLOUR, but `GetPVPRankInfo` takes no unit at all and keys its title off the local
    // player's side and sex. That is faithful — the ref calls it the same way — and it is sound
    // because the server only lets you inspect a player you cannot attack, i.e. your own faction.
    // Without a player pushed, every title on this page reads NONE (found by this test).
    s.set_unit("player", Some(alliance_player()));
    // 4 yards away (d² = 16), inside the verified `CanInspect` 100.0 — the window refuses to open
    // otherwise, and a silently-refused open would read as a broken pane.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(16.0))]));
    s.set_inspect_honor(Some(inspect_reply()));
    s.run(r#"InspectUnit("target")"#).unwrap();
    s.run(r#"ToggleInspect("InspectHonorFrame")"#).unwrap();
    s.resolve();
    s
}

/// The twelve-value tuple lands in the twelve slots, and the rank block reads the INSPECTED unit —
/// not the player. Everything the character page's row test pins, on the other feed.
#[test]
fn the_inspect_page_paints_the_reply_it_holds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_inspect_honor_page();
    for (frame, want) in [
        ("InspectHonorFrameCurrentHKValue", "17"),
        ("InspectHonorFrameCurrentDKValue", "2"),
        ("InspectHonorFrameYesterdayHKValue", "41"),
        ("InspectHonorFrameYesterdayContributionValue", "640"),
        ("InspectHonorFrameThisWeekHKValue", "123"),
        ("InspectHonorFrameThisWeekContributionValue", "1250"),
        ("InspectHonorFrameLastWeekHKValue", "420"),
        ("InspectHonorFrameLastWeekContributionValue", "8431"),
        ("InspectHonorFrameLastWeekStandingValue", "57"),
        ("InspectHonorFrameLifeTimeHKValue", "3907"),
        ("InspectHonorFrameLifeTimeDKValue", "12"),
        ("InspectHonorFrameLifeTimeRankValue", "Lieutenant Commander"),
    ] {
        assert_eq!(text(&mut s, frame), want, "{frame}");
    }
    // The rank block: the target's CURRENT rank (12 → "Knight-Captain", badge 08), and the bar off
    // the reply's own byte rather than the player's.
    assert_eq!(
        text(&mut s, "InspectHonorFrameCurrentPVPTitle"),
        "Knight-Captain"
    );
    assert_eq!(text(&mut s, "InspectHonorFrameCurrentPVPRank"), "(Rank 8)");
    let bar = s
        .eval::<f64>("return InspectHonorFrameProgressBar:GetValue()")
        .unwrap();
    assert!(
        (bar - 191.0 / 255.0).abs() < 1e-6,
        "the bar reads the REPLY's rank byte, got {bar}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The latch the reference's `OnShow` gates on: with nothing held the page asks and paints nothing;
/// with a reply held it paints and asks for nothing.
///
/// This is the one behaviour on this page that is a *round trip* rather than a read, and getting it
/// backwards is invisible on screen in the common case — a window opened on a player whose data
/// happens to be held looks identical either way.
#[test]
fn the_page_asks_only_when_it_holds_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_inspect_honor_page();
    // Held: the open above consumed no request.
    assert_eq!(
        s.take_inspect_honor_requests(),
        0,
        "a page holding a reply must not re-ask"
    );

    // Cleared — the app drops the reply when the inspected player changes. Re-showing the page
    // asks, and paints nothing until an answer lands.
    s.set_inspect_honor(None);
    s.run(r#"InspectHonorFrame_OnShow()"#).unwrap();
    s.resolve();
    assert_eq!(
        s.take_inspect_honor_requests(),
        1,
        "a page holding nothing must ask exactly once"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
