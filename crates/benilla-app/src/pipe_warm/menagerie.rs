//! The menagerie — `pipe_warm`'s warm-set builder: one tiny rig per reachable pipeline variant,
//! spawned behind the entry cover by [`super::run_warm_pass`]. The watch/tripwire half (and the
//! why of the whole pass) lives in the parent module; this file owns WHAT gets warmed — the
//! model lane and its derived keys, the sky/water lanes, the plain-`StandardMaterial` lanes,
//! and the portrait-booth samples=1 twins — plus the lane-coverage gate test that keeps a new
//! material lane from shipping unwarmed.

use benilla_formats::{FogPolicy, ModelBlend, RenderSubmesh};
use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::MeshAabb;
use bevy::ecs::system::SystemParam;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;

use benilla_assets::materials::{LiquidMaterial, WowModelMaterial};
use benilla_world::clouds::CloudMaterial;
use benilla_world::model_render::{
    far_twin_of, model_material, zfill_material, MaterialCache, ShadeSel,
};
use benilla_world::sky::SkyMaterial;
use benilla_world::sun::{CelestialMaterial, StarMaterial};

use super::WarmRig;

// --------------------------------------------------------------------------------------------
// The warm pass — the fix half of 0837, widened by 0937.
//
// The 0837 inventory (pipes1.log) showed the model lane's REACHABLE pipeline space is small once
// the batch-order axis left the key: 4 vertex layouts × the blend/depth-flag families, ~28
// observed in a wilderness-to-Stormwind leg. 0937 added the spaces 0837 left out — the
// shard-rung buckets and far-side-of-water twins of that same model lane, and the sky/water
// lanes (celestial discs/glares, stars, cloud dome, gradient dome, WMO skybox, liquid), each of
// which had fired the tripwire as a director-felt live stall. The menagerie below compiles the
// whole space behind the entry loading cover — one 1 cm rig per variant, parented to the world
// camera (the camera renders under the cover, 0540, so every rig's draw queues its pipeline
// through the production specialize path), the cover held (via `WarmPass::satisfied` in the
// loading screen's clear condition) until the cache drains, then the rigs despawn. A variant
// this misses shows up as the tripwire's "compiled LIVE" warn — extend the loops, don't guess.

/// The sky/water lanes' material stores, grouped so [`run_warm_pass`] stays under Bevy's
/// system-param arity. Every store here is populated at `Startup` by its own subsystem (the
/// celestial rig, the star dome, the cloud dome, the gradient dome, the liquid frame sets), so
/// by the time the menagerie spawns behind the entry cover, iterating the store IS the complete
/// reachable set — warming can never drift from what's spawned. The one exception is the WMO
/// skybox, whose materials are built on first need; since decision 1264 it is not a lane of its own
/// at all but a cross of `WowModelMaterial` keys, warmed with the model cross below.
#[derive(SystemParam)]
pub(super) struct WarmLanes<'w> {
    celestial: ResMut<'w, Assets<CelestialMaterial>>,
    stars: ResMut<'w, Assets<StarMaterial>>,
    clouds: ResMut<'w, Assets<CloudMaterial>>,
    sky: ResMut<'w, Assets<SkyMaterial>>,
    liquid: ResMut<'w, Assets<LiquidMaterial>>,
    /// The plain-`StandardMaterial` lanes: representatives for the on-demand nameplate
    /// and raid-mark materials go through this store the way their builders do.
    standard: ResMut<'w, Assets<StandardMaterial>>,
    /// The fallback cube: production mesh + materials, drawn while a model streams.
    cubes: Option<Res<'w, crate::entities::CubeAssets>>,
    /// The image store: the twin booth's render target and the effect-lane warm's
    /// stand-in texture are created here for the life of the pass.
    pub(super) images: ResMut<'w, Assets<Image>>,
    /// The UI quad store (2262): the minimap interior composite's tile material is built here
    /// through its own production builder, for the tile-quad warm rig below.
    ui_quads: ResMut<'w, Assets<crate::ui_pass::UiQuadMaterial>>,
}

/// One portrait/paperdoll booth camera + its layer ([`crate::portrait`]): the booths run
/// `Msaa::Off`, so every model-lane pipeline has a samples=1 twin that otherwise compiles live
/// on the first in-world portrait (the first click-target). The booth cameras exist from
/// `Startup` and render during the warm window (the demand gate counts the pass as demand), so
/// menagerie rigs duplicated onto ONE booth's layer compile that twin space behind the cover —
/// the view shape (HDR, no tonemap, the glow node) is otherwise the world camera's, leaving TWO
/// twin axes: samples, and the projection CLASS. A real booth carries the Perspective
/// placeholder until its first bake installs `Projection::custom(WowPortraitProjection)` — a
/// distinct bevy_pbr view key — so the rigs are ALSO duplicated onto the pass's own twin booth
/// ([`crate::portrait::spawn_warm_booth`]), which is that custom-projection space; 0938 warmed
/// only the placeholder class, and the whole samples=1 space compiled again, live, on the first
/// target portrait.
pub(super) type BoothCamQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static bevy::camera::visibility::RenderLayers),
    With<crate::portrait::BoothCam>,
>;

