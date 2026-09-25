//! The actuation probes: chat lines, key taps, a Lua chunk, a pointer drag and a hover sweep
//! driven into a live session. Each is env-gated, waits for the world and runs on [`ProbeClock`].

use bevy::prelude::*;

use super::ProbeClock;

/// `WOW_PROBE_CHAT="<line>[;<line>…]"`: types each line into the chat box once in-world, from
/// `WOW_PROBE_CHAT_AT` seconds (default 8), `WOW_PROBE_CHAT_EVERY` seconds apart (default one
/// burst). Spacing matters: two field flips inside one drain merge to a no-op.
pub(crate) struct ProbeChatPlugin;

impl Plugin for ProbeChatPlugin {
    fn build(&self, app: &mut App) {
        let lines = std::env::var("WOW_PROBE_CHAT").unwrap_or_default();
        let at = std::env::var("WOW_PROBE_CHAT_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8.0);
        let every = std::env::var("WOW_PROBE_CHAT_EVERY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        app.insert_resource(ProbeChat {
            lines,
            at,
            every,
            sent: 0,
        })
        .add_systems(Update, fire_probe_chat);
    }
}

/// [`ProbeChatPlugin`] state; `every` of 0 sends every line in one burst.
#[derive(Resource)]
struct ProbeChat {
    lines: String,
    at: f32,
    every: f32,
    sent: usize,
}

/// Submits due lines once a self player exists (the server drops a `.go` sent before world entry),
/// through the chat box so client-side slash commands parse as typed ones do.
fn fire_probe_chat(
    mut probe: ResMut<ProbeChat>,
    time: ProbeClock,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
) {
    if probe.lines.is_empty() {
        return;
    }
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let Some(mut script) = script else {
        return;
    };
    // Line N is due at `at + N·every`.
    loop {
        let Some(line) = probe
            .lines
            .split(';')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .nth(probe.sent)
        else {
            return; // all sent
        };
        let due = probe.at + probe.every * probe.sent as f32;
        if time.elapsed_secs() < due {
            return;
        }
        info!("probe-chat: sending {line:?}");
        script.push_chat_input(line.to_string());
        probe.sent += 1;
    }
}

/// `WOW_PROBE_KEY="<key>@<secs>[:<hold>][;…]"`: presses each key at its time once in-world and
/// releases it `<hold>` seconds later ([`PROBE_KEY_TAP_SECS`] when omitted). Reaches input no chat
/// line or Lua chunk can (1.12 has no jump Lua API). Runs in `PreUpdate` after
/// [`bevy::input::InputSystems`], so every `Update` reader sees the `just_pressed` that frame.
pub(crate) struct ProbeKeyPlugin;

impl Plugin for ProbeKeyPlugin {
    fn build(&self, app: &mut App) {
        let spec = std::env::var("WOW_PROBE_KEY").unwrap_or_default();
        let taps = spec
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let (key, rest) = s.split_once('@')?;
                let (at, hold) = match rest.split_once(':') {
                    Some((at, hold)) => (at, hold.trim().parse::<f32>().ok()?),
                    None => (rest, PROBE_KEY_TAP_SECS),
                };
                match (probe_key_by_name(key.trim()), at.trim().parse::<f32>()) {
                    (Some(key), Ok(at)) => Some(ProbeKeyTap {
                        key,
                        at,
                        hold,
                        pressed: false,
                        released: false,
                    }),
                    _ => {
                        warn!("probe-key: unparseable tap {s:?} (want e.g. Space@14 or W@20:6) — skipped");
                        None
                    }
                }
            })
            .collect();
        app.insert_resource(ProbeKeys { taps, armed: false })
            .add_systems(
                bevy::app::PreUpdate,
                fire_probe_key
                    .after(bevy::input::InputSystems)
                    // After the loading cover empties every button plane, so the press survives.
                    .after(crate::loading_screen::CoverInput),
            );
    }
}

/// Default hold: several frames for a held-key reader, still a tap.
const PROBE_KEY_TAP_SECS: f32 = 0.25;

