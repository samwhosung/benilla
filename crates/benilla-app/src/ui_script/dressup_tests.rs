//! The dressing room and the chat-link insert through the real click paths: CTRL opens the room
//! wearing the item, SHIFT posts its link into an open chat box, and at a bag slot SHIFT with chat
//! closed still opens the stack splitter. The stock handlers read `this`, so these drive the mouse.

use benilla_ui::script::{
    ContainerSlot, ContainerState, DressUpIntent, InvSlotView, InventorySlots, SoundRequest,
    UiScript, UnitState,
};

use super::test_ui::{bag_slot_button, click, load_ui as load_xml, BAG_UI, CHARACTER_UI};

/// A real 1.12 item link: Tough Jerky (117), quality white.
const JERKY_LINK: &str = "|cffffffff|Hitem:117|h[Tough Jerky]|h|r";

/// The room's own files, in manifest order; both loaders end with these.
const ROOM_UI: &[&str] = &[
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
    "Interface\\FrameXML\\UnitPopup.xml",
    "Interface\\FrameXML\\ItemRef.xml",
    "ScrollTemplates.xml", // our scroll kits
    "Interface\\FrameXML\\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\MerchantFrame.xml",
    "Interface\\FrameXML\\StackSplitFrame.xml",
    "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\ChatFrame.xml",
    "Interface\\FrameXML\\UIPanelTemplates.lua",
    "Interface\\FrameXML\\UIPanelTemplates.xml",
    // After the panel kit: the room's Close and Reset buttons inherit its templates at load.
    "Interface\\FrameXML\\DressUpFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\FloatingChatFrame.xml",
];

/// The room with no bag window, for the click sites that never touch a container.
fn load_room(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\Cooldown.xml",
    ] {
        load_xml(s, file);
    }
    for file in ROOM_UI {
        load_xml(s, file);
    }
}

/// The room over the stock bag windows; `UIParent.xml` leads because every window parents to it
/// and `BAG_UI` has no line for it.
fn load_room_with_bags(s: &UiScript) {
    load_xml(s, r"Interface\FrameXML\UIParent.xml");
    for file in BAG_UI {
        load_xml(s, file);
    }
    for file in ROOM_UI {
        load_xml(s, file);
    }
}

/// The character window on its paper doll, with the room and a chat box beside it. The player
/// has race and class, which stock `PaperDollFrame_SetLevel` formats unguarded on every show
/// (`PaperDollFrame.lua:100-104`).
fn shown_paper_doll() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, file);
    }
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UIPanelTemplates.lua",
        "Interface\\FrameXML\\UIPanelTemplates.xml",
        // After the panel kit its buttons inherit from.
        "Interface\\FrameXML\\DressUpFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        super::test_ui::load_ui_strict(&s, file);
    }
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 60,
            race: Some("Human".into()),
            race_file: Some("Human".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..UnitState::default()
        }),
    );
    s
}

/// Opens a backpack holding 5 Tough Jerky in slot 1 and returns the name of that slot's button,
/// asked of the buttons: the stock windows are recycled and number their buttons backwards.
fn backpack_with_jerky(s: &mut UiScript) -> String {
    s.set_money(0);
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            bar_placeable: true,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: Some(JERKY_LINK.into()),
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    s.resolve();
    bag_slot_button(s, 0, 1)
}

