//! **The one text-input law, host half.** Every text field in the client — the FrameXML EditBoxes
//! (chat, `/who`, the StaticPopup) and the three glue-screen fields (login account/password, the
//! character name, the delete confirmation) — resolves its keystrokes here.
//!
//! There are three parts, and the split is deliberate:
//!
//! - [`keymap`] — *which chord means what*, per OS. The one place Ctrl+V vs Cmd+V is decided.
//! - [`clipboard`] — *the OS pasteboard*, one held handle per process, per-platform backend.
//! - [`feed_key`] (here) — the glue between them and the **engine's** byte-verified box law
//!   ([`EditBoxState`]). It owns no editing semantics of its own.
//!
//! ## Why this module exists
//!
//! The editing law was reachable only through the Lua UI runtime, so the glue screens — which have
//! no Lua VM — each hand-rolled a three-case imitation: append a printable char, Backspace, Tab.
//! No caret movement, no selection, no Ctrl+A, and no clipboard at all. Worse, they matched on
//! winit's `logical_key`, which is `Character("v")` for Ctrl+V, so **pasting into the login box
//! typed a literal `v` into the password**. Four fields, four different laws, three of them wrong.
//!
//! Now the law is [`EditBoxState`]'s (pure, no Lua) and the *routing* is this
//! module's, so a field gets the whole thing by owning an `EditBoxState` and calling [`feed_key`].
//! The FrameXML path keeps its own dispatcher because it must also fire Lua handlers and route
//! focus through the widget arena, but it reads the same [`keymap`] and the same [`clipboard`].

use std::ffi::c_void;

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_ui::widget::EditBoxState;

pub(crate) mod clipboard;
pub(crate) mod keymap;

pub(crate) use clipboard::{wayland_display, HostClipboard};
pub(crate) use keymap::{chord, Chord, Mods};

/// Adds the process-wide OS pasteboard. Nothing else here is a system: [`feed_key`] is a plain
/// function each screen calls from its own input pass, because every screen already owns its focus
/// model (the login form's two-field enum, the dialog's single box) and inverting that into a
/// component-driven focus would be churn for no gain.
pub(crate) struct TextInputPlugin;

impl Plugin for TextInputPlugin {
    fn build(&self, app: &mut App) {
        // Held for the whole run: on X11 dropping the handle *is* clearing the clipboard.
        // `NonSend` — no backend is `Sync`, NSPasteboard is main-thread-only.
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

/// Which characters a field accepts. The box law's own `numeric` flag covers digits-only; this
/// covers the one other rule the client has — a character name is letters (the create screen's
/// `is_ascii_alphabetic` guard). It is applied to **pasted** text as well as typed, which the old
/// hand-rolled screens could not do at all, having no paste.
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
    /// The field handled it — an edit, a caret move, or a clipboard operation. The screen must not
    /// also act on this key.
    Consumed,
    /// Not a text-editing key. The screen decides (ENTER submits, ESCAPE cancels, TAB cycles
    /// focus) — those are the screen's semantics, not the field's, exactly as the FrameXML box
    /// hands ENTER/ESCAPE/TAB to its own scripts.
    Passthrough,
}

/// Feed one key press to `field`. The whole shared law in one call: the per-OS chord table decides
/// what the chord means, [`EditBoxState`] executes it, and the clipboard trio resolves against the
/// held host pasteboard.
///
/// `wl_display` comes from [`wayland_display`] — `None` off Wayland.
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
    // The three box-event keys are the screen's, always — even though a chord table entry could
    // claim them. This mirrors the FrameXML split (`script::editbox::key_input`).
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
                    // Filter before the box sees it, so a name box can't be pasted full of digits.
                    field.paste(&filter.keep(&text));
                }
            }
        }
        return FieldKey::Consumed;
    }
    // Plain character input. A command-modified char never types (Cmd/Ctrl+L must not insert "l"),
    // but Ctrl+Alt passes: that is AltGr, the plane European layouts type real characters with —
    // the same guard the FrameXML feed uses, and the reason the chord table excludes AltGr too.
    if !(mods.sup || (mods.ctrl && !mods.alt)) {
        if let Some(text) = &ev.text {
            // C0 control characters are consumed-but-inert, as in the box's own `char_input`.
            let printable = filter.keep(text);
            if !printable.is_empty() {
                field.insert(&printable);
                return FieldKey::Consumed;
            }
        }
    }
    FieldKey::Passthrough
}

/// A fresh single-line field with `max_letters` (0 = unlimited) and optional password masking —
/// the glue screens' constructor, so they never hand-assemble an [`EditBoxState`] and quietly miss
/// a flag.
pub(crate) fn field(max_letters: usize, password: bool) -> EditBoxState {
    EditBoxState {
        max_letters,
        password,
        ..Default::default()
    }
}

/// Advance the caret blink and report whether it is currently drawn — the box's own blink law
/// (`E+0x370`/`E+0x374`, ctor default 0.5 s), so a glue caret and a chat caret blink identically.
/// An unfocused field always reports "hidden" without accumulating.
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

    /// The name box takes letters only — and the filter runs on **pasted** text too, which is the
    /// case the old hand-rolled screens could not express at all (they had no paste). Regression
    /// for.
    #[test]
    fn letters_filter_applies_to_pasted_text() {
        assert_eq!(CharFilter::Letters.keep("Bob123"), "Bob");
        assert_eq!(CharFilter::Letters.keep("a b\tc\n"), "abc");
        assert_eq!(CharFilter::Letters.keep("123"), "");
    }

    /// `Any` keeps everything printable but still drops control characters — a pasted newline must
    /// never reach a single-line box as a literal.
    #[test]
    fn any_filter_keeps_printables_and_drops_controls() {
        assert_eq!(CharFilter::Any.keep("pass word!"), "pass word!");
        assert_eq!(CharFilter::Any.keep("one\ntwo\r"), "onetwo");
    }

    /// A letters-only paste still honours the box's own cap, because the filter feeds
    /// `EditBoxState::paste`, which enforces `max_letters` like any other insert.
    #[test]
    fn a_filtered_paste_still_obeys_max_letters() {
        let mut f = field(4, false);
        f.paste(&CharFilter::Letters.keep("ab12cdef"));
        assert_eq!(f.text, "abcd");
    }

    /// A password field masks its display but never its buffer — the login password box relies on
    /// this, and on `selected_text` yielding the mask rather than the secret.
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
