//! The player-UI input pass: [`feed_ui_input`] feeds mouse and keyboard events into the UI engine
//! ahead of the frame's packets and its tick, against the rects the last frame's paint resolved.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{ClickPress, KeyWalk, PointerTarget, UiScript, WheelWalk};

use super::{PlayerUiClickConsumed, PlayerUiHover, PlayerUiPressConsumed, UiKeyboardCapture};
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
    /// A headless probe drives the pointer ([`super::SyntheticPointer`]), not the real cursor.
    synthetic: Res<'w, super::SyntheticPointer>,
    /// A capture owns the pointer ([`super::CapturePointerPinned`]): no OS cursor in the shot.
    capture_pinned: Res<'w, super::CapturePointerPinned>,
    egui_keyboard: Res<'w, super::EguiKeyboard>,
    /// The dev overlay holds the pointer: a press then is no frame's and not the world's.
    egui_pointer: Option<Res<'w, super::EguiPointerOver>>,
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

/// The world frame's wheel slot as the wheel walk meets it (`0x483d20`).
pub(crate) trait WorldWheelSlot {
    /// Whether the slot takes a notch up (`up`) or down: the notch's chord has a binding, so the
    /// slot answers nonzero (`0x483d87`) and the walk stops there.
    fn takes(&self, up: bool) -> bool;
    /// The notch's binding, run inside the walk.
    fn run(&mut self, script: &mut UiScript, up: bool);
}

/// One wheel record, one step up (`up`) or down: the sweep `0x7664f0`, `arg1 = ±1` (the stock
/// handlers act on the sign alone, `ScrollFrameTemplate_OnMouseWheel` scrolls half a pane per call,
/// `UIPanelTemplates.lua:150`), and, when no frame took it, the world frame's binding at its place
/// in the walk. The sweep is at the cursor the record was queued at (UI units), which is the last
/// move's; with none (the cursor off the window, a probe's pointer) it is at the cursor the last
/// mouse event left, as the reference walks at its saved cursor (`+0x1118`, `0x766524`).
pub(crate) fn feed_wheel(
    script: &mut UiScript,
    at: Option<Vec2>,
    up: bool,
    world: &mut impl WorldWheelSlot,
) {
    let at = at.unwrap_or_else(|| Vec2::from(script.saved_cursor()));
    let sign = if up { 1.0 } else { -1.0 };
    if script.mouse_wheel_bound(at.x, at.y, sign, world.takes(up)) == WheelWalk::World {
        world.run(script, up);
    }
}

/// The world frame's wheel slot of one record: its binding with the record's modifiers, in the
/// world only, and none while the egui dev overlay holds the pointer.
struct WorldSlot<'a, 'w> {
    frame: &'a mut crate::bindings::WorldFrameBindings<'w>,
    mods: keymap::Mods,
    live: bool,
}

impl WorldWheelSlot for WorldSlot<'_, '_> {
    fn takes(&self, up: bool) -> bool {
        self.frame.takes_wheel(self.mods, up)
    }

    fn run(&mut self, script: &mut UiScript, up: bool) {
        if self.live {
            self.frame.wheel(Some(script), self.mods, up);
        }
    }
}

/// Where the frame's walk of its records stands: whether it began, the next record, the point the
/// hover was last walked at, the record it stopped after for that record's world steps
/// ([`crate::target::world_steps`]), and a right press it stopped ahead of the binding of, for the
/// world frame's hook ([`crate::target::right_press_hook`]): the press's record and whether a
/// frame took it; and whether the frame's records came in relative mode.
#[derive(Resource, Default)]
pub(crate) struct RecordWalk {
    begun: bool,
    /// The camera was in freelook as the frame's records were queued: the window posts the mouse
    /// as deltas then (`0x42d31c` → `0x42d34a`, category 0xd), which `0x766250` hands the world
    /// frame's hook without a hover walk, so no record moves the hover; only the re-pick flag
    /// `[root+0x1100]` does.
    relative: bool,
    next: usize,
    walked: Option<Vec2>,
    stopped: Option<super::InputRecord>,
    hooked: Option<(usize, bool)>,
    hook_due: bool,
}

