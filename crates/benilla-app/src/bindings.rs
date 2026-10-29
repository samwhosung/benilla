//! Key bindings: the one chord-to-command engine every rebindable input runs through. The key
//! table and the commands live engine-side ([`benilla_ui::script::keybind`]): the commands are the
//! `Bindings.xml` rows the core and each addon loaded, the live keys what `SetBinding` and the Key
//! Bindings window write. This module derives an exact-match dispatch map from the keys whenever
//! the table's generation moves, and runs a pressed chord's command.
//!
//! Dispatch ([`WorldFrameBindings`], the world frame's leg of each input record, in
//! [`crate::ui_script::UiInput`] after the interface's walk of that record), as
//! `CBindings::ExecuteBinding` (`0x4b7990`) and `RunCommand` (`0x4b7b50`) do:
//! - a press probes its exact chord, then once more with its leftmost modifier dropped
//!   ([`Chord::fallback`]);
//! - the resolved command's body runs with `keystate = "down"`, and the base key's release runs a
//!   `runOnUp` body again with `"up"`. UI focus suppresses presses and releases nothing already
//!   held; a latch ends only on its base key's release, the stuck-latch sweep (OS focus loss, the
//!   loading cover) or a VM swap;
//! - what the bodies call reaches the engine through the Lua binding functions
//!   ([`benilla_ui::script::BindingInput`]) as [`BindingsState`]'s [`Input`]s.
//!
//! Deviation: a press with Super (Cmd) held runs no binding, so a system chord such as Cmd+Tab or
//! Cmd+Q never acts in game. The reference has no such modifier and runs the bare key's binding:
//! the Mac enqueues the key with Command held and skips only its text (`0x8a519`), and Windows
//! hands `VK_LWIN`/`VK_RWIN` to the layout arm (`0x42da39`) like any key.
//!
//! Persistence: `benilla-config/bindings/account.txt` and `<Realm>-<Char>.txt` ([`store`]); the
//! character file's existence is the character-set state, as in the reference.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::MouseScrollUnit;
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_ui::script::keybind::KeybindRequest;
use benilla_ui::script::{KeyUp, KeyWalk, UiScript};

use crate::client_state::InWorldGated;
use crate::textinput::keymap::Mods;
#[cfg(test)]
use crate::ui_script::{PlayerUiHover, UiKeyboardCapture};

pub(crate) mod chord;
mod layout;
mod script_input;
pub(crate) mod store;

use chord::{BindKey, Chord};
pub(crate) use layout::LayoutNames;
pub(crate) use script_input::Input;

/// The chord-to-command map, rebuilt whenever the engine table's generation moves. Probe it
/// through [`BindingDispatch::resolve`]: the lookup is two probes.
#[derive(Resource, Default)]
pub(crate) struct BindingDispatch {
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

/// The engine's input state the binding functions drive, the reference's `[InputControl+4]` and
/// the camera's axes, which engine-side consumers read instead of raw keys: the [`Input`]s the
/// bodies' Lua calls set, each inside its call ([`benilla_ui::script::ScriptCall::Input`]).
#[derive(Resource, Default)]
pub(crate) struct BindingsState {
    /// The held bits a Start set and no Stop has cleared.
    held: Vec<Input>,
    /// This frame's edges, which the frame's readers take after the binding pass.
    edges: Edges,
    /// The edges of the calls made after this frame's readers ran, for the next frame's.
    carry: Edges,
    /// The binding pass runs ([`BindingHead`] to [`BindingTail`]), ahead of every reader.
    open: bool,
}

/// One frame's binding edges.
#[derive(Default)]
struct Edges {
    /// Inputs whose Start, or whose one-shot, came this frame (the press edge).
    just: Vec<Input>,
    /// One-shots fired this frame.
    fired: Vec<Input>,
    /// Amount per one-shot this frame: a zoom's yards, else 1.0.
    amounts: Vec<(Input, f32)>,
    /// This frame's `MoveView*` calls in call order, `(axis, start)`, for the camera's axes.
    view_calls: Vec<(script_input::ViewAxis, bool)>,
    /// The edges whose `SetCommands` work ran inside their call ([`BindingsState::call`]).
    settled: Vec<Input>,
}

/// The world frame's binding latches and the keys it believes are down: `CBindings`' state, not
/// the input control's.
#[derive(Resource, Default)]
pub(crate) struct BindingLatches {
    /// Live latches, (base input, command): a press not yet released, whose release runs the
    /// command's `runOnUp` half. The reference replays the press-time chord at key-up (`0x483bd0`);
    /// latching the resolved command is equivalent.
    latched: Vec<(Held, String)>,
    /// The keys we believe are down: a press of one is a repeat. The reference classifies
    /// auto-repeat off its own list (`0x4248b3`), never the Win32 `lParam` repeat bit.
    /// Normalized like [`latched`](Self::latched) and reconciled against `ButtonInput` every
    /// pass, so a window deactivate empties it and a key held across an alt-tab resumes, as in
    /// the reference. Keyboard only: mouse edges cannot repeat.
    down: KeyDownList,
}

/// The keys believed down, the reference's own list (`0x4248b3`): a press of a key on it is an
/// auto-repeat, whatever the OS flagged. One instance is the bindings', another the UI key feed's
/// ([`crate::ui_script`]), which decides repeat for the frames by the same rule and reconcile.
#[derive(Default)]
pub(crate) struct KeyDownList(Vec<KeyCode>);

impl KeyDownList {
    /// Drop what is no longer physically down, per bevy's button plane: a window deactivate
    /// empties it (the reference's `0x424790` wipe), so the first repeat after an alt-tab is a
    /// fresh press and re-latches.
    pub(crate) fn reconcile(&mut self, keys: &ButtonInput<KeyCode>) {
        self.0.retain(|&kc| key_physically_down(kc, keys));
    }

    /// Record a press; whether the key was already down, which makes it a repeat.
    pub(crate) fn press(&mut self, key: KeyCode) -> bool {
        let key = chord::normalize_key(key);
        let repeat = self.0.contains(&key);
        if !repeat {
            self.0.push(key);
        }
        repeat
    }

