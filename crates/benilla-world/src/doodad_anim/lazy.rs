//! The lazy palette-rig lane: a placed doodad claims its skin-palette slot at its first draw-gate
//! wake, not at spawn, keeps it while parked, and gives it back only under table pressure. An
//! emitter-only host has no skinned parts, gets no [`LazyRig`] and never takes a slot.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::rig_palette::{RigPalettes, RigSkin};

use super::DoodadAnimHost;

/// Reaping starts below this slot headroom: room for the eager lanes and a stream-in burst.
pub(crate) const REAP_LOW_WATER: usize = 256;

/// Demotions per frame under pressure: enough to outpace a stream-in without a one-frame spike.
const REAP_PER_FRAME: usize = 64;

/// Seconds a host must be parked before it is reaped, so a camera swing and back does not churn.
const REAP_MIN_PARKED_SECS: f32 = 2.0;

/// On the anim-host root of a placement with skinned parts: what a wake's allocation needs.
#[derive(Component)]
pub(crate) struct LazyRig {
    /// The skeleton's bone count, the allocation size.
    pub(crate) bones: u32,
    pub(crate) ibp: Handle<SkinnedMeshInverseBindposes>,
    /// The [`SkinnedTwin`] part entities the promote and demote swap.
    pub(crate) parts: Vec<Entity>,
}

/// A part's static and skinned meshes, swapped by promote and demote and kept resident here.
#[derive(Component)]
pub(crate) struct SkinnedTwin {
    pub(crate) skinned: Handle<Mesh>,
    pub(crate) stat: Handle<Mesh>,
}

/// The part query both edges rewrite; a part despawned mid-frame is skipped, its host with it.
pub(super) type TwinParts<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Mesh3d,
        &'static mut MeshTag,
        &'static SkinnedTwin,
    ),
>;

/// The wake edge: allocate the slot, seed its rows from the pose buffer, then swap the parts, so
/// the first skinned frame shows the pose the static mesh did. Returns whether the host rigged.
pub(super) fn promote_lazy_rig(
    commands: &mut Commands,
    palettes: &mut RigPalettes,
    ibps: &Assets<SkinnedMeshInverseBindposes>,
    worlds: &Query<&GlobalTransform>,
    root: Entity,
    lazy: &LazyRig,
    pose: Option<&crate::rig_anim::RigPose>,
    parts: &mut TwinParts,
) -> bool {
    let Some(rig) = RigSkin::allocate_bones(palettes, lazy.bones, lazy.ibp.clone()) else {
        return false; // table full: the gate retries while drawn
    };
    let slot = rig.slot;
    if let (Some(ibp), Some(pose)) = (ibps.get(&lazy.ibp), pose) {
        // Rig-relative, from the root's propagated world, as the world pass composes.
        let root_g = worlds.get(root).copied().unwrap_or_default();
        crate::rig_anim::seed_rig_rows(pose, root_g, &rig, ibp, palettes);
    }
    // Liveness-checked at apply: only `RigSkin`'s `on_replace` hook frees the slot, so an insert
    // (or a failed `try_insert`) racing this frame's tile-unload despawn would leak it.
    commands.queue(
        move |world: &mut bevy::ecs::world::World| match world.get_entity_mut(root) {
            Ok(mut e) => {
                e.insert(rig);
            }
            Err(_) => {
                let slot = rig.slot;
                world.resource_mut::<RigPalettes>().free(slot);
            }
        },
    );
    for &part in &lazy.parts {
        let Ok((mut mesh, mut tag, twin)) = parts.get_mut(part) else {
            continue;
        };
        mesh.0 = twin.skinned.clone();
        tag.0 = crate::mesh_tag::with_rig(tag.0, slot);
    }
    true
}

