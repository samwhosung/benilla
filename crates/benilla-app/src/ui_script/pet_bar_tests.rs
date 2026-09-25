//! The pet action bar, stock `PetActionBarFrame.xml`, from a pushed slot list to its quads.

use benilla_ui::script::{PetActionView, QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The pet bar's chain in manifest order, then the chat window, whose edit box `ShowPetActionBar`
/// raises over the sliding bar (PetActionBarFrame.lua:175).
pub(super) fn load_pet_bar(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
        "Interface\\FrameXML\\PetActionBarFrame.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(s, file);
    }
}

/// A token slot's `name` is a `GlobalStrings` key: the two the tests read, at their stock values.
pub(super) fn declare_token_strings(s: &UiScript) {
    s.run(
        "PET_ACTION_ATTACK = 'Attack' \
         PET_MODE_DEFENSIVE = 'Defensive'",
    )
    .unwrap();
}

/// Packed words as the server sends them, Attack (`ACT_COMMAND`) and Claw (`ACT_ENABLED`, spell
/// 3010): the drag moves words.
const ATTACK_WORD: u32 = 0x0700_0002;
const CLAW_WORD: u32 = 0xC100_0BC2;

/// A hunter's bar: Attack (a lit command) in slot 1, Claw (autocast on) in 4, Defensive (a lit
/// reaction) in 9, the rest empty.
pub(super) fn hunter_slots() -> Vec<PetActionView> {
    let mut slots = vec![PetActionView::default(); 10];
    slots[0] = PetActionView {
        name: Some("PET_ACTION_ATTACK".into()),
        texture: Some("PET_ATTACK_TEXTURE".into()),
        is_token: true,
        active: true,
        attack_active: true,
        packed: ATTACK_WORD,
        ..Default::default()
    };
    slots[3] = PetActionView {
        name: Some("Claw".into()),
        subtext: Some("Rank 3".into()),
        texture: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
        spell_id: Some(3010),
        autocast_allowed: true,
        autocast_enabled: true,
        packed: CLAW_WORD,
        ..Default::default()
    };
    slots[8] = PetActionView {
        name: Some("PET_MODE_DEFENSIVE".into()),
        texture: Some("PET_DEFENSIVE_TEXTURE".into()),
        is_token: true,
        active: true,
        packed: 0x0600_0001,
        ..Default::default()
    };
    slots
}

