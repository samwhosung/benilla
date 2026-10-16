//! The stock glue screens: the player's own `GlueXML.toc` off the patch chain, run into a glue VM
//! ([`UiScript::new_glue`]) that stands in the world while no character is in it, where the boot VM
//! otherwise stands. They are the glue screens of every run but a dev build's
//! `WOW_GLUE_NATIVE=1` ([`GlueStock::from_env`]), which puts up the native screens until they are
//! deleted.
//!
//! The reference's glue builder (`0x46a7b0`) resets the Lua state on a rebuild (`0x46a87b`; the
//! cold first build, `0x46a505`, passes 0 and skips it), registers the glue functions
//! (`0x46abb0`) and the 26 glue events (`0x46a88f`), opens `Logs\GlueXML.log` (`0x46a8ce`), then
//! checks `GlueXML.toc`'s signature (`0x46a90b`). Only a valid one (`0x6f10f0` answering 3) loads
//! the toc through the shared runner (`0x6edb90`, at `0x46a9b3`), and only when the loaded files'
//! MD5 matches the signature's (`0x46a9d3`) does `FRAMES_LOADED` fire (`0x46aa34`). Signature
//! results 0–2 (the table at `0x46aaac`) log their line, missing, corrupt or modified (`0x836fb8`,
//! `0x836f98`, `0x836f78`), and load nothing; an MD5 mismatch logs no line, its frames already
//! built, and skips `FRAMES_LOADED`. Either way the client quits with
//! error 9 (`0x401560`: the exit code, then the quit post `ForceQuit` makes with code 0).
//! Deviation: no signature or digest check (`0x46a90b`, `0x46a9d3`, `0x401560(9)`), so the chain's
//! toc always loads and `FRAMES_LOADED` follows, because the check verifies Blizzard's signature
//! with a key carried in the client binary, which benilla does not ship, and a modified interface
//! file is loaded rather than refused.
//!
//! The screen comes after the build: at startup the reference fires `SET_GLUE_SCREEN` with
//! `"movie"` while the `movie` CVar is 1 (clearing it), else `"login"` (`0x402b3b`–`0x402b65`);
//! back from the world `0x402090` fires it with `"charselect"` or `"login"`. benilla plays no movie
//! yet (#968), so its boot screen is `"login"`.
//!
//! The screens' `ModelFFX` panes drive the glue booth ([`super::glue_scene`]): its scene rig, the
//! scene M2's own point lights and the `FfxGlow::GLUE_SCENE` pass, never the `ui_models` tile
//! path, where every lit batch draws black.

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use super::reference_ui;

/// The dev switch that puts the native glue screens up instead of the stock ones.
const NATIVE_SWITCH: &str = "WOW_GLUE_NATIVE";

/// The instruments outside the native screens' modules that still drive them, which a dev run
/// that names one puts up: the rig and the create probe (`capture::probe_rig`,
/// `capture::probe_charcreate`) create, delete and pick through the park's own enumerations, which
/// the stock screen replaces with its own ask, and the glue captures (`WOW_CAPTURE=glue-…`,
/// `capture::scenarios::GLUE_SCENARIOS`) photograph and feed the native screens.
const NATIVE_INSTRUMENTS: [&str; 2] = ["WOW_RIG", "WOW_PROBE_CHARCREATE"];

/// The native screens' own instruments, inert under the stock screens: named in a warning when a
/// run sets one without [`NATIVE_SWITCH`].
const NATIVE_ONLY: [&str; 10] = [
    "WOW_LOGIN_SMOKE",
    "WOW_CHARSELECT_DIALOG",
    "WOW_CHARSELECT_ADDONS",
    "WOW_CHARSELECT_SHOT_OUT",
    "WOW_CHARSELECT_SHOT_AT",
    "WOW_CHARCREATE_NAME",
    "WOW_CHARCREATE_SHOT",
    "WOW_CHARCREATE_PICK",
    "WOW_CHARCREATE_DIALS",
    "WOW_CHARCREATE_SHOT_OUT",
];

/// Whether this run puts the stock glue screens up, read once at plugin build. Absent, as in a
/// test's partial app, it reads as stock, the default ([`is_stock`]).
#[derive(Resource, Clone, Copy)]
pub(crate) struct GlueStock(pub(crate) bool);

impl Default for GlueStock {
    fn default() -> Self {
        Self(true)
    }
}

impl GlueStock {
    /// The switch as this process has it: stock, except in a dev build whose environment asks for
    /// the native screens ([`native_asked`]). A player build always stands on the stock screens.
    pub(crate) fn from_env() -> Self {
        if !crate::run_mode::dev_affordances() {
            return Self(true);
        }
        let var = |k: &str| std::env::var(k).ok();
        let asked = native_asked(var);
        match &asked {
            Some(why) => info!("glue: {why}: the native glue screens stand this run"),
            None => {
                for k in NATIVE_ONLY.into_iter().filter(|k| var(k).is_some()) {
                    warn!("glue: {k} drives the native glue screens and does nothing under the stock ones; set {NATIVE_SWITCH}=1 for it");
                }
            }
        }
        Self(asked.is_none())
    }
}

