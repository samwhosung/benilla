//! The UI-engine bridge: hosts [`benilla_ui::script::UiScript`] and feeds its
//! [`extract`](benilla_ui::script::UiScript::extract) output into the quad pass
//! ([`crate::ui_pass::UiQuads`]) every frame. The script side is WoW UI space (y-up, origin
//! bottom-left, a screen root 768 units tall at scale 1); [`seam_scale`] carries quads ×s out and
//! the mouse ÷s in, and extraction flips to y-down window px. The UI scale is `UIParent`'s own
//! frame scale inside the VM ([`UiScaleCvar`]), never part of the seam.

use bevy::prelude::*;

use benilla_ui::script::{ActionSlot, ScriptValue, UnitState};

use crate::ui_unit::UnitFeed;
use benilla_world::schedule::WorldStage;

/// The addon folder: discovery, manifests, the enable file and the load walk.
pub(crate) mod addons;
mod content;
pub(crate) mod extract;
/// The stock glue screens as the session and the instruments see them.
pub(crate) mod glue;
/// The stock create screen's catalog off the host's DBC catalogs.
pub(crate) mod glue_create;
/// `CGlueMgr`'s screen states over benilla's login, for the stock glue screens.
pub(crate) mod glue_mgr;
pub(crate) mod glue_scene;
mod input;
mod movie;
pub(crate) mod raise_origin;
mod records;
/// The server alert's fetch and `LaunchURL`'s browser, for the stock glue screens.
pub(crate) mod server_alert;
#[cfg(test)]
pub(crate) use input::dispatch_input_records;
#[cfg(test)]
pub(crate) use input::feed_keys;
#[cfg(test)]
pub(crate) use records::records_from_planes;
pub(crate) use records::{collect_input_records, InputRecord, InputRecords, Record};
#[cfg(test)]
pub(crate) mod layer_tests;
mod load_log;
mod manifest;

/// The stock FrameXML this client runs off the player's own patch chain; its header is the rule.
mod reference_ui;

/// What the host remembers per VM, so a seed or change-memo never outlives the VM it was for.
mod session;

/// The feed gate: a UI feed's input-side early-out, audited by `WOW_FEED_GATE_CHECK=1`.
pub(crate) mod gate;

pub(crate) use session::{VmMemo, WindowMemo};

// Not test-only: the addon harness loads the whole shipped interface under each addon.
pub(crate) use manifest::default_bindings;
pub(crate) use manifest::load_default_ui;
pub(crate) use manifest::load_ingame_ui;
#[cfg(test)]
pub(crate) use manifest::{load_stock_bindings, stock_bindings_file};
pub(crate) use reference_ui::{boot_client_locale, client_locale, client_region};

/// Whether the pointer is over any UI (the egui dev overlay or a player-UI frame), combined by
/// [`arbitrate_pointer_over_ui`]; gameplay reads it, so it is not the dev plugin's.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUi(pub(crate) bool);

/// A synthetic pointer owns the mouse this frame (the drag probe's gesture), so
/// [`input::feed_ui_input`] skips its mouse half and never feeds the real cursor.
#[derive(Resource, Default)]
pub(crate) struct SyntheticPointer(pub(crate) bool);

/// A capture never reads the OS pointer: set for a whole `$WOW_CAPTURE`/`$WOW_CAPTURE_UI` run, so
/// its pixels never depend on where the real cursor rests.
#[derive(Resource, Default)]
pub(crate) struct CapturePointerPinned(pub(crate) bool);

/// The egui dev overlay's half of the pointer arbitration, written by `track_pointer_over_ui`;
/// defined here so a build without dev overlays still has the type.
#[derive(Resource, Default)]
pub(crate) struct EguiPointerOver(pub(crate) bool);

/// Whether a dev overlay's text field has the keyboard: the UI and the bindings get no keys, as
/// when an EditBox has focus.
#[derive(Resource, Default)]
pub(crate) struct EguiKeyboard(pub(crate) bool);

/// Whether the dev `I` inspector's world picking is armed; `player::control` and `target::click`
/// read it every frame, so it lives here, and the default (disarmed) is the player's.
#[derive(Resource, Default)]
pub(crate) struct InspectMode {
    pub(crate) enabled: bool,
}

/// One frame's UI-pass phase split in μs, as the `[ui-cost]` line prints it; owned by the
/// producer so it exists whether or not its reader, `hover_log`, is compiled in.
#[derive(Resource, Default, Clone)]
pub(crate) struct UiFrameCost {
    /// FontStrings the layout had the font engine shape this frame, and the first few by name.
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
    /// Full layout-graph derivations this frame, each about 30× a solve; expected zero.
    pub(crate) derives: u64,
    pub(crate) skipped: bool,
    /// Entries the splice re-converted this frame; 0 on a settled or full-conversion frame.
    pub(crate) spliced: usize,
    /// Entries the splice dropped (drawn last frame, not now); `[ui-cost] dropped=` prints it.
    pub(crate) dropped: usize,
}

/// Whether [`UiFrameCost`] is wanted this run: `WOW_UI_COST=1` or the hover recorder.
#[derive(Resource, Default)]
pub(crate) struct UiCostWanted(pub(crate) bool);

/// The mouse-enabled player-UI frame under the cursor; world pick and camera look yield to it.
#[derive(Resource, Default)]
pub(crate) struct PlayerUiHover(pub(crate) Option<u32>);

