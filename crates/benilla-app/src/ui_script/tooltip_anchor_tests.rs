//! The tooltip ANCHOR law over the real shipped XMLs — where each hover SEATS the plate.
//!
//! The world/unit-frame/action-bar rows of the law all route through the ref's
//! `GameTooltip_SetDefaultAnchor` (ref GameTooltip.lua l.73-77): the screen's bottom-right
//! corner, `-CONTAINER_OFFSET_X - 13` in from the right, `CONTAINER_OFFSET_Y` up from the
//! bottom. These tests exist because the wiring had two silent holes only the live game showed
//! (the world tooltip parked ON the character): `GameTooltip.xml` never wired
//! `<OnTooltipSetDefaultAnchor>`, and the `UIParent` GLOBAL the ref handler passes didn't exist
//! — engine tests stubbed the handler, so nothing asserted the real files' geometry. Everything
//! here loads the shipped XMLs and asserts resolved rects / anchors, never a stub.

use benilla_ui::script::{AuraState, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// A 1024×768 screen with the anchor law's three fixed files (fonts, the real UIParent, the
/// real GameTooltip) plus `extra`. `CONTAINER_OFFSET_X/Y` hold their UIParent.xml load values
/// (0 / 70) — the manage pass only runs from the app's post-load bootstrap, so the expected
/// default corner in every test here is x = 1024−13 = 1011, y = 70.
fn harness(extra: &[&str]) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // `FACTION_BAR_COLORS`, which the stock `GameTooltip_UnitColor` indexes on every unit hover:
    // the reference defines it at ReputationFrame.lua's file scope (1968).
    load_xml(&s, r"Interface\FrameXML\ReputationFrame.lua");
    for f in extra {
        load_xml(&s, f);
    }
    // The stock tooltip declares no size: it sizes from its lines through the font engine, as
    // the client's does (1968) — every test here reads its rect, so the fixed-width font is
    // that engine. And 1.12 ships detailed tips ON (`SHOW_NEWBIE_TIPS = "1"`, UIOptionsFrame_Init's;
    // ours in OptionsFrame.xml's uvar block) — a harness without the options file says so itself.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s
}

fn wolf() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Timber Wolf".into()),
        health: 30,
        max_health: 50,
        level: 10,
        reaction: 2,
        creature_type_name: Some("Beast".into()),
        ..Default::default()
    }
}

/// UIParent is a real, named, full-screen frame (ref UIParent.xml l.5) — the Lua global
/// resolves, and its rect IS the screen.
#[test]
fn uiparent_is_a_real_full_screen_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.resolve();
    let ok: bool = s
        .eval(
            "return UIParent ~= nil and UIParent:GetName() == \"UIParent\" \
               and UIParent:GetLeft() == 0 and UIParent:GetBottom() == 0 \
               and UIParent:GetRight() == 1024 and UIParent:GetTop() == 768",
        )
        .unwrap();
    assert!(ok, "UIParent exists and fills the screen");
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The world mouseover seats the plate at the DEFAULT corner — the engine fires
/// `OnTooltipSetDefaultAnchor`, the shipped handler (ref GameTooltip.lua l.73-77 via
/// ref GameTooltipTemplate.xml l.617-619) anchors BOTTOMRIGHT to UIParent at (−13, 70).
/// THE regression test for the "tooltip on my character" bug: without the wiring the plate
/// kept its load-time position instead.
#[test]
fn world_hover_seats_the_default_corner() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.set_unit("mouseover", Some(wolf()));
    assert!(s.world_tooltip_unit("mouseover"), "the hover shows");
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    s.resolve();
    let ok: bool = s
        .eval(
            "return GameTooltip:IsVisible() \
               and GameTooltip:GetRight() == 1011 and GameTooltip:GetBottom() == 70",
        )
        .unwrap();
    assert!(ok, "world tooltip sits at the screen's bottom-right corner");
}

