//! The stock group-loot roll popups (`GroupLootFrame1..4` in `LootFrame.xml`), driven by a pushed
//! [`LootRollsState`] and `START_LOOT_ROLL`/`CANCEL_LOOT_ROLL`.

use benilla_ui::script::{
    DressUpIntent, ExtractedQuad, LootRollEntry, LootRollsState, QuadContent, ScriptValue, UiScript,
};

use super::test_ui::{load_ui as load_xml, load_ui_no_warnings as load_xml_no_warnings};

/// The centre of the first texture quad whose path contains `needle`, for clicking it.
fn quad_center(quads: &[ExtractedQuad], needle: &str) -> (f32, f32) {
    let r = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
        .and_then(|q| q.rect)
        .unwrap_or_else(|| panic!("no quad for {needle}"));
    ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)
}

/// The colour of the first text quad reading `t`.
fn text_color(quads: &[ExtractedQuad], t: &str) -> Option<[f32; 4]> {
    quads.iter().find_map(|q| match &q.content {
        QuadContent::Text {
            text: Some(x),
            color,
            ..
        } if x == t => Some(*color),
        _ => None,
    })?
}

/// The four popups, declared in stock `LootFrame.xml`. The caller loads `UIParent.xml`, whose
/// `UIParent_OnEvent` routes `START_LOOT_ROLL` (`UIParent.lua:513-516`).
fn load_group_loot(s: &UiScript) {
    // `GroupLootDropDown`'s OnLoad runs its initializer at once, which reads `MAX_PARTY_MEMBERS`
    // (`LootFrame.lua:217`), a constant of `PartyMemberFrame.lua`.
    load_xml(s, r"Interface\FrameXML\UIDropDownMenu.xml");
    load_xml(s, r"Interface\FrameXML\PartyMemberFrame.lua");
    load_xml(s, r"Interface\FrameXML\LootFrame.xml");
}

fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The loot window's labels (`ITEMS`, `PREV`, `NEXT`) are GlobalStrings keys; a missing one is
    // a loader warning, which `load_ui_no_warnings` fails.
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\ItemButtonTemplate.xml");
    // `UIPanelCloseButton`, which each popup's pass button inherits (`LootFrame.xml:364`).
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // the vote and item hovers
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
    s
}

/// The resolved rolls' links as `ui_loot_roll` builds them: quality colour, four-field `|Hitem:`.
const STAFF_LINK: &str = "|cffa335ee|Hitem:17182:0:0:0|h[Staff of Jordan]|h|r";
const SWORD_LINK: &str = "|cffffffff|Hitem:25:0:0:0|h[Worn Shortsword]|h|r";

/// Roll 7 resolved Epic and bind-on-pickup, roll 8 resolved Common, roll 9 with no template yet.
fn rolls() -> LootRollsState {
    LootRollsState {
        rolls: vec![
            LootRollEntry {
                roll_id: 7,
                name: Some("Staff of Jordan".into()),
                texture: Some("Interface\\Icons\\INV_Staff_12".into()),
                quantity: 1,
                quality: Some(4), // Epic -> purple
                bind_on_pickup: true,
                time_left_ms: 42_000,
                item_id: 17182,
                link: Some(STAFF_LINK.into()),
                random_property_id: 0,
            },
            LootRollEntry {
                roll_id: 8,
                name: Some("Worn Shortsword".into()),
                texture: Some("Interface\\Icons\\INV_Sword_04".into()),
                quantity: 1,
                quality: Some(1), // Common -> white
                bind_on_pickup: false,
                time_left_ms: 60_000,
                item_id: 25,
                link: Some(SWORD_LINK.into()),
                random_property_id: 0,
            },
            // No template yet: name, texture, quality and link (which embeds the name) are nil.
            LootRollEntry {
                roll_id: 9,
                name: None,
                texture: None,
                quantity: 1,
                quality: None,
                bind_on_pickup: false,
                time_left_ms: 55_000,
                item_id: 4306,
                link: None,
                random_property_id: 0,
            },
        ],
    }
}

/// The popups load with no loader warning of any kind and start hidden.
#[test]
fn shipped_group_loot_frame_loads_clean_and_starts_hidden() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\PartyMemberFrame.lua");
    // The popups come inside the whole loot window, so the frame count is only a floor.
    assert!(
        load_xml_no_warnings(&s, r"Interface\FrameXML\LootFrame.xml") > 4,
        "the loot window brought its frames"
    );

    for i in 1..=4 {
        let name = format!("GroupLootFrame{i}");
        assert!(
            !s.eval::<bool>(&format!("return {name}:IsVisible()"))
                .unwrap(),
            "{name} starts hidden"
        );
    }
}

