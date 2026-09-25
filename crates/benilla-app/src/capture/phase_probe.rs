//! `WOW_PHASE` — **what did the renderer actually do with this batch, frame by frame?**
//!
//! Every other flicker instrument we have reads the *scene*: `benilla-visual` says which pixels would
//! not hold still, `WOW_PICK` says what geometry stands at them and whether it is visible. B38
//! exhausted that layer. There, the awning is nearer than the plank behind it, writes depth, compares
//! `GreaterEqual`, is not discarded and is not culled, and keeps the same mesh, material and texture
//! on every single frame — and on half the frames the plank behind it wins the pixel anyway.
//! Under those facts that is impossible, so one of them stops being true
//! somewhere *after* the scene and before the draw, and nothing we had could see into that gap.
//!
//! So: `WOW_PHASE=<uniqueId>` watches every model batch of one placed object and reports, per frame,
//! which render phase each batch landed in and **where in the draw order** it sits. Absent from every
//! phase means it was never submitted, which no amount of looking at pixels can distinguish from
//! losing a depth test. Present, but at a different position than last frame, means the draw order
//! moved — and for two surfaces that tie, draw order *is* the result.
//!
//! `WOW_PHASE_AT=<secs>` (default 20) / `WOW_PHASE_COUNT=<n>` (default 1) shape the sampling the same
//! way the screenshot burst and the ray pick do, so a phase log and a frame stack line up frame for
//! frame and "was it drawn?" can be read against "was this frame dim or bright?".
//!
//! Only the **main** 3D view's phases are read. Shadow and prepass views bin the same entities and
//! would triple every line for nothing; a defect in *those* would show up as the wrong shadow, which
//! is a different report.

use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{ViewBinnedRenderPhases, ViewSortedRenderPhases};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};

use super::probes::ProbeClock;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::interact::WorldObject;

pub(crate) struct PhaseProbePlugin;

