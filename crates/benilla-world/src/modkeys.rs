//! **The modifier keys** — the dev-overlay chord every instrument is bound on, and the
//! stuck-modifier reconciliation that keeps the keyboard honest underneath it.
//!
//! The chord ([`dev_chord`]) lived in `debug_panel` because the panel was its first user; six
//! modules then reached into a *debug overlay* to ask whether Ctrl+Shift was down — the player
//! controller's free-fly toggle and land-here, the perf HUD, the sound mute, the inspector.
//! Decision 1160 moves it here, where the leaf module that already owns modifier state is
//! (92 lines, no dependants of its own). Nothing about "which two modifiers are the dev plane" is
//! a debug-panel opinion.
//!
//! ## Stuck-modifier reconciliation against the OS's live flag state
//!
//! macOS system shortcuts that grab the keyboard *without* de-focusing the window — the ⇧⌘5
//! screenshot overlay is the canonical one — swallow the modifiers' release events: the overlay is
//! a non-activating panel, so the app never receives `Focused(false)` (Bevy's stuck-key reset,
//! `KeyboardFocusLost` → `release_all`, never fires), and winit only synthesizes modifier key
//! events from `flagsChanged`, which the grabbed keyboard never delivers (verified in
//! winit 0.30.13 `platform_impl/macos/view.rs::update_modifiers`). `ButtonInput<KeyCode>` then
//! reports Shift/Cmd held indefinitely, and every bare-key binding — the action bar's number row
//! behind `ui_script::input`'s bare-binding gate — goes dead until the user happens to tap the
//! stuck modifier again.
//!
//! The fix: poll the OS's live modifier state (`+[NSEvent modifierFlags]` — hardware-derived and
//! documented as independent of the event stream) once per frame, right after Bevy's own input
//! collection, and **release** any modifier Bevy believes is down but the OS says is up.
//! Release-only by design: the hardware state leads the event stream, so a legitimately
//! just-pressed modifier is never wrongly released — whereas synthesizing *presses* from the poll
//! could race in-flight release events and manufacture the same bug in reverse. The poll can't
//! tell left from right within a family, so an up family releases both variants (only ever a
//! correction — a genuinely held key keeps its family bit set and is never touched).

use bevy::prelude::*;

/// How the dev plane is written on screen. Every surface that names a chord reads this instead of
/// spelling one out, so the panel footer, the inspector badge and the mute checkbox can't drift from
/// what [`dev_chord`] actually listens for.
pub const DEV_CHORD: &str = "Ctrl+Shift";

/// Did the **dev-overlay chord** — [`DEV_CHORD`]+*key* — just fire? (
/// 1043 — which moved the last two dev keys onto it, so the whole fleet is here now.)
///
/// The dev instruments used to sit on bare letters, which is a namespace we don't own: every letter is
/// a *game* binding in the reference client, so `P` both opened the spellbook and toggled the perf HUD.
/// They moved to `Ctrl`+`Cmd`, and then off it: on Windows that is `Ctrl`+`Win`, a plane the
/// shell owns and keeps extending — `Win+Ctrl+M` is Magnifier settings, which had our mute.
///
/// **`Ctrl`+`Shift`, one plane on every OS** (director's call). The alternative was keeping
/// `Ctrl`+`Cmd` on macOS for its one real advantage — Cmd is outside the reference's binding namespace
/// (1.12 builds binding names from `ALT-`/`CTRL-`/`SHIFT-` only), so nothing in game could *ever* claim
/// it. That buys protection against a binding no default declares and no player has yet written, and
/// it costs a per-OS split in the docs, the hints and the reader's head. One plane wins. It also drops
/// our dependence on winit's `sendEvent:` swizzle, without which AppKit's swallowed `keyUp` under Cmd
/// would latch a chord after one use (0585's macOS risk, now simply not run).
///
/// `Ctrl`+`Shift` is the emptiest plane the reference *can* name: of its 152 defaults, exactly two
/// carry two modifiers — `CTRL-SHIFT-TAB` and `CTRL-SHIFT-PAGEDOWN` — and no letter at all.
/// `Ctrl`+`Alt` was never available: that is AltGr, which European layouts type real characters with.
///
/// **Exactly those two modifiers and no others**, both sides of each. The block is what makes this
/// plane safe to leave ungated below: AltGr+Shift+*key* is `Ctrl`+`Alt`+`Shift`+*key*, and a German
/// layout typing one of those into chat must not fire an overlay.
///
/// **"No letter at all" was half the story, and the missing half cost us a plane's worth of safety**.
/// The reference does not match bindings by equality alone: an exact miss re-probes
/// **once** with the leftmost modifier dropped, so `CTRL-SHIFT-`*key* falls through to
/// `SHIFT-`*key* — never to the bare letter, which is why this plane survived the correction at all,
/// but far enough that `Ctrl`+`Shift`+`P` would open the pet paper doll (`SHIFT-P`,
/// `TOGGLECHARACTER3`) under the perf HUD. So the other half of the rule now lives where the law
/// itself does, in `bindings::BindingDispatch::resolve`: this plane spends the keyboard's
/// **fallback probe**, and only that — an exact `CTRL-SHIFT-` binding, the reference's two included,
/// still dispatches normally.
///
/// Deliberately **not** gated on [`crate::ui_script::UiKeyboardCapture`] the way the bare-key toggles
/// were: a chord can't be mistaken for typed text, so the dev overlays stay reachable with the chat bar
/// open.
pub fn dev_chord(keys: &ButtonInput<KeyCode>, key: KeyCode) -> bool {
    dev_plane(keys) && keys.just_pressed(key)
}

