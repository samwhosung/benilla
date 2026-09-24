//! The UI-engine bridge: hosts [`benilla_ui::script::UiScript`] (the Lua VM + widget arena +
//! layout, all engine-free) and feeds its [`extract`](benilla_ui::script::UiScript::extract) output
//! into the quad pass ([`crate::ui_pass::UiQuads`]) every frame. This is the seam decision 0068
//! draws between the engine-free crate and the app: everything above it is data + Lua; everything
//! below it is Bevy.
//!
//! Coordinates: the script/layout side is WoW UI space — **y-up**, origin bottom-left, in the
//! client's 768-virtual units (decision 0582): `set_screen_size` feeds a screen `768/uiScale`
//! units tall every frame, and [`seam_scale`] carries quads ×s out / mouse ÷s in. The quad pass
//! wants y-down window px, so extraction also flips through the window height.
//!
//! The **unit frames** (the reference's own `Interface\FrameXML\{PlayerFrame,PartyFrame,
//! TargetFrame,PetFrame}.xml`, off the player's chain since 1751 — our `UnitFrames.xml`
//! transcription is retired) load at startup through [`benilla_ui::loader`] — the decision 0068 slice-1
//! game-shell: real FrameXML+Lua rendering unit health/power/level/name through the whole chain
//! (snapshot → `Unit*` bindings → Lua → event → StatusBars+text → quad pass). Since captures run
//! server-less (no real game state), `WOW_CAPTURE_UI=1` also feeds synthetic `"player"`/`"target"`
//! snapshots ([`demo_unit_feed`]) so the frames populate on screen. `FontString` regions draw
//! through [`crate::ui_text::layout_text_quads`] against the glyph atlas
//! [`crate::ui_text::UiFontAtlas`] builds at startup (decision 0068 §2).

use bevy::prelude::*;

use benilla_ui::script::{ActionSlot, ScriptValue, UiScript, UnitState};

use crate::ui_unit::UnitFeed;
use benilla_world::schedule::WorldStage;

/// The addon folder: discovery, manifests, the enable file, and the load walk. `pub(crate)`
/// because the AddOns screens read the same folder without a VM (decision 1196).
pub(crate) mod addons;
mod content;
pub(crate) mod extract;
mod input;
mod manifest;

/// The reference FrameXML this client EXECUTES off the player's own patch chain instead of
/// transcribing it — the rule, the list, and the licensing reason are that module's header.
mod reference_ui;

/// What the host remembers about the VM, and how it forgets — the one mechanism that stops a seed
/// or a change-memo outliving the VM it was written against (decision 1290).
mod session;

/// The feed gate (decision 1439): the input-side early-out for a per-frame UI feed, its
/// [`gate::Watch`] counter memory, and the `WOW_FEED_GATE_CHECK=1` audit that catches a gate
/// missing an input.
pub(crate) mod gate;

pub(crate) use session::VmMemo;

// The manifest's loaders read as `ui_script::…` at every call site, including the tests' `super::`.
// `load_default_ui` is no longer test-only: the addon harness (1188 phase 6) loads the whole
// shipped interface under each surveyed addon, because roughly half of what an addon calls is
// FrameXML's Lua rather than the engine's (decision 1190).
pub(crate) use manifest::load_default_ui;
pub(crate) use manifest::{load_font_registry, load_ingame_ui};

/// Is the pointer over *any* UI this frame — the egui dev overlay OR a mouse-enabled player-UI
/// frame? The single source of truth for "the mouse is talking to the UI, not the world",
/// combined by [`arbitrate_pointer_over_ui`] from both contributors (dev overlay =
/// [`EguiPointerOver`]; player UI = [`PlayerUiHover`]). Gameplay reads it so
/// a drag doesn't start mouse-look; the inspector reads it so a pick doesn't fire behind an
/// overlaid frame. Owned HERE, not by the dev plugin (decision 0026): gameplay's read must
/// survive a build without the dev overlays, so the combiner treats the egui half as optional.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUi(pub(crate) bool);

/// **A synthetic pointer owns the mouse this frame** — set by the headless drag probe
/// ([`crate::capture::ProbeDragPlugin`]) while it drives a gesture through the real pointer path.
///
/// [`input::feed_ui_input`] skips its whole mouse half while this is set, and that is the ONLY
/// thing it does. Without it a scripted gesture cannot exist: the OS cursor is wherever the
/// director left it (usually outside a backgrounded probe window), so every frame between the
/// synthetic press and the synthetic release would feed the real position — dragging the frame to
/// the wrong place at best, and at worst calling `pointer_left_window`, which disarms the very
/// gesture the probe just armed. The keyboard half is untouched: a probe driving the mouse has no
/// business swallowing keys.
#[derive(Resource, Default)]
pub(crate) struct SyntheticPointer(pub(crate) bool);

/// **A capture never reads the OS pointer** — set for the whole life of a `$WOW_CAPTURE` /
/// `$WOW_CAPTURE_UI` run, and never cleared.
///
/// [`input::feed_ui_input`] treats this exactly like [`SyntheticPointer`]: the mouse half of the
/// pass is skipped entirely, touching nothing. The keyboard half is untouched.
///
/// **Why it is not just `SyntheticPointer`.** They are different statements. That one means "a
/// probe is driving a gesture *right now*", and the drag probe clears it when its gesture ends
/// (`capture::probes::act`) — so borrowing it would re-expose the real cursor for the rest of the
/// run, which is the opposite of the guarantee wanted here.
///
/// **Why it exists at all.** The mouse feed reads `window.cursor_position()`, so a UI capture's
/// pixels depended on where the person at the keyboard had left their mouse: a cursor resting over
/// the window arms a hover and a tooltip that a cursor an inch to the left does not. The old
/// defence was an assumption written in a comment — *"probes park the cursor outside"* — with
/// nothing enforcing it. On 2026-08-26 a `ui-tooltip` A/B came back with an MAE of 5.275 against
/// 0.020 for every other UI scenario; that particular anomaly turned out to be a rebuilt-between-
/// legs mistake, and three attempts to reproduce a cursor-driven one all failed, because
/// `CGWarpMouseCursorPosition` does not synthesise the events winit needs. So the hazard was left
/// on the record as **suspected, unproven** — and this closes it by construction instead, which
/// costs nothing and does not require ever winning that argument. A capture that cannot read the
/// pointer cannot be perturbed by it.
#[derive(Resource, Default)]
pub(crate) struct CapturePointerPinned(pub(crate) bool);

/// The egui dev overlay's half of the pointer arbitration, written each egui pass by the debug
/// panel's `track_pointer_over_ui`. **Defined here, with the arbiter that reads it** (decision
/// 1174 finishing 0026): the type has to exist in a build with no dev overlays compiled in, and
/// the arbiter takes it as `Option<Res<…>>` so its absence simply means "nothing is hovering the
/// dev UI" — which is the player-faithful answer. The writer lives in `debug_panel`; a dev
/// module writing an always-present fact is the allowed direction.
#[derive(Resource, Default)]
pub(crate) struct EguiPointerOver(pub(crate) bool);

/// Whether mouseover **world picking** is armed — the dev-chord `I` inspector's mode, toggled by
/// `debug_panel::inspect`.
///
/// The second of the two resources 0026 named as needing "a player-safe home", and here for the
/// same reason as [`EguiPointerOver`]: `player::control` and `target::click` read it every frame
/// to decide whether a left-click is the inspector's or the game's, and those reads must compile
/// and behave in a build with no inspector. The [`Default`] — **disarmed** — *is* the player
/// behaviour, so a player build's readers take the ordinary branch with nothing to switch them.
#[derive(Resource, Default)]
pub(crate) struct InspectMode {
    pub(crate) enabled: bool,
}