/// Unit-frame hovers take the SAME default corner (ref UnitFrame_OnEnter l.56 calls
/// GameTooltip_SetDefaultAnchor, not an owner anchor).
///
/// Leave drops the plate AT ONCE, not on the fade ramp: `UnitFrame_OnLeave` l.84-88 branches on
/// `SHOW_NEWBIE_TIPS`, and 1.12's default is on — `FadeOut` is the tips-off arm. The
/// world mouseover keeps the ramp; this is the unit *frame*.
#[test]
fn unit_frame_hover_takes_the_default_corner_and_drops_on_leave() {
    benilla_formats::wow_data_or_skip!();
    // The kit + popups precede the unit frames (their DropDown children's OnLoad), app order.
    let mut s = harness(&[
        // The stock unit frames resolve GlobalStrings at LOAD (`CombatFeedback.lua` l.7-17,
        // `UnitFrame.lua` l.1-6) and `UnitFrame_OnEnter` passes `PARTY_OPTIONS_LABEL` /
        // `PLAYER_OPTIONS_LABEL` (l.60/63) straight into `GameTooltip:SetText`, which raises on
        // nil. The app loads this file ahead of the whole manifest; so does the fixture.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
    ]);
    s.set_unit("target", Some(wolf()));
    // The reference's handler takes no arguments and reads `this` (ref UnitFrame.lua l.45), which
    // is exactly how the stock frames wire it: `<OnEnter>UnitFrame_OnEnter();</OnEnter>`
    // (ref TargetFrame.xml l.505, PlayerFrame.xml l.435). Our deleted transcription's
    // `BenillaUnitFrame_OnEnter(frame)` adapter went with the file.
    s.run("this = TargetFrame UnitFrame_OnEnter() this = nil")
        .unwrap();
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    s.resolve();
    let ok: bool = s
        .eval(
            "return GameTooltip:IsVisible() \
               and GameTooltip:GetRight() == 1011 and GameTooltip:GetBottom() == 70 \
               and GameTooltip:IsOwned(TargetFrame)",
        )
        .unwrap();
    assert!(
        ok,
        "unit-frame tooltip sits at the default corner, owned by the frame"
    );
    // A wolf is no player, so the hover is the ordinary unit readout — the fork below never fires.
    assert!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap()
            .contains("Wolf"),
        "a non-player target still gets the unit lines"
    );
    // Leave: gone on the spot, no ramp to wait out.
    s.run("this = TargetFrame UnitFrame_OnLeave() this = nil")
        .unwrap();
    let hidden: bool = s.eval("return not GameTooltip:IsShown()").unwrap();
    assert!(
        hidden,
        "unit-frame tooltip hides on leave, it does not fade"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// **The title a world hover paints while the creature query is still in flight**, over the
/// shipped files: `UNKNOWNOBJECT` as `GlobalStrings.lua` defines it on the PLAYER'S OWN CHAIN.
/// Read out of the VM at the assert rather than written as a literal, because that is the whole
/// point of the resolver's `0x703bf0` read — a translated GlobalStrings translates the
/// placeholder too (closing 2002's residue).
///
/// The engine half (the miss legs, the empty-global fallback, the `"player"` case) is
/// `benilla_ui`'s own `tooltip_unit` suite; what this adds is the chain: the string really is
/// defined, the plate really reads it, and the answer really replaces it.
#[test]
fn a_pending_name_hover_titles_the_chains_unknownobject() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness(&["Interface\\FrameXML\\GlobalStrings.lua"]);
    // The snapshot the feed pushes before `SMSG_CREATURE_QUERY_RESPONSE` lands: the descriptor is
    // in, and the name and the type word — which ride the same record — are not.
    s.set_unit(
        "mouseover",
        Some(UnitState {
            name: None,
            creature_type_name: None,
            ..wolf()
        }),
    );
    assert!(s.world_tooltip_unit("mouseover"), "the hover shows");
    let global = s.eval::<String>("return UNKNOWNOBJECT").unwrap();
    assert!(
        !global.is_empty(),
        "the chain's GlobalStrings.lua defines UNKNOWNOBJECT"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        global,
        "a name in flight titles the plate with the GlobalString, not an empty line"
    );

    // The query answers; the next paint of the same hover carries the real name.
    s.set_unit("mouseover", Some(wolf()));
    assert!(s.world_tooltip_unit("mouseover"));
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Timber Wolf"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The detailed-tooltip fork (ref UnitFrame_OnEnter l.58-67, director-approved 0663): with tips on
/// — the 1.12 default — the frame explains its RIGHT-CLICK MENU and returns BEFORE `SetUnit`, so
/// the unit lines never render. Your own portrait always; another player's whenever they're your
/// target.
///
/// This is the most behaviour-changing thing in the newbie-tip arc, so it is pinned from both
/// sides: the explanation replaces the readout, and a non-player target still gets the readout
/// (the sibling test above).
#[test]
fn your_own_portrait_explains_the_menu_instead_of_showing_your_health() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        // The stock unit frames resolve GlobalStrings at LOAD (`CombatFeedback.lua` l.7-17,
        // `UnitFrame.lua` l.1-6) and `UnitFrame_OnEnter` passes `PARTY_OPTIONS_LABEL` /
        // `PLAYER_OPTIONS_LABEL` (l.60/63) straight into `GameTooltip:SetText`, which raises on
        // nil. The app loads this file ahead of the whole manifest; so does the fixture.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
    ]);
    s.set_unit("player", Some(wolf()));

    s.run("this = PlayerFrame UnitFrame_OnEnter() this = nil")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Party Options",
        "your own frame explains the party menu"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft2:GetText()")
            .unwrap(),
        s.eval::<String>("return NEWBIE_TOOLTIP_PARTYOPTIONS")
            .unwrap()
    );
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        2,
        "it RETURNS before SetUnit — no health/level lines underneath"
    );

    // A player target takes the other arm. `player` stays a wolf here on purpose: the ref reads
    // UnitIsPlayer on the "target" token alone, never on the hovered frame's own unit.
    s.set_unit(
        "target",
        Some(UnitState {
            is_player: true,
            player_controlled: true,
            name: Some("Someone".into()),
            ..wolf()
        }),
    );
    s.run("this = TargetFrame UnitFrame_OnEnter() this = nil")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Player Options",
        "another player's frame explains the player menu"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Action-bar hovers take the default corner too — ref ActionButton_SetTooltip l.366-372
