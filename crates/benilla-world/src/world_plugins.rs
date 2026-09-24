//! **[`WorldPlugins`] — the engine, as one name.**
//!
//! Decision 1160 splits the world renderer out of `benilla-app`; 1164 counted what a second
//! program has to name to use it and found **31 of the 214 items were plugin registrations** — one
//! per engine module, every one named by the composition root and by nothing else. That is not an
//! API, it is an assembly manual, and it made the engine's doorway look five times wider than the
//! engine's actual vocabulary. Published as a group, the whole set is one item and the members go
//! back to being internals.
//!
//! **The order below is load-bearing and is the client's own**, so the two binaries stay diffable
//! (it was `worldview::engine`, "the cut line written down", before it was this). Two edges inside
//! it are documented dependencies rather than taste: `SkyboxPlugin` registers after `SkyPlugin`,
//! whose dome it stands down, and `PerfPlugin` after the debug panel, whose egui context it
//! needs (both instruments, and both the client's to add in that order now). `ArtScopePlugin` before `AssetPlugin` is *not* one — its own comment says so — but it
//! costs nothing to keep.
//!
//! **What is deliberately NOT here.** `pipe_warm` looks engine-adjacent and is not: the viewer
//! disproved it by running, grinding 1051 ms main-thread hitches warming the *game's* pipeline set
//! (nameplates, blob shadows, the UI quad lane, the portrait booths) for a frame that would never
//! be drawn. It is a client instrument and stays with the client.
//!
//! `debug_panel`, `perf` and the particle census are **not** here either, and the reason they once
//! had to be is worth keeping: the panel was the model-`Visibility` authority (the WMO portal PVS
//! is applied through it), so the world did not draw without it. That
//! authority is `model_render`'s now and the toggles it reads are `dev_state`'s, so the panel is an
//! instrument again — which is what 1160 wanted, instruments at the top of the stack rather than
//! the bottom. `art_scope` stays: within-map art residency is engine behaviour, not a readout, and
//! a viewer that never evicts is a viewer that measures the wrong thing.
//!
//! This is what makes `benilla-worldview` mean something. With the instruments out, the viewer is
//! the engine and nothing else, and every gameplay resource it still needs shows up as a stub.

use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use benilla_assets::materials::{TerrainMaterial, WowModelMaterial};

/// Everything `benilla-world` will own, in one group. One `add_plugins(WorldPlugins)` installs the
/// engine; nothing in it may reach for a server, a player, or a UI.
pub struct WorldPlugins;