/// The promote's inverse; removing the [`RigSkin`] frees the slot through its hook.
fn demote_lazy_rig(commands: &mut Commands, root: Entity, lazy: &LazyRig, parts: &mut TwinParts) {
    for &part in &lazy.parts {
        let Ok((mut mesh, mut tag, twin)) = parts.get_mut(part) else {
            continue;
        };
        mesh.0 = twin.stat.clone();
        tag.0 = crate::mesh_tag::with_rig(tag.0, 0);
    }
    // A host despawned this frame already freed its slot; `entity(root)` would panic.
    commands.queue(move |world: &mut bevy::ecs::world::World| {
        if let Ok(mut e) = world.get_entity_mut(root) {
            e.remove::<RigSkin>();
        }
    });
}

/// Under [`REAP_LOW_WATER`] headroom, demote up to [`REAP_PER_FRAME`] parked hosts, oldest first;
/// only hosts this lane promoted, never the eager lanes.
pub(super) fn reap_parked_rigs(
    time: Res<Time>,
    palettes: Res<RigPalettes>,
    hosts: Query<(Entity, &DoodadAnimHost, &LazyRig), With<RigSkin>>,
    mut parts: TwinParts,
    mut commands: Commands,
) {
    if palettes.slot_headroom() >= REAP_LOW_WATER {
        return;
    }
    let now = time.elapsed_secs();
    let mut parked: Vec<(f32, Entity)> = hosts
        .iter()
        .filter(|(_, host, _)| !host.active && now - host.parked_at >= REAP_MIN_PARKED_SECS)
        .map(|(root, host, _)| (host.parked_at, root))
        .collect();
    parked.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(_, root) in parked.iter().take(REAP_PER_FRAME) {
        let Ok((_, _, lazy)) = hosts.get(root) else {
            continue;
        };
        demote_lazy_rig(&mut commands, root, lazy, &mut parts);
        // Headroom moves only when the queued removals apply, so the batch size is the cap.
    }
}

#[cfg(test)]
mod tests {
    use super::super::{gate_doodad_anim, DoodadAnimHost};
    use super::*;
    use bevy::animation::graph::AnimationNodeIndex;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    fn app() -> App {
        let mut app = App::new();
        // `Time` is manual: the real-clock driver would clobber the tests' own advances.
        app.add_plugins((
            bevy::MinimalPlugins
                .build()
                .disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
        ));
        app.init_resource::<Time>();
        app.init_resource::<crate::view::ViewDistance>();
        app.init_resource::<crate::wmo_portal::ExteriorWindows>();
        app.init_resource::<crate::wmo_portal::CameraInteriorClaim>();
        app.init_resource::<RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.init_asset::<Mesh>();
        app.add_systems(Update, (gate_doodad_anim, reap_parked_rigs).chain());
        app
    }