/// One frame's UI-pass phase split, in μs — written by [`extract::tick_script`] and [`extract::paint_script`] under the same
/// marks the `[ui-cost]` line prints. **Owned by the producer** (decision 1174): the split is a
/// fact this pass publishes about itself, so it must exist whether or not the recorder that reads
/// it (`hover_log`) is compiled in. Its consumers are instruments; its writer is not.
#[derive(Resource, Default, Clone)]
pub(crate) struct UiFrameCost {
    /// How many FontStrings the layout asked the font engine to shape this frame, and the first
    /// few by name — a steady hover that keeps asking is the churn the recorder exists to catch.
    pub(crate) measured: usize,
    pub(crate) measured_texts: Vec<String>,
    pub(crate) tick: u128,
    pub(crate) resolve: u128,
    pub(crate) measure: u128,
    pub(crate) extract: u128,
    pub(crate) convert: u128,
    pub(crate) diff: u128,
    pub(crate) quads: usize,
    pub(crate) solves: u64,
    /// How many times this frame's resolve DERIVED the layout graph from scratch (decision 1388's
    /// `layout_derivations`). The law is zero, and it is here because it was not: the recorder was
    /// built for the hover-cost symptom and reported `solves` — the cheap term — while a
    /// derivation, ~30× more expensive and paid on the same frames, was invisible to it
    /// (decision 1625).
    pub(crate) derives: u64,
    pub(crate) skipped: bool,
    /// How many entries the per-entry splice re-converted this frame — `0` on a settled or
    /// full-conversion frame. Nonzero is the proof the splice path fired (the equivalence tests
    /// and the live `[ui-cost] spliced=` field both read it).
    pub(crate) spliced: usize,
    /// How many entries the splice *dropped* — drew last frame and does not draw now. A frame
    /// that only closes something has `spliced == 0` and still rode the splice, so this is the
    /// other half of "did the splice path fire" (decision 1638). Not a CSV column: the recorder's
    /// row is the phase timings, and `[ui-cost] dropped=` already carries it inline.
    pub(crate) dropped: usize,
}

/// Does anything want [`UiFrameCost`] filled in this run? Measuring the split costs a clock read
/// per phase plus the churn strings, so the pass only pays when asked.
///
/// `WOW_UI_COST=1` (this module's own `[ui-cost]` line) arms it inline; the hover recorder arms it
/// by setting this — the direction that keeps the pass free of the instrument's name, and the
/// reason the flag is a resource rather than an env read here (decision 1174). Default `false` is
/// the player answer.
#[derive(Resource, Default)]
pub(crate) struct UiCostWanted(pub(crate) bool);

/// The frame the cursor is currently over (a mouse-enabled, visible player-UI frame), or `None`.
/// Written by [`feed_ui_input`], read by the pointer arbiter below so world-pick
/// and camera-look yield to the UI (decision 0026's single-source `PointerOverUi`).
#[derive(Resource, Default)]
pub(crate) struct PlayerUiHover(pub(crate) Option<u32>);

/// **Who owns this frame's keys** — written by [`feed_ui_input`], read by the binding dispatch and the
/// dev keyboard readers, all of which are ordered after `UiInput` so they see this frame's value and
/// not last frame's. The app-side twin of the client's `DAT_00cf4dc8 != 0` gate.
///
/// **Two fields because the reference has two mechanisms, and collapsing them into one boolean was
/// bug 2196.** A focused EditBox swallows *every* key for as long as it holds focus
/// ([`typing`](Self::typing)); a shown keyboard-enabled *frame* swallows *the one key* its
/// existence gate ate this frame ([`consumed`](Self::consumed)). Both suppress the key's binding —
/// and nothing more. Neither releases anything already held: the only things that clear the
/// reference's direction bits are the OS **window deactivate** (`0x514490`, whose sole caller
/// `0x493058` hangs off the WM_ACTIVATE callback slot) and the world-enter cascade (`0x5144c0`).
/// A UI focus change clears nothing — the reason holding
/// W keeps you running while you type or read the map. See `decisions/2196`.
#[derive(Resource, Default)]
pub(crate) struct UiKeyboardCapture {
    /// True while a focused EditBox is eating every key (`0x77b35e` returns 1 on every path but
    /// the alt-arrow one below). Whole-frame, because there is at most one focused box.
    pub(crate) typing: bool,
    /// The keys a shown keyboard-enabled **frame** consumed this frame (decision 1319's existence
    /// gate, `0x76b7d0`) — `WorldMapFrame`'s fullscreen `OnKeyDown`,
    /// `CinematicFrame`, the stack-split spinner. **Per key, not per frame**: the map eating its
    /// own `M` must not also suppress an unrelated binding, and — the bug this list exists for —
    /// must not be mistaken for a text box taking focus.
    ///
    /// Raw [`bevy::input::keyboard::KeyCode`]s, as the message carried them (the binding dispatch
    /// normalizes for chord lookup, but matches this list on the raw code it read).
    pub(crate) consumed: Vec<bevy::input::keyboard::KeyCode>,
    /// **The four arrow keys are exempt this frame** — the focused box is in alt-arrow mode
    /// (`ignoreArrows` / `SetAltArrowKeyMode`) and ALT is not held, so the reference's own key
    /// handler declines LEFT/UP/RIGHT/DOWN at `0x77b1c4` and the strata walk carries them down to
    /// `CGWorldFrame`, which runs their bindings. That is what lets you turn while the chat box
    /// has focus.
    ///
    /// It is a whole-frame flag rather than a per-key one because both of its terms are:
    /// there is at most one focused box, and ALT is read off the same modifier mirror. Only the
    /// four arrows may use it — every other key a focused box still swallows, since the
    /// reference's handler returns 1 on every other path (`0x77b35e`).
    pub(crate) arrows_fall_through: bool,
}

/// Set each frame by [`feed_ui_input`]: true for a LEFT press this frame that hit no frame but was
/// consumed by the UI — the world-drop of a held cursor payload over EMPTY world (0216 §3,
/// narrowed by decision 0571: over a world object the reference keeps an item payload and runs
/// SELECT, so the press is NOT consumed there): world click-pick and camera orbit-start must
/// yield exactly as they do for a hovered click.
#[derive(Resource, Default)]
pub(crate) struct PlayerUiClickConsumed(pub(crate) bool);

/// Set each frame by [`feed_ui_input`] (after the mouse feed, so a same-frame pickup counts):
/// whether the cursor currently carries a payload ([`UiScript::cursor_payload`]) — a `Send`
/// mirror for world-click consumers that shouldn't take the `NonSend` VM just to ask. Read by
/// [`crate::target`]'s select router: a click on NOTHING (sky) with a payload held never
/// deselects (the reference's nothing-leg `SetSelection(0,0)` is no-payload-gated — `0x492d30`'s
/// local flag test; the terrain leg is not — decisions 0571 + 0574).
#[derive(Resource, Default)]
pub(crate) struct CursorPayloadHeld(pub(crate) bool);

/// What [`extract::tick_script`] hands [`extract::paint_script`] — the UI pass is two systems, and
/// this is the frame they agree on (decision 2168).
///
/// `live` is the load-bearing field: the tick half returns early with no VM or no window, and the
/// paint half must then do nothing at all rather than re-derive a seam for a frame the VM never
/// had. Everything else is a value the two halves must not compute twice, because computing it
/// twice is how they would come to disagree.
#[derive(Resource, Default)]
pub(crate) struct UiPassState {
    /// The tick half ran to completion this frame.
    pub(crate) live: bool,
    /// The 768-virtual seam scale (decision 0582/0584) this frame was ticked under.
    pub(crate) seam: f32,
    /// The window's device scale factor.
    pub(crate) dpi: f32,
    /// The meter's first three phases, so one `[ui-cost]` row still describes one frame.
    pub(crate) us_tick: u128,
    pub(crate) us_resolve: u128,
    pub(crate) us_measure: u128,
    /// The layout counters as they stood before the tick, so the row's `solves`/`derives` cover
    /// BOTH halves' resolves.
    pub(crate) solves_before: u64,
    pub(crate) derives_before: u64,
}

