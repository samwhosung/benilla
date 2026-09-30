//! Key bindings: the one chord-to-command engine every rebindable input runs through. The key
//! table and the commands live engine-side ([`benilla_ui::script::keybind`]): the commands are the
//! `Bindings.xml` rows the core and each addon loaded, the live keys what `SetBinding` and the Key
//! Bindings window write. This module derives an exact-match dispatch map from the keys whenever
//! the table's generation moves, and runs a pressed chord's command.
//!
//! Dispatch ([`latch_and_dispatch`], in [`crate::ui_script::UiInput`] after the UI key feed), as
//! `CBindings::ExecuteBinding` (`0x4b7990`) and `RunCommand` (`0x4b7b50`) do:
//! - a press probes its exact chord, then once more with its leftmost modifier dropped
//!   ([`Chord::fallback`]); Super held matches nothing;
//! - the resolved command's body runs with `keystate = "down"`, and the base key's release runs a
//!   `runOnUp` body again with `"up"`. UI focus suppresses presses and releases nothing already
//!   held; a latch ends only on its base key's release, the stuck-latch sweep (OS focus loss, the
//!   loading cover) or a VM swap;
//! - what the bodies call reaches the engine through the Lua binding functions
//!   ([`benilla_ui::script::BindingInput`]) as [`BindingsState`]'s [`Input`]s.
//!
//! Persistence: `benilla-config/bindings/account.txt` and `<Realm>-<Char>.txt` ([`store`]); the
//! character file's existence is the character-set state, as in the reference.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_ui::script::keybind::KeybindRequest;
use benilla_ui::script::UiScript;
use benilla_world::layout_keys::LayoutChars;

use crate::char_select::InWorldGated;
use crate::ui_script::{PlayerUiHover, PointerOverUiPanel, UiKeyboardCapture};

pub(crate) mod chord;
mod script_input;
mod store;

use chord::{BindKey, Chord};
pub(crate) use script_input::Input;

/// The chord-to-command map, rebuilt whenever the engine table's generation moves. Probe it
/// through [`BindingDispatch::resolve`]: the lookup is two probes.
#[derive(Resource, Default)]
struct BindingDispatch {
    map: std::collections::HashMap<Chord, String>,
    /// The engine table's generation this map was built from, keyed per VM: a fresh VM restarts
    /// its counter at 0.
    seen_generation: crate::ui_script::VmMemo<Option<u64>>,
}

impl BindingDispatch {
    /// Resolve a press as the reference's `CBindings::ExecuteBinding` (`0x4b7990`) does: the exact
    /// chord, then one retry with the leftmost modifier dropped ([`Chord::fallback`]).
    ///
    /// Deviation, dev builds only: `dev_plane` suppresses the keyboard retry, because
    /// `Ctrl`+`Shift` is the dev overlays' plane ([`benilla_world::modkeys::DEV_CHORD`]) and
    /// `Ctrl`+`Shift`+`P` would otherwise fall back to `SHIFT-P` (`TOGGLECHARACTER3`). An exact
    /// `CTRL-SHIFT-` binding still dispatches; mouse and wheel are never suppressed.
    fn resolve(&self, chord: Chord, dev_plane: bool) -> Option<&str> {
        if let Some(command) = self.map.get(&chord) {
            return Some(command);
        }
        if dev_plane {
            return None;
        }
        self.map.get(&chord.fallback()?).map(String::as_str)
    }
}

/// This frame's binding activity, which engine-side consumers read instead of raw keys: the
/// [`Input`]s the running bodies' Lua calls set.
#[derive(Resource, Default)]
pub(crate) struct BindingsState {
    /// Live latches, (base input, command): a press not yet released, whose release runs the
    /// command's `runOnUp` half. The reference replays the press-time chord at key-up (`0x483bd0`);
    /// latching the resolved command is equivalent.
    latched: Vec<(Held, String)>,
    /// The held bits a Start set and no Stop has cleared.
    held: Vec<Input>,
    /// Inputs whose Start, or whose one-shot, came this frame (the press edge).
    just: Vec<Input>,
    /// One-shots fired this frame.
    fired: Vec<Input>,
    /// Amount per one-shot this frame: a zoom's yards, else 1.0.
    amounts: Vec<(Input, f32)>,
    /// The keys we believe are down: a press of one is a repeat. The reference classifies
    /// auto-repeat off its own list (`0x4248b3`), never the Win32 `lParam` repeat bit.
    /// Normalized like [`latched`](Self::latched) and reconciled against `ButtonInput` every
    /// pass, so a window deactivate empties it and a key held across an alt-tab resumes, as in
    /// the reference. Keyboard only: mouse edges cannot repeat.
    down: Vec<KeyCode>,
}

/// The physical input a latch waits on: a keyboard key by its code, not its name, so its release
/// ends the latch whatever the layout names it by then.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Held {
    Key(KeyCode),
    Mouse(MouseButton),
    Wheel,
}

impl BindingsState {
    /// Is a held bit set right now (the reference's `[InputControl+4]`)?
    pub(crate) fn pressed(&self, i: Input) -> bool {
        self.held.contains(&i)
    }
    /// Did this bit's Start, or this one-shot, come this frame (the key-down edge)?
    pub(crate) fn just_pressed(&self, i: Input) -> bool {
        self.just.contains(&i)
    }
    /// Did this one-shot fire this frame?
    pub(crate) fn fired(&self, i: Input) -> bool {
        self.fired.contains(&i)
    }
    /// Total amount for a one-shot this frame (0.0 when idle): the camera zoom's yards.
    pub(crate) fn amount(&self, i: Input) -> f32 {
        self.amounts
            .iter()
            .filter(|&&(a, _)| a == i)
            .map(|&(_, v)| v)
            .sum()
    }
    /// Both mouse bits held by their Lua Starts, the both-button run: MOVEANDSTEER's body
    /// (`Bindings.xml:3-11`) runs `CameraOrSelectOrMoveStart` and `TurnOrActionStart`.
    pub(crate) fn steering(&self) -> bool {
        self.pressed(Input::TurnOrAction) && self.pressed(Input::CameraOrSelectOrMove)
    }
    /// [`Self::steering`] began this frame.
    pub(crate) fn steering_began(&self) -> bool {
        self.steering()
            && (self.just_pressed(Input::TurnOrAction)
                || self.just_pressed(Input::CameraOrSelectOrMove))
    }
}

/// Which files this session's bindings live in: written by [`seed_bindings_for_vm`], read by the
/// save.
#[derive(Resource, Default)]
struct BindingFiles {
    account: Option<std::path::PathBuf>,
    character: Option<std::path::PathBuf>,
}

/// This module's systems inside [`crate::ui_script::UiInput`]. The UI key feed runs before it, so
/// a key a focused box consumed is already in the capture gate when dispatch runs.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BindingSet;

pub(crate) struct BindingsPlugin;

impl Plugin for BindingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BindingDispatch>()
            .init_resource::<BindingsState>()
            .init_resource::<BindingFiles>()
            .add_systems(
                Update,
                (
                    // The key sets are seeded at the VM's birth, not here: `seed_bindings_for_vm`
                    // runs in `load_ingame_ui_on_world_entry`, before FrameXML and every addon;
                    // the commands come with the load.
                    (sync_dispatch, latch_and_dispatch)
                        .chain()
                        .in_set(crate::ui_script::UiInput)
                        .in_set(BindingSet)
                        .in_set(InWorldGated),
                    // After the tick: the requests are Lua's own (`SaveBindings`), queued by
                    // handlers the tick dispatched, and a save must not wait a frame.
                    drain_binding_requests.after(crate::ui_script::UiInput),
                ),
            );
    }
}