/// What in a dev run's environment asks for the native screens, if anything: the switch, or an
/// instrument that still drives them ([`NATIVE_INSTRUMENTS`], a glue capture).
fn native_asked(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if var(NATIVE_SWITCH).as_deref() == Some("1") {
        return Some(format!("{NATIVE_SWITCH}=1"));
    }
    if let Some(name) = var("WOW_CAPTURE").filter(|n| n.starts_with("glue-")) {
        return Some(format!("WOW_CAPTURE={name} photographs the native screens"));
    }
    NATIVE_INSTRUMENTS
        .into_iter()
        .find(|k| var(k).is_some())
        .map(|k| format!("{k} drives the native screens' roster"))
}

/// The stock screens stand: the resource says so, or is absent and the default holds.
pub(crate) fn is_stock(stock: Option<&GlueStock>) -> bool {
    stock.is_none_or(|s| s.0)
}

/// The stock glue screens as a dev instrument reaches them, so no instrument names the VM: which
/// screens stand, and through [`StockVm`] the glue VM's frames and verbs.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct StockGlueProbe<'w> {
    stock: Option<Res<'w, GlueStock>>,
    script: Option<NonSend<'w, UiScript>>,
}

impl StockGlueProbe<'_> {
    /// The glue VM, `None` under the native screens. With the stock screens and the glue VM not
    /// standing (the world's VM, or a build that failed), a [`StockVm`] that shows nothing and
    /// presses nothing.
    pub(crate) fn vm(&self) -> Option<StockVm<'_>> {
        is_stock(self.stock.as_deref()).then(|| {
            StockVm(
                self.script
                    .as_deref()
                    .filter(|s| s.vm_kind() == benilla_ui::script::VmKind::Glue),
            )
        })
    }
}

/// The glue VM an instrument reads and presses.
#[derive(Clone, Copy)]
pub(crate) struct StockVm<'a>(Option<&'a UiScript>);

impl StockVm<'_> {
    /// The glue VM is standing.
    pub(crate) fn is_up(&self) -> bool {
        self.0.is_some()
    }

    /// `IsVisible()` of the frame with this global name; false with no VM.
    pub(crate) fn frame_visible(&self, name: &str) -> bool {
        self.0.is_some_and(|s| s.frame_visible(name))
    }

    /// A Lua chunk's string value, as a stock function's caller reads it; `None` with no VM, for
    /// `nil`, and on a raise, which is logged.
    pub(crate) fn text(&self, chunk: &str) -> Option<String> {
        self.0?
            .eval::<Option<String>>(chunk)
            .inspect_err(|e| error!("glue probe: {chunk} raised: {e}"))
            .ok()
            .flatten()
    }

    /// A Lua chunk's number value; `None` with no VM and on a raise, which is logged.
    pub(crate) fn number(&self, chunk: &str) -> Option<f64> {
        self.0?
            .eval::<f64>(chunk)
            .inspect_err(|e| error!("glue probe: {chunk} raised: {e}"))
            .ok()
    }

    /// Run a Lua chunk, the way a button's handler calls a stock function; a raise is logged.
    pub(crate) fn press(&self, chunk: &str) {
        if let Some(Err(e)) = self.0.map(|s| s.run(chunk)) {
            error!("glue probe: {chunk} raised: {e}");
        }
    }
}

/// Run condition: the stock glue screens are up, they stand this run and no character is in the
/// world.
pub(crate) fn glue_ui_up(
    stock: Option<Res<GlueStock>>,
    state: Option<Res<State<crate::client_state::ClientState>>>,
) -> bool {
    is_stock(stock.as_deref())
        && state.is_none_or(|s| *s.get() != crate::client_state::ClientState::InWorld)
}

/// Run condition: the stock screens stand this run, whether or not a character is in the world.
pub(crate) fn stock_glue(stock: Option<Res<GlueStock>>) -> bool {
    is_stock(stock.as_deref())
}

/// Run condition: the native glue screens draw and take input, a dev run having asked for them.
pub(crate) fn native_glue(stock: Option<Res<GlueStock>>) -> bool {
    !is_stock(stock.as_deref())
}

/// The glue screen `SET_GLUE_SCREEN` names, `GlueParent.lua`'s `GlueScreenInfo` key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueScreen {
    Login,
    CharSelect,
}

impl GlueScreen {
    /// The key, as the reference's literals spell it (`0x82e3e0`, `0x82e3e8`).
    pub(crate) fn key(self) -> &'static str {
        match self {
            GlueScreen::Login => "login",
            GlueScreen::CharSelect => "charselect",
        }
    }
}

