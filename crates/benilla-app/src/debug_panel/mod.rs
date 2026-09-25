//! The in-window egui debug panel, floated top-right over the full-screen world and toggled by
//! [`DEV_CHORD`]+`D`. It edits [`DebugState`]; apply systems elsewhere turn that into world
//! changes. [`EguiPointerOver`] is owned by `ui_script` and only written here, so gameplay never
//! names a dev-owned type. egui runs in manual-context mode on its own overlay camera.

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

/// The egui half of the pointer arbitration: written here, owned by `ui_script`.
use crate::ui_script::EguiPointerOver;

mod inspect;
mod journal;

/// The dev state this panel edits; the engine owns and inits it.
use benilla_world::dev_state::DebugState;
use benilla_world::model_render::{blend_index, kind_index};

/// The World section: identity, map, zone and position, and the `.go xyz` copy.
mod world;
use world::{world_section, WorldReadout};

/// The dev overlays' shared backing, near-solid so the text contrast does not depend on the scene.
pub(crate) const OVERLAY_FILL: egui::Color32 = egui::Color32::from_black_alpha(224);
/// Primary overlay text, set as `override_text_color`; an explicit `.color(...)` still wins.
pub(crate) const OVERLAY_TEXT: egui::Color32 = egui::Color32::from_gray(235);
/// De-emphasised overlay text: ids, distances, hints.
pub(crate) const OVERLAY_TEXT_DIM: egui::Color32 = egui::Color32::from_gray(180);

/// Overlay text for the compact fixed-size surfaces: [`OVERLAY_TEXT`], no wrapping, which also
/// stops a fresh auto-sized container breaking a label across lines on its first frame.
pub(crate) fn overlay_text(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.visuals.override_text_color = Some(OVERLAY_TEXT);
    style.wrap_mode = Some(egui::TextWrapMode::Extend);
}

/// Strip Tab from egui's input: it is bound to `TargetNearestEnemy`, and egui would take it as
/// focus navigation and grab the keyboard. Bevy's `ButtonInput<KeyCode>` still sees it.
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

/// The panel's display order for [`ModelKind`].
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

pub(crate) use inspect::MouseoverTarget;

/// Adds the egui plugin, the panel UI, the inspector and the cast journal.
pub struct DebugPanelPlugin;

impl Plugin for DebugPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default());
        // Manual context mode, egui hosted on its own overlay camera: disable auto-creation right
        // after adding the plugin, before its startup system runs.
        {
            let mut egui = app.world_mut().resource_mut::<EguiGlobalSettings>();
            egui.auto_create_primary_context = false;
            // The player hides the OS cursor and draws `Point.blp`, so egui must not drive it.
            egui.enable_cursor_icon_updates = false;
        }

        // The inspector, its mouseover pick and the cast journal. `DebugState`, `InspectMode` and
        // `EguiPointerOver` are inited by their owners.
        app.init_resource::<MouseoverTarget>()
            .init_resource::<journal::CastJournal>()
            // After `UiInput`: `update_mouseover` reads `PointerOverUi`, whose player-UI half it
            // writes, and must see this frame's hover.
            .add_systems(
                Update,
                (inspect::toggle_inspect, inspect::update_mouseover)
                    .chain()
                    .after(crate::ui_script::UiInput),
            )
            // Always recording; messages persist two frames, so no ordering is needed.
            .add_systems(Update, journal::record_casts)
            .add_systems(
                EguiPrimaryContextPass,
                (inspect::inspect_ui, journal::journal_ui),
            )
            .add_systems(Startup, spawn_egui_camera)
            // Between bevy_egui filling `EguiInput` and the pass consuming it, the seam bevy_egui
            // documents for input edits.
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
            // No ordering against `UiInput`: a dev chord cannot be typed text.
            .add_systems(Update, (toggle_panel, gate_egui_lane).chain())
            // A gated frame still owes bevy_egui a prepared empty output, between the end pass it
            // skipped and the output consumer that still runs.
            .add_systems(
                PostUpdate,
                feed_gated_egui_output
                    .after(EguiPostUpdateSet::EndPass)
                    .before(EguiPostUpdateSet::ProcessOutput),
            );
    }
}

