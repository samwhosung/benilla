//! The **actuation** probes: the three headless channels into a live session — scripted chat
//! sends ([`ProbeChatPlugin`]), synthetic key taps ([`ProbeKeyPlugin`]), and a Lua chunk in the
//! live UI VM ([`ProbeLuaPlugin`]). Each is env-gated, waits for the session to be in-world, and
//! schedules on [`ProbeClock`]; together they are "do what the director would do, unattended".

use bevy::prelude::*;

use super::ProbeClock;

/// The PROBE CHAT one-shot (`WOW_PROBE_CHAT="<line>[;<line>…]"`, delay via `WOW_PROBE_CHAT_AT`
/// seconds, default 8): send each `;`-separated line as Say once we are in-world — the "park the
/// probe character anywhere" instrument. The probe account (gmlevel 6) makes `.go xyz …`, `.gm on`,
/// `.additem` etc. work headlessly, which a direct `characters` DB edit does NOT (the live world
/// server's logout save overwrites it, and the row is only re-read at login). Pair with
/// [`LiveShotPlugin`] at a later `WOW_LIVE_SHOT_AT` so the destination has streamed in.
/// `WOW_PROBE_CHAT_EVERY=<secs>` spaces the lines apart instead of sending them in one burst —
/// the "do X, wait, then do Y" probe (a mount-then-dismount transition, a buff-then-cancel):
/// two field flips inside one drain merge to a no-op, so time-separated sends are what actually
/// exercise a transition (decision 0441's teardown verification).
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

/// [`ProbeChatPlugin`] state: the `;`-separated lines, the first-fire time, the per-line spacing
/// (`0` = one burst), and how many lines have gone out.
#[derive(Resource)]
struct ProbeChat {
    lines: String,
    at: f32,
    every: f32,
    sent: usize,
}

/// Submit the probe lines once the delay has elapsed AND the session is in-world (the self player
/// exists) — a `.go` sent before world-enter would be dropped server-side.
///
/// Lines go in through the **chat EditBox seam**, not straight to the wire: a probe line is "what
/// the director would type", so a client-side slash command (`/duel`, `/reaction`) is parsed by
/// the same drain that serves the real chat box, while plain text and `.gm`/`.go` server commands
/// still leave as Say exactly as before. Sending them as Say instead — the original shape — meant
/// every client-side command silently went out as public chat and did nothing.
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
        return; // not in-world yet — keep waiting past the delay
    }
    let Some(mut script) = script else {
        return;
    };
    // With no spacing every line goes in the first eligible frame (the original burst); with
    // `every`, line N waits until `at + N·every` — the "do X, wait, then do Y" cadence.
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

/// The PROBE KEY one-shots (`WOW_PROBE_KEY="<key>@<secs>[:<hold>][;…]"`): synthesize a key press
/// at each given time once in-world, released `<hold>` seconds later ([`PROBE_KEY_TAP_SECS`] when
/// omitted — the tap this instrument shipped with). The "press space headlessly" instrument for
/// input-gated behavior (the mounted flourish, a jump, the X/Z toggles), which neither a chat
/// command nor a Lua chunk can reach (1.12 has no jump Lua API; the gate lives in the
/// controller's key read).
///
/// The optional hold is what makes *sustained* locomotion reachable headlessly: a 0.25 s W tap
/// travels ~1.2 yd, far too little to cross a liquid surface's own slope, so a swim defect that
/// only appears while moving over water could not be reproduced without asking the director to
/// drive (the gap `WOW_PROBE_LOOK` closed for mouse-turns, on the key side).
///
/// Runs in `PreUpdate`
/// after winit's input processing ([`bevy::input::InputSystems`]) so the synthetic
/// `just_pressed` is visible to every `Update` reader that same frame — a press from inside
/// `Update` would be cleared at the next frame's input pass before an earlier-ordered
/// controller ever saw it.
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
                    // …and after the loading cover's input swallow, which runs in the same window
                    // and empties every button plane: an instrument driving the client is the
                    // operator, not the player, so its press has to land on the far side of it.
                    .after(crate::loading_screen::CoverInput),
            );
    }
}

