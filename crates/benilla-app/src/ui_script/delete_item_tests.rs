//! The delete confirm: the world drop's `DELETE_ITEM_CONFIRM` raises the stock `DELETE_ITEM`
//! popup, or `DELETE_GOOD_ITEM` for quality 3 and up (`UIParent.lua:344-352`). The fixtures are
//! real 1.12 items: Tough Jerky (117, quality 1) and Flurry Axe (871, quality 4).

use benilla_ui::script::{ContainerSlot, ContainerState, UiScript};

use super::test_ui::{
    bag_open, bag_slot_button, load_ui as load_xml, load_world_frame, world_click, BAG_UI,
};

/// A backpack holding a 5-stack of `item_id` at `quality`, the parameter the popup forks on.
fn one_item_backpack(item_id: u32, name: &str, quality: u32) -> ContainerState {
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(quality),
            item_id,
            link: Some(format!("|cffffffff|Hitem:{item_id}|h[{name}]|h|r")),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots,
    }
}

/// The stock popup stack and the world frame the drop is clicked on.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    // The stock `DELETE_GOOD_ITEM`'s `OnHide` hands focus back to `ChatFrameEditBox`.
    load_xml(&s, r"Interface\FrameXML\UIMenu.xml");
    load_xml(&s, r"Interface\FrameXML\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TOOLTIP_DEFAULT_COLOR, for dropdowns
    load_xml(&s, r"Interface\FrameXML\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, r"Interface\FrameXML\FloatingChatFrame.xml"); // declares ChatFrameEditBox

    // The drop is a click on the full-screen world frame, so it must be loaded.
    load_world_frame(&s);
    s.set_money(0);
    s
}

/// The stock bag windows (`BAG_UI`) and the world frame, for the test that watches a bag repaint.
fn bag_setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    // `ContainerFrame_Update` reads `MerchantFrame:IsShown()` on any slot the tooltip owns.
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_world_frame(&s); // the drop target

    // `BAG_UI` leaves out `CharacterFrame.xml`, so `PaperDollFrame` has no hidden parent and would
    // cover the world; the real client never shows it here.
    s.run("PaperDollFrame:Hide()")
        .expect("the paper doll the missing CharacterFrame would have hidden");
    s.set_money(0);
    s
}

/// Drops Tough Jerky on the world: a full left click, press and release both on the world frame,
/// fires `DELETE_ITEM_CONFIRM(name, quality)`.
fn pick_up_and_drop_in_world(s: &mut UiScript) {
    drop_in_world(s, 117, "Tough Jerky", 1);
}

/// The world drop for any fixture item; its quality (`arg2`) picks the popup.
fn drop_in_world(s: &mut UiScript, item_id: u32, name: &str, quality: u32) {
    s.set_container(0, Some(one_item_backpack(item_id, name, quality)));
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.cursor_item().is_some(), "fixture: the item is held");
    world_click(s);
    s.tick(0.01); // flush the queued DELETE_ITEM_CONFIRM into UIParent's OnEvent
}

#[test]
fn delete_item_confirm_shows_the_real_strings_and_yes_deletes() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    pick_up_and_drop_in_world(&mut s);

    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the confirm popup shows on DELETE_ITEM_CONFIRM"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to destroy Tough Jerky?",
        "the real GlobalStrings DELETE_ITEM text, formatted with the item name"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button1:GetText()")
            .unwrap(),
        "Yes"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button2:GetText()")
            .unwrap(),
        "No"
    );

    // Yes runs `OnAccept`, `DeleteCursorItem()`; a destroy count of 0 is the whole stack.
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Yes hides the popup"
    );
    assert!(s.cursor_item().is_none(), "DeleteCursorItem cleared it");
    assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// No runs `OnCancel` (`ClearCursor()`) and destroys nothing; only the clear's `ITEM_LOCK_CHANGED`