    /// A host with a one-bone [`LazyRig`] whose single [`SkinnedTwin`] part carries its visibility.
    fn lazy_host(app: &mut App, visible: bool) -> (Entity, Entity) {
        let stat = Handle::<Mesh>::default();
        let skinned = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .reserve_handle();
        let part = app
            .world_mut()
            .spawn((
                Mesh3d(stat.clone()),
                MeshTag(crate::mesh_tag::alpha_bits(1.0)),
                SkinnedTwin {
                    skinned,
                    stat: stat.clone(),
                },
                if visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ))
            .id();
        let ibp = app
            .world_mut()
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY]));
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: vec![part],
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: Some((AnimationNodeIndex::new(1), 2.0)),
                    armed_at: 0.0,
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: false,
                    parked_at: 0.0,
                },
                LazyRig {
                    bones: 1,
                    ibp,
                    parts: vec![part],
                },
            ))
            .id();
        let skeleton = benilla_assets::ModelSkeleton {
            joints: vec![benilla_assets::ModelJoint {
                parent: -1,
                local_translation: Vec3::ZERO,
                billboard: None,
                parent_arm: None,
            }],
            spine_bone: None,
            head_bone: None,
        };
        let pose = crate::rig_anim::RigPose::new(host, &skeleton);
        app.world_mut().entity_mut(host).insert(pose);
        (host, part)
    }

    fn rig_slot(app: &App, host: Entity) -> Option<u16> {
        app.world().entity(host).get::<RigSkin>().map(|r| r.slot)
    }

    fn part_state(app: &mut App, part: Entity) -> (Handle<Mesh>, u16) {
        let e = app.world().entity(part);
        (
            e.get::<Mesh3d>().unwrap().0.clone(),
            crate::mesh_tag::rig_of(e.get::<MeshTag>().unwrap().0),
        )
    }

    #[test]
    fn a_host_rigs_at_first_wake_not_at_spawn() {
        let mut app = app();
        let (host, part) = lazy_host(&mut app, false);
        app.update();
        assert_eq!(rig_slot(&app, host), None, "hidden ⇒ no slot claimed");
        assert_eq!(
            app.world().resource::<RigPalettes>().occupancy().0,
            0,
            "the table is untouched while the host is parked"
        );

        *app.world_mut()
            .entity_mut(part)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Inherited;
        app.update();
        assert_eq!(
            rig_slot(&app, host),
            None,
            "one drawn frame is the spawn-race lie — no slot yet"
        );
        app.update();
        let slot = rig_slot(&app, host).expect("second drawn frame ⇒ slot");
        let (mesh, tag_rig) = part_state(&mut app, part);
        let twin = app
            .world()
            .entity(part)
            .get::<SkinnedTwin>()
            .unwrap()
            .skinned
            .clone();
        assert_eq!(mesh, twin, "the part swapped to its skinned twin");
        assert_eq!(tag_rig, slot, "the tag's rig field names the new slot");
        assert_eq!(
            app.world().resource::<RigPalettes>().computed_rigs(),
            1,
            "the promote seeded the rows — never a zeroed (origin-collapsed) first frame"
        );
    }

    #[test]
    fn a_denied_wake_retries_until_the_table_has_room() {
        let mut app = app();
        // Fill all 2047 slots with one-bone rigs, held so they stay live.
        let hoard: Vec<RigSkin> = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            std::iter::from_fn(|| RigSkin::allocate_bones(&mut palettes, 1, Handle::default()))
                .collect()
        };
        assert_eq!(app.world().resource::<RigPalettes>().slot_headroom(), 0);

        let (host, _part) = lazy_host(&mut app, true);
        app.update(); // frame 1: the spawn-race frame never promotes
        app.update(); // frame 2: drawn, denied by the full table
        assert_eq!(
            rig_slot(&app, host),
            None,
            "full table ⇒ denied, static mesh"
        );

        let freed = hoard[0].slot;
        app.world_mut().resource_mut::<RigPalettes>().free(freed);
        app.update();
        assert!(rig_slot(&app, host).is_some(), "room ⇒ the retry lands");
    }

    #[test]
    fn pressure_reaps_the_parked_and_the_next_wake_re_rigs() {
        let mut app = app();
        let (host, part) = lazy_host(&mut app, true);
        app.update();
        app.update(); // the second drawn frame promotes
        assert!(rig_slot(&app, host).is_some(), "drawn ⇒ rigged");

        // Park and age past the hysteresis: with headroom, the slot stays.
        *app.world_mut()
            .entity_mut(part)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Hidden;
        app.update();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                REAP_MIN_PARKED_SECS + 1.0,
            ));
        app.update();
        assert!(
            rig_slot(&app, host).is_some(),
            "parked with headroom ⇒ the slot is kept (zero churn)"
        );

        // Choke the table below the low-water mark.
        let _hoard: Vec<RigSkin> = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            (0..(crate::mesh_tag::MAX_RIG_SLOTS - 1 - REAP_LOW_WATER))
                .filter_map(|_| RigSkin::allocate_bones(&mut palettes, 1, Handle::default()))
                .collect()
        };
        app.update();
        app.update(); // the queued removal applies
        assert_eq!(
            rig_slot(&app, host),
            None,
            "pressure ⇒ the parked slot is reaped"
        );
        let (mesh, tag_rig) = part_state(&mut app, part);
        assert_eq!(
            mesh,
            app.world()
                .entity(part)
                .get::<SkinnedTwin>()
                .unwrap()
                .stat
                .clone(),
            "the part demoted to its static form"
        );
        assert_eq!(tag_rig, 0, "and its tag's rig field cleared");

        // Re-wake: the reaped slot is free again, and two drawn frames re-promote.
        *app.world_mut()
            .entity_mut(part)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Inherited;
        app.update();
        app.update();
        assert!(rig_slot(&app, host).is_some(), "the next wake re-rigs");
    }

    #[test]
    fn a_host_without_skinned_parts_never_takes_a_slot() {
        let mut app = app();
        let (host, _part) = lazy_host(&mut app, true);
        app.world_mut().entity_mut(host).remove::<LazyRig>();
        app.update();
        assert_eq!(rig_slot(&app, host), None);
        assert_eq!(app.world().resource::<RigPalettes>().occupancy().0, 0);
    }
}