impl PluginGroup for WorldPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            // The engine's own WGSL, compiled into the binary. First in the group
            // because every material below specializes against a shader handle, and a shader that
            // was never registered fails silently — as a whole world drawn with nothing on it.
            .add(crate::shaders::plugin)
            .add(MaterialPlugin::<TerrainMaterial>::default())
            .add(MaterialPlugin::<WowModelMaterial>::default())
            // Physics (avian3d): collider storage + collider BVH + shape-casts for the character
            // controller. The streamed terrain/placement entities carry
            // `Collider`s; the player drives `MoveAndSlide` against them. No character controller
            // rides them in the viewer, but the ray-caster and the shape queries do.
            .add_group(PhysicsPlugins::default())
            // ...but NOT the contact pipeline. Nothing in this workspace reads a contact:
            // `CollisionStart`, `CollisionEnd`, `ContactPair` and `Collisions` have zero
            // consumers, because the player is a shape-cast controller and units carry no
            // colliders at all. The broad phase is the only thing that *creates* contact pairs,
            // so disabling it leaves the narrow phase and the solver iterating an empty
            // `ContactGraph` — every resource still exists (`BroadPhaseCorePlugin` owns
            // `ContactGraph`/`JointGraph`), and the collider BVH the shape-casts actually ride
            // is `ColliderTreePlugin`'s, untouched.
            //
            // This is not a micro-optimisation. Pairs were still generated for every
            // kinematic-or-standalone collider overlapping the static world, and each one cost a
            // parry `contact_manifolds_composite_shape_composite_shape` — trimesh vs trimesh,
            // against 65k-triangle terrain tiles — recomputed every physics tick, and Bevy's
            // fixed-timestep catch-up runs up to 16 ticks in one frame (`Time<Virtual>`'s stock
            // 250 ms `max_delta`). Four stall captures across three sessions caught the main
            // thread inside `update_narrow_phase` for 12–41% of their samples; the run that
            // prompted this stalled >600 ms three times in five minutes.
            .disable::<BvhBroadPhasePlugin>()
            // …and NOT the dynamics pipeline either (stage 2 of the same diet,
            // on the same recorded premise as `SubstepCount(1)` below: the world has ZERO
            // `RigidBody::Dynamic` bodies, zero joints, zero velocity/force consumers — verified
            // by grep across the workspace, only Static colliders and Kinematic transports that
            // write their `Position` directly). With no contacts (broad phase off) and no
            // dynamic bodies, the solver bodies, the integrators, the xpbd joint solver, CCD,
            // islands/sleeping, the joint graphs, mass-property upkeep, the narrow phase's
            // empty-graph iteration and the render-side interpolation (zero
            // `TransformInterpolation` users; transports tick in `Update` at render rate) were
            // pure fixed-tick schedule overhead — ~60% of a parked frame's physics share on the
            // 1445 trace. What stays is exactly what benilla consumes: the collider storage +
            // backend, the collider BVH (`ColliderTreePlugin`) and spatial queries the
            // controller/pick shape-casts ride, collider transform propagation, and the
            // Transform↔Position sync. Revisit the moment a dynamic body enters the world —
            // the `SubstepCount` note below is the standing tripwire.
            .disable::<ForcePlugin>()
            .disable::<MassPropertyPlugin>()
            .disable::<NarrowPhasePlugin<Collider>>()
            .disable::<JointPlugin>()
            .disable::<PhysicsInterpolationPlugin>()
            // `SolverSchedulePlugin` deliberately STAYS: it owns the `PhysicsSchedule`'s
            // step-set scaffolding (the chain every kept system's `in_set` ordering hangs off —
            // removing it left the collider-tree systems floating and tripped the ambiguity
            // gate), plus the substep runner, which now runs a near-empty schedule.
            .disable::<SolverBodyPlugin>()
            .disable::<IntegratorPlugin>()
            .disable::<SolverPlugin>()
            .disable::<XpbdSolverPlugin>()
            .disable::<CcdPlugin>()
            .disable::<IslandPlugin>()
            .disable::<IslandSleepingPlugin>()
            .disable::<avian3d::dynamics::solver::joint_graph::JointGraphPlugin<FixedJoint>>()
            .disable::<avian3d::dynamics::solver::joint_graph::JointGraphPlugin<RevoluteJoint>>()
            .disable::<avian3d::dynamics::solver::joint_graph::JointGraphPlugin<PrismaticJoint>>()
            .disable::<avian3d::dynamics::solver::joint_graph::JointGraphPlugin<DistanceJoint>>()
            .disable::<avian3d::dynamics::solver::joint_graph::JointGraphPlugin<SphericalJoint>>()
            .add(WorldFoundation)
            // The per-frame world-transition ordering (Input → Stream → Present) the loading
            // screen relies on to cover a teleport the same frame it happens. See `schedule.rs`.
            .add(crate::schedule::SchedulePlugin)
            // World-interaction foundation: mouseover picking + object identity (the debug
            // inspector reads it now; hover tooltips / contextual cursor / targeting will later).
            .add(crate::interact::InteractPlugin)
            // M2 billboard cards (glow halos, chains) — faced to the camera each frame.
            .add(crate::billboard::BillboardPlugin)
            // The owned skin palette: every skinned rig's joint matrices, computed
            // by us and skinned in wow_model.wgsl — Bevy's SkinnedMesh lane is fully replaced.
            // The ground-fx decal lane (1164: 326 lines of pure render that had been
            // filed in the app). Its ordering rides the billboard place set, which is why the
            // lane registers itself rather than asking a caller to know that.
            .add(crate::ground_fx::plugin)
            .add(crate::frame_pace::plugin)
            .add(crate::rig_palette::plugin)
            // The rig machinery beside the palette it fills: the pose evaluator, the pose
            // post-pass window, the world/palette composition, and the global-sequence channels.
            // Split out of `creature_anim` (1163's re-check of `finalize_rig_worlds`) — the game
            // still decides which clip plays; posing and composing are the world's.
            .add(crate::rig_anim::plugin)
            // The per-instance body tint, on the same slot index as that palette:
            // the aura state kit's CharProc-1 colour, in its own region of the shared light buffer.
            .add(crate::instance_tint::plugin)
            // The mat-anim delta table, one region over: the per-frame samples of
            // every UV/tint-animated batch material, so animating a waterfall never mutates its
            // material asset again.
            .add(crate::mat_anim_table::plugin)
            // The M2 render lane's three own plugins (stage zero). All three used to
            // be registered by `EntitiesPlugin` — the entity streamer — which is why booting the
            // engine without the game left the model-`Visibility` authority reading a
            // `FarSideTwins` that nobody had created. Nothing in them is about streamed entities:
            // it is the render-alpha channel, the water-plane far-side twins, and the depth-prime
            // twins, all of which a terrain doodad needs exactly as much as an NPC does.
            .add(crate::model_fade::plugin)
            .add(crate::model_render::plugin)
            .add(crate::zfill::plugin)
            // The straddle split: a translucent model crossing its water plane draws
            // on both sides of the water pass, each half clipped at the waterline — the band
            // verdict, the per-slot clip region and the far-side twins, beside the two lanes
            // (water-side twins, depth primes) it composes with.
            .add(crate::straddle::plugin)
            // Within-map art residency: the dedup caches expire by DISTANCE, so a
            // long flight inside one map stops ratcheting. Before `AssetPlugin` only so the census
            // resource exists for anything that reads it at startup; it needs no ordering.
            .add(crate::art_scope::ArtScopePlugin)
            // Foundation: opens the patch chain + inserts WorldAssets/RenderConfig
            // (`AssetSet::Open`), which every other subsystem's startup runs after.
            .add(crate::assets::AssetPlugin)
            // World-map state (Map.dbc catalog + CurrentMap), loaded right after the chain opens —
            // the terrain/WDL streamers, loading screen, and lighting all key off it.
            .add(crate::world_map::WorldMapPlugin)
            // Time-of-day lighting: sun + WoW shader colors, sky background, per-frame
            // update→apply.
            .add(crate::lighting::LightingPlugin)
            // Sky dome: the Light.dbc gradient backdrop (camera-centred), driven by that lighting.
            .add(crate::sky::SkyPlugin)
            // WMO skybox: the authored sky a building's `0x40000` group swaps in for that gradient
            // (Stratholme's burning city) — registered after SkyPlugin, whose dome it stands down.
            .add(crate::skybox::SkyboxPlugin)
            // Cloud coverage: the reference's procedural field — glare occlusion (occ1) + the
            // visible layer.
            .add(crate::clouds::CloudsPlugin)
            // Weather: the SMSG_WEATHER state machine driving the storm light-blend +
            // precipitation. Lighting reads its densities `.after(WeatherTick)`.
            .add(crate::weather::WeatherPlugin)
            // Sun disc + glow halo: the celestial sprites WoW draws at the sun (RE'd from
            // CSky::Render).
            .add(crate::sun::SunPlugin)
            // Interior lighting classifier: lights M2 entities (GameObjects/NPCs/other players)
            // standing inside a WMO room off the baked floor colour, day/night-independent (the
            // streamer fills its volume registry).
            .add(crate::interior::InteriorPlugin)
            .add(crate::entity_shade::EntityShadePlugin)
            // The world camera's pose, published before the `Update`-stage viewer authorities
            // below read it — without it they answer about where the camera was
            // last frame, which on a teleport frame is the place we just left.
            .add(crate::view::ViewPlugin)
            // WMO portal visibility: per-frame, decides which of a building's groups are reachable
            // through portals from the camera's group, so the Stormwind cathedral culls from the
            // Trade District. Only computes the PVS; the `Visibility` authority
            // (`model_render::visibility`) applies it.
            .add(crate::wmo_portal::WmoPortalPlugin)
            // The exterior scene draws only through portal windows the flood left behind (decision
            // 0774): from inside a building, terrain and ADT doodads are gated on the deferred
            // window worklist.
            .add(crate::exterior_cull::ExteriorCullPlugin)
            // Doodad animation: placed M2s loop their first sequence + global
            // sequences, gated to drawn instances.
            .add(crate::doodad_anim::DoodadAnimPlugin)
            // Ground clutter: the GroundEffect catalog + the lazy per-chunk build lifecycle, owned
            // independently of the terrain streamer (so the streamer can be swapped). Whichever
            // streamer is active scatters into the ClutterChunks this builds.
            .add(crate::clutter::ClutterPlugin)
            // Distant low-detail terrain (WDL): the fogged horizon hills beyond the streamed tiles.
            .add(crate::wdl::WdlPlugin)
            // Liquid: animated lake/river/ocean water surfaces (MCLQ), spawned with their tile.
            .add(crate::liquid::LiquidPlugin)
            // Particle emitters: the additive flames/glows of campfires, torches, braziers.
            .add(crate::particles::ParticlePlugin)
            // Water foam decals (CWater0Ripple wake/ring/step-in splash) — the record model,
            // rebuilt from the byte RE + two reference-trace reconstructions.
            .add(crate::water_fx::WaterFxPlugin)
            .add(crate::ffx_glow::FfxGlowPlugin)
            .add(crate::ribbons::RibbonPlugin)
            // Stuck-modifier reconciliation: macOS system shortcuts (⇧⌘5) swallow modifier
            // releases without a focus loss, wedging every bare-key binding.
            .add(crate::modkeys::ModKeysPlugin)
            // Terrain streaming: the benilla-assets `AdtTile` pipeline — streams tiles around the
            // viewer through the `AssetServer`, owning the terrain mesh/material/collision,
            // doodads/WMOs, liquid, clutter, and loading-screen residency. Last, as in both
            // binaries before this group existed.
            .add(crate::terrain_stream::TerrainPlugin)
    }
}