/// branches on the UberTooltips CVar, whose stock default is "1" (byte-read from WoW.exe
/// 0x48fdd9 / default string 0x82e748; see `cvars::REGISTERED`). An empty slot renders nothing,
/// but the anchor must already be seated — asserted through GetPoint, resolved rect or not.
///
/// Registered, not merely absent: before B230 the CVar was not in the table at all, so `GetCVar`
/// answered nil and the "1" leg was reached by accident rather than by value. This seeds the real
/// table so the pass means what it says.
#[test]
fn action_button_hover_takes_the_default_corner() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
    ]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.run("this = ActionButton3 ActionButton_SetTooltip()")
        .unwrap();
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    let ok: bool = s
        .eval(
            "local p, rel, rp, x, y = GameTooltip:GetPoint() \
             return p == \"BOTTOMRIGHT\" and rel ~= nil and rel:GetName() == \"UIParent\" \
               and rp == \"BOTTOMRIGHT\" and x == -13 and y == 70",
        )
        .unwrap();
    assert!(
        ok,
        "action-button hover anchors the plate to UIParent's bottom-right"
    );
}

/// The other leg of that same branch, live since B230 registered the CVar: with `UberTooltips`
/// off, an action button's plate leaves the screen corner and seats BESIDE the button — LEFT for
/// the three bars the reference lists (`MultiBarBottomRight`, `MultiBarRight`, `MultiBarLeft`: the
/// ones at or against the right edge, whose plates have to open toward the centre), RIGHT for
/// everything else, including the main bar. All three exist here since 1219/1500; the set used to
/// be the first alone.
///
/// The two anchors are read off the resolved SetPoint pair, which is what `SetOwner` actually
/// writes: ANCHOR_RIGHT = the plate's BOTTOMLEFT on the button's TOPRIGHT, ANCHOR_LEFT its mirror
/// (`script/tooltip/verbs.rs`).
#[test]
fn ubertooltips_off_seats_action_bar_plates_beside_the_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
    ]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_cvar_engine("UberTooltips", "0");

    let seat = |s: &UiScript| {
        s.eval::<String>(
            "local p, rel, rp = GameTooltip:GetPoint() \
             return p .. \"/\" .. (rel and rel:GetName() or \"?\") .. \"/\" .. rp",
        )
        .unwrap()
    };

    s.run("this = ActionButton3 ActionButton_SetTooltip()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip.default == nil").unwrap(),
        "off: the main bar's plate is owner-anchored, not the default corner"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(ActionButton3)")
            .unwrap(),
        "…owned by the button it opened from"
    );
    assert_eq!(
        seat(&s),
        "BOTTOMLEFT/ActionButton3/TOPRIGHT",
        "ANCHOR_RIGHT — the main bar is not in the ref's LEFT set"
    );

    // All three members of the ref's LEFT set, by frame — membership is not gated on visibility,
    // and the two vertical bars are hidden until their option is ticked.
    for bar in ["MultiBarBottomRight", "MultiBarRight", "MultiBarLeft"] {
        s.run(&format!("this = {bar}Button1 ActionButton_SetTooltip()"))
            .unwrap();
        assert_eq!(
            seat(&s),
            format!("BOTTOMRIGHT/{bar}Button1/TOPLEFT"),
            "ANCHOR_LEFT — {bar} opens toward screen centre"
        );
    }

    // And the CVar back on restores the corner — the fork is a fork, not a one-way door.
    s.set_cvar_engine("UberTooltips", "1");
    s.run("this = MultiBarBottomRightButton1 ActionButton_SetTooltip()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip.default ~= nil").unwrap(),
        "on: back to the screen corner"
    );
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
}