/// The key names [`ProbeKeyPlugin`] accepts.
fn probe_key_by_name(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Space" => KeyCode::Space,
        "W" => KeyCode::KeyW,
        "A" => KeyCode::KeyA,
        "S" => KeyCode::KeyS,
        "D" => KeyCode::KeyD,
        "Q" => KeyCode::KeyQ,
        "E" => KeyCode::KeyE,
        "X" => KeyCode::KeyX,
        "Z" => KeyCode::KeyZ,
        "Tab" => KeyCode::Tab,
        // TOGGLEAUTORUN's 1.12 default; with `WOW_PROBE_LOOK` it scripts a drive.
        "NumLock" => KeyCode::NumLock,
        // The nameplate toggle: bare `V` shows enemy plates, `Shift`+`V` friendly ones.
        "V" => KeyCode::KeyV,
        // Free-fly is the dev chord (`Ctrl`+`Shift`) + `F` (`player.rs`); a held `Ctrl` is its ×5
        // speed boost (`camera.rs`).
        "F" => KeyCode::KeyF,
        "Ctrl" => KeyCode::ControlLeft,
        "Shift" => KeyCode::ShiftLeft,
        // The cost pill's toggle, dev chord + `P` (`perf::hud::toggle_hud`); it starts hidden.
        "P" => KeyCode::KeyP,
        // Text editing: `EditBox` has no Lua deletion API, so caret and deletion need real keys.
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Left" => KeyCode::ArrowLeft,
        "Right" => KeyCode::ArrowRight,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        // Enter at character select enters the world (`char_select::input`): `/logout` then
        // `Enter` is a relog in one process, since `WOW_CHAR` applies once (`run_mode`).
        "Enter" => KeyCode::Enter,
        // The SCREENSHOT binding's key; a Lua `TakeScreenshot()` skips the binding. On macOS it
        // arrives as F13 (`bindings::chord`'s fold) under the same token.
        "PrintScreen" => KeyCode::PrintScreen,
        _ => return None,
    })
}

#[derive(Resource)]
struct ProbeKeys {
    taps: Vec<ProbeKeyTap>,
    /// A self player has existed this run: latched, so taps reach glue screens after a `/logout`.
    armed: bool,
}

struct ProbeKeyTap {
    key: KeyCode,
    at: f32,
    /// Seconds held: the spec's `:<hold>`, else [`PROBE_KEY_TAP_SECS`].
    hold: f32,
    pressed: bool,
    released: bool,
}

