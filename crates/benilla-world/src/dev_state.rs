//! The dev **state** — resource-only, no egui: the per-subsystem toggles the render and gameplay
//! systems read, with **player-faithful defaults** (fog on, dome on, everything visible, follow
//! the server clock, no weather override). Decision 0026's "config out of the editor": this file
//! is the always-present layer; the debug panel is only the *editor* that writes it. When the
//! compile-time `dev` seam lands (0026's deferred phase), this module stays in a player build —
//! the defaults ARE the player behaviour — and only the egui surfaces gate out.
//!
//! It was `debug_panel::state` until. Living inside the panel's module made the
//! panel look like the owner of config eight other subsystems read, and it was the second reason
//! `DebugPanelPlugin` had to be inside the engine's plugin group — the first being the model
//! `Visibility` authority, now `model_render`'s. With both out, the panel is an instrument again
//! and the engine boots without it, which is what `benilla-worldview` is for.

use bevy::prelude::*;

/// **Is this a deterministic capture run?** (`$WOW_CAPTURE` set.)
///
/// Six engine lanes ask it, and every one of them asks the same question: *freeze what varies with
/// wall time, so two runs of the same scene produce the same pixels.* Material animation stops
/// sampling, liquid ticks pin, the doodad-anim tier collapses, the pipeline warmer changes its
/// mind about what to warm. That is a property of **how the world is being run**, not of the
/// screenshot harness that happens to switch it on — which is why it lives here, in the
/// always-present config layer, beside the other environment-armed defaults, rather than in
/// `capture`. The harness above still owns the *scenario*; it just no longer owns the fact that
/// there is one.
///
/// Cheap by construction: an env read per call, the same shape the caller already paid for.
pub fn deterministic_run() -> bool {
    std::env::var("WOW_CAPTURE").is_ok()
}

/// How many frames each still-frame input has read as changed since startup — `[camera
/// transform, DebugState, ViewDistance, ExteriorWindows, CameraInteriorClaim, any
/// WmoPortalInstance, any InheritedVisibility]`. The still-frame skips (decision 1979: the
/// visibility walk, billboards, emitter gates, doodad hosts) engage only when all of these are
/// still; an input rewritten every frame silently disarms every one of them, and `FPS_PROBE`'s
/// `noisy=` prints these as per-window deltas so that input is named. The two population
/// terms are the visibility walk's own last two (`new_portals`, `owner_flips`): they were
/// added the day the walk's `portals` term read 300 of 300 on a parked city frame for a
/// latch written through `Mut` — a gate the five resource counters could not see.
pub static STILL_INPUTS_CHANGED: [std::sync::atomic::AtomicU32; 7] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];