/// Spawn one tiny quad per reachable pipeline variant — the model lane, its shard-rung and
/// far-side-of-water twins, and the sky/water lanes (decision 0945 widened 0837's model-only
/// scope) — parented to the world camera. Materials come from the PRODUCTION builders
/// (`model_material` / `zfill_material` / `far_twin_of`) or the PRODUCTION live asset stores
/// (celestial, stars, clouds, gradient dome, liquid — all populated at `Startup`), so the
/// variant encoding can never drift from the real spawn paths; meshes from the production
/// submesh builders (or their attribute-exact stand-ins) so the vertex layouts can't either.
/// Returns the entity count.
pub(super) fn spawn_menagerie(
    commands: &mut Commands,
    cam: Entity,
    booth: Option<(Entity, &bevy::camera::visibility::RenderLayers)>,
    warm_booth: &(Entity, bevy::camera::visibility::RenderLayers),
    warm_ortho: &(Entity, bevy::camera::visibility::RenderLayers),
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<WowModelMaterial>,
    lanes: &mut WarmLanes,
    cache: &mut MaterialCache,
    light: &Buffer,
) -> usize {
    // The four vertex layouts the model lane ships (0837 dump: strides 32/48/56/72): static ×
    // {plain, vertex-colours} and their skinned twins. Statics are RENDER_WORLD-only, so their
    // Aabb is computed here and inserted explicitly (0832's rule); skinned twins keep main-world
    // data and `calculate_bounds` covers them.
    let mut layouts: Vec<(Handle<Mesh>, Option<bevy::camera::primitives::Aabb>, bool)> = Vec::new();
    for colors in [false, true] {
        let stat = benilla_assets::submesh_to_static_mesh(&warm_quad(colors, false));
        let aabb = stat.compute_aabb();
        layouts.push((meshes.add(stat), aabb, false));
        let skin = benilla_assets::submesh_to_skinned_mesh(&warm_quad(colors, true));
        layouts.push((meshes.add(skin), None, true));
    }

    // The material families. The full cross is deliberate: every branch here is authorable in an
    // M2/WMO (blends, the 0x10/0x08 depth flags, sidedness), and an over-warmed variant costs
    // milliseconds behind a loading bar once per run, while a missed one is a director-felt live
    // stall. The observed set (28) is the floor, not the target.
    let mut mats: Vec<Handle<WowModelMaterial>> = Vec::new();
    // The sky lane is kept apart from `mats`: it rides the STATIC layout only and is never
    // far-twinned (a camera-anchored backdrop has no side of the water plane —
    // `model_render::classify_water_side` skips it) nor booth-duplicated (no portrait sees a sky).
    let mut sky_mats: Vec<Handle<WowModelMaterial>> = Vec::new();
    for two_sided in [false, true] {
        for blend in [
            ModelBlend::Opaque,
            ModelBlend::AlphaTest,
            ModelBlend::Blend,
            ModelBlend::Mod,
            ModelBlend::Mod2x,
        ] {
            for no_depth_write in [false, true] {
                for no_depth_test in [false, true] {
                    mats.push(model_material(
                        cache,
                        materials,
                        None,
                        blend,
                        two_sided,
                        false,
                        false,
                        false,
                        false,
                        false,
                        no_depth_write,
                        no_depth_test,
                        FogPolicy::Scene,
                        // Not a `WowModelKey` axis (it swaps a sampled UV, not pipeline state), so
                        // warming one side warms both.
                        false,
                        ShadeSel::Lit,
                        0,
                        None,
                        None,
                        None,
                        None,
                        false,
                        false, // warms the WORLD lane; the sky lane warms with its own model
                        light,
                        // The pipeline menagerie warms the SHARED batch material; the
                        // per-placement lane (1408) builds an identical pipeline, so a clone
                        // needs no row of its own here.
                        None,
                    ));
                }
            }
        }
        // The additive glow-card blend state (specialize's pure ONE/ONE add) and the
        // doodad/entity distance-fade blend twin, each over the FULL depth-flag cross: the
        // production builders forward the source batch's 0x10/0x08 flags verbatim into both
        // (`assemble.rs` / `display.rs` / `particles/model.rs`), so every combination is
        // authorable — 0938 pinned the additive rows' depth-test and the fade row's both flags
        // false, and the sweep behind 0958 found the pinned keys reachable.
        for additive_not_fade in [true, false] {
            for no_depth_write in [false, true] {
                for no_depth_test in [false, true] {
                    mats.push(model_material(
                        cache,
                        materials,
                        None,
                        ModelBlend::Blend,
                        two_sided,
                        false,
                        false,
                        false,
                        additive_not_fade,
                        !additive_not_fade,
                        no_depth_write,
                        no_depth_test,
                        FogPolicy::Scene,
                        // Not a `WowModelKey` axis (it swaps a sampled UV, not pipeline state), so
                        // warming one side warms both.
                        false,
                        ShadeSel::Lit,
                        0,
                        None,
                        None,
                        None,
                        None,
                        false,
                        false, // warms the WORLD lane; the sky lane warms with its own model
                        light,
                        // The pipeline menagerie warms the SHARED batch material; the
                        // per-placement lane (1408) builds an identical pipeline, so a clone
                        // needs no row of its own here.
                        None,
                    ));
                }
            }
        }
        // The depth-prime twin (colour writes masked off), plain and cutout.
        for cutout in [false, true] {
            mats.push(zfill_material(
                cache, materials, None, two_sided, cutout, light,
            ));
        }
        // The **WMO-skybox lane**: `sky_depth` is a `WowModelKey` axis — it
        // compiles the shader's forced-far-depth branch — so every skybox batch is a pipeline the
        // world cross above does not cover, however identical the rest of its key. Built exactly
        // the way `M2BatchMaterials::skybox` builds them: depth-write pinned off, depth-test
        // pinned on, the batch's own blend and sidedness. `CavernsOfTimeSky.m2` authors Opaque,
        // AlphaTest, Blend and additive-Blend across both sidednesses; Mod/Mod2x are warmed too
        // because nothing stops a skybox authoring them. Unwarmed, walking into the Caverns of
        // Time crater is 21 live compiles in the frame the sky appears.
        for blend in [
            ModelBlend::Opaque,
            ModelBlend::AlphaTest,
            ModelBlend::Blend,
            ModelBlend::Mod,
            ModelBlend::Mod2x,
        ] {
            for additive in [false, true] {
                sky_mats.push(model_material(
                    cache,
                    materials,
                    None,
                    blend,
                    two_sided,
                    false,
                    false,
                    true, // every skybox batch the chain ships authors UNLIT
                    additive,
                    false,
                    true,  // …depth-write off
                    false, // …depth-test on
                    FogPolicy::Off,
                    false,
                    ShadeSel::Lit,
                    0,
                    None,
                    None,
                    None,
                    None,
                    false,
                    true, // the sky lane
                    light,
                    None, // the shared lane — see the note above
                ));
            }
        }
    }
    // The ground-clutter lane (specialize's over-blend), both sidednesses — the first
    // verification leg caught the two-sided one compiling live. Its material is built by
    // `WorldAssets::model_material` (image machinery this pass doesn't need) — the pipeline only
    // sees the KEY bits, so arm `clutter_fade` on a COPY of the plain material (a fresh asset;
    // the dedup cache's own entry stays untouched). ALL THREE alpha modes the clutter builder
    // maps to (Opaque / Mask / Blend — Mod/Mod2x fold to Blend there), not just Mask: a detail
    // doodad's trunk batch is Opaque, its canopy Blend, and the builder's untextured fallback is
    // Opaque + back-cull with the fade still armed — each its own key (0958's sweep; 0938 warmed
    // Mask only).
    for two_sided in [false, true] {
        for blend in [ModelBlend::Opaque, ModelBlend::AlphaTest, ModelBlend::Blend] {
            let plain = model_material(
                cache,
                materials,
                None,
                blend,
                two_sided,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                FogPolicy::Scene,
                // Not a `WowModelKey` axis (it swaps a sampled UV, not pipeline state), so
                // warming one side warms both.
                false,
                ShadeSel::Lit,
                0,
                None,
                None,
                None,
                None,
                false,
                false, // warms the WORLD lane; the sky lane warms with its own model
                light,
                None, // the shared lane — see the note above
            );
            // The warmer reads what it builds (`model_render::lazy` parks a built material
            // until something visible binds it; the warmer's entities bind next frame).
            benilla_world::model_render::lazy::realize_all(materials);
            if let Some(m) = materials.get(&plain) {
                let mut m = m.clone();
                m.extension.clutter_fade = Vec4::new(52.5, 70.0, 0.0, 1.0);
                let clutter = materials.add(m);
                mats.push(clutter);
            }
        }
    }

    // The shard-rung rows. A 3-D model particle's instance material carries its
    // owner-last rung in `depth_bias`, which is ALSO a pipeline-key axis (0837's law) — so the
    // runtime stamps only the closed bucket set (`owner_last_rung_bucket`), and this table
    // compiles that set. Families are the corpus census's floor plus the depth-write axis
    // (`benilla-extract shardcensus`: every 1.12.1 shard batch is Blend, ± two-sided,
    // ± additive, all no-depth-write — over-warming the write axis costs milliseconds). Built as
    // bias-stamped COPIES exactly like the runtime does it, so the dedup cache stays untouched.
    let mut shard_mats: Vec<Handle<WowModelMaterial>> = Vec::new();
    for &bucket in benilla_formats::OWNER_RUNG_BUCKETS.iter() {
        for two_sided in [false, true] {
            for additive in [false, true] {
                for no_depth_write in [false, true] {
                    let h = model_material(
                        cache,
                        materials,
                        None,
                        ModelBlend::Blend,
                        two_sided,
                        false,
                        false,
                        false,
                        additive,
                        false,
                        no_depth_write,
                        false,
                        FogPolicy::Scene,
                        // Not a `WowModelKey` axis (it swaps a sampled UV, not pipeline state), so
                        // warming one side warms both.
                        false,
                        ShadeSel::Lit,
                        0,
                        None,
                        None,
                        None,
                        None,
                        false,
                        false, // warms the WORLD lane; the sky lane warms with its own model
                        light,
                        None, // the shared lane — see the note above
                    );
                    benilla_world::model_render::lazy::realize_all(materials);
                    if let Some(m) = materials.get(&h) {
                        let mut m = m.clone();
                        m.base.depth_bias = bucket;
                        shard_mats.push(materials.add(m));
                    }
                }
            }
        }
    }

    // The far-side-of-water twins. `classify_water_side` swaps every transparent
    // model material for its `far_twin_of` — a DISTINCT pipeline key (the far marker bit + the
    // shifted bias integer) that the cache never dedups against the near one even though
    // `specialize` makes the descriptors byte-identical. Unwarmed, the first eye-and-model
    // straddle of a water plane is a live compile. Twin everything transparent above, through the
    // swap's own builder, with the swap's own predicate.
    let far_mats = far_twins_of(materials, &mats);
    let far_shard_mats = far_twins_of(materials, &shard_mats);

    let mut count = 0;
    // The main cross and its far twins ride every layout (any family can appear static or
    // skinned, plain or vertex-coloured — a submerged character's skinned gear classifies far
    // too); the shard rows and their far twins ride the static layouts only (shard geometry
    // models are static meshes by construction — `particles::model`). Every main-cross rig is
    // ALSO duplicated onto one portrait booth's layer AND the twin booth's: the
    // booths render at `Msaa::Off`, so each model pipeline has a samples=1 twin per projection
    // CLASS — the real booth's Perspective placeholder and the twin booth's custom projection
    // (the class real bakes install) — that otherwise compiles live on the first in-world
    // portrait. A unit's gear (any family, any layout, far-swapped included when submerged)
    // can reach a booth pane. Shard rows can't (particle instances never ride booth layers).
    //
    // …and onto the ORTHOGRAPHIC twin (2262), the third projection class: the UI model tile
    // atlas's camera. Its rigs are built by `ui_models` through `portrait::material_variant`,
    // which clones the world material and swaps only the light buffer — the same pipeline key,
    // under a class nothing warmed. **Minus the far twins**, which the booths do take: a far
    // twin is `model_render::classify_water_side` swapping a material for an eye-and-model
    // straddle of a water plane, and a tile is a `<Model>` widget's own file on its own stage —
    // there is no water plane in a tile and no classify pass runs over one.
    for (mesh, aabb, skinned) in &layouts {
        for mat in mats.iter().chain(far_mats.iter()) {
            spawn_model_rig(commands, cam, None, mesh, aabb, *skinned, mat);
            count += 1;
            if let Some((booth_cam, layers)) = booth {
                spawn_model_rig(
                    commands,
                    booth_cam,
                    Some(layers.clone()),
                    mesh,
                    aabb,
                    *skinned,
                    mat,
                );
                count += 1;
            }
            spawn_model_rig(
                commands,
                warm_booth.0,
                Some(warm_booth.1.clone()),
                mesh,
                aabb,
                *skinned,
                mat,
            );
            count += 1;
        }
        for mat in &mats {
            spawn_model_rig(
                commands,
                warm_ortho.0,
                Some(warm_ortho.1.clone()),
                mesh,
                aabb,
                *skinned,
                mat,
            );
            count += 1;
        }
        if !*skinned {
            for mat in shard_mats.iter().chain(far_shard_mats.iter()) {
                spawn_model_rig(commands, cam, None, mesh, aabb, false, mat);
                count += 1;
            }
        }
    }

    // The MERGED-BLOB layouts (`WOW_STATIC_MERGE`, 1417–1420): the static layouts plus the baked
    // per-vertex fade sphere, and plus the interior-prop probe slot — four more strides
    // (48/52, and their vertex-coloured 64/68), each a distinct pipeline family via the
    // `WOW_MERGED_FADE`/`WOW_MERGED_SLOT` shader defs `specialize` keys on the layout. Built
    // through the REAL blob mesh builder (`merged_static_mesh_faded`) so the warm layout can't
    // drift from production. World camera only: a blob never reaches a portrait booth and never
    // far-twins (no `DoodadFade`, so `classify_water_side` never swaps it). Missing these was a
    // live compile — a director-felt stall — on the first blob of each family to enter view.
    for colors in [false, true] {
        for slot in [false, true] {
            let part = std::sync::Arc::new(warm_quad(colors, false));
            let (mesh, mn, mx, _center) = benilla_assets::merged_static_mesh_faded(
                &[(part, Transform::IDENTITY)],
                &[Vec4::new(0.0, 0.0, 0.0, 1.0)],
                slot.then_some(&[0u32][..]),
            );
            let aabb = Some(bevy::camera::primitives::Aabb::from_min_max(mn, mx));
            let mesh = meshes.add(mesh);
            for mat in &mats {
                spawn_model_rig(commands, cam, None, &mesh, &aabb, false, mat);
                count += 1;
            }
        }
    }

    // The WMO-skybox rows: the STATIC PLAIN layout only, and only on the world
    // camera. `skybox::build_skybox` inserts POSITION + NORMAL + UV_0 — `layouts[0]` exactly — a
    // skybox is never skinned (the asteroid belts' bones are a deferral, not a shipped lane), never
    // vertex-coloured (M2 carries no MOCV), never far-twinned, and never reaches a portrait booth.
    {
        let (plain_mesh, plain_aabb, _) = &layouts[0];
        for mat in &sky_mats {
            spawn_model_rig(commands, cam, None, plain_mesh, plain_aabb, false, mat);
            count += 1;
        }
    }

    // The sky and water lanes (0837's scope was model-lane-only, and every hole
    // was a director-felt stall: the sun disc first drawn on stepping outdoors, the first water
    // in view, a spell's shards mid-cast). Every material that EXISTS in these Startup-populated
    // stores gets a rig with its production mesh layout — iterating the store can't drift from
    // what's spawned. Layout indices: the loop above pushed [static plain, skinned plain,
    // static colours, skinned colours].
    let (plain_mesh, plain_aabb, _) = layouts[0].clone();
    let (colours_mesh, colours_aabb, _) = layouts[2].clone();
    let posuv = meshes.add(warm_pos_uv_mesh());
    let liquid_mesh = meshes.add(warm_liquid_mesh(false));
    let liquid_color_mesh = meshes.add(warm_liquid_mesh(true));
    // Celestial discs + glares (`sun::setup` quads: position+normal+UV).
    for mat in lane_handles(&mut lanes.celestial) {
        spawn_lane_rig(
            commands,
            cam,
            None,
            &plain_mesh,
            plain_aabb.as_ref(),
            mat,
            &mut count,
        );
    }
    // Stars: the real `Stars.m2` patches carry position+UV only; the assetless fallback dome
    // carries normals too — two distinct pipeline keys, warm both.
    for mat in lane_handles(&mut lanes.stars) {
        spawn_lane_rig(commands, cam, None, &posuv, None, mat.clone(), &mut count);
        spawn_lane_rig(
            commands,
            cam,
            None,
            &plain_mesh,
            plain_aabb.as_ref(),
            mat,
            &mut count,
        );
    }
    // The cloud dome (position+normal+UV+colour) and the gradient dome (position+normal+UV).
    for mat in lane_handles(&mut lanes.clouds) {
        spawn_lane_rig(
            commands,
            cam,
            None,
            &colours_mesh,
            colours_aabb.as_ref(),
            mat,
            &mut count,
        );
    }
    for mat in lane_handles(&mut lanes.sky) {
        spawn_lane_rig(
            commands,
            cam,
            None,
            &plain_mesh,
            plain_aabb.as_ref(),
            mat,
            &mut count,
        );
    }
    // Liquid: every kind × fog-block material `setup_liquid` built, on BOTH of the liquid grid's
    // layouts. An **interior** WMO pool bakes its `MOMT.diffColor` into a fifth attribute —
    // `ATTRIBUTE_COLOR`, `liquid::surface::liquid_bevy_mesh` — which `liquid.wgsl:275` reads behind
    // `#ifdef VERTEX_COLORS`: a different shader-def set AND a different vertex buffer, so a
    // different pipeline, exactly the way the minimap composite's quad was (2262). The ADT and
    // WMO-exterior lanes pass `None` and take the shader's `#else` white. Warming only the
    // four-attribute form left the first indoor water pool — a cave, a dungeon, an inn's basin —
    // compiling live on the frame it came into view.
    for mat in lane_handles(&mut lanes.liquid) {
        spawn_lane_rig(
            commands,
            cam,
            None,
            &liquid_mesh,
            None,
            mat.clone(),
            &mut count,
        );
        spawn_lane_rig(
            commands,
            cam,
            None,
            &liquid_color_mesh,
            None,
            mat,
            &mut count,
        );
    }
    // The plain-`StandardMaterial` lanes (the director's evening log). The fallback cube
    // (`entities::CubeAssets`, drawn while any entity's model streams) uses the production mesh
    // + materials, on the world camera AND a booth layer (a cube-bodied target can reach a
    // portrait pane) — but NOT the orthographic twin (2262): `ui_models` has no cube fallback at
    // all, a tile's root simply stays bare until its M2 is resident, so no cube ever draws
    // through the tile camera. The nameplate and raid-mark materials are built on first need, so the lane
    // warms REPRESENTATIVES with the builders' exact key fields (`nameplates::spawn_nameplates`,
    // `raid_marks::place_marks` — texture presence is not a key axis); the plate/mark quads
    // share the static-plain attribute set.
    if let Some(cubes) = lanes.cubes.as_ref() {
        let (cube_mesh, cube_mats) = cubes.warm_parts();
        for mat in cube_mats {
            spawn_lane_rig(
                commands,
                cam,
                None,
                &cube_mesh,
                None,
                mat.clone(),
                &mut count,
            );
            if let Some((booth_cam, layers)) = booth {
                spawn_lane_rig(
                    commands,
                    booth_cam,
                    Some(layers.clone()),
                    &cube_mesh,
                    None,
                    mat.clone(),
                    &mut count,
                );
            }
            spawn_lane_rig(
                commands,
                warm_booth.0,
                Some(warm_booth.1.clone()),
                &cube_mesh,
                None,
                mat,
                &mut count,
            );
        }
    }
    let plate = lanes.standard.add(StandardMaterial {
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        depth_bias: benilla_world::sky_order::Rung::NAMEPLATE,
        ..default()
    });
    spawn_lane_rig(
        commands,
        cam,
        None,
        &plain_mesh,
        plain_aabb.as_ref(),
        plate,
        &mut count,
    );
    let mark = lanes.standard.add(StandardMaterial {
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    spawn_lane_rig(
        commands,
        cam,
        None,
        &plain_mesh,
        plain_aabb.as_ref(),
        mark,
        &mut count,
    );

    // The **minimap interior composite's tile quad** (1466, warmed by 2262). A `Material2d`
    // pipeline is keyed on `(view key, MESH LAYOUT)`, and `UiQuadMaterial` therefore has TWO
    // pipelines, not the one `warm_ui_quad_lane`'s doc claimed: the HUD's batch mesh is
    // POSITION+UV_0+COLOR, the composite's tile is `Rectangle` — POSITION+NORMAL+UV_0. The
    // composite's own camera cannot be warmed through (it is `is_active: false` until the player
    // is inside a baked WMO interior, and an inactive camera is not extracted at all, so there is
    // no view to specialise against) — but it does not need to be: the two cameras' VIEW keys are
    // byte-identical (both `Msaa::Off`, neither HDR, same tonemap defs — confirmed by diffing the
    // two variants in a `WOW_PIPE_TRACE` inventory, which differ in nothing but the vertex defs
    // and buffer layout). So drawing the composite's mesh + its production material through the
    // player-UI camera mints exactly the composite's pipeline, under the cover.
    //
    // The texture is a real 1×1 stand-in rather than a default handle, so the material's
    // `AsBindGroup` cannot land on the retry path: a `#[texture]` binding whose `Some(handle)`
    // does not resolve returns `RetryNextUpdate`, and a warm rig that never prepares is a warm
    // rig that draws nothing and warms nothing, silently. `warm_effect_lane` binds a stand-in
    // for the same reason.
    let tile_tex = lanes.images.add(Image::default());
    commands.spawn((
        Mesh2d(meshes.add(crate::ui_pass::tile_quad_mesh())),
        MeshMaterial2d(
            lanes
                .ui_quads
                .add(crate::ui_pass::UiQuadMaterial::interior_tile(
                    tile_tex,
                    crate::minimap::INTERIOR_TILE_ALPHA_REF,
                )),
        ),
        // Visible — unlike the model rigs, which the pacing reveals a slice at a time. It is one
        // pipeline, drawn once at a scale that covers no pixel worth speaking of.
        Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::splat(0.001)),
        crate::ui_pass::ui_render_layers(),
        WarmRig,
    ));
    count += 1;

    count
}

/// The far twins of every TRANSPARENT material in `src` — the same `alpha_mode == Blend`
/// predicate `classify_water_side` swaps on, through the swap's own `far_twin_of` builder.
fn far_twins_of(
    materials: &mut Assets<WowModelMaterial>,
    src: &[Handle<WowModelMaterial>],
) -> Vec<Handle<WowModelMaterial>> {
    benilla_world::model_render::lazy::realize_all(materials);
    let twins: Vec<WowModelMaterial> = src
        .iter()
        .filter_map(|h| materials.get(h))
        .filter(|m| matches!(m.base.alpha_mode, AlphaMode::Blend))
        .map(far_twin_of)
        .collect();
    twins.into_iter().map(|m| materials.add(m)).collect()
}

/// Strong handles to every material currently in a lane's store (the iterate-the-store warm:
/// what exists is what gets compiled).
fn lane_handles<M: Material>(assets: &mut Assets<M>) -> Vec<Handle<M>> {
    let ids: Vec<AssetId<M>> = assets.iter().map(|(id, _)| id).collect();
    ids.into_iter()
        .filter_map(|id| assets.get_strong_handle(id))
        .collect()
}

/// One model-lane menagerie entity: a 1 cm rig in front of the camera (world or booth — `layers`
/// puts the rig on the booth camera's layer for the samples=1 twin space), drawn under the cover.
fn spawn_model_rig(
    commands: &mut Commands,
    cam: Entity,
    layers: Option<bevy::camera::visibility::RenderLayers>,
    mesh: &Handle<Mesh>,
    aabb: &Option<bevy::camera::primitives::Aabb>,
    skinned: bool,
    mat: &Handle<WowModelMaterial>,
) {
    let tag = if skinned {
        MeshTag(benilla_world::mesh_tag::rig_bits(0) | benilla_world::mesh_tag::alpha_bits(1.0))
    } else {
        MeshTag(benilla_world::mesh_tag::alpha_bits(1.0))
    };
    let mut e = commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(mat.clone()),
        Transform::from_xyz(0.0, 0.0, -0.5).with_scale(Vec3::splat(0.01)),
        tag,
        WarmRig,
        // Hidden at spawn: a hidden rig is never extracted, so it queues no pipeline. The pass
        // reveals rigs a slice at a time (`super::reveal_slice`) — that is what paces the
        // compile burst instead of paying all 1480 in one blocking frame.
        Visibility::Hidden,
        ChildOf(cam),
    ));
    if let Some(aabb) = aabb {
        e.insert(*aabb);
    }
    if let Some(layers) = layers {
        e.insert(layers);
    }
}

