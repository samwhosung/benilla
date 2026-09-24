//! The shipped **pet paper doll** driven end-to-end, engine-only — and since
//! decision 1751's character swap, "shipped" is the reference's own
//! `Interface\FrameXML\PetPaperDollFrame.xml`, read off the player's patch chain behind the
//! reference's `CharacterFrame.xml` and `PaperDollFrame.xml`. Our
//! `assets/ui/PetPaperDollFrame.xml` is deleted; [`super::test_ui::CHARACTER_UI`] is the load
//! list, and every test here opens with `wow_data_or_skip!()` because a chain entry needs the
//! install.
//!
//! The page's rows are the character page's own code with a different unit token, and
//! `character_tests.rs` already pins those. What is genuinely new — and what these test — is the
//! **conditional tab**: it goes up and down with the pet, the tab beside it slides to close the
//! gap, the page refuses to open without a pet, and it closes itself when the pet leaves. That
//! machinery is the half a green build cannot see.

use benilla_ui::script::{
    PetStats, QuadContent, ScriptValue, UiScript, UnitCombatStats, UnitState,
};

/// The page's production load prefix. [`super::test_ui::CHARACTER_UI`] is the whole character
/// block in `benilla.toc`'s order — the pet page cannot stand on its own file, because
/// `PetPaperDollFrame_Update` calls seven `PaperDollFrame_Set*` setters with a `"pet"` unit
/// token (stock `PetPaperDollFrame.lua:75-81`) and `PetTab_Update` moves a tab that
/// `CharacterFrame.xml` declares.
///
/// **Callers must open with `benilla_formats::wow_data_or_skip!()` themselves** — the macro
/// `return`s from the function it is written in, so a helper cannot hold the guard for its caller.
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

/// A hunter pet's stat block — `hunter_pet` is `HasPetUI`'s SECOND return, the one that gates the
/// training-point line and the diet icon. The family pair is the shipped `CreatureFamily.dbc` row
/// for a Boar (id 5, food mask 63) resolved through `ItemPetFood.dbc`.
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

/// Numbers a pet's descriptor really carries: stats and resistances, no PLAYER-block buff split
/// (a creature has none), so every pos/neg stays zero.
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

/// A tab's resolved left edge. Only meaningful with the **window open** — a frame nothing lays out
/// has no rect at all, and `GetLeft()` answers nil rather than a stale number.
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

/// Take it away again — the dismiss/death path: `UNIT_PET` names the OWNER, not the pet.
fn take_pet(s: &mut UiScript) {
    s.set_unit("pet", None);
    s.set_pet_stats(false, PetStats::default());
    s.set_pet_combat_stats(None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
}

/// The loader itself, plus the page's own frame count.
///
/// **A count is a fingerprint of a file, not a target** (decision 1800's closing note): this one
/// is the reference's `PetPaperDollFrame.xml` — the page, the five stat rows and five resistance
/// frames, the XP bar, the model pane with its two rotate buttons, the diet icon and the close
/// button — and it moves only when the player's own file does. It is new with decision 1751's
/// character swap: this test asserted no count while the file it loaded was ours, because a count
/// of our own transcription fingerprints nothing but the transcription.
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
    // The page itself exists and starts down, like every other subframe.
    assert!(!s
        .eval::<bool>("return PetPaperDollFrame:IsVisible()")
        .unwrap());
}

/// **The conditional tab, both ways.** With no pet the Pet tab is down and the tab beside it slides
/// onto its spot; with a pet the Pet tab comes up *at that very spot* and its neighbour moves out
/// past it. Pinning "tab-3-when-closed sits exactly where tab 2 stands when open" is what proves
/// the re-anchor really ran — a tab merely hidden, with the row left as-authored, would leave a gap
/// and pass a weaker check. `PetTab_Update` is the whole mechanism, and it is four lines
/// (stock `PetPaperDollFrame.lua:189-198`).
///
/// **The name was stale, not the test.** It said "Skills closes the gap" from when Skills WAS tab
/// 3; the Reputation page moved it to 4 long before this file went to the chain, and the tab
/// `PetTab_Update` re-anchors is and always was `CharacterFrameTab3` — REPUTATION in the
/// reference's row (`CharacterFrame.xml:115`) and in our deleted copy alike. Renamed so it says
/// what it checks; the assertions are untouched.
#[test]
fn the_pet_tab_rises_and_falls_with_the_pet_and_the_next_tab_closes_the_gap() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_page();
    // The rects only exist while something lays the window out.
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