/// Stock `DressUpItemLink(GetContainerItemLink(...))` (`ContainerFrame.lua:565-566`). `Dress`
/// must precede `TryOn`, or the room shows the player's own gear instead of the item.
#[test]
fn ctrl_click_on_a_bag_item_opens_the_room_wearing_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_room_with_bags(&s);
    let btn = backpack_with_jerky(&mut s);
    assert!(!s.eval::<bool>("return DressUpFrame:IsVisible()").unwrap());

    // Modifiers go in before the click, as the app's input pass sends them.
    s.set_modifiers(false, true, false);
    click(&mut s, &btn, "LeftButton");
    s.set_modifiers(false, false, false);

    assert!(
        s.eval::<bool>("return DressUpFrame:IsVisible()").unwrap(),
        "ctrl-click opened the dressing room"
    );
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(117)],
        "re-dress first, then try the clicked item on"
    );
    assert!(
        s.cursor_item().is_none(),
        "the ctrl fork never picks the item up"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock shift arm (`ContainerFrame.lua:567-578`): the link with chat open, else the stack
/// splitter, which calls back through the `this.SplitStack` closure.
#[test]
fn shift_click_posts_the_link_with_chat_open_and_splits_with_it_closed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_room_with_bags(&s);
    let btn = backpack_with_jerky(&mut s);

    s.set_modifiers(true, false, false);
    click(&mut s, &btn, "LeftButton");
    s.set_modifiers(false, false, false);
    assert!(
        s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "with chat closed, shift-click still opens the stack splitter"
    );
    s.run("StackSplitFrame:Hide()").unwrap();

    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    click(&mut s, &btn, "LeftButton");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        JERKY_LINK,
        "the item's full escaped link landed in the chat box"
    );
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "with chat open the splitter must NOT open"
    );
    assert!(
        s.cursor_item().is_none(),
        "neither shift fork picks the stack up"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `SetItemRef`, which a chat frame's `OnHyperlinkClick` calls: ctrl previews the item
/// (`ItemRef.lua:49-50`).
#[test]
fn ctrl_clicking_a_chat_link_previews_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_room(&s);

    s.set_modifiers(false, true, false);
    s.run(&format!(
        "SetItemRef(\"item:117\", \"{JERKY_LINK}\", \"LeftButton\")"
    ))
    .unwrap();
    s.set_modifiers(false, false, false);

    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(117)],
        "a chat link's id reached TryOn through DressUpItemLink's own gsub"
    );
    assert!(
        s.eval::<bool>("return DressUpFrame:IsVisible()").unwrap(),
        "and the room opened"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `PaperDollItemSlotButton_OnClick` (`PaperDollFrame.lua:647-662`) reads
/// `GetInventoryItemLink("player", slot)` for both the preview and the link.
#[test]
fn the_paper_doll_slots_preview_and_post_what_you_wear() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_paper_doll();
    let mut slots = InventorySlots::default();
    slots[1] = Some(InvSlotView {
        item_id: 1234,
        icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
        count: 1,
        quality: 2,
        name: Some("Test Helm".into()),
        link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
        equip_slots: vec![1],
        ..Default::default()
    });
    s.set_inventory_slots(slots);
    assert_eq!(
        s.eval::<String>("return GetInventoryItemLink(\"player\", 1)")
            .unwrap(),
        "|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r"
    );
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    let _ = s.take_dressup_intents();
    s.resolve();

    s.set_modifiers(false, true, false);
    click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(1234)]
    );

    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r"
    );
    assert!(
        s.cursor_item().is_none(),
        "a modified doll click never picks the worn item up"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An unresolved item has no link, which stock `PaperDollFrame.lua:653` inserts unguarded:
/// `EditBox:Insert(nil)` is a no-op, as the reference reads its argument with `lua_tostring`
/// (`0x7984b0`).
#[test]
fn shift_clicking_an_unresolved_slot_posts_nothing_and_never_raises() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_paper_doll();
    // Occupied but unresolved: an item id with no name or quality, so no link.
    let mut slots = InventorySlots::default();
    slots[1] = Some(InvSlotView {
        item_id: 1234,
        count: 1,
        ..Default::default()
    });
    s.set_inventory_slots(slots);
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    let _ = s.take_dressup_intents();
    s.resolve();
    assert!(s.focus_editbox("ChatFrameEditBox"));

    s.set_modifiers(true, false, false);
    click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "",
        "nothing posts while the template is in flight"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Ctrl is inert too: `DressUpItemLink` returns on a nil link (`DressUpFrame.lua:11-13`).
    s.set_modifiers(false, true, false);
    click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.set_modifiers(false, false, false);
    assert!(
        s.take_dressup_intents().is_empty(),
        "no try-on for an item we cannot even name yet"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The rotate buttons step 0.03 per `OnClick` (`UIParent.lua:1428`) and register both mouse edges
/// (`DressUpFrame.xml:220`), so one tap turns 0.06.
#[test]
fn reset_re_dresses_close_empties_and_the_arrows_spin_the_pane() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_room(&s);
    s.run("ShowUIPanel(DressUpFrame)").unwrap();
    let _ = s.take_sounds();
    let _ = s.take_dressup_intents();
    s.resolve();

    // `Model_OnLoad` seeds the default facing.
    assert!(
        (s.model_pane_facing("DressUpModel") - 0.61).abs() < 1e-6,
        "ref UIParent.lua:1422"
    );

    s.run("DressUpFrameResetButton:Click()").unwrap();
    assert_eq!(s.take_dressup_intents(), vec![DressUpIntent::Dress]);
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("gsTitleOptionOK".into())],
        "Reset plays the ref's own kit"
    );

    let before = s.model_pane_facing("DressUpModel");
    let (x, y) = s
        .eval::<(f32, f32)>(
            "return (DressUpModelRotateLeftButton:GetLeft() \
                     + DressUpModelRotateLeftButton:GetRight()) / 2, \
                    (DressUpModelRotateLeftButton:GetTop() \
                     + DressUpModelRotateLeftButton:GetBottom()) / 2",
        )
        .unwrap();
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        (s.model_pane_facing("DressUpModel") - (before - 0.06)).abs() < 1e-5,
        "a tap fires OnClick twice: {} → {}",
        before,
        s.model_pane_facing("DressUpModel")
    );

    s.run("HideUIPanel(DressUpFrame)").unwrap();
    // The app empties the booth off the frame's visibility; the stock `OnHide` queues nothing.
    assert!(!s.frame_visible("DressUpFrame"), "the room is hidden");
    assert!(s.take_dressup_intents().is_empty());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
