//! [`WorldPlugins`]: the world engine as one plugin group, instruments left to the program. The
//! order is the client's own, so the client and `benilla-worldview` stay diffable.

use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use benilla_assets::materials::{TerrainMaterial, WowModelMaterial};

/// The engine in one group; nothing in it may reach for a server, a player or a UI.
pub struct WorldPlugins;

impl PluginGroup for WorldPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            // The engine's WGSL first: every material below specializes against it, and an
            // unregistered shader fails silently.
            .add(crate::shaders::plugin)
            .add(MaterialPlugin::<TerrainMaterial>::default())
            .add(MaterialPlugin::<WowModelMaterial>::default())
            // Physics (avian3d): colliders, their BVH and the shape-casts of controller and picker.
            .add_group(PhysicsPlugins::default())
            // No contact pipeline: nothing reads a contact (the player is a shape-cast controller,
            // units carry no colliders), and trimesh pairs against terrain tiles cost whole ticks.
            // The collider BVH the shape-casts ride is `ColliderTreePlugin`'s, kept.
            .disable::<BvhBroadPhasePlugin>()
            // No dynamics either: the world has no dynamic bodies, joints or force consumers, only
            // static colliders and kinematic transports that write `Position` directly, and no
            // `TransformInterpolation` user. Re-enable when a dynamic body enters the world.
            .disable::<ForcePlugin>()
            .disable::<MassPropertyPlugin>()
            .disable::<NarrowPhasePlugin<Collider>>()
            .disable::<JointPlugin>()
            .disable::<PhysicsInterpolationPlugin>()
            // `SolverSchedulePlugin` stays: it owns the step sets every kept system orders on.
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
            // The per-frame transition order (Input → Stream → Present) by which the loading
            // screen covers a teleport the frame it happens.
            .add(crate::schedule::SchedulePlugin)
            // Mouseover picking and object identity.
            .add(crate::interact::InteractPlugin)
            // M2 billboard cards (glow halos, chains), faced to the camera each frame.
            .add(crate::billboard::BillboardPlugin)
            // The ground-fx decal lane, which orders itself on the billboard place set.
            .add(crate::ground_fx::plugin)
            .add(crate::frame_pace::plugin)
            // The skin palette: every rig's joint matrices, skinned in `wow_model.wgsl` in place of
            // Bevy's `SkinnedMesh`.
            .add(crate::rig_palette::plugin)
            // The rig machinery the palette reads: pose evaluation, composition and the global
            // sequences; the game decides which clip plays.
            .add(crate::rig_anim::plugin)
            // The per-instance body tint (a state kit's CharProc-1 colour), on the palette's slot.
            .add(crate::instance_tint::plugin)
            // The mat-anim table: each frame's UV and tint samples, so no material is mutated.
            .add(crate::mat_anim_table::plugin)
            // The M2 render lane: render alpha, water far-side twins and depth-prime twins.
            .add(crate::model_fade::plugin)
            .add(crate::model_render::plugin)
            .add(crate::zfill::plugin)
            // The straddle split, beside the two lanes it composes with.
            .add(crate::straddle::plugin)
            // Within-map art residency: the dedup caches expire by distance.
            .add(crate::art_scope::ArtScopePlugin)
            // Opens the patch chain (`AssetSet::Open`), which every other startup runs after.
            .add(crate::assets::AssetPlugin)
            // Map.dbc and the current map, keyed off by the streamers, loading screen and lighting.
            .add(crate::world_map::WorldMapPlugin)
            .add(crate::lighting::LightingPlugin)
            // Sky dome: the Light.dbc gradient backdrop (camera-centred), driven by the lighting.
            .add(crate::sky::SkyPlugin)
            // The WMO skybox, after `SkyPlugin`, whose dome it stands down.
            .add(crate::skybox::SkyboxPlugin)
            // Clouds: the reference's procedural field, glare occlusion and the visible layer.
            .add(crate::clouds::CloudsPlugin)
            // Weather (`SMSG_WEATHER`): the storm light blend and precipitation.
            .add(crate::weather::WeatherPlugin)
            // The sun disc and glow halo (`CSky::Render`).
            .add(crate::sun::SunPlugin)
            // The interior classifier: M2 entities in a WMO room lit off its baked floor colour.
            .add(crate::interior::InteriorPlugin)
            .add(crate::entity_shade::EntityShadePlugin)
            // The world camera's pose, published before the viewer authorities below read it.
            .add(crate::view::ViewPlugin)
            // The WMO portal PVS: groups reachable from the camera's, applied by `model_render`.
            .add(crate::wmo_portal::WmoPortalPlugin)
            // From inside a building, the exterior draws only through the flood's portal windows.
            .add(crate::exterior_cull::ExteriorCullPlugin)
            // Doodad animation: placed M2s loop their first and global sequences while drawn.
            .add(crate::doodad_anim::DoodadAnimPlugin)
            // Ground clutter: the catalog and per-chunk build; the active streamer scatters.
            .add(crate::clutter::ClutterPlugin)
            // Distant low-detail terrain (WDL): the fogged horizon hills beyond the streamed tiles.
            .add(crate::wdl::WdlPlugin)
            // Liquid: animated lake/river/ocean water surfaces (MCLQ), spawned with their tile.
            .add(crate::liquid::LiquidPlugin)
            .add(crate::particles::ParticlePlugin)
            // Water foam decals (`CWater0Ripple`): wake, ring and step-in splash.
            .add(crate::water_fx::WaterFxPlugin)
            .add(crate::ffx_glow::FfxGlowPlugin)
            .add(crate::ribbons::RibbonPlugin)
            // Stuck-modifier reconciliation: macOS system shortcuts (⇧⌘5) swallow modifier
            // releases without a focus loss, wedging every bare-key binding.
            .add(crate::modkeys::ModKeysPlugin)
            // Terrain streaming (`AdtTile` through the `AssetServer`): tiles with their doodads,
            // WMOs, liquid and clutter.
            .add(crate::terrain_stream::TerrainPlugin)
    }
}

