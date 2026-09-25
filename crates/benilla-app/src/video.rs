//! Video settings that reach the window and the presentation path: 1.12's `gxWindow` and
//! `gxVSync` rows (`OptionsFrame.lua:14`, `:9`) and the `gxResolution` windowed size.
//!
//! Deviation: `gxWindow "0"` is borderless fullscreen, not 1.12's exclusive mode-set at
//! `gxResolution`, because Wayland has no client mode-setting (winit ignores
//! `Fullscreen::Exclusive` there), X11's XRandR changes the desktop's mode and a crash leaves it
//! changed, and macOS has no exclusive mode. Bevy's `WindowMode::Fullscreen` also panics on a live
//! change that cannot resolve a monitor (`bevy_winit::winit_windows:91`, `system.rs:333`).
//!
//! The `gx*` rows are latched until `RestartGx()`, as in 1.12; a restart re-asserts them against
//! the window rather than re-creating the device, since wgpu reconfigures the surface live.
//!
//! Uncapped is `AutoNoVsync`, never `Immediate`: on Metal `Immediate` takes ~1 s `nextDrawable`
//! stalls ([`crate::capture::probe_uncap_mode`]).

use benilla_ui::script::ScreenResolution;
use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode, WindowResolution};

/// `$WOW_NOVSYNC=1`: uncap for this session only; it never reaches `config.toml` (the
/// `session_owned` set in [`crate::cvars`]).
pub(crate) fn novsync_env() -> bool {
    std::env::var("WOW_NOVSYNC").as_deref() == Ok("1")
}

/// Whether this run sizes its own window (a capture, a background run) and so stays windowed.
/// Session-only: `gxWindow`/`gxResolution` are env-overridden while it holds, never saved over.
pub(crate) fn windowed_env() -> bool {
    std::env::var_os("WOW_WIN").is_some()
        || std::env::var_os("WOW_CAPTURE").is_some()
        || std::env::var_os("WOW_CAPTURE_UI").is_some()
        || benilla_world::bgwin::background_run()
}

/// `$WOW_WIN=WxH` in logical px; the one parser, so the size asked for and the size checked agree.
pub(crate) fn requested_window_size() -> Option<UVec2> {
    let v = std::env::var("WOW_WIN").ok()?;
    let (w, h) = v.split_once('x')?;
    Some(UVec2::new(w.parse().ok()?, h.parse().ok()?))
}

/// `$WOW_DPI=<f32>`: render at another display's pixel grid, since text rasterizing and snapping
/// quantize in device pixels. It overrides the window's scale factor, so `WOW_WIN` then means
/// physical pixels and a capture is the framebuffer that display would show.
pub(crate) fn requested_dpi() -> Option<f32> {
    let v: f32 = std::env::var("WOW_DPI").ok()?.parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v)
}

/// Apply [`requested_dpi`] to a window resolution; the one place the knob is spent.
pub(crate) fn at_requested_dpi(res: WindowResolution) -> WindowResolution {
    match requested_dpi() {
        Some(dpi) => res.with_scale_factor_override(dpi),
        None => res,
    }
}

/// The display modes benilla ships; neither is the reference's mode-setting fullscreen.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum DisplayMode {
    /// Borderless, filling the monitor: `gxWindow "0"`, the reference's default.
    #[default]
    Fullscreen,
    /// A window at [`VideoConfig::windowed`]. `gxWindow "1"`.
    Windowed,
}

/// The windowed size a fresh config gets.
pub(crate) const DEFAULT_WINDOWED: UVec2 = UVec2::new(1600, 900);

/// `gxWindow`'s value to the mode, by the reference's 0/1 parse (int, `!= 0`); shared by
/// [`crate::cvars`]'s arm and the boot read.
pub(crate) fn display_from_flag(v: f32) -> DisplayMode {
    if v != 0.0 {
        DisplayMode::Windowed
    } else {
        DisplayMode::Fullscreen
    }
}

/// `gxResolution`'s value (`"1280x800"`, parsed by the reference with `sscanf("%d%c%d")`) to a
/// size. A zero extent is refused rather than handed to the windowing system.
pub(crate) fn parse_resolution(value: &str) -> Option<UVec2> {
    let (w, h) = value.split_once(['x', 'X'])?;
    let size = UVec2::new(w.trim().parse().ok()?, h.trim().parse().ok()?);
    (size.x > 0 && size.y > 0).then_some(size)
}

