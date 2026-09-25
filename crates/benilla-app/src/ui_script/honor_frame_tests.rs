//! The stock Honor tab (`HonorFrame.xml`) and the inspect window's honor page, engine only: what
//! the honor bindings' answers become on screen.

use benilla_ui::script::{HonorState, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// A rank-12 Alliance character with thirteen distinct figures, so a swapped pair of rows fails;
/// the numbers are the protocol crate's `inspect_honor_stats_golden`.
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
        // Different on purpose: the badge and title draw `rank`, the Highest Rank row the other.
        rank: 12,
        highest_rank: 14,
        // 191/255, three quarters through the rank.
        rank_bar: 191,
    }
}

/// The character window, whose fifth tab this page is; [`super::test_ui::CHARACTER_UI`] carries
/// `HonorFrame.xml`, as `PaperDollFrame.lua:103` writes into it on every show.
fn load_page(s: &UiScript) {
    for file in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(s, file);
    }
}

/// An Alliance male level-60 player. Race and class carry both halves: the level line formats
/// `UnitRace` and `UnitClass` unguarded on every show (`PaperDollFrame.lua:100-104`).
fn alliance_player() -> UnitState {
    UnitState {
        exists: true,
        level: 60,
        sex: 2,
        faction_group: Some("Alliance".into()),
        // The title's team digit comes from the race through ChrRaces and FactionTemplate
        // (`0x5efe00`), not from the live `faction_group`; human is race 1, team 1.
        pvp_team: crate::ui_unit::race_pvp_team(1),
        race: Some("Human".into()),
        race_file: Some("Human".into()),
        class: Some("Warrior".into()),
        class_file: Some("WARRIOR".into()),
        pvp_rank: 12,
        ..UnitState::default()
    }
}

/// The Honor tab open over [`state`], every section painted, as world entry leaves it.
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
        // The highest rank's title (14), not the current one (12).
        ("HonorFrameLifeTimeRankValue", "Lieutenant Commander"),
    ] {
        assert_eq!(text(&mut s, frame), want, "{frame}");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The title is keyed by the internal rank and the badge by the visual one, four lower.
#[test]
fn the_title_is_the_internal_rank_and_the_badge_is_the_visual_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    assert_eq!(text(&mut s, "HonorFrameCurrentPVPTitle"), "Knight-Captain");
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

/// 1.12 has no `PVP_RANK_0_*` string, so the binding answers nil and the page writes `NONE`
/// (`HonorFrame.lua:44-54`).
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

/// vmangos puts a GM on faction template 35 (`Player.cpp:2661`), group mask 0, so
/// `UnitFactionGroup` answers nil; the rank title still resolves, as `0x5efe00` reads the unit's
/// race, not its faction. The bar keeps the sideless red, so the side must stay gone.
#[test]
fn a_gm_flagged_player_keeps_his_rank_title_and_loses_only_the_faction_group() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    s.set_honor(Some(HonorState {
        // Grand Marshal, internal rank 18.
        rank: 18,
        highest_rank: 18,
        ..state()
    }));
    s.set_unit(
        "player",
        Some(UnitState {
            // `.gm on`: no side.
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
    // No side takes the bar colour's `else` arm, red (`HonorFrame.lua:68-72`).
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return HonorFrameProgressBar:GetStatusBarColor()")
        .unwrap();
    assert!(
        r > g && r > b,
        "a sideless player keeps the else-arm's red bar, got ({r}, {g}, {b})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_bar_takes_the_fraction_and_the_faction_colour() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = shown_honor_page();
    // The client multiplies by the f32 nearest 1/255 (`0x3B808081`, at `0x51aace`) rather than
    // dividing by 255; the two differ in the eighth decimal.
    let want = 191.0 * f64::from(f32::from_bits(0x3B80_8081));
    let binding = s.eval::<f64>("return GetPVPRankProgress()").unwrap();
    assert!(
        (binding - want).abs() < 1e-12,
        "the binding answers 191 × the reference's own constant = {want}, got {binding}"
    );
    // Compared at `f32`: a StatusBar's value is a `float` in the 1.12 client too.
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

/// The weekly sections repaint on world entry only (`HonorFrame.lua:7-33`), so this drives the real
/// `OnEvent`, where the flag comes from the event name.
#[test]
fn a_kill_repaints_the_session_but_not_the_week() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_honor_page();
    // A weekly figure moves too, and the kill must not repaint it.
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

    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    assert_eq!(text(&mut s, "HonorFrameThisWeekHKValue"), "124");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The key we build, `PVP_RANK_<internal>_<team>`, against the install's own `GlobalStrings.lua`,
/// for both teams.
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
// The same rows fed from an inspect reply, which has to be asked for.

use benilla_ui::script::InspectHonorData;
use std::collections::HashMap;

/// A live, inspectable unit at squared distance `d2`.
fn reach(d2: f64) -> benilla_ui::script::UnitReach {
    benilla_ui::script::UnitReach {
        dist_sq: d2,
        inspectable: true,
    }
}

/// The inspected player's reply, with [`state`]'s figures.
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
        // `GetInspectHonorData`'s twelfth return, `lifetimeRank` (`InspectHonorFrame.lua:21`).
        highest_rank: s.highest_rank,
        rank_bar: s.rank_bar,
    }
}

/// The inspect window on its Honor tab, over a rank-12 Alliance target, [`inspect_reply`] held.
fn shown_inspect_honor_page() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        // `TEXT`, the stock level line's formatter.
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The inspect window's tabs inherit its tab template.
        r"Interface\FrameXML\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        // Before the inspect addon, as `inherits=` resolves at load: its slot buttons inherit
        // `ItemButtonTemplate`, its honor rows `HonorFrame.xml`'s row templates.
        "Interface\\FrameXML\\ItemButtonTemplate.xml",
        "Interface\\FrameXML\\HonorFrame.xml",
    ] {
        load_xml(&s, file);
    }
    // A LoadOnDemand addon, seated off the chain and loaded by stock `InspectFrame_LoadUI`
    // (`UIParent.lua:170`), as the app does.
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_InspectUI");
    s.run("InspectFrame_LoadUI()").unwrap();
    s.set_unit("target", Some(alliance_player()));
    // The player is needed too: the page titles the target through `GetPVPRankInfo`
    // (`InspectHonorFrame.lua:50`), which keys off the local player's side and sex.
    s.set_unit("player", Some(alliance_player()));
    // 4 yards (d² = 16), inside `CanInspect`'s d² of 100; the window refuses to open otherwise.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(16.0))]));
    s.set_inspect_honor(Some(inspect_reply()));
    s.run(r#"InspectUnit("target")"#).unwrap();
    s.run(r#"ToggleInspect("InspectHonorFrame")"#).unwrap();
    s.resolve();
    s
}

/// The rank block reads the inspected unit, not the player.
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
    // The target's current rank, 12, and the bar from the reply's own byte.
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

/// Stock `InspectHonorFrame_OnShow` asks only with no reply held (`InspectHonorFrame.lua:11-17`).
#[test]
fn the_page_asks_only_when_it_holds_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_inspect_honor_page();
    assert_eq!(
        s.take_inspect_honor_requests(),
        0,
        "a page holding a reply must not re-ask"
    );

    // The app drops the reply when the inspected player changes; the next show asks.
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
