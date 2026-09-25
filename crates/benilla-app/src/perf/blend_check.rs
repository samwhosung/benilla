//! The additive blend-state check: does every transparent draw bind the blend state its
//! material asked for?
//!
//! An additive glow card needs two halves to agree: the material's marker (`clutter_fade.z`
//! bit 2) makes `wow_model.wgsl` fold the radial alpha into the colour, and the same marker,
//! through `WowModelExt::specialize`, gives the pipeline its pure `(ONE, ONE)` add. A folded
//! colour on an alpha-blend pipeline draws a dim skirt; an unfolded colour on the pure add draws
//! a hard, saturated disc.
//!
//! Render world, `Cleanup`, every frame: each `Transparent3d` item bound to one of our materials
//! has its pipeline's colour blend compared with the material's marker. Mismatches are counted
//! into an atomic the pill prints red, and runs that outlast the specialization tick are logged
//! with the entity, the material and the blend state bound.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bevy::asset::AssetId;
use bevy::core_pipeline::core_3d::Transparent3d;
use bevy::pbr::RenderMaterialInstances;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_phase::ViewSortedRenderPhases;
use bevy::render::render_resource::{BlendFactor, PipelineCache};
use bevy::render::{Render, RenderApp, RenderSystems};

use benilla_assets::materials::WowModelMaterial;

/// The realized materials whose marker says additive, so their draws must bind `(ONE, ONE)`;
/// kept from the main world's asset events and extracted as an `Arc` clone each frame.
#[derive(Resource, Clone, Default, ExtractResource)]
pub(crate) struct AdditiveMaterials(pub(crate) Arc<HashSet<AssetId<WowModelMaterial>>>);

/// Draws this frame whose bound blend state contradicted their material's additive marker.
#[derive(Resource, Clone)]
pub(crate) struct BlendMismatchShared(pub(crate) Arc<AtomicU64>);

/// Consecutive frames a mismatch must last to warn: a one-frame run is the pipeline
/// specialization catching up with a material swap.
const REPORT_AFTER_FRAMES: u32 = 2;

/// One mismatching draw, tracked across frames so a transient reads differently from a
/// persisting one.
pub(crate) struct Mismatch {
    pub(crate) material: AssetId<WowModelMaterial>,
    /// Whether the bound pipeline was the pure add (so the material was not additive).
    pub(crate) bound_add: bool,
    /// Consecutive frames this entity has mismatched.
    pub(crate) frames: u32,
    /// The `frames` count the main world last reported at.
    pub(crate) reported: u32,
    /// The draw has left the transparent phase; kept one more pass so the main world can report
    /// the run's length.
    pub(crate) ended: bool,
    /// The end has been reported.
    pub(crate) end_reported: bool,
}

/// The live mismatch table by main-world entity, kept by the render world and reported by
/// [`report_blend_mismatches`] with the names only the main world knows.
#[derive(Resource, Clone, Default)]
pub(crate) struct BlendMismatchLive(pub(crate) Arc<Mutex<HashMap<Entity, Mismatch>>>);

/// Main world: keep [`AdditiveMaterials`] current off the material asset events.
fn track_additive_materials(
    mut events: MessageReader<AssetEvent<WowModelMaterial>>,
    materials: Res<Assets<WowModelMaterial>>,
    mut set: ResMut<AdditiveMaterials>,
) {
    let mut changed = false;
    let mut next: Option<HashSet<AssetId<WowModelMaterial>>> = None;
    for ev in events.read() {
        let (id, present) = match ev {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => (*id, true),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => (*id, false),
            AssetEvent::LoadedWithDependencies { id } => (*id, true),
        };
        let additive = present
            && materials
                .get(id)
                .is_some_and(|m| (m.extension.clutter_fade.z as u32) & 4 != 0);
        let s = next.get_or_insert_with(|| (*set.0).clone());
        let moved = if additive {
            s.insert(id)
        } else {
            s.remove(&id)
        };
        changed |= moved;
    }
    if changed {
        if let Some(s) = next {
            set.0 = Arc::new(s);
        }
    }
}