    pub(crate) fn release(&mut self, key: KeyCode) {
        let key = chord::normalize_key(key);
        self.0.retain(|&kc| kc != key);
    }
}

/// `ENTER` is down while either physical key is, since [`chord::normalize_key`] folds
/// `NUMPADENTER` into it.
fn key_physically_down(kc: KeyCode, keys: &ButtonInput<KeyCode>) -> bool {
    match kc {
        KeyCode::Enter => keys.pressed(KeyCode::Enter) || keys.pressed(KeyCode::NumpadEnter),
        kc => keys.pressed(kc),
    }
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
        self.edges.just.contains(&i)
    }
    /// Did this frame's edge of `i` come from a call that ran its `SetCommands` work inside it
    /// ([`Self::call`])?
    pub(crate) fn settled(&self, i: Input) -> bool {
        self.edges.settled.contains(&i)
    }
    /// Did this one-shot fire this frame?
    pub(crate) fn fired(&self, i: Input) -> bool {
        self.edges.fired.contains(&i)
    }
    /// Fire the one-shot `i` this frame, as a body's Lua call would.
    #[cfg(test)]
    pub(crate) fn fire_for_test(&mut self, i: Input) {
        self.edges.fired.push(i);
    }
    /// End the frame's one-shots.
    #[cfg(test)]
    pub(crate) fn clear_for_test(&mut self) {
        self.edges.fired.clear();
    }
    /// The binding pass runs, so a call's edge is this frame's ([`Self::call`]).
    #[cfg(test)]
    pub(crate) fn open_for_test(&mut self) {
        self.open = true;
    }
    /// Total amount for a one-shot this frame (0.0 when idle): the camera zoom's yards.
    pub(crate) fn amount(&self, i: Input) -> f32 {
        self.edges
            .amounts
            .iter()
            .filter(|&&(a, _)| a == i)
            .map(|&(_, v)| v)
            .sum()
    }
    /// This frame's `MoveView*` calls in call order, `(axis, start)`.
    pub(crate) fn view_calls(&self) -> &[(script_input::ViewAxis, bool)] {
        &self.edges.view_calls
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
    /// The account-data sections' files, the reference's `bindings-cache.wtf` pair.
    account_cache: Option<std::path::PathBuf>,
    character_cache: Option<std::path::PathBuf>,
    /// Whether this VM's set 1 and set 2 wait for their sections (2 and 3): nothing read, so
    /// nothing saved over them (`0x4b6600`, `0x4b6611` through `0x5afed0`).
    held: crate::ui_script::VmMemo<[bool; 2]>,
}

/// This module's systems inside [`crate::ui_script::UiInput`]: the pass's head ([`BindingHead`])
/// and its tail ([`BindingTail`]). The interface's walk of the frame's records runs between them,
/// each record's binding inside it, as the world frame is the walk's last frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BindingSet;

/// The pass's head, ahead of the frame's records: the dispatch map and the frame's clears.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BindingHead;

/// The pass's tail, after the frame's records: the stuck-latch sweep and the calls the bodies made.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BindingTail;

pub(crate) struct BindingsPlugin;

impl Plugin for BindingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(layout::plugin)
            .configure_sets(
                Update,
                (BindingHead, BindingTail)
                    .in_set(crate::ui_script::UiInput)
                    .in_set(BindingSet)
                    .in_set(InWorldGated),
            )
            .configure_sets(Update, BindingHead.before(BindingTail))
            .init_resource::<BindingDispatch>()
            .init_resource::<BindingsState>()
            .init_resource::<BindingLatches>()
            .init_resource::<BindingFiles>()
            .add_systems(
                Update,
                (
                    // The key sets are seeded at the VM's birth, not here: `seed_bindings_for_vm`
                    // runs in `load_ingame_ui_on_world_entry`, before FrameXML and every addon;
                    // the commands come with the load.
                    (sync_dispatch, begin_dispatch).chain().in_set(BindingHead),
                    end_dispatch.in_set(BindingTail),
                    // After the tick: the requests are Lua's own (`SaveBindings`), queued by
                    // handlers the tick dispatched, and a save must not wait a frame.
                    drain_binding_requests.in_set(crate::ui_script::UiDrain),
                    reread_arrived_bindings
                        .run_if(crate::ui_script::ingame_ui_up)
                        .in_set(crate::ui_script::UiFeed)
                        .in_set(InWorldGated),
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
    let account = crate::local_state::bindings_account_path();
    // No roster identity (a rigged or capture run) leaves set 2 unseeded.
    let id = world
        .get_resource::<crate::glue_session::roster::Roster>()
        .and_then(crate::ui_macro::identity);
    let character = id
        .as_ref()
        .and_then(|(realm, name)| crate::local_state::bindings_character_path(realm, name));
    // The reader (`0x4b6240`) registers for each section through `0x5afe50` (`0x4b62ef`,
    // `0x4b6300`): a set is read now on a settled section, else held until it settles
    // ([`reread_arrived_bindings`]), the live table empty until set 2's read chooses.
    let held = world
        .get_resource::<crate::account_data::AccountData>()
        .map_or([false; 2], |data| {
            crate::account_data::BINDINGS.map(|section| !data.register(section))
        });
    if !held[0] {
        read_account_set(script, &account);
    }
    if !held[1] {
        read_character_set(script, &character);
    }
    if let Some(mut files) = world.get_resource_mut::<BindingFiles>() {
        *files.held.get(script) = held;
        files.account = account;
        files.character = character;
        files.account_cache = crate::local_state::bindings_cache_account_path();
        files.character_cache = id.as_ref().and_then(|(realm, name)| {
            crate::local_state::bindings_cache_character_path(realm, name)
        });
    }
}

/// Section 2's read (`0x4b6240` with set 1): set 1 replaced by the account file, emptied first
/// (`0x4b6161`); a file that does not read leaves it as it stands (`0x4b624c`, `0x4b6259`).
fn read_account_set(script: &mut UiScript, account: &Option<std::path::PathBuf>) {
    if let Some(overrides) = read_diff(account) {
        let defaults = script.default_bindings();
        script.seed_binding_set(1, Some(store::resolve(&overrides, &defaults)));
    }
}

/// Section 3's read (`0x4b6240` with set 2): set 2 from the character file as
/// [`read_account_set`] reads set 1, then the set choice (`0x4b629a` → `0x4b64b0`): the
/// character's set in use when it has keys, else set 1 as it then stands, the defaults when it is
/// still empty (`0x4b64e2`).
fn read_character_set(script: &mut UiScript, character: &Option<std::path::PathBuf>) {
    if let Some(overrides) = read_diff(character) {
        let defaults = script.default_bindings();
        script.seed_binding_set(2, Some(store::resolve(&overrides, &defaults)));
        info!("bindings: character-specific set read");
    }
    script.load_binding_set(if script.character_bindings_exist() {
        2
    } else {
        1
    });
}

/// A binding section that settled: its set read again, as the reference's reader (`0x4b6240`)
/// runs once its section settles (`0x5afaca`, `0x5afd19`), in the table's order.
fn reread_arrived_bindings(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut files: ResMut<BindingFiles>,
    data: Option<Res<crate::account_data::AccountData>>,
) {
    let Some(data) = data else { return };
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        let [account, character] = crate::account_data::BINDINGS.map(|s| data.take_owed(s));
        if account {
            read_account_set(script, &files.account);
            files.held.get(script)[0] = false;
        }
        if character {
            read_character_set(script, &files.character);
            files.held.get(script)[1] = false;
        }
    })
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
/// deletes the character file. Left at the frame, not in the call: the call's own work, the copy
/// and the dirty mark (`0x4b8260` → `0x4b6310`), is the VM's and signals nothing (only a copy into
/// the live set does, `0x4b6498`), and the reference writes the files at the interface's shutdown
/// (`0x490bd0` → `0x4b65e0`), which no script reads.
fn drain_binding_requests(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut files: ResMut<BindingFiles>,
    data: Option<Res<crate::account_data::AccountData>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        let [account_held, character_held] = *files.held.get(script);
        for req in script.take_keybind_requests() {
            let KeybindRequest::Save(which) = req;
            // The set as the call stored it (`0x4b6310`), not the live table a later binding in
            // the frame moved. None: a later `SaveBindings(1)` in the frame dropped set 2, and
            // its own request writes the files that leaves.
            let Some(saved) = script.stored_binding_snapshot(which) else {
                continue;
            };
            let defaults = script.default_bindings();
            let text = store::to_diff(&saved, &defaults);
            // The section's file beside the diff, the whole table in the reference's form
            // (`0x4b65b0`).
            let table = store::to_cache(&store::from_diff(&text), &defaults);
            let (path, cache, held) = match which {
                1 => (&files.account, &files.account_cache, account_held),
                2 => (&files.character, &files.character_cache, character_held),
                _ => continue,
            };
            // A held set's section is not written over (`0x4b6600`, `0x4b6611` → `0x5afee0`).
            if !held {
                write_file(path, &text);
                write_file(cache, &table);
            }
            if which == 1 && !character_held {
                if let Some(chr) = &files.character {
                    match std::fs::remove_file(chr) {
                        Ok(()) => info!("bindings: character-specific set deleted"),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => warn!("bindings: deleting {}: {e}", chr.display()),
                    }
                }
                // The character's section file emptied, as the account set is the one in use
                // (`0x4b65c6` writes set 2 only when it is current).
                write_file(&files.character_cache, "");
            }
            // Owed the shutdown's write: the account set's save both sections, the character's its
            // own (`0x4b65ed`, `0x4b6617`).
            if let Some(data) = &data {
                data.saved(if which == 1 { &[2, 3] } else { &[3] });
            }
        }
    })
}

/// Write one bindings file, if this session has it.
fn write_file(path: &Option<std::path::PathBuf>, text: &str) {
    if let Some(path) = path {
        if let Err(e) = crate::local_state::write_atomic(path, text) {
            warn!("bindings: saving {}: {e}", path.display());
        }
    }
}

/// Rebuild the dispatch map when the engine table moved (a rebind, a set switch, the seed, a
/// load, a registration), and fire the `UPDATE_BINDINGS` a host-side load owes so the Lua
/// consumers (the action bar's hotkey corners) repaint. The reference signals it from `SetBinding`
/// (`0x4b805b`), which signals in place, and from the copy of a set into the live table
/// (`0x4b649d`); reading a `Bindings.xml` (`0x4b6f70`) signals nothing.
fn sync_dispatch(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut dispatch: ResMut<BindingDispatch>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else { return };
        let generation = script.keybinds_generation();
        if *dispatch.seen_generation.get(script) != Some(generation) {
            *dispatch.seen_generation.get(script) = Some(generation);
            dispatch.map.clear();
            for (key, command) in script.binding_keys() {
                match Chord::parse(&key) {
                    Some(ch) => {
                        dispatch.map.insert(ch, command);
                    }
                    None => warn!("bindings: {command}: unpressable chord '{key}' (unknown token)"),
                }
            }
        }
        if script.take_keybinds_signal_owed() {
            script.fire_event("UPDATE_BINDINGS", vec![]);
        }
    })
}