/// Count `path` quads in the pet bar's row, cut at centre y 50: the rings overhang their buttons
/// (54 px on 30), so only centres part the rows (main bar 21, pet bar 70-75).
fn textures(quads: &[benilla_ui::script::ExtractedQuad], path: &str) -> usize {
    quads
        .iter()
        .filter(|q| q.rect.is_none_or(|r| (r.top + r.bottom) / 2.0 >= 50.0))
        .filter(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
        .count()
}

fn texture_rect(
    quads: &[benilla_ui::script::ExtractedQuad],
    path: &str,
) -> Option<benilla_ui::layout::Rect> {
    quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
        .and_then(|q| q.rect)
}

#[test]
fn the_shipped_pet_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);

    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    assert_eq!(
        textures(&s.extract(), "Interface\\PetActionBar\\UI-PetBar"),
        0,
        "no pet, no shelf"
    );

    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.tick(0.05);
    s.resolve();
    let quads = s.extract();

    assert_eq!(
        textures(&quads, "Interface\\PetActionBar\\UI-PetBar"),
        2,
        "the two shelf strips"
    );

    // A token slot's texture names a global, resolved by `getglobal` (PetActionBarFrame.lua:102).
    assert_eq!(
        textures(&quads, "Interface\\Icons\\Ability_GhoulFrenzy"),
        1,
        "Attack's icon came from PET_ATTACK_TEXTURE"
    );
    assert_eq!(
        textures(&quads, "Interface\\Icons\\Ability_Defend"),
        1,
        "Defensive's icon came from PET_DEFENSIVE_TEXTURE"
    );
    assert_eq!(textures(&quads, "Interface\\Icons\\Ability_Druid_Rake"), 1);

    // An unnamed slot's button hides outright (PetActionBarFrame.lua:122-128).
    assert_eq!(
        textures(&quads, "Interface\\Buttons\\UI-Quickslot2"),
        3,
        "one filled ring per occupied slot"
    );
    assert_eq!(
        textures(&quads, "Interface\\Buttons\\UI-Quickslot"),
        0,
        "an unnamed pet slot hides; it does not draw the empty ring"
    );

    assert_eq!(
        textures(&quads, "Interface\\Buttons\\CheckButtonHilight"),
        2,
        "Attack + Defensive are lit; Claw is not"
    );

    // The shine is the `$parentAutoCast` model; a hidden pane is not extracted.
    assert_eq!(
        textures(&quads, "Interface\\Buttons\\UI-AutoCastableOverlay"),
        1,
        "only Claw can autocast"
    );
    let shines: Vec<_> = quads
        .iter()
        .filter(|q| q.rect.is_none_or(|r| (r.top + r.bottom) / 2.0 >= 50.0))
        .filter(|q| matches!(&q.content, QuadContent::ModelPane { model: Some(p), model_scale, .. }
            if p == "Interface\\Buttons\\UI-AutoCastButton.mdx" && (*model_scale - 1.2).abs() < 1e-6))
        .collect();
    assert_eq!(
        shines.len(),
        1,
        "the shine pane on Claw alone — enabled, not merely allowed"
    );

    // TOPLEFT at MainMenuBar's BOTTOMLEFT (0, 0) + (36, 97) (UIParent.lua:1589,1736): the 43-tall
    // frame spans y[54,97], and button 1 at (36, 2) in it is x[72,102] y[56,86].
    let attack =
        texture_rect(&quads, "Interface\\Icons\\Ability_GhoulFrenzy").expect("Attack icon");
    assert_eq!(
        (attack.left, attack.bottom, attack.right, attack.top),
        (72.0, 56.0, 102.0, 86.0)
    );

    // The pet goes: the bar slides out over `PETACTIONBAR_SLIDETIME` (0.09 s) and then hides.
    s.set_pet_actions(false, true, true, Vec::new());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    for _ in 0..3 {
        s.tick(0.05);
    }
    s.resolve();
    let gone = s.extract();
    assert_eq!(textures(&gone, "Interface\\PetActionBar\\UI-PetBar"), 0);
    assert!(
        !gone.iter().any(
            |q| matches!(&q.content, QuadContent::ModelPane { model: Some(p), .. }
            if p == "Interface\\Buttons\\UI-AutoCastButton.mdx")
        ),
        "the shine pane hides with its bar"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Left casts, right flips autocast, and a left click on an active Attack calls it off
/// (`IsPetAttackActive`, PetActionBarFrame.lua:257-265).
#[test]
fn clicks_route_through_the_attack_toggle_fork() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();

    // Button 1 (Attack) spans x[72,102] y[56,86], centre (87, 71); the 30 px buttons chain +8, so
    // button 4 (Claw) starts at 72 + 3 * 38 = 186, centre (201, 71).
    let click = |s: &mut UiScript, x: f32, y: f32, button: &str| {
        s.mouse_button(x, y, button, true);
        s.mouse_button(x, y, button, false);
    };

    click(&mut s, 87.0, 71.0, "LeftButton");
    assert_eq!(s.take_pet_stop_attacks(), 1);
    assert!(
        s.take_pet_actions().is_empty(),
        "the call-off replaces the press, it does not accompany it"
    );

    let mut slots = hunter_slots();
    slots[0].active = false;
    slots[0].attack_active = false;
    s.set_pet_actions(true, true, true, slots);
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();
    click(&mut s, 87.0, 71.0, "LeftButton");
    assert_eq!(s.take_pet_actions(), vec![1]);
    assert_eq!(s.take_pet_stop_attacks(), 0);

    // A right-click flips a spell's autocast; a token is not autocastable.
    click(&mut s, 201.0, 71.0, "RightButton");
    assert_eq!(s.take_pet_autocast_toggles(), vec![4]);
    click(&mut s, 87.0, 71.0, "RightButton");
    assert!(
        s.take_pet_autocast_toggles().is_empty(),
        "a command token cannot autocast"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Every click starts with `this:SetChecked(0)` (PetActionBarFrame.lua:253); the repaint restores
/// the ring from `isActive`, never by diffing views. The reference signals it from state writes
/// (`0x4bc940`/`0x4bc960`), not from a refused `TogglePetAutocast` (`0x4bcbf7`), so a right-click
/// on a token leaves the ring off until something else repaints.
#[test]
fn a_click_drops_the_ring_and_the_repaint_restores_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);

    let checked = |s: &UiScript| {
        s.eval::<bool>("return PetActionButton1:GetChecked()")
            .unwrap()
    };

    // Attack's centre. Either button: the CheckButton toggles, then `SetChecked(0)` lands it at 0.
    for button in ["RightButton", "LeftButton"] {
        s.set_pet_actions(true, true, true, hunter_slots());
        s.fire_event("PET_BAR_UPDATE", vec![]);
        s.resolve();
        assert!(checked(&s), "{button}: the Attack token starts lit");

        s.mouse_button(87.0, 71.0, button, true);
        s.mouse_button(87.0, 71.0, button, false);
        assert!(
            !checked(&s),
            "{button}: the ref's own SetChecked(0), reproduced"
        );

        s.fire_event("PET_BAR_UPDATE", vec![]);
        assert!(
            checked(&s),
            "{button}: an unchanged repaint must re-light, not diff to a no-op"
        );
    }
    let _ = s.take_pet_actions();
    let _ = s.take_pet_stop_attacks();
    let _ = s.take_pet_autocast_toggles();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An unusable pet's bar (`PetHasActionBar` true, `GetPetActionsUsable` false) still draws, every
/// icon desaturated (PetActionBarFrame.lua:129-134).
#[test]
fn a_disabled_bar_greys_rather_than_hides() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);

    s.set_pet_actions(true, false, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();
    let quads = s.extract();
    assert_eq!(
        textures(&quads, "Interface\\PetActionBar\\UI-PetBar"),
        2,
        "the bar is still on screen"
    );
    // `SetDesaturated` reports shader support, so `SetDesaturation` takes its shader arm
    // (UIParent.lua:1500-1510): a greyscale flag on the icon, not the 0.5 vertex-colour fallback.
    let grey = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                desaturated,
                ..
            } if p == "Interface\\Icons\\Ability_Druid_Rake" => Some(*desaturated),
            _ => None,
        })
        .expect("Claw's icon");
    assert!(grey, "a disabled bar greys every icon on it");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The pet bar with the bottom-left multibar up or not: shelf strips drawn, top of Attack's icon.
