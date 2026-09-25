//! The dev/player seam and its two instrument groups. Dev may see anything; nothing may depend on
//! dev. The player build (`cargo build -p benilla --no-default-features` in `scripts/gates.sh`)
//! holds the direction: a fact gameplay needs from an instrument goes to [`crate::run_mode`], never
//! through a `use` from gameplay.
//!
//! Not here, because they ship: `benilla_world::dev_state` (its defaults are the player behaviour),
//! `pipe_warm` (without it a player on macOS eats every synchronous pipeline stall) and `art_scope`
//! (engine, it travels with `WorldPlugins`).

use bevy::prelude::*;

/// `WOW_CAPTURE=list`: prints the harness scenario names `scripts/visual.sh` reads, before any
/// window or asset setup; nothing in a player build.
pub(crate) fn print_scenario_names() {
    #[cfg(feature = "dev")]
    crate::capture::print_scenario_names();
}

/// `WOW_PROBE=list`: prints the probe environment registry (`capture::probe_env`) before any window
/// or asset setup; nothing in a player build.
pub(crate) fn print_probe_vars() {
    #[cfg(feature = "dev")]
    crate::capture::probe_env::print();
}

/// `WOW_HOVER_LOG_REPORT=<csv>`: prints a recorded hover-log run's report, with no window or game.
pub(crate) fn report_recorded_hover_log(_path: &str) {
    #[cfg(feature = "dev")]
    crate::hover_log::report_recorded_file(_path);
}

/// The instruments, debug panel first: `PerfPlugin` needs the egui plugin and context it sets up.
pub(crate) struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    #[cfg_attr(not(feature = "dev"), allow(unused_variables))]
    fn build(&self, app: &mut App) {
        #[cfg(feature = "dev")]
        {
            app.add_plugins(crate::debug_panel::DebugPanelPlugin)
                .add_plugins(crate::perf::PerfPlugin)
                // `WOW_FX_CENSUS=1`: where particle draws are addressed, and whether their view is
                // on.
                .add_plugins(crate::capture::fx_draw_census_plugin)
                // `WOW_HOVER_LOG` and `WOW_ASSET_CHURN`: no-ops without their variable.
                .add_plugins(crate::hover_log::HoverLogPlugin)
                .add_plugins(crate::asset_churn::AssetChurnPlugin)
                // A banner per world entry naming the body, warning on states that invalidate a
                // reading (dead, GM mode, server-blocked movement); never env-gated.
                .add_plugins(crate::preflight::PreflightPlugin)
                // A probe account's body gets vmangos `.cheat god` (damage stops at 1 hp) and GM
                // mode off on every world entry; inert on any other account.
                .add_plugins(crate::probe_shield::ProbeShieldPlugin);
        }
    }
}

/// The probe fleet: the capture harness and every scripted live probe, each inert without its own
/// environment variable. Added last so they observe the fully-built app.
pub(crate) struct DevProbesPlugin;

