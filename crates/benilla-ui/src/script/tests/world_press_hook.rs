//! The world input hook `0x492b50`, which the UI root's press handler `0x7662c0` runs ahead of
//! every frame when no frame holds a press (`0x76635c`-`0x766376`): a right press with a payload
//! on the cursor or a gift wrap pending clears the cursor with `ClearCursor(1, 1)` and is
//! consumed, so no frame sees it, and the world does not either.

use super::common::script;
use crate::script::cursor::{two_item_bag, CursorMacro, CursorPayload, CursorSpell};
use crate::script::tick::heard;
use crate::script::UiScript;

/// A button at (100..300, 100..200) counting what reaches it.
fn button_script() -> UiScript {
    let mut s = script();
    s.run(
        r#"
        DOWNS, UPS, CLICKS = 0, 0, 0
        local b = CreateFrame("Button", "B")
        b:SetPoint("BOTTOMLEFT", 100, 100); b:SetWidth(200); b:SetHeight(100)
        b:EnableMouse(true)
        b:RegisterForClicks("LeftButtonUp", "RightButtonDown", "RightButtonUp")
        b:SetScript("OnMouseDown", function() DOWNS = DOWNS + 1 end)
        b:SetScript("OnMouseUp", function() UPS = UPS + 1 end)
        b:SetScript("OnClick", function() CLICKS = CLICKS + 1 end)
        "#,
    )
    .unwrap();
    s.resolve();
    s
}

fn counts(s: &UiScript) -> (i64, i64, i64) {
    s.eval::<(i64, i64, i64)>("return DOWNS, UPS, CLICKS")
        .unwrap()
}

fn spell() -> CursorPayload {
    CursorPayload::Spell(CursorSpell {
        passive: false,
        spell_id: 133,
        texture: None,
    })
}

/// Over a frame or over nothing, a right press with any payload drops it and goes no further: the
/// frame gets no `OnMouseDown`, no `OnClick` and, at the release, no `OnMouseUp`; the events of
/// the clear have run by the time the press returns.
#[test]
fn a_right_press_with_a_payload_drops_it_and_reaches_no_frame() {
    let mut s = button_script();
    heard::listen(&s, &["CURSOR_UPDATE"], None);
    s.set_cursor_for_test(spell());
    assert!(
        s.mouse_button(150.0, 150.0, "RightButton", true),
        "consumed"
    );
    assert!(s.cursor_payload().is_none(), "the payload dropped");
    assert_eq!(heard::take(&s), ["CURSOR_UPDATE"], "fired in the press");
    assert_eq!(counts(&s), (0, 0, 0), "no frame saw the press");
    s.mouse_button(150.0, 150.0, "RightButton", false);
    assert_eq!(counts(&s), (0, 0, 0), "nor its release");

    // Over nothing there is no frame to consume it: the hook does.
    s.set_cursor_for_test(spell());
    assert!(s.mouse_button(900.0, 700.0, "RightButton", true));
    assert!(s.cursor_payload().is_none());
    assert!(
        !s.mouse_button(900.0, 700.0, "RightButton", true),
        "with nothing held, the same press is nobody's"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `0x492b50` tests each payload word in turn (`0x492b62`-`0x492ba3`): an item, coins, a spell,
/// a pet action, a macro, a stabled pet and a preview all count.
#[test]
fn every_payload_kind_is_the_hooks() {
    let mut s = button_script();
    s.set_container(0, Some(two_item_bag()));
    for held in ["item", "macro", "spell"] {
        match held {
            "item" => s.run("PickupContainerItem(0, 1)").unwrap(),
            "macro" => s.set_cursor_for_test(CursorPayload::Macro(CursorMacro {
                index: 1,
                texture: None,
            })),
            _ => s.set_cursor_for_test(spell()),
        }
        assert!(s.cursor_payload().is_some(), "{held} is held");
        assert!(s.mouse_button(150.0, 150.0, "RightButton", true), "{held}");
        assert!(s.cursor_payload().is_none(), "{held} dropped");
    }
    assert_eq!(counts(&s), (0, 0, 0));
    assert!(
        !s.item_locked(117) && !s.item_locked(118),
        "a held item is unlocked by the clear"
    );
}

/// A pending gift wrap is held cursor state too (`0x492ba5`, `0x5edf10`): the press cancels it.
#[test]
fn a_right_press_cancels_a_pending_gift_wrap() {
    let mut s = button_script();
    s.set_container(0, Some(two_item_bag()));
    s.arm_gift_wrap(0, 1);
    assert!(s.gift_wrap_armed().is_some());
    assert!(s.mouse_button(150.0, 150.0, "RightButton", true));
    assert_eq!(s.gift_wrap_armed(), None);
    assert_eq!(counts(&s), (0, 0, 0));
}

/// With nothing held the press is an ordinary one: the frame under it gets it.
#[test]
fn a_right_press_with_nothing_held_reaches_the_frame() {
    let mut s = button_script();
    assert!(s.mouse_button(150.0, 150.0, "RightButton", true));
    s.mouse_button(150.0, 150.0, "RightButton", false);
    assert_eq!(
        counts(&s),
        (1, 1, 2),
        "down, up, the down click and the up click"
    );
}

/// The hook runs only when no frame holds a press (`0x766361`-`0x766369`): a right press chorded
/// onto a held left press goes to the frame that holds it, and the payload stays. And only the
/// right button (`0x492b5c`) is the hook's.
#[test]
fn the_hook_waits_for_a_free_mouse_and_a_right_press() {
    let mut s = button_script();
    s.mouse_button(150.0, 150.0, "LeftButton", true);
    s.set_cursor_for_test(spell());
    s.mouse_button(150.0, 150.0, "RightButton", true);
    assert!(
        s.cursor_payload().is_some(),
        "the chord's press is the frame's"
    );
    assert_eq!(counts(&s).0, 2, "both presses reached the held frame");
    s.mouse_button(150.0, 150.0, "RightButton", false);
    s.mouse_button(150.0, 150.0, "LeftButton", false);

    s.set_cursor_for_test(spell());
    s.mouse_button(150.0, 150.0, "LeftButton", true);
    assert!(s.cursor_payload().is_some(), "a left press never drops it");
    assert_eq!(counts(&s).0, 3);
}
