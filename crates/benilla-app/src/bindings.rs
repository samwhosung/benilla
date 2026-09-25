//! Key bindings: the one chord-to-command engine every rebindable input runs through. The table
//! lives engine-side ([`benilla_ui::script::keybind`], which the Key Bindings window's Lua edits);
//! this module derives an exact-match dispatch map from it whenever its generation moves.
//!
//! Dispatch ([`latch_and_dispatch`], in [`crate::ui_script::UiInput`] after the UI key feed):
//! - a press probes its exact chord, then once more with its leftmost modifier dropped
//!   ([`Chord::fallback`]); Super held matches nothing;
//! - [`Kind::Held`] commands latch on the press and unlatch on the base key's release (the
//!   reference's `runOnUp` law). UI focus suppresses presses and releases nothing already held;
//!   a latch ends only on its base key's release, the stuck-latch sweep (OS focus loss, the
//!   loading cover) or a VM swap;
//! - [`Kind::Edge`]/[`Kind::EdgeUpDown`] run their 1.12 Lua bodies in the VM;
//! - [`Kind::Host`] lands in [`BindingsState::fired`] for engine consumers.
//!
//! An addon's `Bindings.xml` rows ([`benilla_ui::bindings_xml`]) dispatch here too: a resolved
//! chord names a [`Bound`], either a registry [`Cmd`] or an index into the addon table.
//!
//! While the Keybindings page has a capsule selected (`BenillaBindCapture`), raw input is
//! canonicalized (`ALT-CTRL-SHIFT-<TOKEN>`) and handed to the page, by the 1.12 law: lone
//! modifiers ignored, left and right clicks stay UI clicks, ESC binds like any key.
//!
//! Persistence: `benilla-config/bindings/account.txt` and `<Realm>-<Char>.txt` ([`store`]); the
//! character file's existence is the character-set state, as in the reference.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_ui::script::keybind::{AddonBindingBody, KeybindCommand, KeybindRequest};
use benilla_ui::script::UiScript;

use crate::char_select::InWorldGated;
use crate::ui_script::{PlayerUiHover, PointerOverUiPanel, UiKeyboardCapture};

pub(crate) mod chord;
pub(crate) mod commands;
mod store;

use chord::{BindKey, Chord};
pub(crate) use commands::cmd;
use commands::{Cmd, Kind, SPECS};

/// What a bound chord names: a registry command or an addon's `Bindings.xml` body. An enum, not
/// one index space, so no `SPECS[cmd]` read can be handed an addon index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bound {
    /// A registry command, an index into [`SPECS`].
    Spec(Cmd),
    /// An addon-declared binding, an index into [`BindingDispatch::addons`].
    Addon(u16),
}

/// The chord-to-binding map, rebuilt whenever the engine table's generation moves. Probe it
/// through [`BindingDispatch::resolve`]: the lookup is two probes.
#[derive(Resource, Default)]
struct BindingDispatch {
    map: std::collections::HashMap<Chord, Bound>,
    /// The addon bindings [`Bound::Addon`] indexes, in registration order; snapshotted with the
    /// map so the two agree on what an index means.
    addons: Vec<AddonBindingBody>,
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
    fn resolve(&self, chord: Chord, dev_plane: bool) -> Option<Bound> {
        if let Some(&bound) = self.map.get(&chord) {
            return Some(bound);
        }
        if dev_plane {
            return None;
        }
        self.map.get(&chord.fallback()?).copied()
    }
}

/// This frame's binding activity, which engine-side consumers read instead of raw keys.
#[derive(Resource, Default)]
pub(crate) struct BindingsState {
    /// Live latches, (base key, binding): a [`Kind::Held`], [`Kind::EdgeUpDown`] or `runOnUp`
    /// addon press not yet released.
    latched: Vec<(BindKey, Bound)>,
    /// Commands whose first latch began this frame (the press edge).
    just: Vec<Cmd>,
    /// Host-edge commands fired this frame.
    fired: Vec<Cmd>,
    /// Analog amount per host command this frame (wheel notches; a key press adds the
    /// reference's 1.0 step), the camera zoom's input.
    amounts: Vec<(Cmd, f32)>,
    /// The keys we believe are down: a press of one is a repeat. The reference classifies
    /// auto-repeat off its own list (`0x4248b3`), never the Win32 `lParam` repeat bit.
    /// Normalized like [`latched`](Self::latched) and reconciled against `ButtonInput` every
    /// pass, so a window deactivate empties it and a key held across an alt-tab resumes, as in
    /// the reference. Keyboard only: mouse edges cannot repeat.
    down: Vec<KeyCode>,
}

impl BindingsState {
    /// Is a registry command latched right now (the reference's held movement bit)?
    pub(crate) fn pressed(&self, c: Cmd) -> bool {
        self.latched.iter().any(|&(_, l)| l == Bound::Spec(c))
    }
    /// Did this command's latch begin this frame (the key-down edge)?
    pub(crate) fn just_pressed(&self, c: Cmd) -> bool {
        self.just.contains(&c)
    }
    /// Did this host-edge command fire this frame?
    pub(crate) fn fired(&self, c: Cmd) -> bool {
        self.fired.contains(&c)
    }
    /// Total analog amount for a host command this frame (0.0 when idle).
    pub(crate) fn amount(&self, c: Cmd) -> f32 {
        self.amounts
            .iter()
            .filter(|&&(a, _)| a == c)
            .map(|&(_, v)| v)
            .sum()
    }
    /// Test seam for consumer systems: a state in which these host commands fired this frame.
    #[cfg(test)]
    pub(crate) fn test_fired(cmds: &[Cmd]) -> Self {
        Self {
            fired: cmds.to_vec(),
            ..Default::default()
        }
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
                    // The registry and both sets are seeded at the VM's birth, not here:
                    // `seed_bindings_for_vm` runs in `load_ingame_ui_on_world_entry`, before
                    // FrameXML and every addon.
                    (sync_dispatch, latch_and_dispatch)
                        .chain()
                        .in_set(crate::ui_script::UiInput)
                        .in_set(BindingSet)
                        .in_set(InWorldGated),
                    // After the tick: the requests are Lua's own (`SaveBindings`, `RunBinding`),
                    // queued by handlers the tick dispatched, and a save must not wait a frame.
                    drain_binding_requests.after(crate::ui_script::UiInput),
                ),
            );
    }
}

