//! The delete-item confirm popup driver (UiPanels.xml's
//! `BenillaDeleteItemConfirmDriver`) — Lua wiring the Rust-side cursor tests can't reach: the
//! world-drop's `DELETE_ITEM_CONFIRM` showing the ref's `DELETE_ITEM` StaticPopup entry (decision
//! 0308 §3's engine) with the real `DELETE_ITEM`/`YES`/`NO` GlobalStrings, its Yes/No/ESC routing
//! to `DeleteCursorItem`/`ClearCursor`, and the entry's own `OnUpdate` auto-hide poll.
//!
//! Plus the `arg2 >= 3` fork the driver grew in 1743 (ref `UIParent.lua:344-352`): a RARE-or-better
//! payload raises `DELETE_GOOD_ITEM` instead, whose OKAY stays disabled until the player types
//! `DELETE_ITEM_CONFIRM_STRING` into its edit box. The two fixtures are real 1.12 rows (vmangos
//! `item_template`): **Tough Jerky** 117, quality 1 — the plain arm; **Flurry Axe** 871, quality 4 —
//! the typed arm.

use benilla_ui::script::{ContainerSlot, ContainerState, UiScript};

use super::test_ui::{
    bag_open, bag_slot_button, load_ui as load_xml, load_world_frame, world_click, BAG_UI,
};

/// A one-item, one-slot backpack holding `item_id`/`name` at `quality`, so the confirm text and
/// the wire's destroy count are exercisable end to end. Quality is what forks the driver, so it is
/// the fixture's only real parameter.
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

/// The popup engine and its driver, and nothing else: `UiPanels.xml` owns both `StaticPopup*` and
/// `BenillaDeleteItemConfirmDriver`, and every test but the repaint one below only ever asks about
/// the dialog. It carried `MerchantFrame.xml` + `Cooldown.xml` + `BagFrame.xml` until 1751 for one
/// reason — the repaint test used to count `BenillaBagFrame_Update` — and that test now brings the
/// reference's own bag stack itself ([`bag_setup`]), so the whole tail went with it.
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
    // The stock DELETE_GOOD_ITEM's OnHide hands focus back to `ChatFrameEditBox` (1960).
    load_xml(&s, r"Interface\FrameXML\UIMenu.xml");
    load_xml(&s, r"Interface\FrameXML\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TOOLTIP_DEFAULT_COLOR, read by the dropdown backdrop
    load_xml(&s, r"Interface\FrameXML\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, r"Interface\FrameXML\FloatingChatFrame.xml"); // declares ChatFrameEditBox

    // **The world frame, because the world drop is what raises this popup** (decision
    // 2089). Every test here used to click at `(-50, -50)` — off-screen, over nothing — which is
    // a world click only in a house where no `WorldFrame` is loaded. The real client's is
    // full-screen and mouse-enabled, and it swallowed every one of these clicks for three days
    // while all ten tests stayed green. `world_click` lands on the world now, and this is the
    // file that has to be there for there to be one.
    load_world_frame(&s);
    s.set_money(0);
    s
}

/// [`setup`] plus the reference's bag stack — the harness for the one test that has to watch a bag
/// WINDOW repaint. The windows are `ContainerFrame1..12` off the player's chain now (1751), so
/// this needs client data and the caller opens with `wow_data_or_skip!`.
fn bag_setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    // `ContainerFrame_Update` reads `MerchantFrame:IsShown()` on any slot the tooltip owns.
    // `BankFrame`'s purse is an ordinary `SmallMoneyFrameTemplate` off the chain's own
    // `MoneyFrame.xml` (1937/1962), so it is not a merchant dependency.
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_world_frame(&s); // [`setup`]'s reason, same fixture

    // …and the paper doll goes back where the reference keeps it. [`BAG_UI`] leaves
    // `CharacterFrame.xml` out on purpose (its own note: a window no bag test opens), so the
    // frame `PaperDollFrame.xml` declares has no hidden parent to sit inside and stands on screen
    // instead — it and its model child covering the world the drop has to be clicked on. The real
    // client never shows it there; hiding it restores its state, it does not dodge the fixture's.
    s.run("PaperDollFrame:Hide()")
        .expect("the paper doll the missing CharacterFrame would have hidden");
    s.set_money(0);
    s
}

/// Pick up the fixture item and click-carry it into the world — a completed LEFT CLICK (press +
/// release, both on the world frame; 0218's byte-verified trigger) fires the world-drop
/// `DELETE_ITEM_CONFIRM(name, quality)` the driver listens for.
fn pick_up_and_drop_in_world(s: &mut UiScript) {
    drop_in_world(s, 117, "Tough Jerky", 1);
}