/// Who owns this frame's keys (the client's `DAT_00cf4dc8 != 0` gate), written in [`UiInput`] and
/// read after it: a focused EditBox takes every key, a shown keyboard-enabled frame only the key
/// it ate. Neither releases a held key: only the window deactivate (`0x514490`, from `0x493058`)
/// and the world-enter cascade (`0x5144c0`) clear the direction bits, so W keeps running while
/// you type.
#[derive(Resource, Default)]
pub(crate) struct UiKeyboardCapture {
    /// A focused EditBox eats every key (`0x77b35e` returns 1 on every path but alt-arrow).
    pub(crate) typing: bool,
    /// Keys a shown keyboard-enabled frame consumed this frame (the existence gate, `0x76b7d0`),
    /// per key so the map eating `M` suppresses no other binding; raw codes, as read.
    pub(crate) consumed: Vec<bevy::input::keyboard::KeyCode>,
    /// The arrows fall through: the focused box is in alt-arrow mode and ALT is up, so the
    /// reference declines them at `0x77b1c4` and `CGWorldFrame` runs their bindings.
    pub(crate) arrows_fall_through: bool,
}

/// A left press the UI consumed without hitting a frame (a payload dropped on empty world), which
/// world click-pick and orbit-start yield to. Over a world object the reference keeps an item
/// payload and runs SELECT instead.
#[derive(Resource, Default)]
pub(crate) struct PlayerUiClickConsumed(pub(crate) bool);

/// A press the UI consumed this frame, whatever is under the pointer: the layout drag's press
/// returns at `0x766353`–`0x76635b`, and a right press with a payload held at the world input
/// hook (`0x492b50`, `0x766370`), each before any frame, the world frame included, sees it, so
/// the world latch takes no press this frame even with no UI frame hovered.
#[derive(Resource, Default)]
pub(crate) struct PlayerUiPressConsumed(pub(crate) bool);

/// Who this frame's button presses went to, as the records dispatched them: `(button, over_ui)`
/// per press, `over_ui` when a frame was hovered, the interface consumed it or the dev overlay held
/// the pointer. The world frame's binding and the world's mouse latch both read it, so they agree
/// on who owns a press; a button no record pressed this frame is absent.
#[derive(Resource, Default)]
pub(crate) struct WorldPresses(pub(crate) Vec<(MouseButton, bool)>);

impl WorldPresses {
    /// Whether the frame's last press of `button` was the interface's, `None` with no press.
    pub(crate) fn over_ui(&self, button: MouseButton) -> Option<bool> {
        self.0
            .iter()
            .rev()
            .find(|(b, _)| *b == button)
            .map(|(_, over)| *over)
    }
}

/// What [`extract::tick_script`] hands [`extract::paint_script`]; with `live` false (no VM or
/// window) the paint does nothing.
#[derive(Resource, Default)]
pub(crate) struct UiPassState {
    pub(crate) live: bool,
    /// The 768-virtual seam scale this frame was ticked under.
    pub(crate) seam: f32,
    pub(crate) dpi: f32,
    /// The meter's first three phases, so one `[ui-cost]` row still describes one frame.
    pub(crate) us_tick: u128,
    pub(crate) us_resolve: u128,
    pub(crate) us_measure: u128,
    /// The layout counters before the tick, so the row's `solves` and `derives` cover both halves.
    pub(crate) solves_before: u64,
    pub(crate) derives_before: u64,
}

/// The player-UI feed phase, after the input pass and [`WorldStage::Net`] and ungated (`UnitFeed`
/// is the gated sub-phase): every push this frame's tick must see, as in the reference's tick,
/// where the packets (category 6, `0x420d55`) follow the window input and precede the frame pass's
/// `OnUpdate`. Every VM holder in `Update` is in the frame's input records ([`UiInput`],
/// [`crate::player::WorldMouse`]), in this set or
/// `.after(UiTick)`, as `game_plugins::schedule_tests` checks.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiFeed;

/// The drains of what the pass's Lua asked for (a send, a window's verb, a save, a sound), after
/// the tick and after every later host they are free to follow: the reference applies each
/// inside its call, so an ask is drained the frame it is made, wherever in the pass its Lua ran.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiDrain;

/// The player-UI paint pass, after the camera so world-anchored widgets use this frame's view.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiPaint;

/// The player-UI input pass: the frame's window input records ([`InputRecords`]), each whole
/// through the interface's walk and the world frame's binding before the next, which the reference
/// runs ahead of its packets (`0x423920` at `0x420d23`), so ahead of the net drain
/// ([`WorldStage::Net`]) and [`UiFeed`]: a handler and the calls it makes read the world as the
/// last frame left it. First of the frame's input records, so its [`PlayerUiHover`] reaches
/// `PointerOverUi` before the world's mouse reads it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiInput;

/// The VM's views the input records read pushed ahead of them, at the head of [`UiInput`]: what an
/// engine writer outside the calls moved after the last frame's feeds (the targeting cursor's
/// word).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiInputViews;

/// The tick: the `OnUpdate` walk of the frame pass (category 0x11 at `0x420d6a`, `0x764330` →
/// `0x765650`), after the input pass and the feeds, so a handler reads this frame's input and
/// packets applied.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiTick;

/// The frame's atomic (`Instant`, `GetTime`) pair, the one base for mapping a store `Instant` onto
/// `GetTime`: `anchor`'s steps are the VM clock's deltas, so a mapped start is stable frame to
/// frame. Neither leg restarts with the VM: the reference's `GetTime` is `GetTickCount`, and stock
/// `Cooldown.lua:3` gates on `start > 0`, so a reset would hide every running cooldown's sweep.
#[derive(Resource)]
pub(crate) struct UiClock {
    /// [`FrameStep::at`] at the tick that produced [`Self::ui_now`].
    ///
    /// [`FrameStep::at`]: benilla_world::frame_pace::FrameStep::at
    pub(crate) anchor: std::time::Instant,
    /// The VM clock after that tick, in seconds since this process started.
    pub(crate) ui_now: f64,
}

/// [`UiClock`] as the call host reads it. The system that ticks the VM lends the host and writes
/// [`UiClock`], so the host reads this copy, made after each tick and at a new VM's seed: the last
/// tick's pair until the next, which maps an instant onto `GetTime` the same way, as both of its
/// legs move by the same steps.
#[derive(Resource, Default)]
pub(crate) struct HostClock(pub(crate) UiClock);