/// A wheel delta in lines (notches), whatever unit the OS reported: a trackpad's `Pixel` deltas
/// go through Bevy's own conversion factor. Every wheel reader shares it.
pub(crate) fn wheel_lines(unit: MouseScrollUnit, dy: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => dy,
        MouseScrollUnit::Pixel => dy / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
    }
}

/// Whether the OS turns Shift plus the vertical wheel into a horizontal scroll before the window
/// sees it: macOS does, Windows (`WM_MOUSEWHEEL` with `MK_SHIFT` in its key flags) does not.
pub(crate) const OS_REMAPS_SHIFT_WHEEL: bool = cfg!(target_os = "macos");

/// The vertical travel of a scroll: `y`, except that where the OS remapped Shift plus the wheel to
/// a horizontal-only scroll (`remapped`), that scroll is the wheel notch it came from. It undoes
/// the OS's remap so the reference's `WM_MOUSEWHEEL` arrives as it would on Windows (`0x42d54f`),
/// SHIFT-MOUSEWHEELUP and DOWN stay bindable and Shift plus the wheel still counts as input.
pub(crate) fn notch_travel_with(x: f32, y: f32, shift: bool, remapped: bool) -> f32 {
    if remapped && shift && y == 0.0 {
        x
    } else {
        y
    }
}

/// [`notch_travel_with`] on this platform.
pub(crate) fn notch_travel(x: f32, y: f32, shift: bool) -> f32 {
    notch_travel_with(x, y, shift, OS_REMAPS_SHIFT_WHEEL)
}

/// The chord a wheel notch runs: the modifiers its record holds and the direction (`0x4b6aa0`
/// names it from the event). `None` while Super is held, where the notch runs no binding.
fn wheel_chord(mods: Mods, up: bool) -> Option<Chord> {
    if mods.sup {
        return None;
    }
    Some(Chord {
        alt: mods.alt,
        ctrl: mods.ctrl,
        shift: mods.shift,
        key: if up {
            BindKey::WheelUp
        } else {
            BindKey::WheelDown
        },
    })
}

/// Whether the world frame's wheel slot takes a notch: it runs the notch's chord and answers the
/// sum of the two `ExecuteBinding` results (`0x483d87`), which is 0 for a chord with no binding.
pub(crate) fn world_takes_wheel(dispatch: Option<&BindingDispatch>, mods: Mods, up: bool) -> bool {
    match (dispatch, wheel_chord(mods, up)) {
        (Some(dispatch), Some(chord)) => dispatch.resolve(chord, false).is_some(),
        (None, _) => true,
        (Some(_), None) => false,
    }
}

/// Whole notches out of a stream of line deltas, carrying the fraction between frames.
///
/// Deviation: a trackpad's `Pixel` deltas are carried until they make a line, and step at most
/// once per event ([`crate::ui_script::Record::Wheel`]). The reference steps on every
/// `WM_MOUSEWHEEL`, whatever its delta (`0x76c1a9`); carried, a trackpad does not zoom or scroll
/// at its event rate.
///
/// For consumers that act per notch off the sign alone: the UI's `OnMouseWheel` (stock
/// `ScrollFrameTemplate_OnMouseWheel` moves half a pane per call, `UIPanelTemplates.lua:150`) and
/// the world frame's binding, each wheel record's ([`crate::ui_script::Record::Wheel`]). A
/// reversal drops the carried fraction: it is a new gesture.
#[derive(Default)]
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

/// The dispatch pass's head (see the module doc), ahead of the frame's records: the frame's
/// activity cleared, the bulk clears, Lua's binding calls since the last pass, and the
/// pressed-key reconcile.
fn begin_dispatch(
    // The input state with the host, whose calls write it ([`BindingsState::call`]).
    mut vm: crate::script_calls::VmWith<ResMut<'static, BindingsState>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut latches: ResMut<BindingLatches>,
    mut same_vm: Local<crate::ui_script::VmMemo<bool>>,
    mut focus_lost: MessageReader<bevy::input::keyboard::KeyboardFocusLost>,
    cover: Option<Res<crate::loading_screen::LoadingScreen>>,
) {
    let (script, set) = vm.parts();
    let mut state = set.p0();
    state.begin_pass();
    // Deviation: latches die with the VM that made them, because a latch names a command of that
    // VM's table and releasing it against a new one would run another load's up-half. The
    // reference keeps a held key running through `ReloadUI`; here it re-latches on its next press.
    // The held bits die with it, as the key's Stop would have cleared them.
    if let Some(script) = script {
        if same_vm.claim(script) {
            latches.latched.clear();
            state.held.clear();
        }
        // Lua's binding calls made with no host lent since the last pass, before this frame's
        // keys.
        let inputs = script.take_binding_input();
        state.apply_script_input(inputs);
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
    // ── The pressed-key reconcile ── against bevy's button planes, before this frame's records: a
    // release the window never saw drops off the list, and a window deactivate empties it, the
    // reference's `0x424790` wipe, so a key held across an alt-tab re-latches on its next repeat.
    latches.down.reconcile(&keys);
}

/// How the interface's walk left one key record, which gates the world frame's binding.
#[derive(Clone, Copy, Default)]
pub(crate) struct KeyVerdict {
    /// A focused EditBox holds the keyboard ([`crate::ui_script::UiKeyboardCapture::typing`]).
    pub(crate) typing: bool,
    /// The focused box passes the arrows on ([`crate::ui_script::UiKeyboardCapture::arrows_fall_through`]).
    pub(crate) arrows_fall_through: bool,
    /// A keyboard frame or a box took this record's key-down walk.
    pub(crate) eaten: bool,
}

/// The world frame's leg of each input record, last in the interface's walk: its key and mouse
/// slots run the binding (`0x483b70`, `0x483bd0`, `0x483c40`, `0x483cb0`). The caller lends the
/// call host around the leg, so a body's calls apply inside it.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WorldFrameBindings<'w> {
    dispatch: Option<Res<'w, BindingDispatch>>,
    state: Option<ResMut<'w, BindingLatches>>,
}

impl WorldFrameBindings<'_> {
    /// One key record, after the interface's walk left `verdict`.
    pub(crate) fn key(
        &mut self,
        script: Option<&mut UiScript>,
        ev: &KeyboardInput,
        mods: Mods,
        verdict: KeyVerdict,
        layout: &LayoutNames,
    ) {
        if let (Some(dispatch), Some(state)) = (self.dispatch.as_deref(), self.state.as_deref_mut())
        {
            key_leg(dispatch, state, script, ev, mods, verdict, layout);
        }
    }

    /// One button record; `over_ui` is whether a frame was hovered when it came.
    pub(crate) fn button(
        &mut self,
        script: Option<&mut UiScript>,
        button: MouseButton,
        pressed: ButtonState,
        mods: Mods,
        over_ui: bool,
    ) {
        if let (Some(dispatch), Some(state)) = (self.dispatch.as_deref(), self.state.as_deref_mut())
        {
            button_leg(dispatch, state, script, button, pressed, mods, over_ui);
        }
    }

    /// Whether the world frame's wheel slot takes a notch of a record holding `mods`
    /// ([`world_takes_wheel`]).
    pub(crate) fn takes_wheel(&self, mods: Mods, up: bool) -> bool {
        world_takes_wheel(self.dispatch.as_deref(), mods, up)
    }

    /// One wheel notch the interface's walk left to the world frame (`0x483d20`): a press and its
    /// release, back to back, as the reference hands the chord to `CBindings::ExecuteBinding` with
    /// `isDown=1` (`0x483d6f`) then `isDown=0` (`0x483d82`). A `runOnUp` command runs both halves;
    /// a plain one's up leg is the `RunCommand` (`0x4b7bf1`) no-op.
    pub(crate) fn wheel(&mut self, mut script: Option<&mut UiScript>, mods: Mods, up: bool) {
        let (Some(dispatch), Some(state)) = (self.dispatch.as_deref(), self.state.as_deref_mut())
        else {
            return;
        };
        if let Some(command) = wheel_chord(mods, up).and_then(|c| dispatch.resolve(c, false)) {
            press(state, script.as_deref_mut(), command, Held::Wheel);
            release(state, script, Held::Wheel);
        }
    }
}