/// Seed the key sets at the VM's birth, before any interface file runs: the defaults off the
/// player's chain (`WTF\DefaultBindings.wtf`), the account set, and the character's set if it has
/// one. The keys are their own table, so a command a later file declares finds its keys there,
/// and stock `ActionButton_OnLoad` paints its hotkey corner from `GetBindingKey` at load.
///
/// The reference reads the defaults later in `UI_Init` (`0x4b62b0` at `0x4900c7`); here they are
/// read first because the stored sets are diffs against them ([`store`]'s deviation), and nothing
/// reads set 0 before `LoadBindings(0)`.
pub(crate) fn seed_bindings_for_vm(world: &mut World, script: &mut UiScript) {
    let defaults = crate::ui_script::default_bindings();
    script.set_default_bindings(defaults);
    let defaults = script.default_bindings();
    let account = crate::local_state::bindings_account_path();
    let overrides = read_diff(&account).unwrap_or_default();
    script.seed_binding_set(1, Some(store::resolve(&overrides, &defaults)));
    script.load_binding_set(1);

    // The character's own set: its file existing makes it the active set, the reference's rule.
    // No roster identity (a rigged or capture run) leaves set 2 unseeded.
    let id = world
        .get_resource::<crate::char_select::Roster>()
        .and_then(crate::ui_macro::identity);
    let character = id
        .as_ref()
        .and_then(|(realm, name)| crate::local_state::bindings_character_path(realm, name));
    match read_diff(&character) {
        Some(overrides) => {
            script.seed_binding_set(2, Some(store::resolve(&overrides, &defaults)));
            script.load_binding_set(2);
            info!("bindings: character-specific set loaded");
        }
        None => {
            script.seed_binding_set(2, None);
            script.load_binding_set(1);
        }
    }
    if let Some(mut files) = world.get_resource_mut::<BindingFiles>() {
        files.account = account;
        files.character = character;
    }
}

/// Read and parse one diff file; `None` (the defaults) when absent or unreadable.
fn read_diff(path: &Option<std::path::PathBuf>) -> Option<Vec<(String, Vec<String>)>> {
    let path = path.as_ref()?;
    match std::fs::read_to_string(path) {
        Ok(text) => Some(store::from_diff(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!("bindings: reading {}: {e}", path.display());
            None
        }
    }
}

/// Drain the VM's binding requests: `SaveBindings` writes the set's diff; saving the account set
/// deletes the character file.
fn drain_binding_requests(script: Option<NonSendMut<UiScript>>, files: Res<BindingFiles>) {
    let Some(mut script) = script else { return };
    for req in script.take_keybind_requests() {
        let KeybindRequest::Save(which) = req;
        let text = store::to_diff(&script.keybind_snapshot(), &script.default_bindings());
        let path = match which {
            1 => &files.account,
            2 => &files.character,
            _ => continue,
        };
        if let Some(path) = path {
            if let Err(e) = crate::local_state::write_atomic(path, &text) {
                warn!("bindings: saving {}: {e}", path.display());
            }
        }
        if which == 1 {
            if let Some(chr) = &files.character {
                match std::fs::remove_file(chr) {
                    Ok(()) => info!("bindings: character-specific set deleted"),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => warn!("bindings: deleting {}: {e}", chr.display()),
                }
            }
        }
    }
}

/// Rebuild the dispatch map when the engine table moved (a rebind, a set switch, the seed, a
/// load), and fire `UPDATE_BINDINGS` so the Lua consumers (the action bar's hotkey corners)
/// repaint.
fn sync_dispatch(script: Option<NonSendMut<UiScript>>, mut dispatch: ResMut<BindingDispatch>) {
    let Some(mut script) = script else { return };
    let generation = script.keybinds_generation();
    if *dispatch.seen_generation.get(&script) == Some(generation) {
        return;
    }
    *dispatch.seen_generation.get(&script) = Some(generation);
    dispatch.map.clear();
    for (key, command) in script.binding_keys() {
        match Chord::parse(&key) {
            Some(ch) => {
                dispatch.map.insert(ch, command);
            }
            None => warn!("bindings: {command}: unpressable chord '{key}' (unknown token)"),
        }
    }
    script.fire_event("UPDATE_BINDINGS", vec![]);
}

/// A wheel delta in lines (notches), whatever unit the OS reported: a trackpad's `Pixel` deltas
/// go through Bevy's own conversion factor. Every wheel reader shares it.
pub(crate) fn wheel_lines(unit: MouseScrollUnit, dy: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => dy,
        MouseScrollUnit::Pixel => dy / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
    }
}

/// Whole notches out of a stream of line deltas, carrying the fraction between frames.
///
/// For consumers that act per notch off the sign alone: the UI's `OnMouseWheel` (stock
/// `ScrollFrameTemplate_OnMouseWheel` moves half a pane per call, `UIPanelTemplates.lua:150`) and
/// the realm list's row step. A reversal drops the carried fraction: it is a new gesture.
#[derive(Default, Resource)]
pub(crate) struct WheelNotches {
    carry: f32,
}

impl WheelNotches {
    /// Add `lines` of travel; returns the signed whole notches it completes (usually 0 or ±1).
    pub(crate) fn feed(&mut self, lines: f32) -> i32 {
        if lines == 0.0 || !lines.is_finite() {
            return 0;
        }
        if self.carry != 0.0 && self.carry.signum() != lines.signum() {
            self.carry = 0.0;
        }
        self.carry += lines;
        let whole = self.carry.trunc();
        self.carry -= whole;
        whole as i32
    }
}