/// How long a probe press stays held when the spec gives no `:<hold>`. Long enough that a
/// `pressed`-reader (a held-key gate) sees it across several frames; short enough to stay a tap.
const PROBE_KEY_TAP_SECS: f32 = 0.25;

/// The key names [`ProbeKeyPlugin`] accepts — the controller-read set; extend as probes need.
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
        // TOGGLEAUTORUN's 1.12 default. The moving-axis protocol (task: grade streaming while
        // running) engages autorun with one tap and steers with `WOW_PROBE_LOOK` — the whole
        // scripted drive out of two instruments that already existed.
        "NumLock" => KeyCode::NumLock,
        // The nameplate toggles (`vplates`): bare `V` is enemy plates, `Shift`-held `V` is
        // friendly. A probe that wants plates over a town's *friendly* NPCs — the only way to
        // measure plate behaviour without standing in aggro range of something — presses this.
        "V" => KeyCode::KeyV,
        // The detached free-fly toggle (`player.rs`) and its Ctrl speed boost (`camera.rs`, ×5).
        // Added because the harness could not reach the leg that broke decision 0793: the director's
        // first real run was a boosted free-fly, whose camera crosses the art radius in ~5 s, and
        // reproducing it needed a held `Ctrl` + `W` behind an `F`.
        //
        // The toggle is the dev chord + `F` since decision 1043 (it was a bare `F`), so a probe that
        // wants free-fly holds `Ctrl` and `Shift` across the `F` tap — hence `Shift` here. `Ctrl`
        // stays held afterwards for the ×5 boost, which is unchanged.
        "F" => KeyCode::KeyF,
        "Ctrl" => KeyCode::ControlLeft,
        "Shift" => KeyCode::ShiftLeft,
        // The cost pill's toggle, dev chord + `P` (`perf::hud::toggle_hud`). Reachable no other
        // way — it is a host chord, not a binding or a Lua verb — and since the pill starts hidden
        // (2099) a probe that wants to see it in a shot has to press it.
        "P" => KeyCode::KeyP,
        // The text-editing keys. Added for decision 1077's hyperlink-atomicity law, whose whole
        // observable — "one BACKSPACE removes a whole item link" — is a keypress no chat command
        // and no Lua chunk can reach (`EditBox` has no Lua deletion API; the law lives behind the
        // host's chord table). Without these a caret/deletion defect could only be reproduced by
        // asking the director to type, which is the asymmetry probes exist to remove.
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Left" => KeyCode::ArrowLeft,
        "Right" => KeyCode::ArrowRight,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        // Enter at the character-select screen IS "enter world" (`char_select::input`), which makes
        // the **second** login of a run reachable headlessly: `/logout` then `Enter` reproduces a
        // character switch in one process. Session-scoped state that survives that boundary is a
        // whole bug class, and until this key existed no probe could reach it — 1284's stale channel
        // membership had to be found by the director, twice. `WOW_CHAR` is a deliberate one-shot
        // (`run_mode`), so synthesizing the keypress is the only way back into the world in-process.
        "Enter" => KeyCode::Enter,
        // Print screen. The whole player path — key → SCREENSHOT binding →
        // `TakeScreenshot()` → `Screenshot()` → the writer → SCREENSHOT_SUCCEEDED — is reachable
        // no other way: a Lua chunk can call the verb but skips the binding, and B261's contract
        // ("the message is not in the file") is a property of the KEY press, since the whole
        // reason `TakeScreenshot` hides the status line is a second press landing on a live one.
        // On macOS this arrives as F13 (`bindings::chord`'s fold) and the token is the same.
        "PrintScreen" => KeyCode::PrintScreen,
        _ => return None,
    })
}

/// [`ProbeKeyPlugin`] state: one entry per scheduled tap, plus the once-armed latch.
#[derive(Resource)]
struct ProbeKeys {
    taps: Vec<ProbeKeyTap>,
    /// **A self player has existed at some point this run.** The gate used to be "a self player
    /// exists *right now*", which is what keeps a tap from firing into the loading screen — but it
    /// also made every glue screen unreachable, and the second half of a `/logout` + `Enter`
    /// character-switch probe happens at exactly one of those (1284). Latching it keeps the "not
    /// before the world" intent and drops the "only while in it" accident.
    armed: bool,
}