/// Dispatch the frame's input records, each whole and in arrival order (`0x423920` → `0x4239a0` →
/// `0x4245b0`): the interface's walk of a record and the world frame's binding
/// ([`feed_ui_input`]), whose mouse commands apply the world click inside their calls, then the
/// world's steps the record made ([`crate::target::world_steps`]), before the next record. The
/// walk stops after each button record and after any record whose binding made a click step, and
/// goes on once the world's steps are done; what no command dispatched runs where the records end
/// ([`crate::target::run_world_clicks`]).
/// A right press stops ahead of its binding as well, for the world frame's hook, and goes on at
/// the binding.
///
/// The input records' world mouse ([`crate::player::WorldMouse`]) follows: the look session, which
/// adds the armed click's travel, and the command tick.
pub(crate) fn dispatch_input_records(
    world: &mut World,
    mut feed: Local<Option<bevy::ecs::system::SystemId>>,
) {
    dispatch(world, &mut feed, true);
}

/// [`dispatch_input_records`] without the legs' last run, for a test of the walk over a world
/// that holds the interface's state alone.
#[cfg(test)]
pub(super) fn dispatch_records_alone(
    world: &mut World,
    mut feed: Local<Option<bevy::ecs::system::SystemId>>,
) {
    dispatch(world, &mut feed, false);
}

/// The dispatch, the legs' last run when `legs`.
fn dispatch(world: &mut World, feed: &mut Option<bevy::ecs::system::SystemId>, legs: bool) {
    let id = *feed.get_or_insert_with(|| world.register_system(feed_ui_input));
    *world.get_resource_or_init::<RecordWalk>() = RecordWalk::default();
    crate::target::begin_records(world);
    loop {
        if let Err(err) = world.run_system(id) {
            debug!("ui_script(input): the walk did not run: {err}");
            return;
        }
        crate::creature_anim::apply_unit_edits(world);
        let mut walk = world.resource_mut::<RecordWalk>();
        if std::mem::take(&mut walk.hook_due) {
            let (_, over_ui) = walk.hooked.unwrap_or_default();
            crate::target::right_press_hook(world, over_ui);
            continue;
        }
        let Some(record) = walk.stopped.take() else {
            break;
        };
        crate::target::world_steps(world, &record);
    }
    if legs {
        crate::target::run_world_clicks(world);
    }
}

/// The keyboard and the pasteboard the feed reads and writes, as one param.
type KeyboardFeed<'w> = (
    Res<'w, ButtonInput<KeyCode>>,
    ResMut<'w, UiKeyboardCapture>,
    NonSendMut<'w, HostClipboard>,
);