/// The `WindowMode` a display mode means, on a given monitor.
pub(crate) fn window_mode(display: DisplayMode, monitor: MonitorSelection) -> WindowMode {
    match display {
        DisplayMode::Fullscreen => WindowMode::BorderlessFullscreen(monitor),
        DisplayMode::Windowed => WindowMode::Windowed,
    }
}

/// The mode the primary window is born in, resolved before the `App` exists so a fullscreen launch
/// does not flash windowed until `Startup`. `MonitorSelection::Primary`, since `Current` has no
/// answer before the window exists (`bevy_winit::select_monitor`).
pub(crate) fn boot_window_mode() -> WindowMode {
    let display = if windowed_env() {
        DisplayMode::Windowed
    } else {
        crate::cvars::boot_cvar("gxWindow")
            .and_then(|v| v.parse::<f32>().ok())
            .map_or_else(DisplayMode::default, display_from_flag)
    };
    window_mode(display, MonitorSelection::Primary)
}

/// The windowed size the primary window is born at, `gxResolution`, read as [`boot_window_mode`]
/// is; `bevy_winit` ignores it while fullscreen.
pub(crate) fn boot_windowed_size() -> UVec2 {
    crate::cvars::boot_cvar("gxResolution")
        .and_then(|v| parse_resolution(&v))
        .unwrap_or(DEFAULT_WINDOWED)
}

/// The video knobs a CVar write lands on. Defaults read only the environment; `load_config`
/// applies the file at `Startup`, which the boot window already matches.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VideoConfig {
    pub(crate) vsync: bool,
    pub(crate) display: DisplayMode,
    /// The windowed size, `gxResolution`. Kept while fullscreen so leaving it can restore it.
    pub(crate) windowed: UVec2,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            vsync: !novsync_env(),
            display: if windowed_env() {
                DisplayMode::Windowed
            } else {
                DisplayMode::default()
            },
            windowed: DEFAULT_WINDOWED,
        }
    }
}

/// The present mode a vsync setting means. On is `PresentMode::default()` (`Fifo`, never tears),
/// not `AutoVsync`, which resolves to `FifoRelaxed` and can tear on a late frame.
pub(crate) fn present_mode(vsync: bool) -> PresentMode {
    if vsync {
        PresentMode::default()
    } else {
        PresentMode::AutoNoVsync
    }
}

pub(crate) struct VideoPlugin;

/// The change callbacks of the reference's video-options registration block (`0x688470`); each
/// arm writes only its own resource.
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut cfg: ResMut<VideoConfig>,
    mut view: ResMut<benilla_world::view::ViewDistance>,
    mut msaa: ResMut<benilla_world::view::MsaaSetting>,
    msaa_formats: Res<benilla_world::view::MsaaFormats>,
    mut tex_filter: ResMut<benilla_assets::TexFilterSetting>,
    mut clutter: ResMut<benilla_world::clutter::ClutterConfig>,
    mut weather: ResMut<benilla_world::weather::WeatherState>,
    mut cvars: ResMut<crate::cvars::Cvars>,
) {
    use benilla_world::view::{FARCLIP_RANGE, MSAA_RANGE};
    let v = ev.num();
    match ev.key().as_str() {
        // A value that is not a size is ignored with a warning.
        "gxresolution" => match parse_resolution(&ev.new) {
            Some(size) => cfg.windowed = size,
            None => warn!("cvar gxResolution: unparseable value '{}' ignored", ev.new),
        },
        "gxvsync" => cfg.vsync = ev.flag(),
        // The reference's polarity: `1` is windowed (the row is "Windowed Mode").
        "gxwindow" => cfg.display = display_from_flag(v),
        "farclip" => view.farclip = v.clamp(*FARCLIP_RANGE.start(), *FARCLIP_RANGE.end()),
        // Clamped, where the reference refuses an out-of-range write and keeps the value
        // (`0x688d90` echoes "NearClip must be in range 0.01 - 0.33" and returns 0).
        "nearclip" => view.set_nearclip(v),
        // The reference's `atoi` and clamp to `[1, 16]` (`0x63b250`), then this GPU's ceiling:
        // an unoffered count is a wgpu validation error on the first frame. The camera reads it
        // once, at spawn.
        "gxmultisample" => {
            let asked = (v as u32).clamp(*MSAA_RANGE.start(), *MSAA_RANGE.end());
            let granted = msaa_formats.clamp(asked);
            if granted != asked {
                warn!("cvar gxMultisample: this GPU does not offer {asked}x multisampling — using {granted}x");
            }
            msaa.samples = granted;
        }
        // Not applied until the next launch (the filter policy is published once at the end of
        // `CvarLoad`); the reference registers both with `flags = 1` and applies them live.
        // `anisotropic` takes the reference's clamp to `[1, 16]` (`0x689110`).
        "trilinear" => tex_filter.trilinear = ev.flag(),
        "anisotropic" => {
            tex_filter.aniso = (v as u32).clamp(
                *benilla_assets::ANISO_RANGE.start(),
                *benilla_assets::ANISO_RANGE.end(),
            )
        }
        // The panel's 0/1/2 is the density multiplier x1/x2/x3, clamped to the 1.12 slider's
        // range. `frillDensity` is the same knob, mirrored so `GetCVar` never answers two levels.
        "worlddetail" => {
            clutter.density = v.clamp(0.0, 2.0) + 1.0;
            cvars.mirror(
                benilla_ui::script::CVAR_FRILL_DENSITY,
                &clutter.frill_density().to_string(),
            );
        }
        // The same knob in the reference's cells per chunk, clamped to `[1, 256]`
        // (`ClutterConfig::set_frill_density`); the loaded tiles re-scatter off the change.
        "frilldensity" => {
            clutter.set_frill_density(v);
            cvars.mirror(
                benilla_ui::script::CVAR_WORLD_DETAIL,
                &(clutter.density - 1.0).to_string(),
            );
        }
        // Weather Intensity 0..3: the reference's callback `0x67b870` jumps (`0x67b8e8`) to the
        // quality cells {0.1, 0.33, 0.66, 1.0} at `[0x8680ec]`. Its off-grid handling is
        // untraced; this clamps.
        "weatherdensity" => weather.weather_density = v.trunc().clamp(0.0, 3.0) as u8,
        _ => {}
    }
}

