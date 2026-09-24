//! The dev state: the per-subsystem toggles the render and gameplay systems read, at
//! player-faithful defaults. The debug panel only edits it, so it stays in a player build.

use bevy::prelude::*;

/// Whether this is a deterministic capture run (`$WOW_CAPTURE`): wall-time variation freezes.
pub fn deterministic_run() -> bool {
    std::env::var("WOW_CAPTURE").is_ok()
}

/// Frames each still-frame input has read as changed since startup: `[camera transform,
/// DebugState, ViewDistance, ExteriorWindows, CameraInteriorClaim, any WmoPortalInstance, any
/// InheritedVisibility]`. The still-frame skips (visibility walk, billboards, emitter gates,
/// doodad hosts) engage only while all are still; `FPS_PROBE`'s `noisy=` prints them per window.
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

/// Root debug state, one section per subsystem.
#[derive(Resource)]
pub struct DebugState {
    /// Panel visible; toggled with the dev chord + `D`. `$WOW_PANEL=1` starts it open, so a
    /// headless capture can shoot it.
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

/// Weather override through the wire's own `WeatherState::apply` path.
#[derive(Default)]
pub struct WeatherDebug {
    /// Override armed: the scrub below replaces the wire, whose weather is consumed and ignored.
    pub force: bool,
    /// One-shot: set by the panel when the scrub changes, taken by `weather_tick`.
    pub dirty: bool,
    /// Wire weather type (0 fine / 1 rain / 2 snow / 3 sand).
    pub kind: u32,
    /// Wire grade (0..1).
    pub grade: f32,
    /// Apply instantly (the wire's `instant` flag) instead of the ramp.
    pub instant: bool,
}

/// The sound kit probe, played through the real kit path (`sound::kit`); the sound settings are
/// `sound::SoundConfig`.
pub struct SoundDebug {
    /// A `SoundEntries` kit id or name for the "Play kit" probe.
    pub kit_query: String,
    /// One-shot: play `kit_query` through the kit player.
    pub play_kit: bool,
    /// Copies of `kit_query` fired in one frame, the overlap case (many attackers, a group buff):
    /// five of kit 3116 (`HolyProtection`) is a group Fortitude on a full party.
    pub play_copies: u32,
}

impl Default for SoundDebug {
    fn default() -> Self {
        Self {
            // A UI kit with several variations, to exercise the depleting weighted pick.
            kit_query: "igMiniMapZoomIn".into(),
            play_kit: false,
            play_copies: 1,
        }
    }
}

/// Scene lighting controls: `Light.dbc` is sampled at the server game clock unless scrubbed.
pub struct LightingDebug {
    /// Follow the live server game clock; when `false`, `manual_minute` sets the time.
    pub follow_server_time: bool,
    /// Minute of the game day (`0..1440`) while not following the server.
    pub manual_minute: u32,
    /// Disable the `Light.dbc` distance fog; the sky dome's horizon band is not fog and stays.
    pub disable_fog: bool,
    /// Hide the gradient sky dome.
    pub disable_sky_dome: bool,
}

impl Default for LightingDebug {
    fn default() -> Self {
        // `WOW_CLOCK=<minute 0..1439>` arms the manual scrub, to match a reference shot's hour.
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

/// World-model render toggles: visibility per model kind and per blend layer.
pub struct ModelDebug {
    /// Visible flags indexed by [`crate::model_render::kind_index`].
    pub kind_visible: [bool; 4],
    /// Visible flags indexed by [`crate::model_render::blend_index`].
    pub blend_visible: [bool; 5],
    /// WMO portal culling: on is the reference's per-group PVS, off draws every group.
    pub portal_cull: bool,
}

impl Default for ModelDebug {
    fn default() -> Self {
        Self {
            kind_visible: [true; 4],
            blend_visible: [true; 5],
            // `WOW_NOPORTALCULL=1` starts the cull off, for a headless A/B of one viewpoint.
            portal_cull: std::env::var("WOW_NOPORTALCULL").is_err(),
        }
    }
}