/// The modifier half of [`dev_chord`]. See there for why the plane is what it is.
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

/// Keys a **synthetic** input source is holding this frame — the probe harness's `WOW_PROBE_KEY`
/// taps are the only writer today.
///
/// The reconciliation above polls the *hardware* flag state, which by construction reads "up" for
/// a key no hand is on. Without this list a synthesized `Shift`/`Ctrl` is released the frame after
/// it is pressed — and logged as a stuck-key correction, so the instrument reads as working while
/// **no probe can reach a chord binding on macOS at all** (`SHIFT-V`'s friendly nameplates, the
/// `Ctrl`+`Shift`+`F` free-fly the `"Shift"` tap name was added for). The engine owns the type
/// because the reconciler does; whoever synthesizes input writes it, on every platform.
#[derive(Resource, Default)]
pub struct SyntheticHold(pub Vec<KeyCode>);

/// Is `code` a modifier Bevy holds down that nothing is legitimately holding — i.e. stuck, and the
/// reconciler's to release? Pure, so the rule is testable off-platform (the reconciler itself
/// needs AppKit and the main thread) — which is also why it is compiled for the test build of
/// every platform, and only there off macOS.
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
            // Guarded so an in-sync frame (the overwhelmingly common case) never marks the
            // resources changed — and a key a synthetic source is holding
            // ([`super::SyntheticHold`]) is never "stuck": the hardware poll cannot see it.
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

    /// One modifier is not the chord, and a third one names something else. The case that matters:
    /// AltGr *is* `Ctrl`+`Alt`, so AltGr+Shift+key is a character a European layout types, never a
    /// dev chord — the overlays are ungated while the chat bar is open. `Ctrl`+`Cmd` is likewise
    /// nothing of ours now: on Windows it is the shell's, `Win+Ctrl+M` being Magnifier
    /// settings.
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

    /// A modifier a **synthetic** source is holding is never "stuck". The macOS reconciler polls
    /// the hardware flags, which read up for a key no hand is on — so without the exemption a
    /// synthesized chord dies the frame after it is pressed, and every `WOW_PROBE_KEY` chord
    /// (`SHIFT-V`'s friendly nameplates, `Ctrl`+`Shift`+`F`'s free-fly) is unreachable on the
    /// platform we develop on, while the log calls it a stuck-key fix.
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

    /// The label a hint prints is the plane actually listened for — one const, one predicate, so a
    /// player is never told to press a key we stopped reading.
    #[test]
    fn the_label_matches_the_plane() {
        assert_eq!(DEV_CHORD, "Ctrl+Shift");
        assert!(dev_plane(&keys(&[
            KeyCode::ControlLeft,
            KeyCode::ShiftLeft
        ])));
    }
}