/// One sky/water/standard-lane menagerie entity — same rig, no `MeshTag` (those lanes don't
/// carry one in production, and the tag is instance data, never a pipeline axis).
fn spawn_lane_rig<M: Material>(
    commands: &mut Commands,
    cam: Entity,
    layers: Option<bevy::camera::visibility::RenderLayers>,
    mesh: &Handle<Mesh>,
    aabb: Option<&bevy::camera::primitives::Aabb>,
    mat: Handle<M>,
    count: &mut usize,
) {
    let mut e = commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_xyz(0.0, 0.0, -0.5).with_scale(Vec3::splat(0.01)),
        WarmRig,
        // Hidden at spawn — revealed a slice at a time; see `spawn_model_rig`.
        Visibility::Hidden,
        ChildOf(cam),
    ));
    if let Some(aabb) = aabb {
        e.insert(*aabb);
    }
    if let Some(layers) = layers {
        e.insert(layers);
    }
    *count += 1;
}

/// A tiny triangle carrying POSITION + UV_0 only — the layout the real `Stars.m2` patches ship
/// (`sun::setup` inserts exactly these two attributes). The WMO skybox used to share it; since
/// decision 1264 it draws on the model lane and carries that lane's NORMAL too, so it rides
/// `layouts[0]` instead. Main-world-resident (`RenderAssetUsages::default()`), so `calculate_bounds`
/// covers it — no explicit Aabb needed.
fn warm_pos_uv_mesh() -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
    );
    m.insert_indices(Indices::U32(vec![0, 1, 2]));
    m
}