struct ProbeKeyTap {
    key: KeyCode,
    at: f32,
    /// Seconds the key stays down — the spec's `:<hold>`, else [`PROBE_KEY_TAP_SECS`].
    hold: f32,
    pressed: bool,
    released: bool,
}

/// Press each due tap (in-world gated, like the chat probe) and release it after its hold window.
///
/// Both input currencies, deliberately: `ButtonInput` for every held-state reader (and
/// the binding dispatch's stuck-latch sweep, which treats "latched but not pressed" as a missed
/// release), plus the raw [`KeyboardInput`] message the binding dispatch's press/release edges
/// actually consume — a state-only synthetic press was invisible to the chord latcher, which
/// would have silently killed this instrument the day the dispatch landed.
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
    // Publish what we are holding, so the macOS stuck-modifier reconciler
    // ([`benilla_world::modkeys`]) doesn't undo it: it polls the *hardware* flags, which read
    // "up" for a key no hand is on, so before this every synthesized chord — `SHIFT-V`,
    // `Ctrl`+`Shift`+`F` — was released the frame after it was pressed, and the release was
    // logged as a stuck-key correction. Rewritten only when the held set changes (a plain
    // `ResMut` write every frame would mark the resource changed forever).
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

/// The PROBE LUA driver (`WOW_PROBE_LUA="<chunk>"`, first fire at `WOW_PROBE_LUA_AT` seconds,
/// default 10; re-armed `WOW_PROBE_LUA_AGAIN` seconds into every LATER world entry, default 4):
/// run one Lua chunk in the live UI VM — the "press the button headlessly" instrument. The chunk
/// drives the REAL FrameXML API surface (`CastSpell`, `UseAction`, `TargetUnit`, …), so whatever
/// it triggers takes the exact app path a click takes — a headless wire probe can measure the
/// server, but only the live VM exercises the button feed and the widget clock.
///
/// **The chunk fires once per WORLD ENTRY, not once per process** (decision 2116's named gap).
/// The UI VM is destroyed and rebuilt at every logout/login (1290/1291), taking `_G` and the
/// `ProbeLog` channel below with it — so while this was a process-wide one-shot, a probe could
/// not read the same value before and after a relog *at all*. 2116 needed exactly that (the
/// cooldown triple across a `/logout`) and had to fall back to `WOW_PROBE_CHAT` sending
/// `/script … error("…")` on a schedule and reading the warn line the input drain logs — a trick
/// standing in for an instrument, and a morning instead of an hour.
///
/// **World entry, and deliberately not "VM rebuild"**, which is the wider edge and the wrong one:
/// `ReloadUI()` rebuilds the VM without leaving the world, and `WOW_PROBE_LUA="ReloadUI()"` is a
/// probe we actually run. Re-arming on the VM would make that chunk reload the
/// interface forever. [`ProbeLog`](install_probe_log) *does* follow the VM, because it is a
/// channel rather than an action.
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

/// [`ProbeLuaPlugin`] state: the chunk, the two schedules, and the per-entry bookkeeping that
/// replaced the process-wide once-latch.
#[derive(Resource, Default)]
struct ProbeLua {
    chunk: String,
    /// `WOW_PROBE_LUA_AT` — the FIRST fire, on the run's own [`ProbeClock`].
    at: f32,
    /// `WOW_PROBE_LUA_AGAIN` — the settle every LATER entry gets, on its own clock.
    again: f32,
    /// World entries that have become able to take a chunk — counted on the rising edge.
    entries: u32,
    /// Entries already fired into; `fired < entries` is "this entry is still owed its chunk".
    fired: u32,
    /// When the current entry became ready — the re-arm's zero.
    ready_at: f32,
    /// Last frame's readiness, so the count above is an edge and not a per-frame increment.
    was_ready: bool,
    /// The VM session `ProbeLog` was last installed into. `0` is "none" —
    /// [`benilla_ui::script::UiScript::session`] hands out from 1, so a fresh probe matches no VM
    /// and the first ready frame installs.
    logged_vm: u64,
}