/// `/console detailDoodadAlpha [0..255]`, the reference's console command (`0x6739a0`, registered
/// by `0x63f9e0` as a command, not a CVar, so it never persists): the ground-clutter cutout that
/// `texel.a x distance_ramp` is tested against, so at the default 128 grass ends ~61 yd out of
/// the 70 yd horizon. Out of range is rejected (`0x6739b9`) with a readout. A bare command also
/// prints the readout, where the reference reads an uninitialised stack slot.
fn detail_doodad_alpha(world: &mut World, args: &str) -> Vec<String> {
    let Some(mut clutter) = world.get_resource_mut::<benilla_world::clutter::ClutterConfig>()
    else {
        return vec!["detailDoodadAlpha: this run has no ground clutter".to_string()];
    };
    match args
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<u8>().ok())
    {
        Some(v) => {
            clutter.alpha_ref = f32::from(v) / 255.0;
            vec![format!("detailDoodadAlpha set to {v}")]
        }
        None => vec![format!(
            "detailDoodadAlpha is {} (usage: /console detailDoodadAlpha 0-255)",
            (clutter.alpha_ref * 255.0).round() as u32
        )],
    }
}

impl Plugin for VideoPlugin {
    fn build(&self, app: &mut App) {
        use crate::console::ConsoleCommandApp;
        app.add_observer(on_cvar);
        app.console_command(
            "detailDoodadAlpha",
            "The ground-clutter cutout reference, 0-255 (128 = the default).",
            detail_doodad_alpha,
        );
        app.init_resource::<VideoConfig>()
            .init_resource::<GxRestarts>()
            .add_systems(Startup, (log_display_session, check_window_pinned).chain())
            .add_systems(
                Update,
                (
                    // After the tick and the CVar sync: the stock Okay handler calls `SetCVar` per
                    // row, then `RestartGx()`, so the staged rows are registered before the commit.
                    (drain_restart_gx, (apply_present_mode, apply_window_mode))
                        .chain()
                        .after(crate::ui_script::UiInput)
                        .after(crate::cvars::sync_cvars),
                    // A push the tick may read (`GetScreenResolutions`): the feed phase.
                    publish_display_modes.in_set(crate::ui_script::UiFeed),
                ),
            );
    }
}

/// How many `RestartGx()` calls the interface has made. A counter, not a flag: each applier keeps
/// its own last value, so one bump forces one re-assertion in each, in any order.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GxRestarts(u32);