/// Load the chain's `GlueXML.toc` into `script`, a glue VM, and fire `FRAMES_LOADED`: the glue
/// builder's steps from the toc on (`0x46a9b3`, `0x46aa34`). Returns every failure; a loader error
/// is tagged `"GlueXML/<file>: <error>"`, a handler that raised during the walk is one too. The
/// load's record is left for [`finish_glue_load_log`].
pub(super) fn load_glue(script: &mut UiScript) -> Vec<String> {
    // The builder's sweep before the checks and the toc (`0x46a8eb`).
    super::addons::sweep_loose_interface();
    let mut failures = match reference_ui::glue() {
        Some(glue) => {
            let mut toc = benilla_ui::status::Status::default();
            let failures = glue.load_files_into(script, &glue.toc.files, &mut toc);
            let mut log = benilla_ui::status::Status::default();
            let banner = benilla_ui::status::toc_banner(reference_ui::GLUE_TOC);
            toc.close_into(&mut log, script.framexml_debug(), banner);
            script.report_load_status(log);
            failures
        }
        None => {
            // The runner's `"Couldn't open %s"` (`0x846ff4`) at severity 2; no glue frame exists.
            let e = format!(
                "{}: not found — the stock glue screens are not built",
                reference_ui::GLUE_TOC
            );
            error!("ui_script: {e}");
            script.report_load_failure(&e);
            let mut log = benilla_ui::status::Status::default();
            log.report(
                benilla_ui::status::FAILURE,
                benilla_ui::status::missing(reference_ui::GLUE_TOC, false),
            );
            script.report_load_status(log);
            vec![e]
        }
    };
    if super::manifest::layer_enabled() {
        failures.extend(load_glue_layer(script));
    }
    script.fire_event("FRAMES_LOADED", vec![]);
    failures.extend(
        script
            .errors()
            .into_iter()
            .map(|e| format!("a handler raised during the glue load: {e}")),
    );
    failures
}

/// benilla's one glue file, its realmlist control over the stock `AccountLogin`, which loads after
/// every `GlueXML.toc` row and before `FRAMES_LOADED`, as the in-game layer loads after
/// `FrameXML.toc` ([`super::manifest::LAYER_MANIFEST`]); `WOW_STOCK_UI=1` drops it with that layer.
pub(super) const GLUE_LAYER: &str = "AccountLoginRealmlist.xml";

/// Load [`GLUE_LAYER`] off the shipped tree; returns its failures.
fn load_glue_layer(script: &UiScript) -> Vec<String> {
    let files = vec![GLUE_LAYER.to_string()];
    let layer = super::addons::Addon::new(
        "benilla".to_string(),
        benilla_ui::toc::Toc {
            directives: Vec::new(),
            files: files.clone(),
        },
        super::addons::Source::Builtin,
    );
    layer.load_files(script, &files)
}

/// Drain the glue load's record into `Logs\GlueXML.log`, as the builder does after
/// `FRAMES_LOADED` (`0x46aa61`-`0x46aa94`).
pub(super) fn finish_glue_load_log(script: &mut UiScript) {
    script.finish_ui_load_log();
    super::load_log::write_glue(script.take_load_log_writes());
}

/// Put `screen` up: `SET_GLUE_SCREEN` with its key as `arg1` (`0x46b840`, event 0).
#[cfg(test)]
pub(super) fn set_glue_screen(script: &mut UiScript, screen: GlueScreen) {
    signal_glue_screen(script, screen.key());
}

/// `SET_GLUE_SCREEN` with `name` as `arg1` (`0x46b840`, event 0), whatever screen it names.
pub(super) fn signal_glue_screen(script: &mut UiScript, name: &str) {
    script.fire_event("SET_GLUE_SCREEN", vec![ScriptValue::Str(name.to_string())]);
}

/// The glue rebuild's cells: the request (`[0xb41d8c]`), which the console's `reloadUI`
/// (`0x4035f0`) and the glue's size callback (`0x46ac76`) set and the build (`0x46a7e0`) and the
/// per-frame glue update (`0x46b9ac`) clear; the screen size the build or the last size event left
/// (`[0xb41e1c]`, `[0xb41e20]`); and the aspect the glue was built or last resized at (`[0xb41e24]`).
#[derive(Resource, Default)]
pub(crate) struct GlueRebuild {
    requested: bool,
    size: UVec2,
    aspect: f32,
}

impl GlueRebuild {
    /// Ask for a rebuild, as `reloadUI` does whatever state the client is in (`0x4035f0`).
    pub(crate) fn request(&mut self) {
        self.requested = true;
    }
}