/// Bevy's `calculate_bounds` recomputes a bound whenever `Mesh3d` changes, so an animated
/// placement's authored all-animation bound survives the lazy rig's twin swap only under
/// `NoAutoAabb`; these tests pin that Bevy contract.
#[cfg(test)]
mod bound_survives_the_twin_swap {
    use super::*;
    use bevy::camera::primitives::Aabb;
    use bevy::camera::visibility::NoAutoAabb;

    /// A 1 yd cube, the skinned twin's bind-pose bound.
    fn tiny_mesh() -> Mesh {
        Mesh::from(bevy::math::primitives::Cuboid::new(1.0, 1.0, 1.0))
    }

    /// The authored all-animation bound: a bird's 67 yd circuit around that 1 yd body.
    fn authored() -> Aabb {
        Aabb::from_min_max(Vec3::new(-4.2, 8.8, -30.5), Vec3::new(13.6, 16.0, 36.6))
    }

    /// Bevy's bounds system over the authored bound, right after a promote-style `Mesh3d` swap.
    fn swapped(guard: bool) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Mesh>();
        app.add_systems(Update, bevy::camera::visibility::calculate_bounds);
        let stat = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(tiny_mesh());
        let skinned = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(tiny_mesh());
        let e = app.world_mut().spawn((Mesh3d(stat), authored())).id();
        if guard {
            app.world_mut().entity_mut(e).insert(NoAutoAabb);
        }
        app.update();
        app.world_mut().entity_mut(e).get_mut::<Mesh3d>().unwrap().0 = skinned;
        app.update();
        (app, e)
    }

    fn bound(app: &App, e: Entity) -> Aabb {
        *app.world().entity(e).get::<Aabb>().expect("a bound")
    }

    #[test]
    fn an_unguarded_bound_is_clobbered_by_the_mesh_swap() {
        let (app, e) = swapped(false);
        let b = bound(&app, e);
        assert!(
            b.half_extents.max_element() < 1.0,
            "expected the 1 yd cube's own bound, got half-extents {:?} — if this now keeps the \
             authored box, Bevy changed `calculate_bounds` and `NoAutoAabb` may be unnecessary",
            b.half_extents
        );
    }

    #[test]
    fn a_guarded_bound_survives_the_mesh_swap() {
        let (app, e) = swapped(true);
        let b = bound(&app, e);
        assert!(
            (Vec3::from(b.min()) - Vec3::from(authored().min())).length() < 1e-3
                && (Vec3::from(b.max()) - Vec3::from(authored().max())).length() < 1e-3,
            "the authored all-animation bound must survive the promote, got {:?}..{:?}",
            b.min(),
            b.max()
        );
    }
}