/// The dispatch pass (see the module doc). Runs after the UI key feed and before
/// `WorldStage::Input`, so a bound key acts this frame, once.
fn latch_and_dispatch(
    mut script: Option<NonSendMut<UiScript>>,
    mut keyboard: MessageReader<KeyboardInput>,
    // The characters the active layout makes, which name a letter or punctuation press.
    layout: Res<LayoutChars>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    capture: Res<UiKeyboardCapture>,
    hover: Res<PlayerUiHover>,
    // The chrome flag; only the wheel branch reads it.
    over_ui: Res<PointerOverUiPanel>,
    dispatch: Res<BindingDispatch>,
    mut state: ResMut<BindingsState>,
    mut same_vm: Local<crate::ui_script::VmMemo<bool>>,
    mut notches: Local<WheelNotches>,
    mut focus_lost: MessageReader<bevy::input::keyboard::KeyboardFocusLost>,
    cover: Option<Res<crate::loading_screen::LoadingScreen>>,
) {
    state.just.clear();
    state.fired.clear();
    state.amounts.clear();

    // Deviation: latches die with the VM that made them, because a latch names a command of that
    // VM's table and releasing it against a new one would run another load's up-half. The
    // reference keeps a held key running through `ReloadUI`; here it re-latches on its next press.
    // The held bits die with it, as the key's Stop would have cleared them.
    if let Some(script) = script.as_ref() {
        if same_vm.claim(script) {
            state.latched.clear();
            state.held.clear();
        }
    }
    // The reference's two bulk clears: the window deactivate (`0x514490`'s `and eax,0xfffff00f`,
    // the direction bits only, so the mouse pair and autorun survive) and the world enter behind
    // the loading cover (`0x5144c0`), which clears every bit. A key's latch meets them in the
    // stuck-latch sweep too, whose up-half runs the Stop.
    if cover.is_some_and(|c| c.covering()) {
        state.held.clear();
    } else if focus_lost.read().count() > 0 {
        state
            .held
            .retain(|&i| matches!(i, Input::TurnOrAction | Input::CameraOrSelectOrMove));
    }
    // Lua's binding calls made since the last pass, before this frame's keys.
    if let Some(s) = script.as_mut() {
        let inputs = s.take_binding_input();
        state.apply_script_input(inputs);
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let sup = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    // The dev overlays' plane (`modkeys::dev_chord`; the `sup` gate covers its Super arm), which
    // costs the keyboard its fallback probe (`BindingDispatch::resolve`). Only in a build with dev
    // affordances: a player build keeps the reference's `CTRL-SHIFT-P` fallback to `SHIFT-P`.
    let dev_plane = ctrl && shift && !alt && crate::run_mode::dev_affordances();

    // ── Who owns this frame's keys ── a focused EditBox eats every key while it holds focus; a
    // shown keyboard frame ate the keys in `capture.consumed`. Both suppress a press and nothing
    // else: in the reference a focused box makes the movement handlers no-ops, so the direction
    // bits are frozen, not cleared. `0x514490`, which does clear them, is the OS window-deactivate
    // handler (sole caller `0x493058`, off the root's WM_ACTIVATE slot `[root+0x1134]`); here
    // bevy's `KeyboardFocusLost` and `ButtonInput::release_all` reach the stuck-latch sweep below.
    //
    // ── The pressed-key reconcile ── against bevy's button planes, before this frame's messages:
    // a release the window never saw drops off the list, and a window deactivate empties it, the
    // reference's `0x424790` wipe, so a key held across an alt-tab re-latches on its next repeat.
    state
        .down
        .retain(|&kc| physically_down(Held::Key(kc), &keys, &buttons));

    // ── Keyboard ── press edges run their command, gated on ownership and the capture arm;
    // release edges unlatch and run the `runOnUp` up-half.
    let typing = capture.typing;
    for ev in keyboard.read() {
        let key = chord::normalize_key(ev.key_code);
        match ev.state {
            ButtonState::Pressed => {
                // A focused box in alt-arrow mode declines the four arrows, so their bindings
                // fire while typing (`UiKeyboardCapture::arrows_fall_through`).
                let arrow_exempt = capture.arrows_fall_through
                    && matches!(
                        ev.key_code,
                        KeyCode::ArrowLeft
                            | KeyCode::ArrowRight
                            | KeyCode::ArrowUp
                            | KeyCode::ArrowDown
                    );
                // A keyboard frame ate this one key (the world map's `OnKeyDown`, a cinematic);
                // per key, so every other binding is untouched.
                let eaten = capture.consumed.contains(&ev.key_code);
                // A repeat is a key we already believe is down, the reference's test. Recorded
                // before the gates below, so a key held through a focused box stays down.
                let repeat = state.down.contains(&key);
                if !repeat {
                    state.down.push(key);
                }
                if (typing && !arrow_exempt) || eaten || sup || repeat {
                    continue;
                }
                if state.latched.iter().any(|(k, _)| *k == Held::Key(key)) {
                    continue; // already latched (missed release would double-latch)
                }
                // Named under the layout this press was typed in; a key 1.12 cannot name binds
                // nothing.
                let Some(name) = chord::key_token(ev.key_code, &layout) else {
                    continue;
                };
                let chord = Chord {
                    alt,
                    ctrl,
                    shift,
                    key: BindKey::Key(name),
                };
                if let Some(command) = dispatch.resolve(chord, dev_plane) {
                    press(&mut state, &mut script, command, Held::Key(key));
                }
            }
            ButtonState::Released => {
                state.down.retain(|&kc| kc != key);
                release(&mut state, &mut script, Held::Key(key));
            }
        }
    }

    // ── Mouse buttons ── presses only with the cursor over the world (a frame owns its clicks),
    // releases always.
    for b in [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Forward,
        MouseButton::Back,
    ] {
        if buttons.just_pressed(b)
            && !sup
            && hover.0.is_none()
            && !state.latched.iter().any(|(k, _)| *k == Held::Mouse(b))
        {
            let chord = Chord {
                alt,
                ctrl,
                shift,
                key: BindKey::Mouse(b),
            };
            if let Some(command) = dispatch.resolve(chord, false) {
                press(&mut state, &mut script, command, Held::Mouse(b));
            }
        }
        if buttons.just_released(b) {
            release(&mut state, &mut script, Held::Mouse(b));
        }
    }

    // ── Wheel ── each notch is a press and its release, back to back: the reference hands one
    // chord to `CBindings::ExecuteBinding` with `isDown=1` (`0x483d6f`) then `isDown=0`
    // (`0x483d82`) per wheel message. A `runOnUp` command runs both halves; a plain one's up leg is
    // the `RunCommand` (`0x4b7bf1`) no-op. A trackpad's fractional lines carry to whole notches.
    let wheel = wheel_lines(scroll.unit, scroll.delta.y);
    // Over chrome only (`PointerOverUiPanel`): the wheel still zooms with the cursor on a
    // nameplate, a mouse-enabled widget but not a panel.
    if wheel != 0.0 && !sup && !over_ui.0 {
        let steps = notches.feed(wheel);
        let key = if steps > 0 {
            BindKey::WheelUp
        } else {
            BindKey::WheelDown
        };
        let chord = Chord {
            alt,
            ctrl,
            shift,
            key,
        };
        if let Some(command) = dispatch.resolve(chord, false) {
            for _ in 0..steps.unsigned_abs() {
                press(&mut state, &mut script, command, Held::Wheel);
                release(&mut state, &mut script, Held::Wheel);
            }
        }
    }

    // ── The stuck-latch sweep ── a release the window never saw: a latch whose base key reads up
    // unlatches now and runs its up-half. The reference's two bulk clears land here too, since
    // bevy zeroes `ButtonInput` for both (see the clears above). UI keyboard focus never reaches
    // here.
    let mut stuck: Vec<Held> = Vec::new();
    for (k, _) in &state.latched {
        if !physically_down(*k, &keys, &buttons) && !stuck.contains(k) {
            stuck.push(*k);
        }
    }
    for k in stuck {
        release(&mut state, &mut script, k);
    }

    // The calls this pass's binding bodies made act this frame, as the reference's return from
    // `0x515090` has already moved the bit.
    if let Some(s) = script.as_mut() {
        let inputs = s.take_binding_input();
        state.apply_script_input(inputs);
    }
}

/// Is this input physically down, per bevy's button planes? The stuck-latch sweep and the
/// pressed-key reconcile must agree, so both ask here. `ENTER` is down while either physical
/// key is, since [`chord::normalize_key`] folds `NUMPADENTER` into it.
fn physically_down(
    key: Held,
    keys: &ButtonInput<KeyCode>,
    buttons: &ButtonInput<MouseButton>,
) -> bool {
    match key {
        Held::Key(KeyCode::Enter) => {
            keys.pressed(KeyCode::Enter) || keys.pressed(KeyCode::NumpadEnter)
        }
        Held::Key(kc) => keys.pressed(kc),
        Held::Mouse(b) => buttons.pressed(b),
        // A notch is a press and a release in one frame; it is never "held".
        Held::Wheel => false,
    }
}

/// One matching press: the command's body with `keystate = "down"`, and a latch on its input so
/// the release can run the up-half.
fn press(
    state: &mut BindingsState,
    script: &mut Option<NonSendMut<UiScript>>,
    command: &str,
    key: Held,
) {
    if let Some(s) = script.as_mut() {
        if let Err(e) = s.execute_binding(command, true) {
            warn!("bindings({command}): {e}");
        }
    }
    state.latched.push((key, command.to_owned()));
}

/// A base key's release: drop its latch and run the command's `runOnUp` up-half, even while
/// typing (the reference completes a pressed binding's release regardless of focus).
fn release(state: &mut BindingsState, script: &mut Option<NonSendMut<UiScript>>, key: Held) {
    let mut i = 0;
    while i < state.latched.len() {
        if state.latched[i].0 == key {
            let (_, command) = state.latched.remove(i);
            if let Some(s) = script.as_mut() {
                if let Err(e) = s.execute_binding(&command, false) {
                    warn!("bindings({command}): {e}");
                }
            }
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};

    use super::*;

    /// A core `Bindings.xml` in the reference's shapes over the real binding functions, and bodies
    /// that count their runs where the stock one would open a window; plus its defaults.
    const CORE: &str = r#"<Bindings>
        <Binding name="MOVEANDSTEER" runOnUp="true" header="MOVEMENT">
            if keystate == "down" then CameraOrSelectOrMoveStart() TurnOrActionStart()
            else CameraOrSelectOrMoveStop() TurnOrActionStop() end
        </Binding>
        <Binding name="MOVEFORWARD" runOnUp="true">
            if keystate == "down" then MoveForwardStart() else MoveForwardStop() end
        </Binding>
        <Binding name="TURNLEFT" runOnUp="true">
            if keystate == "down" then TurnLeftStart() else TurnLeftStop() end
        </Binding>
        <Binding name="JUMP">Jump()</Binding>
        <Binding name="SITORSTAND">SitOrStand()</Binding>
        <Binding name="TOGGLESHEATH">ToggleSheath()</Binding>
        <Binding name="TOGGLEAUTORUN">ToggleAutoRun()</Binding>
        <Binding name="ACTIONBUTTON1" runOnUp="true" header="ACTIONBAR">AB1 = keystate</Binding>
        <Binding name="BONUSACTIONBUTTON1" runOnUp="true">PET1 = keystate</Binding>
        <Binding name="BONUSACTIONBUTTON10" runOnUp="true">PET10 = keystate</Binding>
        <Binding name="TARGETNEARESTENEMY" header="TARGETING">TABS = (TABS or 0) + 1</Binding>
        <Binding name="TARGETPREVIOUSENEMY">STABS = (STABS or 0) + 1</Binding>
        <Binding name="TOGGLECHARACTER3" header="INTERFACE">PETPAPER = 1</Binding>
        <Binding name="TOGGLEUI" header="MISC">UITOGGLES = (UITOGGLES or 0) + 1</Binding>
        <Binding name="CAMERAZOOMIN" header="CAMERA">CameraZoomIn(1.0)</Binding>
        <Binding name="CAMERAZOOMOUT">CameraZoomOut(1.0)</Binding>
    </Bindings>"#;
    const DEFAULTS: &str = "bind BUTTON3 MOVEANDSTEER\r\nbind W MOVEFORWARD\r\nbind UP MOVEFORWARD\r\n\
        bind A TURNLEFT\r\nbind LEFT TURNLEFT\r\nbind SPACE JUMP\r\nbind X SITORSTAND\r\n\
        bind Z TOGGLESHEATH\r\nbind NUMLOCK TOGGLEAUTORUN\r\nbind BUTTON4 TOGGLEAUTORUN\r\n\
        bind 1 ACTIONBUTTON1\r\nbind CTRL-1 BONUSACTIONBUTTON1\r\nbind CTRL-0 BONUSACTIONBUTTON10\r\n\
        bind TAB TARGETNEARESTENEMY\r\nbind SHIFT-TAB TARGETPREVIOUSENEMY\r\n\
        bind SHIFT-P TOGGLECHARACTER3\r\nbind ALT-Z TOGGLEUI\r\n\
        bind MOUSEWHEELUP CAMERAZOOMIN\r\nbind MOUSEWHEELDOWN CAMERAZOOMOUT\r\n";

    /// A VM with [`CORE`] declared and [`DEFAULTS`] live, as a first login has them.
    fn core_script() -> UiScript {
        let mut script = UiScript::new().expect("VM");
        script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(DEFAULTS));
        script.load_binding_set(1);
        script.register_bindings(&benilla_ui::bindings_xml::parse(CORE).expect("well-formed"));
        script
    }

    /// The dispatch harness over [`core_script`].
    fn harness() -> App {
        vm_harness(core_script())
    }

    fn key(app: &mut App, k: KeyCode, state: bevy::input::ButtonState, repeat: bool) {
        app.world_mut().write_message(KeyboardInput {
            key_code: k,
            logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
            state,
            text: None,
            repeat,
            window: Entity::PLACEHOLDER,
        });
    }
    fn press_key(app: &mut App, k: KeyCode) {
        key(app, k, bevy::input::ButtonState::Pressed, false);
    }
    fn release_key(app: &mut App, k: KeyCode) {
        key(app, k, bevy::input::ButtonState::Released, false);
    }
    /// A press the OS has flagged as auto-repeat; whether it acts as one is ours to decide.
    fn repeat_key(app: &mut App, k: KeyCode) {
        key(app, k, bevy::input::ButtonState::Pressed, true);
    }
    fn state(app: &App) -> &BindingsState {
        app.world().resource::<BindingsState>()
    }
    fn mouse(app: &mut App, button: MouseButton, state: bevy::input::ButtonState) {
        app.world_mut().write_message(MouseButtonInput {
            button,
            state,
            window: Entity::PLACEHOLDER,
        });
    }
    fn wheel(app: &mut App, y: f32) {
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y,
            window: Entity::PLACEHOLDER,
        });
    }

    /// What a keyboard frame ate this frame. Set, not pushed: `feed_ui_input` rewrites the list
    /// every pass.
    fn frame_ate(app: &mut App, keys: &[KeyCode]) {
        app.world_mut().resource_mut::<UiKeyboardCapture>().consumed = keys.to_vec();
    }

    /// One addon's `Bindings.xml` in the reference's shape: a `runOnUp` binding forking on
    /// `keystate`, and a one-shot. Each half counts itself in a global.
    const PROBE_BINDINGS: &str = r#"<Bindings>
        <Binding name="PROBEHOLD" runOnUp="true" header="PROBE">
            if ( keystate == "down" ) then
                PROBE_DOWN = (PROBE_DOWN or 0) + 1;
            else
                PROBE_UP = (PROBE_UP or 0) + 1;
            end
            PROBE_LAST = keystate;
        </Binding>
        <Binding name="PROBEEDGE">
            PROBE_EDGE = (PROBE_EDGE or 0) + 1;
        </Binding>
    </Bindings>"#;

    /// The with-VM harness: a real engine table with the real [`sync_dispatch`] chained before
    /// [`latch_and_dispatch`], so the map's derivation is under test too.
    fn vm_harness(script: UiScript) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
            .init_resource::<UiKeyboardCapture>()
            .init_resource::<PlayerUiHover>()
            .init_resource::<PointerOverUiPanel>()
            .init_resource::<BindingsState>()
            .init_resource::<BindingDispatch>()
            .init_resource::<LayoutChars>()
            .add_systems(Update, (sync_dispatch, latch_and_dispatch).chain());
        app.insert_non_send_resource(script);
        app
    }

    /// A counter a binding body left in the VM; `0` when the body never ran.
    fn lua_count(app: &App, global: &str) -> i64 {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<i64>(&format!("return {global} or 0"))
            .expect("eval")
    }

    /// A string a binding body left in the VM; `""` when it never ran.
    fn lua_str(app: &App, global: &str) -> String {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<String>(&format!(r#"return {global} or """#))
            .expect("eval")
    }

    fn with_addon(bindings: &str) -> UiScript {
        let mut script = core_script();
        script.register_addon_bindings(
            "ProbeAddon",
            &benilla_ui::bindings_xml::parse(bindings).expect("well-formed"),
        );
        script
    }

    /// A `runOnUp` body is one chunk run twice, `keystate` "down" then "up"; a plain one runs on
    /// the press alone. The core's and an addon's rows dispatch the same way.
    #[test]
    fn a_binding_fires_its_lua_and_runs_again_on_release_when_it_asked_to() {
        let script = with_addon(PROBE_BINDINGS);
        // A 1.12 `<Binding>` ships no default chord, so an addon binding starts unbound.
        script
            .run(r#"SetBinding("J", "PROBEHOLD"); SetBinding("G", "PROBEEDGE")"#)
            .expect("bind");
        let mut app = vm_harness(script);

        // Press: one run, `keystate == "down"`.
        press_key(&mut app, KeyCode::KeyJ);
        app.update();
        assert_eq!(
            lua_count(&app, "PROBE_DOWN"),
            1,
            "the press reaches the body"
        );
        assert_eq!(lua_str(&app, "PROBE_LAST"), "down");
        assert_eq!(
            lua_count(&app, "PROBE_UP"),
            0,
            "no release has happened yet"
        );

        // Release: the SAME body again, with `keystate == "up"`.
        release_key(&mut app, KeyCode::KeyJ);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_UP"), 1);
        assert_eq!(lua_str(&app, "PROBE_LAST"), "up");
        assert_eq!(
            lua_count(&app, "PROBE_DOWN"),
            1,
            "the release runs the chunk with keystate=up, not the down half a second time"
        );

        // The one-shot binding (no `runOnUp`): press runs it, release does not.
        press_key(&mut app, KeyCode::KeyG);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_EDGE"), 1);
        release_key(&mut app, KeyCode::KeyG);
        app.update();
        assert_eq!(
            lua_count(&app, "PROBE_EDGE"),
            1,
            "no runOnUp, no second run — an addon that toggled here would toggle back"
        );

        // The core's rows dispatch beside them.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert_eq!(lua_count(&app, "PROBE_DOWN"), 1, "W is nobody else's");

        // `keystate` does not outlive the call: `RunCommand` sets it to nil (`0x4b7c42`).
        assert_eq!(lua_str(&app, "tostring(keystate)"), "nil");
    }

    /// A focused EditBox swallows every key (the reference's handler returns 1 on every path,
    /// `0x77b35e`), except that in alt-arrow mode (`ignoreArrows`, `SetAltArrowKeyMode`) with ALT
    /// up it returns 0 for the four arrows (`0x77b1c4`) and their bindings run. The stock chat box
    /// ships the flag.
    #[test]
    fn a_flagged_editbox_lets_the_arrow_keys_through_to_their_bindings() {
        let mut app = harness();

        // Typing with no exemption: the arrow is swallowed, exactly like every other key.
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;
        press_key(&mut app, KeyCode::ArrowLeft);
        app.update();
        assert!(
            !state(&app).pressed(Input::TurnLeft),
            "an unflagged focused box eats the arrow"
        );
        release_key(&mut app, KeyCode::ArrowLeft);
        app.update();

        // Same key, same focus, exemption armed: the binding fires.
        app.world_mut()
            .resource_mut::<UiKeyboardCapture>()
            .arrows_fall_through = true;
        press_key(&mut app, KeyCode::ArrowLeft);
        app.update();
        assert!(
            state(&app).pressed(Input::TurnLeft),
            "a flagged box declines the arrow, so TURNLEFT runs"
        );

        // The exemption is four keys, not a hole in the typing gate: W is still swallowed.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "only the arrows are exempt — every other key a focused box still eats"
        );
    }

    /// A box taking focus releases nothing already held: the reference's focused box freezes the
    /// direction bits rather than clearing them (`0x514490` is the OS window-deactivate clear).
    #[test]
    fn a_held_latch_rides_out_a_box_taking_focus_and_still_releases() {
        let script = with_addon(PROBE_BINDINGS);
        script.run(r#"SetBinding("J", "PROBEHOLD")"#).expect("bind");
        let mut app = vm_harness(script);

        press_key(&mut app, KeyCode::KeyW);
        press_key(&mut app, KeyCode::KeyJ);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert_eq!(lua_count(&app, "PROBE_DOWN"), 1);

        // A box takes focus; both latches ride it out.
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "holding W and opening the chat box keeps you running"
        );
        assert_eq!(
            lua_count(&app, "PROBE_UP"),
            0,
            "the focus edge is not a release — nothing has run the up half yet"
        );

        // The keys still stop when the player lets go, box focused or not.
        release_key(&mut app, KeyCode::KeyW);
        release_key(&mut app, KeyCode::KeyJ);
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "releasing W stops you, while typing exactly as otherwise"
        );
        assert_eq!(
            lua_count(&app, "PROBE_UP"),
            1,
            "the up half is delivered even while typing, like every other pressed binding's"
        );
    }

    /// The whole wheel-bind path: the real Keybindings page over the stock commands, a capsule
    /// armed by a click, a notch over the page (its OnMouseWheel, as 1.12's window binds it), then
    /// the bound chord dispatching the stock JUMP body.
    #[test]
    fn a_wheel_notch_binds_through_the_real_page_and_then_dispatches() {
        benilla_formats::wow_data_or_skip!();
        let mut s = crate::ui_script::keybindings_tests::harness();
        crate::ui_script::keybindings_tests::on_page(&mut s);
        const ROW: &str = "BenillaOptionsFrameContainerBodyKeybindingsRow";
        // Expand Movement and arm JUMP's first capsule: the reference accepts the wheel on any
        // command.
        s.run(&format!("{ROW}1Header:Click()")).expect("expand");
        s.run(&format!("{ROW}9Key1Button:Click()")).expect("select");
        assert_eq!(
            s.eval::<String>(&format!("return {ROW}9Description:GetText()"))
                .unwrap(),
            crate::ui_script::keybindings_tests::label(&s, "BINDING_NAME_JUMP", "JUMP")
        );
        s.resolve();
        let (x, y): (f32, f32) = s
            .eval(&format!(
                "return ({ROW}5:GetLeft() + {ROW}5:GetRight()) / 2, \
                        ({ROW}5:GetTop() + {ROW}5:GetBottom()) / 2"
            ))
            .unwrap();
        s.mouse_wheel(x, y, 1.0);
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("MOUSEWHEELUP")"#)
                .unwrap(),
            "JUMP",
            "a notch on an armed capsule is a bind"
        );
        assert!(
            s.eval::<bool>("return BenillaKeyBindingsPage.selected == nil")
                .unwrap(),
            "the completed bind disarms"
        );
        let mut app = vm_harness(s);

        // The bound chord now dispatches: the next notch jumps rather than binding.
        wheel(&mut app, 1.0);
        app.update();
        assert!(state(&app).fired(Input::Jump));
        assert!(
            !state(&app).fired(Input::CameraZoomIn),
            "JUMP stole the wheel from the camera, the 1.12 steal law"
        );
    }

    /// A notch is a press and its release (`0x483d6f`, `0x483d82`), so a `runOnUp` binding on the
    /// wheel runs both halves in one frame.
    #[test]
    fn a_wheel_notch_runs_both_halves_of_a_press_and_release_binding() {
        let mut script = with_addon(PROBE_BINDINGS);
        script.seed_binding_set(
            1,
            Some(vec![("MOUSEWHEELUP".to_string(), "PROBEHOLD".to_string())]),
        );
        script.load_binding_set(1);
        let mut app = vm_harness(script);

        wheel(&mut app, 1.0);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_DOWN"), 1, "the notch's press half");
        assert_eq!(
            lua_count(&app, "PROBE_UP"),
            1,
            "…and its release half, in the same frame — a notch has no key left to lift"
        );
        assert_eq!(lua_str(&app, "PROBE_LAST"), "up");
        // Nothing is left latched: no key could end a wheel latch.
        assert!(app.world().resource::<BindingsState>().latched.is_empty());
    }

    /// Auto-repeat is classified off our pressed-key set, as the reference's (`0x4248b3`), not the
    /// OS bit:
    /// 1. a repeat of a key we have down does not re-run its binding (a held SPACE jumps once);
    /// 2. a flagged repeat of a key we do not have down is a fresh press;
    /// 3. so after a window deactivate (`0x514490` and `0x424790`), the first repeat of a key
    ///    still held re-latches, and running resumes without lifting the key.
    #[test]
    fn a_repeat_is_a_key_we_already_have_down_so_a_held_key_resumes_after_an_alt_tab() {
        let mut app = harness();
        press_key(&mut app, KeyCode::Space);
        app.update();
        assert!(state(&app).fired(Input::Jump), "the first press jumps");

        // (1) Only the pressed-key set stops a jump per repeat.
        repeat_key(&mut app, KeyCode::Space);
        app.update();
        assert!(
            !state(&app).fired(Input::Jump),
            "a repeat of a key already down is not a press"
        );

        // Movement, so the resume below has something to observe.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));

        // (3) The window is deactivated: bevy empties the keyboard plane, which unlatches through
        // the stuck-latch sweep, whose up-half runs the Stop, and empties our pressed-key set.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "the deactivate releases the direction bits (0x514490)"
        );

        // (2) and (3): back in the window, still holding W. The OS calls this a repeat; we do not
        // have the key down, so it is a fresh press.
        repeat_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "the first repeat after re-activation re-latches"
        );
    }

    /// An addon's bindings that call the movement functions: a Start and a Stop alone.
    const MOVE_BINDINGS: &str = r#"<Bindings>
        <Binding name="PROBEGO">MoveForwardStart()</Binding>
        <Binding name="PROBESTOP">MoveForwardStop()</Binding>
    </Bindings>"#;

    /// `MoveForwardStart`/`Stop` set and clear the bit the W key's body does (`0x515090(0x10, …)`):
    /// a Start with no Stop keeps moving, a Stop clears it whoever set it, and outside a binding
    /// body the gate (`0x494a50`) refuses the call.
    #[test]
    fn the_movement_functions_drive_the_held_state_as_the_key_does() {
        let script = with_addon(MOVE_BINDINGS);
        script
            .run(r#"SetBinding("K", "PROBEGO"); SetBinding("L", "PROBESTOP")"#)
            .expect("bind");
        let mut app = vm_harness(script);
        let tap = |app: &mut App, k: KeyCode| {
            press_key(app, k);
            app.update();
            release_key(app, k);
            app.update();
        };

        // The `runOnUp` pair: forward from the press frame, stopped on the release.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert!(
            state(&app).just_pressed(Input::MoveForward),
            "the press edge"
        );
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "held across frames"
        );
        assert!(!state(&app).just_pressed(Input::MoveForward));
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(!state(&app).pressed(Input::MoveForward));

        // A Start with no Stop keeps moving past its key's release; the Stop ends it.
        tap(&mut app, KeyCode::KeyK);
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "no Stop, still moving"
        );
        tap(&mut app, KeyCode::KeyL);
        assert!(!state(&app).pressed(Input::MoveForward));

        // W's release runs MOVEFORWARD's Stop, which clears a Lua Start too.
        tap(&mut app, KeyCode::KeyK);
        tap(&mut app, KeyCode::KeyW);
        assert!(!state(&app).pressed(Input::MoveForward));
        // And a Stop clears the bit under a held key, which stays stopped until pressed again.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        tap(&mut app, KeyCode::KeyL);
        assert!(!state(&app).pressed(Input::MoveForward));
        release_key(&mut app, KeyCode::KeyW);
        app.update();

        // Outside a binding body (a macro, `RunScript`, an addon's own code) nothing moves.
        app.world()
            .non_send_resource::<UiScript>()
            .run("MoveForwardStart() Jump() ToggleAutoRun() RunBinding('MOVEFORWARD')")
            .expect("the calls answer");
        app.update();
        assert!(!state(&app).pressed(Input::MoveForward));
        assert!(!state(&app).fired(Input::Jump));
        assert!(!state(&app).fired(Input::ToggleAutoRun));
    }

    /// The ungated one-shots fire as the keys do: `CameraZoomIn(yards)` carries its amount to the
    /// zoom (1.0 when absent, `0x50b42d`), `SitOrStand` and `ToggleSheath` fire.
    #[test]
    fn the_one_shot_functions_fire_their_inputs() {
        let mut app = harness();
        let run = |app: &mut App, lua: &str| {
            app.world()
                .non_send_resource::<UiScript>()
                .run(lua)
                .expect("the call answers");
            app.update();
        };
        run(&mut app, "CameraZoomIn(3)");
        assert!(state(&app).fired(Input::CameraZoomIn));
        assert_eq!(state(&app).amount(Input::CameraZoomIn), 3.0);
        run(&mut app, "CameraZoomOut()");
        assert_eq!(state(&app).amount(Input::CameraZoomOut), 1.0);
        assert_eq!(
            state(&app).amount(Input::CameraZoomIn),
            0.0,
            "one frame only"
        );
        run(&mut app, "SitOrStand()");
        assert!(state(&app).fired(Input::SitOrStand));
        run(&mut app, "ToggleSheath()");
        assert!(state(&app).fired(Input::ToggleSheath));
        assert!(!state(&app).fired(Input::SitOrStand));
    }

    /// Two keys on one command share its one bit: W and UP both run MOVEFORWARD's body, so the
    /// second Start is no new edge (`0x514840`) and either release's Stop clears the bit whoever
    /// set it (`0x514b70`).
    #[test]
    fn two_keys_on_one_command_share_its_bit_so_either_release_stops_it() {
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert!(state(&app).just_pressed(Input::MoveForward), "press edge");
        press_key(&mut app, KeyCode::ArrowUp);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert!(
            !state(&app).just_pressed(Input::MoveForward),
            "second key on an already-held command is no new edge"
        );
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "W's Stop clears the bit though UP is still down"
        );
        release_key(&mut app, KeyCode::ArrowUp);
        app.update();
        assert!(!state(&app).pressed(Input::MoveForward));
        // A repeat press (held-key auto-repeat) neither re-runs nor re-edges.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        repeat_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert!(!state(&app).just_pressed(Input::MoveForward));
    }

    /// The two-probe lookup: the exact chord, then one retry with the leftmost modifier dropped,
    /// never a third.
    #[test]
    fn a_press_probes_its_chord_then_falls_back_once() {
        // Shift held, W pressed: `SHIFT-W` is unbound, so the retry drops SHIFT and MOVEFORWARD
        // runs (the reference's `strchr` step, `0x4b7990`).
        let mut app = harness();
        press_key(&mut app, KeyCode::ShiftLeft);
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "an unbound SHIFT-W falls back to W, the modifier dropped"
        );
        // It releases on the base key with the modifier still down (the reference replays the
        // press-time chord at key-up, `0x483bd0`).
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(!state(&app).pressed(Input::MoveForward));
        // Bare Z is the sheath toggle, ALT-Z is TOGGLEUI: the exact probe runs first, so a
        // fallback never overrides a real entry.
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(state(&app).fired(Input::ToggleSheath));
        assert_eq!(lua_count(&app, "UITOGGLES"), 0);
        release_key(&mut app, KeyCode::KeyZ);
        press_key(&mut app, KeyCode::AltLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert_eq!(
            lua_count(&app, "UITOGGLES"),
            1,
            "ALT-Z is TOGGLEUI: the exact chord probes first"
        );
        assert!(!state(&app).fired(Input::ToggleSheath));
        // CTRL-ALT-Z runs nothing: the single strip drops the leftmost modifier (`ALT-CTRL-Z`
        // to `CTRL-Z`, unbound) and stops, never reaching ALT-Z or Z.
        release_key(&mut app, KeyCode::KeyZ);
        press_key(&mut app, KeyCode::ControlLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            lua_count(&app, "UITOGGLES") == 1 && !state(&app).fired(Input::ToggleSheath),
            "ALT-CTRL-Z probes CTRL-Z and stops — no second strip to ALT-Z or Z"
        );
        // TAB and SHIFT-TAB are both bound, so each resolves exactly.
        let mut app = harness();
        press_key(&mut app, KeyCode::Tab);
        app.update();
        assert_eq!(lua_count(&app, "TABS"), 1);
        release_key(&mut app, KeyCode::Tab);
        press_key(&mut app, KeyCode::ShiftLeft);
        press_key(&mut app, KeyCode::Tab);
        app.update();
        assert_eq!(lua_count(&app, "STABS"), 1);
        assert_eq!(lua_count(&app, "TABS"), 1);
        // Super is never a binding modifier: a Super-held press builds no chord, so no fallback.
        let mut app = harness();
        press_key(&mut app, KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(!state(&app).fired(Input::ToggleSheath));
    }

    /// The layout's unshifted character on `k`, as `layout_keys` records it from the key's own
    /// OS message, ahead of the press.
    fn layout_key(app: &mut App, k: KeyCode, unshifted: &str) {
        app.world_mut()
            .resource_mut::<LayoutChars>()
            .record(k, &Key::Character(unshifted.into()));
    }

    /// A press with the logical key the OS reports, modifiers applied.
    fn type_key(app: &mut App, k: KeyCode, logical: &str) {
        app.world_mut().write_message(KeyboardInput {
            key_code: k,
            logical_key: Key::Character(logical.into()),
            state: bevy::input::ButtonState::Pressed,
            text: Some(logical.into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// 1.12 binds a letter or punctuation key by the character its layout prints there
    /// (`MapVirtualKeyA` at `0x42da39`): on AZERTY the key where a US W sits runs `Z`'s binding,
    /// and `W`'s moves to the key labelled W. Shift is a prefix, never part of the character.
    #[test]
    fn a_key_runs_the_binding_of_the_character_its_layout_prints_on_it() {
        let script = core_script();
        script
            .run(r#"SetBinding("SHIFT-Z", "TOGGLEUI"); SetBinding("SHIFT-1", "TARGETPREVIOUSENEMY")"#)
            .expect("bind");
        let mut app = vm_harness(script);
        layout_key(&mut app, KeyCode::KeyW, "z");
        layout_key(&mut app, KeyCode::KeyZ, "w");
        type_key(&mut app, KeyCode::KeyW, "z");
        app.update();
        assert!(
            state(&app).fired(Input::ToggleSheath),
            "the key labelled Z is Z"
        );
        assert!(!state(&app).pressed(Input::MoveForward));
        release_key(&mut app, KeyCode::KeyW);
        type_key(&mut app, KeyCode::KeyZ, "w");
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "the key labelled W is W"
        );
        release_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "its release ends it"
        );
        // Shift makes the character `Z`, and the chord `SHIFT-Z`.
        press_key(&mut app, KeyCode::ShiftLeft);
        type_key(&mut app, KeyCode::KeyW, "Z");
        app.update();
        assert_eq!(lua_count(&app, "UITOGGLES"), 1, "SHIFT-Z");
        release_key(&mut app, KeyCode::KeyW);
        // Shift+1 makes `!` on a US layout, and is `SHIFT-1`.
        type_key(&mut app, KeyCode::Digit1, "!");
        app.update();
        assert_eq!(lua_count(&app, "STABS"), 1, "SHIFT-1, never SHIFT-!");
    }

    /// A key's name follows a layout switch while running: its next press is named under the
    /// layout it was typed in.
    #[test]
    fn a_layout_switch_renames_a_key_at_its_next_press() {
        let mut app = harness();
        layout_key(&mut app, KeyCode::KeyW, "z");
        type_key(&mut app, KeyCode::KeyW, "z");
        app.update();
        assert!(state(&app).fired(Input::ToggleSheath));
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        // Back on a US layout, the key's next message reports `w`.
        layout_key(&mut app, KeyCode::KeyW, "w");
        type_key(&mut app, KeyCode::KeyW, "w");
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        assert!(!state(&app).fired(Input::ToggleSheath));
    }

    /// The dev plane spends the keyboard's fallback probe and nothing else, asserted on
    /// [`BindingDispatch::resolve`] itself.
    #[test]
    fn the_dev_plane_keeps_its_letters_without_stealing_bound_chords() {
        let mut app = harness();
        app.update();
        let mut dispatch = app
            .world_mut()
            .remove_resource::<BindingDispatch>()
            .expect("synced");
        let plane_p = Chord::parse("CTRL-SHIFT-P").expect("parses");
        // Ctrl+Shift+P is the perf HUD's; off the plane it falls back onto SHIFT-P...
        assert_eq!(
            dispatch.resolve(plane_p, false),
            Some("TOGGLECHARACTER3"),
            "without the plane rule the retry does reach SHIFT-P — this is what is being blocked"
        );
        // ...so on the plane the retry is suppressed.
        assert_eq!(dispatch.resolve(plane_p, true), None);
        // Only the fallback is suppressed: an exact CTRL-SHIFT- entry, as a player would bind,
        // still resolves.
        dispatch.map.insert(plane_p, "TOGGLECHARACTER3".into());
        assert_eq!(
            dispatch.resolve(plane_p, true),
            Some("TOGGLECHARACTER3"),
            "the plane spends the retry, never the exact probe"
        );
        let shift_p = Chord::parse("SHIFT-P").expect("parses");
        assert_eq!(dispatch.resolve(shift_p, false), Some("TOGGLECHARACTER3"));
    }

    /// The pet bar routes on the CTRL digits and the number row is untouched: the two share base
    /// keys, kept apart only by the exact-modifier probe. CTRL-0 is slot 10.
    #[test]
    fn the_pet_lane_dispatches_on_the_ctrl_digits() {
        let mut app = harness();
        press_key(&mut app, KeyCode::ControlLeft);
        press_key(&mut app, KeyCode::Digit1);
        app.update();
        assert_eq!(lua_str(&app, "PET1"), "down");
        assert_eq!(
            lua_str(&app, "AB1"),
            "",
            "the modifier decides: CTRL-1 is not the number row's"
        );
        // The release runs the up half with Ctrl still held.
        release_key(&mut app, KeyCode::Digit1);
        app.update();
        assert_eq!(lua_str(&app, "PET1"), "up");
        press_key(&mut app, KeyCode::Digit0);
        app.update();
        assert_eq!(lua_str(&app, "PET10"), "down", "CTRL-0 is slot 10");
        // Bare 1 is still the action bar's.
        let mut app = harness();
        press_key(&mut app, KeyCode::Digit1);
        app.update();
        assert_eq!(lua_str(&app, "AB1"), "down");
        assert_eq!(lua_str(&app, "PET1"), "");
    }

    /// The typing gate blocks new presses and releases nothing already held: holding W and
    /// pressing ENTER keeps you running.
    #[test]
    fn the_typing_gate_blocks_new_input_but_a_held_binding_keeps_running() {
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
        // A box takes focus. You keep running, and new presses type instead of binding.
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "the capture edge is not a release — holding W keeps you running while you type"
        );
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(
            !state(&app).fired(Input::SitOrStand),
            "typed keys are not bindings"
        );
        // Letting go still stops you, box focused or not.
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "the release is delivered regardless of focus"
        );
        // Focus drops; keys work again.
        release_key(&mut app, KeyCode::KeyX);
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = false;
        app.update();
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(state(&app).fired(Input::SitOrStand));
    }

    /// A shown keyboard frame eating the key that closes it costs that key its binding and
    /// nothing else. `WorldMapFrame` is a keyboard-enabled fullscreen frame whose `OnKeyDown`
    /// runs `RunBinding("TOGGLEWORLDMAP")` itself (`WorldMapFrame.xml:629`), so it eats every key.
    #[test]
    fn a_keyboard_frame_eating_its_own_toggle_key_does_not_stop_a_held_run() {
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));

        frame_ate(&mut app, &[KeyCode::KeyM]);
        press_key(&mut app, KeyCode::KeyM);
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "the frame ate the toggle key; you are still running"
        );

        // The eaten key loses its binding...
        release_key(&mut app, KeyCode::KeyM);
        app.update();
        frame_ate(&mut app, &[KeyCode::KeyX]);
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(
            !state(&app).fired(Input::SitOrStand),
            "the frame ate this key: its binding must not also fire"
        );

        // ...and only that key.
        release_key(&mut app, KeyCode::KeyX);
        app.update();
        frame_ate(&mut app, &[KeyCode::KeyM]);
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(
            state(&app).fired(Input::SitOrStand),
            "consumption is per key, not per frame"
        );
        assert!(
            state(&app).pressed(Input::MoveForward),
            "and W has been held throughout"
        );
    }

    #[test]
    fn mouse_buttons_bind_only_over_the_world_and_the_wheel_respects_ui() {
        let mut app = harness();
        // BUTTON4 (winit Forward) is TOGGLEAUTORUN's second default.
        mouse(&mut app, MouseButton::Forward, ButtonState::Pressed);
        app.update();
        assert!(state(&app).fired(Input::ToggleAutoRun));
        // Over a UI frame the press belongs to the frame.
        mouse(&mut app, MouseButton::Forward, ButtonState::Released);
        app.update();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        mouse(&mut app, MouseButton::Forward, ButtonState::Pressed);
        app.update();
        assert!(!state(&app).fired(Input::ToggleAutoRun));
        // MOVEANDSTEER (BUTTON3): its body holds both mouse bits, the both-button run, until the
        // release's Stops.
        let mut app = harness();
        mouse(&mut app, MouseButton::Middle, ButtonState::Pressed);
        app.update();
        assert!(state(&app).steering() && state(&app).steering_began());
        app.update();
        assert!(state(&app).steering() && !state(&app).steering_began());
        mouse(&mut app, MouseButton::Middle, ButtonState::Released);
        app.update();
        assert!(!state(&app).steering());
        // The wheel: each notch runs CAMERAZOOMIN's `CameraZoomIn(1.0)`; over UI it belongs to
        // the frame.
        let mut app = harness();
        wheel(&mut app, 2.0);
        app.update();
        assert!(state(&app).fired(Input::CameraZoomIn));
        assert_eq!(state(&app).amount(Input::CameraZoomIn), 2.0, "two notches");
        app.world_mut().resource_mut::<PointerOverUiPanel>().0 = true;
        wheel(&mut app, 2.0);
        app.update();
        assert!(
            !state(&app).fired(Input::CameraZoomIn),
            "a UI wheel is the frame's"
        );
    }

    /// A trackpad's fractional lines carry to whole notches, each one `CameraZoomIn(1.0)`, and a
    /// reversal drops the carried fraction.
    #[test]
    fn fractional_wheel_lines_dispatch_per_whole_notch() {
        let mut app = harness();
        for want in [0.0, 0.0, 1.0] {
            wheel(&mut app, 0.4);
            app.update();
            assert_eq!(
                state(&app).amount(Input::CameraZoomIn),
                want,
                "0.4 lines a frame: the third completes the notch"
            );
        }
        wheel(&mut app, -1.0);
        app.update();
        assert_eq!(state(&app).amount(Input::CameraZoomOut), 1.0);
        assert_eq!(state(&app).amount(Input::CameraZoomIn), 0.0);
    }

    /// The window deactivate (`0x514490`) clears the direction bits a Lua Start set and keeps the
    /// mouse pair.
    #[test]
    fn focus_loss_clears_the_direction_bits_and_keeps_the_mouse_pair() {
        let mut app = harness();
        app.add_message::<bevy::input::keyboard::KeyboardFocusLost>();
        let script_starts = |app: &mut App| {
            app.world()
                .non_send_resource::<UiScript>()
                .execute_binding("MOVEANDSTEER", true)
                .expect("runs");
            app.world()
                .non_send_resource::<UiScript>()
                .execute_binding("MOVEFORWARD", true)
                .expect("runs");
            app.update();
        };
        script_starts(&mut app);
        assert!(state(&app).steering() && state(&app).pressed(Input::MoveForward));
        app.world_mut()
            .write_message(bevy::input::keyboard::KeyboardFocusLost);
        app.update();
        assert!(!state(&app).pressed(Input::MoveForward));
        assert!(state(&app).steering(), "0x514490 masks 0xfffff00f");
    }

    /// B opens the backpack and SHIFT-B opens every bag, end to end: a real key event, the
    /// install's defaults, the stock binding body and the stock Lua. The windows are the stock
    /// `ContainerFrame1..12`, recycled across containers, so the test asks `IsBagOpen(id)`; the
    /// interface loads through [`crate::ui_script::load_default_ui`], so it needs client data.
    #[test]
    fn b_opens_the_backpack_and_shift_b_opens_every_bag() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut script = UiScript::new().expect("VM");
        script.set_default_bindings(crate::ui_script::default_bindings());
        script.load_binding_set(1);
        script.set_screen_size(1024.0, 768.0);
        // A player exists by the time the in-game UI loads, and the stock macro window formats
        // `UnitName("player")` into its character tab in OnLoad.
        script.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some("Probefour".into()),
                level: 60,
                ..Default::default()
            }),
        );
        let failures = crate::ui_script::load_default_ui(&script);
        assert!(
            failures.is_empty(),
            "default UI failed to load: {failures:?}"
        );
        script.set_money(0);
        // The backpack plus one bag in slot 2, fed before any key: stock `OpenBag` builds a
        // window only for a container with `size > 0`.
        script.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                name: Some("Backpack".into()),
                num_slots: 16,
                slots: std::collections::HashMap::new(),
            }),
        );
        script.set_container(
            2,
            Some(benilla_ui::script::ContainerState {
                name: Some("Small Pouch".into()),
                num_slots: 6,
                slots: std::collections::HashMap::new(),
            }),
        );
        let mut app = vm_harness(script);
        // `IsBagOpen(id)` scans `ContainerFrame1..12`: which window a bag lands in varies.
        let open = |app: &App, id: i64| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<bool>(&format!("return IsBagOpen({id}) ~= nil"))
                .expect("eval")
        };

        // B, TOGGLEBACKPACK's default: the backpack and nothing else.
        press_key(&mut app, KeyCode::KeyB);
        app.update();
        release_key(&mut app, KeyCode::KeyB);
        app.update();
        assert!(open(&app, 0), "B opens the backpack");
        assert!(
            !open(&app, 2),
            "B does NOT open the equipped bag — TOGGLEBACKPACK opens the backpack alone"
        );

        // B again: bag 0 is open, so this is the close arm.
        press_key(&mut app, KeyCode::KeyB);
        app.update();
        release_key(&mut app, KeyCode::KeyB);
        app.update();
        assert!(!open(&app, 0), "B again shuts it");

        // SHIFT-B, OPENALLBAGS' default: a different command with a different body.
        press_key(&mut app, KeyCode::ShiftLeft);
        press_key(&mut app, KeyCode::KeyB);
        app.update();
        release_key(&mut app, KeyCode::KeyB);
        app.update();
        assert!(open(&app, 0) && open(&app, 2), "SHIFT-B opens every bag");

        // SHIFT-B again closes them all.
        press_key(&mut app, KeyCode::KeyB);
        app.update();
        release_key(&mut app, KeyCode::KeyB);
        release_key(&mut app, KeyCode::ShiftLeft);
        app.update();
        assert!(
            !open(&app, 0) && !open(&app, 2),
            "SHIFT-B again shuts them all"
        );
    }

    /// No production source in the bindings code carries a stock body: every command's Lua is the
    /// file's own, read off the chain at load. Each string literal outside the test modules, its
    /// whitespace collapsed, is held against every stock body's.
    #[test]
    fn no_stock_binding_body_is_retyped_in_the_tree() {
        benilla_formats::wow_data_or_skip!();
        let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let stock = crate::ui_script::stock_bindings_file().expect("the install's Bindings.xml");
        let bodies: Vec<String> = benilla_ui::bindings_xml::parse(&stock)
            .expect("parses")
            .iter()
            .map(|b| collapse(&b.body))
            .collect();
        assert!(bodies.len() > 200);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = vec![
            root.join("benilla-app/src/bindings.rs"),
            root.join("benilla-ui/src/script/keybind.rs"),
            root.join("benilla-ui/src/bindings_xml.rs"),
        ];
        for e in std::fs::read_dir(root.join("benilla-app/src/bindings")).unwrap() {
            files.push(e.unwrap().path());
        }
        assert!(
            !root.join("benilla-app/src/bindings/commands.rs").exists(),
            "the retyped registry is gone"
        );
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            for literal in string_literals(production) {
                let literal = collapse(&literal);
                if literal.is_empty() {
                    continue;
                }
                for body in &bodies {
                    assert!(
                        !literal.contains(body.as_str()),
                        "{} carries the stock body {body:?}",
                        path.display()
                    );
                }
            }
        }
    }

    /// The contents of every `"…"` and `r#"…"#` literal in Rust source, escapes left as written.
    fn string_literals(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let b = src.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
                i = src[i..].find('\n').map_or(b.len(), |n| i + n);
                continue;
            }
            if b[i] == b'\'' {
                // A char literal or a lifetime: skip `'x'` and `'\x'` so a quote char is no string.
                if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                    continue;
                }
                if b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'') {
                    i += 4;
                    continue;
                }
                i += 1;
                continue;
            }
            if b[i] == b'r' && (b.get(i + 1) == Some(&b'#') || b.get(i + 1) == Some(&b'"')) {
                let hashes = src[i + 1..].bytes().take_while(|&c| c == b'#').count();
                if b.get(i + 1 + hashes) == Some(&b'"') {
                    let start = i + 2 + hashes;
                    let close = format!("\"{}", "#".repeat(hashes));
                    let end = src[start..].find(&close).map_or(b.len(), |n| start + n);
                    out.push(src[start..end].to_string());
                    i = end + close.len();
                    continue;
                }
            }
            if b[i] == b'"' {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                out.push(src[start..j.min(b.len())].to_string());
                i = j + 1;
                continue;
            }
            i += 1;
        }
        out
    }

    /// The literal scanner finds what it must, so the check above cannot pass by seeing nothing.
    #[test]
    fn the_literal_scanner_reads_both_kinds_of_string() {
        let found = string_literals(
            "let a = \"x\\\"y\"; // \"comment\"\nlet c = '\"'; let b = r#\"ToggleBackpack();\"#;",
        );
        assert_eq!(found, ["x\\\"y", "ToggleBackpack();"]);
    }
}