/// **The pointer is over UI CHROME** — [`PointerOverUi`] minus the nameplates.
///
/// A plate is real mouse-enabled UI (2148/2159): it takes the hover, fires an addon's `OnEnter`,
/// and a click on it selects. It is not a *panel*, though, and the two consumers that mean "the
/// player is working in the interface, keep the world's hands off the mouse" have to say so:
///
/// - the camera's world-mouse latch — a right-drag that starts on a plate must still turn the view
///   (decision 2159 §2, which argued it from the reference's own `0x60f830`);
/// - the WHEEL — scrolling with the cursor on a plate must still zoom.
///
/// Both were regressions of the day a plate became a widget, and both are the same mistake, so
/// they are one named concept rather than two `&& plate_hover.is_none()`s.
///
/// **This is a narrowing of a coarse flag, not a fidelity claim.** The reference has no
/// "pointer over UI" boolean at all: it walks the strata for a frame that handles the message, and
/// a notch over any frame with no `OnMouseWheel` falls through to `CGWorldFrame` — an action button
/// included. Ours is one bit for the whole interface, and the plate is where that bit is visibly
/// wrong. Widening it to the reference's per-frame dispatch is its own arc.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUiPanel(pub(crate) bool);

/// The player-UI FEED phase — every push into the VM that this frame's tick must see: the
/// snapshot feeds, the one-shot drains that `fire_event`, the per-VM seeds. Chained
/// `UiFeed` → [`UiInput`] → [`UiPaint`] and ordered after [`WorldStage::Net`] (decision 2304), so
/// a feed reads what this frame's packets did, and a handler its event fires reads what every
/// other feed pushed — the reference's own frame, where packet dispatch precedes every
/// `FrameScript_SignalEvent` and both precede `OnUpdate`. The failure the order closes was found
/// in the unit feed first ([`crate::ui_unit::UnitFeed`]'s doc): unordered, `apply_net_updates`
/// could land between two feeds, and a synchronous handler fired by the later one re-read the
/// earlier one's pre-mutation push. **No run condition on the set** — whether a feed may run
/// before the in-game interface exists is a judgement per feed (2232), and the ones that may not
/// ride `UnitFeed`, a gated sub-phase of this one. Every system holding the VM in `Update`
/// declares its side of the tick — this set, or `.after(UiInput)` — and
/// `game_plugins::schedule_tests` holds it on the built schedule.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiFeed;

/// The player-UI PAINT pass — the quad walk ([`extract::paint_script`]), ordered after the world's
/// camera update so that world-anchored widgets are drawn from this frame's camera (decision 2168).
/// The last of the three UI phases (see [`UiFeed`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiPaint;

/// The player-UI input pass — the tick, hit-testing + handler firing. Ordered before
/// [`WorldStage::Input`] so the [`PlayerUiHover`] it produces is folded into `PointerOverUi` before
/// `player::control` reads it; the middle of the three UI phases (see [`UiFeed`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiInput;

/// The frame's atomic (`Instant`, `GetTime`) clock pair — the ONE lawful base for mapping a
/// store-side `Instant` onto the VM's `GetTime` clock (`CooldownInfo::ui_triple` and kin).
///
/// Written at the single `script.tick` site ([`extract::tick_script`]): `ui_now` is the value the
/// VM clock just advanced to, `anchor` is `Time<Real>`'s own `last_update()` — the exact instant
/// whose frame-to-frame differences ARE the deltas the VM clock accumulates. Because both legs
/// advance in lockstep by construction, a conversion `ui_now - (anchor - start)` yields the SAME
/// number every frame for one fixed `start` — the frame-stability 0375's absolute-start triples
/// require. Converting through `Instant::now()` sampled inside a feed system instead (the pre-fix
/// shape) re-measures the tick→feed scheduling gap every frame and wobbles the derived start by
/// that jitter (±12 ms observed live), turning every running cooldown into a per-frame "changed"
/// triple — the diff churn 0375 existed to kill.
///
/// **Both legs run on the PROCESS's clock and neither restarts with the VM** (decision 2116). A
/// rebuilt VM (any logout/login, any `ReloadUI`) is handed the running clock by
/// [`lifecycle::seed_vm_clock`], which writes this pair in the same breath — the reference's
/// `GetTime` is `KERNEL32!GetTickCount`, an OS clock, and stock `Cooldown.lua` gates on
/// `start > 0`, so a clock that went back to zero at the character screen pushed every already-
/// running cooldown into the past and hid its sweep.
#[derive(Resource)]
pub(crate) struct UiClock {
    /// The `Instant` leg: `Time<Real>::last_update()` at the tick that produced [`Self::ui_now`].
    pub(crate) anchor: std::time::Instant,
    /// The `GetTime` leg: the clock's value after that tick (seconds since this process started).
    pub(crate) ui_now: f64,
}

/// The pre-boot pair, replaced by [`lifecycle::seed_vm_clock`] the moment the first VM exists —
/// `init_resource` needs it, nothing else should build one.
impl Default for UiClock {
    fn default() -> Self {
        Self {
            anchor: std::time::Instant::now(),
            ui_now: 0.0,
        }
    }
}

/// Run a Lua chunk, logging (never discarding) a failure. App systems drive the VM with fire-and-
/// forget chunks; `let _ = script.run(…)` swallows the error — the chat header machine died
/// mid-chunk on a missing `EditBox:SetTextColor` every single frame and nothing ever said so
/// (the /w caret bug). A chunk failure is always an app or engine defect; this is the mandatory
/// form for any run whose `Result` isn't otherwise consumed.
/// The FrameXML digest of the interface this process loads — see [`content::digest`]. Re-exported
/// so the corpus harness can stamp every report with the tree it measured.
pub(crate) fn framexml_digest() -> String {
    content::digest()
}

pub(crate) fn run_or_warn(script: &benilla_ui::script::UiScript, chunk: &str) {
    if let Err(e) = script.run(chunk) {
        warn!("ui_script: chunk failed: {e}");
    }
}

/// The reference's `uiScale` cvar — the user dial on TOP of the 768-virtual base (decision 0584):
/// px-per-UI-unit multiplies by it, so the VM's virtual screen is `768/uiScale` units tall — the
/// same law that makes the reference's `uiScale = 768/screenH` its known pixel-perfect setting.
/// Folded into the one seam scale by [`seam_scale`]; at `1.0` (this `Default`, what every test
/// pins) the pipeline is bit-identical to the pre-dial 0582 behavior. The app inserts
/// [`default_ui_scale`] instead — the director's taste default, `WOW_UI_SCALE=` per-run override —
/// until a real cvar system subsumes this dial (0582's named residual).
#[derive(Resource)]
pub(crate) struct UiScaleCvar(pub(crate) f32);

impl Default for UiScaleCvar {
    fn default() -> Self {
        Self(1.0)
    }
}

/// The shipped default (the reference's slider spans 0.64..1.0; 1.0 read oversized to the
/// director's eye — the taste call this dial exists for).
///
/// **It also happens to be what the reference itself lands on above ~853 px tall**, which we did
/// not know when it was chosen: `0x492f70` computes
/// `max((H > 768) ? 768/H : 1.0, 0.9)` and is called
/// **both** on a mode set and from the `useUiScale`-OFF leg (`0x4908ad`) — and OFF is the shipped
/// default (`0x8430c0` = `"0"`). So the reference is 1.0 at 768 and below, 0.96 at 1280×800, and
/// **0.9 at 1080p and 1440p**. Our flat 0.9 therefore agrees with it on every modern window and
/// diverges only *below* ~853 px tall, where the reference is nearer 1.0. Left alone deliberately:
/// this constant is a director taste call, and the divergence is confined to window heights we
/// do not ship against. The reference's own law is written here so the next session inherits it
/// rather than re-deriving it.
pub(crate) const DEFAULT_UI_SCALE: f32 = 0.9;

