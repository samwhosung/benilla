//! The host half of text input: [`keymap`] maps chords per OS, [`clipboard`] holds the OS
//! pasteboard, and [`feed_key`] routes a key into the engine's [`EditBoxState`], which owns the
//! editing itself. The glue-screen fields call [`feed_key`]; the FrameXML EditBoxes have their own
//! dispatcher, which fires Lua handlers, over the same keymap and clipboard.

use std::ffi::c_void;

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_ui::widget::EditBoxState;

pub(crate) mod clipboard;
pub(crate) mod keymap;

pub(crate) use clipboard::{wayland_display, HostClipboard};
pub(crate) use keymap::{chord, Chord, Mods};

/// Adds the process-wide OS pasteboard; [`feed_key`] is called from each screen's own input pass.
pub(crate) struct TextInputPlugin;

impl Plugin for TextInputPlugin {
    fn build(&self, app: &mut App) {
        // Held for the whole run: on X11 dropping the handle clears the clipboard. `NonSend`: no
        // backend is `Sync`, and NSPasteboard is main-thread-only.
        app.init_non_send_resource::<HostClipboard>();
    }
}

/// The modifier snapshot for this frame, read once and handed to every [`feed_key`] call.
pub(crate) fn mods_now(keys: &ButtonInput<KeyCode>) -> Mods {
    Mods {
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        sup: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}

/// Which characters a field accepts, typed or pasted; the box's own `numeric` flag covers digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CharFilter {
    /// Everything printable (the login boxes).
    Any,
    /// ASCII letters only (the character-name box).
    Letters,
}

impl CharFilter {
    fn allows(self, c: char) -> bool {
        match self {
            CharFilter::Any => true,
            CharFilter::Letters => c.is_ascii_alphabetic(),
        }
    }

    /// Keep only the characters this filter allows.
    fn keep(self, text: &str) -> String {
        match self {
            CharFilter::Any => text.chars().filter(|c| !c.is_control()).collect(),
            CharFilter::Letters => text.chars().filter(|&c| self.allows(c)).collect(),
        }
    }
}

/// What [`feed_key`] did with a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldKey {
    /// The field handled it; the screen must not also act on this key.
    Consumed,
    /// Not a text-editing key; the screen decides, as the FrameXML box hands ENTER, ESCAPE and TAB
    /// to its scripts.
    Passthrough,
}

/// Feed one key press to `field`. `wl_display` comes from [`wayland_display`], `None` off Wayland.
pub(crate) fn feed_key(
    field: &mut EditBoxState,
    ev: &KeyboardInput,
    mods: Mods,
    clipboard: &mut HostClipboard,
    wl_display: Option<*mut c_void>,
    filter: CharFilter,
) -> FieldKey {
    if ev.state != ButtonState::Pressed {
        return FieldKey::Passthrough;
    }
    // The box-event keys are always the screen's, as in `script::editbox::key_input`.
    if matches!(
        ev.key_code,
        KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape | KeyCode::Tab
    ) {
        return FieldKey::Passthrough;
    }
    if let Some(chord) = chord(ev.key_code, mods, cfg!(target_os = "macos")) {
        match chord {
            Chord::Edit(action) => {
                field.apply(action);
            }
            Chord::Copy => {
                if let Some(text) = field.selected_text() {
                    clipboard.write(wl_display, &text);
                }
            }
            Chord::Cut => {
                if let Some(text) = field.cut_selection() {
                    clipboard.write(wl_display, &text);
                }
            }
            Chord::Paste => {
                if let Some(text) = clipboard.read(wl_display) {
                    field.paste(&filter.keep(&text));
                }
            }
        }
        return FieldKey::Consumed;
    }
    // A command-modified char never types, but Ctrl+Alt (AltGr) does; the FrameXML feed and the
    // chord table use the same guard.
    if !(mods.sup || (mods.ctrl && !mods.alt)) {
        if let Some(text) = &ev.text {
            // C0 control characters are dropped, as in the box's own `char_input`.
            let printable = filter.keep(text);
            if !printable.is_empty() {
                field.insert(&printable);
                return FieldKey::Consumed;
            }
        }
    }
    FieldKey::Passthrough
}

/// A fresh single-line field with `max_letters` (0 = unlimited) and optional password masking.
pub(crate) fn field(max_letters: usize, password: bool) -> EditBoxState {
    EditBoxState {
        max_letters,
        password,
        ..Default::default()
    }
}

/// Advance the caret blink and report whether it is drawn: the box's own blink (`E+0x370`,
/// `E+0x374`, default 0.5 s). An unfocused field is hidden and does not accumulate.
pub(crate) fn tick_caret(field: &mut EditBoxState, focused: bool, dt: f32) -> bool {
    if !focused {
        return false;
    }
    if field.blink_period > 0.0 {
        field.blink_accum += dt;
        if field.blink_accum > field.blink_period {
            field.caret_shown = !field.caret_shown;
            field.blink_accum = 0.0;
        }
    }
    field.caret_shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_filter_applies_to_pasted_text() {
        assert_eq!(CharFilter::Letters.keep("Bob123"), "Bob");
        assert_eq!(CharFilter::Letters.keep("a b\tc\n"), "abc");
        assert_eq!(CharFilter::Letters.keep("123"), "");
    }

    #[test]
    fn any_filter_keeps_printables_and_drops_controls() {
        assert_eq!(CharFilter::Any.keep("pass word!"), "pass word!");
        assert_eq!(CharFilter::Any.keep("one\ntwo\r"), "onetwo");
    }

    #[test]
    fn a_filtered_paste_still_obeys_max_letters() {
        let mut f = field(4, false);
        f.paste(&CharFilter::Letters.keep("ab12cdef"));
        assert_eq!(f.text, "abcd");
    }

    /// Copying from a password field yields the mask, not the secret.
    #[test]
    fn a_password_field_masks_display_and_copies() {
        let mut f = field(0, true);
        f.insert("hunter2");
        assert_eq!(f.text, "hunter2");
        assert_eq!(f.display(), "*******");
        f.highlight_text(0, -1);
        assert_eq!(f.selected_text().as_deref(), Some("*******"));
    }
}