impl Plugin for PhaseProbePlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_PHASE").ok();
        // `WOW_PHASE=particles` — the same question asked of every live PARTICLE emitter's quad
        // mesh instead of one placement's batches (a pool that is emitted, meshed, visible
        // and textured, whose pixels never change — "was its mesh ever SUBMITTED to a phase?" is
        // exactly the gap between the sim-side depth dump and the framebuffer).
        // `WOW_PHASE=particles[:<bone>,…]` — the bone list is the ARMING key, not a filter: the
        // watch list is collected once and then held, and a world scene is full of other emitters
        // (torches, braziers) that go live long before a fixture subject spawns. Naming the bones
        // the question is about (`60,61` = the voidwalker's eyes; `44,45,46` = the wisp's three
        // streamers) makes the probe wait for THAT effect instead of latching onto the first live
        // one it sees. Ribbon trails arm and report on the same bone key as emitters.
        let spec = raw.as_deref().map(str::trim);
        let particles = spec.is_some_and(|v| v == "particles" || v.starts_with("particles:"));
        let bones: Vec<u16> = spec
            .and_then(|v| v.strip_prefix("particles:"))
            .map(|list| {
                list.split(',')
                    .filter_map(|b| b.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        let object = spec.and_then(|v| v.parse::<u32>().ok());
        if !particles && object.is_none() {
            warn!(
                "phase: WOW_PHASE wants a placement uniqueId (e.g. 235256) or \
                 `particles[:<bone>,…]` — inert"
            );
            return;
        }
        let object = object.unwrap_or(0);
        let at = std::env::var("WOW_PHASE_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0);
        let count = std::env::var("WOW_PHASE_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1u32)
            .max(1);
        app.insert_resource(PhaseWatch {
            object,
            particles,
            bones,
            at,
            count,
            batches: Vec::new(),
            armed: false,
        })
        .add_systems(Update, (collect_batches, collect_emitters))
        .add_plugins(ExtractResourcePlugin::<PhaseWatch>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            warn!("phase: no render app — inert");
            return;
        };
        // After `PhaseSort`: binning and sorting are both settled, so membership and order are final.
        render_app.add_systems(Render, report_phases.after(RenderSystems::PhaseSort));
    }
}

/// The watch list, and the sampling window. Lives in the main world; the render half reads it through
/// the extract boundary as a plain clone (it is a handful of entities once per sampled frame, so the
/// cheap thing and the correct thing are the same).
#[derive(Resource, Clone)]
struct PhaseWatch {
    /// The placement `uniqueId` whose batches to follow.
    object: u32,
    /// `WOW_PHASE=particles`: follow every live particle emitter's quad mesh instead.
    particles: bool,
    /// `WOW_PHASE=particles:<bone>,…`: don't arm until every one of these emitter bones is live.
    bones: Vec<u16>,
    at: f32,
    count: u32,
    /// The watched batches: main-world entity, its WMO batch order, how it draws, and (particles
    /// mode) its mesh + texture assets — so the render half can say what the GPU-side copies hold
    /// at draw time.
    batches: Vec<WatchedBatch>,
    /// Have we found the object and started sampling?
    armed: bool,
}

impl ExtractResource for PhaseWatch {
    type Source = PhaseWatch;
    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}

/// One watched batch: entity, WMO batch order (or emitter bone), how it draws, and — particles
/// mode only — the mesh + texture asset ids the render half resolves to GPU-side state.
type WatchedBatch = (
    Entity,
    i32,
    String,
    Option<(Option<AssetId<Mesh>>, AssetId<Image>)>,
);

/// Find the object's batch entities once it has streamed in. Re-run until it is found, then hold:
/// the set is per-placement and does not change, and re-collecting every frame would make the
/// watch list itself a moving part of the measurement.
fn collect_batches(
    mut watch: ResMut<PhaseWatch>,
    time: ProbeClock,
    // `WorldObject` rides on **each submesh entity**, not on a parent that holds them as children —
    // the same shape `WOW_PICK` relies on, where a single ray hit carries both its object identity
    // and its batch. So the watch list is a filter over those entities, not a child walk.
    parts: Query<(
        Entity,
        &WorldObject,
        &benilla_world::model_render::ModelPart,
        &MeshMaterial3d<WowModelMaterial>,
    )>,
    materials: Res<Assets<WowModelMaterial>>,
) {
    if watch.armed || time.elapsed_secs() < watch.at {
        return;
    }
    let want = watch.object;
    let mut found: Vec<WatchedBatch> = parts
        .iter()
        .filter(|(_, obj, _, _)| obj.id == want)
        .map(|(e, _, part, mat)| {
            // The WMO authored batch order rides in the material's `sun_scale.y` (`model_render`),
            // which is also how `WOW_PICK` names a batch — so a phase line and a pick line describe
            // the same surface by the same number.
            let order = materials
                .get(&mat.0)
                .map_or(-1, |m| m.extension.sun_scale.y as i32);
            (e, order, format!("{:?}", part.blend), None)
        })
        .collect();
    if found.is_empty() {
        return;
    }
    // Sort by batch order so the report reads in the WMO's authored order, not spawn order.
    found.sort_by_key(|&(_, order, _, _)| order);
    info!(
        "phase: watching {} batches of #{want} for {} frames",
        found.len(),
        watch.count
    );
    watch.batches = found;
    watch.armed = true;
}

/// The `particles` mode's collector: every LIVE **effect** mesh — particle-emitter quad clouds and
/// ribbon trails alike — labeled by its emitter bone + blend (the same identity the sim-side depth
/// dump prints, so a phase line and a dump line name the same pool) — **and every model batch in
/// the scene alongside them**.
///
/// Ribbons are here because they are the same kind of thing under the same law: a trail is one of
/// its owner model's emitters, drawn in that model's own post-batch bracket, and in our renderer it
/// is another `NoFrustumCulling` mesh in the one distance-sorted transparent list. A probe that saw
/// only quads could say "the eye glow is ordered right" while a wisp's streamers were still
/// interleaved with the wisp.
///
/// The emitters alone answer "was it submitted?". They cannot answer B16's actual question, which
/// is *relative*: a particle quad and its own model's blend batches share one `Transparent3d` list
/// sorted back-to-front, so which of the two draws last is what decides whether the eyes survive.
/// That is only readable if both sides are in the same report, on the same frame, with the sort
/// distances that produced the order — so this list holds both, and the batch column says which.
///
/// One-shot like [`collect_batches`], but gated on a live emitter rather than the clock alone: in
/// a fixture capture the subject spawns after streaming settles, so a fixed `WOW_PHASE_AT` either
/// fires before it exists or after the shot. Retrying until the pool is live pins the window to
/// the subject instead of to a wall-clock guess.
fn collect_emitters(
    mut watch: ResMut<PhaseWatch>,
    time: ProbeClock,
    emitters: Query<(Entity, &benilla_world::particles::ParticleEmitter)>,
    trails: Query<(
        Entity,
        &benilla_world::ribbons::RibbonTrail,
        &GlobalTransform,
    )>,
    parts: Query<(
        Entity,
        &benilla_world::model_render::ModelPart,
        &GlobalTransform,
    )>,
) {
    if !watch.particles || watch.armed || time.elapsed_secs() < watch.at {
        return;
    }
    let mut found: Vec<WatchedBatch> = emitters
        .iter()
        .filter(|(_, e)| e.live() > 0)
        .map(|(ent, e)| {
            let def = e.def();
            (
                ent,
                i32::from(def.bone),
                format!("EMIT {:?}(pool {})", def.blend, e.live()),
                // No mesh asset since the shared effect lane (0732 P1) — the texture is the
                // GPU-side link left to check; the vertices ride the lane's one buffer.
                Some((None, e.texture().id())),
            )
        })
        .collect();
    // A trail with fewer than two edges builds an empty mesh, so it is submitted but draws
    // nothing — the same "live" bar the emitters use (`live() > 0`).
    found.extend(trails.iter().filter_map(|(ent, t, _)| {
        let (blend, edges) = t.shape();
        (edges > 0).then(|| {
            (
                ent,
                i32::from(t.bone()),
                format!("RIBB {blend:?}(edges {edges})"),
                None,
            )
        })
    }));
    if found.is_empty() {
        return;
    }
    // The arming key (see the plugin's parse): every named bone must be live, or this frame is too
    // early and the list would freeze around the wrong pool.
    if !watch
        .bones
        .iter()
        .all(|b| found.iter().any(|&(_, bone, _, _)| bone == i32::from(*b)))
    {
        return;
    }
    // The model side of the comparison, scoped to the batches that could actually overlap this
    // pool on screen. A world scene holds tens of thousands of model batches and all of them are
    // in the same sorted list; the ones that matter are the ones standing where the cloud is, so
    // the scope is a radius around the watched anchors rather than "every model in the world".
    const NEAR: f32 = 6.0;
    let anchors: Vec<Vec3> = emitters
        .iter()
        .filter(|(_, e)| e.live() > 0)
        .map(|(_, e)| e.anchor_world())
        // A trail's entity translation IS its sort point (the sim writes the live head node
        // there), so its `GlobalTransform` is the anchor without an accessor of its own.
        .chain(trails.iter().map(|(_, _, gt)| gt.translation()))
        .collect();
    found.extend(
        parts
            .iter()
            .filter(|(_, _, gt)| {
                anchors
                    .iter()
                    .any(|a| gt.translation().distance(*a) <= NEAR)
            })
            .map(|(ent, part, _)| (ent, -1, format!("PART {:?}", part.blend), None)),
    );
    info!(
        "phase: watching {} live effect meshes + model batches for {} frames",
        found.len(),
        watch.count
    );
    watch.batches = found;
    watch.armed = true;
}

/// Where each watched batch sits in each phase, this frame.
fn report_phases(
    watch: Option<Res<PhaseWatch>>,
    opaque: Res<ViewBinnedRenderPhases<Opaque3d>>,
    alpha_mask: Res<ViewBinnedRenderPhases<AlphaMask3d>>,
    transparent: Res<ViewSortedRenderPhases<Transparent3d>>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    mut seen: Local<u32>,
) {
    let Some(watch) = watch else { return };
    if !watch.armed || *seen >= watch.count {
        return;
    }
    let frame = *seen;
    *seen += 1;
    // The whole-frame census, before the per-batch lines. Every instrument we have is scoped to the
    // thing we already suspect — this object's batches, `WorldObject`-tagged ray hits, the depth
    // between the opaque and transmissive passes — so a draw that is *none of those* is invisible to
    // all of them at once. An alpha-blended sprite over the awning (a particle, a glow card) would
    // halve the pixel while leaving depth, material, tag, draw order and the light buffer exactly as
    // measured, which is precisely the contradiction B38 now sits in. A transparent count that moves
    // between a bright and a dim frame names that draw; one that does not, rules out the whole family.
    let transparent_total: usize = transparent.values().map(|p| p.items.len()).sum();
    info!(
        "phase#{frame} CENSUS opaque {} alphamask {} transparent {}",
        binned_total(&opaque),
        binned_total(&alpha_mask),
        transparent_total,
    );
    // `particles` mode: the transparent phase's TAIL — the items drawn LAST, i.e. over everything
    // else. A watched mesh that is submitted, late, correct and still invisible means someone
    // even later paints its pixels; this names the suspects with their sort distances.
    if watch.particles {
        if let Some(phase) = transparent.values().max_by_key(|p| p.items.len()) {
            let n = phase.items.len();
            for (i, item) in phase.items.iter().enumerate().skip(n.saturating_sub(25)) {
                info!(
                    "phase#{frame} tail @{i} {:?} dist {:.3}",
                    item.entity.1, item.distance
                );
            }
        }
    }
    for &(entity, submesh, ref blend, assets) in &watch.batches {
        // What the GPU-side copies hold at draw time — the last links between "submitted to a
        // phase" and "fragments exist": a phase item whose RenderMesh is absent draws nothing,
        // and one whose material TEXTURE never became a GpuImage has no material bind group, so
        // the draw command chain aborts silently every frame.
        let gpu = assets.map(|(mesh_id, tex_id)| {
            let mesh = match mesh_id {
                // The shared effect lane: no per-emitter mesh asset exists to check.
                None => "shared-lane".to_string(),
                Some(id) => match render_meshes.get(id) {
                    None => "gpu_mesh MISSING".to_string(),
                    Some(m) => format!("gpu_verts {}", m.vertex_count),
                },
            };
            let tex = match gpu_images.get(tex_id) {
                None => "gpu_tex MISSING".to_string(),
                Some(img) => format!("gpu_tex {:?}", img.texture_format),
            };
            format!("{mesh} {tex}")
        });
        let main = MainEntity::from(entity);
        let mut found = Vec::new();
        // The main view is whichever phase map holds the most entities: shadow and prepass views bin
        // subsets, so the largest is the one that draws the frame. Picking by view entity would tie
        // this to the camera's identity in the render world, which is not stable to read from here.
        if let Some(pos) = binned_position::<Opaque3d>(&opaque, main) {
            found.push(format!("Opaque3d @{pos}"));
        }
        if let Some(pos) = binned_position::<AlphaMask3d>(&alpha_mask, main) {
            found.push(format!("AlphaMask3d @{pos}"));
        }
        for phase in transparent.values() {
            if let Some(pos) = phase.items.iter().position(|item| item.entity.1 == main) {
                // The sort key itself, not just the slot: `Transparent3d` orders on view-space z +
                // the material's depth bias, ascending = farthest first (`benilla_world::sky_order`). Two
                // slots tell you WHICH drew last; the two distances tell you WHY, and whether the
                // gap is a real spatial one or a tie the sort broke arbitrarily.
                found.push(format!(
                    "Transparent3d @{pos} d {:.3}",
                    phase.items[pos].distance
                ));
            }
        }
        // "NOT SUBMITTED" is the whole point of the instrument: a batch that reached no phase was
        // never drawn, and that is indistinguishable in the pixels from one that drew and lost.
        // The entity, because **batch order does not identify a surface**: a WMO placement is many
        // groups and the order is per-group, so one placement has several batches sharing an order
        // (79 entities over 19 orders on the Far Watch Post tower). Keying a report by order alone
        // silently merges them and can hide the one batch that moved.
        info!(
            "phase#{frame} {entity} batch order {submesh:3} {blend:10} {} -> {}",
            gpu.as_deref().unwrap_or(""),
            if found.is_empty() {
                "NOT SUBMITTED".to_string()
            } else {
                found.join(", ")
            }
        );
    }
}

/// The entity's ordinal within the largest binned phase for `BPI`, walking bins in iteration order —
/// which is the order the pass draws them, so the ordinal is a draw position and not just a yes/no.
fn binned_position<BPI>(phases: &ViewBinnedRenderPhases<BPI>, main: MainEntity) -> Option<usize>
where
    BPI: bevy::render::render_phase::BinnedPhaseItem,
{
    let phase = phases.values().max_by_key(|p| {
        p.multidrawable_meshes
            .values()
            .map(|bins| bins.values().map(|b| b.entities().len()).sum::<usize>())
            .sum::<usize>()
            + p.batchable_meshes
                .values()
                .map(|b| b.entities().len())
                .sum::<usize>()
    })?;
    let mut n = 0usize;
    for bins in phase.multidrawable_meshes.values() {
        for bin in bins.values() {
            if let Some(i) = bin.entities().get_index_of(&main) {
                return Some(n + i);
            }
            n += bin.entities().len();
        }
    }
    for bin in phase.batchable_meshes.values() {
        if let Some(i) = bin.entities().get_index_of(&main) {
            return Some(n + i);
        }
        n += bin.entities().len();
    }
    None
}

/// How many entities the main binned phase for `BPI` holds this frame — the census counterpart of
/// [`binned_position`], picking the same "largest phase map is the main view" way so the two numbers
/// describe the same view.
fn binned_total<BPI>(phases: &ViewBinnedRenderPhases<BPI>) -> usize
where
    BPI: bevy::render::render_phase::BinnedPhaseItem,
{
    let count = |p: &bevy::render::render_phase::BinnedRenderPhase<BPI>| {
        p.multidrawable_meshes
            .values()
            .map(|bins| bins.values().map(|b| b.entities().len()).sum::<usize>())
            .sum::<usize>()
            + p.batchable_meshes
                .values()
                .map(|b| b.entities().len())
                .sum::<usize>()
    };
    phases.values().map(count).max().unwrap_or(0)
}