/// The app's `uiScale`: `WOW_UI_SCALE=` if set (clamped to the plausible dial range), else
/// [`DEFAULT_UI_SCALE`] — the env override makes taste iteration a relaunch, not a rebuild.
fn default_ui_scale() -> f32 {
    std::env::var("WOW_UI_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.5, 1.5))
        .unwrap_or(DEFAULT_UI_SCALE)
}

/// THE seam scale `s` (decision 0582 + the 0584 dial): window px per UI unit —
/// `windowH/768 × uiScale`. Every crossing of the VM boundary uses this one number: extraction
/// ×s out, mouse ÷s in, measures at ×s answered ÷s, the atlas bakes at ×s. A degenerate window
/// (h ≤ 0, the pre-winit frame) is identity.
pub(crate) fn seam_scale(window_h: f32, ui_scale: f32) -> f32 {
    if window_h > 0.0 {
        window_h / 768.0 * ui_scale
    } else {
        1.0
    }
}

/// Adds the Lua UI host as a `NonSend` resource (an mlua VM is `!Send`) and the per-frame
/// tick → resolve → extract pipeline into [`UiQuads`], plus the input pass ([`feed_ui_input`]).
pub(crate) struct UiScriptPlugin;

/// `uiScale`'s change callback (decision 2303): the dial's own `[0.5, 1.5]` clamp.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut scale: ResMut<UiScaleCvar>) {
    if ev.is("uiScale") {
        scale.0 = ev.num().clamp(0.5, 1.5);
    }
}

/// `/console reloadUI` — the reference's own console command, and the same deferred rebuild
/// `ReloadUI()` and `/reload` queue (decision 1291): through the session seam, run by
/// [`run_pending_reload`] at the top of the next frame.
fn console_reload_ui(world: &mut World, _args: &str) -> Vec<String> {
    match world.get_non_send_resource_mut::<UiScript>() {
        Some(mut script) => {
            script.queue_session_request(benilla_ui::script::SessionRequest::ReloadUi);
            Vec::new()
        }
        None => vec!["reloadUI: no interface to reload".to_string()],
    }
}

impl Plugin for UiScriptPlugin {
    fn build(&self, app: &mut App) {
        use crate::console::ConsoleCommandApp;
        app.add_observer(on_cvar);
        app.console_command("reloadUI", "Reload the interface.", console_reload_ui);
        // **The quit root of the shutdown tail, on the exit edge** (decision 1528). It was
        // `.add_systems(Update, shutdown_on_exit)` below, which cannot see the `AppExit` a player
        // produces — the close button's is written in `PostUpdate` — so quitting from in-world
        // wrote no saved variables, no per-addon files and no `AddOns.txt` at all. `Last` is after
        // every announcement; [`crate::shutdown`] is where that argument lives.
        crate::shutdown::on_app_exit(app, shutdown_on_exit.into_configs());
        app.insert_resource(UiScaleCvar(default_ui_scale()))
            // The UI pass publishes its per-frame phase split here every frame the cost meter or
            // the hover recorder is armed; the producer owns the resource so any minimal app that
            // runs the UI pass (the extract tests) has it.
            .init_resource::<UiFrameCost>()
            .init_resource::<crate::bindings::WheelNotches>()
            .init_resource::<UiCostWanted>()
            .init_resource::<PointerOverUi>()
            .init_resource::<SyntheticPointer>()
            // Not `init_resource`: the value IS the answer, read once from the env at build.
            .insert_resource(CapturePointerPinned(
                std::env::var_os("WOW_CAPTURE").is_some()
                    || std::env::var_os("WOW_CAPTURE_UI").is_some(),
            ))
            .init_resource::<EguiPointerOver>()
            .init_resource::<InspectMode>()
            .init_resource::<PlayerUiHover>()
            .init_resource::<UiKeyboardCapture>()
            .init_resource::<PlayerUiClickConsumed>()
            .init_resource::<CursorPayloadHeld>()
            .init_resource::<UiClock>()
            .init_resource::<AddOnIdentity>()
            // After `AssetSet::Open` so the patch chain exists at boot: the VM's first load is
            // the real `GlobalStrings.lua` (the reference's own FrameXML order), which the
            // cast-fail display (0427) resolves its messages from.
            .add_systems(Startup, setup_script.after(benilla_assets::AssetSet::Open))
            // The in-game UI materializes on entering the world, not at boot (1051) — the
            // reference's own seam; only the font registry loads at `Startup`. The entry edge
            // ARMS the load; it runs a few frames later, once the loading cover has actually
            // presented — the ~0.5 s burst must never stall the frame whose render would first
            // show the cover, or what covers it is the frozen character screen (0962's frame
            // accounting; see [`lifecycle::PendingEntryUiLoad`]).
            .add_systems(
                OnEnter(crate::char_select::ClientState::InWorld),
                lifecycle::arm_entry_ui_load,
            )
            // The whole shutdown tail — events then writes, in the reference's order. See
            // [`shutdown_ui_state`]; the two edges here are its five roots as far as our session
            // has them. (The quit root is registered at the top of this function, through
            // [`crate::shutdown::on_app_exit`] — it is a `Last` system, not an `Update` one.)
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                end_ui_session,
            )
            // The world latch (2239) and its create-side arm. The resource is `init_` rather than
            // `insert_` here and in [`crate::ui_unit::UiUnitPlugin`], because both of that law's
            // producers live in different plugins and either may be built alone in a test.
            .init_resource::<LeavingWorldArmed>()
            .add_systems(Update, lifecycle::arm_leaving_world_on_self_create)
            // A queued `ReloadUI()` runs in `PreUpdate` — one whole frame after the drain that
            // queued it (the reference's own deferral, `0x495590`), and BEFORE every `Update`
            // system, so no per-VM seed or feed can run against the dying VM in the reload frame
            // and then leave the new one unseeded until the next. In `Update` the exclusive
            // system would float: the scheduler could place it between `sync_cvars` and
            // `save_config`, discarding a dirty CVar edit, or after `seed_bindings`, giving one
            // whole frame with an empty binding table. See [`run_pending_reload`].
            .init_resource::<ReloadUiPending>()
            // The armed entry load shares the reload's slot, chained after it: both are
            // exclusive edges on the same VM, and a reload must not interleave a pending entry
            // load.
            .add_systems(
                PreUpdate,
                (run_pending_reload, lifecycle::run_pending_entry_load).chain(),
            )
            // **The three UI phases, in order** (decision 2304): every push after the net
            // drain, then the tick, then the paint. `UiFeed`'s own doc has the why; `UiInput`
            // sits before the world's input stage so the hover it produces reaches the pointer
            // arbiter in time (its doc).
            .configure_sets(
                Update,
                (
                    UiFeed.after(WorldStage::Net),
                    UiInput.before(WorldStage::Input),
                    UiPaint,
                )
                    .chain(),
            )
            // `GetFramerate()`'s host half (decision 1195), in the feed phase so an `OnUpdate`
            // handler reads this frame's number rather than the previous one's.
            .add_systems(Update, feed_framerate.in_set(UiFeed))
            // `tick_script` resolves layout; `feed_ui_input` hit-tests against those rects, so they
            // chain (also required because both take the single `NonSend` VM). The input pass is
            // in-world only (decision 0193): the character-select glue screen owns the pointer +
            // keyboard there (its exit edge resets the latches this pass normally drives).
            .init_resource::<UiPassState>()
            .init_resource::<PointerOverUiPanel>()
            .add_systems(
                Update,
                (
                    extract::tick_script,
                    input::feed_ui_input.in_set(crate::char_select::InWorldGated),
                )
                    .chain()
                    .in_set(UiInput)
                    // The binding dispatch (0997) runs in this same set, after the feed: it
                    // reads the capture gate the feed just wrote, so a key a focused box
                    // consumed this frame never also fires a binding.
                    .before(crate::bindings::BindingSet),
            )
            // **The paint half, after the camera** (decision 2168): the quad walk runs once the
            // world's own update has happened, so a widget anchored to the world — the nameplates,
            // which since 2148 are real `WorldFrame` children seated by `vplates::drive_vplates`
            // out of THIS frame's camera — is drawn where it belongs instead of a frame behind.
            // Bounded on both sides: after the plate driver that writes those anchors, and before
            // the append lane, whose minimap producer fills the widget slot this pass parks.
            .add_systems(
                Update,
                extract::paint_script
                    .in_set(UiPaint)
                    .after(crate::vplates::VPlateSet)
                    .before(crate::ui_pass::UiQuadAppend),
            )
            // Combine the two pointer contributions (dev overlay + player UI) into the single
            // `PointerOverUi` source of truth, after the hover is known and before gameplay reads it.
            .add_systems(
                Update,
                arbitrate_pointer_over_ui
                    .after(UiInput)
                    .before(WorldStage::Input),
            );