/// The registry as the engine table's registration payload, for the boot seed and the capture
/// fixtures.
pub(crate) fn registry_commands() -> Vec<KeybindCommand> {
    SPECS
        .iter()
        .map(|s| KeybindCommand {
            name: s.name,
            category: s.category,
            run_on_up: s.run_on_up(),
            default1: s.d1,
            default2: s.d2,
        })
        .collect()
}

/// Seed the keybinding table at the VM's birth, before any interface file runs: the command
/// registry, the account set, and the character's set if it has one.
///
/// Order matters, and is the reference's: stock `ActionButton_OnLoad` paints its hotkey corner
/// from `GetBindingKey` at load, an addon's `SetBinding` on a stock command needs the command
/// registered, and the registry must hold the low indices the Key Bindings window walks.
/// `seed_binding_set` keeps the diff by name, so a stored addon row binds when the addon
/// registers it later.
pub(crate) fn seed_bindings_for_vm(world: &mut World, script: &mut UiScript) {
    script.register_bindings(&registry_commands());
    let account = crate::local_state::bindings_account_path();
    let overrides = read_diff(&account).unwrap_or_default();
    script.seed_binding_set(1, Some(store::resolve(&overrides)));
    script.load_binding_set(1);
    // `SPECS` and `ABSENT` together are the client's whole 1.12 command surface.
    info!(
        "bindings: {} of {} 1.12 commands registered ({} recorded absent)",
        SPECS.len(),
        SPECS.len() + commands::ABSENT.len(),
        commands::ABSENT.len()
    );

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
            script.seed_binding_set(2, Some(store::resolve(&overrides)));
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

/// Drain the VM's binding requests. `Save` (`SaveBindings`) writes the set's diff; saving the
/// account set deletes the character file. `Run` is `RunBinding(name)`, which stock keyboard
/// frames use to pass a chord back to its binding (`CinematicFrame.xml:42`,
/// `WorldMapFrame.xml:629`).
fn drain_binding_requests(script: Option<NonSendMut<UiScript>>, files: Res<BindingFiles>) {
    let Some(mut script) = script else { return };
    for req in script.take_keybind_requests() {
        // Only the Lua-bodied kinds can run this way: a `Held`/`Host` action is engine state a Lua
        // call cannot assert. The stock callers run `SCREENSHOT` and `TOGGLEWORLDMAP`, both
        // `Kind::Edge`.
        let which = match req {
            KeybindRequest::Save(which) => which,
            KeybindRequest::Run(name) => {
                match SPECS.iter().find(|s| s.name == name).map(|s| &s.kind) {
                    Some(Kind::Edge(lua)) | Some(Kind::EdgeUpDown(lua, _)) => {
                        let lua = *lua;
                        if let Err(e) = script.run(lua) {
                            warn!("bindings(RunBinding {name}): {e}");
                        }
                    }
                    Some(_) => warn!("RunBinding({name}): a held/engine action has no body to run"),
                    None => warn!("RunBinding({name}): no such command"),
                }
                continue;
            }
        };
        let snapshot = script.keybind_snapshot();
        let text = store::to_diff(&snapshot);
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

/// Rebuild the dispatch map when the engine table moved (a rebind, a set switch, the seed), and
/// fire `UPDATE_BINDINGS` so the Lua consumers (the action bar's hotkey corners) repaint.
fn sync_dispatch(script: Option<NonSendMut<UiScript>>, mut dispatch: ResMut<BindingDispatch>) {
    let Some(mut script) = script else { return };
    let generation = script.keybinds_generation();
    if *dispatch.seen_generation.get(&script) == Some(generation) {
        return;
    }
    *dispatch.seen_generation.get(&script) = Some(generation);
    dispatch.map.clear();
    // The addon table is where names the registry does not know resolve.
    let addons = script.addon_binding_bodies();
    let mut by_name: std::collections::HashMap<&str, Bound> = SPECS
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name, Bound::Spec(Cmd(i as u16))))
        .collect();
    for (i, a) in addons.iter().enumerate() {
        // A registry name is never overwritten. `try_from`, because a wrapped index would
        // dispatch the wrong body rather than none.
        let Ok(i) = u16::try_from(i) else { break };
        by_name.entry(a.name.as_str()).or_insert(Bound::Addon(i));
    }
    for (name, keys) in script.keybind_snapshot() {
        let Some(&bound) = by_name.get(name.as_str()) else {
            // A name with no home: an uninstalled addon's row (kept on purpose) or a 1.12
            // command benilla does not implement, which `ABSENT` explains in the log.
            if let Some(absent) = commands::ABSENT.iter().find(|a| a.name == name) {
                if !keys.is_empty() {
                    warn!(
                        "bindings: {name} is bound to {keys:?} but benilla does not implement it \u{2014} {}",
                        absent.why
                    );
                }
            }
            continue;
        };
        for key in keys {
            match Chord::parse(&key) {
                Some(ch) => {
                    dispatch.map.insert(ch, bound);
                }
                None => warn!("bindings: {name}: unpressable chord '{key}' (unknown token)"),
            }
        }
    }
    dispatch.addons = addons;
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
    script: Option<NonSendMut<UiScript>>,
    mut keyboard: MessageReader<KeyboardInput>,
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
) {
    state.just.clear();
    state.fired.clear();
    state.amounts.clear();

    // Deviation: latches die with the VM that made them, because a latch indexes that VM's
    // dispatch table and releasing it against a new one would run the wrong addon's up-half. The
    // reference keeps a held key running through `ReloadUI`; here it re-latches on its next press.
    if let Some(script) = script.as_ref() {
        if same_vm.claim(script) {
            state.latched.clear();
        }
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let sup = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    // The dev overlays' plane (`modkeys::dev_chord`; the `sup` gate covers its Super arm), which
    // costs the keyboard its fallback probe (`BindingDispatch::resolve`). Only in a build with dev
    // affordances: a player build keeps the reference's `CTRL-SHIFT-P` fallback to `SHIFT-P`.
    let dev_plane = ctrl && shift && !alt && crate::run_mode::dev_affordances();

    let mut script = script;
    let armed = script.as_ref().is_some_and(|s| s.bind_capture_armed());
    let run_lua = |script: &mut Option<NonSendMut<UiScript>>, lua: &str, tag: &str| {
        if let Some(s) = script.as_mut() {
            if let Err(e) = s.run(lua) {
                warn!("bindings({tag}): {e}");
            }
        }
    };

    // ── The capture seam ── a Keybindings capsule is selected: swallow raw input and hand the
    // chord string to the page's Lua. The 1.12 law: lone modifiers and unknown keys ignored,
    // left and right stay UI clicks, the wheel is a chord. Super is not a 1.12 modifier, so a
    // Super press is ignored outright.
    if armed {
        let mut captured: Option<String> = None;
        for ev in keyboard.read() {
            if ev.state != ButtonState::Pressed || ev.repeat || sup {
                continue;
            }
            if let Some(token) = chord::key_token(ev.key_code) {
                captured = Some(chord::chord_string(alt, ctrl, shift, token));
            }
        }
        for b in [MouseButton::Middle, MouseButton::Forward, MouseButton::Back] {
            if buttons.just_pressed(b) && !sup {
                if let Some(token) = chord::mouse_token(b) {
                    captured = Some(chord::chord_string(alt, ctrl, shift, token));
                }
            }
        }
        if scroll.delta.y != 0.0 && !sup {
            let token = if scroll.delta.y > 0.0 {
                "MOUSEWHEELUP"
            } else {
                "MOUSEWHEELDOWN"
            };
            captured = Some(chord::chord_string(alt, ctrl, shift, token));
        }
        if let Some(chord_str) = captured {
            run_lua(
                &mut script,
                &format!("KeyBindings_OnHostKey(\"{chord_str}\")"),
                "capture",
            );
        }
        // Releases still unlatch below so a key held across the arm cannot stick; nothing new
        // latches or fires while armed.
    }

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
        .retain(|&kc| physically_down(BindKey::Key(kc), &keys, &buttons));

    // ── Keyboard ── press edges latch or fire, gated on ownership and the capture arm; release
    // edges unlatch and fire the `runOnUp` up-half.
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
                if armed || (typing && !arrow_exempt) || eaten || sup || repeat {
                    continue;
                }
                if state.latched.iter().any(|&(k, _)| k == BindKey::Key(key)) {
                    continue; // already latched (missed release would double-latch)
                }
                let chord = Chord {
                    alt,
                    ctrl,
                    shift,
                    key: BindKey::Key(key),
                };
                if let Some(bound) = dispatch.resolve(chord, dev_plane) {
                    press(
                        &mut state,
                        &mut script,
                        run_lua,
                        &dispatch,
                        bound,
                        BindKey::Key(key),
                    );
                }
            }
            ButtonState::Released => {
                state.down.retain(|&kc| kc != key);
                release(
                    &mut state,
                    &mut script,
                    run_lua,
                    &dispatch,
                    BindKey::Key(key),
                );
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
            && !armed
            && !sup
            && hover.0.is_none()
            && !state.latched.iter().any(|&(k, _)| k == BindKey::Mouse(b))
        {
            let chord = Chord {
                alt,
                ctrl,
                shift,
                key: BindKey::Mouse(b),
            };
            if let Some(bound) = dispatch.resolve(chord, false) {
                press(
                    &mut state,
                    &mut script,
                    run_lua,
                    &dispatch,
                    bound,
                    BindKey::Mouse(b),
                );
            }
        }
        if buttons.just_released(b) {
            release(
                &mut state,
                &mut script,
                run_lua,
                &dispatch,
                BindKey::Mouse(b),
            );
        }
    }

    // ── Wheel ── a notch is a press and its release, back to back: the reference hands one chord
    // to `CBindings::ExecuteBinding` with `isDown=1` (`0x483d6f`) then `isDown=0` (`0x483d82`).
    // A `runOnUp` command runs both halves in the frame; a plain one's up leg is the
    // `RunCommand` (`0x4b7b50`) no-op.
    let wheel = wheel_lines(scroll.unit, scroll.delta.y);
    // Over chrome only (`PointerOverUiPanel`): the wheel still zooms with the cursor on a
    // nameplate, a mouse-enabled widget but not a panel.
    if wheel != 0.0 && !armed && !sup && !over_ui.0 {
        let (key, amount) = if wheel > 0.0 {
            (BindKey::WheelUp, wheel)
        } else {
            (BindKey::WheelDown, -wheel)
        };
        let chord = Chord {
            alt,
            ctrl,
            shift,
            key,
        };
        match dispatch.resolve(chord, false) {
            // A host command takes the notch's analog magnitude (the camera zoom), which `press`
            // cannot carry: it spends the 1.0 key step. It has no release half.
            Some(Bound::Spec(cmd)) if matches!(SPECS[cmd.0 as usize].kind, Kind::Host) => {
                state.fired.push(cmd);
                state.amounts.push((cmd, amount));
            }
            // Everything else is the reference's pair: `Kind::EdgeUpDown` runs both halves (a
            // wheel-bound action button casts), an addon's `runOnUp` body runs down then up, and
            // `Kind::Held` sets and clears in the one tick, as the reference's movement bit does.
            Some(bound) => {
                press(&mut state, &mut script, run_lua, &dispatch, bound, key);
                release(&mut state, &mut script, run_lua, &dispatch, key);
            }
            None => {}
        }
    }

    // ── The stuck-latch sweep ── a release the window never saw: a latch whose base key reads up
    // unlatches now and fires its up-half. The reference's two bulk clears land here, since bevy
    // zeroes `ButtonInput` for both: OS window deactivate (`0x514490`'s `and eax,0xfffff00f`, the
    // direction bits released while autorun survives) and the loading cover
    // (`loading_screen::input`'s `swallow`, the world-enter `0x5144c0`, which clears everything).
    // UI keyboard focus never reaches here.
    let mut stuck: Vec<BindKey> = Vec::new();
    for &(k, _) in &state.latched {
        if !physically_down(k, &keys, &buttons) && !stuck.contains(&k) {
            stuck.push(k);
        }
    }
    for k in stuck {
        release(&mut state, &mut script, run_lua, &dispatch, k);
    }
}

#[cfg(test)]
impl BindingDispatch {
    /// A dispatch seeded from the registry defaults, the no-VM test seam; no addon bindings.
    fn test_defaults() -> Self {
        let mut map = std::collections::HashMap::new();
        for (i, s) in SPECS.iter().enumerate() {
            for d in [s.d1, s.d2].into_iter().flatten() {
                map.insert(
                    Chord::parse(d).expect("default parses"),
                    Bound::Spec(Cmd(i as u16)),
                );
            }
        }
        Self {
            map,
            addons: Vec::new(),
            seen_generation: crate::ui_script::VmMemo::default(),
        }
    }
}

/// Is this base key physically down, per bevy's button planes? The stuck-latch sweep and the
/// pressed-key reconcile must agree, so both ask here. `ENTER` is down while either physical
/// key is, since [`chord::normalize_key`] folds `NUMPADENTER` into it.
fn physically_down(
    key: BindKey,
    keys: &ButtonInput<KeyCode>,
    buttons: &ButtonInput<MouseButton>,
) -> bool {
    match key {
        BindKey::Key(KeyCode::Enter) => {
            keys.pressed(KeyCode::Enter) || keys.pressed(KeyCode::NumpadEnter)
        }
        BindKey::Key(kc) => keys.pressed(kc),
        BindKey::Mouse(b) => buttons.pressed(b),
        // A notch is a press and a release in one frame; it is never "held".
        BindKey::WheelUp | BindKey::WheelDown => false,
    }
}

/// One matching press: latch the held kinds, run or fire by kind.
fn press(
    state: &mut BindingsState,
    script: &mut Option<NonSendMut<UiScript>>,
    run_lua: impl Fn(&mut Option<NonSendMut<UiScript>>, &str, &str),
    dispatch: &BindingDispatch,
    bound: Bound,
    key: BindKey,
) {
    let cmd = match bound {
        Bound::Spec(cmd) => cmd,
        // Run the body with `keystate = "down"`; latch only if its `runOnUp` asks for the
        // release half.
        Bound::Addon(i) => {
            if let Some(a) = dispatch.addons.get(i as usize) {
                run_addon(script, a, "down");
                if a.run_on_up {
                    state.latched.push((key, bound));
                }
            }
            return;
        }
    };
    let spec = &SPECS[cmd.0 as usize];
    match &spec.kind {
        Kind::Held => {
            if !state.pressed(cmd) {
                state.just.push(cmd);
            }
            state.latched.push((key, bound));
        }
        Kind::Edge(lua) => run_lua(script, lua, spec.name),
        Kind::EdgeUpDown(down, _) => {
            run_lua(script, down, spec.name);
            state.latched.push((key, bound));
        }
        Kind::Host => {
            state.fired.push(cmd);
            if !state.pressed(cmd) {
                state.just.push(cmd);
            }
            state.amounts.push((cmd, 1.0));
        }
    }
}

/// A base key's release: drop its latch and fire a `runOnUp` up-half, even while typing (the
/// reference completes a pressed binding's release regardless of focus).
fn release(
    state: &mut BindingsState,
    script: &mut Option<NonSendMut<UiScript>>,
    run_lua: impl Fn(&mut Option<NonSendMut<UiScript>>, &str, &str),
    dispatch: &BindingDispatch,
    key: BindKey,
) {
    let mut i = 0;
    while i < state.latched.len() {
        if state.latched[i].0 == key {
            match state.latched.remove(i).1 {
                Bound::Spec(cmd) => {
                    if let Kind::EdgeUpDown(_, up) = &SPECS[cmd.0 as usize].kind {
                        run_lua(script, up, SPECS[cmd.0 as usize].name);
                    }
                }
                // The same body again, with `keystate = "up"`.
                Bound::Addon(a) => {
                    if let Some(a) = dispatch.addons.get(a as usize) {
                        run_addon(script, a, "up");
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Run one addon binding's body with the `keystate` global set for the call and restored after.
///
/// A `runOnUp` body is one chunk run on press and release, forking on the bare global `keystate`
/// (`Bindings.xml:4`); it is set in `_G`, not prepended, so the addon's line numbers hold.
/// `keystate` is absent from the 1.12 client's in-world `_G` (`reference/1.12-globals.tsv`), so
/// the reference sets it transiently, as it does `this`/`arg1` (`0x703f50` → `0x704f10`).
/// Save-and-restore because bodies nest: an inner binding must not clear the outer `keystate`.
fn run_addon(script: &mut Option<NonSendMut<UiScript>>, bind: &AddonBindingBody, keystate: &str) {
    let Some(s) = script.as_mut() else { return };
    let globals = s.lua().globals();
    // `Option<String>`, since benilla-app does not depend on mlua; `None` converts back to nil.
    let prior: Option<String> = globals.get("keystate").unwrap_or(None);
    if let Err(e) = globals.set("keystate", keystate) {
        warn!("bindings({}): setting keystate: {e}", bind.name);
        return;
    }
    if let Err(e) = s.run(&bind.body) {
        warn!("bindings({}): {e}", bind.name);
    }
    if let Err(e) = s.lua().globals().set("keystate", prior) {
        warn!("bindings({}): restoring keystate: {e}", bind.name);
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};

    use super::*;

    /// A minimal app around [`latch_and_dispatch`] with the registry defaults and no VM, fed real
    /// input events through `InputPlugin` as winit feeds them.
    fn harness() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
            .init_resource::<UiKeyboardCapture>()
            .init_resource::<PlayerUiHover>()
            .init_resource::<PointerOverUiPanel>()
            .init_resource::<BindingsState>()
            .insert_resource(BindingDispatch::test_defaults())
            .add_systems(Update, latch_and_dispatch);
        app
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

    /// An addon's `runOnUp` body is one chunk run twice, `keystate` "down" then "up"; our own
    /// registry holds two strings ([`Kind::EdgeUpDown`]) instead.
    #[test]
    fn an_addon_binding_fires_its_lua_and_runs_again_on_release_when_it_asked_to() {
        let mut script = UiScript::new().expect("VM");
        script.register_bindings(&registry_commands());
        script.register_addon_bindings(
            "ProbeAddon",
            &benilla_ui::bindings_xml::parse(PROBE_BINDINGS).expect("well-formed"),
        );
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
            "the press must reach the addon's body — this is the phase-4 bug"
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

        // The registry dispatches unchanged beside them.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));
        assert_eq!(
            lua_count(&app, "PROBE_DOWN"),
            1,
            "a built-in press is nobody else's"
        );

        // `keystate` does not outlive the call: the 1.12 client's in-world `_G` has none
        // (`reference/1.12-globals.tsv`).
        assert_eq!(
            lua_str(&app, "tostring(keystate)"),
            "nil",
            "keystate must be restored after a binding body runs, not left standing in _G"
        );
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
            !state(&app).pressed(cmd::TURN_LEFT),
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
            state(&app).pressed(cmd::TURN_LEFT),
            "a flagged box declines the arrow, so TURNLEFT runs"
        );

        // The exemption is four keys, not a hole in the typing gate: W is still swallowed.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            !state(&app).pressed(cmd::MOVE_FORWARD),
            "only the arrows are exempt — every other key a focused box still eats"
        );
    }

    /// A box taking focus releases nothing already held: the reference's focused box freezes the
    /// direction bits rather than clearing them (`0x514490` is the OS window-deactivate clear).
    #[test]
    fn a_held_latch_rides_out_a_box_taking_focus_and_still_releases() {
        let mut script = UiScript::new().expect("VM");
        script.register_bindings(&registry_commands());
        script.register_addon_bindings(
            "ProbeAddon",
            &benilla_ui::bindings_xml::parse(PROBE_BINDINGS).expect("well-formed"),
        );
        script.run(r#"SetBinding("J", "PROBEHOLD")"#).expect("bind");
        let mut app = vm_harness(script);

        press_key(&mut app, KeyCode::KeyW);
        press_key(&mut app, KeyCode::KeyJ);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));
        assert_eq!(lua_count(&app, "PROBE_DOWN"), 1);

        // A box takes focus; both latches ride it out.
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;
        app.update();
        assert!(
            state(&app).pressed(cmd::MOVE_FORWARD),
            "holding W and opening the chat box keeps you running (2196)"
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
            !state(&app).pressed(cmd::MOVE_FORWARD),
            "releasing W stops you, while typing exactly as otherwise"
        );
        assert_eq!(
            lua_count(&app, "PROBE_UP"),
            1,
            "the up half is delivered even while typing, like every other pressed binding's"
        );
    }

    /// The armed capture seam, driven by a real wheel event.
    #[test]
    fn an_armed_capture_takes_a_wheel_notch() {
        let mut script = UiScript::new().expect("VM");
        script.register_bindings(&registry_commands());
        script
            .run(
                r#"CAPTURED = nil
                   function KeyBindings_OnHostKey(chord) CAPTURED = chord end
                   BenillaBindCapture(true)"#,
            )
            .expect("arm");
        let mut app = vm_harness(script);

        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(
            lua_str(&app, "tostring(CAPTURED)"),
            "MOUSEWHEELUP",
            "a wheel notch while armed is a binding key"
        );
        assert!(
            !state(&app).fired(cmd::CAMERA_ZOOM_IN),
            "the armed seam swallows the notch — it must not also zoom"
        );

        // Down, and a modified notch.
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(lua_str(&app, "tostring(CAPTURED)"), "MOUSEWHEELDOWN");
        press_key(&mut app, KeyCode::ShiftLeft);
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(lua_str(&app, "tostring(CAPTURED)"), "SHIFT-MOUSEWHEELUP");
    }

    /// The whole wheel-bind path: the real Keybindings page, a capsule armed by a click, a real
    /// notch, then the bound chord dispatching.
    #[test]
    fn a_wheel_notch_binds_through_the_real_page_and_then_dispatches() {
        benilla_formats::wow_data_or_skip!();
        let by_name =
            |n: &str| Cmd(SPECS.iter().position(|s| s.name == n).expect("registered") as u16);
        let mut s = crate::ui_script::keybindings_tests::harness();
        crate::ui_script::keybindings_tests::on_page(&mut s);
        const ROW: &str = "BenillaOptionsFrameContainerBodyKeybindingsRow";
        // Expand Movement and arm JUMP's first capsule: JUMP is not `runOnUp`, so the reference
        // accepts the wheel on it.
        s.run(&format!("{ROW}1Header:Click()")).expect("expand");
        s.run(&format!("{ROW}9Key1Button:Click()")).expect("select");
        assert_eq!(
            s.eval::<String>(&format!("return {ROW}9Description:GetText()"))
                .unwrap(),
            crate::ui_script::keybindings_tests::label(&s, "BINDING_NAME_JUMP", "JUMP")
        );
        assert!(s.bind_capture_armed());
        let mut app = vm_harness(s);

        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        {
            let s = app.world().non_send_resource::<UiScript>();
            assert_eq!(
                s.eval::<String>(r#"return GetBindingAction("MOUSEWHEELUP")"#)
                    .unwrap(),
                "JUMP",
                "a notch on an armed capsule is a bind"
            );
            assert!(!s.bind_capture_armed(), "the completed bind disarms");
        }

        // The bound chord now dispatches: the next notch jumps rather than binding.
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(state(&app).fired(by_name("JUMP")));
        assert!(
            !state(&app).fired(cmd::CAMERA_ZOOM_IN),
            "JUMP stole the wheel from the camera, the 1.12 steal law"
        );
    }

    /// A notch is a press and its release (`0x483d6f`, `0x483d82`), so a `runOnUp` binding on the
    /// wheel runs both halves in one frame. Seeded through the stored set, since `SetBinding`
    /// refuses a wheel chord on a press-and-release command: the hand-edited file case.
    #[test]
    fn a_wheel_notch_runs_both_halves_of_a_press_and_release_binding() {
        let mut script = UiScript::new().expect("VM");
        script.register_bindings(&registry_commands());
        script.register_addon_bindings(
            "ProbeAddon",
            &benilla_ui::bindings_xml::parse(PROBE_BINDINGS).expect("well-formed"),
        );
        script.seed_binding_set(
            1,
            Some(vec![(
                "PROBEHOLD".to_string(),
                vec!["MOUSEWHEELUP".to_string()],
            )]),
        );
        script.load_binding_set(1);
        let mut app = vm_harness(script);

        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
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
        assert!(state(&app).fired(cmd::JUMP), "the first press jumps");

        // (1) JUMP never latches, so only the pressed-key set stops a jump per repeat.
        repeat_key(&mut app, KeyCode::Space);
        app.update();
        assert!(
            !state(&app).fired(cmd::JUMP),
            "a repeat of a key already down is not a press"
        );

        // Movement, so the resume below has something to observe.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));

        // (3) The window is deactivated: bevy empties the keyboard plane, which unlatches through
        // the stuck-latch sweep and empties our pressed-key set.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        app.update();
        assert!(
            !state(&app).pressed(cmd::MOVE_FORWARD),
            "the deactivate releases the direction bits (0x514490)"
        );

        // (2) and (3): back in the window, still holding W. The OS calls this a repeat; we do not
        // have the key down, so it is a fresh press.
        repeat_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            state(&app).pressed(cmd::MOVE_FORWARD),
            "the first repeat after re-activation re-latches (2204)"
        );
    }

    #[test]
    fn held_commands_latch_across_frames_and_release_per_base_key() {
        let mut app = harness();
        // W and UP are both MOVEFORWARD: press both, release one, still moving.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));
        assert!(state(&app).just_pressed(cmd::MOVE_FORWARD), "press edge");
        press_key(&mut app, KeyCode::ArrowUp);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));
        assert!(
            !state(&app).just_pressed(cmd::MOVE_FORWARD),
            "second key on an already-held command is no new edge"
        );
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD), "UP still holds it");
        release_key(&mut app, KeyCode::ArrowUp);
        app.update();
        assert!(!state(&app).pressed(cmd::MOVE_FORWARD));
        // A repeat press (held-key auto-repeat) neither re-latches nor re-edges.
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        key(
            &mut app,
            KeyCode::KeyW,
            bevy::input::ButtonState::Pressed,
            true,
        );
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));
        assert!(!state(&app).just_pressed(cmd::MOVE_FORWARD));
    }

    /// The two-probe lookup: the exact chord, then one retry with the leftmost modifier dropped,
    /// never a third.
    #[test]
    fn a_press_probes_its_chord_then_falls_back_once() {
        // Shift held, W pressed: `SHIFT-W` is unbound, so the retry drops SHIFT and MOVEFORWARD
        // latches (the reference's `strchr` step, `0x4b7990`).
        let mut app = harness();
        press_key(&mut app, KeyCode::ShiftLeft);
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            state(&app).pressed(cmd::MOVE_FORWARD),
            "SHIFT-W falls back to W — the whole point of 1142"
        );
        // It unlatches on the base key with the modifier still down (the reference replays the
        // press-time chord at key-up, `0x483bd0`; latching the resolved command is equivalent).
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(!state(&app).pressed(cmd::MOVE_FORWARD));
        // Bare Z is the sheath toggle, ALT-Z is TOGGLEUI: the exact probe runs first, so a
        // fallback never overrides a real entry.
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(state(&app).fired(cmd::TOGGLE_SHEATH));
        assert!(!state(&app).fired(cmd::TOGGLE_UI));
        release_key(&mut app, KeyCode::KeyZ);
        press_key(&mut app, KeyCode::AltLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            state(&app).fired(cmd::TOGGLE_UI),
            "ALT-Z is TOGGLEUI (0870)"
        );
        assert!(!state(&app).fired(cmd::TOGGLE_SHEATH));
        // CTRL-ALT-Z fires nothing: the single strip drops the leftmost modifier (`ALT-CTRL-Z`
        // to `CTRL-Z`, unbound) and stops, never reaching ALT-Z or Z.
        release_key(&mut app, KeyCode::KeyZ);
        press_key(&mut app, KeyCode::ControlLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            !state(&app).fired(cmd::TOGGLE_UI) && !state(&app).fired(cmd::TOGGLE_SHEATH),
            "ALT-CTRL-Z probes CTRL-Z and stops — no second strip to ALT-Z or Z"
        );
        // TAB and SHIFT-TAB are both bound, so each resolves exactly.
        let mut app = harness();
        press_key(&mut app, KeyCode::Tab);
        app.update();
        assert!(state(&app).fired(cmd::TARGET_NEAREST_ENEMY));
        release_key(&mut app, KeyCode::Tab);
        press_key(&mut app, KeyCode::ShiftLeft);
        press_key(&mut app, KeyCode::Tab);
        app.update();
        assert!(state(&app).fired(cmd::TARGET_PREVIOUS_ENEMY));
        assert!(!state(&app).fired(cmd::TARGET_NEAREST_ENEMY));
        // Super is never a binding modifier: a Super-held press builds no chord, so no fallback.
        let mut app = harness();
        press_key(&mut app, KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(!state(&app).fired(cmd::TOGGLE_SHEATH));
    }

    /// The dev plane spends the keyboard's fallback probe and nothing else. Asserted on
    /// [`BindingDispatch::resolve`] because the colliding command is `Kind::Edge`, which the no-VM
    /// harness cannot observe.
    #[test]
    fn the_dev_plane_keeps_its_letters_without_stealing_bound_chords() {
        let by_name =
            |n: &str| Cmd(SPECS.iter().position(|s| s.name == n).expect("registered") as u16);
        let pet_paper_doll = by_name("TOGGLECHARACTER3"); // SHIFT-P
        let mut dispatch = BindingDispatch::test_defaults();
        let plane_p = Chord::parse("CTRL-SHIFT-P").expect("parses");
        // Ctrl+Shift+P is the perf HUD's; off the plane it falls back onto SHIFT-P...
        assert_eq!(
            dispatch.resolve(plane_p, false),
            Some(Bound::Spec(pet_paper_doll)),
            "without the plane rule the retry does reach SHIFT-P — this is what is being blocked"
        );
        // ...so on the plane the retry is suppressed.
        assert_eq!(dispatch.resolve(plane_p, true), None);
        // Only the fallback is suppressed: an exact CTRL-SHIFT- entry, as a player would bind,
        // still resolves.
        dispatch.map.insert(plane_p, Bound::Spec(pet_paper_doll));
        assert_eq!(
            dispatch.resolve(plane_p, true),
            Some(Bound::Spec(pet_paper_doll)),
            "the plane spends the retry, never the exact probe"
        );
        // SHIFT-P itself is still the pet paper doll.
        let shift_p = Chord::parse("SHIFT-P").expect("parses");
        assert_eq!(
            dispatch.resolve(shift_p, false),
            Some(Bound::Spec(pet_paper_doll))
        );
    }

    /// The pet bar routes on the CTRL digits and the number row is untouched: the two share base
    /// keys, kept apart only by the exact-modifier probe. CTRL-0 is slot 10.
    #[test]
    fn the_pet_lane_dispatches_on_the_ctrl_digits() {
        let by_name =
            |n: &str| Cmd(SPECS.iter().position(|s| s.name == n).expect("registered") as u16);
        let mut app = harness();
        press_key(&mut app, KeyCode::ControlLeft);
        press_key(&mut app, KeyCode::Digit1);
        app.update();
        assert!(state(&app).pressed(by_name("BONUSACTIONBUTTON1")));
        assert!(
            !state(&app).pressed(by_name("ACTIONBUTTON1")),
            "the modifier decides: CTRL-1 is not the number row's"
        );
        // The latch drops on the base key's release with Ctrl still held; in the VM that release
        // runs `PetActionButtonUp`, which casts.
        release_key(&mut app, KeyCode::Digit1);
        app.update();
        assert!(!state(&app).pressed(by_name("BONUSACTIONBUTTON1")));
        // CTRL-0 is slot 10.
        press_key(&mut app, KeyCode::Digit0);
        app.update();
        assert!(state(&app).pressed(by_name("BONUSACTIONBUTTON10")));
        // Bare 1 is still the action bar's, with no pet command in sight.
        let mut app = harness();
        press_key(&mut app, KeyCode::Digit1);
        app.update();
        assert!(state(&app).pressed(by_name("ACTIONBUTTON1")));
        assert!(!state(&app).pressed(by_name("BONUSACTIONBUTTON1")));
    }

    /// The typing gate blocks new presses and releases nothing already held: holding W and
    /// pressing ENTER keeps you running.
    #[test]
    fn the_typing_gate_blocks_new_input_but_a_held_binding_keeps_running() {
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));
        // A box takes focus. You keep running, and new presses type instead of binding.
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;
        app.update();
        assert!(
            state(&app).pressed(cmd::MOVE_FORWARD),
            "the capture edge is not a release — holding W keeps you running while you type"
        );
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(
            !state(&app).fired(cmd::SIT_OR_STAND),
            "typed keys are not bindings"
        );
        // Letting go still stops you, box focused or not.
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            !state(&app).pressed(cmd::MOVE_FORWARD),
            "the release is delivered regardless of focus"
        );
        // Focus drops; keys work again.
        release_key(&mut app, KeyCode::KeyX);
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = false;
        app.update();
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(state(&app).fired(cmd::SIT_OR_STAND));
    }

    /// A shown keyboard frame eating the key that closes it costs that key its binding and
    /// nothing else. `WorldMapFrame` is a keyboard-enabled fullscreen frame whose `OnKeyDown`
    /// runs `RunBinding("TOGGLEWORLDMAP")` itself (`WorldMapFrame.xml:629`), so it eats every key.
    #[test]
    fn a_keyboard_frame_eating_its_own_toggle_key_does_not_stop_a_held_run() {
        let mut app = harness();
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_FORWARD));

        // The map eats this frame's `M`. `M` is `Kind::Edge`, invisible to a no-VM harness, so
        // the suppression is asserted on `X` below.
        frame_ate(&mut app, &[KeyCode::KeyM]);
        press_key(&mut app, KeyCode::KeyM);
        app.update();
        assert!(
            state(&app).pressed(cmd::MOVE_FORWARD),
            "the frame ate the toggle key; you are still running (2196)"
        );

        // The eaten key loses its binding...
        release_key(&mut app, KeyCode::KeyM);
        app.update();
        frame_ate(&mut app, &[KeyCode::KeyX]);
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(
            !state(&app).fired(cmd::SIT_OR_STAND),
            "the frame ate this key: its binding must not also fire"
        );

        // ...and only that key.
        release_key(&mut app, KeyCode::KeyX);
        app.update();
        frame_ate(&mut app, &[KeyCode::KeyM]);
        press_key(&mut app, KeyCode::KeyX);
        app.update();
        assert!(
            state(&app).fired(cmd::SIT_OR_STAND),
            "consumption is per key, not per frame"
        );
        assert!(
            state(&app).pressed(cmd::MOVE_FORWARD),
            "and W has been held throughout"
        );
    }

    #[test]
    fn mouse_buttons_bind_only_over_the_world_and_the_wheel_respects_ui() {
        let mut app = harness();
        // BUTTON4 (winit Forward) is TOGGLEAUTORUN's second default.
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Forward,
            state: bevy::input::ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(state(&app).fired(cmd::TOGGLE_AUTORUN));
        // Over a UI frame the press belongs to the frame.
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Forward,
            state: bevy::input::ButtonState::Released,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Forward,
            state: bevy::input::ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!state(&app).fired(cmd::TOGGLE_AUTORUN));
        // MOVEANDSTEER (BUTTON3) is a HELD command on a mouse button: press latches, release ends.
        let mut app = harness();
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Middle,
            state: bevy::input::ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(state(&app).pressed(cmd::MOVE_AND_STEER));
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Middle,
            state: bevy::input::ButtonState::Released,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!state(&app).pressed(cmd::MOVE_AND_STEER));
        // The wheel: a notch fires CAMERAZOOMIN with its amount; over UI it belongs to the frame.
        let mut app = harness();
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 2.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(state(&app).fired(cmd::CAMERA_ZOOM_IN));
        assert_eq!(state(&app).amount(cmd::CAMERA_ZOOM_IN), 2.0);
        app.world_mut().resource_mut::<PointerOverUiPanel>().0 = true;
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 2.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(
            !state(&app).fired(cmd::CAMERA_ZOOM_IN),
            "a UI wheel is the frame's"
        );
    }

    /// B opens the backpack and SHIFT-B opens every bag, end to end: a real key event, the shipped
    /// defaults, the binding body and the stock Lua. The windows are the stock
    /// `ContainerFrame1..12`, recycled across containers, so the test asks `IsBagOpen(id)`; the
    /// interface loads through [`crate::ui_script::load_default_ui`], so it needs client data.
    #[test]
    fn b_opens_the_backpack_and_shift_b_opens_every_bag() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut script = UiScript::new().expect("VM");
        script.register_bindings(&registry_commands());
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
            "B does NOT open the equipped bag — this is the bug 1494 fixes"
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
}