/// **Is this entry's chunk due?** — the whole schedule, as one decision a test can hold.
///
/// The first fire keeps `WOW_PROBE_LUA_AT`'s run-relative meaning ([`ProbeClock`], whose zero
/// trails process start by the boot), and that is not inertia: every recorded probe line in
/// `decisions/` is tuned against it (`WOW_PROBE_LUA_AT=34` in `scripts/summon-live.sh`, and a
/// dozen records besides), and re-reading it as "34 s after the loading screen" would quietly
/// stop reproducing all of them. A later entry cannot use that
/// clock at all — "34 s into the run" is long past by the time a relog lands — so it runs on
/// its own settle instead, measured from the moment that entry could take a chunk.
fn probe_lua_due(probe: &ProbeLua, now: f32) -> bool {
    if probe.fired >= probe.entries {
        return false; // this entry has had its chunk (or there is no entry yet)
    }
    if probe.fired == 0 {
        now >= probe.at
    } else {
        now >= probe.ready_at + probe.again
    }
}

/// Install `ProbeLog(text)` — the chunk's data channel OUT of the VM, as greppable `probe-log:`
/// lines; before it existed a probe could only report through screenshots or by erroring.
///
/// **Into whichever VM is live, and re-installed whenever that changes**. A
/// global installed once is gone at the next logout/login *and* at the next `ReloadUI()`, both of
/// which build a new Lua state (1290/1291) — so a chunk that logged happily on its first run died
/// on a nil call afterwards, which is precisely the shape of failure that reads as "the probe did
/// nothing". Installed only while a probe chunk is configured; never part of the shipping API
/// surface.
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

/// Run the probe chunk once per world entry, on [`probe_lua_due`]'s schedule.
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
        // No VM in the world at all — the deferral window between the world-entry edge and the
        // entry load, where the boot VM is parked out of every feed's reach
        // (`ui_script::ParkedBootVm`). Not an entry yet.
        probe.was_ready = false;
        return;
    };
    // **Ready = this entry can take a chunk**, which is three more facts on top of the VM the
    // block above just took. A chunk driving `CastSpell` against the *boot* VM — strings, emote
    // tokens and fonts, not one frame — dies on a nil, and "attempt to call a nil value" out of a
    // probe reads as a benilla bug rather than as a mis-scheduled probe.
    //
    // - **`InWorld`** — the entry edge has been taken, which is what ARMS the deferral window
    //   below. Without it the window has a hole on its leading side: the entered-world message
    //   and our first object update can arrive in the same drain, a frame ahead of the state
    //   transition that parks the VM, so a chunk already past `at` would run there.
    // - **`PendingEntryUiLoad` gone** — the same window named from the other side: the in-game
    //   UI is built, not merely owed.
    // - **a self player** — `UnitName("player")` is in half the chunks anyone writes.
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