fn pet_bar_row(with_multibar: bool) -> (usize, f32) {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    if with_multibar {
        s.run("SHOW_MULTI_ACTIONBAR_1 = 1 MultiActionBar_Update()")
            .unwrap();
    }
    declare_token_strings(&s);

    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.tick(0.05);
    s.resolve();
    let quads = s.extract();
    let shelf = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p == "Interface\\PetActionBar\\UI-PetBar")
        })
        .count();
    let attack_top = texture_rect(&quads, "Interface\\Icons\\Ability_GhoulFrenzy")
        .expect("Attack's icon draws")
        .top;
    (shelf, attack_top)
}

/// With the bottom-left bar up, `UIParent_ManageFramePositions` raises the pet bar 43 px and hides
/// its shelf art, which would draw across that row (UIParent.lua:1589,1706-1708).
#[test]
fn the_pet_bar_rises_and_sheds_its_shelf_over_the_bottom_left_bar() {
    benilla_formats::wow_data_or_skip!();
    let (low_shelf, low_top) = pet_bar_row(false);
    let (high_shelf, high_top) = pet_bar_row(true);

    assert_eq!(
        low_shelf, 2,
        "on the main bar, the shelf IS the bar's border"
    );
    assert_eq!(high_shelf, 0, "raised, it would draw across the row below");

    let risen = low_top - high_top;
    assert!(
        (risen.abs() - 43.0).abs() < 0.5,
        "the bar must rise by the ref's 43 px (moved {risen})"
    );
}

/// Both drag ends call `PickupPetAction` (PetActionBarFrame.lua:269-283), a move by the binding's
/// carrying-cursor fork; meanwhile `PET_BAR_SHOWGRID` shows every slot, the empty ones included.
#[test]
fn dragging_a_pet_spell_between_slots_moves_it_through_the_shipped_handlers() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();

    // Slot 5 is empty, so its button hides until the grid comes up.
    assert!(!s.eval::<bool>("return PetActionButton5:IsShown()").unwrap());

    s.run("this = PetActionButton4; PetActionButton_OnDragStart()")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_pet_set_actions(),
        vec![vec![(3, CLAW_WORD & 0xFFFF_0000)]],
        "the pickup blanks the slot it came from and tells the server"
    );
    s.tick(0.01);
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return PetActionBarFrame.showgrid").unwrap(),
        1
    );
    assert!(
        s.eval::<bool>("return PetActionButton5:IsShown()").unwrap(),
        "the grid reveals the empty slots to drop onto"
    );

    s.run("this = PetActionButton5; PetActionButton_OnReceiveDrag()")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_pet_set_actions(),
        vec![vec![(4, CLAW_WORD)]],
        "and the drop writes the word verbatim into slot 5 (0-based 4)"
    );
    s.tick(0.01);
    assert_eq!(
        s.eval::<i64>("return PetActionBarFrame.showgrid").unwrap(),
        0,
        "the grid goes down with the payload"
    );
    assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
}

