//! The lazy palette-rig lane: a placed doodad's slot in the 2048-entry skin
//! palette is claimed at its **first draw-gate wake**, not at spawn — and given back under table
//! pressure while the host is parked.
//!
//! Why: the slot table — not the 131 k-bone slab — is the palette's scarce axis, and the eager
//! design spent it on the wrong population. The 0863 census across an Elwynn/Stormwind/Westfall
//! hop leg: 1300–1750 of ~1900 live slots were **parked** doodad hosts (drawn by nobody, player
//! stopped, slot held), the drawn-set peak was ~630, and the table hit its 2047 cap — at which
//! point every creature streaming in at the visibility boundary was refused a rig and froze at
//! bind pose *permanently* (the director's "statue mobs after flying around"). Slots that follow
//! the drawn set leave the table sized 3× over the real demand.
//!
//! The lane's three laws:
//! - **Promote at wake** ([`promote_lazy_rig`], called from the draw gate): allocate, write the
//!   current pose's rows (so the swapped-in skinned mesh never renders zeroed — origin-collapsed
//!   — rows), insert the [`RigSkin`], swap every [`SkinnedTwin`] part `static → skinned` and
//!   write its tag's rig field. A denial (table momentarily full) is retried every frame the
//!   host stays drawn — never permanent.
//! - **Keep while parked** — parking alone frees nothing, so a camera pan is zero churn,
//!   exactly the pre-0863 behaviour when the table has room.
//! - **Reap under pressure** ([`reap_parked_rigs`]): when slot headroom drops under
//!   [`REAP_LOW_WATER`], the longest-parked hosts demote (skinned → static, rig field cleared,
//!   `RigSkin` removed — the component hook frees the slot). A parked host's meshes are all
//!   `Visibility::Hidden`, so the demote is invisible by construction.
//!
//! An emitter-only host (a chimney's smoke plume: joints for the emitter to ride, no skinned
//! parts) never gets a [`LazyRig`], so it never takes a slot at all — under the eager design it
//! held one nobody could read.

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::rig_palette::{RigPalettes, RigSkin};

use super::DoodadAnimHost;

/// Start reclaiming parked rigs when the palette's slot headroom drops below this. Sized to keep
/// real room for the eager lanes (units at ~120 peak, held items, spell fx) plus a stream-in
/// burst, while never engaging at all in ordinary scenes.
pub(crate) const REAP_LOW_WATER: usize = 256;

/// Demotions per frame while under pressure — enough to outpace any stream-in burst (a full
/// tile's placements land over several frames) without a single-frame command spike.
const REAP_PER_FRAME: usize = 64;

/// A host must have been parked this long (secs) before the reaper may take its slot — a camera
/// swing across the pack must not demote what the swing-back re-promotes.
const REAP_MIN_PARKED_SECS: f32 = 2.0;

/// On the anim-host root: what the wake-edge allocation needs, held until the host is first
/// drawn. Present ⇔ the placement has skinned parts worth a slot.
#[derive(Component)]
pub(crate) struct LazyRig {
    /// The skeleton's bone count — the allocation size. No joint list since decision 1365: the
    /// pose lives in the host's [`crate::rig_anim::RigPose`] buffer and the world pass writes
    /// the rows.
    pub(crate) bones: u32,
    pub(crate) ibp: Handle<SkinnedMeshInverseBindposes>,
    /// The [`SkinnedTwin`]-carrying part entities the promote/demote swaps.
    pub(crate) parts: Vec<Entity>,
}

/// On a part spawned on its static form with a skinned twin waiting: the promote swaps
/// `Mesh3d` to `skinned`, the demote back to `stat`. Both forms are app-built
/// and the handles held here keep them resident across the swaps.
#[derive(Component)]
pub(crate) struct SkinnedTwin {
    pub(crate) skinned: Handle<Mesh>,
    pub(crate) stat: Handle<Mesh>,
}

/// The part query both edges rewrite. A part that despawned mid-frame (tile streaming out under
/// the gate) just skips — its host root is on the same placement list and follows it this frame.
pub(super) type TwinParts<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Mesh3d,
        &'static mut MeshTag,
        &'static SkinnedTwin,
    ),
>;

