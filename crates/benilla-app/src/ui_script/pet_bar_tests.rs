//! The pet action bar (the reference's `PetActionBarFrame.xml`, 1953) driven end to end through the REAL shipped XML
//! — the `multibar_stance_tests` pattern: a self-contained loader, then the
//! whole chain from a pushed slot list to the quads it actually paints.

use benilla_ui::script::{PetActionView, QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The pet bar's own load prerequisites, in manifest order: UiPanels (`SetDesaturation`, the
/// disabled-bar grey), UIParent (the managed bottom stack its OnShow/OnHide re-fires), Cooldown
/// (`CooldownFrame_SetTimer`), ActionBar (the `MainMenuBar` anchor target) — then the bar.
/// The reference's pet bar needs what the manifest loads before it: the action-bar chain
/// (`ActionButton_UpdateState`, `MainMenuBar` as its parent, `CooldownFrame_SetTimer`), the
/// options window's uvars for `MultiActionBars.lua`, `SetDesaturation` and `GetBindingText` from
/// our UIParent.lua slices, `TEXT` — and the chat window, whose edit box `ShowPetActionBar`
/// raises over the sliding bar (PetActionBarFrame.lua l.175). One chain, in the manifest's order,
/// for every test here (1953).
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

/// GlobalStrings is loaded from the MPQ at runtime, not by the loader — a token slot's `name` is
/// a KEY into it, so the tests declare the two keys they read. (That the real keys exist in the
/// shipped file is a separate fact, asserted in `ui_pet`'s own tests by name.)
pub(super) fn declare_token_strings(s: &UiScript) {
    s.run(
        "PET_ACTION_ATTACK = 'Attack' \
         PET_MODE_DEFENSIVE = 'Defensive'",
    )
    .unwrap();
}

/// The words the drag moves — `ACT_COMMAND`/Attack and `ACT_ENABLED`/Claw as the server packs
/// them. Carried on the views because the drag is word arithmetic.
const ATTACK_WORD: u32 = 0x0700_0002;
const CLAW_WORD: u32 = 0xC100_0BC2;

/// A hunter's bar as the server actually sends it: Attack (a lit command token), an empty spell
/// slot, Claw (a spell with autocast running), and Defensive (a lit reaction token).
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

/// Count `path` quads **in the pet bar's own row**. The prerequisite `ActionBar.xml` brings 12
/// action buttons wearing the same `UI-Quickslot2` ring, so an unscoped count would be measuring
/// the main bar. Cut by the quad's CENTRE, not an edge: the button rings overhang their buttons by
/// a dozen pixels each way (54 px of ring on a 30 px button), so the two rows' extents actually
/// overlap even though their centres — 21 for the main bar, 70..75 for the pet bar — do not.
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

/// The whole bar, end to end: hidden with no pet, shown with one, the three slot classes each
/// painting what they should, and hidden again when the pet goes.
#[test]
fn the_shipped_pet_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_pet_bar(&s);
    declare_token_strings(&s);

    // No pet: the bar is hidden, and nothing of it reaches the quad pass.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    assert_eq!(
        textures(&s.extract(), "Interface\\PetActionBar\\UI-PetBar"),
        0,
        "no pet, no shelf"
    );

    // A pet appears.
    s.set_pet_actions(true, true, true, hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.tick(0.05);
    s.resolve();
    let quads = s.extract();

    // The shelf art: both UI-PetBar strips draw.
    assert_eq!(
        textures(&quads, "Interface\\PetActionBar\\UI-PetBar"),
        2,
        "the two shelf strips"
    );

    // A TOKEN slot resolved its texture through the global the app named — the `getglobal` fork.
    // Seeing the ICON PATH here (not the literal "PET_ATTACK_TEXTURE") is the whole proof.
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
    // A SPELL slot took its path verbatim.
    assert_eq!(textures(&quads, "Interface\\Icons\\Ability_Druid_Rake"), 1);

    // Occupancy: 3 named slots show their filled ring, the 7 unnamed ones are hidden outright
    // (the reference hides an unnamed button rather than showing an empty well).
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

    // The checked ring is on both lit slots, and nowhere else.
    assert_eq!(
        textures(&quads, "Interface\\Buttons\\CheckButtonHilight"),
        2,
        "Attack + Defensive are lit; Claw is not"
    );

    // Autocast: the static ring on the one slot that allows it, and the stock shine MODEL on the
    // one slot where it is running — `$parentAutoCast`, `UI-AutoCastButton.mdx` over the whole
    // button at `scale="1.2"`, rendered as a tile. A hidden pane is not
    // extracted, so one pane is exactly one running shine.
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

    // Geometry, quoted from the ref: the bar's TOPLEFT is MainMenuBar's BOTTOMLEFT +(36,97),
    // and MainMenuBar is the 1024x53 frame at screen BOTTOM ⇒ its BOTTOMLEFT is (0,0), so the
    // frame spans y[54,97]. Button 1 sits at the frame's own BOTTOMLEFT +(36,2) ⇒ x[72,102]
    // y[56,86] — the same 72 the reference lands on (its PETACTIONBAR_XPOS 36 + the button's 36).
    let attack =
        texture_rect(&quads, "Interface\\Icons\\Ability_GhoulFrenzy").expect("Attack icon");
    assert_eq!(
        (attack.left, attack.bottom, attack.right, attack.top),
        (72.0, 56.0, 102.0, 86.0)
    );

    // The pet goes: the bar slides out over the reference's PETACTIONBAR_SLIDETIME (0.09 s,
    // `PetActionBarFrame_OnUpdate`) and hides at the end of the slide, shine marker and all.
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

/// The click law: left runs the slot, right flips autocast, and a left click on the ATTACK slot
/// while the pet is already attacking calls it off instead of re-ordering it (the reference's
/// `IsPetAttackActive` fork — the one branch that makes the Attack button a toggle).
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

    // Button 1 (Attack) spans x[72,102] y[56,86] ⇒ centre (87,71). Claw is slot 4: the buttons
    // chain +8 on a 30 px width, so button 4's left = 72 + 3*38 = 186 ⇒ centre (201,71).
    let click = |s: &mut UiScript, x: f32, y: f32, button: &str| {
        s.mouse_button(x, y, button, true);
        s.mouse_button(x, y, button, false);
    };

    // The pet IS attacking, so a left click on Attack calls it off — no action is queued.
    click(&mut s, 87.0, 71.0, "LeftButton");
    assert_eq!(s.take_pet_stop_attacks(), 1);
    assert!(
        s.take_pet_actions().is_empty(),
        "the call-off replaces the press, it does not accompany it"
    );

    // Not attacking any more: the same click orders the attack.
    let mut slots = hunter_slots();
    slots[0].active = false;
    slots[0].attack_active = false;
    s.set_pet_actions(true, true, true, slots);
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.resolve();
    click(&mut s, 87.0, 71.0, "LeftButton");
    assert_eq!(s.take_pet_actions(), vec![1]);
    assert_eq!(s.take_pet_stop_attacks(), 0);

    // Right-clicking the spell slot flips its autocast; right-clicking a command token does not
    // (a token has no spell id for the wire verb to name).
    click(&mut s, 201.0, 71.0, "RightButton");
    assert_eq!(s.take_pet_autocast_toggles(), vec![4]);
    click(&mut s, 87.0, 71.0, "RightButton");
    assert!(
        s.take_pet_autocast_toggles().is_empty(),
        "a command token cannot autocast"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A click takes the ring off; the repaint puts it back** — the contract decision 1021's signal
/// count depends on, pinned here so it cannot quietly rot.
///
/// `PetActionButton_OnClick`'s first line is `this:SetChecked(0)` and it runs for every click, so
/// the ring always comes off. What puts it back is `PET_BAR_UPDATE` — and the repaint that answers
/// it must re-derive `SetChecked` from `isActive` per slot, **not** diff the views, or a press that
/// changed no state would have no way home.
///
/// The reference signals that repaint from the state writes (`0x4bc940`/`0x4bc960`) and **not**
/// from a `TogglePetAutocast` it refuses (`0x4bcbf7` — a token is not autocastable, i.e. every
/// right-click on Follow, Stay or a reaction). So on a right-click the ring genuinely stays off
/// until something else repaints the bar. That is the reference's own quirk, checked there by the
/// director; decision 1027 diverged from it and 1030 put it back.
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

    // Button 1 (Attack, lit) spans x[72,102] y[56,86] ⇒ centre (87,71). Both mouse buttons take
    // the ring off — the CheckButton toggles itself, then `SetChecked(0)` lands it at 0 either way.
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

        // A repaint carrying the very SAME views restores it — the property the signal count buys.
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

/// A DISABLED bar still draws — every icon desaturated, nothing hidden. That pair
/// (`PetHasActionBar` true, `GetPetActionsUsable` false) is what a feared or mind-controlled pet
/// looks like, and collapsing it to "hide the bar" would lose the state entirely.
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
    // Since decision 1327 `SetDesaturation` takes its SHADER arm — the icon carries the greyscale
    // flag to the renderer and keeps its own tint, rather than the reference's no-shader 0.5
    // vertex-colour fallback (which is what this asserted while nothing greyed).
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

/// The pet bar with and without the bottom-left multibar RAISED. Since 1500 that bar is a player
/// option shipping off, so the flag now raises it rather than merely loading the file — both states
/// are ones a real session lands in. `shelf` counts the two `UI-PetBar` strips; `attack_top` is the
/// top edge of Attack's icon, the bar's own row in one number.
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

/// **The row the pet bar shares with the bottom-left multibar** (director-caught:
/// the bar drew straight across a live row of spells, border and all).
///
/// Two rules, both the reference's, both applied by `UIParent_ManageFramePositions`: with that bar
/// up the pet bar rises by the ref's 43 px, and its shelf art — which is a border only while the
/// bar sits directly on the main bar — is hidden rather than drawn over the buttons below.
///
/// The regression this locks is specifically that the manage pass RUNS: the bar's own XML anchor
/// is the base position, so a pass that never fired would leave it exactly where the bug was.
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

/// **The drag, through the shipped XML**. One verb serves both ends — the button's
/// `OnDragStart` and its `OnReceiveDrag` both call `PickupPetAction` — so what makes this a move
/// rather than two pick-ups is the binding's own fork on whether the cursor is already carrying.
///
/// Also locks the grid: while a pet action rides the cursor every slot shows, including the empty
/// ones, so there is somewhere visible to drop it. That is the whole reason `PET_BAR_SHOWGRID`
/// exists, and the reason an unnamed button's `Hide()` is conditional.
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

    // Slot 5 is empty, so its button is hidden — until the grid comes up.
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

/// The pet bar honours **Lock ActionBars** too — the reference's second consumer of
/// the same uvar (`PetActionBarFrame.lua:270/278`), reading the global `ActionBar.xml` declares.
/// Both drag ends refuse while locked; the shift-click pick-up above stays live, exactly as the
/// reference leaves it (l.253-255).
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

    // Shift-click is still the way through, and then the drop is refused too while locked.
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

/// A shift-click is the same pick-up, whichever mouse button carried it — the reference's own fork
/// puts shift above the left/right split, so it never toggles autocast by accident.
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

/// **The keybind pair** — what a bound `BONUSACTIONBUTTONn` runs. The ref's
/// binding body calls `BonusActionButtonDown/Up`, one-liners onto `PetActionButtonDown/Up`
/// (l.218-231), so the whole lane is these two functions: press shows the pushed art, release
/// fires the slot.
///
/// The load-bearing half is what the key path does NOT do: it is a bare `CastPetAction`, with none
/// of `PetActionButton_OnClick`'s forks. Pinned below on an Attack token fed as already-active — a
/// left click on that same view calls the attack off (the test above) while the key re-issues it.
///
/// **What that fixture is and is not.** `attack_active` is pushed straight onto the view here, and
/// on a real *pet* bar it can never be true: `ui_pet`'s `possessing` carve (`0x4bd420`) raises the
/// latch only for a unit you are POSSESSING, so on a hunter's bar the click fork is dead code and
/// key and click behave identically. The state below is a **possess** bar's — Mind Control, Eye of
/// Kilrogg — which is the one case where the two paths genuinely diverge, and therefore the only
/// one worth pinning. Read as "ordinary pets differ from clicks", this test would be lying.
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

    // Down shows the pushed art and fires nothing (the ref's runOnUp shape).
    s.run("PetActionButtonDown(1)").unwrap();
    assert_eq!(state(&s), "PUSHED");
    assert!(
        s.take_pet_actions().is_empty(),
        "the press is visual only — the ref fires on the release"
    );

    // Up releases the art and casts — slot 1 is Attack and the view says the order is live, so a
    // LEFT CLICK here would call it off instead (a possess bar's case; see the header). The key
    // has no such fork.
    s.run("PetActionButtonUp(1)").unwrap();
    assert_eq!(state(&s), "NORMAL");
    assert_eq!(s.take_pet_actions(), vec![1]);
    assert_eq!(
        s.take_pet_stop_attacks(),
        0,
        "IsPetAttackActive lives in OnClick, which a key press never reaches"
    );

    // The state guard is the whole re-entrancy story: an up with nothing pushed does nothing
    // (a focus-stolen release, a stuck-latch sweep), and a second down does not re-fire.
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

    // An EMPTY slot is inert through the same path (CastPetAction's own guard): slot 2 of the
    // hunter bar carries no name, and its button is hidden.
    s.run("PetActionButtonDown(2) PetActionButtonUp(2)")
        .unwrap();
    assert!(
        s.take_pet_actions().is_empty(),
        "an unnamed slot queues nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// The autocast trail's MOTION is the model's own since decisions 2013/2014: the stock
// `$parentAutoCast` Model's four bones and four emitters, rendered as a tile on the pane's clock
// — the script layer never moves a spark, and no test here needs to.

/// A hunter bar hovered through the REAL gesture, with the real binding registry and the real CVar
/// table behind it — `mouse_move` runs the shipped `<OnEnter>` with `this` bound, which is the only
/// way this fork gets exercised the way a player's hover exercises it.
///
/// Buttons are 30 px chained +8 from the bar's own origin: button 1 spans x[72,102] y[56,86] ⇒
/// centre (87,71), button 4's left = 72 + 3·38 ⇒ centre (201,71) — the geometry
/// `clicks_route_through_the_attack_toggle_fork` already leans on.
const ATTACK_BUTTON: (f32, f32) = (87.0, 71.0);
const CLAW_BUTTON: (f32, f32) = (201.0, 71.0);

fn hovered_pet_bar() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The real command set and the real registered defaults, the way the app's own seed installs
    // them — so `GetBindingKey("BONUSACTIONBUTTON1")` answers CTRL-1 (its byte-real default) and
    // `GetCVar("UberTooltips")` answers "1" (its byte-read registrar value) rather than nil.
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

/// **B230 — Attack/Follow/Stay name their keybinding; the pet's own spells do not.**
///
/// On 1.12, hovering Attack, Follow or Stay shows the button's keybinding in its tooltip. That is
/// narrower than "the pet bar": `PetActionButton_OnEnter` (PetActionBarFrame.lua l.285-305) forks
/// on `isToken or UberTooltips == "0"`, and only THAT branch concatenates
/// `NORMAL_FONT_COLOR_CODE.." ("..GetBindingText(GetBindingKey("BONUSACTIONBUTTON"..id), "KEY_")..")"`.
/// The other branch is a bare `SetPetAction` and appends nothing — so a fix that suffixed every
/// slot would be wrong, and the control below is what says ours doesn't.
///
/// The exact string is pinned, colour codes and all, because the ref's concatenation puts the
/// colour BEFORE the space (`Attack|cffffd200 (CTRL-1)|r`, not `Attack |cffffd200(CTRL-1)|r` —
/// which is what the stock `MicroButtonTooltipText` — the same idea, the other shape — produces).
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
    // Uber on + a token ⇒ still the default corner; only the CVar-off leg owner-anchors.
    assert!(
        s.eval::<bool>("return GameTooltip.default ~= nil").unwrap(),
        "a token's plate takes the default corner while UberTooltips is on"
    );

    // The CONTROL: slot 4 is Claw, a real pet spell, and it goes through the engine channel —
    // its name, and nothing in parentheses.
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

/// The append is UNCONDITIONAL in that branch, so an unbound row really does render empty
/// parentheses — `GetBindingText` answers `""` for a nil key (UIParent.xml, transcribed from the
/// ref's own l.1819-1821), and nothing guards the `"("`. That is the reference's behaviour, not a
/// slip of ours: MicroButtonTooltipText guards its own suffix and this one does not.
///
/// Stock it is unreachable — BONUSACTIONBUTTON1-10 default to CTRL-1..CTRL-0 — so it takes a
/// deliberate unbind to see, which is exactly what this does.
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

/// The CVar is the fork's OTHER half, and it is not decoration: with `UberTooltips` off, a real pet
/// SPELL stops going through the engine channel and takes the token branch too — same built text,
/// same binding suffix — and the plate moves from the screen corner to beside the button
/// (`SetOwner(this, "ANCHOR_RIGHT")`). Both are the reference's, in the same five lines.
///
/// This is also the row's reason to exist in `cvars::REGISTERED`: benilla registers `UberTooltips`
/// at all because these Lua sites read it (the honest-tree rule's live-consumer half).
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