        // `WOW_CAPTURE_UI=1` on a capture: override the "player" token with a synthetic snapshot so
        // the unit frames are populated in a server-less capture. Ordered after the real feed (so it
        // wins) and before the VM ticks/dispatches this frame. Never active outside capture mode —
        // synthetic data must not reach a real run.
        app.add_systems(
            Update,
            demo_unit_feed
                .in_set(UiFeed)
                .after(UnitFeed)
                .run_if(capture_ui_active),
        );
    }
}

/// Should this CAPTURE include the player UI (+ the synthetic unit snapshot)? The visual harness's
/// baselines must stay UI-free (they regression-test the WORLD render), so captures skip the UI
/// unless it is opted in — [`crate::run_mode::capture_ui_opted_in`], the SAME predicate the UI load and the
/// window sizing use, because the three disagreeing is how `ui-unitframes` ends up photographing
/// empty frames. Normal runs always load the UI and never take synthetic data.
fn capture_ui_active(capture: Option<Res<crate::run_mode::CaptureMode>>) -> bool {
    capture.is_some() && crate::run_mode::capture_ui_opted_in()
}

/// The pointer arbiter (decision 0026): `PointerOverUi = egui dev overlay ∨ player-UI hover`. Runs
/// regardless of whether the UI VM exists (if it's absent, [`PlayerUiHover`] stays `None` and this is
/// just the egui bit) — and regardless of the DEV overlays existing (their half is `Option`, so a
/// player build without the debug-panel plugin arbitrates on the player UI alone).
fn arbitrate_pointer_over_ui(
    egui: Option<Res<EguiPointerOver>>,
    hover: Res<PlayerUiHover>,
    plate_hover: Res<crate::vplates::PlateHover>,
    mut over: ResMut<PointerOverUi>,
    mut panel: ResMut<PointerOverUiPanel>,
) {
    // A cinematic used to need a third term here, and no longer does — the deletion is the point
    // of decision 1734. `CinematicFrame` is `setAllPoints` + `enableMouse="true"`, so while it is
    // up it is the only mouse target the hit test can reach, because `UIParent:Hide()` has taken
    // every other frame out of it. The hit test now arrives at that on its own, so
    // [`PlayerUiHover`] carries it and the special case is gone.
    over.0 = egui.is_some_and(|e| e.0) || hover.0.is_some();
    // A nameplate is mouse-enabled UI and it is not CHROME — see [`PointerOverUiPanel`]. The plate
    // hover is last frame's (the plate driver runs after the camera), which is the same vintage the
    // camera's own plate exception has always used.
    panel.0 = over.0 && plate_hover.0.is_none();
}

/// The session lifecycle — the VM's birth, identity, death, and the reload. Every edge function
/// lives there; this module keeps the per-frame bridge. See its header.
mod lifecycle;
pub(crate) use lifecycle::{
    end_ui_session, ingame_ui_up, run_pending_reload, setup_script, AddOnIdentity,
    LeavingWorldArmed, PendingEntryUiLoad, ReloadUiPending,
};
// Consumed only from other modules' test code (the emote-table checks, the harness's UI-init
// tail, the quit-once pin) — a plain re-export would warn unused in a non-test build.
#[cfg(test)]
pub(crate) use lifecycle::load_ingame_ui_on_world_entry;
use lifecycle::shutdown_on_exit;
#[cfg(test)]
pub(crate) use lifecycle::{
    finish_ui_load, is_emote_token_line, seat_from_roster, shutdown_ui_state,
};

/// `GetFramerate()`'s host half: push a **smoothed** frames-per-second into the VM each frame
/// (decision 1195).
///
/// Smoothed, not `1.0 / delta`, for the reason the reference smooths it too: 71 corpus addons put
/// this number on a panel, and a raw per-frame reciprocal reads as a flickering three-digit mess
/// that nobody can take a value off. The pole is a one-second time constant — fast enough that a
/// real stall is visible within a frame or two, slow enough to read.
///
/// `Time<Real>` deliberately, the same choice `tick_script` documents: this is a *frame rate*, and
/// a virtual clock that clamps its delta would report a rate the machine is not achieving.
fn feed_framerate(
    script: Option<NonSendMut<UiScript>>,
    time: Res<Time<bevy::time::Real>>,
    mut smoothed: Local<f64>,
) {
    let Some(mut script) = script else {
        return;
    };
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return; // the first frame, and any paused one — nothing to average in
    }
    let instant = 1.0 / dt;
    // One-pole IIR with a 1 s time constant, frame-rate independent.
    let alpha = (dt / 1.0).min(1.0);
    *smoothed = if *smoothed <= 0.0 {
        instant
    } else {
        *smoothed + alpha * (instant - *smoothed)
    };
    script.set_framerate(*smoothed);
}