/// The copy after the tick ([`HostClock`]).
fn copy_host_clock(clock: Res<UiClock>, mut host: ResMut<HostClock>) {
    host.0 = UiClock {
        anchor: clock.anchor,
        ui_now: clock.ui_now,
    };
}

/// The pre-boot pair, replaced by [`lifecycle::seed_vm_clock`] at the first VM.
impl Default for UiClock {
    fn default() -> Self {
        Self {
            anchor: std::time::Instant::now(),
            ui_now: 0.0,
        }
    }
}

/// The FrameXML digest this process loads, which the corpus harness stamps on each report.
pub(crate) fn framexml_digest() -> String {
    content::digest()
}

/// Run a Lua chunk and log a failure: the form for any run whose `Result` is not consumed.
pub(crate) fn run_or_warn(script: &benilla_ui::script::UiScript, chunk: &str) {
    if let Err(e) = script.run(chunk) {
        warn!("ui_script: chunk failed: {e}");
    }
}

/// The reference's `uiScale` CVar, which the VM makes `UIParent`'s own scale
/// ([`benilla_ui::script::UiScript::set_ui_scale`]): the screen root stays 768 units tall, so
/// `uiScale = 768/screenH` is pixel-perfect inside `UIParent`, and a frame outside it is not
/// scaled. Tests pin the `Default` 1.0.
#[derive(Resource)]
pub(crate) struct UiScaleCvar(pub(crate) f32);

impl Default for UiScaleCvar {
    fn default() -> Self {
        Self(1.0)
    }
}

/// The shipped `uiScale`. Deviation: a flat 0.9, because 1.0 reads oversized. With `useUiScale`
/// off, its default (`0x8430c0`), the reference sets `max(768/H, 0.9)` above 768 px tall and 1.0
/// at or below (`0x492f70`, on a mode set and from `0x4908ad`), so the two agree from ~853 px up.
/// benilla never reads `useUiScale`: this CVar always applies to `UIParent` alone, as with it on.
/// A change goes through the CVar callback (`0x490770`: the narrow-aspect cap, the 0.64 floor);
/// the load and a display change through the display handler's ON leg (`0x492e90`: the cap, and
/// below 0.64 the automatic scale `0x492f70` instead of the floor).
pub(crate) const DEFAULT_UI_SCALE: f32 = 0.9;

/// The display the reference's automatic UI scale last ran for (`0x492f70`'s `[0xb4e304]` and
/// `[0xb4e308]`), which outlives every UI: the display handler skips a display it already ran for.
#[derive(Resource, Default)]
pub(crate) struct UiAutoScaleCache(pub(crate) Option<(u32, u32)>);

/// `WOW_UI_SCALE=` if set, clamped to the dial's range, else [`DEFAULT_UI_SCALE`].
fn default_ui_scale() -> f32 {
    std::env::var("WOW_UI_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.5, 1.5))
        .unwrap_or(DEFAULT_UI_SCALE)
}

/// The seam scale `s`, window px per screen-root unit (`windowH/768`: the root is 768 units tall at
/// every aspect, `0x41ad10`), used at every crossing of the VM boundary; identity for a degenerate
/// window (h ≤ 0, before winit). The UI scale is not in it: it is `UIParent`'s, inside the VM.
pub(crate) fn seam_scale(window_h: f32) -> f32 {
    if window_h > 0.0 {
        window_h / 768.0
    } else {
        1.0
    }
}

/// The interface's own screen on the window. Its cells take the aspect `0x4901e0` (in glue
/// `0x46ab40`) answers, the window's while `widescreen` is on and 4/3 when it is 0, through
/// `0x41ad10` (`0x48fe1e`, `0x46a81c`), and the layout always spans the whole window. So with
/// `widescreen` 0 the interface lays out 4:3, `4/3` of the window's height across, and is drawn
/// stretched across the window, as 1.12.1 does on any other shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UiScreen {
    /// The interface's screen in its own px: the window's height, and its width or `4/3` of it.
    pub(crate) size: Vec2,
    /// Window px per interface px across; 1 unless stretched.
    pub(crate) stretch: f32,
    /// The display aspect the cells and the automatic scale take (`0x4901e0`, stored at
    /// `0x48fe1e`): the window's `RN24(w/h)`, or exactly `RN24(4/3)` with `widescreen` 0, never
    /// derived from a rounded size.
    pub(crate) aspect: f32,
    /// The window in logical px, the display size the automatic scale is keyed on (`0x492f70`).
    pub(crate) window: Vec2,
}

impl UiScreen {
    /// The screen for a window of `window` logical px.
    pub(crate) fn of(window: Vec2, widescreen: bool) -> Self {
        if window.x <= 0.0 || window.y <= 0.0 {
            return Self {
                size: window,
                stretch: 1.0,
                aspect: 4.0 / 3.0,
                window,
            };
        }
        if widescreen {
            return Self {
                size: window,
                stretch: 1.0,
                aspect: window.x / window.y,
                window,
            };
        }
        let size = Vec2::new(window.y * (4.0 / 3.0), window.y);
        Self {
            size,
            stretch: window.x / size.x,
            aspect: 4.0 / 3.0,
            window,
        }
    }

    /// The display size in whole logical px, as the automatic scale takes it (`0x492f70`).
    pub(crate) fn display(self) -> (u32, u32) {
        (self.window.x.round() as u32, self.window.y.round() as u32)
    }

    /// The screen for `window` under the `widescreen` the CVar holds (on when absent).
    pub(crate) fn of_window(
        window: &bevy::window::Window,
        widescreen: Option<&crate::video::Widescreen>,
    ) -> Self {
        Self::of(
            Vec2::new(window.width(), window.height()),
            widescreen.is_none_or(|w| w.0),
        )
    }