/// `Last`: count this frame's changed still-frame inputs into [`STILL_INPUTS_CHANGED`].
pub(crate) fn count_still_inputs(
    cam: Query<Ref<GlobalTransform>, With<crate::view::WorldCamera>>,
    debug: Res<DebugState>,
    view: Res<crate::view::ViewDistance>,
    windows: Res<crate::wmo_portal::ExteriorWindows>,
    claim: Res<crate::wmo_portal::CameraInteriorClaim>,
    portals: Query<(), Changed<crate::wmo_portal::WmoPortalInstance>>,
    flips: Query<(), Changed<InheritedVisibility>>,
) {
    let changed = [
        cam.iter().any(|t| t.is_changed()),
        debug.is_changed(),
        view.is_changed(),
        windows.is_changed(),
        claim.is_changed(),
        !portals.is_empty(),
        !flips.is_empty(),
    ];
    for (slot, c) in STILL_INPUTS_CHANGED.iter().zip(changed) {
        if c {
            slot.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Root debug state. One resource, grouped into per-subsystem sections. Defaults: panel hidden
/// (`open: false`), each section its own `Default` (so this derives cleanly).
#[derive(Resource)]
pub struct DebugState {
    /// Panel visible? Hidden by default; toggled with the dev chord + `D`.
    ///
    /// `$WOW_PANEL=1` starts it **open**, which is how a headless capture run gets the panel into
    /// the frame. Without it a panel change (a new footer line, a section that grew past the
    /// scroll reserve) could only be checked in the director's own window, and clipping is exactly
    /// the failure a capture catches for free. Read here rather than by the panel's plugin,
    /// alongside this file's other environment-armed defaults.
    pub open: bool,
    pub models: ModelDebug,
    pub lighting: LightingDebug,
    pub sound: SoundDebug,
    pub weather: WeatherDebug,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            open: std::env::var_os("WOW_PANEL").is_some(),
            models: default(),
            lighting: default(),
            sound: default(),
            weather: default(),
        }
    }
}

/// Weather-instrument state: a panel-armed override that drives the same `WeatherState::apply`
/// path as the wire, so any type/grade transition can be exercised without a GM
/// `.wchange`. While `force` is on, wire weather is consumed and ignored.
#[derive(Default)]
pub struct WeatherDebug {
    /// Override armed: the scrub below substitutes for the wire.
    pub force: bool,
    /// One-shot: the scrub changed — re-apply it (set by the panel, taken by `weather_tick`).
    pub dirty: bool,
    /// Wire weather type (0 fine / 1 rain / 2 snow / 3 sand).
    pub kind: u32,
    /// Wire grade (0..1).
    pub grade: f32,
    /// Apply instantly (the wire's `instant` flag) instead of the ramp.
    pub instant: bool,
}

/// Sound-instrument state: the kit probe (the real play path — variation pick + per-shot
/// variation + category mix), consumed by `sound::kit`. The *config* (enable, master volume)
/// lives in [`crate::sound::SoundConfig`] — always-on player state per decision 0026; the panel
/// only edits it. (The pre-kit raw-path probes retired once the kit player became the one real
/// path — the kit probe exercises device + MPQ + decode end-to-end anyway.)
pub struct SoundDebug {
    /// A `SoundEntries` kit id or name for the "Play kit" probe.
    pub kit_query: String,
    /// One-shot: play `kit_query` through the kit player.
    pub play_kit: bool,
    /// How many copies of `kit_query` the probe fires **in one frame**.
    ///
    /// The reported defect is an overlap defect — "a lot of mobs attacking same time, or a priest
    /// buffing a group with mass fort" — and it needs no game state to reproduce, only N
    /// sample-aligned copies of one kit. Five copies of kit 3116 (`HolyProtection`) *is* mass
    /// Fortitude on a full party, arithmetically: the health line reports the peak the mix asked
    /// for, and `SoundOutputLimiter` A/Bs what it sounds like.
    pub play_copies: u32,
}

impl Default for SoundDebug {
    fn default() -> Self {
        Self {
            // A UI kit with several variations — exercises the depleting weighted pick.
            kit_query: "igMiniMapZoomIn".into(),
            play_kit: false,
            play_copies: 1,
        }
    }
}

/// Scene lighting controls. Colors come from `Light.dbc` sampled at the current time of day (the
/// **server** game-clock by default). The lighting section is
/// mostly a readout — the resolved DBC values + sun direction at the current time — plus the time
/// scrub and a fog-disable toggle. (The tone-gap discovery knobs were removed once that investigation
/// closed; the faithful path is the only path now.)
pub struct LightingDebug {
    /// Follow the live server game-clock (default). When `false`, scrub time with `manual_minute`.
    pub follow_server_time: bool,
    /// Manually-set minute of the game day (`0..1440`), used when `follow_server_time` is off.
    pub manual_minute: u32,
    /// Disable the `Light.dbc` distance fog on terrain + models (debug). The sky-dome horizon colour
    /// is unaffected (it's the sky band, not the distance fog). Default `false` = faithful fog on.
    pub disable_fog: bool,
    /// Hide the gradient sky dome (debug). Default `false` = dome shown (faithful). Useful for A/B and
    /// for the FPS-debug Performance toggles.
    pub disable_sky_dome: bool,
}

impl Default for LightingDebug {
    fn default() -> Self {
        // `WOW_CLOCK=<minute 0..1439>` arms the manual scrub from the environment — the
        // matched-hour capture instrument (a headless probe can't reach the panel slider, and a
        // time-of-day A/B against a reference screenshot is meaningless at the wrong hour —
        // bug B33's whole diagnosis hinged on one). Unset = follow the server clock, as before.
        let clock = std::env::var("WOW_CLOCK")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|m| m % 1440);
        Self {
            follow_server_time: clock.is_none(),
            manual_minute: clock.unwrap_or(720), // noon, only used when not following the server
            disable_fog: false,
            disable_sky_dome: false,
        }
    }
}

/// Toggles for the world-model render: per-layer (blend) and per-type visibility (a scene
/// inspector). Material values (alpha-key cutoff, two-sided culling, and the WoW lighting) are
/// baked in `model_material` / driven from the Lighting section.
pub struct ModelDebug {
    /// Visible flags indexed by [`crate::model_render::kind_index`].
    pub kind_visible: [bool; 4],
    /// Visible flags indexed by [`crate::model_render::blend_index`].
    pub blend_visible: [bool; 5],
    /// WMO portal visibility culling on/off. On = the faithful per-group PVS; off =
    /// every group of a building always drawn (the pre-portal behaviour). An A/B switch for the
    /// director: flip it off in the Trade District and the cathedral above reappears.
    pub portal_cull: bool,
}

impl Default for ModelDebug {
    fn default() -> Self {
        Self {
            kind_visible: [true; 4],
            blend_visible: [true; 5],
            // `WOW_NOPORTALCULL=1` presets the panel's A/B switch off, so a headless capture can
            // shoot the same viewpoint with and without the cull (the B65 diff loop).
            portal_cull: std::env::var("WOW_NOPORTALCULL").is_err(),
        }
    }
}
