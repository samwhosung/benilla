//! The stock stable window (`PetStable.xml`) repaints while closed on every `UNIT_PET`
//! (`PetStable.lua:20`) and concatenates `UnitName("pet")` (`PetStable.lua:129`); until the pet
//! name query answers, `UnitName` reads `UNKNOWNOBJECT`, as in the reference.

use benilla_ui::script::{PetStats, ScriptValue, UiScript, UnitState};

use super::test_ui::load_ui_strict;

/// The window and what its paint calls. Callers open with `wow_data_or_skip!()` themselves: every
/// entry comes off the chain, and the macro returns from the function it is written in.
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

/// The pet as `ui_pet::unit` pushes it on the summon, nameless until the name query answers.
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

/// Icon and family come from `CreatureFamily.dbc` on the same tick, so `GetPetIcon()`, the paint's
/// is-there-a-pet test (`PetStable.lua:161`), answers before the name does.
fn hunter_pet_stats() -> PetStats {
    PetStats {
        icon: Some(r"Interface\Icons\Ability_Hunter_Pet_Boar".into()),
        hunter_pet: true,
        loyalty: Some("(Loyalty Level 6) Best Friend".into()),
        family: Some("Boar".into()),
        ..PetStats::default()
    }
}

/// An emptied line reads back nil: `FontString:GetText` (`0x79d690`) returns nil for "".
fn level_text(s: &UiScript) -> Option<String> {
    s.eval::<Option<String>>("return PetStableLevelText:GetText()")
        .unwrap()
}

/// Call Pet with the stable closed: the paint runs before the name arrives, and a nil name would
/// raise "attempt to concatenate a nil value" at `PetStable.lua:129`.
#[test]
fn call_pet_repaints_the_closed_stable_with_unknownobject_while_the_name_is_in_flight() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_stable();

    // No stable visited: `GetStablePetInfo` misses and `GetSelectedStablePet()` is -1.
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

    // The name lands; the window re-reads it on its next repaint, not on `UNIT_NAME_UPDATE`.
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

/// With no pet and no stable row 0, the paint takes its empty arm (`PetStable.lua:151`).
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