    /// A window point (logical px) in the interface's px.
    pub(crate) fn point(self, window_px: Vec2) -> Vec2 {
        Vec2::new(window_px.x / self.stretch, window_px.y)
    }

    /// [`Self::point`]'s inverse: an interface point in window px.
    pub(crate) fn window_point(self, ui_px: Vec2) -> Vec2 {
        Vec2::new(ui_px.x * self.stretch, ui_px.y)
    }
}

/// [`window_to_ui`]'s inverse: a point in the VM's root units as a window cursor.
pub(crate) fn ui_to_window(screen: UiScreen, at: Vec2) -> Vec2 {
    let h = screen.size.y;
    let s = seam_scale(h);
    screen.window_point(Vec2::new(at.x * s, h - at.y * s))
}

/// A window cursor (logical px, y-down from the top left) in the VM's root units, y up: what the
/// pointer feed hands the engine and `GetCursorPosition` answers (`0x48b820`, no scale divided out).
pub(crate) fn window_to_ui(screen: UiScreen, cursor: Vec2) -> Vec2 {
    let h = screen.size.y;
    let s = seam_scale(h);
    let p = screen.point(cursor);
    Vec2::new(p.x / s, (h - p.y) / s)
}

/// The Lua UI host (a `NonSend` resource: an mlua VM is `!Send`) and its per-frame passes.
pub(crate) struct UiScriptPlugin;

/// `uiScale`'s change callback: the dial's own `[0.5, 1.5]` clamp.
pub(crate) fn on_cvar(
    ev: &crate::cvars::CvarChanged,
    mut scale: bevy::prelude::Mut<'_, UiScaleCvar>,
) {
    if ev.is("uiScale") {
        scale.0 = ev.num().clamp(0.5, 1.5);
    }
}

/// `/console reloadUI` (`0x4035f0`): a glue rebuild asked for whatever the state
/// ([`glue::run_pending_glue_rebuild`] spends it), then the same deferred rebuild `ReloadUI()` and
/// `/reload` queue, which the world alone honours. It prints nothing in any state: the handler
/// sets the flag `[0xb41d8c]` and calls `CGGameUI::Reload` (`0x491380`), neither of which writes
/// a line, so a run with no interface to reload is silent too.
fn console_reload_ui(steps: &mut crate::console::ConsoleSteps, _args: &str) -> Vec<String> {
    if let Some(rebuild) = steps.glue_rebuild.as_deref_mut() {
        rebuild.request();
    }
    // `CGGameUI::Reload`'s flag `0xb4b3f4`, which the session's `ReloadUI()` sets too.
    if let Some(reload) = steps.reload.as_deref_mut() {
        reload.0 = true;
    }
    Vec::new()
}

