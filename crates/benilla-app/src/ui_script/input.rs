//! The player-UI input pass: [`feed_ui_input`] hit-tests the cursor and dispatches mouse/keyboard
//! events into the UI engine (after [`super::extract::tick_script`] has resolved the frame's
//! rects), plus the action-bar key map. The OS pasteboard itself lives in [`crate::textinput`].
//! Split out of [`super`] purely for size — the plugin wiring and the extraction pass live there
//! and in [`super::extract`] respectively.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::UiScript;

use super::{CursorPayloadHeld, PlayerUiClickConsumed, PlayerUiHover, UiKeyboardCapture};
use crate::bindings::WheelNotches;
use crate::textinput::{self, keymap, HostClipboard};

/// The pointer-side state [`feed_ui_input`] reads and writes, as one
/// [`bevy::ecs::system::SystemParam`] (the argument ceiling): the UI hover + click-consumed
/// outputs, the world pick inputs (LAST frame's hovered unit/GameObject + the occlusion ray —
/// the target chain runs after this pass; a frame's staleness is within the pick's own
/// tolerance) that route the world-click payload legs (decisions 0571 + 0574), and the
/// payload-held mirror.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PointerFeed<'w> {
    hover: ResMut<'w, PlayerUiHover>,
    click_consumed: ResMut<'w, PlayerUiClickConsumed>,
    hovered: Res<'w, crate::target::Hovered>,
    hovered_object: Res<'w, crate::target::HoveredObject>,
    occlusion: Res<'w, crate::target::PickOcclusion>,
    payload_held: ResMut<'w, CursorPayloadHeld>,
    /// TOGGLEUI ([`crate::ui_hide::UiHidden`]): a hidden UI takes no mouse at all — it rides here
    /// because that is precisely the pointer half of this pass.
    hidden: Res<'w, crate::ui_hide::UiHidden>,
    /// A headless probe is driving the pointer itself ([`super::SyntheticPointer`]) — the real
    /// cursor must not fight it. Same arm as `hidden` below, for the same reason: the mouse half
    /// of this pass is not ours this frame.
    synthetic: Res<'w, super::SyntheticPointer>,
    /// A capture owns the pointer for its whole run ([`super::CapturePointerPinned`]) — the OS
    /// cursor belongs to whoever is at the keyboard and must not reach the shot.
    capture_pinned: Res<'w, super::CapturePointerPinned>,
}

impl PointerFeed<'_> {
    /// The reference's click-time pick state, derived from what benilla already tracks: a
    /// hovered unit/GameObject is an `Object` pick; else a finite occlusion-ray hit (terrain/
    /// WMO/doodad under the cursor) is `Terrain`; else `Nothing` (sky). Decision 0574's
    /// terrain-vs-nothing split rides the occlusion ray the unit pick already casts.
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

/// The frame's wheel travel into the pane under the cursor's `OnMouseWheel` — **one call per whole
/// notch**, `arg1 = ±1`.
///
/// The travel is normalised to lines first ([`crate::bindings::wheel_lines`]) and the fraction
/// carried in `notches`: a trackpad reports a gesture as a `Pixel` trickle across many frames, and
/// passing each frame's raw delta fired the handler once per frame — the stock handlers act on
/// the sign alone (`ScrollFrameTemplate_OnMouseWheel` scrolls half a pane per call), so a gentle
/// swipe slammed the pane to its end. A mouse wheel's `Line` notch still fires once, as before.
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