/// The stance bar's own leg of the same branch (ref BonusActionBarFrame.xml l.40-45 — ANCHOR_RIGHT
/// with no bar fork of its own), pinned separately because it is a different file's handler and a
/// different tooltip verb; it was collapsed to the "1" leg alongside the action bar's and
/// un-collapsed with it.
#[test]
fn ubertooltips_off_seats_stance_plates_beside_the_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
    ]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_shapeshift_forms(vec![benilla_ui::script::ShapeshiftFormView {
        spell_id: 5487,
        name: "Bear Form".into(),
        texture: Some("Interface\\Icons\\Ability_Racial_BearForm".into()),
        active: false,
        castable: true,
        cooldown: None,
    }]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    s.resolve();

    s.run("this = ShapeshiftButton1 ShapeshiftButton1:GetScript(\"OnEnter\")()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip.default ~= nil").unwrap(),
        "on (the stock default): the screen corner"
    );

    s.set_cvar_engine("UberTooltips", "0");
    s.run("this = ShapeshiftButton1 ShapeshiftButton1:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "local p, rel, rp = GameTooltip:GetPoint() \
             return p .. \"/\" .. (rel and rel:GetName() or \"?\") .. \"/\" .. rp",
        )
        .unwrap(),
        "BOTTOMLEFT/ShapeshiftButton1/TOPRIGHT",
        "off: ANCHOR_RIGHT, beside the button"
    );
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
}

/// Buff hovers hang BELOW the button — ref BuffFrame.xml l.37 is ANCHOR_BOTTOMLEFT (the buff
/// row lives at the screen's top-right): the tooltip's TOPRIGHT seats on the button's
/// BOTTOMLEFT.
#[test]
fn buff_hover_hangs_below_left_of_the_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
    ]);
    s.set_auras(
        "player",
        Some(vec![AuraState {
            spell_id: 1459,
            name: Some("Arcane Intellect".into()),
            icon: Some("Interface\\Icons\\Spell_Holy_MagicalSentry".into()),
            count: 1,
            debuff_type: None,
            duration: 1800.0,
            expiration_time: 1800.0,
            helpful: true,
            cancelable: true,
            until_cancelled: false,
            channeled: false,
        }]),
    );
    // The reference's own event, which the buff buttons register for (`ui_aura` fires it beside
    // the Era-shaped UNIT_AURA on the same rebuild).
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    s.resolve();
    // Through the template's real `<OnEnter>` — the reference keeps the SetOwner/SetPlayerBuff pair
    // inline there rather than in a named function, so the handler body itself is what this drives.
    // `this` is set by hand because the engine sets it only when it *fires* a handler (`0x704d50`);
    // calling the compiled function directly does not, and the body reads `this`, not its argument.
    s.run("this = BuffButton0; BuffButton0:GetScript(\"OnEnter\")(BuffButton0)")
        .unwrap();
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    let ok: bool = s
        .eval(
            "local p, rel, rp = GameTooltip:GetPoint() \
             return p == \"TOPRIGHT\" and rel ~= nil and rel:GetName() == \"BuffButton0\" \
               and rp == \"BOTTOMLEFT\"",
        )
        .unwrap();
    assert!(
        ok,
        "buff tooltip hangs its TOPRIGHT on the button's BOTTOMLEFT"
    );
}