/// A tiny quad in the liquid grid's layout — POSITION + NORMAL + UV_0 + UV_1, plus
/// `ATTRIBUTE_COLOR` when `body_color` is set.
///
/// **Both forms are real** (2262's lesson, found by the sweep after it): `liquid_bevy_mesh` takes
/// an `Option<[f32; 3]>` body colour and inserts the fifth attribute only for an INTERIOR WMO
/// pool that is not fullbright — the ADT and WMO-exterior lanes pass `None`. `liquid.wgsl:275`
/// reads it behind `#ifdef VERTEX_COLORS`, so the two are separate pipelines. This doc used to
/// claim `liquid::surface` "inserts exactly these four attributes", which is what kept the second
/// one out of the menagerie.
fn warm_liquid_mesh(body_color: bool) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
    );
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs.clone());
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvs);
    if body_color {
        // The production insert's shape exactly (`liquid_bevy_mesh`): one RGBA per vertex, alpha 1.
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0, 1.0, 1.0, 1.0]; 4]);
    }
    m.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    m
}

/// A unit quad in each attribute combination the model lane ships. Every `RenderSubmesh` field
/// is spelled out on purpose: a new field breaks THIS build, which is the drift alarm that keeps
/// the menagerie honest against the format.
fn warm_quad(colors: bool, skinned: bool) -> RenderSubmesh {
    let n = 4usize;
    RenderSubmesh {
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; n],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
        texture: None,
        skin_slot: None,
        geoset_id: 0,
        char_slot: None,
        blend: ModelBlend::Opaque,
        wrap_x: true,
        wrap_y: true,
        two_sided: false,
        joints: if skinned { vec![[0; 4]; n] } else { Vec::new() },
        weights: if skinned {
            vec![[1.0, 0.0, 0.0, 0.0]; n]
        } else {
            Vec::new()
        },
        vertex_colors: if colors {
            vec![[1.0, 1.0, 1.0, 1.0]; n]
        } else {
            Vec::new()
        },
        interior: false,
        emissive: false,
        icon_slot: false,
        uv_rot_seq: None,
        uv_scale_seq: None,
        sidn: None,
        window: false,
        additive: false,
        no_depth_write: false,
        no_depth_test: false,
        fog_policy: FogPolicy::Scene,
        env_map: false,
        billboard: None,
        welded_billboard: false,
        alpha_anim: None,
        uv_anim: None,
        uv_seq: None,
        rgb_anim: None,
        rgb_seq: None,
        wmo_batch: None,
        section: None,
    }
}