/// The reference's `ToggleCharacter` guard (stock `CharacterFrame.lua:4-6`): asking for the pet page
/// with no pet does **nothing at all** — it does not open the window on some other tab, and it does
/// not close an already-open one.
#[test]
fn asking_for_the_pet_page_without_a_pet_does_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = load_pet_page();
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(
        !s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap(),
        "no pet: the window stays shut"
    );

    // …and with the window already open on the character page, the refusal leaves it open.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.run(r#"ToggleCharacter("PetPaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return PaperDollFrame:IsVisible()").unwrap(),
        "the refusal must not switch pages or close the window"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The page paints the **pet's** numbers, not the player's — the whole point of un-gating the stat
/// bindings. No player COMBAT-STATS snapshot is pushed here (only the unit one the window needs to
/// open), so anything that leaked through the `"player"` path would read zeros and this would fail.
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
    // Unspent training points = total - spent (stock `PetPaperDollFrame.lua:85-86`), not either
    // number by itself.
    assert!(
        drawn.iter().any(|t| t == "40"),
        "unspent training points (170-130); drew {drawn:?}"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The window has ONE name line and the two pages take turns holding it (stock
/// `PetPaperDollFrame.lua:50-60`) — the pet's
/// name replaces the player's on show and hands it back on hide. A page that forgot the hand-back
/// would leave the character sheet nameless.
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

/// A pet dismissed or killed while its own page is open **closes the window** (stock
/// `PetPaperDollFrame.lua:32-37`). This
/// is the one arm that must run while the page is visible and the pet is already gone, so it is
/// also the arm that would blow up on a page that repainted before checking.
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

/// A warlock's minion: `HasPetUI` says yes, its second return says no. The page opens in full and
/// the three hunter-only pieces stay down (stock `PetPaperDollFrame.lua:83-93`; 1005's class gate,
/// seen from the page's side).
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
    // …and the stat rows still read the minion's own numbers.
    assert!(
        texts(&mut s).iter().any(|t| t.contains("210")),
        "the shared rows are not hunter-gated"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// **The level line, both ways**. The reference guards its whole `SetText` on
/// `UnitCreatureFamily("pet")` (stock `PetPaperDollFrame.lua:68-70`), so the family is not
/// decoration on an existing line — it is the *condition* for the line existing at all. With a
/// family the row reads "Level 58 Imp"; without one nothing about the pet reaches the line, which
/// is the state 1057 shipped in and the hole this closes.
///
/// The minion is deliberate: a warlock's Imp fails `HasPetUI`'s second return, so this also pins
/// that the family word rides OUTSIDE 1005's hunter gate. Gating it would blank this line for every
/// warlock in the game.
///
/// **The two halves are two separate page loads on purpose.** The guard skips the `SetText`;
/// it does not clear the FontString — so pushing a family and then taking it away leaves the old
/// word on screen (this test found that by asserting the opposite first). That is the reference's
/// own shape and it is unreachable in play: a family is a property of the pet's *template*, it
/// never goes away under a live pet, and the page closes with the pet anyway. The case that IS
/// reachable is "the page painted before the creature query answered", which is a page that never
/// had a family — the second half here.
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
                ..PetStats::default() // hunter_pet false — a warlock's minion
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

    // No family — the creature query has not answered, or the template has none. The guard skips
    // the whole `SetText`, so the FontString keeps whatever it already held: nothing derived from
    // the pet is written, which is why nil (never "") is the binding's contract.
    //
    // **Retired divergence (decision 1751's character swap).** This used to read "no family ⇒ no
    // level line at all", because our deleted `assets/ui/PetPaperDollFrame.xml` declared
    // `PetLevelText` with no `text=` and it started empty. The reference declares it
    // `text="Level level race class"` (stock `PetPaperDollFrame.xml:70`) — a design-time
    // placeholder, and not a GlobalStrings key. **A real 1.12 client draws that literal too**:
    // `CSimpleFontString::LoadXML` substitutes the raw attribute when `FrameScript_GetText` comes
    // back empty (`771029`-`771032 mov eax,esi`), exactly as `Button::LoadXML 0x778c31` does — so
    // this is fidelity, not the loader divergence this comment used to call it. Both spellings say
    // the same thing about the guard; this one says it about the file that ships.
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

/// **The diet tooltip**: the happiness-art icon under the rotate buttons is the
/// pet's DIET affordance, and its hover runs the reference's own
/// `format(PET_DIET_TEMPLATE, BuildListString(GetPetFoodTypes()))` (stock
/// `PetPaperDollFrame.xml:267-270`) — the vararg binding feeding the `UIParent.lua` joiner.
///
/// Driven through the pointer, not by re-typing the expression, so a guard left in the XML or a
/// frame that has stopped taking the mouse fails here.
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

    // …and it is actually PAINTED, not merely a mouse-enabled hole. The director's 2026-08-06
    // report was exactly that split — the tooltip answered while nothing was on screen — so the
    // hover assertion below is not sufficient on its own: pin the art too, with its size and its
    // reference TexCoords (the icon is the pet's DIET affordance, so its coords are static).
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
    // …and it draws ON TOP of the model pane it sits inside — the actual regression. The pane is
    // opaque and the icon's rect is wholly within it, so being in the render list is not the same
    // as being seen. The pane rides BACKGROUND for exactly this reason: at ARTWORK
    // the draw layer, which is bucket-wide and outranks the frame, buried the icon no
    // matter which frame was declared later.
    //
    // The pane is found by NAME rather than by "a texture quad with an empty path", which is what
    // this used to look for. Our deleted `assets/ui/PetPaperDollFrame.xml` declared `PetModelFrame`
    // as a plain `<Frame>` carrying one opaque BACKGROUND booth texture standing in for a widget
    // this engine did not draw; the reference declares it `<PlayerModel>` (stock
    // `PetPaperDollFrame.xml:177`), which is a real widget here now and extracts as
    // `QuadContent::ModelPane`. The old needle matches nothing against it — this asserts the
    // containment itself rather than folding it into a `find`, so a pane that stopped covering the
    // icon fails loudly instead of dropping out of the search.
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
    // **The pane's scene draws AFTER the icon, and that is the reference's own order.**
    // `PetPaperDollPetInfo` is a sibling frame of `PetModelFrame` (same parent, so the same
    // `(strata, level)` bucket) and its happiness art is a BACKGROUND texture on it; a model's
    // scene drains out of that bucket's ARTWORK batch, last (`0x76d160` registers the render
    // callback only for layer 2, and `0x76fb00` drains quads, then text, then callbacks). The icon
    // stays visible because a model pane is not an opaque quad — the bake is transparent everywhere
    // the pet is not, here as in the client. This assertion is the ordering, not the pixels.
    assert!(
        pane.z > icon.z,
        "the pane's scene draws out of the ARTWORK batch, after a sibling's BACKGROUND art \
         (icon z={:#x}, pane z={:#x})",
        icon.z,
        pane.z
    );

    let centre = super::test_ui::centre_of(&mut s, "PetPaperDollPetInfo");
    // Through the MOUSE, not `GetScript("OnEnter")(f)`: the reference's handler is inline XML that
    // opens `GameTooltip:SetOwner(this, "ANCHOR_RIGHT")` (stock `PetPaperDollFrame.xml:268`), and
    // only the engine sets `this` — calling the script by hand passes nil and raises. Driving the
    // pointer also puts the frame's `enableMouse` and the hit test under test, which is the half
    // that was never covered.
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

/// `BuildListString`'s own edges, on the shipped `UIParent.xml`. The reference's is
/// a bare comma join with **no** "and" and **no** localization string, and it answers **nil** for
/// zero arguments — the shape `format("%s", …)` would error on, which is exactly why the diet icon
/// is hunter-gated. Quietly returning `""` here would hide that coupling.
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