impl Plugin for UiScriptPlugin {
    fn build(&self, app: &mut App) {
        use crate::console::ConsoleCommandApp;
        crate::ui_script::raise_origin::arm_each_frame(app);
        // Category 1 with no help (`0x402ba6`-`0x402bb4`).
        app.console_command_in_call(
            "reloadUI",
            crate::console::GRAPHICS,
            None,
            console_reload_ui,
        );
        // The quit root runs in `Last`: the close button's `AppExit` is written in `PostUpdate`.
        crate::shutdown::on_app_exit(app, shutdown_on_exit.into_configs());
        app.insert_resource(UiScaleCvar(default_ui_scale()))
            .init_non_send_resource::<glue_mgr::SharedGlueMgr>()
            .init_resource::<UiAutoScaleCache>()
            .init_resource::<UiFrameCost>()
            .init_resource::<UiCostWanted>()
            .init_resource::<PointerOverUi>()
            .init_resource::<SyntheticPointer>()
            .insert_resource(CapturePointerPinned(
                std::env::var_os("WOW_CAPTURE").is_some()
                    || std::env::var_os("WOW_CAPTURE_UI").is_some(),
            ))
            .init_resource::<EguiPointerOver>()
            .init_resource::<EguiKeyboard>()
            .init_resource::<InspectMode>()
            .init_resource::<PlayerUiHover>()
            .init_resource::<UiKeyboardCapture>()
            .init_resource::<PlayerUiClickConsumed>()
            .init_resource::<PlayerUiPressConsumed>()
            .init_resource::<WorldPresses>()
            .init_resource::<UiClock>()
            .init_resource::<HostClock>()
            .init_resource::<benilla_world::frame_pace::FrameStep>()
            .init_resource::<AddOnIdentity>()
            // After `AssetSet::Open`: the VM's first load is the chain's `GlobalStrings.lua`.
            .add_systems(
                Startup,
                // After the CVars load: the glue screens read the agreements as they come up. After
                // the glyph cache, which measures the text their first show sizes frames by. After
                // the word lists, which every VM is seated with as it is made.
                setup_script
                    .after(benilla_assets::AssetSet::Open)
                    .after(crate::text_filter::TextFilterLoad)
                    .after(crate::cvars::CvarLoad)
                    .after(crate::ui_text::FontAtlasInit),
            )
            // The in-game UI loads on world entry, as the reference's does, once the loading cover
            // has presented, so its ~0.5 s never stalls the cover's first frame.
            .add_systems(
                OnEnter(crate::client_state::ClientState::InWorld),
                lifecycle::arm_entry_ui_load,
            )
            // The shutdown tail: events, then writes, in the reference's order; the clears of what
            // the dying state shows after its `PLAYER_LOGOUT`, then the glue VM.
            .configure_sets(
                OnExit(crate::client_state::ClientState::InWorld),
                (SessionEnd::Shutdown, SessionEnd::Clears, SessionEnd::Glue).chain(),
            )
            .add_systems(
                OnExit(crate::client_state::ClientState::InWorld),
                (
                    lifecycle::shut_down_session.in_set(SessionEnd::Shutdown),
                    lifecycle::install_session_glue.in_set(SessionEnd::Glue),
                ),
            )
            // `init_` here and in `UiUnitPlugin`: either plugin may be built alone in a test.
            .init_world_leave_sweep()
            .add_systems(Update, sweep::arm_leaving_world_on_self_create)
            // A queued `ReloadUI()` runs in `PreUpdate`, a frame after its drain (the reference's
            // deferral, `0x495590`) and before every `Update` seed or feed.
            .init_resource::<ReloadUiPending>()
            .init_resource::<glue::GlueRebuild>()
            // Both are exclusive edges on the VM and must not interleave.
            .add_systems(
                PreUpdate,
                (
                    run_pending_reload,
                    // The glue top's size callback, then the rebuild it or a `reloadUI` asked
                    // for. The callback reads the `gxResolution` the screen shows this frame.
                    glue::watch_glue_aspect
                        .after(crate::video::show_screen_size)
                        .run_if(glue::glue_ui_up),
                    glue::run_pending_glue_rebuild,
                    lifecycle::run_pending_entry_load,
                )
                    .chain(),
            )
            // The UI phases in the reference's tick order (`core/loop`, "What one tick runs"): the
            // window input records (`0x423920` at `0x420d23`) ahead of the net drain, then the
            // feeds of the packets it applied (category 6, `0x420d55`), then the tick, the frame
            // pass's `OnUpdate` walk (category 0x11, `0x420d6a`), and the paint; the drains of
            // what the Lua asked after the tick, and after every later host the VM pass can put
            // them behind, before the paint (`crate::vm_pass`).
            .configure_sets(
                Update,
                (
                    UiInput.before(WorldStage::Net),
                    UiFeed.after(WorldStage::Net),
                    UiTick.before(WorldStage::Input),
                    UiPaint,
                )
                    .chain(),
            )
            .configure_sets(Update, UiDrain.after(UiTick))
            .configure_sets(
                Update,
                UiInputViews
                    .in_set(UiInput)
                    .before(input::dispatch_input_records),
            )
            // `GetFramerate()`'s ring, a static of the client's that no reload or logout
            // resets: the feed hands a new VM its rate before the tick, and each world frame's
            // step joins the ring after its paint, standing for `CGWorldFrame::OnRender`
            // (`0x482ea0`), so an `OnUpdate` reads the frames before its own.
            .init_resource::<WorldFrameTimes>()
            .add_systems(Update, feed_framerate.in_set(UiFeed))
            .add_systems(
                Update,
                record_world_frame_time
                    .after(UiPaint)
                    .after(WorldStage::Present)
                    .run_if(in_state(crate::client_state::ClientState::InWorld)),
            )
            // The input pass hit-tests the rects the last frame's paint resolved, as the reference's
            // records meet the rects its last frame pass drained (`0x7656ba`, `0x765799`), and runs
            // in-world only or on the stock glue screens.
            .init_resource::<UiPassState>()
            // The frame's window input records, read first, ungated, so no frame dispatches a
            // stale one.
            .init_resource::<InputRecords>()
            .add_message::<bevy::window::WindowEvent>()
            .add_systems(
                Update,
                collect_input_records
                    .in_set(UiInput)
                    .before(UiInputViews)
                    .before(crate::bindings::BindingHead),
            )
            // In `Last`, after a look session's end has put the cursor back, so the next frame's
            // records start from where the pointer is.
            .add_systems(Last, records::note_frame_end_cursor)
            .init_resource::<input::RecordWalk>()
            .add_systems(
                Update,
                input::dispatch_input_records
                    .run_if(
                        in_state(crate::client_state::ClientState::InWorld).or(glue::glue_ui_up),
                    )
                    .in_set(UiInput)
                    // Exclusive, as its records' world steps run the click's legs: placed among
                    // the hosts by hand, so the pass chains it as it did the walk.
                    .in_set(crate::vm_pass::VmPass::Member)
                    .after(glue::skip_unattended_startup_movie)
                    // Each record's binding runs inside its walk, between the pass's head and tail.
                    .after(crate::bindings::BindingHead)
                    .before(crate::bindings::BindingTail),
            )
            .add_systems(
                Update,
                (
                    extract::tick_script,
                    copy_host_clock,
                    // After the input pass's hover walk and the pump's leaves and re-pick, for the
                    // pick after the pass.
                    crate::vplates::seed_plate_hover,
                )
                    .chain()
                    .in_set(UiTick),
            )
            // After the plate driver seats this frame's nameplates, and before the append lane
            // fills the widget slot this pass parks.
            // A scripted run's startup movie skipped before its first tick.
            .add_systems(Update, glue::skip_unattended_startup_movie.in_set(UiInput))
            // The movie audio's play times, a feed of the tick whose clock reads them.
            .add_systems(Update, movie::feed_movie_audio.in_set(UiFeed))
            // The movie pictures the tick showed, ahead of the paint that draws them.
            .init_resource::<movie::MovieScreen>()
            .add_systems(
                Update,
                // After the frame's window and present modes apply, so the aspect is the
                // window's this frame.
                movie::upload_movie_pictures
                    .in_set(UiPaint)
                    .after(crate::video::apply_present_mode)
                    .after(crate::video::apply_window_mode)
                    .before(extract::paint_script),
            )
            .add_systems(
                Update,
                extract::paint_script
                    .in_set(UiPaint)
                    .after(crate::vplates::VPlateSet)
                    .before(crate::ui_pass::UiQuadAppend),
            )
            // After the hover is known and before gameplay reads `PointerOverUi`: the world's
            // mouse among the input records, so a press the cursor brings onto a frame is the
            // frame's alone.
            .add_systems(
                Update,
                arbitrate_pointer_over_ui
                    .after(UiInput)
                    .before(crate::player::WorldMouse),
            )
            .add_message::<server_alert::LaunchUrl>()
            .add_message::<server_alert::LaunchAddonUrl>()
            // The stock glue screens' frame, ahead of the login engine, so the manager's asks reach
            // it in the frame whose Lua made them.
            .add_systems(
                Update,
                (
                    // After the tick whose Lua called its verbs.
                    glue_mgr::drive_glue_mgr.in_set(crate::ui_script::UiDrain),
                    // Off the roster and the screen the manager and the tick just moved.
                    glue_scene::feed_stock_glue_preview.in_set(crate::portrait::GluePreviewFeed),
                    // Each reads only its message, so no state orders it but the sets.
                    server_alert::open_launched.after(WorldStage::Stream),
                    server_alert::open_addon_launched.after(WorldStage::Stream),
                )
                    .chain()
                    .before(crate::glue_session::login::LoginPolicy)
                    .after(WorldStage::Net),
            );

        // A UI capture's synthetic unit snapshots, after the real feed so they win.
        app.add_systems(
            Update,
            demo_unit_feed
                .in_set(UiFeed)
                .after(UnitFeed)
                .run_if(capture_ui_active),
        );
    }
}