#[cfg(test)]
mod tests {
    /// The gate's second half: a lane can also be a hand-rolled
    /// `SpecializedRenderPipeline`/`SpecializedMeshPipeline`/`SpecializedComputePipeline` impl —
    /// invisible to the `MaterialPlugin` scan below, which is exactly how the `wow_effect` lane
    /// (particles, decals, the selection ring) shipped unwarmed and the ring's first-target
    /// compile stalled live twice (0837's inventory missed it too). Every such impl's TYPE must
    /// be named in the pipe_warm module, or this red-bars the build.
    #[test]
    fn every_custom_pipeline_lane_has_a_warm_contributor() {
        // Lanes whose one pipeline compiles covered BY CONSTRUCTION, each with the reason:
        // - UiGammaPipeline (`ui_gamma`): one variant, keyed on the swapchain's
        //   view format, specialised in the first frame's prepare — pre-world, so covered
        //   (`publish_cover`: `state != InWorld`) — and the surface's format never changes
        //   after, so no later variant exists. Not a timing race like `UiQuadMaterial`'s: the
        //   view exists from frame one, and the pipeline is what bevy's own output blit was.
        //   (Its world-lane twin, benilla-world's `FfxCombinePipeline`, is outside this scan
        //   and compiles under the same cover: the player-UI camera's backdrop pair is keyed on
        //   that camera's own main texture and specialised on its first frame, pre-world, and a
        //   bake's pair on the bake image's fixed format. That list was one short — a world view
        //   also has its OWN pair, for the frames nothing claims it, and 2262 found it compiling
        //   live at app exit. `prepare_textures` now specialises that pair on every frame rather
        //   than only on the frame it first needs it.)
        let exempt = ["UiGammaPipeline"];
        let own_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let warm_src = std::fs::read_to_string(own_src.join("pipe_warm/mod.rs")).unwrap()
            + &std::fs::read_to_string(own_src.join("pipe_warm/menagerie.rs")).unwrap();
        let mut missing = Vec::new();
        let files: Vec<_> = workspace_src_roots()
            .iter()
            .flat_map(|r| walk_rs(r))
            .collect();
        for (path, text) in files {
            for needle in [
                "impl SpecializedRenderPipeline for ",
                "impl SpecializedMeshPipeline for ",
                "impl SpecializedComputePipeline for ",
            ] {
                for (i, _) in text.match_indices(needle) {
                    let rest = &text[i + needle.len()..];
                    let ty: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !exempt.contains(&ty.as_str()) && !warm_src.contains(&ty) {
                        missing.push(format!("{ty} (impl in {})", path.display()));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "custom pipeline lanes with no pipe_warm contributor: {missing:?} — a lane the \
             menagerie can't see compiles its pipelines live on first draw (decisions \
             0837/0938/0958)"
        );
    }

    /// **The scan root: every crate in the workspace, not just this one.**
    ///
    /// Both lane-coverage scans below used to walk `CARGO_MANIFEST_DIR/src` — which is
    /// `crates/benilla-app/src`, and every lane either scan exists to catch lives in
    /// **`benilla-world`**: all eight 3-D `MaterialPlugin` registrations, and both real hand-rolled
    /// pipelines (`FfxCombinePipeline`, `EffectPipeline`). The only `Specialized*Pipeline` impl in
    /// benilla-app is `UiGammaPipeline`, which the test then exempts — so the custom-lane assertion
    /// could not fail, ever, and the material assertion could only ever see the two 2-D/UI
    /// families. Both tests were green because they were looking at an empty room; the exempt list
    /// even named `TerrainMaterial`/`WdlMaterial`, types the walk could never reach. Found by the
    /// sweep after 2262, which is the third time in this file's history that a lane shipped
    /// unwarmed because the instrument could not see it.
    fn workspace_src_roots() -> Vec<std::path::PathBuf> {
        // `crates/benilla-app` → `crates`, then every crate's `src` under it.
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("CARGO_MANIFEST_DIR always has a parent")
            .to_path_buf();
        let mut roots: Vec<std::path::PathBuf> = std::fs::read_dir(&crates)
            .expect("the crates/ directory is always readable from a test")
            .filter_map(|e| {
                let p = e.ok()?.path().join("src");
                p.is_dir().then_some(p)
            })
            .collect();
        roots.sort();
        assert!(
            roots.len() > 1,
            "the lane scans must walk the whole workspace — a single root is the bug this \
             function exists to prevent (only {roots:?} found under {})",
            crates.display(),
        );
        roots
    }

    /// Every `.rs` file under `src`, read — shared by both lane-coverage scans.
    fn walk_rs(src_root: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![src_root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                out.push((path, text));
            }
        }
        out
    }

    /// The lane-coverage gate (widened by 0958): every material lane registered
    /// anywhere in this crate — 3-D (`MaterialPlugin::<X>`), 2-D (`Material2dPlugin::<X>`), and
    /// UI (`UiMaterialPlugin::<X>`) — must be NAMED in this file. The warm pass is the one place
    /// that compiles a lane's pipelines behind the loading cover, so a lane nobody considered
    /// for warming is a future director-felt live stall on its first sight. 0938 exempted the
    /// 2-D/UI families wholesale ("pre-world counts as covered"), which was wrong for
    /// `UiQuadMaterial` — its quads only exist in-world, so its one pipeline's compile time was
    /// a race with the cover lift (0958's sweep). Exemptions carry their reason beside them;
    /// anything else red-bars the build. The runtime half of the contract stays
    /// `watch_pipelines`' "compiled LIVE" tripwire, which catches per-VARIANT drift inside a
    /// covered lane; this test catches whole lanes.
    #[test]
    fn every_material_lane_has_a_warm_contributor() {
        // Lanes that never need the menagerie, each with the reason it is safe:
        // - TerrainMaterial / WdlMaterial: the ground the player spawns on and its horizon
        //   ring — always drawn under the entry cover by construction, no per-variant key axis.
        // - AddUiMaterial: drawn only by the glue screens, and every pre-world frame counts
        //   as covered (`publish_cover`: `state != InWorld`).
        let families: [(&str, &[&str]); 3] = [
            ("MaterialPlugin::<", &["TerrainMaterial", "WdlMaterial"]),
            ("Material2dPlugin::<", &[]),
            ("UiMaterialPlugin::<", &["AddUiMaterial"]),
        ];
        let own_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // "Named in this file" = anywhere in the pipe_warm module folder.
        let warm_src = std::fs::read_to_string(own_src.join("pipe_warm/mod.rs")).unwrap()
            + &std::fs::read_to_string(own_src.join("pipe_warm/menagerie.rs")).unwrap();
        let mut missing = Vec::new();
        let files: Vec<_> = workspace_src_roots()
            .iter()
            .flat_map(|r| walk_rs(r))
            .collect();
        for (path, text) in files {
            for (needle, exempt) in families {
                for (i, _) in text.match_indices(needle) {
                    // A preceding ident char means this match is really a longer family's name
                    // (`Material2dPlugin::<` contains no bare `MaterialPlugin::<`, but
                    // `UiMaterialPlugin::<` does) — that family gets its own row above.
                    if i > 0 && (text.as_bytes()[i - 1].is_ascii_alphanumeric()) {
                        continue;
                    }
                    let rest = &text[i + needle.len()..];
                    let Some(end) = rest.find('>') else { continue };
                    // A registration may wrap (`UiMaterialPlugin::<\n    AddUiMaterial,\n>`):
                    // strip the path, any trailing turbofish comma, and surrounding whitespace.
                    let ty = rest[..end]
                        .rsplit("::")
                        .next()
                        .unwrap()
                        .trim()
                        .trim_end_matches(',');
                    if exempt.contains(&ty) || warm_src.contains(ty) {
                        continue;
                    }
                    missing.push(format!("{ty} (registered in {})", path.display()));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "material lanes with no pipe_warm contributor: {missing:?} — every registered \
             lane's pipelines compile behind the loading cover, or its first sight is a live \
             render-thread stall (decisions 0837/0937/0938/0958)"
        );
    }
}
