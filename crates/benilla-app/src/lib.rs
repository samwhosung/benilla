//! `benilla` — a from-scratch World of Warcraft 1.12.1 client on Bevy, talking to a local vmangos server.
//!
//! Opens the vanilla patch chain from wherever the install is (`$WOW_DATA`, the project folder on a
//! dev build, else beside the binary — decision 1175) and streams the world around the
//! player through Bevy's `AssetServer` (the `benilla-assets` `mpq://` pipeline): ADT terrain tiles within
//! the live Terrain Distance window (decision 1513) — their splat-blended ground, doodads/WMOs, water,
//! and ground clutter — plus the avian colliders the character controller walks on. Lit by a time-of-day WoW
//! lighting model (`Light.dbc` sampled against the server clock) with a sky dome, sun/moon discs, and
//! distance fog; a faithful `EffectGlow` bloom on top.
//!
//! In parallel a background thread ([`net`]) logs in (`$WOW_USER`/`$WOW_PASS`/`$WOW_HOST`, host
//! default `localhost`), enters the world, and streams object updates. NPCs and GameObjects
//! render as their real models (resolved from the display id via the creature/GameObject catalogs); other
//! players stay cyan cubes, and our own avatar is blue until we take third-person control of it.
//! **The world is loaded when a character enters it, and released when they leave** — the glue
//! screens have no world behind them (decision 0777). With no server there is no world: the client
//! sits at the login screen, which is what the real one does. The scene harness (`$WOW_CAPTURE`)
//! boots straight in-world and is unaffected.
//!
//! Controls: WASD walks the avatar; right-drag turns it, left-drag orbits the camera (both
//! hide/freeze the cursor while held), scroll wheel zooms. Those are all **bindings** now (0997) —
//! rebindable, and nothing in the client squats on a bare key beside them (1043). The dev chord
//! (`Ctrl`+`Shift`) + `F` toggles free-fly, then WASD flies the camera with Space up / C down and
//! `Ctrl` boosts — a boost that exists only inside free-fly, itself behind the chord.

// **A player build's dead code is the seam working, not a defect.** With `--no-default-features`
// every symbol whose only caller is an instrument goes unused — the camera-park seam, the aim
// seam, the pool-slot identity, a dozen accessors the panel and the probes read. Warning about
// each one would bury a REAL warning in the one build nobody looks at daily, and the alternative
// (a `cfg` attribute per item) would spread seam knowledge back across the gameplay modules 1174
// spent its whole diff clearing it out of. Dev builds still warn normally.
#![cfg_attr(not(feature = "dev"), allow(dead_code))]

/// The realtime-audio allocation tripwire (decision 1857). In debug builds an allocation inside
/// the output IO callback or the render pass (`sound::output`'s `no_alloc` scopes) aborts the
/// process, so a realtime-safety regression fails a test or a smoke instead of reaching an ear;
/// release builds carry no wrapper at all.
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
/// **The dev/player seam** (decisions 0026/1173, built in 1174) — the group, and the boundary
/// rule, in one file. Always compiled; what it *holds* is not.
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
/// Ships in part: the FPS journal and the clocks it reads (2008); the rest is `dev` (1173).
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
/// Where "the client is going down" may be observed, and why that is `Last` and not `Update`
/// (decision 1528). Every system that persists state on the way out registers through it.
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

// The `benilla` launcher shim (the bin package) is this library's only caller: it stamps the
// build id at compile time and hands it into [`run`]. Re-exported so the shim needs no bevy
// dep of its own.
pub use benilla_world::build_id::BuildId;
/// The world viewer's entry point — the engine with no game attached (decision 1160).
/// Its shim (`benilla-worldview`) is this library's second caller; see [`worldview`].
pub use benilla_world::worldview::run as run_worldview;
pub use bevy::app::AppExit;

