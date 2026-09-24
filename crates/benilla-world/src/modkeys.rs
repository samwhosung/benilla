//! The modifier keys: the dev-overlay chord every instrument is bound on, and the stuck-modifier
//! reconciliation beneath it.
//!
//! A macOS shortcut that grabs the keyboard without de-focusing the window (the ⇧⌘5 screenshot
//! overlay) swallows the modifiers' release events: no focus loss arrives, and winit synthesizes
//! modifier events only from `flagsChanged`, which the grab withholds. Shift or Cmd then stays
//! held and every bare-key binding goes dead. Each frame after Bevy's input collection, any
//! modifier the OS's live state (`+[NSEvent modifierFlags]`) reports up is released, both sides of
//! its family. Release only: synthesized presses could race in-flight release events.

use bevy::prelude::*;

/// How the dev plane is written on screen; every surface that names the chord reads this.
pub const DEV_CHORD: &str = "Ctrl+Shift";

/// Whether the dev-overlay chord, [`DEV_CHORD`] + `key`, just fired. Exactly Ctrl and Shift, either
/// side: Alt and Super block it, since AltGr is Ctrl+Alt and AltGr+Shift+key typed into chat must
/// not fire. Of the reference's 152 default bindings only `CTRL-SHIFT-TAB` and
/// `CTRL-SHIFT-PAGEDOWN` use this plane, and no letter does. Not gated on
/// `ui_script::UiKeyboardCapture`: a chord is never typed text.
///
/// Deviation: the reference (`0x4b7990`) retries a binding miss once without the leftmost
/// modifier, so `CTRL-SHIFT-P` would reach `SHIFT-P` (`TOGGLECHARACTER3`, the pet paper doll); in
/// a run with dev affordances `bindings::BindingDispatch::resolve` skips that retry on this plane,
/// because a dev chord must open nothing in game. An exact `CTRL-SHIFT-` binding still dispatches.
pub fn dev_chord(keys: &ButtonInput<KeyCode>, key: KeyCode) -> bool {
    dev_plane(keys) && keys.just_pressed(key)
}

/// The modifier half of [`dev_chord`].
fn dev_plane(keys: &ButtonInput<KeyCode>) -> bool {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let blocked = keys.any_pressed([
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    ctrl && shift && !blocked
}

/// Keys a synthetic input source (the probe's `WOW_PROBE_KEY` taps) holds this frame, written by
/// whatever synthesizes input on every platform: the reconciler's hardware poll reads them as up
/// and would release them a frame after the press.
#[derive(Resource, Default)]
pub struct SyntheticHold(pub Vec<KeyCode>);

/// Whether Bevy holds `code` down with no synthetic source holding it: stuck, if the OS says up.
#[cfg(any(target_os = "macos", test))]
fn is_stuck(code: KeyCode, pressed: &ButtonInput<KeyCode>, hold: &SyntheticHold) -> bool {
    pressed.pressed(code) && !hold.0.contains(&code)
}

pub struct ModKeysPlugin;

impl Plugin for ModKeysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SyntheticHold>();
        #[cfg(target_os = "macos")]
        app.add_systems(
            PreUpdate,
            mac::reconcile_modifiers.after(bevy::input::InputSystems),
        );
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use bevy::ecs::system::NonSendMarker;
    use bevy::input::keyboard::Key;
    use bevy::prelude::*;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags};

    /// Release any modifier key Bevy holds pressed that the OS's live flag state says is up.
    /// `NonSendMarker` pins the system to the main thread, which AppKit requires.
    pub(super) fn reconcile_modifiers(
        _main_thread: NonSendMarker,
        mut codes: ResMut<ButtonInput<KeyCode>>,
        mut logical: ResMut<ButtonInput<Key>>,
        hold: Res<super::SyntheticHold>,
    ) {
        let flags = unsafe { NSEvent::modifierFlags_class() };
        let families = [
            (
                NSEventModifierFlags::NSEventModifierFlagShift,
                [KeyCode::ShiftLeft, KeyCode::ShiftRight],
                Key::Shift,
            ),
            (
                NSEventModifierFlags::NSEventModifierFlagControl,
                [KeyCode::ControlLeft, KeyCode::ControlRight],
                Key::Control,
            ),
            (
                NSEventModifierFlags::NSEventModifierFlagOption,
                [KeyCode::AltLeft, KeyCode::AltRight],
                Key::Alt,
            ),
            (
                NSEventModifierFlags::NSEventModifierFlagCommand,
                [KeyCode::SuperLeft, KeyCode::SuperRight],
                Key::Super,
            ),
        ];
        for (flag, keys, key) in families {
            if flags.contains(flag) {
                continue;
            }
            // Guarded so an in-sync frame never marks the resources changed; a synthetic hold is
            // never stuck, since the hardware poll cannot see it.
            let mut synthetic = false;
            for code in keys {
                if super::is_stuck(code, &codes, &hold) {
                    warn!("modkeys: releasing stuck {code:?} (OS reports it up)");
                    codes.release(code);
                }
                synthetic |= hold.0.contains(&code);
            }
            if !synthetic && logical.pressed(key.clone()) {
                logical.release(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonInput;
    use bevy::prelude::KeyCode;

    fn keys(held: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut k = ButtonInput::default();
        for &c in held {
            k.press(c);
        }
        k
    }

    /// The plane fires on its two modifiers, either side of each.
    #[test]
    fn ctrl_shift_is_the_plane() {
        for pair in [
            [KeyCode::ControlLeft, KeyCode::ShiftRight],
            [KeyCode::ControlRight, KeyCode::ShiftLeft],
        ] {
            assert!(dev_plane(&keys(&pair)), "{pair:?} fires");
        }
    }

    /// AltGr is Ctrl+Alt, so AltGr+Shift+key is typed text, never the chord.
    #[test]
    fn a_lone_or_extra_modifier_is_not_the_chord() {
        for held in [
            vec![KeyCode::ControlLeft],
            vec![KeyCode::ShiftLeft],
            vec![KeyCode::ControlLeft, KeyCode::SuperLeft],
            vec![KeyCode::ControlLeft, KeyCode::ShiftLeft, KeyCode::AltLeft],
            vec![KeyCode::ControlLeft, KeyCode::ShiftLeft, KeyCode::SuperLeft],
        ] {
            assert!(!dev_plane(&keys(&held)), "{held:?} is not the chord");
        }
    }

    #[test]
    fn a_synthesized_modifier_is_not_stuck() {
        let held = keys(&[KeyCode::ShiftLeft]);
        assert!(is_stuck(
            KeyCode::ShiftLeft,
            &held,
            &SyntheticHold::default()
        ));
        let hold = SyntheticHold(vec![KeyCode::ShiftLeft]);
        assert!(!is_stuck(KeyCode::ShiftLeft, &held, &hold));
        assert!(
            !is_stuck(KeyCode::ControlLeft, &held, &hold),
            "an unheld key is nothing to release either way"
        );
    }

    #[test]
    fn the_label_matches_the_plane() {
        assert_eq!(DEV_CHORD, "Ctrl+Shift");
        assert!(dev_plane(&keys(&[
            KeyCode::ControlLeft,
            KeyCode::ShiftLeft
        ])));
    }
}