/// can repaint the open bag, un-darkening the slot (`ContainerFrame.lua:39-42`, `:246`).
#[test]
fn delete_item_confirm_no_clears_without_destroying() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = bag_setup();
    s.set_container(0, Some(one_item_backpack(117, "Tough Jerky", 1)));
    s.run("ToggleBackpack()").unwrap();
    assert!(bag_open(&s, 0), "the backpack window is up");
    // `ContainerFrame_GenerateFrame` numbers buttons backwards (`ContainerFrame.lua:426-428`).
    let slot1 = bag_slot_button(&s, 0, 1);

    pick_up_and_drop_in_world(&mut s);
    assert!(
        desaturated(&mut s, &slot1),
        "held on the cursor, the source slot draws dark (ref SetItemButtonDesaturated(_, locked))"
    );

    // Count repaints. The 1.12 client's Lua 5.0 has no `...` as a value: forward `arg` by `unpack`.
    s.run(
        "repaints = 0\n\
         local real = ContainerFrame_Update\n\
         ContainerFrame_Update = function(...) repaints = repaints + 1; \
         return real(unpack(arg, 1, arg.n)) end",
    )
    .unwrap();

    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "No hides the popup"
    );
    assert!(s.cursor_item().is_none(), "ClearCursor cleared it");
    assert!(s.take_container_destroys().is_empty(), "No never destroys");
    s.tick(0.01); // ITEM_LOCK_CHANGED fires from the pending queue at the next tick
    assert!(
        s.eval::<i64>("return repaints").unwrap() >= 1,
        "the clear's ITEM_LOCK_CHANGED repaints the bag (the stuck-darkened slot, 0218)"
    );
    assert!(
        !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap(),
        "the source slot reads unlocked again"
    );
    assert!(
        !desaturated(&mut s, &slot1),
        "…and the repaint un-darkened it — the 0218 stuck-dark slot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Whether `button`'s icon draws greyscale, off the quad stream: `SetDesaturated` has no getter.
fn desaturated(s: &mut UiScript, button: &str) -> bool {
    s.resolve();
    s.extract()
        .into_iter()
        .filter(|q| s.quad_owner_name(q.target).as_deref() == Some(button))
        .any(|q| match &q.content {
            benilla_ui::script::QuadContent::Texture {
                path: Some(p),
                desaturated,
                ..
            } => p.contains("INV_Misc_Food_16") && *desaturated,
            _ => false,
        })
}

/// ESC runs stock `StaticPopup_EscapePressed`: `DELETE_ITEM` is `hideOnEscape`, so its `OnCancel`
/// runs as No would (`StaticPopup.lua:1879-1893`).
#[test]
fn escape_closes_the_delete_confirm_popup() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    pick_up_and_drop_in_world(&mut s);

    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "ESC closes the confirm popup"
    );
    assert!(s.cursor_item().is_none());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stock `DELETE_ITEM`'s `OnUpdate` hides it once the cursor empties by any path
/// (`StaticPopup.lua:669-673`); the poll is the entry's own.
#[test]
fn the_delete_entry_polls_itself_hidden_and_other_dialogs_are_untouched() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    pick_up_and_drop_in_world(&mut s);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    s.run("ClearCursor()").unwrap();
    s.tick(0.01);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the DELETE_ITEM OnUpdate poll auto-hides once the cursor is empty"
    );

    s.run(
        r#"StaticPopupDialogs["TEST_UNRELATED"] = { text = "unrelated?", button1 = "Yes",
           button2 = "No", timeout = 0, whileDead = 1 }
           StaticPopup_Show("TEST_UNRELATED")"#,
    )
    .unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.fire_event("CURSOR_UPDATE", vec![]);
    s.tick(0.01);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "an unrelated dialog is never touched by the delete-confirm poll"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Quality 3 and up raises `DELETE_GOOD_ITEM` instead (`UIParent.lua:346-350`).
#[test]
fn a_rare_payload_raises_the_typed_confirm_with_okay_disabled() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    drop_in_world(&mut s, 871, "Flurry Axe", 4);

    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "DELETE_GOOD_ITEM",
        "quality 4 forks to the typed-confirmation variant"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to destroy Flurry Axe?\n\nType \"DELETE\" into the field to confirm.",
        "the real GlobalStrings DELETE_GOOD_ITEM text, formatted with the item name"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1EditBox:IsShown()")
            .unwrap(),
        "hasEditBox raises the narrow box"
    );
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("StaticPopup1EditBox"),
        "the entry's OnShow focuses the box, so the player can type straight away"
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        0,
        "OKAY starts disabled — nothing has been typed yet"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `EditBoxOnTextChanged` compares the `strupper` of the text with `DELETE_ITEM_CONFIRM_STRING`