/// Carry `RestartGx()` calls into [`GxRestarts`] and commit the latched `gx*` rows (flags `3`),
/// where the reference's restart calls `CVar::Update` (`0x63e060`) on each; `GetCVar` answers the
/// applied value until then.
fn drain_restart_gx(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut restarts: ResMut<GxRestarts>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    mut commands: Commands,
) {
    let Some(mut script) = script else {
        return;
    };
    let asks = script.take_restart_gx_asks();
    if asks == 0 {
        // A `ResMut` deref-mut is a change signal, so a quiet frame touches nothing.
        return;
    }
    restarts.0 = restarts.0.wrapping_add(asks);
    let committed = cvars.commit_latched();
    if cvars.has_events() {
        for event in cvars.take_events() {
            commands.trigger(event);
        }
    }
    info!("video: RestartGx — {committed} staged setting(s) committed; re-asserting the display mode and present mode");
}

/// The reference's three filters on the resolution list (`0x48bcfa` to `0x48bd18`): keep when
/// `w/h >= 1.248` (`[0x804570]`, just under 5:4, so square and portrait fail), `w >= 800` and
/// `h >= 600`.
fn offerable(r: ScreenResolution) -> bool {
    r.width >= 800 && r.height >= 600 && f64::from(r.width) / f64::from(r.height) >= 1.248
}

/// The reference's four hardcoded modes, in its append order (`0x48bda2`, `0x48bddf`, `0x48be43`,
/// `0x48bea7`), used when no enumerated mode survives [`offerable`]. The reference also takes this
/// path when `widescreen` (`0x63a747`, default `"1"`) is 0; benilla does not register that CVar.
const SCREEN_FALLBACK: [ScreenResolution; 4] = [
    ScreenResolution {
        width: 800,
        height: 600,
    },
    ScreenResolution {
        width: 1024,
        height: 768,
    },
    ScreenResolution {
        width: 1280,
        height: 1024,
    },
    ScreenResolution {
        width: 1600,
        height: 1200,
    },
];

/// The list [`publish_display_modes`] last pushed and the current entry.
type PublishedModes = Option<(Vec<ScreenResolution>, Option<ScreenResolution>)>;

/// The host half of `GetScreenResolutions` / `GetCurrentResolution`: the monitors' mode sizes and
/// full sizes through [`offerable`], else [`SCREEN_FALLBACK`]; a pick sets the windowed size,
/// `gxResolution`. The engine adds the live window size when missing, which `CT_Viewport.lua:201`
/// needs. Deviation: sizes are logical px, because `gxResolution` is a logical inner size here.
/// The list is recomputed on change; the reference builds its list once and never invalidates it.
fn publish_display_modes(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
    mut last: Local<crate::ui_script::VmMemo<PublishedModes>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let res = &window.resolution;
    let current = Some(ScreenResolution {
        width: res.width() as u32,
        height: res.height() as u32,
    })
    .filter(|r| r.width > 0 && r.height > 0);
    let mut offered: Vec<ScreenResolution> = Vec::new();
    for m in &monitors {
        // The monitor's own scale, not the window's: these rows describe the panel.
        let scale = if m.scale_factor > 0.0 {
            m.scale_factor
        } else {
            1.0
        };
        let logical = |size: UVec2| ScreenResolution {
            width: (size.x as f64 / scale).round() as u32,
            height: (size.y as f64 / scale).round() as u32,
        };
        offered.push(logical(m.physical_size()));
        offered.extend(m.video_modes.iter().map(|v| logical(v.physical_size)));
    }
    offered.retain(|r| offerable(*r));
    if offered.is_empty() {
        offered.extend(SCREEN_FALLBACK);
    }
    offered.sort_by_key(|r| (u64::from(r.width) * u64::from(r.height), r.width, r.height));
    offered.dedup();
    // Keyed by VM: a `ReloadUI` VM has been pushed nothing, so a plain `Local` would skip it.
    let memo = last.get(&script);
    if memo.as_ref() == Some(&(offered.clone(), current)) {
        return;
    }
    *memo = Some((offered.clone(), current));
    script.set_screen_resolutions(offered, current);
}