/// The dev overlays' plane, Ctrl+Shift without Alt (`modkeys::dev_chord`, which Super blocks too,
/// as the `sup` gate keeps any Super-held press from dispatching), which costs the keyboard its
/// fallback probe (`BindingDispatch::resolve`). Only in a build with dev affordances: a player
/// build keeps the reference's `CTRL-SHIFT-P` fallback to `SHIFT-P`.
fn dev_plane(mods: Mods) -> bool {
    mods.ctrl && mods.shift && !mods.alt && crate::run_mode::dev_affordances()
}

/// A key record's binding: a press runs its command, gated on ownership and the capture arm; a
/// release unlatches, runs the `runOnUp` up-half and sends the key-up walk.
///
/// Who owns the key: a focused EditBox eats every key while it holds focus; a shown keyboard frame
/// ate the key in `verdict.eaten`. Both suppress a press and nothing else: in the reference a
/// focused box makes the movement handlers no-ops, so the direction bits are frozen, not cleared.
/// `0x514490`, which does clear them, is the OS window-deactivate handler (sole caller
/// `0x493058`, off the root's WM_ACTIVATE slot `[root+0x1134]`); here bevy's `KeyboardFocusLost`
/// and `ButtonInput::release_all` reach the stuck-latch sweep.
fn key_leg(
    dispatch: &BindingDispatch,
    state: &mut BindingLatches,
    mut script: Option<&mut UiScript>,
    ev: &KeyboardInput,
    mods: Mods,
    verdict: KeyVerdict,
    layout: &LayoutNames,
) {
    let Mods {
        shift,
        ctrl,
        alt,
        sup,
    } = mods;
    let dev_plane = dev_plane(mods);
    let key = chord::normalize_key(ev.key_code);
    match ev.state {
        ButtonState::Pressed => {
            // A focused box in alt-arrow mode declines the four arrows, so their bindings fire
            // while typing (`UiKeyboardCapture::arrows_fall_through`).
            let arrow_exempt = verdict.arrows_fall_through
                && matches!(
                    ev.key_code,
                    KeyCode::ArrowLeft
                        | KeyCode::ArrowRight
                        | KeyCode::ArrowUp
                        | KeyCode::ArrowDown
                );
            // The box declines PRINTSCREEN before it looks at focus (`0x77b17f`), so the walk goes
            // on and the world frame's binding runs while typing.
            let arrow_exempt = arrow_exempt
                || chord::key_token(ev.key_code, layout)
                    == Some(chord::KeyName::Word("PRINTSCREEN"));
            // A repeat is a key we already believe is down, the reference's test. Recorded before
            // the gates below, so a key held through a focused box stays down.
            let repeat = state.down.press(key);
            // `sup`: the Super deviation (module doc).
            if (verdict.typing && !arrow_exempt) || verdict.eaten || sup || repeat {
                return;
            }
            if state.latched.iter().any(|(k, _)| *k == Held::Key(key)) {
                return; // already latched (missed release would double-latch)
            }
            // Named under the layout this press was typed in; a key 1.12 cannot name binds
            // nothing.
            let Some(name) = chord::key_token(ev.key_code, layout) else {
                return;
            };
            let chord = Chord {
                alt,
                ctrl,
                shift,
                key: BindKey::Key(name),
            };
            if let Some(command) = dispatch.resolve(chord, dev_plane) {
                // The world frame's key-down ran it and returned that (`0x483b70`), so its key-up
                // and repeats are the world frame's, not a stale earlier consumer's.
                if let Some(s) = script.as_deref_mut() {
                    s.note_world_key(&name.to_string());
                }
                press(state, script, command, Held::Key(key));
            }
        }
        ButtonState::Released => {
            state.down.release(key);
            release(state, script.as_deref_mut(), Held::Key(key));
            // The release, to the frame that took the press (`0x765fd0`), here and not in the key
            // feed so it follows this key's binding release: a binding's press has made the world
            // frame its consumer. PRINTSCREEN arrives as an up alone on Windows (`VK_SNAPSHOT`
            // sends no `WM_KEYDOWN`), and with no consumer on record the client runs the key-down
            // walk on it (`0x76603c`-`0x76604d`), the world frame's binding last: the up then runs
            // the chord as a press, with no latch since no release follows.
            let Some(name) = chord::walk_name(ev.key_code, layout) else {
                return;
            };
            let Some(s) = script else {
                return;
            };
            let up = s.key_up(&name);
            if up == KeyUp::Walked(KeyWalk::Declined) && !sup {
                if let Some(token) = chord::key_token(ev.key_code, layout) {
                    let chord = Chord {
                        alt,
                        ctrl,
                        shift,
                        key: BindKey::Key(token),
                    };
                    if let Some(command) = dispatch.resolve(chord, dev_plane) {
                        press(state, Some(s), command, Held::Key(key));
                        state.latched.retain(|(k, _)| *k != Held::Key(key));
                    }
                }
            }
        }
    }
}

/// The five buttons a chord can name (`BUTTON1` to `BUTTON5`).
const BIND_BUTTONS: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::Forward,
    MouseButton::Back,
];

/// A button record's binding: a press only with the cursor over the world (a frame owns its
/// clicks), a release always.
fn button_leg(
    dispatch: &BindingDispatch,
    state: &mut BindingLatches,
    script: Option<&mut UiScript>,
    button: MouseButton,
    pressed: ButtonState,
    mods: Mods,
    over_ui: bool,
) {
    if !BIND_BUTTONS.contains(&button) {
        return;
    }
    match pressed {
        ButtonState::Pressed => {
            if mods.sup || over_ui || state.latched.iter().any(|(k, _)| *k == Held::Mouse(button)) {
                return;
            }
            let chord = Chord {
                alt: mods.alt,
                ctrl: mods.ctrl,
                shift: mods.shift,
                key: BindKey::Mouse(button),
            };
            if let Some(command) = dispatch.resolve(chord, false) {
                press(state, script, command, Held::Mouse(button));
            }
        }
        ButtonState::Released => release(state, script, Held::Mouse(button)),
    }
}

/// The dispatch pass's tail, after the frame's records: the stuck-latch sweep, and the calls the
/// pass's bodies made.
fn end_dispatch(
    // The input state with the host, whose calls write it ([`BindingsState::call`]).
    mut vm: crate::script_calls::VmWith<ResMut<'static, BindingsState>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut latches: ResMut<BindingLatches>,
) {
    vm.run(|mut script| {
        // ── The stuck-latch sweep ── a release the window never saw: a latch whose base key reads
        // up unlatches now and runs its up-half. The reference's two bulk clears land here too,
        // since bevy zeroes `ButtonInput` for both (see `begin_dispatch`). UI keyboard focus never
        // reaches here.
        let mut stuck: Vec<Held> = Vec::new();
        for (k, _) in &latches.latched {
            if !physically_down(*k, &keys, &buttons) && !stuck.contains(k) {
                stuck.push(*k);
            }
        }
        for k in stuck {
            release(&mut latches, script.as_deref_mut(), k);
        }
    });
    // The calls this pass's binding bodies made with no host lent act this frame, as the
    // reference's return from `0x515090` has already moved the bit.
    let (script, set) = vm.parts();
    let mut state = set.p0();
    if let Some(s) = script {
        let inputs = s.take_binding_input();
        state.apply_script_input(inputs);
    }
    state.end_pass();
}

/// The world frame's leg of each record alone, for a test of the bindings without the interface's
/// walk: the verdicts are what the test left in [`UiKeyboardCapture`] and [`PlayerUiHover`], and
/// every wheel notch is the world frame's.
#[cfg(test)]
fn dispatch_records(
    mut script: Option<NonSendMut<UiScript>>,
    mut host: Option<crate::script_calls::ScriptCallHost>,
    records: Res<crate::ui_script::InputRecords>,
    capture: Res<UiKeyboardCapture>,
    hover: Res<PlayerUiHover>,
    layout: Res<LayoutNames>,
    mut legs: WorldFrameBindings,
) {
    use crate::ui_script::Record;
    let mut run = |mut s: Option<&mut UiScript>| {
        for r in records.iter() {
            match &r.record {
                Record::Key(ev) => {
                    let verdict = KeyVerdict {
                        typing: capture.typing,
                        arrows_fall_through: capture.arrows_fall_through,
                        eaten: capture.consumed.contains(&ev.key_code),
                    };
                    legs.key(s.as_deref_mut(), ev, r.mods, verdict, &layout);
                }
                Record::Button { button, state } => {
                    legs.button(s.as_deref_mut(), *button, *state, r.mods, hover.0.is_some());
                }
                Record::Move => {}
                Record::Wheel { up } => legs.wheel(s.as_deref_mut(), r.mods, *up),
            }
        }
    };
    match script.as_mut() {
        Some(s) => crate::script_calls::lent(s, host.as_mut(), |s| run(Some(s))),
        None => run(None),
    }
}