/// The engine's bare settings, after `PhysicsPlugins`: [`SubstepCount`] overwrites its default.
struct WorldFoundation;

impl Plugin for WorldFoundation {
    fn build(&self, app: &mut App) {
        // One solver substep, not avian's 6: with no dynamic bodies, kinematic motion is exact at
        // any count. Revisit when a dynamic body enters the world.
        app.insert_resource(SubstepCount(1))
            // WoW's 19.29 yd/s² gravity for avian's 9.81; a feel knob, not a fidelity target.
            .insert_resource(Gravity(Vec3::NEG_Y * 19.291_105))
            // The view distance (`farclip`), one source for the far wall and the per-object cull.
            .init_resource::<crate::view::ViewDistance>()
            // `gxMultisample`, read once at the camera's spawn, as the reference latches it.
            .init_resource::<crate::view::MsaaSetting>()
            .init_resource::<crate::view::MsaaFormats>()
            // The texture filter (`trilinear`, `anisotropic`), read once at the CVar load: the
            // reference applies it on restart.
            .init_resource::<benilla_assets::TexFilterSetting>()
            // Its clamp to what this GPU accepts, in `finish()`, between the adapter and `Startup`.
            .add_plugins(crate::view::MsaaSupportPlugin)
            // The viewer's body, empty in a program with no avatar.
            .init_resource::<crate::view::Viewer>()
            // The dev state, whose defaults are the player's; the debug panel only edits it.
            .init_resource::<crate::dev_state::DebugState>()
            .add_systems(Last, crate::dev_state::count_still_inputs)
            // The collider-set stamp cached collision answers are dated by; a removal is stamped in
            // `First`, before any consumer.
            .init_resource::<crate::collision::ColliderEpoch>()
            .init_resource::<crate::collision::MoverTraceExclusions>()
            .add_systems(First, crate::collision::track_collider_removals);
        // No pre-step transform propagation: movers write `Position`/`Rotation` directly, so a
        // second whole-world sweep buys nothing; an opening door's hull reaches physics a frame
        // late. `WOW_PHYS_PREPROP=1` restores avian's default.
        if std::env::var_os("WOW_PHYS_PREPROP").is_none() {
            app.insert_resource(avian3d::physics_transform::PhysicsTransformConfig {
                propagate_before_physics: false,
                ..Default::default()
            });
        }
        // `WOW_PHYS_HZ=<hz>`: the fixed loop's rate, a measurement lever and never a setting (at
        // 8 Hz streamed colliders reach the spatial trees up to 125 ms late).
        if let Some(hz) = std::env::var("WOW_PHYS_HZ")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
        {
            app.insert_resource(bevy::time::Time::<bevy::time::Fixed>::from_hz(hz));
        }
        // Static transform tracking always on: movers are 1–3% of the world, so the adaptive
        // check's two full scans a frame could only ever conclude "track".
        app.insert_resource(bevy::transform::systems::StaticTransformOptimizations::enabled());
        // The retained static-world pass; `WOW_STATIC_GX=0` opts out, registering nothing.
        app.add_plugins(crate::static_gx::StaticGxPlugin);
        // `WOW_MERGE_CENSUS=1`: the merge census printer.
        if crate::static_merge::census_enabled() {
            app.add_systems(bevy::app::Update, crate::static_merge::log_merge_census);
        }
        // `WOW_UPLOAD_BUDGET=<MB>`: cap the per-frame GPU upload of meshes and images (bevy's
        // `RenderAssetBytesPerFrame`, unlimited by default), a measurement lever.
        if let Some(mb) = std::env::var("WOW_UPLOAD_BUDGET")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            app.insert_resource(bevy::render::render_asset::RenderAssetBytesPerFrame::new(
                mb * 1024 * 1024,
            ));
        }
        // Direct draws on every camera: Bevy's indirect lane is a per-draw encode loop on Metal
        // (no MULTI_DRAW_INDIRECT) plus GPU preprocessing; `WOW_INDIRECT=1` restores it. A
        // required component, as `NoIndirectDrawing` must ride a camera's spawn: the work-item
        // buffers latch the first time a view is seen, and a later insert panics in wgpu.
        if std::env::var_os("WOW_INDIRECT").is_none() {
            app.register_required_components::<bevy::camera::Camera, bevy::render::view::NoIndirectDrawing>();
        }
    }
}