/// `START_LOOT_ROLL` claims the first free popup (`LootFrame.lua:246-258`) and paints the roll:
/// name, quality colour, bind-on-pickup decoration and timer range; Need clicks through.
#[test]
fn start_loot_roll_claims_frames_in_order_and_paints_the_roll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(42_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert_eq!(s.eval::<i64>("return GroupLootFrame1.rollID").unwrap(), 7);
    // The timer's range is set before `Show()` (`LootFrame.lua:253`).
    assert_eq!(
        s.eval::<(f64, f64)>("return GroupLootFrame1Timer:GetMinMaxValues()")
            .unwrap(),
        (0.0, 42_000.0)
    );
    // Bind-on-pickup shows the gold decoration (`LootFrame.lua:263-266`).
    assert!(s
        .eval::<bool>("return GroupLootFrame1Decoration:IsShown()")
        .unwrap());

    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(8), ScriptValue::Int(60_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());
    assert_eq!(s.eval::<i64>("return GroupLootFrame2.rollID").unwrap(), 8);
    assert!(!s
        .eval::<bool>("return GroupLootFrame2Decoration:IsShown()")
        .unwrap());

    s.resolve();
    let quads = s.extract();
    assert_eq!(
        s.eval::<String>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        "Staff of Jordan"
    );
    let purple = text_color(&quads, "Staff of Jordan").expect("Staff of Jordan colour");
    assert!(
        (purple[0] - 0.64).abs() < 0.02 && (purple[1] - 0.21).abs() < 0.02,
        "Epic item text is purple, got {purple:?}"
    );
    let white = text_color(&quads, "Worn Shortsword").expect("Worn Shortsword colour");
    assert!(
        (white[0] - 1.0).abs() < 0.02 && (white[1] - 1.0).abs() < 0.02,
        "Common item text is white, got {white:?}"
    );

    // Found by name: the popups are `toplevel` (`LootFrame.xml:243`), so draw order is not
    // declaration order. Roll 7 is bind-on-pickup, so the vote waits for the confirm.
    let (nx, ny) = s
        .eval::<(f64, f64)>(
            "return (GroupLootFrame1RollButton:GetLeft() + GroupLootFrame1RollButton:GetRight()) \
             / 2, (GroupLootFrame1RollButton:GetBottom() + GroupLootFrame1RollButton:GetTop()) / 2",
        )
        .unwrap();
    let (nx, ny) = (nx as f32, ny as f32);
    assert_eq!(
        s.hit_test_name(nx, ny).as_deref(),
        Some("GroupLootFrame1RollButton"),
        "the click has to land on frame 1's Need button for the rest of this to mean anything"
    );
    s.mouse_button(nx, ny, "LeftButton", true);
    s.mouse_button(nx, ny, "LeftButton", false);
    assert!(
        s.take_loot_roll_votes().is_empty(),
        "a BoP Need must not reach the wire before the confirm"
    );
    assert_eq!(s.take_loot_roll_confirms(), vec![(7, 1)], "Need on roll 7");
}

/// `CONFIRM_LOOT_ROLL` opens the dialog carrying `(rollID, rollType)` (`UIParent.lua:517-523`);
/// its OK calls `ConfirmLootRoll` (`StaticPopup.lua:1375-1377`), which sends the withheld vote.
#[test]
fn the_bop_confirm_popup_lands_the_withheld_vote() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    // The app fires this after draining the confirm queue (`ui_loot_roll::drain_loot_rolls`).
    s.fire_event(
        "CONFIRM_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(1)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // `data2` is the rollType, so `OnAccept` can replay the click.
    assert!(
        s.eval::<bool>("return StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\") ~= nil")
            .unwrap(),
        "the confirm popup should be visible"
    );
    let (data, data2) = s
        .eval::<(i64, i64)>(
            "local d = StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\")\nreturn d.data, d.data2",
        )
        .unwrap();
    assert_eq!((data, data2), (7, 1), "rollID and rollType ride the dialog");

    // The 1.12 `LOOT_NO_DROP` and `OKAY` strings.
    let dialog = s
        .eval::<String>("return StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\"):GetName()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {dialog}Text:GetText()"))
            .unwrap(),
        "Looting this item will bind it to you."
    );
    assert_eq!(
        s.eval::<String>(&format!("return {dialog}Button1:GetText()"))
            .unwrap(),
        "Okay"
    );

    assert!(s.take_loot_roll_votes().is_empty());

    s.eval::<bool>(
        "local d = StaticPopup_FindVisible(\"CONFIRM_LOOT_ROLL\")\n\
         StaticPopupDialogs[\"CONFIRM_LOOT_ROLL\"].OnAccept(d.data, d.data2)\nreturn true",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_loot_roll_votes(),
        vec![(7, 1)],
        "OK on the confirm sends the Need the gate withheld"
    );
}