/// The same world drop for an arbitrary fixture item — 1743's fork reads `arg2` (the quality), so
/// every test below picks the arm it wants by choosing the item.
fn drop_in_world(s: &mut UiScript, item_id: u32, name: &str, quality: u32) {
    s.set_container(0, Some(one_item_backpack(item_id, name, quality)));
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.cursor_item().is_some(), "fixture: the item is held");
    world_click(s);
    s.tick(0.01); // flush the queued DELETE_ITEM_CONFIRM into the driver's OnEvent
}

/// The popup shows with the real 1.12 `DELETE_ITEM`/`YES`/`NO` GlobalStrings text, and Yes runs
/// `DeleteCursorItem()` — the item leaves the cursor and queues its wire destroy.
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

    // Yes → StaticPopup_OnClick(dialog, 1) → the entry's OnAccept → DeleteCursorItem(): the
    // payload clears and the destroy queues (count 0 = the fixture's whole 5-stack was picked up).
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Yes hides the popup"
    );
    assert!(s.cursor_item().is_none(), "DeleteCursorItem cleared it");
    assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// No runs the entry's OnCancel (`ClearCursor()`) instead — the item is dropped, not destroyed (no
/// wire send) — and the bag REPAINTS off the clear's `ITEM_LOCK_CHANGED`, un-darkening the source
/// slot. The repaint is the 0218 stuck-darkening regression: the popup's No is a clear the bag
/// never clicked through, so only the event wiring (the container window's own ITEM_LOCK_CHANGED
/// registration) can reach it.
///
/// **What 1751 changed, and what it did not.** The repaint counted here used to be
/// `BenillaBagFrame_Update`; the window is the reference's `ContainerFrame` now, so the body is
/// its own `ContainerFrame_Update(frame)` (`ContainerFrame.lua:234`) off the player's chain. Two
/// consequences, both faithful: the window has to be OPEN (`ContainerFrame_OnEvent` gates the
/// repaint on `this:IsShown()` at l.39-42, where ours repainted a hidden window too), and the
/// darkening the repaint undoes is the reference's own `SetItemButtonDesaturated(button, locked)`
/// at l.245 — so the slot's greyscale flag is asserted directly here rather than only through the
/// repaint that clears it.
#[test]
fn delete_item_confirm_no_clears_without_destroying() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = bag_setup();
    s.set_container(0, Some(one_item_backpack(117, "Tough Jerky", 1)));
    s.run("ToggleBackpack()").unwrap();
    assert!(bag_open(&s, 0), "the backpack window is up");
    // Asked of the buttons' own GetID: `ContainerFrame_GenerateFrame` numbers a window's buttons
    // BACKWARDS, so slot 1 is the LAST `…Item<j>`, and index arithmetic would pin a coincidence.
    let slot1 = bag_slot_button(&s, 0, 1);

    pick_up_and_drop_in_world(&mut s);
    assert!(
        desaturated(&mut s, &slot1),
        "held on the cursor, the source slot draws dark (ref SetItemButtonDesaturated(_, locked))"
    );

    // Count repaints from here — the No-click path must trigger one via the event, not a click.
    // 5.0's vararg spelling: the implicit `arg` table forwarded with `unpack`, because `...` as a
    // VALUE is not in this VM's grammar — it is not in the 1.12 client's.
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

/// Is `button`'s icon drawn greyscale? The renderer-facing answer, off the quad stream, because
/// the engine publishes `Texture:SetDesaturated` with no getter to ask — and a *drawn* grey is
/// what "the slot is stuck dark" means to the player anyway.
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

/// ESC routes through the ref's `StaticPopup_EscapePressed` — DELETE_ITEM is `hideOnEscape`, so
/// its OnCancel runs with reason "clicked" exactly as a real No-click would. (`ToggleGameMenu`'s
/// existing unconditional `ClearCursor()` already empties the cursor before the popup branch, so
/// this mainly proves the popup itself closes.)
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