/// Dispatches the frame's input records ([`super::InputRecords`]) into the UI engine from where
/// [`RecordWalk`] stands, each whole and in arrival order, as the reference drains its ring
/// (`0x423920` → `0x4239a0` → `0x4245b0`):
/// the record's modifiers set, the hover walked where the record was queued (a move's whole
/// handler, category 0xc, `0x7660d0`), the button's, the key's or the wheel's walk (category 0x10,
/// `0x7664f0`), then, in the world, the world frame's binding
/// ([`crate::bindings::WorldFrameBindings`]), all before the next record. Publishes
/// [`PlayerUiHover`] (the pointer arbiter yields to the UI) and [`UiKeyboardCapture`]. It returns
/// after a record that leaves the world steps to take ([`dispatch_input_records`]), and the next
/// run goes on from the record after it.
#[allow(clippy::too_many_arguments)]
pub(super) fn feed_ui_input(
    mut script: Option<NonSendMut<UiScript>>,
    // Carries the `wl_display` the Wayland clipboard backend needs, once winit has a surface.
    window: Query<(&Window, Option<&bevy::window::RawHandleWrapper>), With<PrimaryWindow>>,
    records: Res<super::InputRecords>,
    mut pointer: PointerFeed,
    mut kbd: KeyboardFeed,
    // The keys this feed believes down, a fact about the keyboard and not the VM.
    mut pressed: Local<crate::bindings::KeyDownList>,
    // Who sends the key-ups: the world frame's binding leg in the world, this feed on the glue
    // screens, which have none.
    state: Option<Res<State<crate::client_state::ClientState>>>,
    // The characters the active layout makes, which name a key for a keyboard frame.
    layout: Res<crate::bindings::LayoutNames>,
    // Lent to the VM for the pass, so a click's, a key's or a binding's script calls apply in the
    // call, the world click's arm picking inside it.
    mut host: crate::script_calls::InputHost,
    mut world_frame: crate::bindings::WorldFrameBindings,
    mut walk: ResMut<RecordWalk>,
) {
    let (keys, capture, clipboard) = (&kbd.0, &mut kbd.1, &mut kbd.2);
    // The frame's first run: what the frame's records write starts clear.
    let first = !std::mem::replace(&mut walk.begun, true);
    let in_world = state
        .as_ref()
        .is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
    let releases = !in_world;
    let world_pick = pointer.world_pick();
    // The OS pointer is not ours while a probe drives a gesture through the real pointer path or a
    // capture pins it: skip the mouse half whole, else-arm included, whose `pointer_left_window`
    // would disarm the probe's gesture between its press and release.
    let synthetic = pointer.synthetic.0 || pointer.capture_pinned.0;
    // egui's keyboard focus is the last pass's: a key in the very frame a click focuses an egui
    // field still reaches the game.
    let egui_keyboard = pointer.egui_keyboard.0;
    let egui_pointer = pointer.egui_pointer.as_ref().is_some_and(|e| e.0);
    let (hover, click_consumed, press_consumed) = (
        &mut pointer.hover,
        &mut pointer.click_consumed,
        &mut pointer.press_consumed,
    );
    if first {
        click_consumed.0 = false;
        press_consumed.0 = false;
        capture.consumed.clear();
        if let Some(script) = script.as_mut() {
            script.take_click_stepped();
        }
    }
    let start = walk.next;
    walk.next = records.iter().count();
    let window = window.single().ok();
    let (mut script, (window, raw_handle)) = match (script, window) {
        (Some(script), Some(window)) => (script, window),
        (script, _) => {
            // No VM or no window: no walk, and the world frame's binding meets every record, the VM's
            // bodies running when there is one.
            capture.typing = false;
            capture.arrows_fall_through = false;
            if in_world {
                let over_ui = hover.0.is_some() || egui_pointer;
                let mut legs = |mut s: Option<&mut UiScript>| {
                    for r in records.iter().skip(start) {
                        // No sweep: every notch is the world's, unless the dev overlay holds the
                        // pointer.
                        if matches!(r.record, super::Record::Wheel { .. }) && egui_pointer {
                            continue;
                        }
                        let verdict = crate::bindings::KeyVerdict::default();
                        world_frame_leg(
                            &mut world_frame,
                            s.as_deref_mut(),
                            r,
                            verdict,
                            reach(r, over_ui, PointerTarget::Nothing),
                            &layout,
                        );
                    }
                };
                match script {
                    Some(mut script) => host.lend(&mut script, |s| legs(Some(s))),
                    None => legs(None),
                }
            }
            return;
        }
    };
    let wl_display = textinput::wayland_display(raw_handle);
    // The interface's screen on the window, 4:3 and stretched with `widescreen` 0
    // ([`super::UiScreen`]).
    let widescreen = host.widescreen();
    let screen = super::UiScreen::of_window(window, widescreen.as_ref());
    // The window cursor in the root's y-up 768-high units, `GetCursorPosition`'s answer. The
    // headless hover probe's aim stands in for a missing cursor, so `PointerOverUi` rises and falls
    // over a panel in an automated run as for a person; a real pointer wins.
    let ui_point = |at: Option<Vec2>| {
        at.or_else(crate::target::hover_probe_point)
            .filter(|_| !synthetic)
            .map(|c| super::window_to_ui(screen, c))
    };
    let plane = keymap::Mods {
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        sup: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    };
    let end = ui_point(window.cursor_position());
    let walk = &mut *walk;
    let feed = |script: &mut UiScript| {
        // The pick routes the world drop: an object keeps the payload, terrain or sky drops it.
        script.set_world_pick(world_pick);
        if first {
            pressed.reconcile(keys);
            walk.walked = None;
            walk.relative = script.freelook_now().unwrap_or(false);
        }
        let relative = walk.relative;
        // The point the hover was last walked at this pass: a move to it again is ignored, as
        // `0x7660d0` ignores a move to the stored position.
        let walked = &mut walk.walked;
        let mut move_to = |script: &mut UiScript, hover: &mut PlayerUiHover, at: Vec2| {
            if *walked != Some(at) {
                *walked = Some(at);
                hover.0 = hover_walk(script, at);
            }
        };
        capture.typing = script.has_keyboard_focus() || egui_keyboard;
        for (i, r) in records.iter().enumerate().skip(start) {
            // A right press the walk stopped ahead of the binding of goes on at the binding, its
            // interface's walk done.
            let resumed = walk
                .hooked
                .filter(|(at, _)| *at == i)
                .map(|(_, over_ui)| over_ui);
            // The record's modifiers, as its kind handler left the mask (`0x423d06`), so a
            // click's `IsShiftKeyDown` sees its own state.
            let mods = r.mods;
            script.set_modifiers(mods.shift, mods.ctrl, mods.alt);
            script.set_alt_down(mods.alt);
            let at = ui_point(r.at);
            // The cursor an arm the record's walk or binding makes picks at: the last mouse event's
            // (`[frame+0xa0]+0x1118`, `0x481f1d`), the record's own.
            script.set_click_press(click_press(r, false));
            // The hover the record meets is the cursor's when it was queued: the moves before it
            // are records of their own (category 0xc, `0x7660d0`). In relative mode there are none.
            if let Some(at) = at.filter(|_| !relative) {
                move_to(script, hover, at);
            }
            let mut verdict = crate::bindings::KeyVerdict::default();
            // The world frame's binding leg, unless the record's walk ran it.
            let mut leg = true;
            // Whose the record's press is: a hovered frame's, the interface's when it consumed the
            // press, the dev overlay's, else the world frame's, whose binding runs.
            let mut over_ui = hover.0.is_some() || egui_pointer;
            // Where a release went, which picks its binding's up-half; a press's is `over_ui`.
            let mut released_to = PointerTarget::Nothing;
            match &r.record {
                super::Record::Button { .. } if resumed.is_some() => {
                    over_ui = resumed.unwrap_or(over_ui);
                }
                super::Record::Button { button, state } => {
                    if let (Some(at), Some(name)) = (at, ui_button_name(*button)) {
                        let down = *state == ButtonState::Pressed;
                        // A left press whose release the payload takes (`world_press_consumed`:
                        // an item, a spell or an action over terrain, any payload over nothing)
                        // is consumed now: the drop fires on the release, but the world
                        // click-pick and camera orbit act on the press. A vendor row over terrain
                        // or an object is put down and the click goes on, to the ground click or
                        // the select (`0x492c90`, `0x492ce0`).
                        if down
                            && *button == MouseButton::Left
                            && hover.0.is_none()
                            && script.world_press_consumed()
                        {
                            click_consumed.0 = true;
                        }
                        // The event is the world frame's as its target is, not the hover's: a
                        // press goes to the frame holding a button, else the hover (`0x76637f`),
                        // and in freelook with none held to the world frame (`0x492bd9`); a
                        // release to the capture, or the world frame in freelook (`0x492be5`).
                        let target = script.mouse_button_to(at.x, at.y, name, down);
                        let took = target == PointerTarget::Ui;
                        if down {
                            press_consumed.0 |= took;
                            over_ui = took || egui_pointer;
                        } else {
                            released_to = target;
                        }
                    } else if *button == MouseButton::Right && *state == ButtonState::Pressed {
                        // No point to walk at: the world input hook still runs ahead of the world
                        // frame (`0x492b50`).
                        let took = script.hook_right_press();
                        press_consumed.0 |= took;
                        over_ui |= took;
                    }
                }
                // The walk above was the move's handler.
                super::Record::Move => {}
                super::Record::Wheel { up } => {
                    let mut slot = WorldSlot {
                        frame: &mut world_frame,
                        mods,
                        live: in_world && !egui_pointer,
                    };
                    script.set_click_press(click_press(r, click_consumed.0));
                    feed_wheel(script, at, *up, &mut slot);
                    script.set_click_press(ClickPress::default());
                    // Its world frame's leg ran inside the walk.
                    leg = false;
                }
                super::Record::Key(ev) => {
                    // ── Keyboard capture gate ── read after the records before it (a click may
                    // have just focused a box) and before this key (an Escape that clears focus
                    // still counts as captured).
                    capture.typing = script.has_keyboard_focus() || egui_keyboard;
                    // ── The alt-arrow exemption ── An EditBox in alt-arrow mode (`ignoreArrows`,
                    // set on the chat box at `ChatFrame.xml:21`) passes the arrows on unless ALT
                    // is held: its handler returns 0 at `0x77b1c4` and the binding runs at
                    // `CGWorldFrame`; a focused box swallows any other key (`0x77b35e` returns
                    // 1), PRINTSCREEN excepted (`0x77b17f`, see `crate::bindings`).
                    capture.arrows_fall_through =
                        !mods.alt && script.editbox_alt_arrow_mode() && !egui_keyboard;
                    // ── Keyboard → VM ── a press is one key-down walk by name (`0x765f10`), in
                    // which a box is a frame like the rest; a box the walk reached then takes the
                    // key's editing action through the per-OS chord table as an `EditAction` or a
                    // clipboard operation (the macOS NSPasteboard needs this main-thread system),
                    // and the characters go to `char_input`, a walk of its own. A repeat goes to
                    // the frame that took the press instead (`0x766070`), a release likewise
                    // (`0x765fd0`).
                    let walk = if reaches_ui(ev, egui_keyboard) {
                        feed_key(
                            script,
                            capture,
                            ev,
                            &mut pressed,
                            releases,
                            mods,
                            &layout,
                            clipboard,
                            wl_display,
                        )
                    } else {
                        KeyWalk::Declined
                    };
                    verdict = crate::bindings::KeyVerdict {
                        typing: capture.typing,
                        arrows_fall_through: capture.arrows_fall_through,
                        eaten: walk != KeyWalk::Declined,
                    };
                }
            }
            // A right press the world frame may take stops here for its hook, which runs ahead of
            // the binding (`0x483c62`, `0x483c93`). A right press the world input hook ate
            // (`0x492b50`) never reaches the world frame, in a look session or not.
            let right = matches!(
                r.record,
                super::Record::Button {
                    button: MouseButton::Right,
                    state: ButtonState::Pressed,
                }
            );
            if in_world && right && resumed.is_none() && !script.press_hooked() {
                walk.next = i;
                walk.hooked = Some((i, over_ui));
                walk.hook_due = true;
                return;
            }
            // The world frame, last in the walk, inside the same record.
            if in_world && leg {
                script.set_click_press(click_press(r, click_consumed.0));
                world_frame_leg(
                    &mut world_frame,
                    Some(&mut *script),
                    r,
                    verdict,
                    reach(r, over_ui, released_to),
                    &layout,
                );
            }
            // The world's steps of a button record, or of one whose binding made a click step,
            // run before the next record: the walk returns to its driver here.
            let button = matches!(r.record, super::Record::Button { .. });
            let stepped = script.take_click_stepped();
            if in_world && (button || stepped) && i + 1 < records.iter().count() {
                walk.next = i + 1;
                walk.stopped = Some(r.clone());
                return;
            }
            if in_world && (button || stepped) {
                walk.stopped = Some(r.clone());
            }
        }
        script.set_click_press(ClickPress::default());
        // The modifiers as the frame's records leave them, which the tick's scripts read, and the
        // keyboard's owner as they leave it, which the frame's later readers take.
        script.set_modifiers(plane.shift, plane.ctrl, plane.alt);
        script.set_alt_down(plane.alt);
        capture.typing = script.has_keyboard_focus() || egui_keyboard;
        capture.arrows_fall_through =
            !plane.alt && script.editbox_alt_arrow_mode() && !egui_keyboard;
        // ── The hover after the frame's records ── at the window's cursor, so a frame that moved
        // under a still cursor is re-picked. A cursor off the window leaves the hovered frame once
        // and disarms any press or drag every frame, since no release ends it and a stale gesture
        // would fire on re-entry. In freelook the cursor is the camera's, and the hover stands.
        let looking = relative || script.freelook_now().unwrap_or(false);
        match end {
            _ if looking => {}
            Some(end) => move_to(script, hover, end),
            None if !synthetic => {
                if hover.0.take().is_some() {
                    script.pointer_left_hover();
                }
                script.pointer_left_window();
            }
            None => {}
        }

        for err in script.take_errors() {
            warn!("ui_script(input): {err}");
        }
    };
    host.lend(&mut script, feed);
}