/// `CANCEL_LOOT_ROLL` hides the popup holding that rollID and no other (`LootFrame.lua:279-285`).
#[test]
fn cancel_loot_roll_hides_only_that_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(42_000)],
    );
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(8), ScriptValue::Int(60_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(7)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());

    // A rollID no popup holds is a no-op.
    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(7)]);
    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(999)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());

    s.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(8)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s
        .eval::<bool>("return GroupLootFrame2:IsVisible()")
        .unwrap());
}

/// A roll with its item template in flight opens without error, with no name and no icon.
#[test]
fn in_flight_roll_does_not_error_and_falls_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    s.set_loot_rolls(rolls());

    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(9), ScriptValue::Int(55_000)],
    );
    assert!(
        s.errors().is_empty(),
        "an in-flight roll (all-nil GetLootRollItemInfo) must not error: {:?}",
        s.errors()
    );
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert_eq!(s.eval::<i64>("return GroupLootFrame1.rollID").unwrap(), 9);
    // `SetText(nil)` (`LootFrame.lua:274`) leaves no text, not "nil".
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None,
        ""
    );
    assert!(!s
        .eval::<bool>("return GroupLootFrame1Decoration:IsShown()")
        .unwrap());

    // No placeholder icon: `GroupLootFrame_OnShow` passes the nil texture to `SetTexture`
    // (`LootFrame.lua:273`), which leaves the icon empty.
    s.resolve();
    let has_fallback_icon = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Misc_QuestionMark"))
    });
    assert!(
        !has_fallback_icon,
        "the reference paints no placeholder for an unresolved item"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stock popup paints only in `OnShow`, so a `START_LOOT_ROLL` that beats its roll to the model
/// stays blank. On a cache miss the reference fires it from the item-template arrival callback
/// (`0x61b460`, armed by `0x61b310`), and `feed_loot_rolls` holds it the same way.
#[test]
fn nothing_repaints_a_frame_that_opened_before_its_snapshot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);

    // The order the app must never produce: the event first, against a model with no such roll.
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(7), ScriptValue::Int(42_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None,
        "nothing to paint yet — this is the state B371 reported"
    );

    s.set_loot_rolls(rolls());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // No repaint: nothing re-enters `OnShow`, as in 1.12.
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None,
        "the name stays blank: the reference has no repaint path"
    );
    assert!(
        !s.eval::<bool>("return GroupLootFrame1Decoration:IsShown()")
            .unwrap(),
        "and so does the BoP decoration"
    );
    s.resolve();
    let quads = s.extract();
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Staff_12"))
        }),
        "and no icon arrives either — same missing repaint, same one cause"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<Option<String>>("return GroupLootFrame1Name:GetText()")
            .unwrap(),
        None
    );
}

/// `UIPARENT_MANAGED_FRAME_POSITIONS["GroupLootFrame1"]` (baseY 60, bottomEither 42, pet 42;
/// `UIParent.lua:1577`) stacks the popups over the bottom bars; the pass finds the frame by name.
#[test]
fn managed_positions_engage_for_the_bare_frame_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    // `GameTooltip.xml` brings `TOOLTIP_DEFAULT_COLOR`, which the dropdown lists read in their
    // OnLoad (`UIDropDownMenuTemplates.xml:186`).
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_group_loot(&s);

    let bottom = |s: &UiScript| s.eval::<f64>("return GroupLootFrame1:GetBottom()").unwrap();

    // No bars: the row's base, 60, which the XML anchor also gives.
    s.run("UIParent_ManageFramePositions()").unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 60.0, "baseY");

    // The pass reads `SHOW_MULTI_ACTIONBAR_1`/`_2`, not the bars (`UIParent.lua:1599-1606`).
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 102.0, "60 + bottomEither 42 — the row engaged");

    // The stance bar shows too. The pass touches its three shelf textures by name, unguarded
    // (`UIParent.lua:1705-1732`), so the stand-in bar needs stand-in textures.
    s.run(
        "local t = { Show = function() end, Hide = function() end } \
         ShapeshiftBarLeft, ShapeshiftBarMiddle, ShapeshiftBarRight = t, t, t",
    )
    .unwrap();
    s.run(
        "ShapeshiftBarFrame = ShapeshiftBarFrame or CreateFrame(\"Frame\", \"ShapeshiftBarFrame\") \
         ShapeshiftBarFrame:Show() UIParent_ManageFramePositions()",
    )
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 144.0, "60 + 42 + pet 42");

    s.run("ShapeshiftBarFrame:Hide() UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s), 102.0, "back to the multibar-only stack");
}