/// The engine's bare settings — the two avian tunings, the view distance, the dev state. A plugin only
/// because a [`PluginGroup`] holds plugins and these are `insert_resource` calls; it sits where the
/// client and the viewer both had them, immediately after `PhysicsPlugins`, because
/// [`SubstepCount`] overwrites avian's own default and must therefore land after it.
struct WorldFoundation;

impl Plugin for WorldFoundation {
    fn build(&self, app: &mut App) {
        // One solver substep, not avian's 6: the world has NO dynamic bodies (static terrain,
        // kinematic transports/attachments; the player is a shape-cast controller), so the substep
        // loop's contact/joint solving iterates over nothing — and kinematic motion integrates
        // exactly (constant velocity, no forces) at any substep count. Six substeps were pure
        // fixed-tick schedule overhead (~10 substep-schedule runs per frame on the idle-floor
        // ledger). Revisit if a dynamic body ever enters the world.
        app.insert_resource(SubstepCount(1))
            // WoW gravity (19.29 yd/s², binary-derived — now a feel knob, not a fidelity target)
            // replaces avian's 9.81 default.
            .insert_resource(Gravity(Vec3::NEG_Y * 19.291_105))
            // The faithful view distance (`farclip`) — one source of truth for the wall + the
            // per-object cull (and, post-split, the stream radius). See `view.rs`.
            .init_resource::<crate::view::ViewDistance>()
            // The world camera's multisampling level (`gxMultisample`) — read
            // ONCE, at the camera's spawn, because the reference's own flag says latched.
            .init_resource::<crate::view::MsaaSetting>()
            .init_resource::<crate::view::MsaaFormats>()
            // The process texture filter policy (`trilinear` / `anisotropic`) — read ONCE, at the
            // CVar load, for the same reason: a sampler is baked into an uploaded texture and the
            // reference's own UI says "enabled upon restart".
            .init_resource::<benilla_assets::TexFilterSetting>()
            // ...and the clamp that keeps it inside what this GPU will actually accept. Runs in
            // plugin `finish()`, so it lands between the render adapter appearing and `Startup`
            // reading the setting — see `view::MsaaSupportPlugin`.
            .add_plugins(crate::view::MsaaSupportPlugin)
            // The viewer's body (wire (a)'s kinematics half) — defaulted by the engine so a
            // program with no avatar leaves it empty and every reader takes its no-body branch.
            .init_resource::<crate::view::Viewer>()
            // The dev state: the always-present config layer eight subsystems
            // read, whose defaults ARE the player behaviour. The debug panel is only its editor
            // and may not be installed at all.
            .init_resource::<crate::dev_state::DebugState>()
            .add_systems(Last, crate::dev_state::count_still_inputs)
            // The collider-set stamp every cached collision answer is dated against
            // (`collision::ColliderEpoch`). Tracked in `First` so a removal is stamped before any
            // consumer runs; the attach half is stamped by the streamer's own attach loop.
            .init_resource::<crate::collision::ColliderEpoch>()
            .init_resource::<crate::collision::MoverTraceExclusions>()
            .add_systems(First, crate::collision::track_collider_removals);
        // Avian's pre-step copy of Bevy's transform propagation is OFF by default (the 1370
        // bracket surfaced the lane; the 3-round SW split then measured the skip at −0.40
        // cpu_ms, negative in every round): `PhysicsTransformPlugin` re-registers
        // `mark_dirty_trees`/`propagate_parent_transforms`/`sync_simple_transforms` to run
        // before each physics step — a second whole-world transform sweep on top of
        // PostUpdate's own. Our world is static geometry plus kinematic movers
        // (`transport::tick_transports`) that mirror `Position`/`Rotation` directly — built
        // that way precisely because the sync's own timing already lagged — so the pre-step
        // sweep buys nothing. The one Transform-moved collider class left is an opening door
        // (a GameObject's model-local hull riding its pose): it reaches physics one frame late
        // via PostUpdate's own propagation — transient, bounded, imperceptible.
        // `WOW_PHYS_PREPROP=1` restores avian's upstream default (the A/B lever back).
        if std::env::var_os("WOW_PHYS_PREPROP").is_none() {
            app.insert_resource(avian3d::physics_transform::PhysicsTransformConfig {
                propagate_before_physics: false,
                ..Default::default()
            });
        }
        // `WOW_PHYS_HZ=<hz>` — run the fixed loop at a different rate. An EXPERIMENT lever
        // (1370 item 10): avian's per-tick stack is the fixed loop's ONLY occupant (nothing
        // first-party lives in any fixed schedule — zero-hit grep, re-verified at flip time),
        // so 8 vs the default 64 prices the whole-avian bracket in one leg. A measurement
        // lever, never a setting: at 8 Hz the spatial trees ingest streamed colliders up to
        // 125 ms late, which the player's shape-cast controller would feel at a tile seam.
        if let Some(hz) = std::env::var("WOW_PHYS_HZ")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
        {
            app.insert_resource(bevy::time::Time::<bevy::time::Fixed>::from_hz(hz));
        }
        // Static transform tracking is ALWAYS on — the adaptive default pays two serial
        // full-population scans per frame (`mark_dirty_trees`'s `count()` calls; a `Changed`
        // filter count degrades to full iteration) to decide whether >30% of the world moved
        // this frame. Ours never has: movers are the streamed units, 1–3% of a world that is
        // overwhelmingly static placements — the check can only ever conclude "track", so the
        // two scans are its entire price. Pinned, not a knob: no scene benilla renders can be
        // on the other side of the threshold.
        app.insert_resource(bevy::transform::systems::StaticTransformOptimizations::enabled());
        // The retained static-world pass — ON by default (1429–1434; `WOW_STATIC_GX=0`
        // opts out; the module doc owns the design). Registers NOTHING when opted out —
        // the divert sites see `None` for its resource and the render graph never gains
        // the node.
        app.add_plugins(crate::static_gx::StaticGxPlugin);
        // `WOW_MERGE_CENSUS=1` — the 1417 population census (module doc): tally rides the
        // assembler, this is just the quiet-timer printer.
        if crate::static_merge::census_enabled() {
            app.add_systems(bevy::app::Update, crate::static_merge::log_merge_census);
        }
        // `WOW_UPLOAD_BUDGET=<MB>` — cap the render app's per-frame GPU asset upload (bevy's
        // `RenderAssetBytesPerFrame`, unlimited by default; only assets implementing `byte_len`
        // participate, which covers meshes and images — exactly the streaming payload). An
        // EXPERIMENT lever for the arrival burst: the LBRS arrival trace (2026-08-17) put the
        // fat frames in drawable/render-schedule WAITS while streamed uploads land — every
        // suspect system's own CPU was flat, avian's whole stack included — so pacing the
        // uploads is the candidate fix, and this knob prices it before anything is redesigned.
        if let Some(mb) = std::env::var("WOW_UPLOAD_BUDGET")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            app.insert_resource(bevy::render::render_asset::RenderAssetBytesPerFrame::new(
                mb * 1024 * 1024,
            ));
        }
        // Direct draws on EVERY camera — the default, not a knob (1374/1376). Bevy's indirect
        // lane is a per-draw indirect encode loop on Metal (no MULTI_DRAW_INDIRECT) plus the GPU
        // preprocessing dispatches, and the 1374 bracket priced it ~4.3 cpu_ms at LBRS on 0.18 —
        // shipped unclaimed for the campaign's whole run because 1364's knob under-measured it
        // (the UI Camera2d leak, 1370). `WOW_INDIRECT=1` restores the upstream default for
        // re-pricing. 1370's note that the lever must cover every camera, not just the world's,
        // is why this covers `Camera` itself rather than our own marker.
        //
        // **A REQUIRED COMPONENT, not a sweep**. 1374/1376 shipped this as an
        // `Update` system that inserted the marker on any camera lacking it, and that is a latent
        // GPU crash: bevy's own doc on `NoIndirectDrawing` says *"This component should only be
        // added when initially spawning a camera. Adding or removing after spawn can result in
        // unspecified behavior"*, and the unspecified behaviour is concrete —
        // `get_or_create_work_item_buffer` latches Direct-vs-Indirect work-item buffers the FIRST
        // time it sees a view (`Entry::Occupied` returns the existing one, forever), while the
        // preprocessing node picks its pipeline from the LIVE `Has<NoIndirectDrawing>`. A camera
        // extracted once before the sweep's deferred insert lands therefore keeps Indirect buffers
        // under a direct pipeline for the rest of its life, and the next dispatch is a wgpu
        // validation panic ("bind group … not compatible … Assigned entry with binding 7 not
        // found"). Measured: the sweep was flagging 18 cameras late at startup and one 0.5 s in.
        // `player::setup` already knew the rule for the world camera ("it must ride the SPAWN: the
        // phase cache latches the preprocessing mode the first time it sees the view") — this
        // gives every other camera the same guarantee, atomically, with no frame to lose.
        if std::env::var_os("WOW_INDIRECT").is_none() {
            app.register_required_components::<bevy::camera::Camera, bevy::render::view::NoIndirectDrawing>();
        }
    }
}