impl Plugin for DevProbesPlugin {
    #[cfg_attr(not(feature = "dev"), allow(unused_variables))]
    fn build(&self, app: &mut App) {
        #[cfg(feature = "dev")]
        {
            // `$WOW_CAPTURE`: one deterministic screenshot, then exit.
            if crate::run_mode::scenario_active() {
                app.add_plugins(crate::capture::CapturePlugin);
            }
            // `WOW_LIVE_SHOT=<png>`: one live-run screenshot at `WOW_LIVE_SHOT_AT` s (default 12).
            if std::env::var("WOW_LIVE_SHOT").is_ok() {
                app.add_plugins(crate::capture::LiveShotPlugin);
            }
            // `WOW_RIG="tauren druid 60 gear:heal-preraid-bis"`: finds or creates that body on the
            // probe account, logs in as it and applies level, spells, gear, spec and place.
            if std::env::var("WOW_RIG").is_ok() {
                app.add_plugins(crate::capture::ProbeRigPlugin);
            }
            // A covered macOS window drops to about 1 fps and probe schedules are wall-clock, so
            // these keep the window un-occludable: the `wall_clock` column of
            // `capture::probe_env::PROBE_VARS` plus four instruments. `WOW_LIVE_FPS` needs it from
            // the first tick, or an occluded settle under-warms the scene.
            if crate::capture::probe_env::wall_clock_vars()
                .chain([
                    "WOW_RIG",
                    "WOW_LIVE_FPS",
                    // On an occluded window a burst captures one stale drawable over and over.
                    "WOW_LIVE_SHOT",
                    "WOW_PICK",
                ])
                .any(|k| std::env::var(k).is_ok())
            {
                app.add_plugins(crate::capture::ProbeFocusPlugin);
            }
            // `WOW_PROBE_CHAT=".go xyz …"`: sends GM or chat lines once in-world.
            if std::env::var("WOW_PROBE_CHAT").is_ok() {
                app.add_plugins(crate::capture::ProbeChatPlugin);
            }
            // `WOW_PROBE_LUA`: runs a chunk in the live UI VM once per world entry, across relogs.
            if std::env::var("WOW_PROBE_LUA").is_ok() {
                app.add_plugins(crate::capture::ProbeLuaPlugin);
            }
            // `WOW_PROBE_DRAG="A>B"`: drags frame A onto B through the real pointer path.
            if std::env::var("WOW_PROBE_DRAG").is_ok() {
                app.add_plugins(crate::capture::ProbeDragPlugin);
            }
            // `WOW_PROBE_HOVER="A;B;…"`: crosses each frame's centre by the real pointer path.
            if std::env::var("WOW_PROBE_HOVER").is_ok() {
                app.add_plugins(crate::capture::ProbeHoverPlugin);
            }
            // `WOW_PROBE_KEY="Space@14"`: presses keys once in-world.
            if std::env::var("WOW_PROBE_KEY").is_ok() {
                app.add_plugins(crate::capture::ProbeKeyPlugin);
            }
            // `WOW_PROBE_EXIT_AT=<secs>`: bounds any scripted live probe's lifetime.
            if std::env::var("WOW_PROBE_EXIT_AT").is_ok() {
                app.add_plugins(crate::capture::ProbeExitPlugin);
            }
            // `WOW_PICK="<x>,<y>"`: every surface on the ray through a pixel, nearest first.
            if std::env::var("WOW_PICK").is_ok() {
                app.add_plugins(crate::capture::PickProbePlugin);
            }
            // `WOW_PHASE=<uniqueId>`: per frame, the render phase and draw order of one placement.
            if std::env::var("WOW_PHASE").is_ok() {
                app.add_plugins(crate::capture::PhaseProbePlugin);
            }
            // `WOW_DEPTH="<x>,<y>"`: the winning depth at each pixel, in yards; `WOW_DEPTH_QUADS`
            // takes it at a particle quad's own pixels.
            if std::env::var("WOW_DEPTH").is_ok() || std::env::var("WOW_DEPTH_QUADS").is_ok() {
                app.add_plugins(crate::capture::DepthProbePlugin);
            }
            // `WOW_NODE_PROBE`: which bevy_ui node owns a rectangle outside the FrameXML quad pass.
            if std::env::var("WOW_NODE_PROBE").is_ok() {
                app.add_plugins(crate::capture::NodeProbePlugin);
            }
            // `WOW_PROBE_RESIZE="<secs>:<W>x<H>"`: a mid-run resize, standing in for fullscreen.
            if std::env::var("WOW_PROBE_RESIZE").is_ok() {
                app.add_plugins(crate::capture::ProbeResizePlugin);
            }
            // `WOW_PARTICLE_CENSUS=<secs>`: per-emitter live counts, once.
            if std::env::var("WOW_PARTICLE_CENSUS").is_ok() {
                app.add_plugins(crate::capture::ParticleCensusPlugin);
            }
            // `WOW_GROUND_CENSUS`: per nearby unit, the server's Z, our drawn Z, the floor above.
            if std::env::var("WOW_GROUND_CENSUS").is_ok() {
                app.add_plugins(crate::capture::GroundCensusPlugin);
            }
            // `WOW_LIFT_CENSUS`: per type-11/15 transport, its arm stage, cycle and visibility.
            if std::env::var("WOW_LIFT_CENSUS").is_ok() {
                app.add_plugins(crate::capture::LiftCensusPlugin);
            }
            // `WOW_STALL="<ms>[,<every_s>[,<after_s>]]"`: blocks the main loop on a schedule to
            // stage a tab-away; `0` only runs the frame-delta and occlusion monitor.
            if std::env::var("WOW_STALL").is_ok() {
                app.add_plugins(crate::capture::StallPlugin);
            }
            // `WOW_TRAIL_CENSUS`: each ribbon streak's world extent; a rider on a moving deck must
            // draw a short one, since its edges are stored on the deck.
            if std::env::var("WOW_TRAIL_CENSUS").is_ok() {
                app.add_plugins(crate::capture::TrailCensusPlugin);
            }
            // `WOW_UNIT_VISUALS`: per nearby entity, a debug cube, real geometry or nothing.
            if std::env::var("WOW_UNIT_VISUALS").is_ok() {
                app.add_plugins(crate::capture::UnitVisualsPlugin);
            }
            // `WOW_JITTER=<name>[,<start_s>]`: per frame, the camera, root and pose terms of the
            // nearest match's position as first and second differences, in mm and pixels.
            if std::env::var("WOW_JITTER").is_ok() {
                app.add_plugins(crate::capture::JitterMeterPlugin);
            }
            // `WOW_DRESS_CENSUS`: per player, `PLAYER_FLAGS` hide bits against what is worn.
            if std::env::var("WOW_DRESS_CENSUS").is_ok() {
                app.add_plugins(crate::capture::DressCensusPlugin);
            }
            // `WOW_REVEAL=<frames>`: per frame after a snap, the cover and each residency term.
            if std::env::var("WOW_REVEAL").is_ok() {
                app.add_plugins(crate::capture::RevealAuditPlugin);
            }
            // `WOW_ENTITY_CENSUS=<secs>`: per-archetype entity counts, once.
            if std::env::var("WOW_ENTITY_CENSUS").is_ok() {
                app.add_plugins(crate::capture::EntityCensusPlugin);
            }
            // `WOW_SCHED_CENSUS=1`: every schedule's systems in both worlds, then exit.
            if std::env::var("WOW_SCHED_CENSUS").is_ok() {
                app.add_plugins(crate::capture::SchedCensusPlugin);
            }
            // `WOW_PROBE=melee`: fights the nearest enemy for the combat-text trace.
            if std::env::var("WOW_PROBE").as_deref() == Ok("melee") {
                app.add_plugins(crate::capture::ProbeMeleePlugin);
            }
            // `WOW_PROBE=partner`: accepts group invites, as the party's second client.
            if std::env::var("WOW_PROBE").as_deref() == Ok("partner") {
                app.add_plugins(crate::capture::ProbePartnerPlugin);
            }
            // `WOW_PROBE=crossing`: boards a cross-continent boat and reports the map seam.
            if std::env::var("WOW_PROBE").as_deref() == Ok("crossing") {
                app.add_plugins(crate::capture::ProbeCrossingPlugin);
            }
            // `WOW_PROBE=taxi`: flies Stormwind to Sentinel Hill on the real wire.
            if std::env::var("WOW_PROBE").as_deref() == Ok("taxi") {
                app.add_plugins(crate::capture::ProbeTaxiPlugin);
            }
            // `WOW_PROBE=guardpoi`: checks a guard's `SMSG_GOSSIP_POI` against the server row.
            if std::env::var("WOW_PROBE").as_deref() == Ok("guardpoi") {
                app.add_plugins(crate::capture::ProbeGuardPoiPlugin);
            }
            // `WOW_PROBE_BGQUEUE=1`: queues at Stormwind's Warsong Gulch battlemaster.
            if std::env::var("WOW_PROBE_BGQUEUE").is_ok() {
                app.add_plugins(crate::capture::ProbeBgQueuePlugin);
            }
            // `WOW_PROBE_BG=wsg|ab|av`: queues, ports by `AcceptBattlefieldPort`, takes a census.
            if std::env::var("WOW_PROBE_BG").is_ok() {
                app.add_plugins(crate::capture::ProbeBgPlugin);
            }
            // `WOW_PROBE_MAIL=1`: inbox, take, send and delete at the Goldshire mailbox.
            if std::env::var("WOW_PROBE_MAIL").is_ok() {
                app.add_plugins(crate::capture::ProbeMailPlugin);
            }
            // `WOW_PROBE_AUCTION=1`: browse, sell, owner list and cancel at a Stormwind auctioneer.
            if std::env::var("WOW_PROBE_AUCTION").is_ok() {
                app.add_plugins(crate::capture::ProbeAuctionPlugin);
            }
            // `WOW_PROBE_BANK=1`: the six-opcode bank wire at a banker.
            if std::env::var("WOW_PROBE_BANK").is_ok() {
                app.add_plugins(crate::capture::ProbeBankPlugin);
            }
            // `WOW_PROBE_BINDER=1`: binds at Innkeeper Keldamyr via `ConfirmBinder()`.
            if std::env::var("WOW_PROBE_BINDER").is_ok() {
                app.add_plugins(crate::capture::ProbeBinderPlugin);
            }
            // `WOW_PROBE_SERVICE=1`: per `UNIT_NPC_FLAGS` shape, the window an NPC's click opens.
            if std::env::var("WOW_PROBE_SERVICE").is_ok() {
                app.add_plugins(crate::capture::ProbeServicePlugin);
            }
            // `WOW_PROBE_VENDOR_SWAP=1`: one Goldshire vendor opened over the other's window.
            if std::env::var("WOW_PROBE_VENDOR_SWAP").is_ok() {
                app.add_plugins(crate::capture::ProbeVendorSwapPlugin);
            }
            // `WOW_PROBE_MODEL_CAMERA=1`: a `<Model>` pane's camera against an ortho control.
            if std::env::var("WOW_PROBE_MODEL_CAMERA").is_ok() {
                app.add_plugins(crate::capture::ProbeModelCameraPlugin);
            }
            // `WOW_PROBE_GMTICKET=1`: the five-opcode ticket wire through the live VM.
            if std::env::var("WOW_PROBE_GMTICKET").is_ok() {
                app.add_plugins(crate::capture::ProbeGmTicketPlugin);
            }
            // `WOW_PROBE_CHARTER=1`: buys, opens and renames a charter at a guild registrar.
            if std::env::var("WOW_PROBE_CHARTER").is_ok() {
                app.add_plugins(crate::capture::ProbeCharterPlugin);
            }
            // `WOW_PROBE_BOOK=1`: the item-text reader's per-frame cost at the Old Town plaque.
            if std::env::var("WOW_PROBE_BOOK").is_ok() {
                app.add_plugins(crate::capture::ProbeBookPlugin);
            }
            // `WOW_PROBE_STONE=1`: clicks a meeting stone and reads the LFG queue back.
            if std::env::var("WOW_PROBE_STONE").is_ok() {
                app.add_plugins(crate::capture::ProbeStonePlugin);
            }
            // `WOW_PROBE_CHEST=1`: the self unit's base anim id around opening a chest.
            if std::env::var("WOW_PROBE_CHEST").is_ok() {
                app.add_plugins(crate::capture::ProbeChestPlugin);
            }
            // `WOW_PROBE_GOQUEST=1`: a quest poster's status below and above its MinLevel.
            if std::env::var("WOW_PROBE_GOQUEST").is_ok() {
                app.add_plugins(crate::capture::ProbeGoQuestPlugin);
            }
            // `WOW_PROBE_CLAM=1`: whether using a clam opens loot on the item's own guid.
            if std::env::var("WOW_PROBE_CLAM").is_ok() {
                app.add_plugins(crate::capture::ProbeClamPlugin);
            }
            // `WOW_PROBE=castcancel`: hearths and presses W mid-cast, timing the local self-cancel.
            if std::env::var("WOW_PROBE").as_deref() == Ok("castcancel") {
                app.add_plugins(crate::capture::ProbeCastCancelPlugin);
            }
            // `WOW_PROBE_CHARCREATE="<name>[,race,class,gender,…]"`: creates, then deletes it.
            if std::env::var("WOW_PROBE_CHARCREATE").is_ok() {
                app.add_plugins(crate::capture::ProbeCharCreatePlugin);
            }
            // `WOW_LIVE_FPS=<frames>`: samples frame times on a live run, then exits.
            if std::env::var("WOW_LIVE_FPS").is_ok() {
                app.add_plugins(crate::capture::LiveFpsPlugin);
            }
            // `WOW_PROBE_LOOK`, `WOW_PROBE_PITCH`, `WOW_PROBE_CAM`: each gated by its own
            // `from_env`, all ordered before `player::PlayerControlSet`.
            app.add_plugins(crate::capture::ProbeLookPlugin);
            app.add_plugins(crate::capture::ProbePitchPlugin);
            app.add_plugins(crate::capture::ProbeCamPlugin);
            // The FPS journal (`WOW_FPS_JOURNAL`) is registered from `lib.rs` in every build.
        }
    }
}
