//! Standardized in-window debug panel (egui).
//!
//! The panel is a rounded, translucent **window floated off the top-right corner** (a gap from the
//! edges, like the perf pill) — it overlays the 3D view, which renders full-screen underneath and is
//! never letterboxed. Fixed width (no resize handle). [`DEV_CHORD`]+`D` shows/hides it (1043, 1048).
//! Its look (translucent dark backing + crisp text) is shared with the perf pill and the inspector card
//! via [`OVERLAY_FILL`] / [`overlay_text`], so the dev overlays read as one family.
//!
//! ## How it's wired (so future subsystems slot in cleanly)
//! - [`DebugState`] is one resource grouped into per-subsystem sections ([`ModelDebug`],
//!   [`LightingDebug`], …; new subsystems add sibling fields).
//! - The panel UI ([`debug_panel_ui`]) reads/writes that resource via egui widgets.
//! - Small **apply** systems turn the resource into world changes (e.g. [`apply_model_visibility`]).
//!   Each only does work when its slice of the state actually changes (tracked with a `Local`
//!   snapshot), so leaving the panel open costs nothing. (Lighting is now resolved in `lighting.rs`;
//!   this section is time controls + a readout, not knobs.)
//! - [`EguiPointerOver`] publishes "the mouse is talking to the egui overlays" each pass; both it
//!   and the combined source of truth gameplay reads ([`crate::ui_script::PointerOverUi`]) are
//!   *defined* by that combiner, and this module only writes them (decisions 0026/1174 — dev
//!   plugins must be droppable without breaking gameplay reads, so gameplay may not name a
//!   dev-owned type).
//!
//! Adding a section for a new subsystem is therefore: a `FooDebug` field, a few widgets in the
//! panel, and one `apply_foo` system — no plumbing changes. (Weather is the worked example.)
//!
//! Rendering uses bevy_egui's manual-context mode: auto-creation is disabled in [`DebugPanelPlugin`]
//! and a dedicated full-window overlay camera composites egui over the finished frame (a transparent
//! canvas, premultiplied over the swapchain). See bevy_egui's `side_panel` example.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::CameraOutputMode;
use bevy::prelude::*;
use bevy::render::render_resource::BlendState;
use bevy_egui::{
    egui, EguiContext, EguiContextSettings, EguiContexts, EguiFullOutput, EguiGlobalSettings,
    EguiInput, EguiPlugin, EguiPostUpdateSet, EguiPreUpdateSet, EguiPrimaryContextPass,
    PrimaryEguiContext,
};

use benilla_assets::LockRecover;
use benilla_world::lighting::{ClockSource, GameClock, WowLighting};
use benilla_world::model_render::{ModelKind, ModelPart};
use benilla_world::modkeys::{dev_chord, DEV_CHORD};

/// The egui half of the pointer arbitration: this panel *writes* it, `ui_script` owns it —
/// so a build without these overlays still compiles gameplay's reads (decision 1174; the
/// module-doc note above). `InspectMode`, the other half of 0026's named pair, lives there too
/// and is imported by the two files that touch it (`inspect`, `journal`).
use crate::ui_script::EguiPointerOver;

mod inspect;
mod journal;

/// The dev state this panel edits — resource-only, faithful defaults (decision 0026: the
/// always-present config layer; this module is only its editor). The engine owns and inits it,
/// and the per-frame model-visibility apply system that reads its toggles is `model_render`'s.
use benilla_world::dev_state::DebugState;
use benilla_world::model_render::{blend_index, kind_index};

/// The World section — identity/map/zone/position readout + the copy-`.go xyz` affordance.
mod world;
use world::{world_section, WorldReadout};

/// Shared look for the floating dev overlays — this panel, the perf pill, the inspector card — so they
/// read as one family. The backing is a **near-solid dark** so the bright text keeps strong contrast
/// *no matter the scene behind it*: a lightly-translucent box let bright terrain/sky bleed through and
/// washed the text out, making legibility depend on the view. [`overlay_text`] sets the bright primary
/// tone; [`OVERLAY_TEXT_DIM`] the readable secondary one. (Lower the alpha for a more see-through box —
/// at the cost of view-independent contrast.)
pub(crate) const OVERLAY_FILL: egui::Color32 = egui::Color32::from_black_alpha(224);
/// Primary overlay text — a crisp near-white that reads over the translucent fill (egui's dark default
/// is only `gray(140)`). Set as the ui's `override_text_color`; explicit `.color(...)` calls still win.
pub(crate) const OVERLAY_TEXT: egui::Color32 = egui::Color32::from_gray(235);
/// De-emphasised overlay text (ids, distances, hints): dimmer than [`OVERLAY_TEXT`] but still crisp on
/// the translucent backing.
pub(crate) const OVERLAY_TEXT_DIM: egui::Color32 = egui::Color32::from_gray(180);

