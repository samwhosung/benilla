//! The dressing room + the chat-link insert, driven through the real click paths (decisions
//! 1059/1060).
//!
//! What these pin is the **fork**, at the sites a player actually clicks: CTRL opens the room
//! wearing the item, SHIFT posts its link into an open chat edit box — and, at a bag slot, SHIFT
//! with chat *closed* still opens the stack splitter it always did (the reference's own `else`,
//! and the regression these two features could most easily cause).
//!
//! **The bag-slot fork is the REFERENCE's own code since 1751.** Both forks used to run through
//! benilla's `BenillaBagSlot_OnClick`; the bag windows are the reference's recycled
//! `ContainerFrame1..12` now and the handler is its `ContainerFrameItemButton_OnClick(button,
//! ignoreModifiers)` — which takes the MOUSE button as its first argument and reads the frame from
//! `this`, so a test cannot call it directly any more. These drive the mouse instead
//! ([`super::test_ui::click`]), which is the stronger test regardless: it puts the template's own
//! `RegisterForClicks("LeftButtonUp", "RightButtonUp")` and script wiring under test too.
//!
//! **The doll slots went the same way.** `CharacterFrame.xml` and `PaperDollFrame.xml` are the
//! reference's own too now, so `BenillaPaperDollSlot_OnClick` is gone and the fork is stock
//! `PaperDollItemSlotButton_OnClick(button, ignoreModifiers)` (`PaperDollFrame.lua:647-662`) —
//! which reads `this` exactly as the bag handler does. Those tests drive the mouse as well, over a
//! window that is genuinely OPEN, which is the only state a player can click a doll slot from.

use benilla_ui::script::{
    ContainerSlot, ContainerState, DressUpIntent, InvSlotView, InventorySlots, SoundRequest,
    UiScript, UnitState,
};

use super::test_ui::{bag_slot_button, click, load_ui as load_xml, BAG_UI, CHARACTER_UI};

/// The jerky stack every bag fixture here uses — a real 1.12 link, quality white.
const JERKY_LINK: &str = "|cffffffff|Hitem:117|h[Tough Jerky]|h|r";

/// The room's own files, past whatever bag interface the caller wants — both loaders below end
/// with these, in manifest order.
const ROOM_UI: &[&str] = &[
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
    "Interface\\FrameXML\\UnitPopup.xml",
    "Interface\\FrameXML\\ItemRef.xml",
    "ScrollTemplates.xml", // our scroll kit + the placeholder icon
    "Interface\\FrameXML\\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\MerchantFrame.xml",
    "Interface\\FrameXML\\StackSplitFrame.xml",
    "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\ChatFrame.xml",
    "Interface\\FrameXML\\UIPanelTemplates.lua",
    "Interface\\FrameXML\\UIPanelTemplates.xml",
    // The reference's own room (1969): its Close/Reset buttons inherit the panel kit's templates,
    // which resolve at load — so after it, as the manifest has it.
    "Interface\\FrameXML\\DressUpFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\FloatingChatFrame.xml",
];

/// The shipped files the dressing room's click sites need, in manifest order — **with no bag
/// window**, for the sites that never touch a container (a chat link, the room's own controls).
/// Deliberately install-free: those tests run on a bare checkout, which [`load_room_with_bags`]
/// cannot.
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

/// [`load_room`] over the REFERENCE's bag windows (1751). `BAG_UI` is the ordered set a test needs
/// before it can open one and it already carries the fonts, panels, tooltip, item-button template
/// and bag bar the room wants, so only the room's own files follow it. `UIParent.xml` leads because
/// it inherits nothing and every window below parents to it — `BAG_UI` has no line for it.
///
/// **Needs client data**: `BAG_UI` names a chain entry, so its callers open with
/// `wow_data_or_skip!`.
fn load_room_with_bags(s: &UiScript) {
    load_xml(s, r"Interface\FrameXML\UIParent.xml");
    for file in BAG_UI {
        load_xml(s, file);
    }
    for file in ROOM_UI {
        load_xml(s, file);
    }
}

/// The character window **open on its paper doll**, with the room and a chat edit box beside it —
/// the two files [`CHARACTER_UI`] does not already carry. (`UIDropDownMenu.xml`,
/// `Interface\FrameXML\UIMenu.xml` and `UnitPopup.xml` are in both lists; loading either twice
/// would redeclare its frames.)
///
/// The player behind it carries **both** halves of the race and class pairs: `UnitRace`/`UnitClass`
/// answer `(localized, file)` or `nil, nil` — the binding `zip`s them — and stock
/// `PaperDollFrame_SetLevel` formats all three into `CharacterLevelText` unguarded
/// (`PaperDollFrame.lua:100-104`) on every show.
///
/// **Needs client data**, like [`load_room_with_bags`].
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
        // The reference's room (1969), after the panel kit its buttons inherit from.
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

/// A backpack holding a 5-stack of Tough Jerky in slot 1, opened; returns the NAME of the button
/// showing that slot.
///
/// **Asked, never derived** (1751). The window is one of the reference's recycled
/// `ContainerFrame1..12` — which one depends on what else is open — and its buttons are numbered
/// BACKWARDS (`ContainerFrame_GenerateFrame`: `index = size - j + 1`, so `…Item1` is the bag's last
/// slot), so [`bag_slot_button`] finds the button whose own `GetID()` is the slot. The old body
/// scanned `BenillaBagSlot<N>` for a `.slot` field and handed back a centre; neither those frames
/// nor that field exist, and the centre is [`click`]'s business now.
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