/// `LOCK_ACTIONBAR` stops both drag ends (PetActionBarFrame.lua:270,278) but not the shift-click
/// pick-up (PetActionBarFrame.lua:254-255).
#[test]
fn the_lock_stops_the_pet_bar_drag_but_not_its_shift_click() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();

    s.run(r#"LOCK_ACTIONBAR = "1""#).unwrap();
    s.run("this = PetActionButton4; PetActionButton_OnDragStart()")
        .unwrap();
    assert!(
        s.take_pet_set_actions().is_empty(),
        "a locked bar sends nothing — the slot was never picked up"
    );
    assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());

    s.set_modifiers(true, false, false);
    s.run("this = PetActionButton4; PetActionButton_OnClick(\"LeftButton\")")
        .unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_pet_set_actions(),
        vec![vec![(3, CLAW_WORD & 0xFFFF_0000)]],
        "shift-click picked it up despite the lock"
    );
    s.run("this = PetActionButton5; PetActionButton_OnReceiveDrag()")
        .unwrap();
    assert!(
        s.take_pet_set_actions().is_empty(),
        "…and a locked slot will not take the drop"
    );

    s.run(r#"LOCK_ACTIONBAR = "0""#).unwrap();
    s.run("this = PetActionButton5; PetActionButton_OnReceiveDrag()")
        .unwrap();
    assert_eq!(
        s.take_pet_set_actions(),
        vec![vec![(4, CLAW_WORD)]],
        "unlocked, the same drop lands"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Shift is tested above the button split (PetActionBarFrame.lua:254): either button picks up.
#[test]
fn shift_clicking_a_pet_button_picks_it_up_rather_than_casting() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();

    s.run(
        "IsShiftKeyDown = function() return 1 end \
         this = PetActionButton4 \
         PetActionButton_OnClick('RightButton')",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_pet_actions().is_empty() && s.take_pet_autocast_toggles().is_empty(),
        "shift outranks both click arms — no cast, no autocast flip"
    );
    assert_eq!(
        s.take_pet_set_actions(),
        vec![vec![(3, CLAW_WORD & 0xFFFF_0000)]]
    );
}

/// A bound `BONUSACTIONBUTTONn` runs `PetActionButtonDown/Up` (PetActionBarFrame.lua:218-231), a
/// bare `CastPetAction` on release. The live Attack is a possess bar's: `0x4bd420` raises the
/// attack latch only for a possessed unit, so on a hunter's bar key and click agree.
#[test]
fn the_keybind_pair_pushes_and_casts_without_the_clicks_forks() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();

    let state = |s: &UiScript| {
        s.eval::<String>("return PetActionButton1:GetButtonState()")
            .unwrap()
    };

    s.run("PetActionButtonDown(1)").unwrap();
    assert_eq!(state(&s), "PUSHED");
    assert!(
        s.take_pet_actions().is_empty(),
        "the press is visual only — the ref fires on the release"
    );

    // A left click on this live Attack would call it off; the key casts.
    s.run("PetActionButtonUp(1)").unwrap();
    assert_eq!(state(&s), "NORMAL");
    assert_eq!(s.take_pet_actions(), vec![1]);
    assert_eq!(
        s.take_pet_stop_attacks(),
        0,
        "IsPetAttackActive lives in OnClick, which a key press never reaches"
    );

    // The button-state guard: an up with nothing pushed does nothing, a second down no re-fire.
    s.run("PetActionButtonUp(1)").unwrap();
    assert!(
        s.take_pet_actions().is_empty(),
        "an unmatched release fires nothing"
    );
    s.run("PetActionButtonDown(1) PetActionButtonDown(1)")
        .unwrap();
    assert_eq!(state(&s), "PUSHED");
    s.run("PetActionButtonUp(1)").unwrap();
    assert_eq!(s.take_pet_actions(), vec![1], "one press, one cast");

    // Slot 2 is empty: `CastPetAction`'s own guard makes it inert.
    s.run("PetActionButtonDown(2) PetActionButtonUp(2)")
        .unwrap();
    assert!(
        s.take_pet_actions().is_empty(),
        "an unnamed slot queues nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Button centres: button 1 spans x[72,102] y[56,86], and the 30 px buttons chain +8.
const ATTACK_BUTTON: (f32, f32) = (87.0, 71.0);
const CLAW_BUTTON: (f32, f32) = (201.0, 71.0);

fn hovered_pet_bar() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The app's registries: `BONUSACTIONBUTTON1` binds CTRL-1 and `UberTooltips` reads "1".
    s.register_bindings(&crate::bindings::registry_commands());
    s.register_cvars(crate::cvars::registered_pairs());
    load_pet_bar(&s);
    declare_token_strings(&s);
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();
    s
}

fn tooltip_line1(s: &UiScript) -> String {
    s.eval::<String>("return GameTooltipTextLeft1:GetText() or \"\"")
        .unwrap()
}

/// `PetActionButton_OnEnter` appends the binding only on its `isToken or UberTooltips == "0"`
/// branch, colour code before the space: `Attack|cffffd200 (CTRL-1)|r`. The other branch is a
/// bare `SetPetAction` (PetActionBarFrame.lua:285-305).
#[test]
fn token_tooltips_name_their_binding_and_pet_spells_do_not() {
    benilla_formats::wow_data_or_skip!();
    let mut s = hovered_pet_bar();

    s.mouse_move(ATTACK_BUTTON.0, ATTACK_BUTTON.1);
    s.resolve();
    assert_eq!(
        tooltip_line1(&s),
        "Attack|cffffd200 (CTRL-1)|r",
        "a command token carries BONUSACTIONBUTTON1's key in the normal-font colour"
    );
    // With `UberTooltips` on, a token's plate still takes the default anchor.
    assert!(
        s.eval::<bool>("return GameTooltip.default ~= nil").unwrap(),
        "a token's plate takes the default corner while UberTooltips is on"
    );

    // The control: Claw, a pet spell, goes through `SetPetAction`.
    s.mouse_move(CLAW_BUTTON.0, CLAW_BUTTON.1);
    s.resolve();
    let claw = tooltip_line1(&s);
    assert_eq!(claw, "Claw", "a pet SPELL renders through SetPetAction");
    assert!(
        !claw.contains('('),
        "…and appends no binding: {claw:?} (the ref's other branch)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The append is unguarded and `GetBindingText` answers "" for a nil key (UIParent.lua:1819-1821),
/// so an unbound token shows empty parentheses. Stock binds CTRL-1..CTRL-0, so this unbinds one.
#[test]
fn an_unbound_token_row_renders_the_references_empty_parentheses() {
    benilla_formats::wow_data_or_skip!();
    let mut s = hovered_pet_bar();
    s.run(r#"SetBinding("CTRL-1")"#).unwrap();
    assert!(
        s.eval::<Option<String>>(r#"return GetBindingKey("BONUSACTIONBUTTON1")"#)
            .unwrap()
            .is_none(),
        "the row really is unbound now"
    );

    s.mouse_move(ATTACK_BUTTON.0, ATTACK_BUTTON.1);
    s.resolve();
    assert_eq!(
        tooltip_line1(&s),
        "Attack|cffffd200 ()|r",
        "unbound: the parentheses stay, empty — the ref's own unguarded concatenation"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// With `UberTooltips` off a pet spell takes the token branch too, binding suffix and all, and the
/// plate anchors `ANCHOR_RIGHT` of the button (PetActionBarFrame.lua:290-296).
#[test]
fn ubertooltips_off_takes_the_spells_through_the_token_branch_and_moves_the_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = hovered_pet_bar();
    s.set_cvar_engine("UberTooltips", "0");

    s.mouse_move(CLAW_BUTTON.0, CLAW_BUTTON.1);
    s.resolve();
    assert_eq!(
        tooltip_line1(&s),
        "Claw|cffffd200 (CTRL-4)|r",
        "with the CVar off a pet spell is built here too, binding and all"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(PetActionButton4)")
            .unwrap(),
        "…and the plate sits beside the button, not at the screen corner"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