/// The roll icon's OnClick (`LootFrame.xml:353-361`): ctrl dresses up the item and shift posts
/// its link to an open chat box, both through `GetLootRollItemLink`; the icon never votes.
#[test]
fn ctrl_and_shift_on_the_roll_icon_preview_and_post_its_link() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    load_group_loot(&s);
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\DressUpFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit the chat menus build from
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    s.set_loot_rolls(rolls());

    // Roll 8 is not bind-on-pickup, so the dice below vote without a confirm.
    s.fire_event(
        "START_LOOT_ROLL",
        vec![ScriptValue::Int(8), ScriptValue::Int(60_000)],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let (x, y) = quad_center(&quads, "INV_Sword_04");

    // A plain click does nothing: the handler has only the ctrl and shift arms.
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        s.take_loot_roll_votes().is_empty() && s.take_loot_roll_confirms().is_empty(),
        "an unmodified icon click votes nothing"
    );

    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        SWORD_LINK,
        "the rolled item's full escaped link landed in the chat box"
    );

    s.set_modifiers(false, true, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(25)],
        "re-dress first, then try the rolled item on"
    );
    assert!(
        s.take_loot_roll_votes().is_empty() && s.take_loot_roll_confirms().is_empty(),
        "no modified icon click may vote"
    );

    let (nx, ny) = quad_center(&quads, "UI-GroupLoot-Dice-Up");
    s.mouse_button(nx, ny, "LeftButton", true);
    s.mouse_button(nx, ny, "LeftButton", false);
    assert_eq!(
        s.take_loot_roll_votes(),
        vec![(8, 1)],
        "the Need button still votes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Addons inherit `GroupLootFrameTemplate` by its 1.12 name and reach for its children; the icon
/// is `IconFrameIcon` because it is nested in the `IconFrame` button (`LootFrame.xml:314`).
#[test]
fn the_roll_template_carries_the_reference_name_and_the_parts_addons_reach_for() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    load_group_loot(&s);

    // `CreateFrame` raises on an unknown template, so this call is the check.
    s.run(r#"Probe = CreateFrame("Frame", "ProbeRoll", nil, "GroupLootFrameTemplate")"#)
        .expect("the reference-named template must resolve");

    for part in [
        "IconFrame",
        "IconFrameIcon",
        "Name",
        "Corner",
        "Decoration",
        "Timer",
    ] {
        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("ProbeRoll{part}") ~= nil"#))
                .unwrap(),
            "an addon inheriting the template must find ProbeRoll{part}"
        );
    }

    // The timer is a StatusBar (`LootFrame.lua:253` sets its range).
    s.run(r#"ProbeRollTimer:SetMinMaxValues(0, 60000)"#)
        .expect("the timer must answer StatusBar verbs");

    assert!(s
        .eval::<bool>(r#"return getglobal("BenillaGroupLootFrameTemplate") == nil"#)
        .unwrap());
}

/// The stock `GroupLootFrame_OnShow` reads `ITEM_QUALITY_COLORS[quality].r` with no fallback
/// (`LootFrame.lua:275-276`), so an unresolved or unknown roll must still answer a quality.
#[test]
fn the_stock_group_loot_frame_survives_an_in_flight_roll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::LOOT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    s.set_loot_rolls(rolls());

    // Roll 9 is in flight and roll 99 unknown: both answer the reference's miss tail, quality 1
    // (`0x4c31a3`).
    for roll in [9, 99] {
        s.run(&format!("GroupLootFrame_OpenNewFrame({roll}, 55000)"))
            .unwrap_or_else(|e| panic!("stock OpenNewFrame raised on roll {roll}: {e}"));
        assert!(
            s.errors().is_empty(),
            "stock GroupLootFrame_OnShow raised on roll {roll}: {:?}",
            s.errors()
        );
    }
    assert!(s
        .eval::<bool>("return GroupLootFrame1:IsVisible() and GroupLootFrame2:IsVisible()")
        .unwrap());

    // The name is painted Common, set at `LootFrame.lua:276`.
    let painted: (f64, f64, f64) = s
        .eval("local r, g, b = GroupLootFrame1Name:GetTextColor()\nreturn r, g, b")
        .unwrap();
    let common: (f64, f64, f64) = s
        .eval("local r, g, b = GetItemQualityColor(1)\nreturn r, g, b")
        .unwrap();
    assert!(
        (painted.0 - common.0).abs() < 1e-6
            && (painted.1 - common.1).abs() < 1e-6
            && (painted.2 - common.2).abs() < 1e-6,
        "the stock roll popup paints the miss tail's Common, got {painted:?} want {common:?}"
    );
}