/// The reference's DELETE_ITEM `OnUpdate` auto-hide poll (StaticPopup.lua:669-673), now VERBATIM
/// as the registry entry's own OnUpdate: if the cursor empties by any OTHER path (not a popup
/// button — say, a same-slot click cancelling a fresh pickup) while the confirm is still showing,
/// the popup auto-hides on the next tick. The poll lives inside the DELETE_ITEM entry, so a
/// DIFFERENT visible dialog is untouched by cursor traffic.
#[test]
fn the_delete_entry_polls_itself_hidden_and_other_dialogs_are_untouched() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    pick_up_and_drop_in_world(&mut s);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    // The cursor empties via ClearCursor() directly (not a popup button) — the entry's own
    // OnUpdate poll hides the still-showing confirm on the next tick.
    s.run("ClearCursor()").unwrap();
    s.tick(0.01);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the DELETE_ITEM OnUpdate poll auto-hides once the cursor is empty"
    );

    // An unrelated registered dialog must NOT be auto-hidden by cursor traffic — only the
    // DELETE_ITEM entry polls, and its OnUpdate only runs while DELETE_ITEM is the shown `which`.
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

/// The RARE-or-better fork (ref `UIParent.lua:346-350`, `arg2 >= 3`): a quality-4 payload raises
/// `DELETE_GOOD_ITEM`, not `DELETE_ITEM` — with the second GlobalString, the edit box up and
/// focused, and OKAY **disabled** until the confirm word is typed.
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

/// The typed gate itself (ref `EditBoxOnTextChanged`, `StaticPopup.lua:718-723`): only the exact
/// `DELETE_ITEM_CONFIRM_STRING` enables OKAY, it is compared through `strupper` so lower case
/// passes, and backing away from the word disables it again. Then OKAY destroys.
#[test]
fn typing_the_confirm_word_enables_okay_and_untyping_it_disables_again() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    drop_in_world(&mut s, 871, "Flurry Axe", 4);

    // OKAY is enabled by the box's `OnTextChanged`, and that fire is deferred to the drain
    // — so what this answers is the button state the LAST DRAINED text produced,
    // which is the state a player ever sees. Ticking here rather than after each write keeps the
    // test reading as the sequence of edits it is about.
    let enabled = |s: &mut UiScript| {
        s.tick(0.0);
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap()
            == 1
    };

    s.run(r#"StaticPopup1EditBox:SetText("DELET")"#).unwrap();
    assert!(!enabled(&mut s), "a prefix of the word is not the word");

    // Typed character by character through the engine's own input path, lower case: the ref
    // compares through strupper, so this arms OKAY exactly as shouting it would.
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

    // OKAY now does what the plain arm's does: DeleteCursorItem().
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "OKAY hides the popup"
    );
    assert!(s.cursor_item().is_none(), "DeleteCursorItem cleared it");
    assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Enter in the box is the ref's `EditBoxOnEnterPressed` (`StaticPopup.lua:712-717`): it destroys
/// only while OKAY is enabled, so a reflexive Enter over a half-typed word does nothing at all.
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
    s.tick(0.0); // the write only marks the box — the drain is what enables OKAY (1831)
    assert!(s.key_input("ENTER"));
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Enter with OKAY enabled destroys and hides"
    );
    assert!(s.cursor_item().is_none());
    assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// NO on the typed variant is the plain one's cancel — `ClearCursor()`, no wire send — and the
/// entry's `OnHide` empties the box so the next raise opens blank (the ref's own reason: a word
/// left over from last time would be armed before the player read the dialog).
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

    // Raise it again: blank box, OKAY disabled — the armed state did not survive.
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

/// ESC inside the typed confirm's focused box is SWALLOWED, the reference's own behaviour (1960):
/// the stock `DELETE_GOOD_ITEM` entry defines no `EditBoxOnEscapePressed`, so the box's
/// `StaticPopup_EditBoxOnEscapePressed` finds nothing to call and the dialog stays up with the
/// item still on the cursor — the player answers with No. (ADD_FRIEND and its kin hide on ESC
/// because their entries say so; this one does not.) Our engine used to fall through to a
/// cancel here, which the earlier form of this test pinned.
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

/// The plain arm keeps its own edit box HIDDEN — the fork is not cosmetic, and a common-quality
/// destroy must not grow a field to type into. The control for every test above.
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

/// **The world drop, under the WHOLE shipped interface** — B380's own test.
///
/// The nine above build a fixture out of the files the popup needs, which is what let B380 hide:
/// a fixture is a subset of the manifest, and the file that broke this one was a file no fixture
/// had a reason to name. This one takes the manifest entire — the same load the player gets,
/// world frame and action bars and chat and all — picks an item up off the bag and clicks the
/// ground. It is the slowest test in the file and the only one that could not have been fooled.
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
    // …and Yes destroys it, through the stock popup's own OnAccept.
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert!(s.cursor_item().is_none(), "the cursor is empty after Yes");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
