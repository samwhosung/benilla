//! `benilla`: a from-scratch World of Warcraft 1.12.1 client on Bevy. It opens the patch chain from
//! the install (`$WOW_DATA`, the project folder on a dev build, else beside the binary), streams
//! the world around the player through the `benilla-assets` `mpq://` pipeline, and runs the network
//! session on a background thread ([`net`]).
//!
//! The world is loaded when a character enters it and released when they leave; the glue screens
//! have no world behind them, so with no server the client sits at the login screen, as the
//! reference does. The capture harness (`$WOW_CAPTURE`) boots straight in-world.

// A player build's dead code is the seam working: with `--no-default-features` every symbol only an
// instrument calls goes unused, and a `cfg` per item would spread seam knowledge into gameplay
// modules. Dev builds warn normally.
#![cfg_attr(not(feature = "dev"), allow(dead_code))]

/// The realtime-audio allocation tripwire: in debug builds an allocation inside `sound::output`'s
/// `no_alloc` scopes (the output IO callback, the render pass) aborts the process; release builds
/// carry no wrapper.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

pub mod addon_harness;
mod area;
mod area_poi;
mod area_trigger;
#[cfg(feature = "dev")]
mod asset_churn;
mod aura_visual;
mod bindings;
mod blob_shadow;
mod bowstring;
mod camera_shake;
#[cfg(feature = "dev")]
mod capture;
mod char_create;
mod char_select;
mod chat_bubble;
mod chr_classes;
mod cinematic;
mod combat_log;
mod combat_text;
mod console;
mod crash;
mod creature_anim;
mod cursor;
mod cvars;
mod death;
#[cfg(feature = "dev")]
mod debug_panel;
/// The dev/player seam: the instrument groups and the boundary rule. Always compiled; what it holds
/// is not.
mod dev;
mod doodad_events;
mod entities;
mod fishing_line;
mod footprints;
mod game_plugins;
mod glue;
mod glue_strings;
mod go_anim;
mod go_templates;
#[cfg(feature = "dev")]
mod hover_log;
mod items;
mod loading_screen;
mod local_state;
mod login;
mod minimap;
mod nameplates;
mod names;
mod net;
mod npc_text;
mod pending_item_ops;
/// Ships in part (the FPS journal and the clocks it reads); the rest is `dev`.
mod perf;
mod pipe_warm;
mod player;
mod poi_marker;
mod portrait;
#[cfg(feature = "dev")]
mod preflight;
#[cfg(feature = "dev")]
mod probe_shield;
mod query_cache;
mod quest_markers;
mod raid_marks;
mod ranged_flex;
mod realm_select;
mod realmlist;
mod run_mode;
mod screen_fade;
mod screenshot;
mod shaders;

mod game_tip;
mod name_persist;
mod opaque2d;
/// Where "the client is going down" is observed, in `Last`; every system that persists state on the
/// way out registers through it.
mod shutdown;
mod smart_rect;
mod sound;
/// The two talent spell-modifier tables (`SMSG_SET_FLAT_/PCT_SPELL_MODIFIER`) and the read that
/// puts them on a number.
mod spell;
/// The melee swing refusal's latch + 4 s repeat (`SMSG_ATTACKSWING_*`).
mod swing_refusal;
mod target;
#[cfg(test)]
pub(crate) mod test_support;
mod text_filter;
mod text_reshape;
mod textinput;
mod transport;
mod tutorial;
mod ui_action;
mod ui_auction;
mod ui_aura;
mod ui_bank;
mod ui_battlefield;
mod ui_battlefield_positions;
mod ui_battlefield_score;
mod ui_bind_confirm;
mod ui_binder;
mod ui_cast;
mod ui_char;
mod ui_chat;
mod ui_craft;
mod ui_dialog_verbs;
mod ui_dressup;
mod ui_duel;
mod ui_follow;
mod ui_gamma;
mod ui_gm_ticket;
mod ui_gossip;
mod ui_guild;
mod ui_hide;
mod ui_honor;
mod ui_inspect;
mod ui_instance;
mod ui_item_text;
mod ui_items;
mod ui_layout;
mod ui_logout;
mod ui_loot;
mod ui_loot_roll;
mod ui_macro;
mod ui_mail;
mod ui_merchant;
mod ui_mirror;
mod ui_models;
mod ui_net;
mod ui_party;
mod ui_pass;
mod ui_pet;
mod ui_pet_book;
mod ui_pet_doll;
mod ui_pet_stats;
mod ui_petition;
mod ui_quest;
mod ui_quest_log;
mod ui_quest_share;
mod ui_reputation;
mod ui_saved;
mod ui_script;
mod ui_session;
mod ui_shapeshift;
mod ui_social;
mod ui_spellbook;
mod ui_stable;
mod ui_summon;
mod ui_tabard;
mod ui_talent;
mod ui_talent_wipe;
mod ui_taxi;
mod ui_text;
mod ui_tooltip;
mod ui_trade;
mod ui_tradeskill;
mod ui_trainer;
mod ui_unit;
mod ui_world_map;
mod video;
mod vplates;
mod weapon_trail;
mod world_backdrop;
mod world_state;
mod world_state_ui;