/// The press `r` hands the mouse commands its binding runs: its point, and whether the frame's
/// payload drop consumed a left press (`eaten`).
fn click_press(r: &super::InputRecord, eaten: bool) -> ClickPress {
    ClickPress {
        at: r.at.map(<[f32; 2]>::from),
        eaten,
    }
}

/// The world frame's leg of one record: its binding, with what the interface's walk left. A move
/// runs no binding; a wheel turn here, with no walk to sweep it, is the world's whole.
fn world_frame_leg(
    world_frame: &mut crate::bindings::WorldFrameBindings,
    script: Option<&mut UiScript>,
    r: &super::InputRecord,
    verdict: crate::bindings::KeyVerdict,
    reach: PointerTarget,
    layout: &crate::bindings::LayoutNames,
) {
    match &r.record {
        super::Record::Key(ev) => world_frame.key(script, ev, r.mods, verdict, layout),
        super::Record::Button { button, state } => {
            world_frame.button(script, *button, *state, r.mods, reach)
        }
        super::Record::Move => {}
        super::Record::Wheel { up } => world_frame.wheel(script, r.mods, *up),
    }
}

/// Where a button record went, for its binding's leg: a press the interface's when `over_ui`, else
/// the world frame's; a release where the walk sent it, `released_to`.
fn reach(r: &super::InputRecord, over_ui: bool, released_to: PointerTarget) -> PointerTarget {
    match r.record {
        super::Record::Button {
            state: ButtonState::Released,
            ..
        } => released_to,
        _ if over_ui => PointerTarget::Ui,
        _ => PointerTarget::World,
    }
}