/// Feed the window's cursor + buttons + wheel + keyboard into the UI engine (after
/// [`super::extract::tick_script`] has resolved this frame's rects), firing
/// OnEnter/OnLeave/OnClick/OnMouseWheel and the EditBox
/// char/key dispatch, publishing [`PlayerUiHover`] (so the pointer arbiter yields world-pick/camera to
/// the UI) and [`UiKeyboardCapture`] (so a key a box or frame ate never also fires its binding).
///
/// Runs in [`UiInput`], before `WorldStage::Input` and every other keyboard reader — a key a focused
/// box consumes must never also reach the world in the same frame.
pub(super) fn feed_ui_input(
    script: Option<NonSendMut<UiScript>>,
    // The window's raw handle rides along with it: on Wayland it carries the `wl_display` the
    // clipboard backend is built from (decision 0702). `Option`, because it only appears once
    // winit has actually created the surface.
    window: Query<(&Window, Option<&bevy::window::RawHandleWrapper>), With<PrimaryWindow>>,
    buttons: Res<ButtonInput<MouseButton>>,
    // The frame's wheel travel, and the fraction of a notch carried between frames
    // ([`feed_wheel`]) — one param for clippy's argument ceiling.
    (scroll, mut notches): (Res<AccumulatedMouseScroll>, ResMut<WheelNotches>),
    // One [`PointerFeed`] (clippy's argument ceiling): the hover + click-consumed outputs this
    // pass writes, the world pick that routes the world-click payload legs (decision 0571), and
    // the payload-held mirror written for the Send-side world-click consumers.
    mut pointer: PointerFeed,
    // Bundled into one param (clippy's argument ceiling): this frame's key messages, the modifier
    // mirror, the capture gate the keyboard feed writes, and the held OS pasteboard the three
    // clipboard chords resolve against (decision 0702).
    mut kbd: (
        MessageReader<KeyboardInput>,
        Res<ButtonInput<KeyCode>>,
        ResMut<UiKeyboardCapture>,
        NonSendMut<HostClipboard>,
    ),
    // The uiScale dial folded into the seam scale (decision 0584).
    ui_scale: Res<super::UiScaleCvar>,
) {
    let (keyboard, keys, capture, clipboard) = (&mut kbd.0, &kbd.1, &mut kbd.2, &mut kbd.3);
    let world_pick = pointer.world_pick();
    let ui_hidden = pointer.hidden.0;
    // A headless probe is driving the pointer itself this frame (the drag probe). It takes the
    // WHOLE mouse half out — including the else-arm below, which is the half that matters: a
    // `pointer_left_window` between the synthetic press and the synthetic release would disarm
    // the gesture the probe is in the middle of.
    // The two reasons the OS pointer is not ours this frame: a probe is driving a gesture through
    // the real pointer path, or this is a capture (where it is never ours). Same treatment — skip
    // the mouse half whole, touch nothing — so they read as one condition here.
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
    // `Some` only on a Wayland session — the signal the clipboard backend picks itself by.
    let wl_display = textinput::wayland_display(raw_handle);
    // The engine's world-drop routing (decisions 0571 + 0574): an object pick keeps every
    // payload (the reference's object leg dispatches SELECT with the item still held), terrain
    // drops items only, nothing drops any arm.
    script.set_world_pick(world_pick);
    // ── Modifiers ── pushed BEFORE the mouse feed: a click handler's modifier fork
    // (`IsShiftKeyDown` — the reference's shift-split/ctrl-dressup/shift-pickup) reads the state
    // as of the click, not last frame's.
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    script.set_modifiers(shift, ctrl, alt);
    // ── Mouse ── (cursor-off-window only skips the mouse feed; keyboard still flows to a focused box)
    // A UI hidden by TOGGLEUI takes the same route as a cursor outside the window: an action bar
    // nobody can see must not eat the click or arm a tooltip, and the else-arm below is exactly the
    // "no pointer here" bookkeeping (leave the hovered frame once, disarm any press/drag every
    // frame) that keeps a stale gesture from firing when the UI comes back.
    // The headless hover probe's aim stands in for a cursor the window does not have (2250/2255),
    // so `PointerOverUi` rises over a panel and falls off it in an automated run exactly as it does
    // for a person — which is what lets a rig run reproduce "open the map, close it, and the world
    // under it goes quiet". A person's pointer always wins; an unarmed probe answers `None` and
    // nothing here changes.
    if let Some(cursor) = window
        .cursor_position()
        .or_else(crate::target::hover_probe_point)
        .filter(|_| !ui_hidden && !synthetic)
    {
        // Window cursor is logical px, y-down from top-left; the UI is y-up 768-virtual units
        // (decisions 0582 + 0584's uiScale dial) — flip through the window height, then ÷s into
        // the VM's space (the inverse of the extract seam's ×s).
        let s = super::seam_scale(window.height(), ui_scale.0);
        let (x, y) = (cursor.x / s, (window.height() - cursor.y) / s);
        // `WOW_HIT_COST=1` — the hit-test lane's own meter (the resolve-lap report's W9): this
        // call rebuilds `order::traversal` + sort + the scroll-clip map EVERY frame the cursor
        // is inside the window, moving or not — a real-play cost every parked probe leg is
        // structurally blind to (probes park the cursor outside). One aggregate line per
        // second, census-meter posture; the number decides whether the epoch-gated skip is
        // worth building, before it is built.
        static HIT_COST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let metering =
            *HIT_COST.get_or_init(|| std::env::var("WOW_HIT_COST").as_deref() == Ok("1"));
        let t0 = metering.then(std::time::Instant::now);
        // The world frame is mouse-enabled by construction and so a legitimate hover target for
        // an addon's handlers, but its hit is the WORLD's (decision 1983): camera look, world
        // clicks and hover targeting stay live over it.
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
                // A LEFT press that would complete as a world DROP belongs to the drop flow
                // (the drop itself fires on the completed click's RELEASE — 0218's
                // byte-verified trigger — but the press is when the world click-pick and
                // camera orbit-start would act, so they must yield now, exactly as for a
                // hovered click). "Would drop" mirrors `world_drop_click`'s pick routing
                // (decisions 0571 + 0574, amended by 0843): ANY payload over terrain/nothing
                // drops (the item's popup, the spell/action's silent dismiss). Only a payload
                // over an OBJECT is not consumed — the reference runs SELECT with the payload
                // still held, so that click must reach the world.
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
        // The OS pointer left the window: leave whatever frame was hovered (once, on the
        // Some→None transition) and — every frame it stays outside — clear any armed press/drag
        // (`UiScript::pointer_left_window`), since no release is ever fed to end it and a stale
        // gesture would fire a spurious `OnDragStart`/`OnClick` on re-entry.
        if hover.0.take().is_some() {
            script.mouse_move(f32::MIN, f32::MIN);
        }
        script.pointer_left_window();
    }

    // ── Keyboard capture gate ── read AFTER the mouse feed (a LeftButton click may have just focused a
    // box) but BEFORE feeding keys (an Escape that clears focus is still "captured" this frame, so the
    // world doesn't also act on it). Gameplay/dev readers run after `UiInput` and see this value.
    capture.typing = script.has_keyboard_focus();
    // The per-key half is this frame's alone (a frame's existence gate ate THIS key).
    capture.consumed.clear();
    // ── The alt-arrow exemption ────────────────────────────────────────────────────────────────
    // A focused EditBox in alt-arrow mode (`ignoreArrows` in XML, `SetAltArrowKeyMode` in Lua)
    // does NOT consume LEFT/UP/RIGHT/DOWN unless ALT is held: the reference's handler returns 0
    // at `0x77b1c4` and the strata walk carries the key down to `CGWorldFrame`, which runs its
    // binding. That is what lets you turn while the chat box has focus — and the reference's own
    // chat box ships the flag, so this is the default experience, not an edge case.
    //
    // Both terms are whole-frame (one focused box; ALT off the same modifier mirror), so the
    // exemption is too. Only the four arrows may use it: a focused box swallows every other key
    // on every other path (`0x77b35e` returns 1).
    capture.arrows_fall_through = !alt && script.editbox_alt_arrow_mode();

    // ── Keyboard → VM ── the three box-event keys route to `key_input` by name (and never also
    // as text, so Enter isn't a stray newline); every *editing* key goes through the per-OS
    // chord table ([`keymap::chord`]) and reaches the box as a semantic `EditAction` — or as
    // one of the clipboard operations, handled here (this is a NonSend, main-thread system —
    // required for the macOS NSPasteboard). What's left goes to `char_input`, minus
    // command-modified chars (Cmd/Ctrl+letter must never type the letter — except Ctrl+Alt, the
    // AltGr plane European layouts type real characters with). Repeats carry `state == Pressed`,
    // so held keys repeat. Modifiers reuse the mirror read above (the message carries none).
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
        // The editing keys/chords — dispatched unconditionally like the named keys (the engine's
        // route law decides; unfocused they return unconsumed, and the camera/turn keys that
        // read arrows after UiInput see them exactly as before).
        let chord = keymap::chord(ev.key_code, mods, mac);
        // A KEYBOARD FRAME gets these by NAME before their chord runs (decision 1319). They are
        // the keys a focused EditBox receives as a semantic `EditAction` instead of a name, so
        // they never reached `key_input` at all — and a dialog you type into needs its BACKSPACE.
        // `frame_key_input` walks in the engine's own order and declines at a focused box, so the
        // box's editing keys can never be stolen by a frame below it; a `true` here means a frame
        // consumed the key, which suppresses BOTH the chord below and the key's binding (the
        // reference's existence gate, `0x76b7d0`).
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
                // Consumed by a frame: THIS key's binding must not also fire. Not `typing` — a
                // frame eating a key is not a text box taking focus, and reading it as one is what
                // stopped a held W dead when the world map ate the `M` that closed it (2196).
                capture.consumed.push(ev.key_code);
                continue;
            }
        }
        // EVERY OTHER KEY reaches a keyboard frame by name too. The reference's key-down walk
        // carries the whole key table — its `arg1` comes from the *same* table the keybinding
        // chord uses (`0x4b66b0`), which is [`chord::key_token`] here — and the frame's gate is
        // EXISTENCE, not handling: a shown keyboard frame with an `OnKeyDown` swallows the key
        // whatever its script does with it (`0x76b7d0`, `0x76ba25`). This host used to feed only
        // the ten names above, so `CinematicFrame` — fullscreen, keyboard-enabled, an `OnKeyDown`
        // that answers ESCAPE — consumed ESC and let W straight through, and the player walked
        // around underneath their own intro cinematic.
        //
        // The reference's own Lua is the proof this is the law and not an over-reading: that
        // handler has to call `RunBinding("SCREENSHOT")` **by hand** to get one key back. It would
        // not need to if unhandled keys fell through to their bindings.
        //
        // Consumption suppresses THE KEY'S BINDING and nothing else. It deliberately does NOT
        // suppress `char_input` below — `OnChar` is a separate channel off a separate dispatcher
        // (`0x765df0`), which is exactly how the stack-split spinner receives a digit whose
        // key-down its own `OnKeyDown` already ate. And it is emphatically not a focus change: it
        // releases nothing that is already held (2196).
        else if named.is_none() {
            if let Some(token) = crate::bindings::chord::key_token(ev.key_code) {
                if script.frame_key_input(token) {
                    capture.consumed.push(ev.key_code);
                }
            }
        }
        if let Some(name) = named {
            // The three box-event keys. An unconsumed press is not acted on here: every GAME
            // action of a key — ESCAPE's close/cancel ladder (TOGGLEGAMEMENU), TAB's targeting,
            // ENTER's chat open, the number row's action buttons — now lives in the binding
            // dispatch (decision 0997, `crate::bindings`), which runs right after this pass and
            // reads the capture gate written above, so a key a focused box consumed this frame
            // never also fires its binding — the real client's ESC precedence, table-wide.
            // Consumption suppresses this key's binding — a keyboard FRAME that took the key
            // counts as the focused box does for *that key* (decision 1319; `capture.typing` above
            // covers the box, which takes every key for as long as it holds focus, and this adds
            // the frame's one-key case).
            if script.key_input(name) {
                capture.consumed.push(ev.key_code);
            }
        } else if let Some(chord) = chord {
            // The gate, on the KEY rather than the action: a gated arrow is not handed to the box
            // at all, which is the reference's `return 0`. It must not be turned into an
            // `EditAction` first — `Move { unit: Edge }` reaches the box from HOME/END as well as
            // from an arrow, and only the arrow is declined.
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
                // The clipboard trio needs the OS pasteboard, so it resolves here against the held
                // [`HostClipboard`]: copy/cut pull the selection out of the box
                // (`0x77e1d0` — selection required; a password box yields its mask run, never the
                // real text), paste sanitizes+inserts.
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
            // Plain character input. Command-modified chars never insert (an unbound Cmd/Ctrl
            // chord like Cmd+L must not type "l") — but Ctrl+Alt passes: that's AltGr, the char
            // plane European layouts type real text with (and macOS Option+letter comes through
            // with only `alt`, composing its special characters).
            if let Some(text) = &ev.text {
                // Same suppression as the named keys, and this is the case that bites: the number
                // row IS a binding (the action buttons), so a digit typed into a keyboard frame —
                // the stack-split spinner — must not also fire action button 3 (decision 1319).
                if script.char_input(text) {
                    capture.consumed.push(ev.key_code);
                }
            }
        }
    }

    // The payload-held mirror, written LAST (after the mouse + keyboard feeds, so a same-frame
    // pickup or an ESC clear is already reflected) — the Send-side view the world-click
    // consumers read (decision 0571's no-payload-gated deselect).
    payload_held.0 = script.cursor_payload().is_some();

    for err in script.take_errors() {
        warn!("ui_script(input): {err}");
    }
}

// The two hardcoded key tables that used to live here — the ACTIONBUTTON number row and the
// bare-key window toggles — moved into the command registry (`crate::bindings::commands`,
// decision 0997): one chord→command table, rebindable, with 0585's modifier law enforced once
// in the dispatch instead of per branch here.

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::mouse::MouseScrollUnit;

    /// A bare VM with one wheel-enabled pane at the screen's centre whose `OnMouseWheel` counts
    /// its calls and sums its `arg1`s.
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

    /// **A gentle trackpad swipe is not a notch a frame.** Ten frames of a small `Pixel` delta —
    /// a tenth of a line apiece at Bevy's factor, one line in total — must fire the pane's
    /// `OnMouseWheel` at most the ONE notch they add up to. Fed raw, it fired ten times, and a
    /// stock `ScrollFrameTemplate_OnMouseWheel` moves half a pane per call off the sign alone.
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

    /// A mouse wheel's notch arrives as one `Line` delta and fires exactly once, `arg1 = 1`.
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