use bevy::prelude::*;

// The `benilla` launcher shim stamps the build id at compile time and hands it to [`run`];
// re-exported so the shim needs no bevy dependency of its own.
pub use benilla_world::build_id::BuildId;
/// The world viewer's entry point, the engine with no game attached, called by the
/// `benilla-worldview` shim.
pub use benilla_world::worldview::run as run_worldview;
pub use bevy::app::AppExit;

/// Builds and runs the client app. `build` is the launcher's compile-time git stamp, passed in as
/// data so the sha lives in the shim's fingerprint and a commit does not recompile this crate.
pub fn run(build: BuildId) -> AppExit {
    // `WOW_HOVER_LOG_REPORT=<csv>` and `WOW_CAPTURE=list` print and exit before any setup.
    if let Ok(path) = std::env::var("WOW_HOVER_LOG_REPORT") {
        dev::report_recorded_hover_log(&path);
        return AppExit::Success;
    }
    if std::env::var("WOW_CAPTURE").as_deref() == Ok("list") {
        dev::print_scenario_names();
        return AppExit::Success;
    }
    // `WOW_PROBE=list` likewise prints the probe environment registry.
    if std::env::var("WOW_PROBE").as_deref() == Ok("list") {
        dev::print_probe_vars();
        return AppExit::Success;
    }

    // From here a panic leaves `benilla-config/Diagnostics/crash-<unix>.txt`; armed before the
    // `App` exists, so a panic while plugins build is reported too.
    crash::install(build);

    let mut app = App::new();
    // The panel footer and the preflight banner read the stamp back.
    app.insert_resource(build);
    // Static-scene transform tracking pinned on: the default threshold re-decides every frame with
    // two full scans of the rows the tracking exists to skip, and this scene is static-heavy. It
    // costs more only when most rows move in one frame (a load burst into a near-empty world).
    app.insert_resource(bevy::transform::systems::StaticTransformOptimizations::enabled());
    // Update runs single-threaded: most per-frame Update systems are non-Send (mlua's `UiScript`,
    // kira's audio handles) and serialize through the multi-threaded executor anyway, so its
    // dispatch is pure overhead (measured faster, tails flat). `WOW_MT_UPDATE=1` restores it.
    if std::env::var_os("WOW_MT_UPDATE").is_none() {
        app.edit_schedule(Update, |s| {
            s.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
        });
    }
    // PostUpdate too: 208 systems paid about 10 µs each of multi-threaded dispatch, and
    // single-threaded measured faster parked and in motion. `WOW_MT_POSTUPDATE=1` restores it; the
    // log line names the config a run measured.
    if std::env::var_os("WOW_MT_POSTUPDATE").is_none() {
        app.edit_schedule(PostUpdate, |s| {
            s.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
        });
        println!(
            "executor: PostUpdate -> single-threaded (1437 default; WOW_MT_POSTUPDATE=1 for MT)"
        );
    }
    // The build banner ships in every build: which build produced a log is the first thing a report
    // from another machine must establish.
    app.add_systems(Startup, benilla_world::build_id::banner);

    // With `$WOW_CAPTURE` set the app runs a deterministic, server-less capture (net off, so no
    // NPCs stream in) and exits.
    let capturing = run_mode::scenario_active();
    // Every instrumented run, captures and live probes, opens in the background so it never takes
    // over a person's screen; `WOW_BG` overrides.
    let background = benilla_world::bgwin::background_run();
    if capturing {
        // Ground clutter scatters with per-run randomness, so captures turn it off for byte-stable
        // baselines; set before plugins build so `ClutterConfig::from_env` reads it. The frame
        // clock, frozen in `capture`, is the other source of drift.
        std::env::set_var("WOW_CLUTTER_DENSITY", "0");
        // Anim-LOD park/wake (`creature_anim::lod::gate_rig_animation`) hangs on asset-load timing,
        // which the frozen clock does not control, so captures turn it off unless overridden. A rig
        // in frame should never be parked, so the shot keeps its subject.
        if std::env::var("WOW_NO_ANIM_LOD").is_err() {
            std::env::set_var("WOW_NO_ANIM_LOD", "1");
        }
    }

    // The `mpq://` source must be registered before `AssetPlugin` (in `DefaultPlugins`) builds. The
    // install is found by `benilla_formats::wow_data`, as everywhere; without one the source is
    // absent and terrain loads fail gracefully.
    match benilla_formats::wow_data() {
        Some(data_dir) => {
            if let Err(e) = benilla_assets::register_mpq_source(&mut app, &data_dir) {
                eprintln!("benilla-assets: mpq:// source unavailable ({e:#})");
            }
        }
        None => eprintln!(
            "benilla: no WoW install found — looked in {:?}",
            benilla_formats::candidates()
        ),
    }

    // No `game://` source: the five UI shaders are compiled in by `crate::shaders`
    // (`embedded://benilla_app/shaders/…`), so no build-machine path reaches the binary.

    app.add_plugins(benilla_world::boot::tuned_default_plugins(Window {
        title: "benilla".into(),
        // Born in the player's display mode (`gxWindow` read straight off `config.toml`) rather
        // than flipped into it at `Startup`, which would flash on every launch and, under
        // gamescope, spend the first second in the input state fullscreen is meant to end.
        // Instrumented runs stay windowed (`video::windowed_env`).
        mode: video::boot_window_mode(),
        // UI-fixture captures size the window to what they photograph: the action bar's 1024 px
        // plus 128 px end caps get a wide, short window, and vplates pins 1024×768, where one gx
        // unit is 1280 px and the plate lands at the border texture's native 128×32, diffable
        // against the BLP.
        resolution: video::at_requested_dpi(
            // Same opt-in as the UI load (`ui_script::lifecycle::ui_wanted`); the two must agree,
            // or the right content is captured at the wrong size.
            if capturing && crate::run_mode::capture_ui_opted_in() {
                // `$WOW_WIN` overrides here too, for scale-dependent UI bugs.
                if let Some(win) = video::requested_window_size() {
                    win
                } else {
                    match std::env::var("WOW_CAPTURE").as_deref() {
                        Ok("ui-actionbar") => UVec2::new(1300, 260),
                        Ok("vplates") => UVec2::new(1024, 768),
                        // Short enough for the action bar to overlap the chat edit box.
                        Ok("ui-chatedit") => UVec2::new(566, 377),
                        // The map's chrome is a centered 1024×768 block, with margin.
                        Ok("ui-worldmap") => UVec2::new(1100, 800),
                        // Margin keeps the 920×724 window's edge tile and close X in frame.
                        Ok("ui-options") | Ok("ui-options-audio") | Ok("ui-options-graphics") => {
                            UVec2::new(1200, 900)
                        }
                        _ => UVec2::new(640, 700),
                    }
                }
                .into()
            } else {
                // `$WOW_WIN=WxH` (logical px) overrides the size. FFXGlow's blur is pinned in
                // texels, so thin bright features self-amplify at high resolution; matching the
                // reference's pixel density (`WOW_WIN=512x288` on a 2× display is 1024×576)
                // isolates that term.
                video::requested_window_size()
                    // A run that reads no pixels gets a small window: it is held `AlwaysOnTop`
                    // against the occlusion throttle (`capture::ProbeFocusPlugin`), and small and
                    // cornered keeps it out of the way. Pixel runs keep the full size.
                    .unwrap_or(if benilla_world::bgwin::no_pixel_run() {
                        UVec2::new(640, 360)
                    } else {
                        // The player's `gxResolution`; `bevy_winit` ignores it while fullscreen.
                        video::boot_windowed_size()
                    })
                    .into()
            },
        ),
        // `$WOW_NOVSYNC=1` uncaps presentation from boot so a headless FPS-journal run measures
        // frame cost, not the vsync ceiling; otherwise the player's `gxVSync` takes over from
        // `Startup` ([`crate::video`]).
        present_mode: video::present_mode(!video::novsync_env()),
        // A background run opens unfocused, so it cannot take keystrokes meant for another app, and
        // is born `AlwaysOnBottom` (`kCGNormalWindowLevel - 1`), so winit's two raises before the
        // first frame cannot flash it on top. `BgWinPlugin` promotes it to Normal once the launch
        // settles, so it can still be raised.
        focused: !background,
        window_level: if background {
            bevy::window::WindowLevel::AlwaysOnBottom
        } else {
            bevy::window::WindowLevel::Normal
        },
        ..default()
    }))
    .add_plugins(benilla_world::thread_qos::ThreadQosPlugin)
    // Undoes winit's forced macOS app activation for background runs; the window-side half is the
    // `Window` above.
    .add_plugins(benilla_world::bgwin::BgWinPlugin)
    // macOS `Cmd+Q` goes straight to `terminate:`, which never runs another frame, so the session
    // was never written; re-pointed at the window close, which [`shutdown`] sees.
    .add_plugins(benilla_world::mac_quit::MacQuitPlugin)
    // The engine as one group; `world_plugins.rs` has its load-bearing ordering edges and what it
    // leaves out (`pipe_warm`).
    .add_plugins(benilla_world::world_plugins::WorldPlugins)
    // The instruments, which the engine group does not carry; `--no-default-features` compiles them
    // all out (see `dev.rs`).
    .add_plugins(dev::DevToolsPlugin)
    // The FPS journal ships: `/console fpsJournal 1` appends a per-second row of position, frame
    // cost and the GPU's per-pass split to `benilla-config/Diagnostics/fps-journal.csv`;
    // `WOW_FPS_JOURNAL=<csv>` is the harness lever.
    .add_plugins(perf::FpsJournalPlugin)
    // The game as one group on top of the engine; `game_plugins.rs` has its members, its
    // load-bearing ordering edges and the test that builds it headless.
    .add_plugins(game_plugins::GamePlugins {
        connect: !capturing,
        start: run_mode::start_state(),
    });

    // benilla-assets' loaders go into the live `AssetServer`, so they register after `AssetPlugin`.
    benilla_assets::register_asset_loaders(&mut app);

    // `ExtractSchedule` runs single-threaded too: bevy_render leaves it multi-threaded, with no
    // non-Send member, and single-threaded measured faster. `WOW_MT_EXTRACT=1` restores it.
    //
    // `Render` stays multi-threaded: under pipelined rendering that executor is also bevy's route
    // for non-Send systems to the main thread, and `create_surfaces` needs it because macOS makes a
    // Metal layer only on the UI thread (the single-threaded executor panics at startup).
    //
    // Set here because the render sub-app exists only once the plugin chain has built.
    if std::env::var_os("WOW_MT_EXTRACT").is_none() {
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.edit_schedule(bevy::render::ExtractSchedule, |s| {
                s.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
            });
            println!(
                "executor: ExtractSchedule -> single-threaded (1437 default; WOW_MT_EXTRACT=1 for MT)"
            );
        } else {
            // A missing sub-app would silently flip nothing.
            eprintln!("executor: no render app — ExtractSchedule flip NOT applied");
        }
    }

    // The probe fleet, last so it observes the fully-built app; compiled out by
    // `--no-default-features`.
    app.add_plugins(dev::DevProbesPlugin);

    // Returns the app's exit status: a failed capture sets `AppExit::error()`
    // (`capture::drive_capture`), which must not exit 0 with no PNG on disk.
    app.run()
}