/// Apply the shared overlay text treatment for the **compact, fixed-size** surfaces (perf pill,
/// inspector card): brighten to [`OVERLAY_TEXT`] and disable wrapping. No-wrap also kills the
/// first-frame layout flash where a fresh auto-sized container hasn't cached its width yet and a label
/// like "60 fps" briefly breaks across two lines. (The resizable debug panel brightens the same way but
/// keeps wrapping, so its labels reflow instead of clipping when narrowed.)
pub(crate) fn overlay_text(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.visuals.override_text_color = Some(OVERLAY_TEXT);
    style.wrap_mode = Some(egui::TextWrapMode::Extend);
}

/// Strip the **Tab** key from egui's per-frame input so egui never treats it as focus navigation.
///
/// Tab is a bound game key — `TargetNearestEnemy` (decision 0166). Left to itself, egui reads Tab at
/// `begin_pass` (`Focus::begin_pass`) and pulls keyboard focus into whatever focusable widget is on
/// screen — the always-on perf pill — ringing it and then owning the keyboard. Our egui surfaces are
/// mouse-driven dev overlays with no tab-to-next-field need, so we drop Tab before the pass sees it.
/// Bevy's own `ButtonInput<KeyCode>` (what `target::scan` reads) is populated independently from the
/// same winit events, so TAB targeting still fires — this only removes the phantom egui focus.
fn strip_egui_tab_focus(mut inputs: Query<&mut EguiInput>) {
    for mut input in &mut inputs {
        input.events.retain(|e| {
            !matches!(
                e,
                egui::Event::Key {
                    key: egui::Key::Tab,
                    ..
                }
            )
        });
    }
}

fn track_pointer_over_ui(mut contexts: EguiContexts, mut over: ResMut<EguiPointerOver>) -> Result {
    let ctx = contexts.ctx_mut()?;
    over.0 = ctx.is_pointer_over_area() || ctx.wants_pointer_input();
    Ok(())
}

/// The panel's display order for [`ModelKind`], and the label it writes. Free functions rather
/// than an inherent `impl`: the type is engine vocabulary now (`model_render`) and an inherent impl
/// cannot follow it across the crate boundary decision 1160 is drawing — the *panel's* opinion
/// about how to present it belongs to the panel either way.
const MODEL_KINDS: [ModelKind; 4] = [
    ModelKind::Doodad,
    ModelKind::Wmo,
    ModelKind::Creature,
    ModelKind::GameObject,
];

fn kind_label(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Doodad => "Doodads (trees/props)",
        ModelKind::Wmo => "WMOs (buildings)",
        ModelKind::Creature => "Creatures (NPCs)",
        ModelKind::GameObject => "GameObjects",
    }
}

const BLEND_LABELS: [&str; 5] = [
    "Opaque (trunk / walls)",
    "AlphaTest (leaf / cutout)",
    "Blend (transparent)",
    "Mod (multiply)",
    "Mod2x (2x multiply / sheen)",
];

/// Minute-of-day (`0..1440`) as `HH:MM`.
fn hhmm(minute: u32) -> String {
    format!("{:02}:{:02}", minute / 60 % 24, minute % 60)
}

fn source_label(s: ClockSource) -> &'static str {
    match s {
        ClockSource::Server => "server clock",
        ClockSource::Manual => "manual scrub",
        ClockSource::Fallback => "noon (not connected)",
    }
}

/// Adds the egui plugin, the debug-state resource, the panel UI, and the apply systems.
pub(crate) use inspect::MouseoverTarget;

pub struct DebugPanelPlugin;

