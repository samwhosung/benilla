//! The player-UI input pass: [`feed_ui_input`] feeds mouse and keyboard events into the UI engine
//! once [`super::extract::tick_script`] has resolved the frame's rects.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::UiScript;

use super::{CursorPayloadHeld, PlayerUiClickConsumed, PlayerUiHover, UiKeyboardCapture};
use crate::bindings::WheelNotches;
use crate::textinput::{self, keymap, HostClipboard};

/// The pointer-side state [`feed_ui_input`] reads and writes. The world pick reads last frame's
/// hover and occlusion ray, since the target chain runs after this pass; one frame of staleness is
/// within the pick's tolerance.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PointerFeed<'w> {
    hover: ResMut<'w, PlayerUiHover>,
    click_consumed: ResMut<'w, PlayerUiClickConsumed>,
    hovered: Res<'w, crate::target::Hovered>,
    hovered_object: Res<'w, crate::target::HoveredObject>,
    occlusion: Res<'w, crate::target::PickOcclusion>,
    payload_held: ResMut<'w, CursorPayloadHeld>,
    /// TOGGLEUI ([`crate::ui_hide::UiHidden`]): a hidden UI takes no mouse at all.
    hidden: Res<'w, crate::ui_hide::UiHidden>,
    /// A headless probe drives the pointer ([`super::SyntheticPointer`]), not the real cursor.
    synthetic: Res<'w, super::SyntheticPointer>,
    /// A capture owns the pointer ([`super::CapturePointerPinned`]): no OS cursor in the shot.
    capture_pinned: Res<'w, super::CapturePointerPinned>,
}

impl PointerFeed<'_> {
    /// The reference's click-time pick: a hovered unit or GameObject is `Object`, else a finite
    /// occlusion-ray hit (terrain, WMO, doodad) is `Terrain`, else `Nothing` (sky).
    fn world_pick(&self) -> benilla_ui::script::WorldPick {
        use benilla_ui::script::WorldPick;
        if self.hovered.target.is_some() || self.hovered_object.target.is_some() {
            WorldPick::Object
        } else if self.occlusion.distance.is_finite() {
            WorldPick::Terrain
        } else {
            WorldPick::Nothing
        }
    }
}

/// One `OnMouseWheel` call per whole notch, `arg1 = ±1`, the fraction carried in `notches`: a
/// trackpad gesture arrives as a `Pixel` trickle, and the stock handlers act on the sign alone
/// (`ScrollFrameTemplate_OnMouseWheel` scrolls half a pane per call, `UIPanelTemplates.lua:150`).
fn feed_wheel(
    script: &mut UiScript,
    notches: &mut WheelNotches,
    x: f32,
    y: f32,
    scroll: &AccumulatedMouseScroll,
) {
    let whole = notches.feed(crate::bindings::wheel_lines(scroll.unit, scroll.delta.y));
    let step = whole.signum() as f32;
    for _ in 0..whole.unsigned_abs() {
        script.mouse_wheel(x, y, step);
    }
}