/// Presses and releases due taps through both `ButtonInput` (held-state readers) and a raw
/// [`KeyboardInput`] message (the binding dispatch's press/release edges); both are needed.
fn fire_probe_key(
    mut probe: ResMut<ProbeKeys>,
    time: ProbeClock,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut events: MessageWriter<bevy::input::keyboard::KeyboardInput>,
    mut hold: ResMut<benilla_world::modkeys::SyntheticHold>,
) {
    probe.armed |= !self_player.is_empty();
    if probe.taps.is_empty() || !probe.armed {
        return;
    }
    let now = time.elapsed_secs();
    let mut synth = |key: KeyCode, state: bevy::input::ButtonState| {
        events.write(bevy::input::keyboard::KeyboardInput {
            key_code: key,
            logical_key: bevy::input::keyboard::Key::Unidentified(
                bevy::input::keyboard::NativeKey::Unidentified,
            ),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    };
    for tap in &mut probe.taps {
        if !tap.pressed && now >= tap.at {
            info!(
                "probe-key: {:?} down ({now:.1}s, hold {:.2}s)",
                tap.key, tap.hold
            );
            keys.press(tap.key);
            synth(tap.key, bevy::input::ButtonState::Pressed);
            tap.pressed = true;
        } else if tap.pressed && !tap.released && now >= tap.at + tap.hold {
            keys.release(tap.key);
            synth(tap.key, bevy::input::ButtonState::Released);
            tap.released = true;
        }
    }
    // Publish the held keys so the macOS stuck-modifier reconciler ([`benilla_world::modkeys`]),
    // which polls hardware flags, does not release them. Written only on change, to keep the
    // resource's change tick meaningful.
    let held: Vec<KeyCode> = probe
        .taps
        .iter()
        .filter(|t| t.pressed && !t.released)
        .map(|t| t.key)
        .collect();
    if hold.0 != held {
        hold.0 = held;
    }
}

/// `WOW_PROBE_LUA="<chunk>"`: runs the chunk in the live UI VM once per world entry, first at
/// `WOW_PROBE_LUA_AT` seconds (default 10), then `WOW_PROBE_LUA_AGAIN` seconds (default 4) into
/// each later entry. Re-armed on world entry, not VM rebuild, or `ReloadUI()` would loop; the
/// [`ProbeLog`](install_probe_log) channel does follow the VM.
pub(crate) struct ProbeLuaPlugin;

impl Plugin for ProbeLuaPlugin {
    fn build(&self, app: &mut App) {
        let chunk = std::env::var("WOW_PROBE_LUA").unwrap_or_default();
        let at = std::env::var("WOW_PROBE_LUA_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10.0);
        let again = std::env::var("WOW_PROBE_LUA_AGAIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4.0);
        app.insert_resource(ProbeLua {
            chunk,
            at,
            again,
            ..default()
        })
        .add_systems(Update, fire_probe_lua);
    }
}

#[derive(Resource, Default)]
struct ProbeLua {
    chunk: String,
    at: f32,
    again: f32,
    /// Ready world entries, counted on the rising edge.
    entries: u32,
    /// Entries already fired into; `fired < entries` means one is owed.
    fired: u32,
    ready_at: f32,
    was_ready: bool,
    /// The VM session `ProbeLog` is installed in; `0` matches none, since
    /// [`benilla_ui::script::UiScript::session`] counts from 1.
    logged_vm: u64,
}

/// Whether this entry's chunk is due. The first fire is run-relative (`WOW_PROBE_LUA_AT`, which
/// `scripts/summon-live.sh` relies on); a later entry settles from when it became ready.
fn probe_lua_due(probe: &ProbeLua, now: f32) -> bool {
    if probe.fired >= probe.entries {
        return false; // fired already, or no entry yet
    }
    if probe.fired == 0 {
        now >= probe.at
    } else {
        now >= probe.ready_at + probe.again
    }
}

/// Installs `ProbeLog(text)`, which logs `probe-log:` lines, into the live VM. Relog and
/// `ReloadUI()` build a new Lua state, so it is reinstalled per VM; only while a chunk is set.
fn install_probe_log(script: &benilla_ui::script::UiScript) {
    match script.lua().create_function(|_, text: String| {
        info!("probe-log: {text}");
        Ok(())
    }) {
        Ok(f) => {
            if let Err(e) = script.lua().globals().set("ProbeLog", f) {
                error!("probe-lua: installing ProbeLog: {e}");
            }
        }
        Err(e) => error!("probe-lua: creating ProbeLog: {e}"),
    }
}

/// Runs the chunk once per world entry, on [`probe_lua_due`]'s schedule.
fn fire_probe_lua(
    mut probe: ResMut<ProbeLua>,
    time: ProbeClock,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
    state: Res<State<crate::char_select::ClientState>>,
    entry_ui_pending: Option<Res<crate::ui_script::PendingEntryUiLoad>>,
) {
    if probe.chunk.is_empty() {
        return;
    }
    let Some(script) = script else {
        // Between the world-entry edge and the entry load the boot VM is parked
        // (`ui_script::ParkedBootVm`): not an entry yet.
        probe.was_ready = false;
        return;
    };
    // Ready means the in-game VM, not the boot one: `InWorld` (the entered-world message and the
    // first object update can share a drain a frame before the VM is parked), the entry UI load
    // done, and a self player for `UnitName("player")`.
    let in_world = *state.get() == crate::char_select::ClientState::InWorld;
    let ready = in_world && entry_ui_pending.is_none() && !self_player.is_empty();
    let now = time.elapsed_secs();
    if ready && !probe.was_ready {
        probe.entries += 1;
        probe.ready_at = now;
        info!("probe-lua: world entry {} at {now:.1}s", probe.entries);
    }
    probe.was_ready = ready;
    if !ready {
        return;
    }
    if probe.logged_vm != script.session() {
        install_probe_log(&script);
        probe.logged_vm = script.session();
    }
    if !probe_lua_due(&probe, now) {
        return;
    }
    probe.fired = probe.entries;
    info!(
        "probe-lua: entry {} at {now:.1}s — running {:?}",
        probe.entries, probe.chunk
    );
    if let Err(e) = script.run(&probe.chunk) {
        error!("probe-lua: {e}");
    }
}

/// `WOW_PROBE_HOVER="Frame1;Frame2;…"`: moves the pointer across each named frame's centre
/// through the real pointer path, pressing nothing, from `WOW_PROBE_HOVER_AT` (default 14 s),
/// one step per `WOW_PROBE_HOVER_STEP` (default 0.25 s; ~0.016 moves every frame), until exit.
/// `WOW_PROBE_HOVER_DUTY=<on>:<off>` alternates sweeping and parking so run drift cancels.
pub(crate) struct ProbeHoverPlugin;

impl Plugin for ProbeHoverPlugin {
    fn build(&self, app: &mut App) {
        let names: Vec<String> = std::env::var("WOW_PROBE_HOVER")
            .unwrap_or_default()
            .split(';')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let at = std::env::var("WOW_PROBE_HOVER_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14.0);
        let step = std::env::var("WOW_PROBE_HOVER_STEP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.25);
        let jitter = std::env::var("WOW_PROBE_HOVER_JITTER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let duty = std::env::var("WOW_PROBE_HOVER_DUTY")
            .ok()
            .and_then(|v| {
                v.split_once(':')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
            })
            .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)));
        app.insert_resource(ProbeHover {
            names,
            at,
            step,
            jitter,
            duty,
            i: 0,
            next: 0.0,
            announced: false,
            parked: false,
        })
        .add_systems(Update, fire_probe_hover);
    }
}

/// [`ProbeHoverPlugin`]'s state: which name is next, and when.
#[derive(Resource)]
struct ProbeHover {
    names: Vec<String>,
    at: f32,
    step: f32,
    /// `WOW_PROBE_HOVER_JITTER=<px>`: offset per step; at 0 a one-name sweep never moves.
    jitter: f32,
    /// `WOW_PROBE_HOVER_DUTY=<on>:<off>`: seconds sweeping, then seconds parked.
    duty: Option<(f32, f32)>,
    i: usize,
    next: f32,
    announced: bool,
    parked: bool,
}

/// The parked pointer: the top-left corner, over no frame.
const HOVER_PARK_AT: (f32, f32) = (2.0, 2.0);

/// Moves the pointer onto the next named frame's centre, looping forever.
fn fire_probe_hover(
    mut probe: ResMut<ProbeHover>,
    mut synthetic: ResMut<crate::ui_script::SyntheticPointer>,
    time: ProbeClock,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
) {
    if probe.names.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    if now < probe.at || self_player.is_empty() || now < probe.next {
        return;
    }
    let Some(mut script) = script else { return };
    synthetic.0 = true;
    // Which half of the duty period this is, and whether it just flipped.
    if let Some((on, off)) = probe.duty {
        let period = (on + off).max(1e-3);
        let parked = (now - probe.at).rem_euclid(period) >= on;
        let flipped = parked != probe.parked;
        probe.parked = parked;
        if parked {
            probe.next = now + probe.step;
            if flipped {
                script.mouse_move(HOVER_PARK_AT.0, HOVER_PARK_AT.1);
            }
            return;
        }
    }
    probe.next = now + probe.step;
    let name = probe.names[probe.i % probe.names.len()].clone();
    probe.i += 1;
    match frame_centre(&script, &name) {
        Some((x, y)) => {
            if !probe.announced {
                probe.announced = true;
                info!(
                    "probe-hover: sweeping {} frame(s) every {:.2}s, first {name} ({x:.0},{y:.0})",
                    probe.names.len(),
                    probe.step
                );
            }
            let (dx, dy) = jitter_offset(probe.jitter, probe.i);
            script.mouse_move(x + dx, y + dy);
        }
        // A sweep over missing frames would read as free hovering, so say so.
        None => warn!("probe-hover: {name} has no resolved rect — nothing hovered this step"),
    }
}

/// A deterministic offset inside a `j`-pixel square: the pointer moves every step while the
/// hovered frame stays put, isolating hit-test cost from tooltip rebuilds.
fn jitter_offset(j: f32, i: usize) -> (f32, f32) {
    if j <= 0.0 {
        return (0.0, 0.0);
    }
    // A 4-phase square walk: (+,+), (-,+), (-,-), (+,-).
    let (sx, sy) = match i % 4 {
        0 => (1.0, 1.0),
        1 => (-1.0, 1.0),
        2 => (-1.0, -1.0),
        _ => (1.0, -1.0),
    };
    (sx * j, sy * j)
}

pub(crate) struct ProbeDragPlugin;

impl Plugin for ProbeDragPlugin {
    fn build(&self, app: &mut App) {
        let pairs = std::env::var("WOW_PROBE_DRAG")
            .unwrap_or_default()
            .split(';')
            .filter_map(|p| p.split_once('>'))
            .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
            .collect();
        let at = std::env::var("WOW_PROBE_DRAG_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14.0);
        let step = std::env::var("WOW_PROBE_DRAG_STEP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.1);
        app.insert_resource(ProbeDrag {
            pairs,
            at,
            step,
            report: std::env::var("WOW_PROBE_DRAG_LUA").unwrap_or_default(),
            pair: 0,
            phase: 0,
            next: 0.0,
            from: (0.0, 0.0),
            to: (0.0, 0.0),
        })
        .add_systems(Update, fire_probe_drag);
    }
}

/// [`ProbeDragPlugin`]'s state: `WOW_PROBE_DRAG="A>B[;C>D…]"` drags frame A onto B through the
/// real pointer path from `WOW_PROBE_DRAG_AT` (default 14 s), one step per `WOW_PROBE_DRAG_STEP`
/// (default 0.1 s); `WOW_PROBE_DRAG_LUA` is a chunk whose returned string is logged after each.
#[derive(Resource)]
struct ProbeDrag {
    pairs: Vec<(String, String)>,
    at: f32,
    step: f32,
    report: String,
    pair: usize,
    /// The step within the current gesture, per [`fire_probe_drag`].
    phase: usize,
    next: f32,
    from: (f32, f32),
    to: (f32, f32),
}

/// A frame's centre from its live `GetLeft`/`GetRight`/`GetTop`/`GetBottom`, in the space
/// [`crate::ui_script::input`] feeds the cursor in.
fn frame_centre(script: &benilla_ui::script::UiScript, name: &str) -> Option<(f32, f32)> {
    let read = |edge: &str| {
        script
            .eval::<f32>(&format!(
                "local f = getglobal(\"{name}\") return f and f:Get{edge}()"
            ))
            .ok()
    };
    Some((
        (read("Left")? + read("Right")?) / 2.0,
        (read("Top")? + read("Bottom")?) / 2.0,
    ))
}

/// Advances the drag at most one step per `probe.step` seconds, each on its own frame: a press
/// and release in one frame is a click.
fn fire_probe_drag(
    mut probe: ResMut<ProbeDrag>,
    mut synthetic: ResMut<crate::ui_script::SyntheticPointer>,
    time: ProbeClock,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
) {
    if probe.pairs.is_empty() || probe.pair >= probe.pairs.len() {
        if synthetic.0 {
            synthetic.0 = false;
        }
        return;
    }
    let now = time.elapsed_secs();
    if now < probe.at || self_player.is_empty() {
        return;
    }
    let Some(mut script) = script else { return };
    if now < probe.next {
        return;
    }
    probe.next = now + probe.step;
    // The probe owns the pointer from the first step of a gesture to the last, never longer.
    synthetic.0 = true;

    let (from_name, to_name) = probe.pairs[probe.pair].clone();
    match probe.phase {
        0 => {
            let (Some(from), Some(to)) = (
                frame_centre(&script, &from_name),
                frame_centre(&script, &to_name),
            ) else {
                warn!("probe-drag: {from_name} > {to_name} — no resolved rect, skipping");
                probe.pair += 1;
                return;
            };
            probe.from = from;
            probe.to = to;
            info!(
                "probe-drag: {from_name} ({:.0},{:.0}) > {to_name} ({:.0},{:.0})",
                from.0, from.1, to.0, to.1
            );
            script.mouse_move(from.0, from.1);
        }
        1 => {
            let (x, y) = probe.from;
            script.mouse_button(x, y, "LeftButton", true);
        }
        // Past the 4-px drag threshold: fires `OnDragStart`.
        2 => {
            let (x, y) = probe.from;
            script.mouse_move(x + 8.0, y + 8.0);
        }
        // Two waypoints, so the drag spends frames in flight.
        3 | 4 => {
            let t = if probe.phase == 3 { 0.4 } else { 0.8 };
            let (fx, fy) = probe.from;
            let (tx, ty) = probe.to;
            script.mouse_move(fx + (tx - fx) * t, fy + (ty - fy) * t);
        }
        5 => {
            let (x, y) = probe.to;
            script.mouse_move(x, y);
        }
        6 => {
            let (x, y) = probe.to;
            script.mouse_button(x, y, "LeftButton", false);
        }
        // Self-check: hover the source where it now draws and ask the hit test who is there, to
        // catch a hit rect that has parted from the drawn one.
        7 => {
            if let Some((x, y)) = frame_centre(&script, &from_name) {
                script.mouse_move(x, y);
                let focus = script
                    .eval::<String>(
                        "return tostring(GetMouseFocus() and GetMouseFocus():GetName())",
                    )
                    .unwrap_or_else(|_| "<eval failed>".into());
                if focus != from_name && !focus.starts_with(&from_name) {
                    warn!(
                        "probe-drag: {from_name} draws at ({x:.0},{y:.0}) but the hit test there \
                         says {focus}"
                    );
                } else {
                    info!("probe-drag: {from_name} still answers its own rect");
                }
            }
        }
        _ => {
            if !probe.report.is_empty() {
                match script.eval::<String>(&probe.report) {
                    Ok(text) => info!("probe-drag: after {from_name} > {to_name}: {text}"),
                    Err(e) => warn!("probe-drag: report chunk raised: {e}"),
                }
            }
            for err in script.errors().drain(..) {
                warn!("probe-drag: UI error during {from_name} > {to_name}: {err}");
            }
            probe.pair += 1;
            probe.phase = 0;
            // Hand the pointer back for a frame between gestures, where a stale hover shows.
            synthetic.0 = false;
            return;
        }
    }
    probe.phase += 1;
}

#[cfg(test)]
mod tests {
    use super::{probe_lua_due, ProbeLua};

    /// A probe as `WOW_PROBE_LUA_AT=10` configures one, with its entry bookkeeping set by hand.
    fn probe(entries: u32, fired: u32, ready_at: f32) -> ProbeLua {
        ProbeLua {
            chunk: "ProbeLog('x')".into(),
            at: 10.0,
            again: 4.0,
            entries,
            fired,
            ready_at,
            ..Default::default()
        }
    }

    #[test]
    fn first_fire_waits_for_the_process_delay() {
        let p = probe(1, 0, 2.0);
        assert!(!probe_lua_due(&p, 9.9), "before WOW_PROBE_LUA_AT");
        assert!(probe_lua_due(&p, 10.0), "at WOW_PROBE_LUA_AT");
    }

    #[test]
    fn nothing_fires_before_the_first_entry() {
        assert!(!probe_lua_due(&probe(0, 0, 0.0), 60.0));
    }

    #[test]
    fn an_entry_takes_exactly_one_chunk() {
        assert!(!probe_lua_due(&probe(1, 1, 2.0), 60.0));
        assert!(!probe_lua_due(&probe(2, 2, 40.0), 90.0));
    }

    /// A later entry re-arms on its own settle, measured from when it became ready.
    #[test]
    fn a_later_entry_re_arms_on_its_own_clock() {
        let p = probe(2, 1, 40.0);
        assert!(
            !probe_lua_due(&p, 41.0),
            "still settling after the re-entry"
        );
        assert!(
            probe_lua_due(&p, 44.0),
            "WOW_PROBE_LUA_AGAIN past the re-entry"
        );
    }
}