impl Plugin for DebugPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default());
        // Manual context mode: we host egui on a dedicated overlay camera (below) so we can inset the
        // *world* camera's viewport without clipping the panel. Disable right after adding the plugin
        // (the resource exists by then) — before the auto-create system runs at startup.
        {
            let mut egui = app.world_mut().resource_mut::<EguiGlobalSettings>();
            egui.auto_create_primary_context = false;
            // Don't let egui drive the OS cursor icon. The player hides the OS cursor and draws its
            // own `Point.blp` sprite, so egui's per-hover `CursorIcon::System` inserts are pointless
            // churn (and a dev panel doesn't need resize/text cursors).
            egui.enable_cursor_icon_updates = false;
        }

        // `DebugState` itself is `benilla_world::dev_state`'s and the engine inits it — this panel is only
        // its editor (decision 0026), and eight other subsystems read it whether or not the panel
        // is installed.
        // The inspector surface and the cast journal — the two instruments that stood on
        // `interact` and were registered by it until decision 1160's stage zero. The mouseover
        // pick comes with them: it never ran except while the inspector was armed, so it was never
        // the engine's picking, only this overlay's. (`InspectMode` and `EguiPointerOver`, which
        // this panel writes, are inited by their owner `UiScriptPlugin` — 1174.)
        app.init_resource::<MouseoverTarget>()
            .init_resource::<journal::CastJournal>()
            // After the UI keyboard feed because `update_mouseover` reads `PointerOverUi`, whose
            // player-UI half `UiInput` writes — the pick must see this frame's hover, not last
            // frame's. (`toggle_inspect` itself no longer needs the ordering: its dev chord can't be
            // typed text, so it reads no keyboard-capture flag — decision 0585.)
            .add_systems(
                Update,
                (inspect::toggle_inspect, inspect::update_mouseover)
                    .chain()
                    .after(crate::ui_script::UiInput),
            )
            // Always recording (messages persist two frames — no ordering constraint needed).
            .add_systems(Update, journal::record_casts)
            .add_systems(
                EguiPrimaryContextPass,
                (inspect::inspect_ui, journal::journal_ui),
            )
            .add_systems(Startup, spawn_egui_camera)
            // Keep Tab out of egui: it's a bound game key, never focus-navigation for our overlays.
            // Runs after bevy_egui fills `EguiInput` and before egui's pass consumes it (the seam
            // bevy_egui documents for input edits).
            .add_systems(
                PreUpdate,
                strip_egui_tab_focus
                    .after(EguiPreUpdateSet::ProcessInput)
                    .before(EguiPreUpdateSet::BeginPass),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (debug_panel_ui, track_pointer_over_ui),
            )
            // No ordering against the UI keyboard feed: the toggle is a dev chord (1043), which no
            // focused EditBox can consume and which needs no capture gate — the same reason `perf`'s
            // and `sound`'s toggles never needed one.
            .add_systems(Update, (toggle_panel, gate_egui_lane).chain())
            // The gate's other half: a gated frame still owes bevy_egui a prepared (empty) pass —
            // between the end-pass it skipped and the output consumer that doesn't (1452).
            .add_systems(
                PostUpdate,
                feed_gated_egui_output
                    .after(EguiPostUpdateSet::EndPass)
                    .before(EguiPostUpdateSet::ProcessOutput),
            );
    }
}

/// The 1445 gate's missing half (decision 1452). Upstream, `run_manually` means "the app will run
/// the egui pass itself this frame" — never "off": `process_output_system` still `take()`s an
/// output from every context every frame and logs at ERROR when none was prepared (bevy_egui
/// 0.39 and 0.41 alike, `output.rs`). With the lane gated that was one ERROR per frame — a red
/// herring beside any real failure, and gate-fatal severity for a by-design idle state (ERROR
/// means "the client is broken": 1450).
///
/// So a gated frame *runs the empty pass itself* — `Context::run` with default input and no UI,
/// exactly what the error message prescribes for manual mode. Zero widgets means zero shapes:
/// the consumer tessellates nothing, uploads nothing, and the sleeping camera draws nothing —
/// the lane keeps costing what 1445 bought, and bevy_egui's output contract holds. A context
/// that genuinely ran (gate open, or a true manual runner) has `Some` output by now and is left
/// alone.
///
/// Not hand-built `FullOutput::default()`: `process_output_system` tessellates whatever it is
/// given, and tessellating a context that never began a pass panics `No fonts loaded` (measured
/// here — fonts only initialize inside a real begin-pass). `run` is the initialization path.
fn feed_gated_egui_output(
    mut contexts: Query<(&mut EguiContext, &mut EguiFullOutput, &EguiContextSettings)>,
    // The empty pass's output after its first run, with the one-time texture delta (the font
    // atlas) already delivered: every later gated frame feeds this clone instead of running a
    // real begin/end pass — memory GC, a fresh `FullOutput`, a tessellation of nothing —
    // 1.4 % of a parked frame's main thread for a panel that is closed (decision 1979).
    mut cached: Local<Option<egui::FullOutput>>,
) {
    for (mut ctx, mut full_output, settings) in &mut contexts {
        if settings.run_manually && full_output.0.is_none() {
            full_output.0 = Some(match &*cached {
                Some(out) => out.clone(),
                None => {
                    // `get_mut`, not `get`: the immutable getter sits behind bevy_egui's
                    // `immutable_ctx` feature, off in our build. The first run is fed whole
                    // (its texture delta carries the fonts); the cache keeps everything but
                    // that delta, which must reach the GPU exactly once.
                    let out = ctx.get_mut().run(egui::RawInput::default(), |_| {});
                    let mut keep = out.clone();
                    keep.textures_delta = Default::default();
                    keep.shapes.clear();
                    *cached = Some(keep);
                    out
                }
            });
        }
    }
}