/// Feed synthetic `"player"`/`"target"` snapshots each frame (overriding the real feed, which finds
/// no avatar in a server-less capture) and fire the initial events once — proving the full chain
/// end-to-end on a screenshot: snapshot → `Unit*` bindings → Lua `OnEvent` → bars + text, for both
/// unit frames (the target one included, so captures regression-test its show-on-target path).
fn demo_unit_feed(script: Option<NonSendMut<UiScript>>, mut fired: Local<VmMemo<bool>>) {
    /// The synthetic target's guid — a creature-family high part, so nothing mistakes it for a
    /// player. Only its *distinctness* matters.
    const DEMO_TARGET_GUID: u64 = 0xF130_0000_0000_0001;

    let Some(mut script) = script else {
        return;
    };
    // Session-keyed like every other seed (1290): the one-shot below is `PLAYER_ENTERING_WORLD`
    // and the bar seeds, which a fresh VM needs again.
    let fired = fired.get(&script);
    // TEMP DEBUG (WOW_CAPTURE_REHOVER=1): re-fire the world hover EVERY frame — the live
    // flapping-raycast simulation for the under-sized-plate investigation.
    if std::env::var("WOW_CAPTURE_REHOVER").as_deref() == Ok("1") {
        script.world_tooltip_unit("mouseover");
    }
    script.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Benilla".into()),
            health: 72,
            max_health: 100,
            level: 12,
            power_type: 0, // mana
            power: 45,
            max_power: 80,
            dead: false,
            reaction: 0, // own avatar — no reaction to itself
            // Race/class so the ui-char capture's level line reads "Level 12 Night Elf Warrior".
            race: Some("Night Elf".into()),
            race_file: Some("NightElf".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            sex: 2,
            is_player: true,
            player_controlled: true,
            ..Default::default()
        }),
    );
    script.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Young Wolf".into()),
            health: 30,
            max_health: 50,
            level: 3,
            // A powerless beast (maxPower 0): exercises the power-bar-hide path in the capture.
            power_type: 0,
            power: 0,
            max_power: 0,
            dead: false,
            reaction: 4, // neutral → the name plate reads yellow (UnitReactionColor[4])
            // A beast: no race/class tokens (UnitRace/UnitClass report the absent shape).
            race: None,
            race_file: None,
            class: None,
            class_file: None,
            sex: 0,
            // A beast type word so the mouseover/target tooltip's level line reads
            // "Level 3 Beast" in captures.
            creature_type_name: Some("Beast".into()),
            // A real guid: the combo seed below banks its points against exactly this unit, and
            // `GetComboPoints` refuses to report points banked on anything but the CURRENT target
            // (decision 0875). Also stops the demo player and target reading as the same unit,
            // which two default zeros would.
            guid: DEMO_TARGET_GUID,
            ..Default::default()
        }),
    );
    // The combo dots (`ComboFrame`) need a class that can SEE them and points banked on the
    // selected unit — seeded only for their own scenario, because the demo player is a warrior
    // everywhere else (ui-char's level line reads "Level 12 Night Elf Warrior" off that) and a
    // warrior authentically lights no dot. Run it with
    // `WOW_CAPTURE_UI=1 WOW_CAPTURE=ui-combopoints`.
    if std::env::var("WOW_CAPTURE").as_deref() == Ok("ui-combopoints") {
        script.set_player_req_state(benilla_ui::script::PlayerReqState {
            level: 12,
            class_id: 4, // rogue
            ..Default::default()
        });
        script.set_combo_points(4, DEMO_TARGET_GUID);
        script.fire_event("PLAYER_COMBO_POINTS", vec![]);
    }
    if !*fired {
        // A few synthetic bar slots so captures show the action bar populated (battle-stance
        // page: actions 73.. — the page a real warrior login lands on). Spread across the 12 wells
        // (buttons 1,2,3,8,12) so the ui-actionbar capture shows icons seated left-to-right. Slot
        // 80 is an ITEM-kind action (decision 0216 §7) with a synthetic count of 5, so the capture
        // also proves the Count fontstring wires up (`GetActionCount` reads the engine's own
        // pushed count directly — no live server template needed in a capture). It must also seed
        // the `IsConsumableAction` GATE, or the count paints nothing: 0926 put a gate in front of
        // it that this seed never fed, so the capture quietly stopped showing the "5" it exists to
        // prove (found while fixing decision 1301).
        script.set_bonus_bar_offset(1);
        // One MACRO slot too (button 4): a GM-style macro that casts nothing, so the capture
        // shows the macro-name line under the icon and the icon full-colour — B340's shape
        // (decision 1636). The table is seeded here because a capture has no character identity
        // for `ui_macro::load_macros` to read a file for; nothing overwrites it.
        script.set_macros(benilla_ui::script::MacroState {
            account: vec![benilla_ui::script::MacroView {
                name: "spawn".into(),
                texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
                body: ".spawn 16032".into(),
                local_only: false,
            }],
            character: Vec::new(),
        });
        for (action, icon, kind, id, count) in [
            (
                73,
                "Interface\\Icons\\Ability_SteelMelee",
                0x00u8,
                100u32,
                0u32,
            ),
            (74, "Interface\\Icons\\Ability_Rogue_Ambush", 0x00, 101, 0),
            (
                75,
                "Interface\\Icons\\Ability_Warrior_BattleShout",
                0x00,
                102,
                0,
            ),
            (
                76,
                "Interface\\Icons\\Ability_Racial_Cannibalize",
                0x40,
                1,
                0,
            ),
            (80, "Interface\\Icons\\INV_Misc_Food_16", 0x80, 117, 5),
            (84, "Interface\\Icons\\Spell_Holy_SealOfMight", 0x00, 103, 0),
            // The always-on multibars (stock MultiActionBars.xml): BottomLeft = actions 61..72, BottomRight
            // = 49..60. A few occupied wells on each so the capture shows both rows seated
            // (empty multibar wells hide — the ref's own default — so without these the rows
            // would be invisible).
            (
                61,
                "Interface\\Icons\\Spell_Nature_Regenerate",
                0x00,
                104,
                0,
            ),
            (62, "Interface\\Icons\\Spell_Shadow_Curse", 0x00, 105, 0),
            (
                72,
                "Interface\\Icons\\Spell_Frost_FrostBolt02",
                0x00,
                106,
                0,
            ),
            (49, "Interface\\Icons\\Spell_Fire_FlameBolt", 0x00, 107, 0),
            (60, "Interface\\Icons\\Spell_Holy_Heal", 0x00, 108, 0),
        ] {
            script.set_action(
                action,
                Some(ActionSlot {
                    texture: Some(icon.into()),
                    kind,
                    action: id,
                    count,
                    // The seed's only ITEM slot is the food stack, which is consumable.
                    consumable: kind == 0x80,
                }),
            );
            // The state feed has nothing to feed server-less (no `PlayerActions`), and a slot
            // with no pushed state answers `IsUsableAction` nil — the 0.4 grey on every icon,
            // which the `ui-actionbar` baseline wore for as long as it existed. Stand in for it
            // as this feed stands in for the unit feed: a live bar's resting state is usable.
            script.set_action_state(
                action,
                Some(benilla_ui::script::ActionState {
                    usable: true,
                    ..Default::default()
                }),
            );
        }
        // The stance bar (stock BonusActionBarFrame.xml): the synthetic warrior's three stances, battle active —
        // matches the bonus offset 1 above (battle stance page). Defensive shows the not-castable
        // grey; berserker a running cooldown swipe.
        script.set_shapeshift_forms(vec![
            benilla_ui::script::ShapeshiftFormView {
                spell_id: 2457,
                texture: Some("Interface\\Icons\\Ability_Warrior_OffensiveStance".into()),
                name: "Battle Stance".into(),
                active: true,
                castable: true,
                cooldown: None,
            },
            benilla_ui::script::ShapeshiftFormView {
                spell_id: 71,
                texture: Some("Interface\\Icons\\Ability_Warrior_DefensiveStance".into()),
                name: "Defensive Stance".into(),
                active: false,
                castable: false,
                cooldown: None,
            },
            benilla_ui::script::ShapeshiftFormView {
                spell_id: 2458,
                texture: Some("Interface\\Icons\\Ability_Racial_Avatar".into()),
                name: "Berserker Stance".into(),
                active: false,
                castable: true,
                cooldown: Some((900, 1500, true)),
            },
        ]);
        script.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
        // XP partway into the level (70%) so the MainMenuBar XP bar renders a purple partial fill.
        // Set before PLAYER_ENTERING_WORLD so the bar's first Update reads it.
        script.set_player_xp(4200, 6000);
        script.fire_event("PLAYER_ENTERING_WORLD", vec![]);
        // The two bottom multibars are player options and ship OFF since 1500, and a capture runs
        // with no server behind it — so the login seed reads a zero toggle byte and neither bar
        // comes up. Raise them the way the Options rows do, or the 61../49.. wells seeded above
        // draw nowhere and `ui-actionbar` loses two rows it exists to show. This is the DEMO's
        // choice about what to photograph, not a default: `MultiActionBar_Update` is the same
        // function the row calls, so nothing here is a private door into the bars. Guarded because
        // this feed also runs with `WOW_CAPTURE_UI` unset, where no interface has been loaded.
        // `WOW_DEMO_BOTTOM_BARS=0` leaves them down — the shipped default a player logs in to
        // (1500), and the only state in which the stance SHELF draws at all: the manage pass
        // hides the shelf art whenever the bottom-left bar is up, so a capture of the shelf
        // (decision 2000's hairline lived on it) needs the bars where the player has them.
        if std::env::var("WOW_DEMO_BOTTOM_BARS").as_deref() != Ok("0") {
            let _ = script.run(
                "SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 \
                 if MultiActionBar_Update then MultiActionBar_Update() end",
            );
        }
        script.fire_event("PLAYER_XP_UPDATE", vec![]);
        for token in ["player", "target"] {
            script.fire_event("UNIT_HEALTH", vec![ScriptValue::Str(token.into())]);
        }
        script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
        *fired = true;
    }
}