/// The name a button reaches the UI's handlers by, `None` for one 1.12 has no name for.
fn ui_button_name(button: MouseButton) -> Option<&'static str> {
    UI_MOUSE_BUTTONS
        .iter()
        .find(|(b, _)| *b == button)
        .map(|(_, name)| *name)
}

/// The hover pick at `at` (`0x7660d0`): the world frame is mouse-enabled, so an addon's handlers can
/// hover it, but its hit is the world's: camera look, world clicks and hover targeting stay live
/// over it.
fn hover_walk(script: &mut UiScript, at: Vec2) -> Option<u32> {
    // `WOW_HIT_COST=1` meters this call's hit-test rebuild, one line a second.
    static HIT_COST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let metering = *HIT_COST.get_or_init(|| std::env::var("WOW_HIT_COST").as_deref() == Ok("1"));
    let t0 = metering.then(std::time::Instant::now);
    let hovered = script
        .mouse_move(at.x, at.y)
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
                "[hit-cost] mouse_move_us/call={:.1} calls={n}",
                us as f64 / n.max(1) as f64
            );
        }
    }
    hovered
}

/// Whether a key event reaches the UI while a dev overlay's text field may hold the keyboard: none
/// of the presses, which are egui's, and every release, so a key held into that focus still ends.
pub(crate) fn reaches_ui(ev: &KeyboardInput, egui_keyboard: bool) -> bool {
    !egui_keyboard || ev.state != ButtonState::Pressed
}

