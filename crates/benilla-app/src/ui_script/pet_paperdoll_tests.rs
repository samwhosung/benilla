//! The stock pet paper doll (`PetPaperDollFrame.xml`) in the stock character window, engine only.
//! Its rows are the character page's code under the `"pet"` token; these test what comes and goes
//! with the pet.

use benilla_ui::script::{
    PetStats, QuadContent, ScriptValue, UiScript, UnitCombatStats, UnitState,
};

/// The whole character block: the page calls seven `PaperDollFrame_Set*` setters
/// (`PetPaperDollFrame.lua:75-81`) and moves a tab `CharacterFrame.xml` declares. Callers open with
/// `wow_data_or_skip!()` themselves, as the macro returns from the function it is written in.
fn load_pet_page() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit("player", Some(player_unit()));
    s
}

fn player_unit() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Benilla".into()),
        health: 100,
        max_health: 100,
        level: 60,
        race: Some("Night Elf".into()),
        race_file: Some("NightElf".into()),
        class: Some("Hunter".into()),
        class_file: Some("HUNTER".into()),
        sex: 2,
        is_player: true,
        player_controlled: true,
        ..UnitState::default()
    }
}

fn pet_unit() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Snarl".into()),
        health: 900,
        max_health: 1000,
        level: 58,
        ..UnitState::default()
    }
}

/// A hunter pet: `hunter_pet` is `HasPetUI`'s second return, which gates the training points and
/// the diet icon. The family and diet are CreatureFamily.dbc's Boar (id 5, food mask 63).
fn hunter_pet_stats() -> PetStats {
    PetStats {
        icon: None,
        hunter_pet: true,
        happiness: Some(3),
        damage_percentage: 125.0,
        loyalty_rate: 20.0,
        loyalty: Some("(Loyalty Level 6) Best Friend".into()),
        training_points: (170, 130),
        experience: (4200, 8000),
        family: Some("Boar".into()),
        food_types: ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]
            .map(String::from)
            .to_vec(),
    }
}

/// A pet's descriptor numbers: a creature has no buff split, so every pos/neg stays zero.
fn pet_combat_stats() -> UnitCombatStats {
    UnitCombatStats {
        stats: [123, 88, 210, 20, 45],
        resistances: [2400, 0, 55, 0, 0, 0, 30],
        min_damage: 90.0,
        max_damage: 130.0,
        main_attack_time_ms: 2000,
        attack_power: 640,
        ..UnitCombatStats::default()
    }
}

/// Every string the UI actually draws this frame.
fn texts(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Text { text: Some(t), .. } => Some(t),
            _ => None,
        })
        .collect()
}

/// A tab's left edge; only with the window open, as an unlaid frame's `GetLeft()` is nil.
fn tab_left(s: &mut UiScript, tab: u32) -> f64 {
    s.resolve();
    s.eval::<f64>(&format!("return CharacterFrameTab{tab}:GetLeft()"))
        .unwrap_or_else(|e| panic!("tab {tab} has no resolved rect: {e}"))
}

/// Put a pet in the world and tell the page about it, the way the app's feeds do.
fn give_pet(s: &mut UiScript) {
    s.set_unit("pet", Some(pet_unit()));
    s.set_pet_stats(true, hunter_pet_stats());
    s.set_pet_combat_stats(Some(pet_combat_stats()));
    s.fire_event("PET_BAR_UPDATE", vec![]);
}

/// Dismiss or lose the pet: `UNIT_PET` names the owner, not the pet.
fn take_pet(s: &mut UiScript) {
    s.set_unit("pet", None);
    s.set_pet_stats(false, PetStats::default());
    s.set_pet_combat_stats(None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
}

/// The frame count fingerprints the stock `PetPaperDollFrame.xml`; it moves only with that file.
#[test]
fn shipped_pet_page_loads_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    let mut pet_frames = 0;
    for f in super::test_ui::CHARACTER_UI {
        let n = super::test_ui::load_ui_strict(&s, f);
        if *f == "Interface\\FrameXML\\PetPaperDollFrame.xml" {
            pet_frames = n;
        }
    }
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());
    assert_eq!(pet_frames, 34, "the reference's own PetPaperDollFrame.xml");
    assert!(!s
        .eval::<bool>("return PetPaperDollFrame:IsVisible()")
        .unwrap());
}