/// Feed each gated context an empty output: `run_manually` is not "off", and bevy_egui's
/// `process_output_system` logs an ERROR every frame a context has none. The first feed is a real
/// `Context::run`, because tessellating a context that never began a pass panics `No fonts loaded`.
fn feed_gated_egui_output(
    mut contexts: Query<(&mut EguiContext, &mut EguiFullOutput, &EguiContextSettings)>,
    // The empty pass's output minus its one-time texture delta, fed on every later gated frame
    // instead of running a real pass.
    mut cached: Local<Option<egui::FullOutput>>,
) {
    for (mut ctx, mut full_output, settings) in &mut contexts {
        if settings.run_manually && full_output.0.is_none() {
            full_output.0 = Some(match &*cached {
                Some(out) => out.clone(),
                None => {
                    // `get_mut`: `get` needs bevy_egui's `immutable_ctx` feature. The font
                    // texture delta must reach the GPU exactly once, so the cache drops it.
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

/// The full-window overlay camera hosting the primary egui context, order 2: above the player-UI
/// pass (order 1) and the world camera (order 0), so dev overlays always sit on top.
fn spawn_egui_camera(mut commands: Commands) {
    commands.spawn((
        PrimaryEguiContext, // `#[require(EguiContext)]` ⇒ this camera carries EguiContext
        // Named for [`crate::preflight`]'s MSAA check, which reports cameras by name.
        Name::new("egui dev-overlay camera"),
        Camera2d,
        // Set explicitly: `Camera` requires `Msaa`, default `Sample4`, and Bevy keys the 2D depth
        // texture on the target alone, so this must match the player-UI camera's `Msaa::Off` or a
        // pass fails validation. egui feathers its own edges anyway.
        bevy::render::view::Msaa::Off,
        RenderLayers::none(),
        Camera {
            order: 2,
            // An overlay composites only its own pixels: egui paints a cleared transparent canvas
            // in premultiplied colour, so the blit over the swapchain blends premultiplied
            // (`SrcAlpha` would weight it twice). Loading the shared main texture instead would
            // re-present the player-UI camera's un-decoded frame. It never touches the world
            // image, so writeback is off.
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

/// Demand-gate the egui lane: with the panel closed and inspect off, the primary context goes
/// `run_manually` (bevy_egui skips its pass) and the overlay camera sleeps, so a hidden dev surface
/// costs nothing. `EguiPointerOver` clears on the way down: its writer runs inside the gated pass.
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
    // The dev chord + `D`: a bare key would squat on a bindable game key.
    if dev_chord(&keys, KeyCode::KeyD) {
        debug.open = !debug.open;
    }
}

/// Draw the panel as an overlay on the right over the full-screen world.
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
    // Full height, rather than auto-sized to the open sections.
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
            // Brighten like the overlays but keep wrapping, so labels reflow rather than clip.
            ui.visuals_mut().override_text_color = Some(OVERLAY_TEXT);
            ui.set_width(280.0); // fixed width, no resize handle
            ui.set_height(panel_h); // fill the height; sections scroll within it

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                // Room for the pinned footer: three lines, as a wider font wraps the hotkey map.
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
                            // Doodad animation cost: anim hosts, ticking ones (hidden ones pause),
                            // material-alpha samplers and UV-scrolling materials.
                            let ticking = anim_hosts.iter().filter(|h| h.active).count();
                            // Samplers at 0 are batches the reference culls (`A <= 0`,
                            // `0x707b3a`-`0x707b5c`), such as a voidwalker's death-only geometry;
                            // `dim` counts batches drawn below full strength.
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
                            // WMO portal cull A/B: off, every building group always draws.
                            ui.checkbox(&mut m.portal_cull, "WMO portal visibility cull");
                            // Dumps the seed and per-portal verdicts to a file.
                            if ui.button("dump WMO cull trace").clicked() {
                                // Into the diagnostics folder; the world crate has no
                                // `local_state`, and a hermetic run has no folder at all.
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
                            // Time of day drives the Light.dbc colours and the sun's arc.
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
                                    // Exact entry: drag by one minute, or type a minute or `HH:MM`.
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
                            // 0..255 bytes, for eyedropping against the reference, and a swatch.
                            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as i32;
                            let swatch = |ui: &mut egui::Ui, name: &str, c: [f32; 3]| {
                                let (r, g, b) = (byte(c[0]), byte(c[1]), byte(c[2]));
                                let col = egui::Color32::from_rgb(r as u8, g as u8, b as u8);
                                // The label is tinted the resolved colour, as its own swatch.
                                ui.colored_label(col, format!("{name}  [{r}, {g}, {b}]"));
                            };
                            swatch(ui, "ambient (row 1)", lighting.ambient);
                            swatch(ui, "diffuse (row 0)", lighting.diffuse);
                            swatch(ui, "specular (row 9)", lighting.spec);
                            // The fog colour (row 7) is also `ClearColor` behind the dome.
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
                            // The two ramped channels.
                            if let Some(ws) = weather_state.as_ref() {
                                ui.strong(format!(
                                    "{:?}  ·  effect {:.2}  ·  sky {:.2}  ·  storm blend {:.2}",
                                    ws.effect_kind,
                                    ws.effect_density,
                                    ws.sky_density,
                                    benilla_world::weather::storm_blend(ws.sky_density),
                                ));
                            }
                            // The `weatherDensity` CVar (Weather Intensity 0-3), scaling spawn
                            // gain through the `0x67b870` quality table.
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
                            // The override drives the same apply path as the wire.
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
                            ui.checkbox(&mut sound_cfg.limiter, "output limiter");
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

                    // Connection state and every opcode the codec ignored or failed to parse.
                    egui::CollapsingHeader::new("Net")
                        .default_open(false)
                        .show(ui, |ui| {
                            // The last sample, not the meter's ring average.
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
                    // The step-up probe's last blocked-frame report, live, so a run can be seen
                    // to fire at the intended spot.
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
                });

            // The pinned footer: the dev-surface hotkey map.
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{DEV_CHORD}:  D panel · P perf · I inspect · M mute · F free-fly"
                ))
                    .small()
                    .color(OVERLAY_TEXT_DIM),
            );
            // The build; a click copies the full sha.
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