/// The glue's aspect (`0x46ab40`): 4/3 with `widescreen` off, else the applied `gxResolution`
/// scanned as width, separator, height (`0x46ab8f`). Not the window's: benilla writes
/// `gxResolution` only through the CVar set path (the config load, `SetCVar`, `/console set`,
/// `SetScreenResolution`, each committed at `RestartGx` as the row is latched), never from a
/// window drag. A size `video::parse_resolution` refuses, a zero or unparsable extent, is no value.
fn gx_aspect(cvars: Option<&crate::cvars::Cvars>) -> Option<f32> {
    let cvars = cvars?;
    if cvars.flag("widescreen") == Some(false) {
        return Some(4.0 / 3.0);
    }
    let size = crate::video::parse_resolution(cvars.get("gxResolution")?)?;
    Some(size.x as f32 / size.y as f32)
}

/// The window's client size in device pixels, the size event's payload (`0x7665c0`).
fn window_size(w: &Window) -> UVec2 {
    UVec2::new(
        w.resolution.physical_width(),
        w.resolution.physical_height(),
    )
}

/// The build's half of [`GlueRebuild`] (`0x46a7b0`): the request cleared (`0x46a7e0`), the size
/// and the aspect stored (`0x46a7f2`-`0x46a81c`).
pub(super) fn glue_built(world: &mut World) {
    let size = {
        let mut q = world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
        q.single(world).map_or(UVec2::ZERO, window_size)
    };
    let aspect = gx_aspect(world.get_resource::<crate::cvars::Cvars>());
    if let Some(mut rebuild) = world.get_resource_mut::<GlueRebuild>() {
        rebuild.requested = false;
        rebuild.size = size;
        if let Some(aspect) = aspect {
            rebuild.aspect = aspect;
        }
    }
}

/// The glue's size callback, its rebuild half (`0x46abe0`). It runs on a size event: the same
/// positive size as stored does nothing (`0x46abeb`-`0x46abfe`); any other is stored, and the
/// `gxResolution` aspect ([`gx_aspect`]), when it moved 0.01 or more from the stored one, is stored
/// and asks for a rebuild (`0x46ac66`-`0x46ac76`). So a window resized with `gxResolution`
/// unchanged rebuilds nothing, and a new `gxResolution` rebuilds once, at the resize its
/// `RestartGx` applies. The callback is the glue top's, so only a standing glue hears it.
pub(crate) fn watch_glue_aspect(
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut rebuild: ResMut<GlueRebuild>,
) {
    let Ok(w) = window.single() else {
        return;
    };
    let size = window_size(w);
    if size.x > 0 && size.y > 0 && size == rebuild.size {
        return;
    }
    rebuild.size = size;
    let Some(aspect) = gx_aspect(cvars.as_deref()) else {
        return;
    };
    // `fcomp`, then `test ah,5; jnp`: only "less" skips.
    if (rebuild.aspect - aspect).abs() < 0.01 {
        return;
    }
    rebuild.aspect = aspect;
    rebuild.requested = true;
}

/// The per-frame glue update's rebuild (`0x46b930`): in screen state 0 alone (`0x46b977`), a
/// pending request suspends and rebuilds the glue (`0x46b993`, `0x46a7b0(1)` at `0x46b99d`) and
/// signals the current screen again (`0x46b9a7`, `0x46b840` with the `SetCurrentScreen` buffer
/// `[0xb41478]`), unless the glue is suspended, in the world (`0x46b98b`); either way the request
/// is spent (`0x46b9ac`). Any other state leaves it for a later frame.
pub(crate) fn run_pending_glue_rebuild(world: &mut World) {
    if !world
        .get_resource::<GlueRebuild>()
        .is_some_and(|r| r.requested)
    {
        return;
    }
    let idle = world
        .get_non_send_resource::<super::glue_mgr::SharedGlueMgr>()
        .is_none_or(|m| m.get().state == super::glue_mgr::ScreenState::Idle);
    if !idle {
        return;
    }
    world.resource_mut::<GlueRebuild>().requested = false;
    let in_world = world
        .get_resource::<State<crate::client_state::ClientState>>()
        .is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
    let screen = world
        .get_non_send_resource::<UiScript>()
        .filter(|s| s.vm_kind() == benilla_ui::script::VmKind::Glue)
        .map(|s| s.glue_screen());
    let Some(screen) = screen.filter(|_| !in_world) else {
        return;
    };
    info!("ui_script: glue rebuild on \"{screen}\"");
    // The old VM goes with the replacement; the buffer is the client's, so it carries over.
    super::lifecycle::install_glue_side_vm(world, &screen);
}