/// Render world, `Cleanup`: the check itself.
fn check_blend_states(
    additive: Res<AdditiveMaterials>,
    shared: Res<BlendMismatchShared>,
    live: Res<BlendMismatchLive>,
    instances: Res<RenderMaterialInstances>,
    transparent: Option<Res<ViewSortedRenderPhases<Transparent3d>>>,
    pipeline_cache: Res<PipelineCache>,
) {
    let mut mismatches = 0u64;
    let mut seen: HashSet<Entity> = HashSet::new();
    let mut table = live.0.lock().unwrap();
    if let Some(phases) = transparent {
        for phase in phases.0.values() {
            for item in &phase.items {
                let Some(inst) = instances.instances.get(&item.entity.1) else {
                    continue;
                };
                let Ok(id) = inst.asset_id.try_typed::<WowModelMaterial>() else {
                    continue; // not one of ours (the sky dome's StandardMaterial)
                };
                let expect_add = additive.0.contains(&id);
                let blend = pipeline_cache
                    .get_render_pipeline_descriptor(item.pipeline)
                    .fragment
                    .as_ref()
                    .and_then(|f| f.targets.first())
                    .and_then(|t| t.as_ref())
                    .and_then(|t| t.blend);
                let is_add = blend.is_some_and(|b| {
                    b.color.src_factor == BlendFactor::One && b.color.dst_factor == BlendFactor::One
                });
                if expect_add == is_add {
                    continue;
                }
                mismatches += 1;
                let main = item.entity.1.id();
                if seen.insert(main) {
                    let e = table.entry(main).or_insert(Mismatch {
                        material: id,
                        bound_add: is_add,
                        frames: 0,
                        reported: 0,
                        ended: false,
                        end_reported: false,
                    });
                    e.material = id;
                    e.bound_add = is_add;
                    e.frames += 1;
                    e.ended = false; // back in the phase: the run continues
                }
            }
        }
    }
    // An entry that left the phase is held one extra pass marked `ended`, so its run's length
    // can be reported.
    table.retain(|e, m| {
        if seen.contains(e) {
            return true;
        }
        if m.ended {
            return false; // its end has been reported
        }
        m.ended = true;
        true
    });
    shared.0.store(mismatches, Ordering::Relaxed);
}

/// Main world, `PostUpdate`: report the live table with the object and texture names the render
/// world lacks, once a run reaches [`REPORT_AFTER_FRAMES`], every 120 frames after, and at its
/// end.
fn report_blend_mismatches(
    live: Res<BlendMismatchLive>,
    objects: Query<&benilla_world::interact::WorldObject>,
    materials: Res<Assets<WowModelMaterial>>,
    server: Res<AssetServer>,
) {
    let mut table = live.0.lock().unwrap();
    for (entity, m) in table.iter_mut() {
        // A run that ends without surviving the specialization tick logs at debug, not warn.
        let ending = m.ended && !m.end_reported;
        if ending {
            m.end_reported = true;
            if m.reported == 0 {
                debug!(
                    "blend mismatch: transient on entity {entity} — ran {} frame(s), never \
                     survived the specialization tick",
                    m.frames,
                );
                continue;
            }
        } else {
            let due = m.frames == REPORT_AFTER_FRAMES
                || (m.frames > REPORT_AFTER_FRAMES && m.frames % 120 == 0);
            if !due || m.reported == m.frames {
                continue;
            }
        }
        m.reported = m.frames;
        let who = objects.get(*entity).map_or_else(
            |_| "<no WorldObject>".to_string(),
            |o| format!("{:?} #{} {}", o.kind, o.id, o.label),
        );
        let tex = materials
            .get(m.material)
            .and_then(|mat| mat.base.base_color_texture.as_ref())
            .and_then(|h| server.get_path(h.id()))
            .map_or("?".to_string(), |p| p.to_string());
        let mat = match m.material {
            AssetId::Index { index, .. } => format!("#{}", index.to_bits()),
            AssetId::Uuid { uuid } => uuid.to_string(),
        };
        warn!(
            "blend mismatch: {who} entity {entity} material {mat} tex {tex} — material \
             additive={} but bound pipeline {} — {}{}",
            !m.bound_add,
            if m.bound_add {
                "ADD (One,One)"
            } else {
                "alpha-blend"
            },
            if ending {
                format!("ran {} frame(s), now clear", m.frames)
            } else {
                format!("{} frame(s) so far", m.frames)
            },
            if !ending && m.frames > 1 {
                " PERSISTING"
            } else {
                ""
            },
        );
    }
}

pub(crate) fn plugin(app: &mut App) {
    let shared = Arc::new(AtomicU64::new(0));
    let live = BlendMismatchLive::default();
    app.init_resource::<AdditiveMaterials>()
        .insert_resource(BlendMismatchShared(shared.clone()))
        .insert_resource(live.clone())
        .add_plugins(ExtractResourcePlugin::<AdditiveMaterials>::default())
        // After the asset event flush, like bevy's `check_entities_needing_specialization`: a
        // material realized this frame is drawn this frame, and a late `Added` read reports
        // its first draw as a false mismatch.
        .add_systems(
            PostUpdate,
            (
                track_additive_materials.after(bevy::asset::AssetEventSystems),
                report_blend_mismatches,
            ),
        );
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(BlendMismatchShared(shared))
        .insert_resource(live)
        .add_systems(Render, check_blend_states.in_set(RenderSystems::Cleanup));
}
