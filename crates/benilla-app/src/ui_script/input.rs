//! The player-UI input pass: [`feed_ui_input`] feeds mouse and keyboard events into the UI engine
//! once [`super::extract::tick_script`] has resolved the frame's rects.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{KeyWalk, UiScript};

use super::{
    CursorPayloadHeld, PlayerUiClickConsumed, PlayerUiHover, PlayerUiPressConsumed,
    UiKeyboardCapture,
};
use crate::bindings::WheelNotches;
use crate::textinput::{self, keymap, HostClipboard};

/// The pointer-side state [`feed_ui_input`] reads and writes. The world pick reads last frame's
/// hover and occlusion ray, since the target chain runs after this pass; one frame of staleness is
/// within the pick's tolerance.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PointerFeed<'w> {
    hover: ResMut<'w, PlayerUiHover>,
    click_consumed: ResMut<'w, PlayerUiClickConsumed>,
    press_consumed: ResMut<'w, PlayerUiPressConsumed>,
    hovered: Res<'w, crate::target::Hovered>,
    hovered_object: Res<'w, crate::target::HoveredObject>,
    occlusion: Res<'w, crate::target::PickOcclusion>,
    payload_held: ResMut<'w, CursorPayloadHeld>,
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

/// The buttons the pointer feed hands the UI, by the names its handlers read (`arg1` of `OnClick`,
/// `OnMouseDown` and `OnMouseUp`, and the ones `RegisterForClicks` takes,
/// `Blizzard_BindingUI.xml:10`); buttons 4 and 5 on the physical buttons the binding chords call
/// BUTTON4 and BUTTON5 (`bindings::chord`).
const UI_MOUSE_BUTTONS: [(MouseButton, &str); 5] = [
    (MouseButton::Left, "LeftButton"),
    (MouseButton::Right, "RightButton"),
    (MouseButton::Middle, "MiddleButton"),
    (MouseButton::Forward, "Button4"),
    (MouseButton::Back, "Button5"),
];

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
    // The keys this feed believes down, a fact about the keyboard and not the VM.
    mut pressed: Local<crate::bindings::KeyDownList>,
    // Who sends the key-ups: the bindings pass in the world, this feed on the glue screens.
    state: Option<Res<State<crate::char_select::ClientState>>>,
    // The characters the active layout makes, which name a key for a keyboard frame.
    layout: Res<crate::bindings::LayoutNames>,
) {
    let (keyboard, keys, capture, clipboard) = (&mut kbd.0, &kbd.1, &mut kbd.2, &mut kbd.3);
    // In the world the bindings pass sends the key-ups, in event order with the presses it
    // follows; on the glue screens, which have none, this feed does.
    let releases = !state
        .as_ref()
        .is_some_and(|s| *s.get() == crate::char_select::ClientState::InWorld);
    let world_pick = pointer.world_pick();
    // The OS pointer is not ours while a probe drives a gesture through the real pointer path or a
    // capture pins it: skip the mouse half whole, else-arm included, whose `pointer_left_window`
    // would disarm the probe's gesture between its press and release.
    let synthetic = pointer.synthetic.0 || pointer.capture_pinned.0;
    let (hover, click_consumed, press_consumed, payload_held) = (
        &mut pointer.hover,
        &mut pointer.click_consumed,
        &mut pointer.press_consumed,
        &mut pointer.payload_held,
    );
    click_consumed.0 = false;
    press_consumed.0 = false;
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
    // ── Mouse ── A cursor off the window skips only the mouse feed.
    // The headless hover probe's aim stands in for a missing cursor, so `PointerOverUi` rises and
    // falls over a panel in an automated run as it does for a person; a real pointer always wins.
    if let Some(cursor) = window
        .cursor_position()
        .or_else(crate::target::hover_probe_point)
        .filter(|_| !synthetic)
    {
        // The window cursor in the root's y-up 768-high units, as `GetCursorPosition` answers it.
        let Vec2 { x, y } = super::window_to_ui(window.height(), cursor);
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
        for (btn, name) in UI_MOUSE_BUTTONS {
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
                // A press the UI consumed is not the world's, hovered frame or not.
                press_consumed.0 |= script.mouse_button(x, y, name, true);
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
            script.pointer_left_hover();
        }
        script.pointer_left_window();
    }

    // ── Keyboard capture gate ── read after the mouse feed (a click may have just focused a box)
    // and before the keys (an Escape that clears focus still counts as captured this frame).
    capture.typing = script.has_keyboard_focus();
    capture.consumed.clear();
    script.set_alt_down(alt);
    // ── The alt-arrow exemption ────────────────────────────────────────────────────────────────
    // An EditBox in alt-arrow mode (`ignoreArrows`, set on the chat box at `ChatFrame.xml:21`)
    // passes the arrows on unless ALT is held: its handler returns 0 at `0x77b1c4` and the
    // binding runs at `CGWorldFrame`; a focused box swallows any other key (`0x77b35e` returns 1),
    // PRINTSCREEN excepted (`0x77b17f`, see `crate::bindings`).
    capture.arrows_fall_through = !alt && script.editbox_alt_arrow_mode();

    // ── Keyboard → VM ── every press is one key-down walk by name (`0x765f10`), in which a box is
    // a frame like the rest; a box the walk reached then takes the key's editing action through
    // the per-OS chord table as an `EditAction` or a clipboard operation (the macOS NSPasteboard
    // needs this main-thread system), and the characters go to `char_input`, a walk of its own.
    // A repeat goes to the frame that took the press instead (`0x766070`), a release likewise
    // (`0x765fd0`).
    let sup = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let mods = keymap::Mods {
        shift,
        ctrl,
        alt,
        sup,
    };
    feed_keys(
        &mut script,
        capture,
        keyboard.read(),
        keys,
        &mut pressed,
        releases,
        mods,
        &layout,
        clipboard,
        wl_display,
    );

    // The payload-held mirror, written last so a same-frame pickup or ESC clear shows: the
    // Send-side view the world-click deselect gates on.
    payload_held.0 = script.cursor_payload().is_some();

    for err in script.take_errors() {
        warn!("ui_script(input): {err}");
    }
}