/// Check the window got the size `$WOW_WIN` asked for: the window manager may clamp it to the
/// display (macOS does). Fatal under a capture, whose diffs assume the scenario's size; a warning
/// otherwise.
fn check_window_pinned(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(want) = requested_window_size() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    // Under `$WOW_DPI`, `$WOW_WIN` is physical px, so compare the physical size.
    let res = &window.resolution;
    let got = match requested_dpi() {
        Some(_) => UVec2::new(res.physical_width(), res.physical_height()),
        None => UVec2::new(res.width() as u32, res.height() as u32),
    };
    if got == want {
        return;
    }
    let unit = if requested_dpi().is_some() {
        "physical"
    } else {
        "logical"
    };
    let capturing =
        std::env::var_os("WOW_CAPTURE").is_some() || std::env::var_os("WOW_CAPTURE_UI").is_some();
    if !capturing {
        warn!(
            "window: asked for {}x{} {unit}, got {}x{} — the window manager clamped it to the \
             display. Harmless here; it would invalidate a capture.",
            want.x, want.y, got.x, got.y
        );
        return;
    }
    error!(
        "window: REFUSING this capture — asked for {}x{} {unit}, got {}x{}. The window manager \
         clamped the request to the display this window opened on, so the image would not be the \
         size the scenario is denominated in and any diff against it would be meaningless. Use a \
         size that fits the current display (or move the window to a bigger one) and re-run.",
        want.x, want.y, got.x, got.y
    );
    exit.write(AppExit::error());
}

/// One boot line naming what the window got and, on Linux, the backend and any nested compositor
/// (mouse-look's `CursorGrabMode::Locked` is rejected on X11). Not dev-only: players paste it.
fn log_display_session(windows: Query<&Window, With<PrimaryWindow>>) {
    let Ok(window) = windows.single() else {
        return;
    };
    let res = &window.resolution;
    info!(
        "video: {:?}, {}x{} logical / {}x{} physical (scale {}){}",
        window.mode,
        res.width(),
        res.height(),
        res.physical_width(),
        res.physical_height(),
        res.scale_factor(),
        display_session(),
    );
}

/// The display-server facts as a trailing clause; empty except on Linux/BSD, where the windowing
/// backend is chosen at runtime.
fn display_session() -> String {
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        let set = |k: &str| std::env::var_os(k).is_some();
        // winit prefers Wayland when `WAYLAND_DISPLAY` is set, else X11 (XWayland under a
        // Wayland compositor); `bevy`'s `default_platform` builds both.
        let backend = match (set("WAYLAND_DISPLAY"), set("DISPLAY")) {
            (true, _) => "wayland",
            (false, true) => "x11",
            (false, false) => "none",
        };
        // gamescope exports its socket name to children; the SteamOS session stamps the Deck.
        let nested = if set("GAMESCOPE_WAYLAND_DISPLAY") {
            ", gamescope"
        } else {
            ""
        };
        let deck = if set("SteamDeck") { ", steamdeck" } else { "" };
        format!(
            " [{backend}{nested}{deck}, XDG_SESSION_TYPE={}]",
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unset".into()),
        )
    }
    #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android")))))]
    String::new()
}

/// Push vsync to the window only when the setting moves or a `RestartGx()` asks: the capture
/// probes write `present_mode` directly, and their override must stick.
fn apply_present_mode(
    cfg: Res<VideoConfig>,
    restarts: Res<GxRestarts>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last_restart: Local<u32>,
) {
    // A `RestartGx()` re-asserts even when nothing moved.
    let forced = std::mem::replace(&mut *last_restart, restarts.0) != restarts.0;
    if !cfg.is_changed() && !forced {
        return;
    }
    let want = present_mode(cfg.vsync);
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    // A spurious change to `Window` is a surface reconfigure.
    if window.present_mode != want {
        window.present_mode = want;
        info!(
            "video: vsync {} ({want:?})",
            if cfg.vsync { "on" } else { "off" }
        );
    }
}