/// The mouse and key feed once, for a test over the whole client: the input records the test's
/// planes and key messages make, each walked and its world steps taken, with the call host the
/// world lends.
#[cfg(test)]
pub(crate) fn feed_input_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    records_from_planes(world);
    world
        .run_system_once(input::dispatch_records_alone)
        .expect("the input feeds");
}

/// The tick once, for a test over the whole client: the `OnUpdate` pump's Lua, with the call host
/// the world lends.
#[cfg(test)]
pub(crate) fn tick_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(extract::tick_script)
        .expect("the tick runs");
}

/// Whether a capture includes the player UI: world baselines stay UI-free unless
/// [`crate::run_mode::capture_ui_opted_in`], which the UI load and window size also read.
fn capture_ui_active(capture: Option<Res<crate::run_mode::CaptureMode>>) -> bool {
    capture.is_some() && crate::run_mode::capture_ui_opted_in()
}

/// `PointerOverUi = egui dev overlay ∨ player-UI hover`; the dev half is optional.
fn arbitrate_pointer_over_ui(
    egui: Option<Res<EguiPointerOver>>,
    hover: Res<PlayerUiHover>,
    mut over: ResMut<PointerOverUi>,
) {
    // No cinematic term: the hit test finds the full-screen, mouse-enabled `CinematicFrame`.
    over.0 = egui.is_some_and(|e| e.0) || hover.0.is_some();
}

/// The session lifecycle: the VM's birth, identity, death and reload.
mod lifecycle;
pub(crate) use lifecycle::{
    ingame_ui_up, run_pending_reload, setup_script, strings_vm, AddOnIdentity, PendingEntryUiLoad,
    ReloadUiPending, SessionEnd,
};
/// The leave-world sweep `0x490a80`: its latch, its schedule and its legs' order.
mod sweep;
#[cfg(test)]
pub(crate) use sweep::test_support as sweep_test;
#[cfg(test)]
pub(crate) use sweep::WorldLeaveSweep;
pub(crate) use sweep::{
    add_worldport_sweep, run_leave_world_sweep, LeavingWorldArmed, SweepApp, SweepLeg, SweptWorlds,
};
#[cfg(test)]
mod sweep_tests;
// Test-only: other modules' tests consume these, and a plain re-export would warn unused.
#[cfg(test)]
pub(crate) use lifecycle::load_ingame_ui_on_world_entry;
use lifecycle::shutdown_on_exit;
#[cfg(test)]
pub(crate) use lifecycle::{
    end_ui_session, finish_ui_load, is_emote_token_line, seat_from_roster, shutdown_ui_state,
};

/// The 30 world-frame times `GetFramerate()` averages (`0xc631c8`).
#[derive(Resource, Default)]
struct WorldFrameTimes(benilla_ui::script::FrameTimeRing);

/// `GetFramerate()`'s rate as the ring stands, for the tick about to run.
fn feed_framerate(mut hosted_vm: crate::script_calls::HostedVm, times: Res<WorldFrameTimes>) {
    hosted_vm.run(|script| {
        if let Some(script) = script {
            script.set_framerate(times.0.rate());
        }
    })
}

/// One world frame's step into the ring (`0x672700`), the elapsed every `OnUpdate` was handed;
/// [`feed_framerate`] hands the VM the rate before the next tick reads it.
fn record_world_frame_time(
    step: Res<benilla_world::frame_pace::FrameStep>,
    mut times: ResMut<WorldFrameTimes>,
) {
    times.0.push(step.delta.as_secs_f32());
}

/// The demo's stances, Battle, Defensive and Berserker, which a UI capture's known spells list
/// (`crate::capture`) and [`demo_unit_feed`] seeds the entries of.
pub(crate) const DEMO_STANCES: [u32; 3] = [2457, 71, 2458];