/// Feeds cursor, buttons, wheel and keys into the UI engine, publishing [`PlayerUiHover`] (the
/// pointer arbiter yields to the UI) and [`UiKeyboardCapture`] (a key a box or frame ate never
/// fires its binding). Runs in the `UiInput` set, before `WorldStage::Input` and every key reader.
pub(super) fn feed_ui_input(
    script: Option<NonSendMut<UiScript>>,
    // Carries the `wl_display` the Wayland clipboard backend needs, once winit has a surface.
    window: Query<(&Window, Option<&bevy::window::RawHandleWrapper>), With<PrimaryWindow>>,
    buttons: Res<ButtonInput<MouseButton>>,
    // The wheel travel and its carried notch fraction, one param for clippy's argument ceiling.
    (scroll, mut notches): (Res<AccumulatedMouseScroll>, ResMut<WheelNotches>),
    mut pointer: PointerFeed,
    // One param for clippy's argument ceiling, with the pasteboard the clipboard chords use.
    mut kbd: (
        MessageReader<KeyboardInput>,
        Res<ButtonInput<KeyCode>>,
        ResMut<UiKeyboardCapture>,
        NonSendMut<HostClipboard>,
    ),
    // The uiScale dial folded into the seam scale.
    ui_scale: Res<super::UiScaleCvar>,
) {
    let (keyboard, keys, capture, clipboard) = (&mut kbd.0, &kbd.1, &mut kbd.2, &mut kbd.3);
    let world_pick = pointer.world_pick();
    let ui_hidden = pointer.hidden.0;
    // The OS pointer is not ours while a probe drives a gesture through the real pointer path or a
    // capture pins it: skip the mouse half whole, else-arm included, whose `pointer_left_window`
    // would disarm the probe's gesture between its press and release.
    let synthetic = pointer.synthetic.0 || pointer.capture_pinned.0;
    let (hover, click_consumed, payload_held) = (
        &mut pointer.hover,
        &mut pointer.click_consumed,
        &mut pointer.payload_held,
    );
    click_consumed.0 = false;
    let Some(mut script) = script else {
        capture.typing = false;
        capture.arrows_fall_through = false;
        capture.consumed.clear();
        payload_held.0 = false;
        return;
    };
    let Ok((window, raw_handle)) = window.single() else {
        capture.typing = false;
        capture.arrows_fall_through = false;
        capture.consumed.clear();
        payload_held.0 = false;
        return;
    };
    let wl_display = textinput::wayland_display(raw_handle);
    // The pick routes the world drop: an object keeps the payload, terrain or sky drops it.
    script.set_world_pick(world_pick);
    // ── Modifiers ── before the mouse feed, so a click's `IsShiftKeyDown` sees the click's state.
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    script.set_modifiers(shift, ctrl, alt);
    // ── Mouse ── A cursor off the window, or a UI hidden by TOGGLEUI, skips only the mouse feed.
    // The headless hover probe's aim stands in for a missing cursor, so `PointerOverUi` rises and
    // falls over a panel in an automated run as it does for a person; a real pointer always wins.
    if let Some(cursor) = window
        .cursor_position()
        .or_else(crate::target::hover_probe_point)
        .filter(|_| !ui_hidden && !synthetic)
    {
        // The window cursor is logical px, y-down from the top left; the UI is y-up in 768-high
        // units under uiScale: flip through the window height, then undo the extract seam's scale.
        let s = super::seam_scale(window.height(), ui_scale.0);
        let (x, y) = (cursor.x / s, (window.height() - cursor.y) / s);
        // `WOW_HIT_COST=1` meters this call's per-frame hit-test rebuild, one line a second.
        static HIT_COST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let metering =
            *HIT_COST.get_or_init(|| std::env::var("WOW_HIT_COST").as_deref() == Ok("1"));
        let t0 = metering.then(std::time::Instant::now);
        // The world frame is mouse-enabled, so an addon's handlers can hover it, but its hit is
        // the world's: camera look, world clicks and hover targeting stay live over it.
        hover.0 = script
            .mouse_move(x, y)
            .filter(|id| !script.is_world_frame(*id));
        if let Some(t0) = t0 {
            use std::sync::atomic::{AtomicU64, Ordering};
            static ACC_US: AtomicU64 = AtomicU64::new(0);
            static CALLS: AtomicU64 = AtomicU64::new(0);
            static LAST: AtomicU64 = AtomicU64::new(0);
            ACC_US.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
            let calls = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
            let now_s = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if LAST.swap(now_s, Ordering::Relaxed) != now_s && calls > 1 {
                let us = ACC_US.swap(0, Ordering::Relaxed);
                let n = CALLS.swap(0, Ordering::Relaxed);
                eprintln!(
                    "[hit-cost] mouse_move_us/frame={:.1} frames={n}",
                    us as f64 / n.max(1) as f64
                );
            }
        }
        for (btn, name) in [
            (MouseButton::Left, "LeftButton"),
            (MouseButton::Right, "RightButton"),
            (MouseButton::Middle, "MiddleButton"),
        ] {
            if buttons.just_pressed(btn) {
                // A left press that would drop into the world (`world_drop_click`: any payload over
                // terrain or nothing) is consumed now: the drop fires on the release, but the world
                // click-pick and camera orbit act on the press. Over an object the reference runs
                // SELECT with the payload still held.
                if btn == MouseButton::Left && hover.0.is_none() {
                    use benilla_ui::script::WorldPick;
                    let would_drop =
                        script.cursor_payload().is_some() && world_pick != WorldPick::Object;
                    if would_drop {
                        click_consumed.0 = true;
                    }
                }
                script.mouse_button(x, y, name, true);
            }
            if buttons.just_released(btn) {
                script.mouse_button(x, y, name, false);
            }
        }
        feed_wheel(&mut script, &mut notches, x, y, &scroll);
    } else if !synthetic {
        // The OS pointer left: leave the hovered frame once, and disarm any press or drag every
        // frame, since no release ends it and a stale gesture would fire on re-entry.
        if hover.0.take().is_some() {
            script.mouse_move(f32::MIN, f32::MIN);
        }
        script.pointer_left_window();
    }

    // ── Keyboard capture gate ── read after the mouse feed (a click may have just focused a box)
    // and before the keys (an Escape that clears focus still counts as captured this frame).
    capture.typing = script.has_keyboard_focus();
    capture.consumed.clear();
    // ── The alt-arrow exemption ────────────────────────────────────────────────────────────────
    // An EditBox in alt-arrow mode (`ignoreArrows`, set on the chat box at `ChatFrame.xml:21`)
    // passes the arrows on unless ALT is held: its handler returns 0 at `0x77b1c4` and the
    // binding runs at `CGWorldFrame`; a focused box swallows any other key (`0x77b35e` returns 1).
    capture.arrows_fall_through = !alt && script.editbox_alt_arrow_mode();

    // ── Keyboard → VM ── box-event keys go to `key_input` by name, editing keys through the
    // per-OS chord table as an `EditAction` or a clipboard operation (the macOS NSPasteboard
    // needs this main-thread system), the rest to `char_input`. Repeats arrive as `Pressed`.
    let sup = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let mods = keymap::Mods {
        shift,
        ctrl,
        alt,
        sup,
    };
    let mac = cfg!(target_os = "macos");
    for ev in keyboard.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let named = match ev.key_code {
            KeyCode::Enter | KeyCode::NumpadEnter => Some("ENTER"),
            KeyCode::Escape => Some("ESCAPE"),
            KeyCode::Tab => Some("TAB"),
            _ => None,
        };
        // Dispatched unconditionally: unfocused, they fall through to the camera and turn keys.
        let chord = keymap::chord(ev.key_code, mods, mac);
        // A keyboard frame gets these by name before their chord runs (a dialog needs BACKSPACE);
        // `frame_key_input` declines at a focused box, so no frame steals its editing keys. `true`
        // suppresses the chord and the key's binding (the reference's existence gate, `0x76b7d0`).
        let frame_named = match ev.key_code {
            KeyCode::Backspace => Some("BACKSPACE"),
            KeyCode::Delete => Some("DELETE"),
            KeyCode::ArrowLeft => Some("LEFT"),
            KeyCode::ArrowRight => Some("RIGHT"),
            KeyCode::ArrowUp => Some("UP"),
            KeyCode::ArrowDown => Some("DOWN"),
            KeyCode::Home => Some("HOME"),
            KeyCode::End => Some("END"),
            _ => None,
        };
        if let Some(name) = frame_named {
            if script.frame_key_input(name) {
                // Not `typing`: a frame eating one key is not a box taking focus, and treating it
                // as one would stop a held movement key.
                capture.consumed.push(ev.key_code);
                continue;
            }
        }
        // Every other key reaches a keyboard frame by name too: the reference's key-down walk
        // takes its `arg1` from the table the binding chord uses (`0x4b66b0`), `chord::key_token`
        // here, and the gate is existence, not handling: a shown keyboard frame with an
        // `OnKeyDown` swallows the key whatever its script does (`0x76b7d0`, `0x76ba25`).
        // Consumption suppresses only the key's binding: `OnChar` is a separate dispatcher
        // (`0x765df0`), so the stack-split spinner still gets a digit its `OnKeyDown` ate, and it
        // is not a focus change, so it releases nothing held.
        else if named.is_none() {
            if let Some(token) = crate::bindings::chord::key_token(ev.key_code) {
                if script.frame_key_input(token) {
                    capture.consumed.push(ev.key_code);
                }
            }
        }
        if let Some(name) = named {
            // An unconsumed press is not acted on here: ESCAPE's close ladder, TAB's targeting and
            // ENTER's chat open live in the binding dispatch (`crate::bindings`), which runs after
            // this pass and reads the capture gate.
            if script.key_input(name) {
                capture.consumed.push(ev.key_code);
            }
        } else if let Some(chord) = chord {
            // The gate is on the key, not the action: a gated arrow never reaches the box (the
            // reference's `return 0`), while HOME/END, which also make `Move { unit: Edge }`, do.
            let gated_arrow = capture.arrows_fall_through
                && matches!(
                    ev.key_code,
                    KeyCode::ArrowLeft
                        | KeyCode::ArrowRight
                        | KeyCode::ArrowUp
                        | KeyCode::ArrowDown
                );
            match chord {
                keymap::Chord::Edit(action) if !gated_arrow => {
                    script.editbox_action(action);
                }
                // Declined: the key falls through to its binding, which `bindings.rs` allows
                // because `arrows_fall_through` exempts exactly these four from the typing gate.
                keymap::Chord::Edit(_) => {}
                // Copy and cut need a selection (`0x77e1d0`), and a password box yields its mask
                // run, never the real text; paste sanitizes and inserts.
                keymap::Chord::Copy => {
                    if let Some(text) = script.editbox_copy() {
                        clipboard.write(wl_display, &text);
                    }
                }
                keymap::Chord::Cut => {
                    if let Some(text) = script.editbox_cut() {
                        clipboard.write(wl_display, &text);
                    }
                }
                keymap::Chord::Paste => {
                    if let Some(text) = clipboard.read(wl_display) {
                        script.paste(&text);
                    }
                }
            }
        } else if !(sup || (ctrl && !alt)) {
            // Command-modified chars never insert (Cmd+L must not type "l"), but Ctrl+Alt passes:
            // that is AltGr, and macOS Option+letter arrives with only `alt`.
            if let Some(text) = &ev.text {
                // Same suppression as the named keys: the number row is a binding, so a digit typed
                // into a keyboard frame (the stack-split spinner) must not also fire an action.
                if script.char_input(text) {
                    capture.consumed.push(ev.key_code);
                }
            }
        }
    }

    // The payload-held mirror, written last so a same-frame pickup or ESC clear shows: the
    // Send-side view the world-click deselect gates on.
    payload_held.0 = script.cursor_payload().is_some();

    for err in script.take_errors() {
        warn!("ui_script(input): {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::mouse::MouseScrollUnit;

    fn wheel_pane() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.run(
            r#"
            local f = CreateFrame("Frame", "WheelProbe")
            f:SetWidth(200) f:SetHeight(200) f:SetPoint("CENTER", nil, "CENTER", 0, 0)
            f:EnableMouseWheel(true)
            f:SetScript("OnMouseWheel", function()
                WheelCalls = (WheelCalls or 0) + 1
                WheelSum = (WheelSum or 0) + arg1
            end)
            f:Show()
            "#,
        )
        .unwrap();
        s.resolve();
        s
    }

    fn calls(s: &UiScript) -> (i64, f64) {
        s.eval::<(Option<i64>, Option<f64>)>("return WheelCalls, WheelSum")
            .map(|(c, v)| (c.unwrap_or(0), v.unwrap_or(0.0)))
            .unwrap()
    }

    fn frame_of(unit: MouseScrollUnit, dy: f32) -> AccumulatedMouseScroll {
        AccumulatedMouseScroll {
            unit,
            delta: Vec2::new(0.0, dy),
        }
    }

    #[test]
    fn a_trackpad_trickle_fires_only_the_notches_it_adds_up_to() {
        let mut s = wheel_pane();
        let mut notches = WheelNotches::default();
        let step = MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR / 10.0;
        for _ in 0..10 {
            feed_wheel(
                &mut s,
                &mut notches,
                512.0,
                384.0,
                &frame_of(MouseScrollUnit::Pixel, step),
            );
        }
        let (n, sum) = calls(&s);
        assert!(
            n <= 1,
            "ten frames adding up to one line fired {n} wheel calls (sum {sum})"
        );
        assert!(sum <= 1.0, "…and they may move at most one notch: {sum}");
    }

    #[test]
    fn a_line_notch_fires_once() {
        let mut s = wheel_pane();
        let mut notches = WheelNotches::default();
        feed_wheel(
            &mut s,
            &mut notches,
            512.0,
            384.0,
            &frame_of(MouseScrollUnit::Line, 1.0),
        );
        assert_eq!(calls(&s), (1, 1.0));
        feed_wheel(
            &mut s,
            &mut notches,
            512.0,
            384.0,
            &frame_of(MouseScrollUnit::Line, -1.0),
        );
        assert_eq!(calls(&s), (2, 0.0), "and the other way is a notch down");
    }
}
