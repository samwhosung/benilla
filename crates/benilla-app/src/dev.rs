//! **The dev/player seam** — the two instrument groups, and the one rule that governs them
//! (decision 0026 set the target, 1173 mandated the build, 1174 is what got built).
//!
//! ```text
//! Dev may see anything; nothing may depend on dev.
//! ```
//!
//! That is the whole boundary, and it is one-directional on purpose. An instrument's job *is* to
//! see everything — 1160 measured 266 references from the instruments into 63 of the app's 142
//! modules — so a crate wall here would mean publishing hundreds of internals as permanent public
//! API shaped by whatever a panel wanted to poke at. A `cfg` seam has the opposite property: the
//! instruments keep reaching in exactly as far as they like, and only the *direction* is
//! constrained.
//!
//! **The mechanism is not this comment.** It is `cargo build -p benilla --no-default-features` in
//! `scripts/gates.sh`. 0026 wrote this same rule down in June 2026 with no failing build behind
//! it, and by August there were 24 references from non-dev code into the instruments across 12
//! files (1173) — not through carelessness, but because a rule with no failing test is a wish.
//! When gameplay needs a fact an instrument happens to know, the fact goes to
//! [`crate::run_mode`] (always present, player-faithful default) and the instrument writes it
//! there. It does not get a `use` line from gameplay.
//!
//! **What is deliberately NOT in here:** `benilla_world::dev_state` (the always-present config
//! layer — its defaults *are* the player behaviour, so it ships), `pipe_warm` (a player on macOS
//! eats every synchronous pipeline stall without it — 0837/1116), and `art_scope` (within-map art
//! residency: engine, and it travels with `WorldPlugins`).

use bevy::prelude::*;

/// `WOW_CAPTURE=list` — the harness scenario names `scripts/visual.sh` reads, printed before any
/// window or asset setup. Answers nothing in a player build, which has no scenarios.
pub(crate) fn print_scenario_names() {
    #[cfg(feature = "dev")]
    crate::capture::print_scenario_names();
}

/// `WOW_PROBE=list` — the probe fleet's environment registry (`capture::probe_env`): every
/// `WOW_PROBE*` variable, whether it arms the un-occludable window, and `WOW_PROBE`'s own named
/// values — printed before any window or asset setup. Answers nothing in a player build, which
/// has no probes.
pub(crate) fn print_probe_vars() {
    #[cfg(feature = "dev")]
    crate::capture::probe_env::print();
}

/// `WOW_HOVER_LOG_REPORT=<csv>` — re-read a recorded run and print its report, no window, no game.
/// New analysis lands on runs already captured (see `hover_log`).
pub(crate) fn report_recorded_hover_log(_path: &str) {
    #[cfg(feature = "dev")]
    crate::hover_log::report_recorded_file(_path);
}

/// **The instruments.** One group, added where the debug panel has always sat — first among them,
/// because `PerfPlugin` needs the egui plugin and context it sets up.
pub(crate) struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    #[cfg_attr(not(feature = "dev"), allow(unused_variables))]
    fn build(&self, app: &mut App) {
        #[cfg(feature = "dev")]
        {
            app.add_plugins(crate::debug_panel::DebugPanelPlugin)
                .add_plugins(crate::perf::PerfPlugin)
                // `WOW_FX_CENSUS=1`: where this frame's particle draws are addressed, and whether
                // the view they name is switched on. An instrument, and one that
                // reads the portrait booths — so it belongs on this side of the line.
                .add_plugins(crate::capture::fx_draw_census_plugin)
                // The hover-cost recorder (`WOW_HOVER_LOG`) and the asset-churn meter
                // (`WOW_ASSET_CHURN`) — both no-ops without their variable.
                .add_plugins(crate::hover_log::HoverLogPlugin)
                .add_plugins(crate::asset_churn::AssetChurnPlugin)
                // The session preflight: one banner per world entry naming the body
                // we logged into, and loud warnings for the states — dead/ghost, GM mode,
                // server-blocked movement — that silently invalidate a session's readings. Never
                // env-gated; a warning nobody switches on isn't one.
                .add_plugins(crate::preflight::PreflightPlugin)
                // The probe shield: a body on a probe account is put into vmangos's
                // `.cheat god` on every world entry — damage clamps at 1 hp instead of killing —
                // and GM mode is turned OFF, because the shield replaces the only reason it was
                // ever on. Inert on any other account.
                .add_plugins(crate::probe_shield::ProbeShieldPlugin);
        }
    }
}