/// A full-window overlay camera that hosts the primary egui context and composites it over the 3D
/// scene (higher order, alpha blend, no clear). Renders no world geometry (`RenderLayers::none`).
/// Order 2 — above the player-UI quad pass (`ui_pass::PlayerUiPlugin`, order 1), which itself sits
/// above the order-0 world camera. Per decision 0025's overlay arbitration ("dev overlays composite
/// over" everything else) and 0068 §2: dev overlays always stay on top of the player UI.
fn spawn_egui_camera(mut commands: Commands) {
    commands.spawn((
        PrimaryEguiContext, // `#[require(EguiContext)]` ⇒ this camera carries EguiContext
        // Named so [`crate::preflight`]'s MSAA check can say WHICH cameras disagree — an
        // entity id in that error is a lookup, a name is the answer (decision 1659).
        Name::new("egui dev-overlay camera"),
        Camera2d,
        // **No MSAA — and saying so is what keeps the panel openable at all** (decision 1659).
        // Silence here does not mean "none": `Camera` requires `Msaa`, whose `Default` is
        // `Sample4`, so this camera used to carry four samples without ever naming them. That was
        // invisible while the player-UI camera silently carried four too — and became a fatal
        // `Attachments have differing sample counts` the moment 1628 gave that one `Msaa::Off`,
        // because Bevy's two prepare passes disagree about whether MSAA is part of a texture's
        // identity: `prepare_view_targets` keys the COLOUR target on `(target, usage, hdr, msaa)`,
        // while `core_2d::prepare_core_2d_depth_textures` keys the DEPTH texture on the target
        // ALONE (`core_3d`'s equivalent does include it — the asymmetry is 2D-only). Two
        // `Camera2d`s on one window then share one depth texture at whichever sample count the
        // first-iterated camera stamped on it, and the other one's pass is invalid.
        //
        // Off is also simply right here: egui feathers its own edges in the tessellator, so
        // multisampling this overlay buys nothing and costs a full-window 4× colour texture and 4×
        // the fill — the same arithmetic as 1628. [`crate::preflight`] now checks the agreement
        // every run, so the next camera that stays silent is named in the log instead of in a wgpu
        // validation error that names no camera at all.
        bevy::render::view::Msaa::Off,
        RenderLayers::none(),
        Camera {
            order: 2,
            // **An overlay composites only its own pixels** — the law the player-UI camera's
            // clear already states. egui paints onto a transparent canvas, and the output blit
            // lays that canvas over the finished frame in the swapchain. bevy_egui's own pass
            // blends PREMULTIPLIED (`egui::Color32` is premultiplied, and its pipeline says so),
            // so the canvas holds premultiplied colour with coverage in alpha, and the blit has
            // to compose it as such — `SrcAlpha` would weight it by alpha twice.
            //
            // It used to load the shared main texture instead (`ClearColorConfig::None`), which
            // happened to hold the player-UI camera's decoded frame: two cameras on one window
            // share bevy's main textures, and that camera's decode flipped them. The blit then
            // re-emitted the whole frame over itself. Since decision 2206 the player-UI camera
            // writes the swapchain directly and leaves its main texture un-decoded, so an
            // overlay that loaded it would present the UI ~2.2× bright whenever the panel was
            // open. Clearing is what an overlay should have done all along; and, like the
            // player-UI camera, it never touches the world image, so writeback is off.
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            msaa_writeback: bevy::camera::MsaaWriteback::Off,
            ..default()
        },
    ));
}

/// Demand-gate the whole egui lane (decision 1445): when no dev overlay is open — panel closed,
/// inspect off (the perf HUD's pill is quads on the player-UI pass and never needs this lane;
/// 1453/1454) — the primary context goes `run_manually` (which
/// `bevy_egui`'s context-pass loop honors by skipping it outright: no begin/end pass, no
/// tessellate, no `EguiPrimaryContextPass` run) and the overlay camera sleeps (no Core2d graph
/// run, no composite, no render-side prep). A hidden dev surface then costs what a player build
/// pays — nothing; before the gate the lane drew an EMPTY overlay for ~0.33 traced ms/frame
/// (the 1445 trace). One-frame lag behind the toggles, invisible on a chord press.
///
/// `EguiPointerOver` clears on the way down: its writer ([`track_pointer_over_ui`]) lives inside
/// the gated pass and would hold the last hover forever.
fn gate_egui_lane(
    debug: Res<DebugState>,
    inspect: Res<crate::ui_script::InspectMode>,
    mut cams: Query<(&mut Camera, &mut EguiContextSettings), With<PrimaryEguiContext>>,
    mut over: ResMut<EguiPointerOver>,
) {
    let open = debug.open || inspect.enabled;
    for (mut cam, mut settings) in &mut cams {
        if cam.is_active != open {
            cam.is_active = open;
            if !open && over.0 {
                over.0 = false;
            }
        }
        if settings.run_manually == open {
            settings.run_manually = !open;
        }
    }
}