/// `PetTab_Update` (`PetPaperDollFrame.lua:189-198`) hides the Pet tab and re-anchors tab 3 onto
/// its spot; the exact position proves the re-anchor ran, where a bare hide would leave a gap.
#[test]
fn the_pet_tab_rises_and_falls_with_the_pet_and_the_next_tab_closes_the_gap() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    // Open, so the tabs have rects.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();

    assert!(
        !s.eval::<bool>("return CharacterFrameTab2:IsVisible()")
            .unwrap(),
        "no pet: the Pet tab is down"
    );
    let closed3 = tab_left(&mut s, 3);

    give_pet(&mut s);
    assert!(
        s.eval::<bool>("return CharacterFrameTab2:IsVisible()")
            .unwrap(),
        "a pet raises the Pet tab"
    );
    let open2 = tab_left(&mut s, 2);
    let open3 = tab_left(&mut s, 3);
    assert_eq!(
        open2, closed3,
        "with no pet, tab 3 stood exactly where the Pet tab stands with one"
    );
    assert!(
        open3 > open2,
        "…and a pet pushes tab 3 out past it ({open3} vs {open2})"
    );

    take_pet(&mut s);
    assert!(
        !s.eval::<bool>("return CharacterFrameTab2:IsVisible()")
            .unwrap(),
        "the pet leaving lowers the tab again"
    );
    assert_eq!(tab_left(&mut s, 3), closed3, "…and the row closes back up");
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Stock `ToggleCharacter`'s guard (`CharacterFrame.lua:4-6`): no open, no close, no page switch.
#[test]
fn asking_for_the_pet_page_without_a_pet_does_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = load_pet_page();
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(
        !s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap(),
        "no pet: the window stays shut"
    );

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return PaperDollFrame:IsVisible()").unwrap(),
        "the refusal must not switch pages or close the window"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// No player combat stats are pushed, so a number read through `"player"` would be zero.