/// Push the display mode to the window, gated as [`apply_present_mode`] is. The guard matches the
/// variant, not the monitor: birth uses `Primary` and a live toggle `Current`, so a `!=` compare
/// would re-assert fullscreen on every launch.
fn apply_window_mode(
    cfg: Res<VideoConfig>,
    restarts: Res<GxRestarts>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last_restart: Local<u32>,
) {
    // A `RestartGx()` re-asserts even when nothing moved, re-applying `gxResolution` to a window
    // that is already windowed.
    let forced = std::mem::replace(&mut *last_restart, restarts.0) != restarts.0;
    if !cfg.is_changed() && !forced {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let want = window_mode(cfg.display, MonitorSelection::Current);
    let already = matches!(
        (&window.mode, &want),
        (WindowMode::Windowed, WindowMode::Windowed)
            | (
                WindowMode::BorderlessFullscreen(_),
                WindowMode::BorderlessFullscreen(_)
            )
    );
    if already && !forced {
        return;
    }
    // Leaving fullscreen hands the size back: entering it overwrote `window.resolution` with the
    // monitor's (`bevy_window`'s documented behaviour). Both writes land in one frame, as
    // `bevy_winit::changed_windows` applies `mode` before `resolution`.
    if cfg.display == DisplayMode::Windowed {
        window
            .resolution
            .set(cfg.windowed.x as f32, cfg.windowed.y as f32);
    }
    window.mode = want;
    info!("video: display mode {:?} ({want:?})", cfg.display);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synced is `Fifo`, not `AutoVsync` (`FifoRelaxed`, which tears on a late frame).
    #[test]
    fn the_default_is_synced_like_the_window_literal() {
        assert!(VideoConfig::default().vsync);
        assert_eq!(present_mode(true), PresentMode::default());
        assert_eq!(present_mode(true), PresentMode::Fifo);
    }

    /// On Metal `Immediate` takes ~1 s `nextDrawable` stalls.
    #[test]
    fn uncapped_is_autonovsync_not_immediate() {
        assert_eq!(present_mode(false), PresentMode::AutoNoVsync);
    }

    /// `WindowMode::Fullscreen` is ignored on Wayland and panics without a monitor.
    #[test]
    fn the_default_is_borderless_fullscreen_not_exclusive() {
        assert_eq!(DisplayMode::default(), DisplayMode::Fullscreen);
        assert!(matches!(
            window_mode(DisplayMode::Fullscreen, MonitorSelection::Primary),
            WindowMode::BorderlessFullscreen(_)
        ));
        assert_eq!(
            window_mode(DisplayMode::Windowed, MonitorSelection::Primary),
            WindowMode::Windowed
        );
    }

    /// The reference's polarity: the row is "Windowed Mode".
    #[test]
    fn gxwindow_one_is_windowed() {
        assert_eq!(display_from_flag(0.0), DisplayMode::Fullscreen);
        assert_eq!(display_from_flag(1.0), DisplayMode::Windowed);
    }

    /// A window born fullscreen is not re-asserted on the first frame, and leaving fullscreen
    /// hands `gxResolution` back rather than the monitor's size.
    #[test]
    fn the_fullscreen_round_trip_keeps_the_monitor_out_of_the_windowed_size() {
        let mut app = App::new();
        app.insert_resource(VideoConfig {
            vsync: true,
            display: DisplayMode::Fullscreen,
            windowed: UVec2::new(1024, 768),
        })
        // Seated by hand: this test runs the one system, not the plugin.
        .init_resource::<GxRestarts>()
        .add_systems(Update, apply_window_mode);
        let win = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        // Born as `lib.rs` builds it: fullscreen, on the boot monitor selection.
        app.world_mut()
            .entity_mut(win)
            .get_mut::<Window>()
            .unwrap()
            .mode = window_mode(DisplayMode::Fullscreen, MonitorSelection::Primary);

        app.update();
        assert!(
            matches!(
                app.world().entity(win).get::<Window>().unwrap().mode,
                WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
            ),
            "an already-fullscreen window must not be re-asserted onto another monitor selection"
        );

        // Stand in for the compositor: fullscreen makes the resolution the monitor's. Then leave.
        app.world_mut()
            .entity_mut(win)
            .get_mut::<Window>()
            .unwrap()
            .resolution
            .set(3440.0, 1440.0);
        app.world_mut().resource_mut::<VideoConfig>().display = DisplayMode::Windowed;
        app.update();
        let w = app.world().entity(win).get::<Window>().unwrap();
        assert_eq!(w.mode, WindowMode::Windowed);
        assert_eq!(
            (w.resolution.width(), w.resolution.height()),
            (1024.0, 768.0),
            "leaving fullscreen restores gxResolution, never the monitor's size"
        );

        // Back in, on `Current`: the monitor the window is on by now.
        app.world_mut().resource_mut::<VideoConfig>().display = DisplayMode::Fullscreen;
        app.update();
        assert!(matches!(
            app.world().entity(win).get::<Window>().unwrap().mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        ));
    }

    #[test]
    fn gxresolution_parses_the_reference_spelling() {
        assert_eq!(parse_resolution("1280x800"), Some(UVec2::new(1280, 800)));
        assert_eq!(parse_resolution("1600X900"), Some(UVec2::new(1600, 900)));
        assert_eq!(parse_resolution("1024 x 768"), Some(UVec2::new(1024, 768)));
        assert_eq!(parse_resolution("0x600"), None);
        assert_eq!(parse_resolution("1280x"), None);
        assert_eq!(parse_resolution("fullscreen"), None);
    }
}