/// The wake edge: allocate the slot, seed its rows from the pose buffer's CURRENT composed
/// affines, and swap the parts. Returns whether the host now has a rig (so the gate can keep
/// retrying a denial while the host stays drawn). Rows are seeded before the meshes swap: the
/// first skinned frame shows the parked pose the static mesh was already showing, and the world
/// pass takes over the same frame (the wake re-arms the player, whose evaluation re-raises
/// `pose_dirty`).
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
        return false; // table full — the warn fired; the gate retries while drawn
    };
    let slot = rig.slot;
    if let (Some(ibp), Some(pose)) = (ibps.get(&lazy.ibp), pose) {
        // Rig-relative like every other seed: the same compose the world pass
        // runs, from the root's propagated world.
        let root_g = worlds.get(root).copied().unwrap_or_default();
        crate::rig_anim::seed_rig_rows(pose, root_g, &rig, ibp, palettes);
    }
    // Queued, liveness-checked at APPLY time: the slot's only free path is the component's
    // `on_replace` hook, so a plain `insert` racing this frame's tile-unload despawn would drop
    // the `RigSkin` un-attached and leak the slot for the session. (`try_insert` has the same
    // hole — a failed insert still never runs the hook.)
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

/// The demote — the promote's exact inverse. Removing the [`RigSkin`] is what frees the slot
/// (its `on_replace` hook), so whoever demotes cannot leak.
fn demote_lazy_rig(commands: &mut Commands, root: Entity, lazy: &LazyRig, parts: &mut TwinParts) {
    for &part in &lazy.parts {
        let Ok((mut mesh, mut tag, twin)) = parts.get_mut(part) else {
            continue;
        };
        mesh.0 = twin.stat.clone();
        tag.0 = crate::mesh_tag::with_rig(tag.0, 0);
    }
    // Liveness-checked like the promote's insert: a host despawned this frame already freed its
    // slot through the hook, and a plain `entity(root)` would panic at apply.
    commands.queue(move |world: &mut bevy::ecs::world::World| {
        if let Ok(mut e) = world.get_entity_mut(root) {
            e.remove::<RigSkin>();
        }
    });
}

/// The pressure reaper: under [`REAP_LOW_WATER`] slot headroom, demote the longest-parked
/// rigged hosts, oldest first, up to [`REAP_PER_FRAME`]. Zero work — one headroom read — when
/// the table has room, which is the ordinary case; the eager lanes (units, quest markers, spell
/// fx, booths) are never touched, only hosts this module promoted.
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
        // The RigSkin removal above is a command — the resource's headroom doesn't move until
        // it applies, so the batch size is the cap here, not the headroom re-read.
    }
}

#[cfg(test)]
mod tests {
    use super::super::{gate_doodad_anim, DoodadAnimHost};
    use super::*;
    use bevy::animation::graph::AnimationNodeIndex;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    /// The gate + the reaper on the schedule shape they ship with (gate stamps `parked_at`, the
    /// reaper reads it), plus every resource the gate's params need. The camera query is empty —
    /// mesh-backed hosts take the `Visibility` branch.
    fn app() -> App {
        let mut app = App::new();
        // TimePlugin disabled, `Time` manual — the reroll harness's rule: the real-clock driver
        // would clobber the whole-window advances the reap-hysteresis assertions need.
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

    /// One host: a hidden-by-default part carrying a [`SkinnedTwin`] of two distinct weak mesh
    /// handles, a one-bone [`LazyRig`] + [`crate::rig_anim::RigPose`] (the collapsed shape,
    /// decision 1365), and the mesh-backed draw-gate shape. Returns `(host, part)` — the part IS
    /// the visibility carrier, like assemble's.
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

    /// The lazy law end to end: born hidden, a host holds NO slot; a wake allocates on its
    /// SECOND consecutive drawn frame (the first is the spawn-default-visibility lie — see the
    /// gate's promote comment), swaps the part to the skinned twin, writes its tag's rig field,
    /// and seeds the palette rows (the pose buffer's identity bind affine × identity bindpose =
    /// a non-zero row — the swapped-in mesh never renders zeroed rows).
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

    /// The retry law: a wake into a FULL table is a delay, not a sentence — the host stays on
    /// the static mesh, and the frame the table has room again (here: one slot freed) the
    /// still-drawn host promotes. This is the exact failure that used to be permanent.
    #[test]
    fn a_denied_wake_retries_until_the_table_has_room() {
        let mut app = app();
        // Fill every slot (2047 one-bone rigs), leaking them into a Vec so they stay live.
        let hoard: Vec<RigSkin> = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            std::iter::from_fn(|| RigSkin::allocate_bones(&mut palettes, 1, Handle::default()))
                .collect()
        };
        assert_eq!(app.world().resource::<RigPalettes>().slot_headroom(), 0);

        let (host, _part) = lazy_host(&mut app, true);
        app.update(); // frame 1: the spawn-race frame — never promotes
        app.update(); // frame 2: a real drawn frame — denied against the full table
        assert_eq!(
            rig_slot(&app, host),
            None,
            "full table ⇒ denied, static mesh"
        );

        // One slot frees; the host is still drawn — the very next frame promotes.
        let freed = hoard[0].slot;
        app.world_mut().resource_mut::<RigPalettes>().free(freed);
        app.update();
        assert!(rig_slot(&app, host).is_some(), "room ⇒ the retry lands");
    }