/// Synthetic player and target snapshots for a server-less UI capture, and its first events once.
pub(crate) fn demo_unit_feed(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut fired: Local<VmMemo<bool>>,
) {
    hosted_vm.run(|script| {
        /// The synthetic target's guid: a creature high part; only its distinctness matters.
        const DEMO_TARGET_GUID: u64 = 0xF130_0000_0000_0001;

        let Some(script) = script else {
            return;
        };
        // Session-keyed: a fresh VM needs the one-shot events and bar seeds again.
        let fired = fired.get(script);
        // Debug (`WOW_CAPTURE_REHOVER=1`): re-fire the world hover every frame, a flapping raycast.
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
                reaction: 0, // own avatar: no reaction to itself
                // Race/class so the ui-char capture's level line reads "Level 12 Night Elf
                // Warrior".
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
                reaction: 4, // neutral: yellow (UnitReactionColor[4])
                // A beast: no race/class tokens (UnitRace/UnitClass report the absent shape).
                race: None,
                race_file: None,
                class: None,
                class_file: None,
                sex: 0,
                // The tooltip's level line reads "Level 3 Beast".
                creature_type_name: Some("Beast".into()),
                guid: DEMO_TARGET_GUID,
                ..Default::default()
            }),
        );
        if !*fired {
            // Battle-stance page slots (actions 73..); slot 80 is a consumable stack of 5, since
            // stock `ActionButton_UpdateCount` shows only a consumable's count
            // (`ActionButton.lua:287`).
            script.set_bonus_bar_offset(1);
            // A macro on button 4; a capture has no character for `ui_macro::load_macros` to read.
            script.set_macros(benilla_ui::script::MacroState::from_scopes(
                vec![benilla_ui::script::MacroView {
                    name: "spawn".into(),
                    texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
                    body: ".spawn 16032".into(),
                    ..Default::default()
                }],
                Vec::new(),
            ));
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
                // BottomLeft is actions 61..72, BottomRight 49..60; an empty multibar well hides.
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
                        // The seed's only item slot is the food stack, which is consumable.
                        consumable: kind == 0x80,
                        use_spell: None,
                    }),
                );
                // Server-less a slot has no usable state; a live bar's resting state is usable.
                script.set_action_state(
                    action,
                    Some(benilla_ui::script::ActionState {
                        usable: true,
                        ..Default::default()
                    }),
                );
            }
            // The stance bar's entries, which its getters read with no player held, for the
            // [`DEMO_STANCES`] the known spells list: battle active (bonus offset 1), defensive
            // uncastable, berserker cooling.
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
            // 70% XP, set before the world-enter events so the bar's first update reads it.
            script.set_player_xp(4200, 6000);
            script.fire_world_enter();
            // The bottom multibars ship off and a capture has no toggle byte, so raise them as the
            // Options rows do; `WOW_DEMO_BOTTOM_BARS=0` leaves them down for the stance shelf art,
            // which `ShapeshiftBar_UpdatePosition` hides under the bottom-left bar.
            if std::env::var("WOW_DEMO_BOTTOM_BARS").as_deref() != Ok("0") {
                let _ = script.run(
                    "SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 \
                 if MultiActionBar_Update then MultiActionBar_Update() end",
                );
            }
            script.fire_event("PLAYER_XP_UPDATE", vec![ScriptValue::Str("player".into())]);
            for token in ["player", "target"] {
                script.fire_event("UNIT_HEALTH", vec![ScriptValue::Str(token.into())]);
            }
            script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
            *fired = true;
        }
    })
}

/// A test font engine: every character `.0` wide, 12 tall per line, greedily wrapped.
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