/// Build and run the client app. `build` is the launcher shim's compile-time git stamp
/// ([`build_id`]) — passed in as plain data so the sha lives in the shim's fingerprint, not
/// this crate's, and a commit stops recompiling the app (decision 0993).
pub fn run(build: BuildId) -> AppExit {
    // `WOW_CAPTURE=list` just prints the harness scenario names (the source of truth `scripts/visual.sh`
    // reads) and exits before any window/asset setup.
    // `WOW_HOVER_LOG_REPORT=<csv>` re-reads a recorded run and prints its report, then exits —
    // no window, no game. New analysis lands on runs already captured (see `hover_log`).
    if let Ok(path) = std::env::var("WOW_HOVER_LOG_REPORT") {
        dev::report_recorded_hover_log(&path);
        return AppExit::Success;
    }
    if std::env::var("WOW_CAPTURE").as_deref() == Ok("list") {
        dev::print_scenario_names();
        return AppExit::Success;
    }
    // `WOW_PROBE=list` prints the probe fleet's environment registry (`capture::probe_env`) the
    // same way — before any window, so a session can ask the binary what the fleet takes.
    if std::env::var("WOW_PROBE").as_deref() == Ok("list") {
        dev::print_probe_vars();
        return AppExit::Success;
    }

    // From here on a panic leaves `benilla-config/Diagnostics/crash-<unix>.txt` behind (decision
    // 2266 §B2) — armed before the `App` exists, so a panic while plugins build is a report too.
    crash::install(build);

    let mut app = App::new();
    // The stamp is plain data from here on — the panel footer and preflight banner read it back.
    app.insert_resource(build);
    // Pin Bevy's static-scene transform tracking ON (decision 1356). At the default threshold
    // `mark_dirty_trees` re-decides per frame by counting every changed-Transform row and every
    // tree row — two full scans of the very population the tracking exists to skip. This scene
    // is provably static-heavy (terrain/WMO batches and parked units barely move), so the
    // auto-tuner can only ever confirm what this line states. The guard it removes bites only
    // when MOST rows move in one frame (a load burst into a near-empty world), where
    // dirty-marking briefly costs more than it saves — a loading-band term, accepted knowingly.
    app.insert_resource(bevy::transform::systems::StaticTransformOptimizations::enabled());
    // The Update schedule runs on the SINGLE-THREADED executor (decision 1366). Not a tuning
    // whim: ~60% of our per-frame Update systems are non-Send (mlua's UiScript, kira's audio
    // handles) and serialize through the multi-threaded executor's one `local_thread_running`
    // flag anyway, so the cross-thread dispatch machinery was pure overhead — measured
    // −1.30 cpu_ms at the LBRS pin under 5-round grading with the wall tail (p95/p99/max)
    // flat-to-better (the 1364 "fatter tail" reading did not reproduce; 1366 has the tables,
    // both pins). `WOW_MT_UPDATE=1` is the A/B lever back to the multi-threaded executor.
    if std::env::var_os("WOW_MT_UPDATE").is_none() {
        app.edit_schedule(Update, |s| {
            s.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
        });
    }
    // PostUpdate too (decision 1437, closing 1366's named open probe — which expected a
    // REGRESSION here and was refuted by measurement). The census: 208 systems paying
    // ~10 µs/system of MT dispatch parked (schedule self 2.09 ms/f in the 1435 band map) while
    // the wide bands the 1366 expectation leaned on are exactly the ones later work gated or
    // pinned (1429 idle-gates the animation par sweep, 1356 pins static-transform tracking).
    // Graded −0.49 cpu_ms parked (all 5 rounds negative) and winning IN MOTION as part of the
    // combo (−0.76/−0.80 across two 4-round LBRS walk sittings), parked tails flat, motion
    // p99/max flipping sign wholesale between sittings on both executors — 1366's own
    // noise-dominated shape. `WOW_MT_POSTUPDATE=1` is the lever back; the engagement line makes
    // every leg log name the config it measured.
    if std::env::var_os("WOW_MT_POSTUPDATE").is_none() {
        app.edit_schedule(PostUpdate, |s| {
            s.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
        });
        println!(
            "executor: PostUpdate -> single-threaded (1437 default; WOW_MT_POSTUPDATE=1 for MT)"
        );
    }
    // …and one startup line says which build produced this log. Registered HERE, beside the stamp
    // it prints, rather than in `preflight` where it sat until decision 1179: **which build is
    // this** is the first thing a bug report from someone else's machine has to establish, and a
    // player build is the one whose logs always come from someone else's machine. Gating it out
    // with the instruments got the argument exactly backwards (`preflight`'s own module doc makes
    // the case; 1174 moved the file without re-reading it).
    app.add_systems(Startup, benilla_world::build_id::banner);

    // Visual A/B harness (decision 0008): with `$WOW_CAPTURE` set, the app runs a deterministic,
    // server-less capture (net off so no NPCs stream in nondeterministically) and exits. See `capture`.
    let capturing = run_mode::scenario_active();
    // Any instrumented run — captures AND the live-probe fleet — opens in the background so it
    // never fights the director's screen (decision 0703; `WOW_BG` overrides). See `bgwin`.
    let background = benilla_world::bgwin::background_run();
    if capturing {
        // Ground clutter scatters with per-run randomness, so disable it for byte-stable baselines
        // — clutter isn't what the lighting rework validates, and the regression diff must not be
        // masked by grass wobble. Set before plugins build so `ClutterConfig::from_env` reads it.
        // It is not the only source of per-run drift, though it was long documented as such: the
        // other is the frame clock itself, frozen in `capture` (decision 0723).
        std::env::set_var("WOW_CLUTTER_DENSITY", "0");
        // Third source of per-run drift, and the one the lighting matrix (decision 0746) hit: the
        // anim-LOD park/wake gate (`creature_anim::lod::gate_rig_animation`). Whether a rig is
        // parked, and which pose it wakes into, depends on when its model finished loading relative
        // to the frustum/room evaluation — asset-load timing, which the frozen clock does not
        // control. MEASURED: the seeded wolf lands in one of exactly two poses, the pair always
        // MAE 4.123 apart, and three runs with the gate off are bit-identical.
        //
        // Off for captures, unless explicitly overridden. A still frame is meant to contain only
        // rigs that are IN view, and a rig in view is one the gate should never park — so what a
        // capture loses is the gate's own correctness, not the shot's subject. That loss is named,
        // not free: a rig wrongly parked in frame stays invisible to the sweep, and the deeper
        // question the measurement raises — why a woken rig does not converge on the absolute-clock
        // pose the gate promises — is open in 0746 and worth its own hunt.
        if std::env::var("WOW_NO_ANIM_LOD").is_err() {
            std::env::set_var("WOW_NO_ANIM_LOD", "1");
        }
    }

    // The `mpq://` asset source must be registered BEFORE `AssetPlugin` (inside `DefaultPlugins`)
    // builds. Finds the install the one way anything does (`benilla_formats::wow_data`, decision
    // 1175) — the same answer the `WorldAssets` foundation gets. On failure the source is simply
    // absent and the AdtTile-pipeline loads fail gracefully (the terrain just doesn't appear).
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

    // NOTE: there is deliberately no `game://` asset source here (decision 1175). 1171 gave the
    // game's five UI shaders their own source pointed at this crate's `assets/` — with the path
    // baked from `CARGO_MANIFEST_DIR`, so it named the build machine's source tree and resolved
    // to nothing anywhere else. The line 1171 drew survives: those five are still this crate's,
    // now compiled in by `crate::shaders` and addressed `embedded://benilla_app/shaders/…`,
    // because `embedded_asset!` is per-crate by construction. Only the directory is gone.

    app.add_plugins(benilla_world::boot::tuned_default_plugins(Window {
        title: "benilla".into(),
        // **Born in the player's display mode, not flipped into it** (decision 1627). `gxWindow`
        // is read straight off `config.toml` here rather than waiting for `Startup`'s
        // `load_config`, because a launch that opens windowed and goes fullscreen one frame later
        // is a visible flash on every start — and, under a compositor that only maps a fullscreen
        // surface 1:1 (gamescope), a first second spent in the exact input state this is meant to
        // end. Every instrumented run stays windowed regardless (`video::windowed_env`), so the
        // capture harness, the probe fleet and `$WOW_WIN` are untouched by any of this.
        mode: video::boot_window_mode(),
        // UI-fixture captures shrink the window so the docked panel fills the frame — the capture
        // is the look-pass instrument and the window is its subject. The action bar is the
        // exception: it spans 1024px + 128px end caps along the screen bottom, so it gets a wide,
        // short window instead of the tall default (else the caps crop). The vplates scenario pins
        // the 1:1 gx window: at 1024×768 one gx unit = 1280 px, so the plate must land at the
        // border texture's native 128×32 — directly diffable against the decoded BLP. Sized
        // per-capture off WOW_CAPTURE.
        resolution: video::at_requested_dpi(
            // Same opt-in as the UI load itself (`ui_script::lifecycle::ui_wanted`): a scenario
            // that declares a `ui:` fixture sizes its window for the window it photographs, with
            // or without the env var. The two must agree — a UI loaded into a world-sized window
            // is a capture of the right content at the wrong size.
            if capturing && crate::run_mode::capture_ui_opted_in() {
                // `$WOW_WIN` overrides here too — the resolution-A/B instrument for UI scenarios (a
                // scale-dependent text bug looks fine at the scenario's default size and truncates at
                // fullscreen heights).
                if let Some(win) = video::requested_window_size() {
                    win
                } else {
                    match std::env::var("WOW_CAPTURE").as_deref() {
                        Ok("ui-actionbar") => UVec2::new(1300, 260),
                        Ok("vplates") => UVec2::new(1024, 768),
                        // The director's small-window shape: short enough that the action bar strip
                        // overlaps the chat edit box rows — the overlap is the subject.
                        Ok("ui-chatedit") => UVec2::new(566, 377),
                        // The fullscreen map's chrome is a centered 1024×768 block; a hair of margin
                        // shows the blackout doing its job.
                        Ok("ui-worldmap") => UVec2::new(1100, 800),
                        // The 920×724 era options window wants air on every side so the straddling
                        // right-edge tile and the hung close X stay in frame.
                        Ok("ui-options") | Ok("ui-options-audio") | Ok("ui-options-graphics") => {
                            UVec2::new(1200, 900)
                        }
                        _ => UVec2::new(640, 700),
                    }
                }
                .into()
            } else {
                // `$WOW_WIN=WxH` (logical px): override the world capture/window size — the
                // resolution-A/B instrument. The FFXGlow blur geometry is byte-pinned in TEXELS, so its
                // angular footprint shrinks as resolution grows and thin bright features (fence rails)
                // self-amplify at 4K where the 1024-era reference diluted them; matching the era's
                // pixel density (e.g. `WOW_WIN=512x288` on a 2× display → 1024×576 physical) isolates
                // that term. Also the knob for any future era-resolution comparison.
                video::requested_window_size()
                    // A run that reads no pixels gets a SMALL window. It is held `AlwaysOnTop` for its
                    // whole life so it can never be occluded into the ~1 fps throttle
                    // (`capture::ProbeFocusPlugin`, decision 0906) — at the full default that meant
                    // every agent probe planted a screen-filling window over the director's work. Small
                    // + cornered (`ProbeFocusPlugin` parks it) is un-occludable AND out of the way;
                    // anything photographing pixels keeps the full size, and `WOW_WIN` overrides either
                    // way (decision 1148).
                    .unwrap_or(if benilla_world::bgwin::no_pixel_run() {
                        UVec2::new(640, 360)
                    } else {
                        // The player's `gxResolution` — what "windowed" means for them, and what
                        // leaving fullscreen restores (1627). Its default is the 1600×900 that was
                        // hard-coded here before, and while `mode` above is fullscreen `bevy_winit`
                        // ignores this entirely (it applies an inner size only on the `Windowed` arm).
                        video::boot_windowed_size()
                    })
                    .into()
            },
        ),
        // The boot present mode. `$WOW_NOVSYNC=1` uncaps presentation so a headless FPS-journal
        // run measures true frame cost, not the vsync ceiling — the same uncap the capture probe
        // flips mid-run, available from boot for non-capture probes (perf triage at the glue
        // screens, where no capture scenario runs). Absent it, we boot synced and the player's
        // `gxVSync` setting takes over from `Startup` on ([`crate::video`]).
        present_mode: video::present_mode(!video::novsync_env()),
        // An instrumented run must never fight the director's screen (decision 0703).
        // Focused, it steals the keyboard — on 2026-07-19 a login-shot run swallowed
        // their keystrokes out of another app and typed them into the account box,
        // which is also how that capture lost the bare caret it was taken to measure.
        // So every probe/capture/regression run (`bgwin`) opens unfocused; an ordinary
        // `cargo run` is unaffected and focuses normally.
        //
        // A background run is BORN at `AlwaysOnBottom` (`kCGNormalWindowLevel - 1`)
        // so it can never flash over their work on the way up — winit raises a new
        // window twice before our first frame runs, and at the normal level that
        // showed as ~half a second of probe window on top (measured). But it does not
        // STAY there: that level is a cage, and 0703 leaving it on for the whole run
        // is why an instrumented window could never be raised again however hard you
        // clicked it. `BgWinPlugin` promotes it back to Normal the moment the launch
        // settles (decision 0709), and owns the app-level half — winit's forced macOS
        // app activation — as well.
        focused: !background,
        window_level: if background {
            bevy::window::WindowLevel::AlwaysOnBottom
        } else {
            bevy::window::WindowLevel::Normal
        },
        ..default()
    }))
    .add_plugins(benilla_world::thread_qos::ThreadQosPlugin)
    // The app-side half of background instrumented runs: undo winit's forced macOS app
    // activation so a probe/capture launch never yanks focus off the director's screen. The
    // window-side half (unfocused + always-on-bottom) is in the `Window` above. Decision 0703.
    .add_plugins(benilla_world::bgwin::BgWinPlugin)
    // The other winit-default the client has to undo (decision 1528): macOS's `Cmd+Q` is wired
    // straight to `terminate:`, which never runs another frame — so the gesture a Mac player
    // reaches for first exited without writing one line of their session. Re-pointed at the
    // window close, which the shutdown tail in [`shutdown`] already sees.
    .add_plugins(benilla_world::mac_quit::MacQuitPlugin)
    // **The engine, as one name** (decision 1164). Everything `benilla-world` will own,
    // in the order both binaries used before this group existed — see `world_plugins.rs`
    // for the two ordering edges inside it that are load-bearing, and for what is
    // deliberately left out (`pipe_warm`).
    .add_plugins(benilla_world::world_plugins::WorldPlugins)
    // The instruments, which the engine group deliberately does not carry (1160: instruments at
    // the top of the stack). The panel first — `PerfPlugin` needs the egui plugin/context it sets
    // up. Toggles: the dev chord + D, and P.
    // **The instruments** (decisions 1173/1174): the debug panel, the perf HUD, the object
    // inspector, the hover-cost recorder, the asset-churn meter, the session preflight and the
    // probe shield — one group, in the slot the panel has always held (it sets up the egui
    // context the perf pill needs). `--no-default-features` compiles every one of them out; see
    // `dev.rs` for what is in the group and the one rule that governs the boundary.
    .add_plugins(dev::DevToolsPlugin)
    // The FPS journal — the one instrument that ships (2008): `/console fpsJournal 1` in any
    // build appends a per-second row of position, frame cost and the GPU's per-pass split to
    // `benilla-config/Diagnostics/fps-journal.csv`; `WOW_FPS_JOURNAL=<csv>` is the harness lever.
    .add_plugins(perf::FpsJournalPlugin)
    // **The game, as one name** (decision 2279): everything the client adds on top of the
    // engine, in the order it always had. See `game_plugins.rs` for the members, the ordering
    // edges inside it that are load-bearing, and the test that builds it headless.
    .add_plugins(game_plugins::GamePlugins {
        connect: !capturing,
        start: run_mode::start_state(),
    });

    // Register benilla-assets' loaders AFTER `AssetPlugin` (they go into the live `AssetServer`).
    benilla_assets::register_asset_loaders(&mut app);

    // The render app's `ExtractSchedule` runs SINGLE-THREADED too (decision 1437, same grading
    // as the PostUpdate flip above): bevy_render never sets an executor kind, so it ran the
    // multi-threaded one by default — 0.34 ms/f of schedule self in the 1435 parked band map
    // over 165 systems (census) with zero true non-Send members. Graded −0.26 parked alone,
    // −0.90 parked as the combo. `WOW_MT_EXTRACT=1` is the lever back.
    //
    // The `Render` schedule stays MULTI-threaded, and not as a tuning judgment (1437, measured
    // the hard way): under pipelined rendering the MT executor is ALSO bevy's non-Send→main-
    // thread routing, and `bevy_render::view::window::create_surfaces` is non-Send precisely to
    // ride it — on macOS a Metal layer can only be made on the UI thread, and the ST executor
    // runs everything on the render thread (`get_metal_layer cannot be called in non-ui thread`,
    // a startup panic). One system pins the schedule; its 1.06 ms/f self stays on the table
    // until that coupling changes upstream.
    //
    // Lives HERE, not beside its siblings: the render sub-app only exists once the plugin chain
    // has built (pipelining detaches it at cleanup, later still).
    if std::env::var_os("WOW_MT_EXTRACT").is_none() {
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.edit_schedule(bevy::render::ExtractSchedule, |s| {
                s.set_executor_kind(bevy::ecs::schedule::ExecutorKind::SingleThreaded);
            });
            println!(
                "executor: ExtractSchedule -> single-threaded (1437 default; WOW_MT_EXTRACT=1 for MT)"
            );
        } else {
            // A silently missing sub-app would flip nothing — say so instead of measuring a ghost.
            eprintln!("executor: no render app — ExtractSchedule flip NOT applied");
        }
    }

    // **The probe fleet** — the capture harness and every scripted live probe, each armed by its
    // own environment variable and inert without it. Added last so they observe the fully-built
    // app; compiled out entirely by `--no-default-features` (decisions 1173/1174), which is why
    // this is one line and the twenty env checks behind it live in `dev.rs`.
    app.add_plugins(dev::DevProbesPlugin);

    // Return the app's own exit status instead of dropping it: a failed capture writes
    // `AppExit::error()` (see `capture::drive_capture`), and discarding it made the process exit 0
    // with no PNG on disk — which is how a sweep carried on around a missing shot (decision 0743).
    app.run()
}