/// `ConsoleExec` lines the glue VM left for the console (`0x515f10` → `ConsoleCommandExecute`),
/// run as the in-game drain runs its own, their output to the log: the glue has no chat to print
/// to.
pub(crate) fn drain_glue_console(world: &mut World) {
    let lines = match world.get_non_send_resource_mut::<UiScript>() {
        Some(mut s) if s.vm_kind() == benilla_ui::script::VmKind::Glue => s.take_console_lines(),
        _ => return,
    };
    for line in lines {
        for out in crate::console::execute(world, &line) {
            info!("console: {out}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The glue functions the stock GlueXML calls that benilla has not built yet: each one's call
    /// raises "attempt to call global `X' (a nil value)", at the load or on the login screen. The
    /// list only shrinks: a name built, or no longer reached, comes off it.
    const UNBUILT_CALLED: &[&str] = &[];

    /// What the player's own files make the reference report too: their `tos.html` uses `<H4>`,
    /// which 1.12's SimpleHTML does not know (`"Frame %s: Unknown element type: %s"`,
    /// `0x87a95c`, and the walk goes on).
    const INSTALL_WARNINGS: &[&str] = &["Frame TOSText: Unknown element type: H4"];

    /// The frames each stock glue screen and dialog is, which the load must build.
    const SCREENS: [&str; 8] = [
        "GlueParent",
        "AccountLogin",
        "CharacterSelect",
        "CharacterCreate",
        "RealmList",
        "GlueDialog",
        "RealmWizard",
        "AddonList",
    ];

    /// The name a "nil value" call error names, in either of Lua's two spellings of it.
    fn nil_call(error: &str) -> Option<&str> {
        let (_, rest) = error.split_once("attempt to call ")?;
        let (kind, rest) = rest.split_once(' ')?;
        if !matches!(kind, "global" | "field" | "method") {
            return None;
        }
        let rest = rest.strip_prefix('`').or_else(|| rest.strip_prefix('\''))?;
        let (name, rest) = rest.split_once('\'')?;
        rest.starts_with(" (a nil value)").then_some(name)
    }

    /// The glue state's functions, `reference/glue_surface.tsv`'s names.
    fn glue_surface() -> BTreeSet<String> {
        let path = format!(
            "{}/../../reference/glue_surface.tsv",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {path}: {e}"))
            .lines()
            .skip(1)
            .filter_map(|l| l.split('\t').next().map(str::to_string))
            .collect()
    }

    #[test]
    fn a_nil_call_names_its_function() {
        assert_eq!(
            nil_call("x.lua:1: attempt to call global `GetBuildInfo' (a nil value)"),
            Some("GetBuildInfo")
        );
        assert_eq!(
            nil_call("x.lua:1: attempt to call field 'Foo' (a nil value)"),
            Some("Foo")
        );
        assert_eq!(
            nil_call("x.lua:1: attempt to index global `X' (a nil value)"),
            None
        );
        assert_eq!(nil_call("x.lua:1: bad argument #1"), None);
    }

    /// The stock screens stand unless a dev run asks for the native ones, by the switch or by an
    /// instrument that still drives them; any other value of the switch leaves the stock screens up.
    #[test]
    fn the_native_screens_stand_only_when_a_dev_run_asks() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(native_asked(env(&[])), None);
        assert_eq!(native_asked(env(&[("WOW_GLUE_NATIVE", "0")])), None);
        assert!(native_asked(env(&[("WOW_GLUE_NATIVE", "1")])).is_some());
        assert!(native_asked(env(&[("WOW_RIG", "60")])).is_some());
        assert!(native_asked(env(&[("WOW_PROBE_CHARCREATE", "Zz")])).is_some());
        assert!(native_asked(env(&[("WOW_CAPTURE", "glue-login")])).is_some());
        assert_eq!(native_asked(env(&[("WOW_CAPTURE", "water-noon")])), None);
        assert!(is_stock(None), "absent, the default holds");
        assert!(is_stock(Some(&GlueStock::default())));
        assert!(!is_stock(Some(&GlueStock(false))));
    }

    /// The stock GlueXML loads into the glue VM, `FRAMES_LOADED` fires and the login screen goes
    /// up, and a few frames run: every screen's frame exists, and the only Lua errors are calls
    /// of the glue functions [`UNBUILT_CALLED`] names, each of which is still unbuilt and still
    /// called. Any other error, an XML problem or a missing widget method, fails here.
    #[test]
    fn the_stock_glue_loads_with_only_unbuilt_verbs_raising() {
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        let mut errors = load_glue(&mut s);
        // `load_glue` reports the walk's handler errors; the rest are the screen's and the ticks'.
        let _ = s.take_errors();
        set_glue_screen(&mut s, GlueScreen::Login);
        for _ in 0..3 {
            s.tick(0.1);
        }
        s.resolve();
        errors.extend(s.take_errors());
        // Every row the session retained: the load's failures and warnings, and what raised.
        errors.extend(s.diagnostics().into_iter().map(|d| d.message));

        let surface = glue_surface();
        let mut called = BTreeSet::new();
        let other: Vec<&String> = errors
            .iter()
            .filter(|e| {
                if let Some(name) = nil_call(e).filter(|n| UNBUILT_CALLED.contains(n)) {
                    called.insert(name);
                    return false;
                }
                if INSTALL_WARNINGS.contains(&e.as_str()) {
                    return false;
                }
                true
            })
            .collect();
        assert!(
            other.is_empty(),
            "the glue load raised {} error(s) or warning(s) that are not a call of an unbuilt \
             glue function on UNBUILT_CALLED:\n{}",
            other.len(),
            other
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let listed: BTreeSet<&str> = UNBUILT_CALLED.iter().copied().collect();
        let stale: Vec<&&str> = listed.difference(&called).collect();
        assert!(
            stale.is_empty(),
            "UNBUILT_CALLED names no longer called unbuilt; take them off: {stale:?}"
        );
        for name in UNBUILT_CALLED {
            assert!(
                surface.contains(*name),
                "{name} is not a glue-state function"
            );
            assert!(
                s.eval::<bool>(&format!("return {name} == nil")).unwrap(),
                "{name} is built; take it off UNBUILT_CALLED"
            );
        }

        for name in SCREENS {
            assert!(
                s.eval::<bool>(&format!(
                    "return type({name}) == 'table' and {name}.GetObjectType ~= nil"
                ))
                .unwrap(),
                "the glue load built no frame {name}"
            );
        }
        assert_eq!(
            s.eval::<(bool, bool)>(
                "return AccountLogin:IsShown() == 1, CharacterSelect:IsShown() == 1"
            )
            .unwrap(),
            (true, false),
            "SET_GLUE_SCREEN \"login\" shows the login screen alone"
        );
    }

    /// The create screen's three info panels raise their icon frame a level in its `OnLoad`
    /// (`CharacterCreate.xml:238`, `:399`, `:560`), above the ScrollFrame beside it. The scroll
    /// child draws in that ScrollFrame's HIGHLIGHT callback (`0x787220`), so the description text
    /// draws under the icon it runs past, not at the scroll child's own level, which ties the icon
    /// and would put the text on top.
    #[test]
    fn the_create_panels_icons_cover_their_scrolled_text() {
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        let _ = load_glue(&mut s);
        set_glue_screen(&mut s, GlueScreen::Login);
        // Shown alone: the screen's art and text need no create catalog.
        s.run("AccountLogin:Hide() CharacterCreate:Show()").unwrap();
        let _ = s.take_errors();
        s.resolve();
        let quads = s.extract();
        let owned = |name: &str| {
            quads
                .iter()
                .filter(|q| s.quad_owner_name(q.target).as_deref() == Some(name))
                .collect::<Vec<_>>()
        };
        for panel in ["Faction", "Race", "Class"] {
            let icon = owned(&format!("CharacterCreate{panel}IconFrame"));
            let icon = icon
                .iter()
                .find(|q| matches!(q.content, benilla_ui::script::QuadContent::Texture { .. }))
                .unwrap_or_else(|| panic!("the {panel} icon draws"));
            let text = owned(&format!("CharacterCreate{panel}ScrollChild"));
            let text: Vec<_> = text
                .iter()
                .filter(|q| matches!(q.content, benilla_ui::script::QuadContent::Text { .. }))
                .collect();
            assert!(!text.is_empty(), "the {panel} panel's text draws");
            let icon_rect = icon.rect.expect("the icon is placed");
            assert!(
                text.iter().any(|q| q
                    .rect
                    .is_some_and(|r| r.left < icon_rect.right && r.top > icon_rect.bottom)),
                "some {panel} text runs under the icon's corner"
            );
            for q in text {
                assert!(q.z < icon.z, "the {panel} text draws under the raised icon");
            }
        }
    }

    /// The stock credits screen (`CreditsFrame.xml`). Its two fades are a black `<Color>` under a
    /// vertical alpha `<Gradient>`, opaque at the scroll frame's edge and clear inward, so the
    /// credits text fades instead of being cut. Its borders are file art in the BACKGROUND layer
    /// beside the solid black bars, and the layer batch's sort (`0x7731a0`) draws them over.
    #[test]
    fn the_stock_credits_fades_ramp_and_its_borders_cover_the_bars() {
        use benilla_ui::script::QuadContent;
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        let _ = load_glue(&mut s);
        set_glue_screen(&mut s, GlueScreen::Login);
        s.run("AccountLogin:Hide() CreditsFrame:Show()").unwrap();
        let _ = s.take_errors();
        s.resolve();
        let quads = s.extract();
        let black = Some([0.0, 0.0, 0.0, 1.0]);

        let (clear, opaque) = ([0.0, 1.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.0]);
        let mut fades: Vec<_> = quads
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture {
                    path: None,
                    color,
                    gradient: Some(g),
                    ..
                } if *color == black => Some((q.rect.expect("a fade is placed"), *g)),
                _ => None,
            })
            .collect();
        fades.sort_by(|a, b| a.0.bottom.total_cmp(&b.0.bottom));
        assert_eq!(fades.len(), 2, "both fades draw");
        assert_eq!(
            fades[0].1,
            [clear, clear, opaque, opaque],
            "the bottom fade: opaque along the bottom, clear at its top"
        );
        assert_eq!(
            fades[1].1,
            [opaque, opaque, clear, clear],
            "the top fade: opaque along the top, clear at its bottom"
        );

        let owned_by_credits = |q: &&benilla_ui::script::ExtractedQuad| {
            s.quad_owner_name(q.target).as_deref() == Some("CreditsFrame")
        };
        let bars: Vec<u128> = quads
            .iter()
            .filter(owned_by_credits)
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: None, color, gradient: None, .. }
                    if *color == black)
            })
            .map(|q| q.z)
            .collect();
        let borders: Vec<u128> = quads
            .iter()
            .filter(owned_by_credits)
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("Credits\\Border1"))
            })
            .map(|q| q.z)
            .collect();
        assert_eq!((bars.len(), borders.len()), (2, 2));
        assert!(
            borders.iter().all(|b| bars.iter().all(|bar| b > bar)),
            "the borders draw over the black bars: {borders:?} over {bars:?}"
        );
    }

    /// The world the glue stands in at the login screens, its console holding `reloadUI`.
    fn glue_world() -> World {
        use crate::console::ConsoleCommandApp;
        let mut app = App::new();
        app.console_command("reloadUI", "", super::super::console_reload_ui);
        let commands = app
            .world_mut()
            .remove_resource::<crate::console::ConsoleCommands>()
            .expect("the registry");
        let mut world = World::new();
        world.insert_resource(commands);
        world.insert_resource(GlueStock(true));
        world.insert_resource(State::new(crate::client_state::ClientState::Login));
        world.init_non_send_resource::<super::super::glue_mgr::SharedGlueMgr>();
        world.init_resource::<GlueRebuild>();
        super::super::lifecycle::setup_script(&mut world);
        world
    }

    fn session(world: &World) -> u64 {
        world.non_send_resource::<UiScript>().session()
    }

    /// `ConsoleExec("reloadUI")` at the login screens reaches the console (`0x515f10`), whose
    /// `reloadUI` asks for a glue rebuild (`0x4035f0`). The glue update spends it in state 0 alone
    /// (`0x46b977`): it builds a new glue (`0x46a7b0(1)`) and signals the screen `SetCurrentScreen`
    /// last named (`0x46b9a7`), the credits here; in the world the glue is suspended and the
    /// request is spent with nothing built (`0x46b98b`).
    #[test]
    fn reload_ui_at_the_login_screens_rebuilds_the_glue_on_its_screen() {
        use super::super::glue_mgr::{ScreenState, SharedGlueMgr};
        use benilla_ui::script::VmKind;
        let _ = benilla_formats::wow_data_or_skip!();
        let mut world = glue_world();
        let set_state = |world: &World, state: ScreenState| {
            world.non_send_resource::<SharedGlueMgr>().get_mut().state = state;
        };
        world
            .non_send_resource::<UiScript>()
            .run("SetGlueScreen('credits') ConsoleExec('reloadUI')")
            .unwrap();
        drain_glue_console(&mut world);
        let old = session(&world);

        set_state(&world, ScreenState::Logon);
        run_pending_glue_rebuild(&mut world);
        assert_eq!(session(&world), old, "a logon in flight holds the rebuild");
        assert!(world.resource::<GlueRebuild>().requested, "and keeps it");

        set_state(&world, ScreenState::Idle);
        run_pending_glue_rebuild(&mut world);
        assert_ne!(session(&world), old, "state 0 rebuilds the glue");
        let s = world.non_send_resource::<UiScript>();
        assert_eq!(s.vm_kind(), VmKind::Glue);
        assert_eq!(s.glue_screen(), "credits");
        assert_eq!(
            s.eval::<(bool, bool)>(
                "return CreditsFrame:IsShown() == 1, AccountLogin:IsShown() == 1"
            )
            .unwrap(),
            (true, false),
            "the rebuilt glue is on the screen it was on"
        );
        assert!(!world.resource::<GlueRebuild>().requested, "spent");
        let rebuilt = session(&world);
        run_pending_glue_rebuild(&mut world);
        assert_eq!(session(&world), rebuilt, "once");

        world.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        world.resource_mut::<GlueRebuild>().request();
        run_pending_glue_rebuild(&mut world);
        assert_eq!(session(&world), rebuilt, "a suspended glue builds nothing");
        assert!(
            !world.resource::<GlueRebuild>().requested,
            "and the request is spent"
        );
    }

    /// The glue's size callback, alone, over a 1024×768 window with `gxResolution` "1024x768",
    /// the glue just built.
    struct SizeCallback {
        app: App,
        win: Entity,
    }

    impl SizeCallback {
        fn new() -> Self {
            let mut app = App::new();
            app.init_resource::<GlueRebuild>()
                .insert_resource(crate::cvars::Cvars::with_value("gxResolution", "1024x768"))
                .add_systems(Update, watch_glue_aspect);
            let mut window = Window::default();
            window.resolution.set(1024.0, 768.0);
            let win = app
                .world_mut()
                .spawn((window, bevy::window::PrimaryWindow))
                .id();
            glue_built(app.world_mut());
            Self { app, win }
        }

        /// `gxResolution` written and committed, as `RestartGx` commits the latched row.
        fn set_gx(&mut self, value: &str) {
            let mut cvars = self.app.world_mut().resource_mut::<crate::cvars::Cvars>();
            cvars.set("gxResolution", value);
            cvars.commit_latched();
        }

        /// One frame with the window at `w`×`h`; whether it asked for a rebuild.
        fn frame(&mut self, w: f32, h: f32) -> bool {
            self.app
                .world_mut()
                .get_mut::<Window>(self.win)
                .unwrap()
                .resolution
                .set(w, h);
            self.app.update();
            let mut r = self.app.world_mut().resource_mut::<GlueRebuild>();
            std::mem::take(&mut r.requested)
        }
    }

    /// A new `gxResolution` aspect rebuilds once, at the resize its `RestartGx` applies
    /// (`0x46abe0` takes its aspect from `0x46ab40`); a move under a hundredth does not
    /// (`0x46ac66`), and the next comparison is from the aspect stored (`0x46ac80`).
    #[test]
    fn a_gx_resolution_aspect_change_rebuilds_the_glue_once() {
        let mut c = SizeCallback::new();
        assert!(!c.frame(1024.0, 768.0), "the size it was built at");
        c.set_gx("1366x768");
        assert!(c.frame(1366.0, 768.0), "4:3 to 16:9 rebuilds");
        assert!(!c.frame(1366.0, 768.0), "once");
        c.set_gx("1360x768");
        assert!(
            !c.frame(1360.0, 768.0),
            "16:9 to 1.7708 moves the stored aspect by under a hundredth"
        );
    }

    /// The window is not the aspect: a drag to any size with `gxResolution` unchanged stores the
    /// size and rebuilds nothing.
    #[test]
    fn a_window_resize_with_gx_resolution_unchanged_rebuilds_nothing() {
        let mut c = SizeCallback::new();
        for (w, h) in [(1366.0, 768.0), (800.0, 900.0), (2560.0, 1080.0)] {
            assert!(!c.frame(w, h), "{w}x{h}");
        }
    }

    /// A height of 0 is no aspect: neither a minimized window nor a `gxResolution` with a zero
    /// height asks for a rebuild.
    #[test]
    fn a_zero_height_rebuilds_nothing() {
        let mut c = SizeCallback::new();
        assert!(!c.frame(1024.0, 0.0), "a minimized window");
        assert!(!c.frame(1024.0, 768.0), "and its restore");
        c.set_gx("1366x0");
        assert!(!c.frame(1366.0, 768.0), "a zero-height gxResolution");
    }

    /// `GlueParent` fills the screen root, 768 tall and `768·a` wide (`0x41ad10`, from `0x46ab40`),
    /// up to 16:9; past that `GlueParent_OnLoad` centres it 16:9 wide on the root's height.
    #[test]
    fn glue_parent_fills_the_root_up_to_sixteen_by_nine() {
        let _ = benilla_formats::wow_data_or_skip!();
        let rect = |aspect: f32| {
            let mut s = UiScript::new_glue().unwrap();
            s.set_screen_size(768.0 * aspect, 768.0);
            let _ = load_glue(&mut s);
            s.resolve();
            s.eval::<(f32, f32, f32, f32)>(
                "return GlueParent:GetLeft(), GlueParent:GetRight(), GlueParent:GetBottom(), \
                 GlueParent:GetTop()",
            )
            .unwrap()
        };
        let near = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
            [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)]
                .iter()
                .all(|(x, y)| (x - y).abs() < 0.01)
        };
        for aspect in [4.0 / 3.0, 16.0 / 10.0, 16.0 / 9.0] {
            let got = rect(aspect);
            assert!(
                near(got, (0.0, 768.0 * aspect, 0.0, 768.0)),
                "{aspect}: {got:?}"
            );
        }
        let wide = 21.0 / 9.0;
        let bar = (768.0 * wide - 768.0 * 16.0 / 9.0) / 2.0;
        let got = rect(wide);
        assert!(
            near(got, (bar, 768.0 * wide - bar, 0.0, 768.0)),
            "21:9: {got:?}"
        );
    }
}