/// The keyboard half of the frame: each event as the key-down or repeat walk and what a box the
/// walk reached does with the key. `down` is this feed's pressed-key list, which decides repeat
/// as the bindings' does (`0x4248b3`), never from the OS flag: after an alt-tab wipes it
/// (`0x424790`, the reconcile against `keys`) the first repeat is a fresh press, and a platform
/// that never sets the flag still repeats right. `releases` says whether the key-up dispatch is
/// this feed's: in the world the bindings pass sends it, in order with the press it follows
/// (`crate::bindings`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn feed_keys<'a>(
    script: &mut UiScript,
    capture: &mut UiKeyboardCapture,
    events: impl Iterator<Item = &'a KeyboardInput>,
    keys: &ButtonInput<KeyCode>,
    down: &mut crate::bindings::KeyDownList,
    releases: bool,
    mods: keymap::Mods,
    layout: &crate::bindings::LayoutNames,
    clipboard: &mut HostClipboard,
    wl_display: Option<*mut std::ffi::c_void>,
) {
    down.reconcile(keys);
    let keymap::Mods {
        shift: _,
        ctrl,
        alt,
        sup,
    } = mods;
    let mac = cfg!(target_os = "macos");
    for ev in events {
        // What the layout names the key: the clipboard chords' letters, and a keyboard frame's
        // `arg1`, which the Key Bindings window also stores, so a key's press dispatches by the
        // name it was bound under.
        let token = crate::bindings::chord::key_token(ev.key_code, layout);
        // `None` is a key the client drops before any walk (a Windows or Command key, a media
        // key): no down, no up, and no focus granted to an `autoFocus` box.
        let name = crate::bindings::chord::walk_name(ev.key_code, layout);
        if ev.state != ButtonState::Pressed {
            down.release(ev.key_code);
            if let Some(name) = name.as_deref().filter(|_| releases) {
                script.key_up(name);
            }
            continue;
        }
        let repeat = down.press(ev.key_code);
        let named = matches!(
            ev.key_code,
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape | KeyCode::Tab
        );
        let walk = match name.as_deref() {
            Some(name) if repeat => script.key_repeat(name),
            Some(name) => script.key_down(name),
            None => KeyWalk::Declined,
        };
        // The gate is existence, not handling: a shown keyboard frame with an `OnKeyDown` swallows
        // the key whatever its script does (`0x76b7d0`, `0x76ba25`). Consumption suppresses only
        // the key's binding: `OnChar` is a separate dispatcher (`0x765df0`), so the stack-split
        // spinner still gets a digit its `OnKeyDown` ate, and it is not a focus change, so it
        // releases nothing held. A box that took the key counts too: it returned 1 (`0x77b1db`
        // -> `0x77b35e`) and the world frame, last in the walk, never saw it, so an F-key that
        // gave an `autoFocus` box the focus runs no binding. An unconsumed press is acted on in
        // the binding dispatch (`crate::bindings`), which runs after this pass and reads the
        // capture gate.
        if walk != KeyWalk::Declined {
            capture.consumed.push(ev.key_code);
        }
        let editing_key = matches!(
            ev.key_code,
            KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::Home
                | KeyCode::End
        );
        if walk == KeyWalk::Frame && editing_key {
            // Not `typing`: a frame eating one key is not a box taking focus, and treating it as
            // one would stop a held movement key.
            continue;
        }
        if named {
            continue;
        }
        // The box's own key-down switch (`0x77b160`) does the editing, copy, cut, paste and
        // select-all, so only a box the walk reached acts; a frame that took the key, or a gated
        // arrow the box declined, leaves it alone.
        let chord = keymap::chord(ev.key_code, token, mods, mac);
        if let Some(chord) = chord.filter(|_| walk == KeyWalk::Box) {
            match chord {
                keymap::Chord::Edit(action) => {
                    script.editbox_action(action);
                }
                // Copy and cut need a selection (`0x77e1d0`), and a password box yields the empty
                // string, never the real text; paste types the clipboard in.
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
        } else if chord.is_none() && !(sup || (ctrl && !alt)) {
            // Command-modified chars never insert (Cmd+L must not type "l"), but Ctrl+Alt passes:
            // that is AltGr, and macOS Option+letter arrives with only `alt`.
            // The char walk gates no binding (`0x765df0` reaches no keybinding), so whether a
            // frame took the character is not recorded: the key-down already decided that.
            if let Some(text) = &ev.text {
                script.char_input(text);
            }
        }
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

    /// Every button 1.12's frames take reaches the UI, under the name its binding chord agrees
    /// with, so the Key Bindings page binds mouse 4 and 5 from a click.
    #[test]
    fn the_ui_takes_all_five_buttons_under_the_chords_names() {
        let names: Vec<&str> = UI_MOUSE_BUTTONS.iter().map(|(_, n)| *n).collect();
        assert_eq!(
            names,
            [
                "LeftButton",
                "RightButton",
                "MiddleButton",
                "Button4",
                "Button5"
            ]
        );
        for (i, (button, _)) in UI_MOUSE_BUTTONS.iter().enumerate() {
            let chord = crate::bindings::chord::Chord::parse(&format!("BUTTON{}", i + 1));
            assert_eq!(
                chord.map(|c| c.key),
                Some(crate::bindings::chord::BindKey::Mouse(*button)),
                "BUTTON{} is {button:?}",
                i + 1
            );
        }
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