fn toggle_panel(keys: Res<ButtonInput<KeyCode>>, mut debug: ResMut<DebugState>) {
    // The dev chord + `D` (decision 1048). It was a *bare* backtick until 1043 — backtick reads as
    // "not a game key", but it is one ([`crate::bindings::chord`] gives it the token `` ` ``, so the
    // reference's binding UI can bind it like any other), which made a bare toggle here exactly the
    // squat 0585 moved the perf HUD off `P` for. 1043 put it on the chord; `` ` `` is a bad key to
    // hold a chord on, so it became a letter like the rest of the fleet. The chat-bar/EditBox gate
    // the bare key needed is gone either way: a chord can't be mistaken for typed text.
    if dev_chord(&keys, KeyCode::KeyD) {
        debug.open = !debug.open;
    }
}

/// Draw the panel as a translucent **overlay** on the right — the world renders full-screen
/// underneath (no viewport inset). `ui_script::PointerOverUi` keeps the cursor's panel
/// interactions from leaking into gameplay mouse-look.
fn debug_panel_ui(
    mut contexts: EguiContexts,
    stamp: Res<benilla_world::build_id::BuildId>,
    mut debug: ResMut<DebugState>,
    clock: Res<GameClock>,
    lighting: Res<WowLighting>,
    parts: Query<&ModelPart>,
    anim_hosts: Query<&benilla_world::doodad_anim::DoodadAnimHost>,
    mat_anims: Query<&benilla_world::doodad_anim::MatAnim>,
    uv_mats: Res<benilla_world::doodad_anim::UvAnimMaterials>,
    mut sound_cfg: ResMut<crate::sound::SoundConfig>,
    mut cull_probe: ResMut<benilla_world::wmo_portal::WmoCullProbe>,
    net_status: Res<crate::net::NetStatus>,
    ping: Res<crate::net::PingShared>,
    dropped: Res<crate::net::DroppedOpcodes>,
    mut weather_state: Option<ResMut<benilla_world::weather::WeatherState>>,
    mut world: WorldReadout,
) -> Result {
    if !debug.open {
        return Ok(());
    }

    // Live counts per layer/type.
    let mut kind_counts = [0u32; 4];
    let mut blend_counts = [0u32; 5];
    for p in &parts {
        kind_counts[kind_index(p.kind)] += 1;
        blend_counts[blend_index(p.blend)] += 1;
    }

    let ctx = contexts.ctx_mut()?;
    // Full height: the window spans the view (an 8 px gap top and bottom), rather than auto-sizing to
    // however much content the open sections happen to have.
    let panel_h = (ctx.content_rect().height() - 32.0).max(120.0);
    egui::Window::new("benilla_debug")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .movable(false)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .frame(
            egui::Frame::NONE
                .fill(OVERLAY_FILL)
                .corner_radius(5.0)
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show(ctx, |ui| {
            // Brighten to match the overlays; keep egui's default wrapping so labels reflow, not clip.
            ui.visuals_mut().override_text_color = Some(OVERLAY_TEXT);
            ui.set_width(280.0); // fixed width — no resize handle
            ui.set_height(panel_h); // fill the height; sections scroll within it

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                // Leave room under the scroll for the pinned footer below (hotkey map + build id).
                // Three lines' worth: the footer keeps the panel's default wrapping, so a font
                // whose metrics run wider than ours reflows the hotkey map instead of having its
                // second line clipped away.
                .max_height(panel_h - 56.0)
                .show(ui, |ui| {
                    // Disjoint borrows of the sections so each can drive its own widgets.
                    let DebugState {
                        models: m,
                        lighting: l,
                        sound: s,
                        weather: w,
                        ..
                    } = &mut *debug;
                    // WHERE AM I — the `.gps` of the panel: identity, map, zone, position, tile
                    // residency, and the click-to-copy `.go xyz` teleport line that feeds the
                    // headless probes (`WOW_PROBE_CHAT`) and the FPS-journal loop.
                    egui::CollapsingHeader::new("World")
                        .default_open(true)
                        .show(ui, |ui| world_section(ui, &mut world));

                    egui::CollapsingHeader::new("Models")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.strong("Layer (blend mode)");
                            for i in 0..BLEND_LABELS.len() {
                                ui.checkbox(
                                    &mut m.blend_visible[i],
                                    format!("{}  ·  {}", BLEND_LABELS[i], blend_counts[i]),
                                );
                            }

                            ui.add_space(6.0);
                            ui.strong("Type");
                            for k in MODEL_KINDS {
                                ui.checkbox(
                                    &mut m.kind_visible[kind_index(k)],
                                    format!("{}  ·  {}", kind_label(k), kind_counts[kind_index(k)]),
                                );
                            }
                            // The doodad-animation cost meter (decision 0130): how many placed
                            // doodads carry an anim host, how many are ticking right now (the
                            // draw gate pauses hidden ones), how many batches sample a
                            // material-alpha loop (phase 2), and how many materials scroll their
                            // UVs (phase 3 — waterfalls).
                            let ticking = anim_hosts.iter().filter(|h| h.active).count();
                            // Of the material samplers, how many resolve to **0 right now** — the
                            // batches the reference culls this frame (`A <= 0`,
                            // `0x707b3a`–`0x707b5c`). Non-zero as soon as a voidwalker/banshee/
                            // slime/infernal is in view: those models author geometry that only
                            // appears on death, and this counter is what says we are hiding it
                            // rather than drawing it. `dim` counts the partial factors — a batch
                            // drawn, but not at full strength.
                            let (mut hidden, mut dim) = (0usize, 0usize);
                            for m in &mat_anims {
                                if m.current <= 0.0 {
                                    hidden += 1;
                                } else if m.current < 1.0 {
                                    dim += 1;
                                }
                            }
                            ui.label(format!(
                                "animated doodads  ·  {} ({} ticking)  ·  {} material                                  ({hidden} culled, {dim} dimmed)  ·  {} uv",
                                anim_hosts.iter().count(),
                                ticking,
                                mat_anims.iter().count(),
                                uv_mats.0.len(),
                            ));

                            ui.add_space(6.0);
                            // WMO portal cull A/B (decision 0031): off ⇒ every building group always
                            // draws (the cathedral reappears from the Trade District).
                            ui.checkbox(&mut m.portal_cull, "WMO portal visibility cull");
                            // The cull probe (decision 0022): a one-click full trace dump — stand where a
                            // room vanishes, click, and the exact seed evidence + per-portal verdicts land
                            // in a file.
                            if ui.button("dump WMO cull trace").clicked() {
                                // Under the one folder (0954/1486), beside the crash reports and
                                // the stall samples. The world crate is handed the path because
                                // it has no `local_state` of its own; a hermetic run has no
                                // folder, and the click says so instead of writing into the cwd.
                                match crate::local_state::diagnostics_dir() {
                                    Some(dir) => {
                                        cull_probe.dump_to = Some(dir.join("wmo-cull-trace.txt"));
                                    }
                                    None => bevy::log::warn!(
                                        "wmo cull trace: no benilla-config folder to write into \
                                         (hermetic run) — set WOW_CULLDUMP=<path> to name one"
                                    ),
                                }
                            }
                        });

                    egui::CollapsingHeader::new("Lighting")
                        .default_open(false)
                        .show(ui, |ui| {
                            // Time of day — drives the DBC-sampled colors + the sun's day-arc.
                            ui.strong(format!(
                                "Time of day: {}  ({})",
                                hhmm(clock.minute),
                                source_label(clock.source),
                            ));
                            ui.checkbox(&mut l.follow_server_time, "follow server clock");
                            ui.add_enabled_ui(!l.follow_server_time, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::Slider::new(&mut l.manual_minute, 0..=1439)
                                            .text("scrub time")
                                            .custom_formatter(|n, _| hhmm(n as u32)),
                                    );
                                    // Exact per-minute entry: drag/scroll ±1 min, or click to type a raw
                                    // minute (0..1439) or an `HH:MM` time — for landing on an exact moment.
                                    ui.add(
                                        egui::DragValue::new(&mut l.manual_minute)
                                            .range(0..=1439)
                                            .speed(1.0)
                                            .custom_formatter(|n, _| hhmm(n as u32))
                                            .custom_parser(|s| {
                                                let s = s.trim();
                                                if let Some((h, m)) = s.split_once(':') {
                                                    let h: u32 = h.trim().parse().ok()?;
                                                    let m: u32 = m.trim().parse().ok()?;
                                                    Some(((h % 24) * 60 + (m % 60)) as f64)
                                                } else {
                                                    s.parse::<f64>().ok()
                                                }
                                            }),
                                    );
                                });
                            });

                            ui.add_space(6.0);
                            ui.strong("Resolved (Light.dbc, this time)");
                            // 0..255 bytes (eyedrop-friendly vs the reference client) + a swatch.
                            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as i32;
                            let swatch = |ui: &mut egui::Ui, name: &str, c: [f32; 3]| {
                                let (r, g, b) = (byte(c[0]), byte(c[1]), byte(c[2]));
                                let col = egui::Color32::from_rgb(r as u8, g as u8, b as u8);
                                // The label text is tinted the resolved color (a swatch the font always
                                // renders); the [r,g,b] are the exact 0..255 values for eyedrop A/Bs.
                                ui.colored_label(col, format!("{name}  [{r}, {g}, {b}]"));
                            };
                            swatch(ui, "ambient (row 1)", lighting.ambient);
                            swatch(ui, "diffuse (row 0)", lighting.diffuse);
                            swatch(ui, "specular (row 9)", lighting.spec);
                            // The backdrop / fog colour (row 7) — what `ClearColor` shows behind the dome.
                            swatch(ui, "fog / backdrop (row 7)", lighting.fog_color);
                            let d = lighting.sun_dir;
                            ui.label(
                                egui::RichText::new(format!(
                                    "sun dir  ({:+.2}, {:+.2}, {:+.2})",
                                    d.x, d.y, d.z
                                ))
                                .weak(),
                            );
                            ui.add_space(8.0);
                            ui.checkbox(&mut l.disable_fog, "disable distance fog");
                            ui.checkbox(&mut l.disable_sky_dome, "disable sky dome");
                        });

                    egui::CollapsingHeader::new("Weather")
                        .default_open(false)
                        .show(ui, |ui| {
                            // Live state readout (the two ramped channels, decision 0302).
                            if let Some(ws) = weather_state.as_ref() {
                                ui.strong(format!(
                                    "{:?}  ·  effect {:.2}  ·  sky {:.2}  ·  storm blend {:.2}",
                                    ws.effect_kind,
                                    ws.effect_density,
                                    ws.sky_density,
                                    benilla_world::weather::storm_blend(ws.sky_density),
                                ));
                            }
                            // The `weatherDensity` setting (video-options Weather Intensity 0–3)
                            // — the real game setting, not a tuning knob. Scales the rain/snow/
                            // mist spawn gain via the `0x67b870` quality table; default 3 matches
                            // the reference install's Config.wtf.
                            if let Some(ws) = weather_state.as_mut() {
                                let mut wd = ws.weather_density;
                                ui.horizontal(|ui| {
                                    ui.label("weather intensity");
                                    for v in 0..=3u8 {
                                        ui.selectable_value(&mut wd, v, format!("{v}"));
                                    }
                                });
                                if wd != ws.weather_density {
                                    ws.weather_density = wd;
                                }
                            }
                            // The override scrub — drives the same apply path as the wire.
                            let before = (w.force, w.kind, w.grade.to_bits(), w.instant);
                            ui.checkbox(&mut w.force, "override server weather");
                            ui.add_enabled_ui(w.force, |ui| {
                                ui.horizontal(|ui| {
                                    for (v, name) in
                                        [(0, "fine"), (1, "rain"), (2, "snow"), (3, "sand")]
                                    {
                                        ui.selectable_value(&mut w.kind, v, name);
                                    }
                                });
                                ui.add(egui::Slider::new(&mut w.grade, 0.0..=1.0).text("grade"));
                                ui.checkbox(&mut w.instant, "instant (skip the ramp)");
                            });
                            if before != (w.force, w.kind, w.grade.to_bits(), w.instant) {
                                w.dirty = true;
                            }
                        });

                    // The object inspector is its own dev-chord `I` surface (see `interact.rs`), not a
                    // section here — identifying a thing shouldn't need the whole panel open.
                    egui::CollapsingHeader::new("Sound")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.checkbox(&mut sound_cfg.enabled, "enabled");
                            ui.checkbox(&mut sound_cfg.muted, format!("muted ({DEV_CHORD}+M)"));
                            ui.add(
                                egui::Slider::new(&mut sound_cfg.master, 0.0..=1.0).text("master"),
                            );
                            ui.add(egui::Slider::new(&mut sound_cfg.sfx, 0.0..=1.0).text("sfx"));
                            ui.add(
                                egui::Slider::new(&mut sound_cfg.music, 0.0..=1.0).text("music"),
                            );
                            ui.add(
                                egui::Slider::new(&mut sound_cfg.ambience, 0.0..=1.0)
                                    .text("ambience"),
                            );
                            ui.checkbox(&mut sound_cfg.limiter, "output limiter (1551)");
                            ui.separator();
                            ui.label("kit probe: a SoundEntries id or name");
                            ui.text_edit_singleline(&mut s.kit_query);
                            ui.add(
                                egui::Slider::new(&mut s.play_copies, 1..=16)
                                    .text("copies, one frame"),
                            );
                            if ui.button("Play kit").clicked() {
                                s.play_kit = true;
                            }
                        });

                    // The wire-coverage section: connection state + the dropped-packet tally
                    // (every opcode the codec ignored or failed to parse, by count — a silent
                    // gap in wire coverage made visible; decision 0022's instrument disposition).
                    egui::CollapsingHeader::new("Net")
                        .default_open(false)
                        .show(ui, |ui| {
                            // The LAST sample, not the ring average the meter shows: the panel
                            // is the instrument, and "what did the most recent pong measure" is
                            // the question a stuck or spiking meter needs answered.
                            let last_rtt = ping.0.lock_recover().last_rtt_ms;
                            ui.label(if net_status.connected {
                                match last_rtt {
                                    Some(ms) => format!("connected · {ms} ms ping"),
                                    None => "connected".to_string(),
                                }
                            } else {
                                format!(
                                    "disconnected — {}",
                                    net_status
                                        .last_reason
                                        .as_deref()
                                        .unwrap_or("never connected")
                                )
                            });
                            ui.add_space(4.0);
                            ui.strong("dropped packets (opcode · count)");
                            if dropped.0.is_empty() {
                                ui.label("none — every received opcode parsed");
                            } else {
                                let mut rows: Vec<_> = dropped.0.iter().collect();
                                rows.sort_by_key(|(_, t)| {
                                    std::cmp::Reverse(t.unknown + t.unparseable)
                                });
                                for (op, t) in rows {
                                    let name =
                                        benilla_protocol::messages::opcode_name(*op).unwrap_or("?");
                                    let mut line =
                                        format!("{op:#06x} {name} · {}", t.unknown + t.unparseable);
                                    if t.unparseable > 0 {
                                        line.push_str(&format!(
                                            "  ({} parse-errored)",
                                            t.unparseable
                                        ));
                                    }
                                    ui.label(line);
                                }
                            }
                        });
                    // The step-up probe's last blocked-frame report (`crate::player::step_probe`).
                    // On the panel *because a capture has to be steerable*: the director walks into
                    // the kerb that will not climb and watches the `t=` stamp tick and the ladder
                    // fill in, instead of finding out afterwards that the probe never fired at the
                    // spot they meant (method §6 — prove the run before reading the result).
                    egui::CollapsingHeader::new("Step-up")
                        .default_open(false)
                        .show(ui, |ui| {
                            let (report, at) = crate::player::step_probe::latest();
                            if report.is_empty() {
                                ui.label(
                                    egui::RichText::new(
                                        "no blocked walk frame yet — walk square into the thing \
                                         that will not step up",
                                    )
                                    .color(OVERLAY_TEXT_DIM),
                                );
                                return;
                            }
                            ui.label(
                                egui::RichText::new(format!("last fired t={at:.1}s"))
                                    .small()
                                    .color(OVERLAY_TEXT_DIM),
                            );
                            egui::ScrollArea::horizontal()
                                .id_salt("stepup_report")
                                .show(ui, |ui| {
                                    for line in report {
                                        ui.label(
                                            egui::RichText::new(line).small().monospace(),
                                        );
                                    }
                                });
                        });
                    // Future sections add their own CollapsingHeader here.
                });

            // The pinned footer: the dev-surface hotkey map, so the other overlays are
            // discoverable from the one surface people find first.
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{DEV_CHORD}:  D panel · P perf · I inspect · M mute · F free-fly"
                ))
                    .small()
                    .color(OVERLAY_TEXT_DIM),
            );
            // …and which build this is (`benilla_world::build_id`). Bottom line of the surface a reader is
            // already on when something looks wrong: a click copies the full sha, so "it looks like
            // this here" can name the code it happened on.
            let build = ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("build {}", stamp.summary()))
                        .small()
                        .color(OVERLAY_TEXT_DIM),
                )
                .sense(egui::Sense::click()),
            );
            if !stamp.sha.is_empty()
                && build
                    .on_hover_text(format!("{}\n(click to copy)", stamp.sha))
                    .clicked()
            {
                ui.ctx().copy_text(stamp.sha.to_string());
            }
        });
    Ok(())
}