#[test]
fn the_page_reads_the_pets_own_snapshot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    give_pet(&mut s);
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(s
        .eval::<bool>("return PetPaperDollFrame:IsVisible()")
        .unwrap());

    let drawn = texts(&mut s);
    for want in ["123", "88", "210", "Snarl", "(Loyalty Level 6) Best Friend"] {
        assert!(
            drawn.iter().any(|t| t.contains(want)),
            "expected {want:?} on the page; drew {drawn:?}"
        );
    }
    // Unspent training points, total minus spent (`PetPaperDollFrame.lua:85-86`).
    assert!(
        drawn.iter().any(|t| t == "40"),
        "unspent training points (170-130); drew {drawn:?}"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The pages share the window's name line: the pet's on show, the player's back on hide
/// (`PetPaperDollFrame.lua:50-60`).
#[test]
fn the_page_borrows_the_windows_name_line_and_gives_it_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    give_pet(&mut s);

    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(!s
        .eval::<bool>("return CharacterNameText:IsVisible()")
        .unwrap());
    assert!(s.eval::<bool>("return PetNameText:IsVisible()").unwrap());

    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return CharacterNameText:IsVisible()")
            .unwrap(),
        "the player's name line comes back"
    );
    assert!(!s.eval::<bool>("return PetNameText:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A pet lost under its open page closes the window before any repaint
/// (`PetPaperDollFrame.lua:32-37`).
#[test]
fn the_pet_leaving_closes_the_page_under_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    give_pet(&mut s);
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap());

    take_pet(&mut s);
    assert!(
        !s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap(),
        "the pet's page cannot outlive the pet"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A warlock's minion passes `HasPetUI` but not its second return, so the three hunter-only pieces
/// stay down (`PetPaperDollFrame.lua:83-93`).
#[test]
fn a_minion_gets_the_page_without_the_hunter_furniture() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    s.set_unit("pet", Some(pet_unit()));
    s.set_pet_stats(true, PetStats::default()); // has_ui, but hunter_pet == false
    s.set_pet_combat_stats(Some(pet_combat_stats()));
    s.fire_event("PET_BAR_UPDATE", vec![]);

    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return CharacterFrameTab2:IsVisible()")
            .unwrap(),
        "a minion still gets the tab"
    );
    assert!(!s
        .eval::<bool>("return PetPaperDollPetInfo:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return PetTrainingPointText:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return PetTrainingPointLabel:IsVisible()")
        .unwrap());
    assert!(
        texts(&mut s).iter().any(|t| t.contains("210")),
        "the shared rows are not hunter-gated"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The level line is written only when `UnitCreatureFamily("pet")` answers
/// (`PetPaperDollFrame.lua:68-70`), outside the hunter gate, so a minion gets it. The guard skips
/// the `SetText` without clearing the line, so each half loads its own page.
#[test]
fn the_level_line_names_the_family_and_is_untouched_without_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let with_family = |family: Option<String>| {
        let mut s = load_pet_page();
        s.set_unit("pet", Some(pet_unit()));
        s.set_pet_combat_stats(Some(pet_combat_stats()));
        s.set_pet_stats(
            true,
            PetStats {
                family,
                ..PetStats::default() // hunter_pet false: a warlock's minion
            },
        );
        s.fire_event("PET_BAR_UPDATE", vec![]);
        s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
        let drawn = texts(&mut s);
        assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
        drawn
    };

    let drawn = with_family(Some("Imp".into()));
    assert!(
        drawn.iter().any(|t| t == "Level 58 Imp"),
        "UNIT_LEVEL_TEMPLATE + the family word; drew {drawn:?}"
    );

    // No family, so the binding answers nil and the line keeps its XML placeholder,
    // `text="Level level race class"` (`PetPaperDollFrame.xml:70`). The 1.12 client draws that
    // too: a FontString's LoadXML falls back to the raw attribute when it is not a string key
    // (`0x771029`), as `Button::LoadXML` does at `0x778c31`.
    let drawn = with_family(None);
    assert!(
        drawn.iter().any(|t| t == "Level level race class"),
        "no family ⇒ the XML's own design-time placeholder is left standing; drew {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|t| t.starts_with("Level 58")),
        "…and nothing derived from the pet reaches the line; drew {drawn:?}"
    );
}

/// The diet icon's hover shows `format(PET_DIET_TEMPLATE, BuildListString(GetPetFoodTypes()))`
/// (`PetPaperDollFrame.xml:267-270`), driven through the pointer so the hit test is covered too.
#[test]
fn hovering_the_diet_icon_lists_what_the_pet_eats() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    give_pet(&mut s);
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return PetPaperDollPetInfo:IsVisible()")
            .unwrap(),
        "the diet icon is shown for a hunter pet"
    );

    // The icon must also be painted: a hover alone passes over a mouse-enabled hole.
    s.resolve();
    let quads = s.extract();
    let icon = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("UI-PetHappiness"))
        })
        .expect("the diet icon's UI-PetHappiness quad is in the render list");
    let rect = icon.rect.expect("…with a resolved rect");
    assert!(
        (rect.right - rect.left - 24.0).abs() < 0.5 && (rect.top - rect.bottom - 23.0).abs() < 0.5,
        "24x23 (stock `PetPaperDollFrame.xml:245-248`), got {}x{}",
        rect.right - rect.left,
        rect.top - rect.bottom
    );
    // The icon sits inside the pet's `<PlayerModel>` pane (`PetPaperDollFrame.xml:177`).
    let pane = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::ModelPane { name: Some(n), .. } if n == "PetModelFrame")
        })
        .expect("the pet model pane is in the render list");
    let pane_rect = pane.rect.expect("…with a resolved rect");
    assert!(
        pane_rect.left <= rect.left
            && pane_rect.right >= rect.right
            && pane_rect.bottom <= rect.bottom
            && pane_rect.top >= rect.top,
        "the icon sits wholly inside the pane, which is what makes the z-order matter"
    );
    // The pane's scene draws after the icon, as in the reference: the two share a (strata, level)
    // bucket, the icon is BACKGROUND art, and a model drains from the ARTWORK batch last
    // (`0x76d160` registers its callback for layer 2 only; `0x76fb00` drains quads, then text,
    // then callbacks). The scene is transparent wherever the pet is not, so the icon shows.
    assert!(
        pane.z > icon.z,
        "the pane's scene draws out of the ARTWORK batch, after a sibling's BACKGROUND art \
         (icon z={:#x}, pane z={:#x})",
        icon.z,
        pane.z
    );

    let centre = super::test_ui::centre_of(&mut s, "PetPaperDollPetInfo");
    // Through the mouse: the handler's `SetOwner(this, ...)` (`PetPaperDollFrame.xml:268`) needs
    // the engine to set `this`.
    super::test_ui::hover(&mut s, "PetPaperDollPetInfo");
    assert_eq!(
        s.hit_test_name(centre.0, centre.1).as_deref(),
        Some("PetPaperDollPetInfo"),
        "the diet icon owns its own point inside the model pane"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Diet: Meat, Fish, Cheese, Bread, Fungus, Fruit",
        "PET_DIET_TEMPLATE around a plain \", \" join — 1.12's BuildListString has no \"and\""
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Stock `BuildListString` (`UIParent.lua:1051-1057`) joins with ", " and answers nil for nothing.
#[test]
fn build_list_string_is_a_plain_comma_join_that_nils_on_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = load_pet_page();
    assert_eq!(
        s.eval::<String>("return BuildListString('Meat', 'Fish', 'Cheese')")
            .unwrap(),
        "Meat, Fish, Cheese"
    );
    assert_eq!(
        s.eval::<String>("return BuildListString('Meat')").unwrap(),
        "Meat",
        "one item is the item, with no separator anywhere"
    );
    assert!(
        s.eval::<bool>("return BuildListString() == nil").unwrap(),
        "zero arguments is nil, not an empty string"
    );
}