/// **The cursor-seated GameObject plate carries an OWNER** — the store the reference's publisher
/// makes through the SetOwner core (`0x492a01 → 0x52ffe0(owner, 6, 0, 0)`, whose `0x53000c`
/// writes `+0x314`), and the one arm of ours that used to skip it.
///
/// Every other world plate reached an owner by accident, through Lua: the corner arm and the unit
/// flow both fire `OnTooltipSetDefaultAnchor`, and the stock handler calls
/// `GameTooltip:SetOwner(UIParent, …)`. The cursor arm fires nothing, so `IsOwned` answered false
/// for exactly the GENERIC(5) objects — a signpost, a mailbox — and the next test is what that
/// cost.
#[test]
fn a_cursor_seated_gameobject_plate_is_owned() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    assert!(s.world_tooltip_gameobject("Brill", &[], Some((512.0, 384.0))));
    let owned: bool = s.eval("return GameTooltip:IsOwned(UIParent)").unwrap();
    assert!(
        owned,
        "the signpost plate is owned; errors: {:?}",
        s.errors()
    );
}

/// **An addon's `OnShow` hook must not hide the plate the world hover just built** — the
/// director's signpost with no tooltip.
///
/// `!Questie` installs an `OnShow` on GameTooltip at PLAYER_LOGIN (`Questie:hookTooltip` — it
/// installs one precisely *because* the stock plate has none) whose handler ends in
/// `GameTooltip:Show()`. Lua's `:Show()` is the reference's EXISTENCE GATE `0x530a80`: owner and
/// line count both non-zero, or it takes the effective-hide `0x530a60` instead. So an unowned
/// plate hides itself the instant it is shown — through our own faithful implementation of that
/// gate, ~26 ms after the engine built it, on every signpost, for the whole session.
///
/// The reference cannot reach that state, because its publisher writes the owner *before* the
/// plate is ever shown. With the owner written, so do we.
#[test]
fn a_cursor_seated_gameobject_plate_survives_an_addons_on_show_hook() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    // Questie's hook, in one line: the plate's own show event calls Show() again.
    s.run(r#"GameTooltip:SetScript("OnShow", function() GameTooltip:Show() end)"#)
        .unwrap();
    assert!(s.world_tooltip_gameobject("Brill", &[], Some((512.0, 384.0))));
    let shown: bool = s
        .eval("return GameTooltip:IsShown() and true or false")
        .unwrap();
    assert!(
        shown,
        "the signpost plate is still up; errors: {:?}",
        s.errors()
    );
}

/// **The CORNER arm must end up owned too — the other half of the existence gate**.
///
/// The reference's corner arm (`0x492a42`) writes no owner itself; the owner is restored purely by
/// the `+0x444` handler, `OnTooltipSetDefaultAnchor` → `GameTooltip_SetDefaultAnchor(this,
/// UIParent)` → `SetOwner(UIParent, "ANCHOR_NONE")`. And the owner really is 0 on the way in:
/// `Tooltip::Hide 0x530a60` *is* `SetOwner(NULL, 0, 0, 0)`, so every hover starts un-owned.
///
/// That makes this test the precondition for narrowing the placement fork at all. Moving an object
/// from the cursor arm to the corner arm is only safe while the corner arm produces an OWNED,
/// SHOWN plate — otherwise those objects would build their lines and then hide, which is precisely
/// the failure 2255 had just fixed on the cursor arm.
#[test]
fn a_corner_seated_gameobject_plate_is_owned_and_shown() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    assert!(s.world_tooltip_gameobject("Ironforge Main Gate", &[], None));
    let owned: bool = s.eval("return GameTooltip:IsOwned(UIParent)").unwrap();
    assert!(owned, "the corner plate is owned; errors: {:?}", s.errors());
    let shown: bool = s
        .eval("return GameTooltip:IsShown() and true or false")
        .unwrap();
    assert!(shown, "and it is on screen; errors: {:?}", s.errors());
}

/// And it survives the same addon hook the cursor arm had to: `!Questie`'s `OnShow` handler ends in
/// `GameTooltip:Show()`, and `:Show()` is the existence gate `0x530a80`.
#[test]
fn a_corner_seated_gameobject_plate_survives_an_addons_on_show_hook() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.run(r#"GameTooltip:SetScript("OnShow", function() GameTooltip:Show() end)"#)
        .unwrap();
    assert!(s.world_tooltip_gameobject("Ironforge Main Gate", &[], None));
    let shown: bool = s
        .eval("return GameTooltip:IsShown() and true or false")
        .unwrap();
    assert!(
        shown,
        "the corner plate is still up; errors: {:?}",
        s.errors()
    );
}