/// [`reaches_ui`] over a batch of events.
#[cfg(test)]
pub(crate) fn past_egui<'a>(
    events: impl Iterator<Item = &'a KeyboardInput>,
    egui_keyboard: bool,
) -> impl Iterator<Item = &'a KeyboardInput> {
    events.filter(move |ev| reaches_ui(ev, egui_keyboard))
}

/// [`feed_key`] over a batch of events, after the pressed-key reconcile, as the feed runs a frame's
/// key records.
#[cfg(test)]
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
    for ev in events {
        feed_key(
            script, capture, ev, down, releases, mods, layout, clipboard, wl_display,
        );
    }
}

/// One key record's walk: the key-down or repeat walk and what a box the walk reached does with
/// the key, or the release. `down` is this feed's pressed-key list, which decides repeat as the
/// bindings' does (`0x4248b3`), never from the OS flag: after an alt-tab wipes it (`0x424790`, the
/// reconcile against the key plane) the first repeat is a fresh press, and a platform that never
/// sets the flag still repeats right. `releases` says whether the key-up dispatch is this feed's:
/// in the world the world frame's binding leg sends it, after the binding's release
/// (`crate::bindings`). Answers what took the press, [`KeyWalk::Declined`] for a release.
#[allow(clippy::too_many_arguments)]
pub(crate) fn feed_key(
    script: &mut UiScript,
    capture: &mut UiKeyboardCapture,
    ev: &KeyboardInput,
    down: &mut crate::bindings::KeyDownList,
    releases: bool,
    mods: keymap::Mods,
    layout: &crate::bindings::LayoutNames,
    clipboard: &mut HostClipboard,
    wl_display: Option<*mut std::ffi::c_void>,
) -> KeyWalk {
    let keymap::Mods {
        shift: _,
        ctrl,
        alt,
        sup,
    } = mods;
    let mac = cfg!(target_os = "macos");
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
        return KeyWalk::Declined;
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
    // gave an `autoFocus` box the focus runs no binding. An unconsumed press goes on to the
    // world frame's binding (`crate::bindings::WorldFrameBindings`) in the same record.
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
        return walk;
    }
    if named {
        return walk;
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
        //
        // Deviation: the character is the platform's Unicode text, so a player can type any
        // language on any platform.
        // The reference's window is ANSI (`RegisterClassExA`, `DefWindowProcA`), so `WM_CHAR`
        // carries a byte of the system code page, which `0x42d185`-`0x42d1f5` maps through a
        // table `GetACP` picks; a character outside that code page never arrives.
        if let Some(text) = &ev.text {
            script.char_input(text);
        }
    }
    walk
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A world frame's slot that binds `binds` and counts the notches it ran, up positive.
    struct Slot {
        binds: fn(bool) -> bool,
        ran: i32,
    }

    impl WorldWheelSlot for Slot {
        fn takes(&self, up: bool) -> bool {
            (self.binds)(up)
        }
        fn run(&mut self, _: &mut UiScript, up: bool) {
            self.ran += if up { 1 } else { -1 };
        }
    }

    /// One wheel record up (`up`) or down at `(x, y)`; the steps the world frame ran.
    fn feed(s: &mut UiScript, (x, y): (f32, f32), up: bool) -> i32 {
        let mut slot = Slot {
            binds: |_| true,
            ran: 0,
        };
        feed_wheel(s, Some(Vec2::new(x, y)), up, &mut slot);
        slot.ran
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

    /// A world frame over the screen, a button with the mouse and no wheel, and a pane that takes
    /// the wheel on the right: the three places a notch can land.
    fn world_with_a_button_and_a_pane() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.run(
            r#"
            local w = CreateFrame("WorldFrame", "WorldFrame")
            w:SetPoint("BOTTOMLEFT", 0, 0) w:SetWidth(1024) w:SetHeight(768)
            local b = CreateFrame("Button", "ActionButton")
            b:SetPoint("BOTTOMLEFT", 100, 100) b:SetWidth(40) b:SetHeight(40)
            local p = CreateFrame("Frame", "Pane")
            p:SetPoint("BOTTOMLEFT", 600, 100) p:SetWidth(100) p:SetHeight(100)
            p:EnableMouseWheel(true)
            p:SetScript("OnMouseWheel", function() WheelCalls = (WheelCalls or 0) + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s
    }

    /// A notch over a button with no `OnMouseWheel` is the world's (`0x76c1cb`, `0x483d20`); over
    /// a pane that has one it is the pane's and the world gets none (`0x76659a`).
    #[test]
    fn a_notch_reaches_the_world_unless_a_frame_with_a_handler_takes_it() {
        let mut s = world_with_a_button_and_a_pane();
        assert_eq!(feed(&mut s, (120.0, 120.0), true), 1);
        assert_eq!(feed(&mut s, (120.0, 120.0), false), -1);
        assert_eq!(calls(&s).0, 0, "the button has no handler");
        assert_eq!(feed(&mut s, (650.0, 150.0), true), 0);
        assert_eq!(calls(&s).0, 1, "the pane's handler took it");
        assert_eq!(feed(&mut s, (900.0, 600.0), true), 1);
    }

    /// A chord with no binding is not taken by the world slot (`0x483d87`): the notch is nobody's.
    #[test]
    fn a_notch_whose_chord_is_unbound_is_not_left_to_the_world() {
        let mut s = world_with_a_button_and_a_pane();
        let mut slot = Slot {
            binds: |up| !up,
            ran: 0,
        };
        feed_wheel(&mut s, Some(Vec2::new(120.0, 120.0)), true, &mut slot);
        assert_eq!(slot.ran, 0, "up is unbound");
        feed_wheel(&mut s, Some(Vec2::new(120.0, 120.0)), false, &mut slot);
        assert_eq!(slot.ran, -1, "down is bound");
    }

    /// With no cursor for the record (off the window, a probe's pointer) the sweep is at the cursor
    /// the last mouse event left (`+0x1118`, `0x766524`): over the pane, the pane's; over the
    /// button, the world's.
    #[test]
    fn a_wheel_with_no_cursor_sweeps_at_the_last_one() {
        let mut s = world_with_a_button_and_a_pane();
        let mut slot = Slot {
            binds: |_| true,
            ran: 0,
        };
        s.mouse_move(650.0, 150.0);
        feed_wheel(&mut s, None, true, &mut slot);
        assert_eq!(
            (slot.ran, calls(&s).0),
            (0, 1),
            "the pane under the saved cursor"
        );
        s.mouse_move(120.0, 120.0);
        feed_wheel(&mut s, None, true, &mut slot);
        assert_eq!(
            (slot.ran, calls(&s).0),
            (1, 1),
            "the world's, over the button"
        );
    }

    #[test]
    fn a_line_notch_fires_once() {
        let mut s = wheel_pane();
        feed(&mut s, (512.0, 384.0), true);
        assert_eq!(calls(&s), (1, 1.0));
        feed(&mut s, (512.0, 384.0), false);
        assert_eq!(calls(&s), (2, 0.0), "and the other way is a notch down");
    }
}