/// The PROBE DRAG driver (`WOW_PROBE_DRAG="A>B[;C>D…]"`, delay via `WOW_PROBE_DRAG_AT` seconds
/// (default 14), one gesture step per `WOW_PROBE_DRAG_STEP` seconds (default 0.1)): drag one named
/// frame onto another **through the real pointer path**, headlessly, in a live session.
///
/// **Why this exists as an instrument rather than a unit test.** The UI's own harness can already
/// press, move and release ([`UiScript::mouse_button`]/`mouse_move`), and a drag test written
/// there passes while the live client's drag is broken — because the harness supplies the whole
/// world: no per-frame app feed between the press and the release, no real frames elapsing, no
/// resolve pass, no `feed_party` re-push, no OS cursor. That gap cost a session (the raid
/// grid's second drag). This closes it by driving the SAME gesture in a real client: the press,
/// the threshold-crossing move, the path, and the release each land on their own frame, with
/// every app system running in between exactly as it does for a hand on the mouse.
///
/// It names FRAMES, not coordinates, and reads their centres out of the live layout, so a probe
/// line survives a window moving. `WOW_PROBE_DRAG_LUA="<chunk>"` runs a chunk after each gesture —
/// the report channel (`ProbeLog` from [`ProbeLuaPlugin`] is installed by that probe; this one
/// prints the chunk's own returned string).
/// The HOVER SWEEP (`WOW_PROBE_HOVER="Frame1;Frame2;…"`, `WOW_PROBE_HOVER_AT` seconds, default 14,
/// `WOW_PROBE_HOVER_STEP` seconds per frame-to-frame step, default 0.25, looping until exit) —
/// the cursor crosses each named frame's centre through the REAL pointer path, pressing nothing.
///
/// **A leg that does not ALTERNATE cannot be read.** `WOW_PROBE_HOVER_DUTY=<on>:<off>` sweeps for
/// `on` seconds, parks the pointer over nothing for `off`, and repeats. Compare-the-two-halves
/// designs that put all the parked frames at the start and all the swept frames after read the
/// run's own drift as the effect: three legs of the same gesture, same binary, gave within-run
/// `cpu_ms` deltas of +2.03, +1.02 and −0.61 ms while their per-phase µs columns agreed to 3%.
/// Alternating pools both regimes across the same minutes, so drift cancels
/// instead of landing on one side.
///
/// **`WOW_PROBE_HOVER_STEP` is the dial that decides what is being measured, and the default is
/// not the director's gesture.** A hand spamming hovers moves the mouse *every frame*; at the
/// 0.25 s default only 4 frames in 60 carry a pointer move, which divides whatever a move costs by
/// fifteen and reads as "hovering is free" (it did, three times, before this note existed). Set it
/// to ~0.016 to sweep at frame rate. `WOW_PROBE_HOVER_JITTER=<px>` then splits the two halves the
/// sweep otherwise fuses: with one name and a jitter, the pointer moves every frame while the
/// hovered frame and its tooltip stay put.
///
/// Built because a hover-cost pin was taken with a Lua driver that called `GameTooltip:SetOwner`
/// directly, and that driver is not the gesture: it never moves `model.mouseover`, so it skips the
/// hit test, `OnEnter`/`OnLeave`, the button's state textures and highlight, and whatever the
/// shipped UI hangs off those. The reported symptom was "hovering costs 2 ms" and the instrument
/// could not hover. This is the missing half of `WOW_HOVER_LOG`: that one records what a hand on
/// the mouse does, and this one supplies the hand.
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
    /// `WOW_PROBE_HOVER_JITTER=<px>` — nudge each step off the frame's centre by up to this many
    /// pixels, cycling through a small square. Zero (the default) parks the cursor exactly on the
    /// centre, so a ONE-NAME sweep issues the same coordinates forever and the pointer never moves.
    jitter: f32,
    /// `WOW_PROBE_HOVER_DUTY=<on>:<off>` — alternate `on` seconds of sweeping with `off` seconds
    /// parked off every frame, forever. See the plugin doc for why a leg that does not alternate
    /// cannot be read at this resolution.
    duty: Option<(f32, f32)>,
    i: usize,
    next: f32,
    announced: bool,
    /// Which half of the duty cycle we are in (meaningless when `duty` is `None`).
    parked: bool,
}

/// Where the pointer goes during a duty cycle's parked half: the top-left corner, over nothing.
const HOVER_PARK_AT: (f32, f32) = (2.0, 2.0);

/// Move the cursor onto the next named frame's centre. Loops forever, so a run of any length is a
/// steady sweep — the population the recorder splits on.
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
    // The duty cycle, if armed: which half of the current period is this, and did it just flip?
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
        // Named but unresolved is worth saying once per pass rather than silently sweeping air —
        // a sweep over frames that do not exist reads as "hovering is free".
        None => warn!("probe-hover: {name} has no resolved rect — nothing hovered this step"),
    }
}