/// A fixed-advance stand-in font engine for tests — every character `.0` wide, one line tall,
/// greedily wrapped. The *numbers* the real engine produces are pinned against the client's own
/// fonts by `ui_text::atlas::metrics_tests`; a fixture wants arithmetic a reader can do in their
/// head, and wants a measure to arrive **in the tick that asked** exactly as the app's does.
#[cfg(test)]
pub(crate) struct FixedWidthFont(pub(crate) f32);

#[cfg(test)]
impl benilla_ui::script::TextMeasure for FixedWidthFont {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.chars().count() as f32 * self.0;
        match req.wrap_width {
            Some(w) if w > 0.0 && natural > w => (w, 12.0 * (natural / w).ceil(), natural),
            _ => (natural, 12.0, natural),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_ui;

/// The chat loader's two login events (`0x498a60`; `ui_chat::settings`):
/// `UPDATE_CHAT_WINDOWS` once, then `UPDATE_CHAT_COLOR` for every registry entry. The reference's
/// `FloatingChatFrame_Update` docks, hides and colours the windows on the first and
/// `ChatFrame_OnEvent` fills `ChatTypeInfo` from the second, so a test VM that loads the
/// reference's chat files needs both before the windows look or route like a client's.
#[cfg(test)]
pub(crate) fn fire_chat_login(s: &mut benilla_ui::script::UiScript) {
    // `FCF_OnUpdate` reads `UIOptionsFrame:IsShown()` every frame — the reference's options
    // window, which OptionsFrame.xml aliases and a chat test VM does not load. A hidden stand-in
    // is what a closed options window answers.
    s.run("if not UIOptionsFrame then UIOptionsFrame = CreateFrame('Frame') UIOptionsFrame:Hide() end")
        .expect("the options stand-in");
    s.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    let renorm = |b: u8| f64::from(b as f32 * (1.0f32 / 255.0f32));
    for entry in s.chat_colors() {
        s.fire_event(
            "UPDATE_CHAT_COLOR",
            vec![
                benilla_ui::script::ScriptValue::Str(entry.name),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[0])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[1])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[2])),
            ],
        );
    }
}

/// The `benilla_formats::TokenContext::text` seam (decision 2045): resolve a `GlobalStrings` key
/// out of the VM and fill its `%d` holes through the one shared filler.
///
/// The split it implements is deliberate. `benilla-formats` walks the spell's columns and knows
/// which key and which numbers a `$d` or `$s` token wants; it has no business knowing about a
/// script VM or carrying a printf-family formatter, and it depends on neither crate that has one.
/// So it names the key, and this renders it.
pub(crate) fn token_text(
    script: &benilla_ui::script::UiScript,
) -> impl Fn(&str, &[i64]) -> Option<String> + '_ {
    |key: &str, args: &[i64]| {
        let template = benilla_ui::strings::global(script.lua(), key)?;
        let args: Vec<_> = args
            .iter()
            .map(|n| benilla_ui::strings::Arg::D(*n))
            .collect();
        Some(benilla_ui::strings::fill(&template, &args))
    }
}

/// [`test_ui::load_ui`] for a test module OUTSIDE `ui_script` — `ui_action::feed_tests` drives the
/// real `UIErrorsFrame` end to end and needs the same both-stores reader everything else uses.
#[cfg(test)]
pub(crate) fn load_ui_for_test(script: &benilla_ui::script::UiScript, entry: &str) -> usize {
    test_ui::load_ui(script, entry)
}

#[cfg(test)]
mod cinematic_tests;

#[cfg(test)]
mod cast_tests;

#[cfg(test)]
mod mirror_timer_tests;

#[cfg(test)]
mod combat_text_tests;

#[cfg(test)]
mod combo_frame_tests;

#[cfg(test)]
mod unit_frame_tests;

#[cfg(test)]
mod unit_popup_tests;

#[cfg(test)]
mod chat_resize_tests;

#[cfg(test)]
mod dropdown_tests;

#[cfg(test)]
mod action_bar_tests;

#[cfg(test)]
mod exp_bar_tests;

#[cfg(test)]
mod multibar_stance_tests;

#[cfg(test)]
mod pet_bar_tests;

#[cfg(test)]
mod pet_frame_tests;

#[cfg(test)]
mod pet_stable_tests;

#[cfg(test)]
mod tot_frame_tests;

#[cfg(test)]
mod micro_menu_tests;

#[cfg(test)]
mod perf_bar_tests;

#[cfg(test)]
mod panel_tests;

#[cfg(test)]
mod fade_tests;

#[cfg(test)]
mod merchant_tests;

#[cfg(test)]
mod money_frame_tests;

#[cfg(test)]
mod faux_scroll_tests;

/// The reference's shared widget kit (UIPanelTemplates.xml + OptionsFrameTemplates.xml), driven
/// through `CreateFrame`'s fourth argument the way an addon drives it — decision 1203's queue.
#[cfg(test)]
mod panel_template_tests;

/// The RETURN-SHAPE gate (decision 1842): `reference/1.12-shapes.tsv` against what this client
/// actually answers. `reference_surface` gates names; nothing gated shapes, and that gap produced
/// six decisions in two days.
#[cfg(test)]
mod shape_gate;

/// The event ARGUMENT-shape gate (decision 2140) — the same question one API over: the two event
/// gates in `reference_ui` compare names, and nothing compared what a fire site pushes.
#[cfg(test)]
mod event_shape_gate;

/// The VERB-FIRED event gate (decision 2251) — the third question on the same seam: 1883/1889
/// compare names and 2140 compares arguments; this asks WHO fires it, because a stock file that
/// calls a verb for its side effect of an event repaints nothing when the verb fires nothing.
#[cfg(test)]
mod verb_event_gate;

/// The reference's BasicControls.xml — TEXT/message/_ERRORMESSAGE and the ScriptErrors dialog,
/// none of which benilla itself calls: every test enters from Lua the way an addon does.
#[cfg(test)]
mod basic_controls_tests;

#[cfg(test)]
mod color_picker_tests;

/// `UIParent.xml`'s loose addon-facing helpers (`MouseIsOver` and kin) — the panel and ESC halves
/// of that file live in `panel_tests` / `escape_tests`.
#[cfg(test)]
mod uiparent_tests;

#[cfg(test)]
mod trainer_tests;

#[cfg(test)]
mod bank_tests;

#[cfg(test)]
mod taxi_tests;

#[cfg(test)]
mod loot_tests;

#[cfg(test)]
mod group_loot_tests;

#[cfg(test)]
mod chat_tests;

/// The chat bubble's `UIMenu` kit driven as a menu — the rows' label/shortcut anchoring
/// (decision 1996), kept apart from `chat_tests` because it is the kit under test, not the window.
#[cfg(test)]
mod ui_menu_tests;

/// The chat tab's options menu, end to end (decision 1589 / B246) — its own file because it needs
/// the whole dropdown + colour-picker stack under `ChatFrame.xml`, where `chat_tests` deliberately
/// runs on the window alone.
#[cfg(test)]
mod chat_options_tests;

#[cfg(test)]
mod bag_tests;
#[cfg(test)]
mod resolve_bench;

#[cfg(test)]
mod tooltip_anchor_tests;

#[cfg(test)]
mod tooltip_compare_tests;

/// `GameTooltipTemplate` as an ADDON sees it — the corpus's most-wanted template, driven through
/// `inherits=` and `CreateFrame` the way the 27 addons that name it do.
#[cfg(test)]
mod tooltip_template_tests;

#[cfg(test)]
mod escape_tests;

#[cfg(test)]
mod game_menu_tests;

#[cfg(test)]
mod macro_tests;

// `pub(crate)` for its `harness`/`on_page` alone: the bindings dispatch tests drive the real
// Keybindings page through the real input systems, and building the page twice would let the two
// copies drift.
#[cfg(test)]
pub(crate) mod keybindings_tests;
#[cfg(test)]
mod options_tests;

#[cfg(test)]
mod delete_item_tests;
#[cfg(test)]
mod instance_tests;