    /// The reaper law: under low slot headroom, a host parked past the hysteresis window gives
    /// its slot back — part demoted to the static form, tag cleared — and the next wake
    /// re-promotes. With headroom, a parked host keeps its slot (a camera pan is zero churn).
    #[test]
    fn pressure_reaps_the_parked_and_the_next_wake_re_rigs() {
        let mut app = app();
        let (host, part) = lazy_host(&mut app, true);
        app.update();
        app.update(); // second consecutive drawn frame — the promote's confirm
        assert!(rig_slot(&app, host).is_some(), "drawn ⇒ rigged");

        // Park, and age past the reap hysteresis: with plenty of headroom, the slot stays.
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

        // Choke the table below the low-water mark: the parked host is reaped.
        let _hoard: Vec<RigSkin> = {
            let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
            (0..(crate::mesh_tag::MAX_RIG_SLOTS - 1 - REAP_LOW_WATER))
                .filter_map(|_| RigSkin::allocate_bones(&mut palettes, 1, Handle::default()))
                .collect()
        };
        app.update();
        app.update(); // command application frame — the RigSkin removal lands
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

        // Re-wake: the still-pressured table has the reaped slot free again — re-promote (two
        // drawn frames, the confirm).
        *app.world_mut()
            .entity_mut(part)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Inherited;
        app.update();
        app.update();
        assert!(rig_slot(&app, host).is_some(), "the next wake re-rigs");
    }

    /// The emitter-only shape: a host with no [`LazyRig`] (assemble inserts none when nothing
    /// skins) wakes and parks without ever touching the table — under the eager design every
    /// chimney's smoke host burned a slot nobody could read.
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

/// The Bevy contract this lane's `Aabb` depends on, pinned against the real
/// `calculate_bounds` system rather than a reading of it.
///
/// `promote_lazy_rig`/`demote_lazy_rig` write `Mesh3d`. Bevy's `calculate_bounds` runs two
/// queries — an inserting one filtered `Without<Aabb>`, and an **updating** one filtered
/// `Or<(AssetChanged<Mesh3d>, Changed<Mesh3d>)>` that overwrites a bound the app authored. So the
/// authored all-animation bound decision 1259 puts on an animated placement survives exactly as
/// long as nothing swaps that placement's mesh — i.e. until its first draw-gate wake, at which
/// point it was silently recomputed from the skinned twin's bind-pose geometry and the birds
/// resumed blinking. `NoAutoAabb` (which both queries exclude) is the fix; this is the guard that
/// a Bevy upgrade cannot quietly take it back.
#[cfg(test)]
mod bound_survives_the_twin_swap {
    use super::*;
    use bevy::camera::primitives::Aabb;
    use bevy::camera::visibility::NoAutoAabb;

    /// A 1 yd cube — what the skinned twin's bind-pose geometry computes to.
    fn tiny_mesh() -> Mesh {
        Mesh::from(bevy::math::primitives::Cuboid::new(1.0, 1.0, 1.0))
    }

    /// The authored all-animation bound: a bird's 67 yd circuit around that 1 yd body.
    fn authored() -> Aabb {
        Aabb::from_min_max(Vec3::new(-4.2, 8.8, -30.5), Vec3::new(13.6, 16.0, 36.6))
    }

    /// `(app, entity)` with Bevy's own bounds system on the schedule and the authored bound in
    /// place, mid-swap: `Mesh3d` has just been rewritten the way `promote_lazy_rig` rewrites it.
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
        // The swap the lazy rig performs at the placement's first wake.
        app.world_mut().entity_mut(e).get_mut::<Mesh3d>().unwrap().0 = skinned;
        app.update();
        (app, e)
    }

    fn bound(app: &App, e: Entity) -> Aabb {
        *app.world().entity(e).get::<Aabb>().expect("a bound")
    }

    /// The bug: unguarded, the twin swap hands the authored bound back to `compute_aabb`.
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

    /// The fix: guarded, the authored bound is still the authored bound after the swap.
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