/// The bindings pass once, as the frame runs it, for a test over the whole client: the dispatch
/// map from the VM's keys, then this frame's key messages and button edges as its records, with the
/// call host the world lends.
#[cfg(test)]
pub(crate) fn dispatch_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    crate::ui_script::records_from_planes(world);
    world.run_system_once(sync_dispatch).expect("the map syncs");
    world
        .run_system_once(begin_dispatch)
        .expect("the pass begins");
    world
        .run_system_once(dispatch_records)
        .expect("the records dispatch");
    world.run_system_once(end_dispatch).expect("the pass ends");
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
        Held::Key(kc) => key_physically_down(kc, keys),
        Held::Mouse(b) => buttons.pressed(b),
        // A notch is a press and a release in one frame; it is never "held".
        Held::Wheel => false,
    }
}

/// One matching press: the command's body with `keystate = "down"`, and a latch on its input so
/// the release can run the up-half.
fn press(state: &mut BindingLatches, script: Option<&mut UiScript>, command: &str, key: Held) {
    if let Some(s) = script {
        if let Err(e) = s.execute_binding(command, true) {
            warn!("bindings({command}): {e}");
        }
    }
    state.latched.push((key, command.to_owned()));
}

/// A base key's release: drop its latch and run the command's `runOnUp` up-half, even while
/// typing (the reference completes a pressed binding's release regardless of focus).
fn release(state: &mut BindingLatches, mut script: Option<&mut UiScript>, key: Held) {
    let mut i = 0;
    while i < state.latched.len() {
        if state.latched[i].0 == key {
            let (_, command) = state.latched.remove(i);
            if let Some(s) = script.as_deref_mut() {
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
    use bevy::input::mouse::MouseButtonInput;

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
        send_key(
            app,
            KeyboardInput {
                key_code: k,
                logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                state,
                text: None,
                repeat,
                window: Entity::PLACEHOLDER,
            },
        );
    }
    /// A key message as winit hands it over: typed, and on the window's stream the records read.
    fn send_key(app: &mut App, ev: KeyboardInput) {
        app.world_mut().write_message(ev.clone());
        app.world_mut()
            .write_message(bevy::window::WindowEvent::KeyboardInput(ev));
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
        let ev = MouseButtonInput {
            button,
            state,
            window: Entity::PLACEHOLDER,
        };
        app.world_mut().write_message(ev);
        app.world_mut()
            .write_message(bevy::window::WindowEvent::MouseButtonInput(ev));
    }
    /// `notches` wheel messages of one line each (up positive), on the window's stream as winit
    /// hands them over; the harness gives every step to the world frame ([`dispatch_records`]).
    fn wheel(app: &mut App, notches: i32) {
        for _ in 0..notches.unsigned_abs() {
            app.world_mut()
                .write_message(bevy::window::WindowEvent::MouseWheel(
                    bevy::input::mouse::MouseWheel {
                        unit: MouseScrollUnit::Line,
                        x: 0.0,
                        y: notches.signum() as f32,
                        window: Entity::PLACEHOLDER,
                    },
                ));
        }
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

    /// The with-VM harness: the frame's records read off the window's stream, then the real
    /// [`sync_dispatch`] and the pass's head, the world frame's leg of each record
    /// ([`dispatch_records`]) and the tail, so the map's derivation is under test too.
    fn vm_harness(script: UiScript) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
            .add_message::<bevy::window::WindowEvent>()
            .init_resource::<crate::ui_script::InputRecords>()
            .init_resource::<UiKeyboardCapture>()
            .init_resource::<PlayerUiHover>()
            .init_resource::<BindingsState>()
            .init_resource::<BindingLatches>()
            .init_resource::<BindingDispatch>()
            .init_resource::<LayoutNames>()
            .add_systems(
                Update,
                (
                    crate::ui_script::collect_input_records,
                    sync_dispatch,
                    begin_dispatch,
                    dispatch_records,
                    end_dispatch,
                )
                    .chain(),
            );
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
        let script = core_script();
        script.register_addon_bindings(
            "ProbeAddon",
            &benilla_ui::bindings_xml::parse(bindings).expect("well-formed"),
        );
        script
    }

    /// `SetBinding` signalled its `UPDATE_BINDINGS` in place (`0x4b805b`): the dispatch map still
    /// re-derives, and the frame fires no second signal for the same change.
    #[test]
    fn a_verbs_binding_change_is_signalled_once() {
        let script = core_script();
        let mut app = vm_harness(script);
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "UPDATES = 0 local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_BINDINGS') \
                 f:SetScript('OnEvent', function() UPDATES = UPDATES + 1 end) \
                 SetBinding('F', 'JUMP')",
            )
            .unwrap();
        assert_eq!(lua_count(&app, "UPDATES"), 1, "the verb's own signal");
        app.update();
        assert_eq!(
            lua_count(&app, "UPDATES"),
            1,
            "no second signal from the feed"
        );
        assert!(app
            .world()
            .resource::<BindingDispatch>()
            .map
            .values()
            .any(|c| c == "JUMP"));
    }

    /// Only the two reference sites signal `UPDATE_BINDINGS`: a set's copy into the live table
    /// (`0x4b649d`, here the host's load) and a stored `SetBinding`. Reading an addon's
    /// `Bindings.xml` (`0x4b6f70`) moves the dispatch table and signals nothing.
    #[test]
    fn registering_an_addons_bindings_signals_nothing_but_a_load_does() {
        let script = core_script();
        script
            .run(
                "UPDATES = 0 local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_BINDINGS') \
                 f:SetScript('OnEvent', function() UPDATES = UPDATES + 1 end)",
            )
            .unwrap();
        let mut app = vm_harness(script);
        app.update();
        assert_eq!(
            lua_count(&app, "UPDATES"),
            1,
            "the seed's load owes its signal"
        );
        app.update();
        assert_eq!(lua_count(&app, "UPDATES"), 1, "once");

        {
            let script = app.world().non_send_resource::<UiScript>();
            // The verb signals in place and owes nothing; only then does the registration run.
            script
                .run(r#"SetBinding("J", "PROBEHOLD") UPDATES = 0"#)
                .unwrap();
            script.register_addon_bindings(
                "ProbeAddon",
                &benilla_ui::bindings_xml::parse(PROBE_BINDINGS).expect("well-formed"),
            );
        }
        app.update();
        assert_eq!(
            lua_count(&app, "UPDATES"),
            0,
            "a registration signals nothing"
        );
        assert!(
            app.world()
                .resource::<BindingDispatch>()
                .map
                .values()
                .any(|c| c == "PROBEHOLD"),
            "but the dispatch map re-derives"
        );

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .load_binding_set(1);
        app.update();
        assert_eq!(lua_count(&app, "UPDATES"), 1, "a load does");
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

    /// The box declines PRINTSCREEN before it looks at focus (`0x77b17f`), so the walk goes on and
    /// the binding runs while typing, with every other key still eaten.
    #[test]
    fn printscreen_runs_its_binding_while_a_box_has_focus() {
        let script = with_addon(PROBE_BINDINGS);
        script
            .run(r#"SetBinding("PRINTSCREEN", "PROBEEDGE"); SetBinding("G", "PROBEHOLD")"#)
            .expect("bind");
        let mut app = vm_harness(script);
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;

        press_key(&mut app, KeyCode::KeyG);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_DOWN"), 0, "G is eaten by the box");

        press_key(&mut app, KeyCode::PrintScreen);
        app.update();
        assert_eq!(
            lua_count(&app, "PROBE_EDGE"),
            1,
            "PRINTSCREEN is declined, so its binding runs"
        );
    }

    /// A press that resolved to a binding is the world frame's (`0x483b70`): the key's up goes to
    /// it, not to the hidden frame that took the key's last press.
    #[test]
    fn a_press_that_runs_a_binding_makes_the_world_frame_its_consumer() {
        let script = with_addon(PROBE_BINDINGS);
        script
            .run(
                r#"
                SetBinding("G", "PROBEEDGE")
                CreateFrame("WorldFrame", "WorldFrame")
                ups = 0
                stale = CreateFrame("Frame", "StaleG")
                stale:EnableKeyboard(true)
                stale:SetScript("OnKeyDown", function() end)
                stale:SetScript("OnKeyUp", function() ups = ups + 1 end)
                "#,
            )
            .expect("bind");
        let mut app = vm_harness(script);
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            assert!(s.key_input("G"), "the frame takes G first");
            s.run("stale:Hide()").unwrap();
        }
        press_key(&mut app, KeyCode::KeyG);
        app.update();
        assert_eq!(
            lua_count(&app, "PROBE_EDGE"),
            1,
            "the binding took the press"
        );
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.run("stale:Show()").unwrap();
        assert!(s.key_up("G").taken());
        assert_eq!(s.eval::<i64>("return ups").unwrap(), 0);
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
        wheel(&mut app, 1);
        app.update();
        assert!(state(&app).fired(Input::Jump));
        assert!(
            !state(&app).fired(Input::CameraZoomIn),
            "JUMP stole the wheel from the camera, the 1.12 steal law"
        );
    }

    /// The whole path of a notch over the stock chrome: the UI's sweep first, then the bindings
    /// pass with what it left. Over a button with no `OnMouseWheel` the camera zooms (`0x76c1cb`,
    /// `0x483d20`); over a frame that has one, the frame scrolls and the camera does not.
    #[test]
    fn a_notch_zooms_the_camera_over_a_button_and_not_over_a_scroll_pane() {
        let mut s = core_script();
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
            p:SetScript("OnMouseWheel", function() SCROLLED = (SCROLLED or 0) + 1 end)
            "#,
        )
        .expect("frames");
        s.resolve();
        let mut to_world = |x, y| {
            i32::from(s.mouse_wheel_bound(x, y, 1.0, true) == benilla_ui::script::WheelWalk::World)
        };
        let over_button = to_world(120.0, 120.0);
        let over_pane = to_world(650.0, 150.0);
        let mut app = vm_harness(s);

        wheel(&mut app, over_pane);
        app.update();
        assert!(
            !state(&app).fired(Input::CameraZoomIn),
            "the pane took the notch"
        );
        assert_eq!(lua_count(&app, "SCROLLED"), 1);

        wheel(&mut app, over_button);
        app.update();
        assert!(
            state(&app).fired(Input::CameraZoomIn),
            "a button has no OnMouseWheel: the notch is the camera's"
        );
        assert_eq!(lua_count(&app, "SCROLLED"), 1);
    }

    /// Where the OS turned Shift plus the wheel sideways, a horizontal-only scroll with Shift held
    /// is the vertical notch it came from; nowhere else is a sideways scroll a notch.
    #[test]
    fn a_remapped_shift_wheel_is_the_vertical_notch() {
        assert_eq!(notch_travel_with(-1.0, 0.0, true, true), -1.0);
        assert_eq!(notch_travel_with(2.0, 0.0, true, true), 2.0);
        assert_eq!(
            notch_travel_with(2.0, 0.0, false, true),
            0.0,
            "no Shift: sideways"
        );
        assert_eq!(
            notch_travel_with(2.0, 0.0, true, false),
            0.0,
            "no remap: sideways"
        );
        assert_eq!(
            notch_travel_with(2.0, 3.0, true, true),
            3.0,
            "a vertical part wins"
        );
        assert_eq!(notch_travel_with(0.0, -1.0, true, true), -1.0);
    }

    /// The world slot takes a notch only when its chord has a binding (`0x483d87`); Super holds
    /// the notch back.
    #[test]
    fn the_world_takes_a_notch_only_for_a_bound_chord() {
        let mut app = harness();
        app.update();
        let mut mods = Mods::default();
        let dispatch = app.world().resource::<BindingDispatch>();
        assert!(world_takes_wheel(Some(dispatch), mods, true));
        assert!(world_takes_wheel(Some(dispatch), mods, false));
        // SHIFT-MOUSEWHEELUP has no binding; the unmodified chord's retry is the leftmost
        // modifier dropped, so it falls back to the bound plain chord.
        mods.shift = true;
        assert!(world_takes_wheel(Some(dispatch), mods, true));
        mods.sup = true;
        assert!(!world_takes_wheel(Some(dispatch), mods, true));
        assert!(world_takes_wheel(None, mods, true), "no table, no say");
        let empty = BindingDispatch::default();
        assert!(!world_takes_wheel(Some(&empty), Mods::default(), true));
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

        wheel(&mut app, 1);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_DOWN"), 1, "the notch's press half");
        assert_eq!(
            lua_count(&app, "PROBE_UP"),
            1,
            "…and its release half, in the same frame — a notch has no key left to lift"
        );
        assert_eq!(lua_str(&app, "PROBE_LAST"), "up");
        // Nothing is left latched: no key could end a wheel latch.
        assert!(app.world().resource::<BindingLatches>().latched.is_empty());
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

    /// The UI key feed beside the dispatch, as the world runs them: the feed first, on the same
    /// events, with the world frame in the walk.
    fn feed_and_dispatch_harness() -> App {
        fn feed(
            mut script: NonSendMut<UiScript>,
            mut capture: ResMut<UiKeyboardCapture>,
            mut events: MessageReader<KeyboardInput>,
            keys: Res<ButtonInput<KeyCode>>,
            layout: Res<LayoutNames>,
            mut clipboard: NonSendMut<crate::textinput::HostClipboard>,
            mut down: Local<KeyDownList>,
        ) {
            capture.consumed.clear();
            crate::ui_script::feed_keys(
                &mut script,
                &mut capture,
                events.read(),
                &keys,
                &mut down,
                false,
                crate::textinput::keymap::Mods {
                    shift: false,
                    ctrl: false,
                    alt: false,
                    sup: false,
                },
                &layout,
                &mut clipboard,
                None,
            );
        }
        let script = core_script();
        script
            .run(r#"CreateFrame("WorldFrame", "WorldFrame")"#)
            .expect("world frame");
        let mut app = vm_harness(script);
        app.insert_non_send_resource(crate::textinput::HostClipboard::holding(""))
            .add_systems(Update, feed.before(dispatch_records));
        app
    }

    /// A key held across an alt-tab resumes at its next OS repeat even when its binding made the
    /// world frame its consumer: the feed decides repeat from the same wiped pressed-key list
    /// (`0x424790`), not from the OS flag, so the repeat is a fresh press for both and W runs
    /// again (`0x4248b3`).
    #[test]
    fn a_held_movement_key_resumes_after_an_alt_tab_through_the_feed_too() {
        let mut app = feed_and_dispatch_harness();
        press_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        app.update();
        assert!(
            !state(&app).pressed(Input::MoveForward),
            "the deactivate releases the direction bits"
        );

        repeat_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(
            state(&app).pressed(Input::MoveForward),
            "the first repeat after re-activation re-latches"
        );
        assert!(
            !app.world()
                .resource::<UiKeyboardCapture>()
                .consumed
                .contains(&KeyCode::KeyW),
            "…the feed did not hand the repeat to the world frame's stub"
        );

        // A real repeat of the key now held is the stub's, and eaten by nobody's binding.
        repeat_key(&mut app, KeyCode::KeyW);
        app.update();
        assert!(state(&app).pressed(Input::MoveForward));
    }

    /// A press and its release in one frame's batch reach the dispatch in event order: the release
    /// follows the press that made the world frame the consumer, so it is not sent on to the frame
    /// that took the key's last press.
    #[test]
    fn a_press_and_a_release_in_one_batch_go_to_the_world_frame_not_a_stale_consumer() {
        let mut app = feed_and_dispatch_harness();
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.run(
                r#"
                ups = 0
                stale = CreateFrame("Frame", "StaleBatch")
                stale:EnableKeyboard(true)
                stale:SetScript("OnKeyDown", function() end)
                stale:SetScript("OnKeyUp", function() ups = ups + 1 end)
                "#,
            )
            .unwrap();
            assert!(s.key_input("W"), "the frame takes W once");
            s.run("stale:EnableKeyboard(false)").unwrap();
        }
        press_key(&mut app, KeyCode::KeyW);
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        let s = app.world().non_send_resource::<UiScript>();
        assert_eq!(s.eval::<i64>("return ups").unwrap(), 0);
    }

    /// PRINTSCREEN reaches Windows as an up alone (`VK_SNAPSHOT` sends no `WM_KEYDOWN`). With no
    /// consumer on record the client runs the key-down walk on the up (`0x76603c`-`0x76604d`), the
    /// world frame's binding last, so the binding runs on the release; a frame that takes the walk
    /// keeps it from the binding; and a platform that sends both halves runs it once.
    #[test]
    fn printscreen_as_an_up_alone_runs_its_binding_through_the_key_down_walk() {
        let script = with_addon(PROBE_BINDINGS);
        script
            .run(
                r#"SetBinding("PRINTSCREEN", "PROBEEDGE") CreateFrame("WorldFrame", "WorldFrame")"#,
            )
            .expect("bind");
        let mut app = vm_harness(script);
        release_key(&mut app, KeyCode::PrintScreen);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_EDGE"), 1, "the up alone runs it");

        // A frame that takes the key-down walk keeps it from the binding.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"
                got = 0
                cover = CreateFrame("Frame", "PrintCover")
                cover:EnableKeyboard(true)
                cover:SetScript("OnKeyDown", function() got = got + 1 end)
                "#,
            )
            .unwrap();
        release_key(&mut app, KeyCode::PrintScreen);
        app.update();
        assert_eq!(lua_count(&app, "got"), 1, "the frame took the walk");
        assert_eq!(
            lua_count(&app, "PROBE_EDGE"),
            1,
            "…so the binding did not run"
        );

        // A platform that sends both halves: the press ran it and made the world frame the
        // consumer, so the up is delivered, not walked again.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("cover:Hide()")
            .unwrap();
        press_key(&mut app, KeyCode::PrintScreen);
        release_key(&mut app, KeyCode::PrintScreen);
        app.update();
        assert_eq!(lua_count(&app, "PROBE_EDGE"), 2, "…once, not twice");
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
    /// zoom (1.0 when absent, `0x50b42d`), and `ToggleSheath` fires, one frame only.
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
        run(&mut app, "ToggleSheath()");
        assert!(state(&app).fired(Input::ToggleSheath));
        run(&mut app, "CameraZoomOut()");
        assert!(!state(&app).fired(Input::ToggleSheath));
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
        // The Super deviation: a Super-held press runs no binding, not even its bare key's.
        let mut app = harness();
        press_key(&mut app, KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(!state(&app).fired(Input::ToggleSheath));
    }

    /// What the layout names `k`, as `layout::record_layout_names` records it from the key's own
    /// OS message, ahead of the press.
    fn layout_key(app: &mut App, k: KeyCode, name: char) {
        app.world_mut()
            .resource_mut::<LayoutNames>()
            .set(k, Some(layout::LayoutName::Char(name)));
    }

    /// A press with the logical key the OS reports, modifiers applied.
    fn type_key(app: &mut App, k: KeyCode, logical: &str) {
        send_key(
            app,
            KeyboardInput {
                key_code: k,
                logical_key: Key::Character(logical.into()),
                state: bevy::input::ButtonState::Pressed,
                text: Some(logical.into()),
                repeat: false,
                window: Entity::PLACEHOLDER,
            },
        );
    }

    /// 1.12 binds a key by what its layout names it: on AZERTY the key where a US W sits is
    /// `VK_Z` and runs `Z`'s binding, and `W`'s moves to the key labelled W. Shift is a prefix,
    /// never part of the name.
    #[test]
    fn a_key_runs_the_binding_of_what_its_layout_names_it() {
        let script = core_script();
        script
            .run(
                r#"SetBinding("SHIFT-Z", "TOGGLEUI"); SetBinding("SHIFT-1", "TARGETPREVIOUSENEMY");
                SetBinding("SHIFT-;", "TARGETNEARESTENEMY")"#,
            )
            .expect("bind");
        let mut app = vm_harness(script);
        layout_key(&mut app, KeyCode::KeyW, 'Z');
        layout_key(&mut app, KeyCode::KeyZ, 'W');
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
        release_key(&mut app, KeyCode::Digit1);
        // And Shift+; makes `:`, and is `SHIFT-;`.
        type_key(&mut app, KeyCode::Semicolon, ":");
        app.update();
        assert_eq!(lua_count(&app, "TABS"), 1, "SHIFT-;, never SHIFT-:");
    }

    /// A key's name follows a layout switch while running: its next press is named under the
    /// layout it was typed in.
    #[test]
    fn a_layout_switch_renames_a_key_at_its_next_press() {
        let mut app = harness();
        layout_key(&mut app, KeyCode::KeyW, 'Z');
        type_key(&mut app, KeyCode::KeyW, "z");
        app.update();
        assert!(state(&app).fired(Input::ToggleSheath));
        release_key(&mut app, KeyCode::KeyW);
        app.update();
        // Back on a US layout, the key's next message reports `w`.
        layout_key(&mut app, KeyCode::KeyW, 'W');
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
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            !state(&app).fired(Input::ToggleSheath),
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
        release_key(&mut app, KeyCode::KeyZ);
        app.world_mut().resource_mut::<UiKeyboardCapture>().typing = false;
        app.update();
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(state(&app).fired(Input::ToggleSheath));
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
        frame_ate(&mut app, &[KeyCode::KeyZ]);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            !state(&app).fired(Input::ToggleSheath),
            "the frame ate this key: its binding must not also fire"
        );

        // ...and only that key.
        release_key(&mut app, KeyCode::KeyZ);
        app.update();
        frame_ate(&mut app, &[KeyCode::KeyM]);
        press_key(&mut app, KeyCode::KeyZ);
        app.update();
        assert!(
            state(&app).fired(Input::ToggleSheath),
            "consumption is per key, not per frame"
        );
        assert!(
            state(&app).pressed(Input::MoveForward),
            "and W has been held throughout"
        );
    }

    #[test]
    fn mouse_buttons_bind_only_over_the_world() {
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
    }

    /// The notches the sweep left to the world each run CAMERAZOOMIN's `CameraZoomIn(1.0)`, and
    /// none left run nothing.
    #[test]
    fn each_notch_left_to_the_world_zooms() {
        let mut app = harness();
        wheel(&mut app, 2);
        app.update();
        assert!(state(&app).fired(Input::CameraZoomIn));
        assert_eq!(state(&app).amount(Input::CameraZoomIn), 2.0, "two notches");
        wheel(&mut app, -1);
        app.update();
        assert_eq!(state(&app).amount(Input::CameraZoomOut), 1.0);
        wheel(&mut app, 0);
        app.update();
        assert!(!state(&app).fired(Input::CameraZoomIn), "no notch, no zoom");
        assert!(!state(&app).fired(Input::CameraZoomOut));
    }

    /// A trackpad's fractional lines carry to whole notches, each one `CameraZoomIn(1.0)`, and a
    /// reversal drops the carried fraction.
    #[test]
    fn fractional_wheel_lines_carry_to_whole_notches() {
        let mut notches = WheelNotches::default();
        let got: Vec<i32> = [0.4, 0.4, 0.4].map(|l| notches.feed(l)).to_vec();
        assert_eq!(
            got,
            [0, 0, 1],
            "0.4 lines a frame: the third completes the notch"
        );
        assert_eq!(
            notches.feed(-1.0),
            -1,
            "a reversal drops the carried fraction"
        );
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
                name: None,
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

    /// The binding harness over the files of a state folder of its own, with the account-data
    /// state the save and the reread talk to, every section settled.
    fn file_harness(home: &std::path::Path) -> (App, std::path::PathBuf, std::path::PathBuf) {
        let account = home.join("bindings/account.txt");
        let character = home.join("bindings/Realm-Probesix.txt");
        let mut app = App::new();
        app.insert_resource(BindingFiles {
            account: Some(account.clone()),
            character: Some(character.clone()),
            account_cache: Some(home.join("bindings/account.wtf")),
            character_cache: Some(home.join("bindings/Realm-Probesix.wtf")),
            held: Default::default(),
        })
        .init_resource::<crate::account_data::AccountData>()
        .insert_non_send_resource(core_script())
        .add_systems(Update, (drain_binding_requests, reread_arrived_bindings));
        app.world()
            .resource::<crate::account_data::AccountData>()
            .settle_for_test(Some(("Realm".into(), "Probesix".into())));
        (app, account, character)
    }

    fn eval_str(app: &App, chunk: &str) -> String {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<String>(chunk)
            .expect("eval")
    }

    /// A binding section that settles again is read again, its own set alone: section 2 reads set
    /// 1 and leaves the live table, section 3 reads set 2 and runs the set choice (`0x4b629a` →
    /// `0x4b64b0`), the character's set in use when it has keys.
    #[test]
    fn a_binding_section_from_the_server_is_read_again() {
        let tmp = std::env::temp_dir().join(format!("benilla-bind-reread-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let (mut app, account, character) = file_harness(&tmp);
        crate::local_state::write_atomic(&account, "bind JUMP G\n").unwrap();
        app.update();
        let jump = r#"return GetBindingKey("JUMP") or """#;
        assert_ne!(eval_str(&app, jump), "G", "nothing came down: no read");

        let data = || app.world().resource::<crate::account_data::AccountData>();
        data().owe_reader_for_test(2);
        app.update();
        assert_ne!(
            eval_str(&app, jump),
            "G",
            "set 1 read; the live table not chosen"
        );
        app.world()
            .non_send_resource::<UiScript>()
            .run("LoadBindings(1)")
            .unwrap();
        assert_eq!(eval_str(&app, jump), "G", "the account set read again");

        crate::local_state::write_atomic(&character, "bind JUMP H\n").unwrap();
        app.world()
            .resource::<crate::account_data::AccountData>()
            .owe_reader_for_test(3);
        app.update();
        assert_eq!(
            eval_str(&app, jump),
            "H",
            "the character's set is the one in use"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The reader registers for each section through `0x5afe50` (`0x4b62ef`, `0x4b6300`): a
    /// pending section's set is not read, nor saved over; section 2's settling reads set 1 alone,
    /// and section 3's reads set 2 and chooses the set in use (`0x4b629a` → `0x4b64b0`).
    #[test]
    fn the_binding_reader_waits_for_its_sections() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-bind-held-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _h = crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let account = crate::local_state::bindings_account_path().unwrap();
        crate::local_state::write_atomic(&account, "bind JUMP G\n").unwrap();
        let mut app = App::new();
        app.init_resource::<BindingFiles>()
            .init_resource::<crate::account_data::AccountData>()
            .add_systems(Update, reread_arrived_bindings);
        let mut script = core_script();
        seed_bindings_for_vm(app.world_mut(), &mut script);
        app.insert_non_send_resource(script);
        let jump = r#"return GetBindingKey("JUMP") or """#;
        assert_ne!(eval_str(&app, jump), "G", "pending: the sets are held");
        app.add_systems(Update, drain_binding_requests);
        app.world()
            .non_send_resource::<UiScript>()
            .run("SaveBindings(1)")
            .unwrap();
        app.update();
        assert_eq!(
            std::fs::read_to_string(&account).unwrap(),
            "bind JUMP G\n",
            "a held set is not saved over its file (`0x5afee0`)"
        );
        let settle = |app: &App, section| {
            app.world()
                .resource::<crate::account_data::AccountData>()
                .settle_section_for_test(section)
        };
        settle(&app, 2);
        app.update();
        assert_ne!(
            eval_str(&app, jump),
            "G",
            "set 1 read; no set chosen while section 3 is pending"
        );
        settle(&app, 3);
        app.update();
        assert_eq!(eval_str(&app, jump), "G", "set 2 read, set 1 chosen");
        drop(app);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// One section settled and its pair held: a save writes the settled section's files and skips
    /// the held one's (`0x4b6600`, `0x4b6611` → `0x5afee0`).
    #[test]
    fn a_save_skips_only_the_held_sections_write() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-bind-mixed-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _h = crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let account = crate::local_state::bindings_account_path().unwrap();
        crate::local_state::write_atomic(&account, "bind JUMP G\n").unwrap();
        let theirs =
            crate::local_state::bindings_cache_character_path("Realm", "Probeone").unwrap();
        crate::local_state::write_atomic(&theirs, "bind H JUMP\r\n").unwrap();
        let mut app = App::new();
        app.insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
            vec![crate::glue_session::roster::test_character(7, "Probeone")],
            7,
        ))
        .init_resource::<BindingFiles>()
        .init_resource::<crate::account_data::AccountData>()
        .add_systems(
            Update,
            (reread_arrived_bindings, drain_binding_requests).chain(),
        );
        app.world()
            .resource::<crate::account_data::AccountData>()
            .settle_section_for_test(2);
        let mut script = core_script();
        seed_bindings_for_vm(app.world_mut(), &mut script);
        app.insert_non_send_resource(script);
        app.world()
            .non_send_resource::<UiScript>()
            .run(r#"SetBinding("K", "JUMP") SaveBindings(1)"#)
            .unwrap();
        app.update();
        let diff = std::fs::read_to_string(&account).unwrap();
        assert!(
            diff.contains("bind JUMP SPACE K"),
            "section 2 settled: written: {diff}"
        );
        assert_eq!(
            std::fs::read_to_string(&theirs).unwrap(),
            "bind H JUMP\r\n",
            "section 3 held: its file is not emptied"
        );
        drop(app);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// `SaveBindings(1)` marks both sections and `SaveBindings(2)` the character's (`0x4b65ed`,
    /// `0x4b6617`) for the interface's shutdown, the reference's one write of them (`0x490c64`);
    /// the save itself arms no upload.
    #[test]
    fn saving_a_set_marks_its_sections_for_the_shutdown() {
        let tmp = std::env::temp_dir().join(format!("benilla-bind-save-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let (mut app, _, _) = file_harness(&tmp);
        let armed = |app: &App, section: usize| {
            let data = app.world().resource::<crate::account_data::AccountData>();
            assert!(!data.armed_for_test(section), "a save arms nothing");
            data.saved_for_test(section)
        };
        app.world()
            .non_send_resource::<UiScript>()
            .run("SaveBindings(2)")
            .unwrap();
        app.update();
        assert!(
            !armed(&app, 2) && armed(&app, 3),
            "the character's set alone"
        );
        app.world()
            .non_send_resource::<UiScript>()
            .run("SaveBindings(1)")
            .unwrap();
        app.update();
        assert!(
            armed(&app, 2) && armed(&app, 3),
            "the account set writes both"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A save writes the set's section file as the reference does (`0x4b65b0`): the account's
    /// whole table, and the character's file emptied while the account set is in use
    /// (`0x4b65c6`), or holding set 2 when it is.
    #[test]
    fn a_save_writes_the_sections_files_as_the_reference_does() {
        let tmp = std::env::temp_dir().join(format!("benilla-bind-wtf-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let (mut app, _, character) = file_harness(&tmp);
        let run = |app: &mut App, chunk: &str| {
            app.world()
                .non_send_resource::<UiScript>()
                .run(chunk)
                .unwrap();
            app.update();
        };
        let read = |name: &str| std::fs::read_to_string(tmp.join("bindings").join(name)).ok();
        run(&mut app, r#"SetBinding("G", "JUMP") SaveBindings(2)"#);
        let set2 = read("Realm-Probesix.wtf").expect("the character's file");
        assert!(set2.contains("bind G JUMP\r\n"), "{set2}");
        run(&mut app, "SaveBindings(1)");
        let set1 = read("account.wtf").expect("the account's file");
        assert!(set1.contains("bind G JUMP\r\n"), "{set1}");
        assert_eq!(
            read("Realm-Probesix.wtf").as_deref(),
            Some(""),
            "emptied, not removed: its digest is MD5 of nothing"
        );
        assert!(
            !character.exists(),
            "the character's diff goes: set 1 is in use"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A save writes the set `SaveBindings` stored, copied from the live table inside the call
    /// (`0x4b8260` -> `0x4b6310`): a binding made after it in the same frame is in neither file.
    #[test]
    fn a_save_writes_the_set_as_it_was_saved() {
        let tmp = std::env::temp_dir().join(format!("benilla-bind-saved-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let (mut app, account, _) = file_harness(&tmp);
        app.world()
            .non_send_resource::<UiScript>()
            .run(r#"SetBinding("F6", "JUMP") SaveBindings(1) SetBinding("F7", "SITORSTAND")"#)
            .unwrap();
        app.update();
        let wtf = std::fs::read_to_string(tmp.join("bindings/account.wtf")).expect("the .wtf");
        let diff = std::fs::read_to_string(&account).expect("the diff");
        assert!(wtf.contains("bind F6 JUMP\r\n"), "the saved binding: {wtf}");
        assert!(!wtf.contains("bind F7 "), "no later binding: {wtf}");
        assert!(
            diff.lines()
                .any(|l| l.starts_with("bind JUMP ") && l.ends_with(" F6")),
            "the saved binding: {diff}"
        );
        assert!(!diff.contains("SITORSTAND"), "no later binding: {diff}");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