#[cfg(test)]
mod static_popup_tests;

#[cfg(test)]
mod binder_tests;

#[cfg(test)]
mod summon_tests;

#[cfg(test)]
mod talent_wipe_tests;

#[cfg(test)]
mod death_tests;

#[cfg(test)]
mod duel_tests;

#[cfg(test)]
mod enchant_confirm_tests;

/// Its FLAG twin (decision 1739): the whole-tree sweep for `toplevel`/mouse/`id` against the
/// reference, read off the loaded engine rather than off our XML.
#[cfg(test)]
mod frame_flag_gate;

#[cfg(test)]
mod friends_tests;

/// The four guild windows — the social window's third tab and its three satellites (decision
/// 1257). Its own module rather than more of `friends_tests` because it stands the whole guild
/// engine API in for in Lua before the XML loads, which that file must not.
#[cfg(test)]
mod guild_tests;

/// The GM help window and its ticket toast (decision 1673). Its own module because every test in
/// it pushes a `GMTicketCategory.dbc` catalog and drives the ticket wire's own event vocabulary,
/// which none of the neighbouring windows share.
#[cfg(test)]
mod help_frame_tests;

/// The two guild-charter windows — the registrar and the petition sheet (decision 1672). Its own
/// module for `guild_tests`' reason: it stands the charter engine API in for in Lua before the XML
/// loads, so what is under test is the window and not `script::petition`'s plumbing.
#[cfg(test)]
mod petition_tests;

/// The social window's fourth tab — the raid pane and its grid (decision 1549). Its own module for
/// `guild_tests`' reason: every test in it pushes a RAID roster first, which a file about the
/// friends list must not be in the business of.
#[cfg(test)]
mod raid_tests;

#[cfg(test)]
mod quest_share_tests;

#[cfg(test)]
mod quest_tests;

#[cfg(test)]
mod quest_timer_tests;

#[cfg(test)]
mod battlefield_tests;

/// The battle map — the reference's `Blizzard_BattlefieldMinimap` addon, demand-loaded the way
/// SHIFT-M loads it, over the overlay/POI/position/arrow verbs it shares with the world map.
#[cfg(test)]
mod battlefield_minimap_tests;

#[cfg(test)]
mod tutorial_tests;

#[cfg(test)]
mod durability_tests;
// `pub(crate)` for its `harness`/`push`/`row` helpers: `perf::hud`'s own test drives the readout
// through them rather than keeping a second copy of the XML-loading boilerplate.
#[cfg(test)]
pub(crate) mod world_state_tests;

#[cfg(test)]
mod screenshot_tests;

#[cfg(test)]
mod questlog_tests;

#[cfg(test)]
mod character_tests;

#[cfg(test)]
mod skills_frame_tests;

#[cfg(test)]
mod reputation_frame_tests;

#[cfg(test)]
mod honor_frame_tests;

#[cfg(test)]
mod pet_paperdoll_tests;

#[cfg(test)]
mod inspect_tests;

#[cfg(test)]
mod dressup_tests;

#[cfg(test)]
mod minimap_tests;

#[cfg(test)]
mod spellbook_tests;

#[cfg(test)]
mod buff_tests;

#[cfg(test)]
mod target_aura_tests;

#[cfg(test)]
mod zone_text_tests;

#[cfg(test)]
mod errors_tests;

#[cfg(test)]
mod shipped_xml_tests;

#[cfg(test)]
mod bottom_hud_tests;

#[cfg(test)]
mod bagnon_render_tests;

/// Bug B267 end to end: a hunter's Quiver publishes its global functions (see the file header for
/// the three walls that stopped it).
#[cfg(test)]
mod quiver_tests;

/// The UI's per-world-entry lifecycle: teardown at the character screen, a genuine second load at
/// the next login (decision 1290).
#[cfg(test)]
mod world_entry_tests;

/// The guild tabard designer — the stock `TabardFrame.xml` off the chain (decision 1977).
#[cfg(test)]
mod tabard_tests;
/// The world map's POI pool — the guard's directions marker today, the AreaPOI landmarks later.
#[cfg(test)]
mod world_map_tests;

#[cfg(test)]
mod seam_scale_tests {
    use super::seam_scale;

    #[test]
    fn seam_scale_is_the_768_base_times_the_dial() {
        // The 0582 base: identity at the design height, proportional elsewhere.
        assert_eq!(seam_scale(768.0, 1.0), 1.0);
        assert_eq!(seam_scale(1536.0, 1.0), 2.0);
        // The 0584 dial multiplies it.
        assert_eq!(seam_scale(768.0, 0.9), 0.9);
        // The reference's pixel-perfect setting: uiScale = 768/screenH → 1 px per UI unit.
        assert!((seam_scale(1080.0, 768.0 / 1080.0) - 1.0).abs() < 1e-6);
        // A degenerate (pre-winit) window is identity, never a division blow-up.
        assert_eq!(seam_scale(0.0, 0.9), 1.0);
    }
}

#[cfg(test)]
mod pointer_arbiter_tests {
    use super::*;

    /// A world it can run the arbiter in: the three inputs and the two outputs, nothing else.
    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<PlayerUiHover>()
            .init_resource::<crate::vplates::PlateHover>()
            .init_resource::<PointerOverUi>()
            .init_resource::<PointerOverUiPanel>()
            .add_systems(Update, arbitrate_pointer_over_ui);
        app
    }

    /// **A plate is UI, and it is not chrome.** The camera's world-mouse latch and the wheel's zoom
    /// binding both read the second bit; both were dead over a plate until they did (2168).
    #[test]
    fn a_hovered_plate_is_ui_but_not_chrome() {
        let mut app = app();
        let plate = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.world_mut()
            .resource_mut::<crate::vplates::PlateHover>()
            .0 = Some(plate);
        app.update();
        assert!(
            app.world().resource::<PointerOverUi>().0,
            "the plate takes the pointer"
        );
        assert!(
            !app.world().resource::<PointerOverUiPanel>().0,
            "…and the WHEEL looks straight through it"
        );
    }

    /// …and what each of the two bits is now FOR, which is the half 2233 moved.
    ///
    /// The camera reads the raw flag: a press landing on a plate is the plate's, because
    /// `0x7662c0` delivers a mouse-down to exactly one frame and stops the bus walk, so the
    /// binding that starts mouselook is never reached. The wheel reads the chrome flag: it is the
    /// one genuine fall-through in the frame system and walks **past** a frame that merely takes
    /// the mouse, so scroll-zoom still works with the cursor on a plate. Two different reference
    /// laws, which is why there are two bits and not one — and 2159 had the camera on the wrong
    /// one for two days.
    #[test]
    fn the_camera_yields_to_a_plate_and_the_wheel_does_not() {
        let mut app = app();
        let plate = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.world_mut()
            .resource_mut::<crate::vplates::PlateHover>()
            .0 = Some(plate);
        app.update();
        // `latch_world_mouse` reads this one; `world_press` is `!over_ui`, so the press never
        // becomes the world's and no look session starts.
        assert!(
            app.world().resource::<PointerOverUi>().0,
            "the camera must yield the press to the plate"
        );
        // …and `bindings`' wheel branch reads this one.
        assert!(
            !app.world().resource::<PointerOverUiPanel>().0,
            "the wheel must still reach the world over a plate"
        );
    }

    /// An ordinary frame is both, which is the whole point of keeping two bits rather than one.
    #[test]
    fn a_hovered_panel_is_both() {
        let mut app = app();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.update();
        assert!(app.world().resource::<PointerOverUi>().0);
        assert!(app.world().resource::<PointerOverUiPanel>().0);
    }

    /// And empty world under the pointer is neither.
    #[test]
    fn no_hover_is_neither() {
        let mut app = app();
        app.update();
        assert!(!app.world().resource::<PointerOverUi>().0);
        assert!(!app.world().resource::<PointerOverUiPanel>().0);
    }
}