/// CTRL + left-click on a bag item opens the dressing room and puts that item on — the ref's
/// `DressUpItemLink(GetContainerItemLink(...))` (ContainerFrame.lua:565-566). The two intents are
/// ordered: `Dress` (the window was closed, so it re-dresses in the player's own gear first) then
/// `TryOn` — reversed, the room would show the player's own gear and not the clicked item.
///
/// Since 1751 that arm is the reference's OWN `ContainerFrameItemButton_OnClick` rather than a
/// transcription of it, so the cited line numbers are now the live code's.
#[test]
fn ctrl_click_on_a_bag_item_opens_the_room_wearing_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_room_with_bags(&s);
    let btn = backpack_with_jerky(&mut s);
    assert!(!s.eval::<bool>("return DressUpFrame:IsVisible()").unwrap());

    // The modifier is pushed BEFORE the mouse, exactly as the app's input pass does it: the click
    // handler's fork reads the state as of the click.
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

/// SHIFT + left-click posts the item's link into the chat edit box **when it is open**, and still
/// opens the stack splitter when it is not — the reference's own if/else (ContainerFrame.lua:
/// 567-577). Both halves in one test because it is the *fork* that matters: the split was the
/// behaviour that already shipped, and the insert must not have eaten it.
///
/// Since 1751 the fork is the reference's own body and the splitter it opens is still ours
/// (`OpenStackSplitFrame`, StackSplit.xml): the reference reaches it through the `this.SplitStack`
/// closure `ContainerFrameItemButton_OnLoad` installs, so what this now also covers is that seam
/// holding — a stack split confirmed from a bag slot still calls `SplitContainerItem` with the
/// window's own bag id and the button's own slot id.
#[test]
fn shift_click_posts_the_link_with_chat_open_and_splits_with_it_closed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_room_with_bags(&s);
    let btn = backpack_with_jerky(&mut s);

    // Chat closed → the splitter, exactly as before.
    s.set_modifiers(true, false, false);
    click(&mut s, &btn, "LeftButton");
    s.set_modifiers(false, false, false);
    assert!(
        s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "with chat closed, shift-click still opens the stack splitter"
    );
    s.run("StackSplitFrame:Hide()").unwrap();

    // Chat open → the link, and no splitter.
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

/// A link already IN chat: ctrl-clicking it previews that item (ref ItemRef.lua:49-50), shift
/// -clicking it re-inserts it (the branch that already shipped). The router is `SetItemRef`, which
/// is what the message frame's `OnHyperlinkClick` calls with the full markup.
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
    // The plain click still shows the link tooltip rather than the room (no modifier).
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The paper doll's own slots (`PaperDollFrame.lua:647-662`): ctrl previews what you are wearing,
/// shift posts its link. Both read the unit-keyed `GetInventoryItemLink`, the binding this arc
/// added — so this also pins that the getter answers for `"player"`.
///
/// Since 1751 that handler is the reference's own `PaperDollItemSlotButton_OnClick(button,
/// ignoreModifiers)`, which reads the frame from `this` — so this drives the mouse over a window
/// that is really open, exactly as the bag tests above do.
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

    // CTRL on the head slot → the room, wearing the helm.
    s.set_modifiers(false, true, false);
    click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![DressUpIntent::Dress, DressUpIntent::TryOn(1234)]
    );

    // SHIFT with chat open → the link; the item is never picked up either way.
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

/// A slot whose item template has not answered yet has **no link** — a state the real client never
/// has (its item cache is synchronous), so the reference carries no guard: stock
/// `PaperDollItemSlotButton_OnClick` shift-inserts `GetInventoryItemLink(...)` straight
/// (`PaperDollFrame.lua:653`). Shift-clicking such a slot must post nothing and raise nothing.
///
/// That is now the ENGINE's promise rather than a transcription's guard: `EditBox:Insert(nil)` is a
/// no-op because the reference reads the argument through `lua_tostring`, and this
/// test is what holds the stock line above harmless over an in-flight item.
#[test]
fn shift_clicking_an_unresolved_slot_posts_nothing_and_never_raises() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_paper_doll();
    // The slot is occupied but unresolved: an item id with no name/quality yet, so no link.
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

    // And ctrl on the same unresolved slot is inert too (DressUpItemLink's own nil guard).
    s.set_modifiers(false, true, false);
    click(&mut s, "CharacterHeadSlot", "LeftButton");
    s.set_modifiers(false, false, false);
    assert!(
        s.take_dressup_intents().is_empty(),
        "no try-on for an item we cannot even name yet"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The window's own controls: Reset re-dresses (and clicks), closing it empties the booth, and the
/// rotate buttons move the pane's yaw by the reference's own ±0.03 per OnClick — which fires on
/// BOTH mouse edges, so one tap is 0.06.
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

    // The pane's OnLoad (`Model_OnLoad`) seeded the ref's default facing on the widget itself.
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

    // One tap of rotate-left: OnClick on press AND release, −0.03 each.
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
    // Closing the window is what empties the booth, and the app reads it off the frame — the
    // stock file's OnHide plays its sound and queues nothing (1969).
    assert!(!s.frame_visible("DressUpFrame"), "the room is hidden");
    assert!(s.take_dressup_intents().is_empty());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