/// **The probe fleet** — the capture harness and every scripted live probe, each armed by its own
/// environment variable and inert without it. Added last so they observe the fully-built app.
pub(crate) struct DevProbesPlugin;

impl Plugin for DevProbesPlugin {
    #[cfg_attr(not(feature = "dev"), allow(unused_variables))]
    fn build(&self, app: &mut App) {
        #[cfg(feature = "dev")]
        {
            // The capture harness drives one deterministic screenshot then exits — added last so it observes
            // the fully-built app. Inert unless `$WOW_CAPTURE` is set.
            if crate::run_mode::scenario_active() {
                app.add_plugins(crate::capture::CapturePlugin);
            }
            // The LIVE probe shot (orthogonal to the harness): `WOW_LIVE_SHOT=<png>` on a NORMAL connected
            // run writes one screenshot `WOW_LIVE_SHOT_AT` seconds (default 12) after startup and keeps
            // running — the agent-side instrument for seeing a live server scene (NPCs, GameObjects, event
            // spawns) without a scenario. Pair with `WOW_USER`/`WOW_CHAR` + an outer `timeout`.
            if std::env::var("WOW_LIVE_SHOT").is_ok() {
                app.add_plugins(crate::capture::LiveShotPlugin);
            }
            // The probe RIG: `WOW_RIG="tauren druid 60 gear:heal-preraid-bis"` finds-or-
            // creates that body on this slot's probe account, logs in as it, and applies level/spells/gear/
            // spec/place — the one verb that replaces the hand-assembled GM recipe every session used to
            // re-derive (see `capture::ProbeRigPlugin`).
            if std::env::var("WOW_RIG").is_ok() {
                app.add_plugins(crate::capture::ProbeRigPlugin);
            }
            // Any scripted probe keeps its window un-occludable: a fully covered macOS window drops to
            // ~1 fps drawables, and every probe schedule is wall-clock — a throttled run doesn't measure
            // slowly, it runs the wrong script (see `capture::ProbeFocusPlugin`).
            // Which probe variables count is the registry's `wall_clock` column
            // (`capture::probe_env::PROBE_VARS`), not a list kept here: this list was hand-kept
            // and had drifted to ten of the twenty-five wall-clock variables when 2265 §A5 read
            // it against the code, so a mail or auction probe ran covered. The four below are the
            // non-probe instruments that schedule on the wall clock too.
            // (`WOW_LIVE_FPS` is in the list because an occluded SETTLE phase streams the world at ~1 fps
            // and under-warms the scene before sampling even starts — the assertion has to be live from
            // the first tick, not at the uncap.)
            if crate::capture::probe_env::wall_clock_vars()
                .chain([
                    "WOW_RIG",
                    "WOW_LIVE_FPS",
                    // A screenshot burst and a pick burst are wall-clock schedules too, and on an
                    // occluded window they capture the same stale drawable over and over: a whole
                    // day of lamppost A/Bs on the Air read identical to the decimal because every
                    // frame in every burst was one frame (the halo record).
                    "WOW_LIVE_SHOT",
                    "WOW_PICK",
                ])
                .any(|k| std::env::var(k).is_ok())
            {
                app.add_plugins(crate::capture::ProbeFocusPlugin);
            }
            // The probe-chat one-shot: `WOW_PROBE_CHAT=".go xyz …"` sends GM/chat lines once in-world —
            // the "park the probe character anywhere" instrument (see `capture::ProbeChatPlugin`).
            if std::env::var("WOW_PROBE_CHAT").is_ok() {
                app.add_plugins(crate::capture::ProbeChatPlugin);
            }
            // The probe-lua driver: `WOW_PROBE_LUA="CastSpell(…)"` runs a chunk in the live UI VM once
            // per world entry — the "press the button headlessly" instrument, re-armed across a relog
            // so a probe can read the same value on both sides of one (see `capture::ProbeLuaPlugin`).
            if std::env::var("WOW_PROBE_LUA").is_ok() {
                app.add_plugins(crate::capture::ProbeLuaPlugin);
            }
            // The probe-drag driver: `WOW_PROBE_DRAG="RaidGroupButton7>RaidGroup6Slot1"` drags one
            // named frame onto another through the real pointer path, one gesture step per frame —
            // the "do what the hand on the mouse does" instrument (see `capture::ProbeDragPlugin`).
            if std::env::var("WOW_PROBE_DRAG").is_ok() {
                app.add_plugins(crate::capture::ProbeDragPlugin);
            }
            // The probe-hover sweep: `WOW_PROBE_HOVER="ActionButton1;ActionButton2;…"` crosses each
            // named frame's centre through the real pointer path, pressing nothing — the "hand on
            // the mouse" `WOW_HOVER_LOG` was built to record and could not supply itself.
            if std::env::var("WOW_PROBE_HOVER").is_ok() {
                app.add_plugins(crate::capture::ProbeHoverPlugin);
            }
            // The probe-key taps: `WOW_PROBE_KEY="Space@14"` presses keys once in-world — the "press
            // space headlessly" instrument for input-gated behavior (see `capture::ProbeKeyPlugin`).
            if std::env::var("WOW_PROBE_KEY").is_ok() {
                app.add_plugins(crate::capture::ProbeKeyPlugin);
            }
            // The probe self-termination: `WOW_PROBE_EXIT_AT=<secs>` bounds any scripted live probe's
            // lifetime — its own knob, not a rider on the Lua probe (see `capture::ProbeExitPlugin`).
            if std::env::var("WOW_PROBE_EXIT_AT").is_ok() {
                app.add_plugins(crate::capture::ProbeExitPlugin);
            }
            // The ray pick: `WOW_PICK="<x>,<y>"` names every surface along the ray through a screenshot
            // pixel, nearest first — "what is at the spot `benilla-visual hotspot` flagged, and what is
            // right behind it" (see `capture::PickProbePlugin`).
            if std::env::var("WOW_PICK").is_ok() {
                app.add_plugins(crate::capture::PickProbePlugin);
            }
            // The render-phase census: `WOW_PHASE=<uniqueId>` reports, per frame, which phase each of one
            // placement's batches landed in and where in the draw order — the one thing every scene-side
            // instrument is blind to, namely whether a surface was submitted at all (see
            // `capture::PhaseProbePlugin`).
            if std::env::var("WOW_PHASE").is_ok() {
                app.add_plugins(crate::capture::PhaseProbePlugin);
            }
            // The depth readback: `WOW_DEPTH="<x>,<y>"` reports what depth actually won each named pixel,
            // per frame, as a distance in yards — the link past submission that decides the pixel. Pair it
            // with `WOW_PICK` at the same pixels to turn "what won" into "whose it was" (see
            // `capture::DepthProbePlugin`).
            // `WOW_DEPTH_QUADS=<bone>…` is the same readback taken at a particle quad's OWN pixels — the
            // moving-subject form, which no hand-written pixel list can hold (see `capture::depth_probe`).
            if std::env::var("WOW_DEPTH").is_ok() || std::env::var("WOW_DEPTH_QUADS").is_ok() {
                app.add_plugins(crate::capture::DepthProbePlugin);
            }
            // The bevy_ui node census — "who owns this rectangle" for UI outside the FrameXML quad pass
            // (see `capture::NodeProbePlugin`).
            if std::env::var("WOW_NODE_PROBE").is_ok() {
                app.add_plugins(crate::capture::NodeProbePlugin);
            }
            // The mid-run window resize: `WOW_PROBE_RESIZE="<secs>:<W>x<H>"` — the headless fullscreen-
            // toggle stand-in for resize-reactive layout (see `capture::ProbeResizePlugin`).
            if std::env::var("WOW_PROBE_RESIZE").is_ok() {
                app.add_plugins(crate::capture::ProbeResizePlugin);
            }
            // The particle census: `WOW_PARTICLE_CENSUS=<secs>` prints per-emitter live counts once —
            // the trace-comparable coverage number (see `capture::ParticleCensusPlugin`).
            if std::env::var("WOW_PARTICLE_CENSUS").is_ok() {
                app.add_plugins(crate::capture::ParticleCensusPlugin);
            }
            // The under-floor census: `WOW_GROUND_CENSUS=<secs>[,<every>]` prints one line per
            // streamed unit near the body — the server's Z for it, the Z we drew it at, the drop
            // between them, and the floor over its head. The instrument B197 was missing: it says
            // whether a unit below a floor was put there by the server or pulled there by us (see
            // `capture::GroundCensusPlugin`).
            if std::env::var("WOW_GROUND_CENSUS").is_ok() {
                app.add_plugins(crate::capture::GroundCensusPlugin);
            }
            // The transport census: `WOW_LIFT_CENSUS=<secs>[,<every>]` prints one line per
            // type-11/15 transport GameObject on the map — its arm stage, its cycle, its
            // visibility pair and its render-descendant count. The instrument B168 was missing: a
            // lift that isn't on screen is either unsent, unarmed (and so still wearing its spawn
            // hide), model-less, or simply elsewhere in its cycle, and only numbers tell those
            // four apart (see `capture::LiftCensusPlugin`).
            if std::env::var("WOW_LIFT_CENSUS").is_ok() {
                app.add_plugins(crate::capture::LiftCensusPlugin);
            }
            // The frame-stall injector: `WOW_STALL="<ms>[,<every_s>[,<after_s>]]"` blocks the
            // main loop on a schedule — the reproduction for "I tabbed away and came back to X",
            // which no probe can otherwise stage (every scripted probe asserts `AlwaysOnTop`
            // precisely so the OS cannot throttle it). `WOW_STALL=0` injects nothing and leaves
            // the frame-delta/occlusion monitor, which is how the premise gets checked rather
            // than assumed (see `capture::StallPlugin`).
            if std::env::var("WOW_STALL").is_ok() {
                app.add_plugins(crate::capture::StallPlugin);
            }
            // The ribbon-trail census: `WOW_TRAIL_CENSUS=<secs>[,<every>]` prints one line per live
            // weapon/spell streak — its committed extent in WORLD space, which is the observable:
            // a rider standing still on a moving deck must draw a SHORT streak, because its edges
            // are stored on the deck and re-projected through the deck's live pose. A long one is
            // a streak drawn against the world while its owner stands still (see
            // `capture::TrailCensusPlugin`).
            if std::env::var("WOW_TRAIL_CENSUS").is_ok() {
                app.add_plugins(crate::capture::TrailCensusPlugin);
            }
            // The unit-visual census: `WOW_UNIT_VISUALS=<secs>[,<every>]` prints one line per
            // streamed entity near the body — whether it got a debug cube, real geometry, or
            // nothing at all. The instrument B13 was missing: a black slab in a screenshot cannot
            // say whether the display named no model (our gap) or named one that draws nothing
            // (an invisible trigger creature — see `capture::UnitVisualsPlugin`).
            if std::env::var("WOW_UNIT_VISUALS").is_ok() {
                app.add_plugins(crate::capture::UnitVisualsPlugin);
            }
            // The motion-jitter meter: `WOW_JITTER=<name-substr>[,<start_s>]` prints one line per
            // frame for the nearest matching unit — the camera, root and pose terms of its
            // rendered position, each as a first AND second difference, in mm and in pixels at
            // its own distance. Δ alone cannot tell a slow idle from noise; Δ² can, and Δ²/dt²
            // says whether a ragged Δ is bad arithmetic or merely uneven frame times (see
            // `capture::JitterMeterPlugin`).
            if std::env::var("WOW_JITTER").is_ok() {
                app.add_plugins(crate::capture::JitterMeterPlugin);
            }
            // The dress census: `WOW_DRESS_CENSUS=<secs>[,<every>]` prints one line per streamed
            // PLAYER — what the wire asked for (`PLAYER_FLAGS`' hide bits), what we resolved
            // (helm/cloak display ids) and what is actually attached — plus a `contradictions=`
            // count for a body dressed in a piece its own preference asked us to hide. B123's
            // instrument (see `capture::DressCensusPlugin`).
            if std::env::var("WOW_DRESS_CENSUS").is_ok() {
                app.add_plugins(crate::capture::DressCensusPlugin);
            }
            // The reveal audit: `WOW_REVEAL=<frames>` prints one line per frame from a snap —
            // the cover, every residency term behind `presentable()`, the settle hold and the
            // retained pass's collected-vs-published regions. The instrument for "the teleport
            // showed me a world with no buildings in it" (see `capture::RevealAuditPlugin`).
            if std::env::var("WOW_REVEAL").is_ok() {
                app.add_plugins(crate::capture::RevealAuditPlugin);
            }
            // The entity census: `WOW_ENTITY_CENSUS=<secs>` prints per-archetype entity counts once —
            // what the resident entity count is made of (see `capture::EntityCensusPlugin`).
            if std::env::var("WOW_ENTITY_CENSUS").is_ok() {
                app.add_plugins(crate::capture::EntityCensusPlugin);
            }
            // The schedule census: `WOW_SCHED_CENSUS=1` prints every schedule's systems with their
            // executor-relevant flags, both worlds, then exits — the structural inventory under
            // the orchestration bands (see `capture::SchedCensusPlugin`).
            if std::env::var("WOW_SCHED_CENSUS").is_ok() {
                app.add_plugins(crate::capture::SchedCensusPlugin);
            }
            // The melee live probe: `WOW_PROBE=melee` auto-fights the nearest enemy so the dbg-trace
            // sink can record the combat-text timeline (see `capture::ProbeMeleePlugin`).
            if std::env::var("WOW_PROBE").as_deref() == Ok("melee") {
                app.add_plugins(crate::capture::ProbeMeleePlugin);
            }
            // The partner live probe: `WOW_PROBE=partner` auto-accepts group invites — the party arc's
            // second-client instrument (see `capture::ProbePartnerPlugin`).
            if std::env::var("WOW_PROBE").as_deref() == Ok("partner") {
                app.add_plugins(crate::capture::ProbePartnerPlugin);
            }
            // The sea-crossing live probe: `WOW_PROBE=crossing` boards a cross-continent boat and reports
            // the map seam surviving — decision 0455's instrument (see `capture::ProbeCrossingPlugin`).
            if std::env::var("WOW_PROBE").as_deref() == Ok("crossing") {
                app.add_plugins(crate::capture::ProbeCrossingPlugin);
            }
            // The taxi-flight live probe: `WOW_PROBE=taxi` opens the flight-master menu on the real wire
            // and rides Stormwind → Sentinel Hill to a measured verdict — decision 0484's end-to-end
            // instrument (see `capture::ProbeTaxiPlugin`).
            if std::env::var("WOW_PROBE").as_deref() == Ok("taxi") {
                app.add_plugins(crate::capture::ProbeTaxiPlugin);
            }
            // The guard-directions live probe: `WOW_PROBE=guardpoi` asks a Stormwind guard where the
            // weapons trainer is and reports the marker `SMSG_GOSSIP_POI` left on the map, checked
            // field by field against the server's own row (see `capture::ProbeGuardPoiPlugin`).
            if std::env::var("WOW_PROBE").as_deref() == Ok("guardpoi") {
                app.add_plugins(crate::capture::ProbeGuardPoiPlugin);
            }
            // The battleground-queue live probe: `WOW_PROBE_BGQUEUE=1` levels past the bracket
            // floor, greets Stormwind's Warsong Gulch battlemaster on the real wire and queues
            // through the guid his list carried — the fixture that makes "log in while queued"
            // reproducible (decision 2232's unexercised shape; see `capture::ProbeBgQueuePlugin`).
            if std::env::var("WOW_PROBE_BGQUEUE").is_ok() {
                app.add_plugins(crate::capture::ProbeBgQueuePlugin);
            }
            // The inside-a-battleground live probe: `WOW_PROBE_BG=wsg|ab|av` walks the whole
            // player road — level, greet, queue, take the port through the stock
            // `AcceptBattlefieldPort` — and then censuses the battleground from inside. The
            // interface arc (1963/1972/1974/1980) built everything up to the port button and
            // nothing past it; this is the first instrument that looks (see `capture::ProbeBgPlugin`).
            if std::env::var("WOW_PROBE_BG").is_ok() {
                app.add_plugins(crate::capture::ProbeBgPlugin);
            }
            // The mail-arc live probe: `WOW_PROBE_MAIL=1` GM-mails the probe's own character, opens the
            // Goldshire mailbox on the real wire, and drives the inbox/take/send/delete surface through
            // the live Lua VM — decisions 0544/0548's end-to-end instrument (see `capture::ProbeMailPlugin`).
            if std::env::var("WOW_PROBE_MAIL").is_ok() {
                app.add_plugins(crate::capture::ProbeMailPlugin);
            }
            // The auction-arc live probe: `WOW_PROBE_AUCTION=1` GM-hops to a Stormwind auctioneer,
            // greets it on the wire and drives browse/throttle/sell/owner-list/cancel through the
            // live Lua VM — decision 1511's end-to-end instrument (see `capture::ProbeAuctionPlugin`).
            if std::env::var("WOW_PROBE_AUCTION").is_ok() {
                app.add_plugins(crate::capture::ProbeAuctionPlugin);
            }
            // The bank-arc live probe: `WOW_PROBE_BANK=1` GM-hops to a pure banker, drives the whole
            // six-opcode bank wire (activate/deposit/withdraw/buy-slot/refusal) — decision 0604's
            // end-to-end instrument (see `capture::ProbeBankPlugin`).
            if std::env::var("WOW_PROBE_BANK").is_ok() {
                app.add_plugins(crate::capture::ProbeBankPlugin);
            }
            // The innkeeper-bind live probe: `WOW_PROBE_BINDER=1` GM-hops to Innkeeper Keldamyr, asserts
            // the bind row's icon reads "binder", selects it, and answers the server's confirm through the
            // live VM's own `ConfirmBinder()` — decision 1331's end-to-end instrument, the evidence that
            // closes B249 (see `capture::ProbeBinderPlugin`).
            if std::env::var("WOW_PROBE_BINDER").is_ok() {
                app.add_plugins(crate::capture::ProbeBinderPlugin);
            }
            // The NPC-service ladder live probe: `WOW_PROBE_SERVICE=1` walks four real NPCs, one per
            // interesting `UNIT_NPC_FLAGS` shape, and reports the arm the shipped ladder takes and the
            // window that actually opened — decision 1861's end-to-end instrument, and the standing
            // answer to "does right-clicking a trainer open the trainer window or a gossip menu?"
            // (see `capture::ProbeServicePlugin`).
            if std::env::var("WOW_PROBE_SERVICE").is_ok() {
                app.add_plugins(crate::capture::ProbeServicePlugin);
            }
            // The vendor-swap live probe: `WOW_PROBE_VENDOR_SWAP=1` stands between the Goldshire
            // inn's two vendors, opens one over the other's window, and reads the `"npc"` token,
            // the title and the round portrait at the `MERCHANT_SHOW` dispatch itself — decision
            // 2022's instrument (see `capture::ProbeVendorSwapPlugin`).
            if std::env::var("WOW_PROBE_VENDOR_SWAP").is_ok() {
                app.add_plugins(crate::capture::ProbeVendorSwapPlugin);
            }
            // The `<Model>` perspective-leg live probe: `WOW_PROBE_MODEL_CAMERA=1` builds a plain
            // pane on a camera-bearing file and reads the renderer's own camera and root back to
            // check the three cancellations against an orthographic control (decision 2027's
            // instrument; see `capture::ProbeModelCameraPlugin`, whose module doc is the recipe).
            if std::env::var("WOW_PROBE_MODEL_CAMERA").is_ok() {
                app.add_plugins(crate::capture::ProbeModelCameraPlugin);
            }
            // The GM trouble-ticket live probe: `WOW_PROBE_GMTICKET=1` drives the whole five-opcode
            // ticket wire through the live VM's own bindings — queue status, clean slate, file, edit,
            // abandon — and prints the row the server must have stored so the operator can check the
            // map/position the client never reads back (decision 1673's end-to-end instrument; see
            // `capture::ProbeGmTicketPlugin`).
            if std::env::var("WOW_PROBE_GMTICKET").is_ok() {
                app.add_plugins(crate::capture::ProbeGmTicketPlugin);
            }
            // The guild-charter live probe: `WOW_PROBE_CHARTER=1` GM-hops to the Stormwind guild
            // registrar, asserts the charter row's icon reads "petition", buys a charter through the
            // registrar's own window, opens it with a real bag right-click and renames it — decision
            // 1672's end-to-end instrument (see `capture::ProbeCharterPlugin`).
            if std::env::var("WOW_PROBE_CHARTER").is_ok() {
                app.add_plugins(crate::capture::ProbeCharterPlugin);
            }
            // The world-book live probe: `WOW_PROBE_BOOK=1` teleports to the Old Town plaque and
            // measures what having the item-text reader open costs per frame, closed vs open —
            // B240's instrument (see `capture::ProbeBookPlugin`).
            if std::env::var("WOW_PROBE_BOOK").is_ok() {
                app.add_plugins(crate::capture::ProbeBookPlugin);
            }
            // The meeting-stone live probe: `WOW_PROBE_STONE=1` parks at a real stone, clicks it
            // on the click's own route below and inside its level band, and reads the queue back
            // out of the live VM — decision 2283's instrument, the end-to-end answer to "can a
            // player get into the LFG queue" (see `capture::ProbeStonePlugin`).
            if std::env::var("WOW_PROBE_STONE").is_ok() {
                app.add_plugins(crate::capture::ProbeStonePlugin);
            }
            // The chest live probe: `WOW_PROBE_CHEST=1` parks at a real chest spawn, opens it on the
            // click's own route and reports the self unit's base anim id before/during/after — B84's
            // instrument, the numeric answer to "does the player kneel at a chest" (
            // see `capture::ProbeChestPlugin`).
            if std::env::var("WOW_PROBE_CHEST").is_ok() {
                app.add_plugins(crate::capture::ProbeChestPlugin);
            }
            // The GameObject-questgiver live probe: `WOW_PROBE_GOQUEST=1` parks at the Goldshire
            // wanted poster and reports the dialog status the server answers for it, below and
            // above the quest's own MinLevel — the numeric answer to "quest objects are never
            // status-queried" (see `capture::ProbeGoQuestPlugin`).
            if std::env::var("WOW_PROBE_GOQUEST").is_ok() {
                app.add_plugins(crate::capture::ProbeGoQuestPlugin);
            }
            // The openable-item live probe: `WOW_PROBE_CLAM=1` stocks a clam, right-clicks it
            // through the live VM's own `UseContainerItem` and reports whether a loot window opens
            // on the item's own guid — the numeric answer to the director's "clams don't open"
            // (see `capture::ProbeClamPlugin`).
            if std::env::var("WOW_PROBE_CLAM").is_ok() {
                app.add_plugins(crate::capture::ProbeClamPlugin);
            }
            // The cast-cancel live probe: `WOW_PROBE=castcancel` hearths and presses W mid-cast — the
            // local self-cancel's end-to-end timing instrument (see `capture::ProbeCastCancelPlugin`).
            if std::env::var("WOW_PROBE").as_deref() == Ok("castcancel") {
                app.add_plugins(crate::capture::ProbeCastCancelPlugin);
            }
            // The char-create live probe: `WOW_PROBE_CHARCREATE="<name>[,race,class,gender,…]"` creates (and
            // cleans up) a character at select to verify the char-create/delete wire (decision 0423 phase 1;
            // see `capture::ProbeCharCreatePlugin`).
            if std::env::var("WOW_PROBE_CHARCREATE").is_ok() {
                app.add_plugins(crate::capture::ProbeCharCreatePlugin);
            }
            // The live FPS probe: `WOW_LIVE_FPS=<frames>` samples frame times on a NORMAL connected run
            // and exits — the harness probe's numbers with the live world in (see `capture::LiveFpsPlugin`).
            if std::env::var("WOW_LIVE_FPS").is_ok() {
                app.add_plugins(crate::capture::LiveFpsPlugin);
            }
            // The scripted probe drivers that live beside `capture` rather than in `player/`
            // — the mouse-turn (`WOW_PROBE_LOOK`), the swim-pitch aim
            // (`WOW_PROBE_PITCH`) and the camera park (`WOW_PROBE_CAM`). Added
            // unconditionally because each plugin's own `from_env` is its gate, so the variable's name is
            // spelled in exactly one place; all three order themselves before `player::PlayerControlSet`.
            app.add_plugins(crate::capture::ProbeLookPlugin);
            app.add_plugins(crate::capture::ProbePitchPlugin);
            app.add_plugins(crate::capture::ProbeCamPlugin);
            // (The FPS journal — `WOW_FPS_JOURNAL=<csv>`, or the `fpsJournal` CVar — is
            // registered from `lib.rs` in every build since 2008: it is the one instrument a
            // player runs for us.)
        }
    }
}
