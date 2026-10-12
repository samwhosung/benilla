//! The host's keyboard feed ([`super::input::feed_keys`]) against the reference's four dispatchers:
//! key-down (`0x765f10`), char (`0x765df0`), key-up (`0x765fd0`) and auto-repeat (`0x766070`).

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::bag_tests::open_backpack_with_a_five_stack;
use super::input::feed_keys;
use super::UiKeyboardCapture;
use crate::bindings::LayoutNames;
use crate::textinput::{keymap::Mods, HostClipboard};

/// One keyboard event as winit hands it over.
fn ev(code: KeyCode, state: ButtonState, repeat: bool, text: Option<&str>) -> KeyboardInput {
    KeyboardInput {
        key_code: code,
        logical_key: Key::Character(text.unwrap_or("?").into()),
        state,
        text: text.map(Into::into),
        repeat,
        window: Entity::PLACEHOLDER,
    }
}

fn down(code: KeyCode, text: Option<&str>) -> KeyboardInput {
    ev(code, ButtonState::Pressed, false, text)
}

fn repeat(code: KeyCode) -> KeyboardInput {
    ev(code, ButtonState::Pressed, true, None)
}

fn up(code: KeyCode) -> KeyboardInput {
    ev(code, ButtonState::Released, false, None)
}

/// A harness for one script: the capture the bindings read, a clipboard holding `paste`.
struct Feed {
    capture: UiKeyboardCapture,
    clipboard: HostClipboard,
    mods: Mods,
}

impl Feed {
    fn new(paste: &str) -> Self {
        Feed {
            capture: UiKeyboardCapture::default(),
            clipboard: HostClipboard::holding(paste),
            mods: Mods {
                shift: false,
                ctrl: false,
                alt: false,
                sup: false,
            },
        }
    }

    fn send(&mut self, script: &mut UiScript, events: &[KeyboardInput]) {
        script.set_alt_down(self.mods.alt);
        feed_keys(
            script,
            &mut self.capture,
            events.iter(),
            self.mods,
            &LayoutNames::default(),
            &mut self.clipboard,
            None,
        );
    }
}

/// A chat-like box in alt-arrow mode at DIALOG, focused, over the real split dialog: the arrows
/// are the box's to decline (`0x77b18e`-`0x77b1c4`), so the dialog steps with them, and holding
/// RIGHT steps it once (`0x766070` hands the repeat to the dialog's stub).
#[test]
fn the_split_dialog_steps_with_the_arrows_under_a_focused_chat_box() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.run(
        r#"
        ChatLike = CreateFrame("EditBox", "ChatLike")
        ChatLike:SetAutoFocus(false)
        ChatLike:SetAltArrowKeyMode(1)
        ChatLike:SetFrameStrata("DIALOG")
        ChatLike:SetFrameLevel(StackSplitFrame:GetFrameLevel() + 1)
        ChatLike:SetFocus()
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 1);

    let mut feed = Feed::new("");
    feed.send(&mut s, &[down(KeyCode::ArrowRight, None)]);
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        2,
        "the dialog took the arrow the box declined"
    );
    assert!(feed.capture.consumed.contains(&KeyCode::ArrowRight));
    feed.send(
        &mut s,
        &[repeat(KeyCode::ArrowRight), repeat(KeyCode::ArrowRight)],
    );
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        2,
        "holding RIGHT steps once"
    );
    feed.send(&mut s, &[up(KeyCode::ArrowRight)]);
    feed.send(&mut s, &[down(KeyCode::ArrowRight, None)]);
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 3);
    assert_eq!(
        s.eval::<String>("return ChatLike:GetText()").unwrap(),
        "",
        "and the box saw nothing"
    );

    // With ALT the box keeps its arrows.
    feed.mods.alt = true;
    feed.send(&mut s, &[down(KeyCode::ArrowRight, None)]);
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 3);
    assert!(s.take_errors().is_empty());
}

/// The paste chord with a keyboard frame above a focused box: the frame took the key-down, and the paste
/// is the box's own key-down arm (`0x77b160`), which the walk never reached.
#[test]
fn a_frame_that_took_the_key_keeps_the_paste_from_the_box() {
    let mut s = UiScript::new().unwrap();
    s.run(
        r#"
        got = {}
        box = CreateFrame("EditBox", "PasteBox")
        box:SetAutoFocus(false)
        box:SetFrameStrata("DIALOG")
        box:SetFocus()
        map = CreateFrame("Frame", "PasteMap")
        map:SetFrameStrata("FULLSCREEN_DIALOG")
        map:EnableKeyboard(true)
        map:SetScript("OnKeyDown", function() table.insert(got, arg1) end)
    "#,
    )
    .unwrap();
    let mut feed = Feed::new("pasted");
    // The paste chord of the platform: Cmd+V on a Mac, Ctrl+V elsewhere.
    feed.mods.ctrl = !cfg!(target_os = "macos");
    feed.mods.sup = cfg!(target_os = "macos");
    feed.send(&mut s, &[down(KeyCode::KeyV, Some("\u{16}"))]);
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "");
    assert_eq!(
        s.eval::<String>("return table.concat(got, ',')").unwrap(),
        "V"
    );

    // The map closes: the walk reaches the box, which pastes.
    s.run("map:Hide()").unwrap();
    feed.send(&mut s, &[down(KeyCode::KeyV, Some("\u{16}"))]);
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "pasted");
    assert!(s.take_errors().is_empty());
}

/// The modifiers go through the walk as key-downs named `SHIFT`, `CTRL` and `ALT` (`0x76b7d0`'s
/// table), a code the table does not name as `UNKNOWN`, each with its key-up.
#[test]
fn modifiers_and_unnamed_keys_walk_as_keys_with_their_ups() {
    let mut s = UiScript::new().unwrap();
    s.run(
        r#"
        log = {}
        f = CreateFrame("Frame", "ModWatch")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyDown", function() table.insert(log, "v" .. arg1) end)
        f:SetScript("OnKeyUp", function() table.insert(log, "^" .. arg1) end)
    "#,
    )
    .unwrap();
    let mut feed = Feed::new("");
    for code in [
        KeyCode::ShiftLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::Fn,
    ] {
        feed.send(&mut s, &[down(code, None), up(code)]);
    }
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "vSHIFT,^SHIFT,vCTRL,^CTRL,vALT,^ALT,vUNKNOWN,^UNKNOWN"
    );
    assert!(feed.capture.consumed.contains(&KeyCode::ShiftLeft));
    assert!(s.take_errors().is_empty());
}