/// (`StaticPopup.lua:718-723`).
#[test]
fn typing_the_confirm_word_enables_okay_and_untyping_it_disables_again() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    drop_in_world(&mut s, 871, "Flurry Axe", 4);

    // `OnTextChanged` fires at the drain, so this ticks before reading the button.
    let enabled = |s: &mut UiScript| {
        s.tick(0.0);
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap()
            == 1
    };

    s.run(r#"StaticPopup1EditBox:SetText("DELET")"#).unwrap();
    assert!(!enabled(&mut s), "a prefix of the word is not the word");

    // Typed key by key in lower case, through the engine's input path.
    s.run(r#"StaticPopup1EditBox:SetText("")"#).unwrap();
    for c in ["d", "e", "l", "e", "t", "e"] {
        assert!(s.char_input(c), "the focused box takes the keystroke");
    }
    assert_eq!(
        s.eval::<String>("return StaticPopup1EditBox:GetText()")
            .unwrap(),
        "delete"
    );
    assert!(
        enabled(&mut s),
        "the ref compares through strupper, so lower case passes"
    );

    s.run(r#"StaticPopup1EditBox:SetText("deletex")"#).unwrap();
    assert!(!enabled(&mut s), "one char past the word disables again");

    s.run(r#"StaticPopup1EditBox:SetText("DELETE")"#).unwrap();
    assert!(enabled(&mut s));

    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "OKAY hides the popup"
    );
    assert!(s.cursor_item().is_none(), "DeleteCursorItem cleared it");
    assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `EditBoxOnEnterPressed` destroys only while OKAY is enabled (`StaticPopup.lua:712-717`).
#[test]
fn enter_in_the_box_destroys_only_once_okay_is_enabled() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    drop_in_world(&mut s, 871, "Flurry Axe", 4);

    s.run(r#"StaticPopup1EditBox:SetText("DEL")"#).unwrap();
    assert!(s.key_input("ENTER"), "the focused box consumes ENTER");
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Enter with OKAY disabled is inert — the dialog stays up"
    );
    assert!(s.cursor_item().is_some(), "and the item is still held");
    assert!(s.take_container_destroys().is_empty());

    s.run(r#"StaticPopup1EditBox:SetText("DELETE")"#).unwrap();
    s.tick(0.0); // the write only marks the box; the drain enables OKAY
    assert!(s.key_input("ENTER"));
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Enter with OKAY enabled destroys and hides"
    );
    assert!(s.cursor_item().is_none());
    assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stock `OnHide` empties the box, so the next raise opens blank and disarmed.
#[test]
fn no_on_the_typed_confirm_clears_and_leaves_the_box_empty_for_next_time() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    drop_in_world(&mut s, 871, "Flurry Axe", 4);
    s.run(r#"StaticPopup1EditBox:SetText("DELETE")"#).unwrap();

    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.cursor_item().is_none(), "ClearCursor cleared it");
    assert!(s.take_container_destroys().is_empty(), "No never destroys");
    assert_eq!(
        s.eval::<String>("return StaticPopup1EditBox:GetText()")
            .unwrap(),
        "",
        "OnHide empties the box"
    );

    drop_in_world(&mut s, 871, "Flurry Axe", 4);
    assert_eq!(
        s.eval::<String>("return StaticPopup1EditBox:GetText()")
            .unwrap(),
        ""
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        0,
        "and OKAY is disabled again"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `DELETE_GOOD_ITEM` has no `EditBoxOnEscapePressed`, so ESC in its box does nothing
/// (`StaticPopup.lua:1817-1822`).
#[test]
fn escape_in_the_typed_confirms_box_is_swallowed_as_the_reference_leaves_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    drop_in_world(&mut s, 871, "Flurry Axe", 4);
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("StaticPopup1EditBox")
    );

    assert!(s.key_input("ESCAPE"), "the focused box consumes ESCAPE");
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the dialog stays: the entry names no escape handler"
    );
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("StaticPopup1EditBox"),
        "and the box keeps focus"
    );
    assert!(s.cursor_item().is_some(), "the item is still on the cursor");
    assert!(s.take_container_destroys().is_empty(), "ESC never destroys");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_plain_arm_shows_no_edit_box() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    pick_up_and_drop_in_world(&mut s); // Tough Jerky, quality 1

    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "DELETE_ITEM"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1EditBox:IsShown()")
            .unwrap(),
        "no hasEditBox on the plain entry"
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        1,
        "and YES is live immediately"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The world drop under the whole shipped manifest: a fixture is a subset, and can miss the file
/// that breaks the drop.
#[test]
fn the_world_drop_survives_the_whole_shipped_interface() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack(117, "Tough Jerky", 1)));
    s.resolve();

    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.cursor_item().is_some(), "fixture: the item is held");
    world_click(&mut s);
    s.tick(0.01);

    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "clicking the ground with an item held raises the destroy confirm"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to destroy Tough Jerky?"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert!(s.cursor_item().is_none(), "the cursor is empty after Yes");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