/// The chat loader's two login events (`0x498a60`): `UPDATE_CHAT_WINDOWS` once, then
/// `UPDATE_CHAT_COLOR` per registry entry.
#[cfg(test)]
pub(crate) fn fire_chat_login(s: &mut benilla_ui::script::UiScript) {
    // `FCF_OnUpdate` reads `UIOptionsFrame:IsShown()` every frame; stand in a closed one.
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

/// The `benilla_formats::TokenContext::printf` seam: `SStrPrintf` over the template filler.
pub(crate) fn token_printf(template: &str, args: &[benilla_formats::TokenNumber]) -> String {
    let args: Vec<_> = args
        .iter()
        .map(|n| match n {
            benilla_formats::TokenNumber::Int(v) => benilla_ui::strings::Arg::D(*v),
            benilla_formats::TokenNumber::Float(v) => benilla_ui::strings::Arg::F(*v),
        })
        .collect();
    benilla_ui::strings::fill(template, &args)
}

/// [`test_ui::load_ui`] for a test module outside `ui_script`, such as `ui_action::feed_tests`.
#[cfg(test)]
pub(crate) fn load_ui_for_test(script: &benilla_ui::script::UiScript, entry: &str) -> usize {
    test_ui::load_ui(script, entry)
}

#[cfg(test)]
mod cinematic_tests;
#[cfg(test)]
mod glue_input_tests;
#[cfg(test)]
mod key_walk_tests;
#[cfg(test)]
mod record_order_tests;
#[cfg(test)]
mod tick_order_tests;

/// Whether `Update` declares `system` before the tick, the frame pass's `OnUpdate` walk.
#[cfg(test)]
pub(crate) fn tick_runs_after<M>(
    app: &mut App,
    system: impl bevy::ecs::system::IntoSystem<(), (), M>,
) -> bool {
    crate::test_support::runs_before(app, system, extract::tick_script)
}

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
mod ui_scale_tests;

#[cfg(test)]
mod frame_step_tests;

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

/// The stock UIPanelTemplates and OptionsFrameTemplates kit, driven the way an addon drives it.
#[cfg(test)]
mod panel_template_tests;

/// The return-shape gate: `reference/bindings.tsv` against what this client answers.
#[cfg(test)]
mod shape_gate;

/// The surface gate: the production load's globals and widget methods against the 1.12 tables.
#[cfg(test)]
mod surface_gate;

/// The reader of every fire site, for the event gates.
#[cfg(test)]
pub(crate) mod fire_scan;

/// The event argument-shape gate: every fire site against `reference/event_args.tsv`.
#[cfg(test)]
mod event_shape_gate;

/// The verb-fired event gate, against `reference/verb_events.tsv`.
#[cfg(test)]
mod verb_event_gate;

/// The stock BasicControls.xml, which benilla never calls, entered from Lua as an addon does.
#[cfg(test)]
mod basic_controls_tests;

#[cfg(test)]
mod color_picker_tests;

/// `UIParent.xml`'s loose addon-facing helpers, such as `MouseIsOver`.
#[cfg(test)]
mod uiparent_tests;

#[cfg(test)]
mod trainer_tests;

#[cfg(test)]
mod auction_tests;

#[cfg(test)]
mod bank_tests;

#[cfg(test)]
mod taxi_tests;

#[cfg(test)]
mod loot_tests;

#[cfg(test)]
mod group_loot_tests;

#[cfg(test)]
pub(crate) mod chat_tests;

/// The chat bubble's `UIMenu` kit driven as a menu: the rows' label and shortcut anchoring.
#[cfg(test)]
mod ui_menu_tests;

/// The chat tab's options menu end to end, over the whole dropdown and colour-picker stack.
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

/// `GameTooltipTemplate` as an addon sees it, through `inherits=` and `CreateFrame`.
#[cfg(test)]
mod tooltip_template_tests;

#[cfg(test)]
mod escape_tests;

#[cfg(test)]
mod game_menu_addon_tests;
#[cfg(test)]
mod game_menu_tests;

#[cfg(test)]
mod macro_tests;

// `pub(crate)` for `harness`, `on_page` and `label`, which the bindings dispatch tests use.
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

/// The `toplevel`, mouse and `id` flags of the whole loaded tree against the reference.
#[cfg(test)]
mod frame_flag_gate;

#[cfg(test)]
mod friends_tests;

/// The four guild windows, over a Lua stand-in for the guild engine API.
#[cfg(test)]
mod guild_tests;

/// The GM help window and its ticket toast, over a pushed `GMTicketCategory.dbc` catalog.
#[cfg(test)]
mod help_frame_tests;

/// The guild-charter registrar and petition sheet, over a Lua stand-in for the charter API.
#[cfg(test)]
mod petition_tests;

/// The social window's fourth tab: the raid pane and its grid, over a pushed raid roster.
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

/// The battle map: the stock `Blizzard_BattlefieldMinimap` addon, demand-loaded as SHIFT-M does.
#[cfg(test)]
mod battlefield_minimap_tests;

#[cfg(test)]
mod tutorial_tests;

#[cfg(test)]
mod durability_tests;
// `pub(crate)` for `harness`, `push` and `row`, which `perf::hud`'s test uses.
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
mod protected_gate_tests;
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

/// The stock Video Options window off the chain: open, Okay, Defaults and Cancel.
#[cfg(test)]
mod video_options_tests;

#[cfg(test)]
mod bottom_hud_tests;

#[cfg(test)]
mod bagnon_render_tests;

/// A hunter's Quiver addon publishes its global functions, end to end.
#[cfg(test)]
mod quiver_tests;

/// Teardown at the character screen and a genuine second load at the next login.
#[cfg(test)]
mod world_entry_tests;

/// The guild tabard designer: the stock `TabardFrame.xml` off the chain.
#[cfg(test)]
mod tabard_tests;
/// The stock world map: its POI pool, unit blips, player arrow and full-screen quads.
#[cfg(test)]
mod world_map_tests;

#[cfg(test)]
mod seam_scale_tests {
    use super::{seam_scale, ui_to_window, window_to_ui, UiScreen};
    use bevy::math::Vec2;

    #[test]
    fn seam_scale_is_the_768_tall_root() {
        // Identity at the design height, proportional elsewhere.
        assert_eq!(seam_scale(768.0), 1.0);
        assert_eq!(seam_scale(1536.0), 2.0);
        // A degenerate (pre-winit) window is identity, never a division blow-up.
        assert_eq!(seam_scale(0.0), 1.0);
    }

    /// `GetCursorPosition` answers root units whatever the dial: the bottom of a 1080-tall window
    /// is 0 and its top 768, its middle column `768·a/2`.
    #[test]
    fn the_cursor_reaches_the_vm_in_root_units() {
        let screen = UiScreen::of(Vec2::new(1920.0, 1080.0), true);
        let at = window_to_ui(screen, Vec2::new(960.0, 108.0));
        assert!((at.x - 960.0 * 768.0 / 1080.0).abs() < 1e-3, "{at}");
        assert!((at.y - 972.0 * 768.0 / 1080.0).abs() < 1e-3, "{at}");
        assert_eq!(window_to_ui(screen, Vec2::new(0.0, 1080.0)), Vec2::ZERO);
        // With `widescreen` 0 the 4:3 root spans the window: its middle column is 512, its right
        // edge 1024.
        let stretched = UiScreen::of(Vec2::new(1920.0, 1080.0), false);
        let mid = window_to_ui(stretched, Vec2::new(960.0, 540.0));
        assert!((mid - Vec2::new(512.0, 384.0)).length() < 1e-3, "{mid}");
        let edge = window_to_ui(stretched, Vec2::new(1920.0, 0.0));
        assert!((edge - Vec2::new(1024.0, 768.0)).length() < 1e-3, "{edge}");
    }

    /// [`ui_to_window`] undoes [`window_to_ui`], stretched or not.
    #[test]
    fn a_root_point_goes_to_the_window_and_back() {
        for widescreen in [true, false] {
            let screen = UiScreen::of(Vec2::new(1920.0, 1080.0), widescreen);
            for p in [
                Vec2::ZERO,
                Vec2::new(512.0, 384.0),
                Vec2::new(1000.0, 700.0),
            ] {
                let back = window_to_ui(screen, ui_to_window(screen, p));
                assert!((back - p).length() < 1e-3, "{widescreen}: {p} -> {back}");
            }
        }
    }
}

#[cfg(test)]
mod pointer_arbiter_tests {
    use super::*;

    /// A world to run the arbiter in: its inputs and its output, nothing else.
    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<PlayerUiHover>()
            .init_resource::<PointerOverUi>()
            .add_systems(Update, arbitrate_pointer_over_ui);
        app
    }

    /// The camera reads this flag (`0x7662c0` gives a mouse-down to exactly one frame, a plate
    /// included); the wheel does not (`ui_script::input`'s sweep).
    #[test]
    fn a_hovered_frame_is_ui() {
        let mut app = app();
        app.world_mut().resource_mut::<PlayerUiHover>().0 = Some(7);
        app.update();
        assert!(app.world().resource::<PointerOverUi>().0);
    }

    #[test]
    fn no_hover_is_not() {
        let mut app = app();
        app.update();
        assert!(!app.world().resource::<PointerOverUi>().0);
    }
}
