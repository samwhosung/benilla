//! The reference's own stable window (`Interface\FrameXML\PetStable.xml`, off the player's chain
//! since 1751) driven through the one event path that reaches it **while it is closed**:
//! `PetStable_OnLoad` registers `UNIT_PET`, and `PetStable_OnEvent` repaints on it
//! unconditionally (stock `PetStable.lua:20-21`) — so every Call Pet a hunter casts runs the whole
//! of `PetStable_Update` against a pet the client has only just learned about.
//!
//! That paint concatenates `UnitName("pet")` (l.129, and again at l.163-164) the moment
//! `UnitExists("pet")` is true, and the pet's name is the one name the client never has at that
//! moment: it does not ride the descriptor, it is answered by `CMSG_PET_NAME_QUERY` a round-trip
//! later. The reference reads `UNKNOWNOBJECT` there; a verb answering nil raises
//! `attempt to concatenate a nil value` in a window nobody opened.

use benilla_ui::script::{PetStats, ScriptValue, UiScript, UnitState};

use super::test_ui::load_ui_strict;

/// The window's production load prefix, in `benilla.toc` order: `GlobalStrings.lua` for
/// `UNIT_LEVEL_TEMPLATE`/`EMPTY_STABLE_SLOT` (and `UNKNOWNOBJECT`, the string under test),
/// `BasicControls.xml` for `TEXT()`, `ItemButtonTemplate.xml` for `SetItemButtonTexture`,
/// `MoneyFrame` for the cost row, `UIParent.xml` for the slot manager, `BuildListString` and
/// `Model_OnLoad` (the pane is a `<PlayerModel>`), `UIPanelTemplates` for the purchase and close
/// buttons, `GameTooltip.xml` for the slot hovers.
///
/// **Callers must open with `benilla_formats::wow_data_or_skip!()` themselves** — every entry
/// here is a chain entry, and the macro `return`s from the function it is written in.
const STABLE_UI: &[&str] = &[
    r"Interface\FrameXML\GlobalStrings.lua",
    r"Interface\FrameXML\Fonts.xml",
    r"Interface\FrameXML\BasicControls.xml",
    r"Interface\FrameXML\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    r"Interface\FrameXML\GameTooltip.xml",
    r"Interface\FrameXML\PetStable.xml",
];

fn load_stable() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in STABLE_UI {
        load_ui_strict(&s, f);
    }
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Benilla".into()),
            health: 100,
            max_health: 100,
            level: 60,
            class: Some("Hunter".into()),
            class_file: Some("HUNTER".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s
}

/// The pet's snapshot exactly as `ui_pet::unit` pushes it on the tick the summon streams: the
/// descriptor's fields are in, the guid is in, and the name is whatever the name cache has —
/// `None` until `SMSG_PET_NAME_QUERY_RESPONSE` lands.
fn pet(name: Option<&str>) -> UnitState {
    UnitState {
        exists: true,
        has_object: true,
        name: name.map(String::from),
        health: 900,
        max_health: 1000,
        level: 58,
        guid: 0xF140_0000_0000_0001,
        ..UnitState::default()
    }
}

/// The stat block the pet feed resolves off the creature template the same tick — the icon and
/// the family word come off one `CreatureFamily.dbc` row, which is why `GetPetIcon()` answers
/// (the paint's "is there a current pet?" test, l.51/161) while the NAME is still in flight.
fn hunter_pet_stats() -> PetStats {
    PetStats {
        icon: Some(r"Interface\Icons\Ability_Hunter_Pet_Boar".into()),
        hunter_pet: true,
        loyalty: Some("(Loyalty Level 6) Best Friend".into()),
        family: Some("Boar".into()),
        ..PetStats::default()
    }
}

/// `Option`, not `String`: an emptied line reads back **nil**, because `FontString:GetText
/// 0x79d690` substitutes nil for an empty string (decision 2110).
fn level_text(s: &UiScript) -> Option<String> {
    s.eval::<Option<String>>("return PetStableLevelText:GetText()")
        .unwrap()
}

/// The director's report: a hunter with a pet out casts Call Pet, and the red script-error dialog
/// names `PetStable.lua:129` — with the stable window closed. `UNIT_PET` fires on the summon,
/// the paint runs, `UnitExists("pet")` is true, and the name is not known yet.
///
/// Failure looks like: `s.errors()` holds `PetStable.lua:129: attempt to concatenate a nil
/// value`, and the level line is whatever the XML shipped as placeholder text. The pass is the
/// reference's own line for that instant — `UNKNOWNOBJECT`, the level template, the family —
/// and no error.
#[test]
fn call_pet_repaints_the_closed_stable_with_unknownobject_while_the_name_is_in_flight() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_stable();

    // No stable master has been visited: nothing pushed through `set_stable`, so every
    // `GetStablePetInfo` answers its miss shape and `GetSelectedStablePet()` is -1.
    s.set_unit("pet", Some(pet(None)));
    s.set_pet_stats(true, hunter_pet_stats());
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    assert!(
        s.errors().is_empty(),
        "the closed stable window raised on Call Pet: {:?}",
        s.errors()
    );
    assert!(
        !s.eval::<bool>("return PetStableFrame:IsVisible()").unwrap(),
        "UNIT_PET must repaint the window without opening it"
    );
    assert_eq!(level_text(&s).as_deref(), Some("Unknown Level 58 Boar"));
    assert_eq!(
        s.eval::<String>("return PetStableCurrentPet.tooltip")
            .unwrap(),
        "Unknown",
        "the slot's hover title is the same read (l.163)"
    );
    assert_eq!(
        s.eval::<String>("return PetStableCurrentPet.tooltipSubtext")
            .unwrap(),
        "Level 58 Boar"
    );

    // The name lands; the next repaint reads it. (`UNIT_NAME_UPDATE` is not one of the window's
    // events — a stable-side repaint is what the reference re-reads the name on.)
    s.set_unit("pet", Some(pet(Some("Snarl"))));
    s.fire_event("PET_STABLE_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(level_text(&s).as_deref(), Some("Snarl Level 58 Boar"));
    assert_eq!(
        s.eval::<String>("return PetStableCurrentPet.tooltip")
            .unwrap(),
        "Snarl"
    );
}

/// The control the fix must not move: the dismiss edge. `UNIT_PET` with no pet token seated and
/// no stable row for slot 0 takes the "nothing" arm (l.151-156) — empty level line, no error.
#[test]
fn dismissing_the_pet_empties_the_closed_stable_without_raising() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_stable();
    s.set_unit("pet", Some(pet(None)));
    s.set_pet_stats(true, hunter_pet_stats());
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.set_unit("pet", None);
    s.set_pet_stats(false, PetStats::default());
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(level_text(&s), None, "an emptied line reads back nil");
    assert_eq!(
        s.eval::<String>("return PetStableCurrentPet.tooltip")
            .unwrap(),
        "Empty Stable Slot"
    );
}