/// A deterministic offset inside a `j`-pixel square, cycling with the step index.
///
/// It exists to separate the two costs a hover sweep otherwise fuses: **the pointer moved** (the
/// hit test re-runs) and **the hovered frame changed** (the tooltip is torn down and rebuilt). A
/// one-name sweep with a jitter moves the pointer every step while the hovered frame — and the
/// tooltip on it — stay put, which is the first of the two on its own.
fn jitter_offset(j: f32, i: usize) -> (f32, f32) {
    if j <= 0.0 {
        return (0.0, 0.0);
    }
    // A 4-phase square walk: (+,+) → (-,+) → (-,-) → (+,-). Every step is a real move, and the
    // cursor never leaves a `j`-pixel box around the centre.
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

/// [`ProbeDragPlugin`]'s state machine: which pair, which step of the gesture, and when the next
/// step is due.
#[derive(Resource)]
struct ProbeDrag {
    pairs: Vec<(String, String)>,
    at: f32,
    step: f32,
    report: String,
    pair: usize,
    /// The step within the current gesture — see [`fire_probe_drag`]'s table.
    phase: usize,
    next: f32,
    from: (f32, f32),
    to: (f32, f32),
}

/// One frame's centre in the VM's own units, read out of the live layout (`GetLeft`/`GetRight`/
/// `GetTop`/`GetBottom` — the same space [`crate::ui_script::input`] feeds the cursor in).
/// `None` when the frame does not exist or has no resolved rect.
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

/// Advance the scripted drag by at most one step per `probe.step` seconds.
///
/// The gesture is deliberately spread across real frames rather than run in one call: a press and
/// a release inside a single frame is a *click*, and the whole point here is to exercise what the
/// app does BETWEEN them.
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
        // Read both centres and park the cursor on the source — the hover the player makes
        // before they press.
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
        // Past the 4-px drag threshold — this is the move that fires `OnDragStart`.
        2 => {
            let (x, y) = probe.from;
            script.mouse_move(x + 8.0, y + 8.0);
        }
        // Two waypoints along the path, so anything that only breaks while the drag is IN FLIGHT
        // (a repaint that re-seats the row under the cursor, say) gets frames to do it in.
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
        // The self-check: hover the source where it now DRAWS and ask who the hit test says is
        // there. A frame whose hit rect and drawn rect have parted company is invisible to any
        // harness that presses the centre it just read — and it is exactly what "I can't grab
        // anything any more" looks like from a hand on the mouse.
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
            // Hand the pointer back between gestures: the app's own feed runs for a frame, which
            // is what a player's hand does too, and is where a stale hover would show up.
            synthetic.0 = false;
            return;
        }
    }
    probe.phase += 1;
}

#[cfg(test)]
mod tests {
    use super::{probe_lua_due, ProbeLua};

    /// A probe configured the way `WOW_PROBE_LUA=… WOW_PROBE_LUA_AT=10` configures one, with the
    /// per-entry bookkeeping set by hand.
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

    /// The first entry is still `WOW_PROBE_LUA_AT` on the PROCESS clock — the meaning every
    /// recorded probe line in `decisions/` is tuned against.
    #[test]
    fn first_fire_waits_for_the_process_delay() {
        let p = probe(1, 0, 2.0);
        assert!(!probe_lua_due(&p, 9.9), "before WOW_PROBE_LUA_AT");
        assert!(probe_lua_due(&p, 10.0), "at WOW_PROBE_LUA_AT");
    }

    /// Nothing fires before there is a world entry to fire into — the login and character
    /// screens sit here however long they take.
    #[test]
    fn nothing_fires_before_the_first_entry() {
        assert!(!probe_lua_due(&probe(0, 0, 0.0), 60.0));
    }

    /// One chunk per entry, and no more: the frames after a fire are not a second one.
    #[test]
    fn an_entry_takes_exactly_one_chunk() {
        assert!(!probe_lua_due(&probe(1, 1, 2.0), 60.0));
        assert!(!probe_lua_due(&probe(2, 2, 40.0), 90.0));
    }

    /// **The gap 2116 hit head-on**: the second entry re-arms, on its OWN clock. The process
    /// delay is long past by the time a relog lands, so reusing it would fire the chunk into the
    /// loading screen; the settle is measured from the moment that entry could take a chunk.
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
