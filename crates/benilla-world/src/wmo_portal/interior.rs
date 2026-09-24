//! The WMO-interior claims published each frame: [`CurrentWmoInterior`], the player's claim by the
//! render seed's ray (faces and portal crossings), and [`CurrentAreaInterior`], the zone-text claim
//! (faces only, the CGLight node's `+0x90` bit 0 set by `0x6a87f0`). A doorway portal under the
//! eye seeds the render flood without making you indoors.

use bevy::math::Affine3A;
use bevy::prelude::*;

/// The `WMOAreaTable` catalog: each WMO group's `AreaTable` id and audio identity; absent when the
/// client data did not load. A room's zone name comes from here before the terrain chunk's.
#[derive(Resource)]
pub(crate) struct WmoAreas(pub(crate) benilla_formats::WmoAreaCatalog);

pub(super) fn load_wmo_areas(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_wmo_area_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("wmo: {} WMO area rows", cat.len());
            commands.insert_resource(WmoAreas(cat));
        }
        Err(e) => warn!("wmo: WMOAreaTable failed to load: {e:#}"),
    }
}

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::{AdtTile, WmoModel};

use super::seed::{area_down_ray, down_ray_claim, down_ray_seeds, up_ray_claim, DownRayClaim};
use super::{WmoPortalInstance, WmoRoom, WorldCamera, EXTERIOR, EXTERIOR_LIT};
use crate::terrain_stream::{terrain_height_under, PropLobeLight, TerrainStreamer};

/// The WMO interior the player is in: the `WMOAreaTable` keys of the group owning the nearest
/// surface under the probe ([`down_ray_seeds`]), or `None` in the open world.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct CurrentWmoInterior(pub Option<WmoInteriorKeys>);

/// The room the player stands in: the claim [`CurrentWmoInterior`] names, as a placement identity.
/// It scopes the liquid query: a building's MLIQ pool holds only a subject in that placement.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlayerWmoRoom(pub Option<WmoRoom>);

/// The WMO room a remote unit stands in, its own liquid claim, re-rayed only when the unit moves
/// or a building streams in or out.
#[derive(Component, Clone, Copy, Default, PartialEq)]
pub struct UnitWmoRoom {
    room: Option<WmoRoom>,
    /// Where the claim was last sampled, and the WMO-residency generation then: the re-test gate.
    at: Vec3,
    generation: u32,
}

impl UnitWmoRoom {
    /// The room, for the liquid query's scope key.
    pub fn room(&self) -> Option<WmoRoom> {
        self.room
    }

    /// A synthetic claim for tests of a room consumer. Not `#[cfg(test)]`: the consumer under test
    /// is `benilla-app`'s, and a dependent crate does not compile this one's test cfg.
    pub fn claimed(room: WmoRoom) -> Self {
        Self {
            room: Some(room),
            at: Vec3::ZERO,
            generation: 0,
        }
    }
}

/// `WMOAreaTable` lookup keys: `(WMOID, NameSetID, WMOGroupID)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WmoInteriorKeys {
    pub wmo_id: u32,
    pub name_set: u32,
    pub group_area_id: u32,
}

/// Height (yd) above the feet of [`CurrentWmoInterior`]'s probe. The position-cast legs cast from
/// the feet (`0x6a8a20`): a lifted origin can take terrain buried above a floor as nearest.
pub const INTERIOR_PROBE_HEIGHT: f32 = 1.7;

/// Float-safety lift (yd) for the position-cast legs: feet rest on the floor plane, and a coplanar
/// `z <= eye.z` test must not lose the floor to rounding.
pub(crate) const POSITION_PROBE_LIFT: f32 = 0.1;

/// The zone-text indoor claim, the CGLight node's `[node+0x90]` bit 0 (`0x6a87f0`): the keys of
/// the group owning the nearest face under the player (faces only, EXTERIOR excluded, terrain
/// raced, 1000 yd), or `None` outdoors.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct CurrentAreaInterior(pub Option<WmoInteriorKeys>);

/// Track [`CurrentAreaInterior`]: per placed building, the faces-only down-ray from the player's
/// position, first claim wins; the camera stands in before login.
pub(super) fn track_area_interior(
    wmos: Res<Assets<WmoModel>>,
    viewer: Res<crate::view::Viewer>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    instances: Query<&WmoPortalInstance>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    mut current: ResMut<CurrentAreaInterior>,
) {
    // Cast from the position, lifted only for float safety, as the client does (`0x6a8a20`).
    let eye_world = if let Some(body) = viewer.at {
        body + Vec3::Y * POSITION_PROBE_LIFT
    } else if let Some(cam_t) = cam.iter().next() {
        cam_t.translation()
    } else {
        return;
    };
    let terrain = terrain_height_under(&streamer, &adt_tiles, eye_world);
    let mut found = None;
    for inst in &instances {
        let Some(model) = wmos.get(&inst.handle) else {
            continue;
        };
        if model.wmo_id == 0 {
            continue;
        }
        let local_from_world = inst.world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye_world));
        if !column_in_collision_bounds(model.collision_bounds, eye_local, false) {
            continue; // no face of this building can own the probe column
        }
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye_world, z));
        // The zone-text law is `0x8` alone: a `0x40`-only street group is indoors for naming
        // (`[node+0x90]` bit 0 is set before the `0x40` test).
        if let Some(gi) = area_down_ray(
            &model.group_collision_tris,
            &model.group_collision_bounds,
            &model.group_collision_grids,
            &model.group_nav,
            eye_local,
            terrain_local,
            EXTERIOR,
        ) {
            found = Some(WmoInteriorKeys {
                wmo_id: model.wmo_id,
                name_set: u32::from(inst.name_set),
                group_area_id: model.group_nav.get(gi).map_or(0, |g| g.area_table_id),
            });
            break;
        }
    }
    if current.0 != found {
        current.0 = found;
    }
}

/// Track [`CurrentWmoInterior`] and [`PlayerWmoRoom`] from the character, where the client's audio
/// listener sits (`SoundListenerAtCharacter` default 1, `0x457890`), not the camera, which stands
/// in before login. Portal-less models count, unlike in the PVS.
pub(super) fn track_current_interior(
    wmos: Res<Assets<WmoModel>>,
    viewer: Res<crate::view::Viewer>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    instances: Query<(Entity, &WmoPortalInstance)>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    mut current: ResMut<CurrentWmoInterior>,
    mut room: ResMut<PlayerWmoRoom>,
) {
    let eye_world = if let Some(body) = viewer.at {
        body + Vec3::Y * INTERIOR_PROBE_HEIGHT
    } else if let Some(cam_t) = cam.iter().next() {
        cam_t.translation()
    } else {
        return;
    };
    // The terrain leg, sampled once for the column (`GetAreaID` races the same two probes,
    // `0x670345`): a tunnel under a hill must not claim the player on the grass above it.
    let terrain = terrain_height_under(&streamer, &adt_tiles, eye_world);
    let mut found = None;
    let mut found_room = None;
    for (entity, inst) in &instances {
        let Some(model) = wmos.get(&inst.handle) else {
            continue;
        };
        if model.wmo_id == 0 {
            continue;
        }
        let local_from_world = inst.world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye_world));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye_world, z));
        if let Some(gi) = down_ray_seeds(model, eye_local, terrain_local).in_group {
            found = Some(WmoInteriorKeys {
                wmo_id: model.wmo_id,
                name_set: u32::from(inst.name_set),
                group_area_id: model.group_nav.get(gi).map_or(0, |g| g.area_table_id),
            });
            // The same claim as a placement identity, the liquid query's scope key.
            found_room = Some(WmoRoom {
                instance: entity,
                group: gi as u16,
            });
            break;
        }
    }
    if current.0 != found {
        current.0 = found;
    }
    if room.0 != found_room {
        room.0 = found_room;
    }
}

/// Squared distance (yd²) a unit must move before its room is re-rayed: a room changes at doorways,
/// so the claim may lag a transition by this much, well inside the wade band.
const UNIT_ROOM_RESAMPLE_DIST_SQ: f32 = 0.25 * 0.25;

/// Maintain [`UnitWmoRoom`] on every net entity with the faces-only down-ray of the zone-text
/// claim, cast from the unit's own position as the client casts its position legs (`0x6a8a20`).
/// Which leg the reference's per-unit liquid depth uses, and whether it scopes to the group rather
/// than the placement, is untraced.
pub(super) fn track_unit_interiors(
    mut commands: Commands,
    wmos: Res<Assets<WmoModel>>,
    instances: Query<(Entity, &WmoPortalInstance)>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    residency: Res<crate::interior::WmoResidency>,
    mut units: Query<(
        Entity,
        &GlobalTransform,
        &crate::world_unit::WorldUnit,
        Option<&mut UnitWmoRoom>,
    )>,
) {
    let generation = residency.generation();
    for (entity, transform, body, claim) in &mut units {
        // World position, not the local one: a mounted unit's `Transform` is seat-relative.
        let pos = transform.translation();
        // A settled unit keeps its room; the generation leg re-claims one a building streamed in
        // under, or it would read the lake overhead forever.
        if let Some(claim) = claim.as_ref() {
            if claim.generation == generation
                && pos.distance_squared(claim.at) < UNIT_ROOM_RESAMPLE_DIST_SQ
            {
                continue;
            }
        }
        // The under-floor fallback reaches the body's bound centre, at least the tolerance: a
        // point the body occupies, so it finds no floor the body does not straddle.
        let reach = body
            .bound
            .map(|b| transform.transform_point(Vec3::from(b.center)).y - pos.y)
            .unwrap_or(0.0)
            .max(ROOM_UNDER_FLOOR_TOLERANCE);
        let room = room_at(&wmos, &instances, &streamer, &adt_tiles, pos, reach);
        let next = UnitWmoRoom {
            room,
            at: pos,
            generation,
        };
        match claim {
            Some(mut claim) => {
                if *claim != next {
                    *claim = next;
                }
            }
            // `try_insert`: a unit despawned this frame by the net teardown must not panic here.
            None => {
                commands.entity(entity).try_insert(next);
            }
        }
    }
}

/// The least reach (yd) of [`room_at`]'s second pass above a body's origin. A planted object's
/// origin can sit under its floor (Orgrimmar's props, 0.45 to 1.16 yd); two yards covers that and
/// reaches no storey above.
pub(crate) const ROOM_UNDER_FLOOR_TOLERANCE: f32 = 2.0;

/// The WMO room a world position stands in, by the faces-only ray; only on a miss, a second pass
/// from `reach` higher finds a body whose origin sits below its floor.
fn room_at(
    wmos: &Assets<WmoModel>,
    instances: &Query<(Entity, &WmoPortalInstance)>,
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    feet_world: Vec3,
    reach: f32,
) -> Option<WmoRoom> {
    room_cast(wmos, instances, streamer, adt_tiles, feet_world, 0.0)
        .or_else(|| room_cast(wmos, instances, streamer, adt_tiles, feet_world, reach))
}

/// One faces-only room cast from `feet_world + rise`.
fn room_cast(
    wmos: &Assets<WmoModel>,
    instances: &Query<(Entity, &WmoPortalInstance)>,
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    feet_world: Vec3,
    rise: f32,
) -> Option<WmoRoom> {
    let probe_world = feet_world + Vec3::Y * (POSITION_PROBE_LIFT + rise);
    let terrain = terrain_height_under(streamer, adt_tiles, probe_world);
    for (entity, inst) in instances {
        let Some(model) = wmos.get(&inst.handle) else {
            continue;
        };
        if model.wmo_id == 0 {
            continue; // as the player's own claim skips it
        }
        let local_from_world = inst.world_from_local.inverse();
        let probe_local = bevy_to_wow(local_from_world.transform_point3(probe_world));
        if !column_in_collision_bounds(model.collision_bounds, probe_local, false) {
            continue; // no face of this building can own the probe column
        }
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe_world, z));
        if let Some(gi) = area_down_ray(
            &model.group_collision_tris,
            &model.group_collision_bounds,
            &model.group_collision_grids,
            &model.group_nav,
            probe_local,
            terrain_local,
            EXTERIOR,
        ) {
            return Some(WmoRoom {
                instance: entity,
                group: gi as u16,
            });
        }
    }
    None
}

/// Whether a world position is indoors for the unit light law: the nearest surface under it is a
/// face of a WMO group with neither `0x8` nor `0x40`, the lighting-class fork on `MOGI & 0x48`
/// (`0x6a87f0`). A WMO without a `WMOAreaTable` identity counts.
pub fn indoors_at<'a>(
    wmos: &Assets<WmoModel>,
    instances: impl IntoIterator<Item = &'a WmoPortalInstance>,
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    feet_world: Vec3,
) -> bool {
    matches!(
        indoor_verdict_at(
            wmos,
            instances.into_iter().map(|inst| ((), inst)),
            streamer,
            adt_tiles,
            feet_world,
            LightAttach::DownRay,
        )
        .0,
        IndoorVerdict::DayNight | IndoorVerdict::Baked { .. }
    )
}

/// Which attach a light node uses to find its WMO group: `[node+0x90]` bit `0x2000`, set at node
/// creation from the typemask (`0x613e10`/`0x670db0`) and read by the dispatch `0x6a86d0`, which
/// has no subclass test: the lane follows the object kind, not the shading law.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LightAttach {
    /// Units, players and ADT/WMO doodads: the down-ray attach `0x6a8a20`, from the node position
    /// (`[node+0xa8]`).
    DownRay,
    /// GameObjects: the containment attach `0x6a8c10` → `0x6a8ed0`, from the world bounding-box
    /// centre (`[node+0x5c]`, `0x6717d0`); on a downward miss it retries 1000 yd up (`0x6a908d`).
    Containment,
}

/// A position's interior light verdict, with the footprint bake when indoors.
pub enum IndoorVerdict {
    /// No WMO face under the position: the exterior law, MCSH-driven 2.5/0.5 intensity.
    Outdoors,
    /// An outdoor-class WMO face (`MOGI & 0x48`) is nearest: the exterior law at the forced-lit
    /// 2.5, not sampling the terrain MCSH beneath (`0x6a8a20` sets skip-shadow `[node+0xd] |= 0x2`
    /// on its WMO branch for every node subclass, `0x6a8bc7`).
    OutdoorsOnWmo,
    /// Indoors, but the footprint ray missed every MOCV face or the hit face's MOPY bit 0x1
    /// selects the exterior colours (`0x6a8410`): the plain matte variant.
    DayNight,
    /// Indoors over a baked floor: the hit's barycentric MOCV bytes and the hit group's MOLR lobes
    /// in world (Bevy) space, colour × intensity.
    Baked {
        mocv: [u8; 3],
        lobes: Vec<PropLobeLight>,
    },
}

/// A position's interior light verdict: the [`down_ray_claim`] classify runs per placement and the
/// nearest claim across placements wins, the client's one global nearest hit. An interior winner
/// runs the footprint ray on that placement's mesh (`0x6717d0` → `0x69e4c0`); the MOLR lobes come
/// from the footprint hit's group, where the reference's link between the two rays is untraced.
///
/// `anchor_world` is the attach's anchor ([`LightAttach`]). Also returns the winning room as
/// `(key, group)`, or `None` outdoors: the per-(instance, group) render record the attach creates
/// (`0x685f85`, `[P+0x98]`), where the interior-fog gate is asked. Pass `()` as the key to skip it.
pub fn indoor_verdict_at<'a, K: Copy>(
    wmos: &Assets<WmoModel>,
    instances: impl IntoIterator<Item = (K, &'a WmoPortalInstance)>,
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    anchor_world: Vec3,
    attach: LightAttach,
) -> (IndoorVerdict, Option<(K, usize)>) {
    let probe_world = anchor_world + Vec3::Y * POSITION_PROBE_LIFT;
    let terrain = terrain_height_under(streamer, adt_tiles, probe_world);
    let mut best: Option<(DownRayClaim, &WmoModel, &WmoPortalInstance, [f32; 3], K)> = None;
    // The containment lane's upward retry (`0x6a9093`, +1000 yd) runs only when the whole downward
    // pass found nothing, so no claim is arbitrated against one cast the other way.
    let instances: Vec<(K, &WmoPortalInstance)> = instances.into_iter().collect();
    for retry_up in [false, true] {
        if retry_up && (best.is_some() || !matches!(attach, LightAttach::Containment)) {
            break;
        }
        for (key, inst) in instances.iter().copied() {
            let Some(model) = wmos.get(&inst.handle) else {
                continue;
            };
            let local_from_world = inst.world_from_local.inverse();
            let probe_local = bevy_to_wow(local_from_world.transform_point3(probe_world));
            if !column_in_collision_bounds(model.collision_bounds, probe_local, retry_up) {
                continue; // no face of this building can own the probe column
            }
            let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe_world, z));
            let claim = if retry_up {
                up_ray_claim(
                    &model.group_collision_tris,
                    &model.group_collision_bounds,
                    &model.group_collision_grids,
                    &model.group_nav,
                    probe_local,
                    EXTERIOR | EXTERIOR_LIT,
                )
            } else {
                down_ray_claim(
                    &model.group_collision_tris,
                    &model.group_collision_bounds,
                    &model.group_collision_grids,
                    &model.group_nav,
                    probe_local,
                    terrain_local,
                    EXTERIOR | EXTERIOR_LIT,
                )
            };
            let Some(claim) = claim else {
                continue;
            };
            if best.as_ref().is_none_or(|(b, ..)| claim.depth < b.depth) {
                best = Some((claim, model, inst, probe_local, key));
            }
        }
    }
    let Some((claim, model, inst, probe_local, key)) = best else {
        return (IndoorVerdict::Outdoors, None);
    };
    if claim.outdoor {
        return (IndoorVerdict::OutdoorsOnWmo, None);
    }
    // The room is the attach ray's group (`0x6a8a20`), not the footprint sample's: the render
    // record hangs off the attach, and the footprint ray only picks the bake colours.
    let room = Some((key, claim.group));
    // The footprint sample, on the winning placement only (an unbounded upward retry would let a
    // deck overhead adopt a unit in the open); the down-ray lane never retries upward (`0x6a8ab0`).
    let sample = footprint_sample(model, probe_local).or_else(|| {
        matches!(attach, LightAttach::Containment)
            .then(|| footprint_sample_above(model, probe_local))
            .flatten()
    });
    let Some((gi, mocv, mopy_daynight)) = sample else {
        return (IndoorVerdict::DayNight, room);
    };
    if mopy_daynight {
        return (IndoorVerdict::DayNight, room);
    }
    let lobes = model
        .group_light_refs
        .get(gi)
        .into_iter()
        .flatten()
        .filter_map(|&li| model.lights.get(li as usize))
        .filter(|l| l.is_omni())
        .map(|l| PropLobeLight {
            pos: inst
                .world_from_local
                .transform_point3(wow_to_bevy(l.position)),
            color_i: l.color.map(|c| c * l.intensity.max(0.0)),
            atten_start: l.attenuation_start,
            atten_end: l.attenuation_end,
        })
        .collect();
    (IndoorVerdict::Baked { mocv, lobes }, room)
}

/// The footprint down-ray: the nearest render face below the probe in the interior groups (faces
/// pre-filtered by the client's BSP mask 0x88), with its group, its barycentric MOCV in bytes
/// (`0x6a77e0` takes a byte colour) and whether MOPY bit 0x1 selects day/night. An exact tie keeps
/// the later face (`0x6bc780`), in face order rather than BSP order; no floor was seen with one.
pub(super) fn footprint_sample(
    model: &WmoModel,
    probe_local: [f32; 3],
) -> Option<(usize, [u8; 3], bool)> {
    footprint_scan(model, probe_local, false, None).map(|(g, c, m, _)| (g, c, m))
}

/// The footstep material ray, the second of the client's two: re-cast over `group`'s render faces,
/// the winning face's `MOPY` material → `MOMT` ground type (`0x6a26c0`). `None` is the client's
/// `-1`, silent: never a reason to fall back to terrain, since the building won the column.
pub(crate) fn surface_terrain_sample(
    model: &WmoModel,
    group: usize,
    probe_local: [f32; 3],
) -> Option<u32> {
    let (_, _, _, material) = footprint_scan(model, probe_local, false, Some(group))?;
    // The client indexes MOMT unchecked, safe because every `0xFF` face was dropped at parse.
    model
        .material_ground_type
        .get(usize::from(material))
        .copied()
}

/// The containment attach's upward retry (`0x6a908d`, re-cast to `anchor.z + 1000`): the nearest
/// render face above the probe.
pub(super) fn footprint_sample_above(
    model: &WmoModel,
    probe_local: [f32; 3],
) -> Option<(usize, [u8; 3], bool)> {
    footprint_scan(model, probe_local, true, None).map(|(g, c, m, _)| (g, c, m))
}

/// The footprint and material rays: `up` flips which side of the probe a face must lie on (the
/// nearest in the cast's direction wins); `only_group` holds the material ray to the group the
/// arbitration chose, since the nearest render face overall can belong to another.
fn footprint_scan(
    model: &WmoModel,
    probe_local: [f32; 3],
    up: bool,
    only_group: Option<usize>,
) -> Option<(usize, [u8; 3], bool, u8)> {
    let (px, py, pz) = (probe_local[0], probe_local[1], probe_local[2]);
    let mut best: Option<(f32, usize, [f32; 3], bool, u8)> = None;
    for (gi, fp) in model.group_footprints.iter().enumerate() {
        let Some(fp) = fp else { continue };
        if only_group.is_some_and(|g| g != gi) {
            continue;
        }
        // Broad phase on the face-derived bounds, never an authored box, so the cull is exact: skip
        // a group whose XY bounds exclude the column or whose faces all lie past the probe.
        if let Some(Some((min, max))) = model.group_footprint_bounds.get(gi) {
            let past_probe = if up { max[2] < pz } else { min[2] > pz };
            if px < min[0] || px > max[0] || py < min[1] || py > max[1] || past_probe {
                continue;
            }
        }
        // Narrow phase: the column index, else every face. The index yields faces in ascending
        // order, so the later-face-wins tie resolves as the linear scan does.
        let mut consider = |ti: usize| {
            let Some(tri) = fp.indices.get(ti * 3..ti * 3 + 3) else {
                return;
            };
            let (Some(&a), Some(&b), Some(&c)) = (
                fp.positions.get(tri[0] as usize),
                fp.positions.get(tri[1] as usize),
                fp.positions.get(tri[2] as usize),
            ) else {
                return;
            };
            // 2D barycentric in the down-ray's projection (x/y), inclusive edges.
            let det = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if det.abs() < 1e-9 {
                return;
            }
            let wb = ((px - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (py - a[1])) / det;
            let wc = ((b[0] - a[0]) * (py - a[1]) - (px - a[0]) * (b[1] - a[1])) / det;
            let wa = 1.0 - wb - wc;
            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                return;
            }
            let z = wa * a[2] + wb * b[2] + wc * c[2];
            let (past_probe, farther) = match up {
                false => (z > pz, best.is_some_and(|(bz, ..)| z < bz)),
                true => (z < pz, best.is_some_and(|(bz, ..)| z > bz)),
            };
            if past_probe || farther {
                return;
            }
            let (Some(ca), Some(cb), Some(cc)) = (
                fp.mocv.get(tri[0] as usize),
                fp.mocv.get(tri[1] as usize),
                fp.mocv.get(tri[2] as usize),
            ) else {
                return;
            };
            let mocv = [
                wa * f32::from(ca[0]) + wb * f32::from(cb[0]) + wc * f32::from(cc[0]),
                wa * f32::from(ca[1]) + wb * f32::from(cb[1]) + wc * f32::from(cc[1]),
                wa * f32::from(ca[2]) + wb * f32::from(cb[2]) + wc * f32::from(cc[2]),
            ];
            let mopy = fp.mopy_flags.get(ti).is_some_and(|f| f & 0x1 != 0);
            let material = fp.mopy_material.get(ti).copied().unwrap_or(0xFF);
            best = Some((z, gi, mocv, mopy, material));
        };
        match model.group_footprint_grids.get(gi).and_then(Option::as_ref) {
            Some(grid) => grid.candidates(px, py).for_each(&mut consider),
            None => (0..fp.indices.len() / 3).for_each(&mut consider),
        }
    }
    best.map(|(_, gi, mocv, mopy, material)| {
        (
            gi,
            mocv.map(|v| v.round().clamp(0.0, 255.0) as u8),
            mopy,
            material,
        )
    })
}

/// Whether a probe column (model-local) can hit any of a building's collision faces: inside the
/// face AABB in XY with faces on the cast's side. Exact, and one test per building in open country.
fn column_in_collision_bounds(
    bounds: Option<([f32; 3], [f32; 3])>,
    probe: [f32; 3],
    up: bool,
) -> bool {
    bounds.is_some_and(|(min, max)| {
        probe[0] >= min[0]
            && probe[0] <= max[0]
            && probe[1] >= min[1]
            && probe[1] <= max[1]
            // The cast's own half-space: a downward ray hits nothing in a building wholly above the
            // probe, an upward one nothing in a building wholly below it.
            && if up { max[2] >= probe[2] } else { min[2] <= probe[2] }
    })
}

/// The terrain surface under `eye_world` (raw WoW `z`) as a placement's model-space `z`, the frame
/// the down-ray races in; exact for a yaw-only placement, where a vertical column stays vertical.
pub fn terrain_z_local(local_from_world: &Affine3A, eye_world: Vec3, terrain_wow_z: f32) -> f32 {
    let eye_wow = bevy_to_wow(eye_world);
    let surface = wow_to_bevy([eye_wow[0], eye_wow[1], terrain_wow_z]);
    bevy_to_wow(local_from_world.transform_point3(surface))[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::footprint_tri_bounds;
    use benilla_formats::FootprintTris;

    /// A model with nothing but footprint groups, all the sample reads.
    fn bare_model() -> WmoModel {
        WmoModel {
            wmo_id: 0,
            submeshes: Vec::new(),
            submesh_group: Vec::new(),
            portal_vertices: Vec::new(),
            portal_infos: Vec::new(),
            portal_refs: Vec::new(),
            group_nav: Vec::new(),
            fogs: Vec::new(),
            skybox: None,
            group_collision_tris: Vec::new(),
            group_camera_only_tris: Vec::new(),
            group_collision_bounds: Vec::new(),
            group_collision_grids: Vec::new(),
            collision_bounds: None,
            collision: None,
            collision_camera: None,
            doodads: Vec::new(),
            doodad_sets: Vec::new(),
            lights: Vec::new(),
            group_bounds: Vec::new(),
            group_footprints: Vec::new(),
            material_ground_type: Vec::new(),
            material_diff_color: Vec::new(),
            group_footprint_bounds: Vec::new(),
            group_footprint_grids: Vec::new(),
            group_light_refs: Vec::new(),
            group_liquids: Vec::new(),
            doodad_base: Default::default(),
            doodad_owner: Default::default(),
            doodad_groups: Default::default(),
        }
    }

    /// Two single-face footprint groups: group 0 owns the floor under the probe (z = 0), group 1
    /// sits far off in XY with a higher face (z = 5) that would shadow it through a broken cull.
    fn two_group_footprints() -> Vec<Option<FootprintTris>> {
        let face = |offset: [f32; 3], material: u8| FootprintTris {
            positions: vec![
                [offset[0], offset[1], offset[2]],
                [offset[0] + 10.0, offset[1], offset[2]],
                [offset[0], offset[1] + 10.0, offset[2]],
            ],
            indices: vec![0, 1, 2],
            mocv: vec![[10, 20, 30]; 3],
            mopy_flags: vec![0],
            mopy_material: vec![material],
        };
        vec![
            Some(face([-5.0, -5.0, 0.0], 1)),
            Some(face([100.0, 100.0, 5.0], 2)),
        ]
    }

    /// A dungeon-scale footprint group: an `n × n` grid of floor quads at a sawtooth height, so a
    /// column's answer depends on which face wins; 64 faces or more, so `ColumnGrid` indexes it.
    fn slab_field(n: usize) -> FootprintTris {
        let (mut positions, mut indices, mut mocv, mut mopy_flags) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut mopy_material = Vec::new();
        for iy in 0..n {
            for ix in 0..n {
                let (x, y) = (ix as f32 * 4.0, iy as f32 * 4.0);
                // Sawtooth z: a mis-picked face changes the verdict.
                let z = ((ix + iy) % 3) as f32 * 0.5;
                let base = positions.len() as u16;
                positions.extend_from_slice(&[
                    [x, y, z],
                    [x + 4.0, y, z],
                    [x + 4.0, y + 4.0, z],
                    [x, y + 4.0, z],
                ]);
                let shade = ((ix * 7 + iy * 13) % 200) as u8;
                mocv.extend_from_slice(&[[shade, shade / 2, shade / 3]; 4]);
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                mopy_flags.extend_from_slice(&[0, 0]);
                mopy_material.extend_from_slice(&[0, 0]);
            }
        }
        FootprintTris {
            positions,
            indices,
            mocv,
            mopy_flags,
            mopy_material,
        }
    }

    /// The material ray reads the hit face's material inside the group the arbitration chose; a
    /// group with no face under the column is silent, the client's `-1`.
    #[test]
    fn material_ray_resolves_per_group_and_bounds_checks() {
        let mut model = bare_model();
        model.group_footprints = two_group_footprints();
        model.group_footprint_bounds = footprint_tri_bounds(&model.group_footprints);
        // A root MOMT: material 0 unauthored ("None"), 1 = Wood (TerrainType 4), 2 = Snow (3).
        model.material_ground_type = vec![10, 4, 3];
        assert_eq!(surface_terrain_sample(&model, 0, [0.0, 0.0, 2.0]), Some(4));
        // Group 1's only face is 100 yd away: silent, never group 0's face.
        assert_eq!(surface_terrain_sample(&model, 1, [0.0, 0.0, 2.0]), None);
        assert_eq!(
            surface_terrain_sample(&model, 1, [103.0, 103.0, 7.0]),
            Some(3)
        );
        // A material id past the root's MOMT is silent.
        model.material_ground_type = vec![10];
        assert_eq!(surface_terrain_sample(&model, 0, [0.0, 0.0, 2.0]), None);
    }

    /// The column index never changes a verdict, the sawtooth's exact-z ties included.
    #[test]
    fn footprint_column_index_is_exact() {
        let mut model = bare_model();
        model.group_footprints = vec![Some(slab_field(12))];
        model.group_footprint_bounds = footprint_tri_bounds(&model.group_footprints);
        let grids = benilla_assets::footprint_tri_grids(&model.group_footprints);
        assert!(grids[0].is_some(), "the field must actually be indexed");
        let mut probed_hits = 0;
        for gx in -2..=50 {
            for gy in -2..=50 {
                let probe = [gx as f32 * 1.1, gy as f32 * 0.9, 3.0];
                model.group_footprint_grids = Vec::new();
                let linear = footprint_sample(&model, probe);
                model.group_footprint_grids = grids.clone();
                let indexed = footprint_sample(&model, probe);
                assert_eq!(
                    linear, indexed,
                    "column {probe:?}: the index changed the footprint verdict"
                );
                probed_hits += usize::from(linear.is_some());
            }
        }
        assert!(
            probed_hits > 500,
            "the sweep must land on the field, got {probed_hits} hits"
        );
    }

    /// The broad phase is a pure skip: the verdict with face bounds equals the verdict with none.
    #[test]
    fn footprint_bounds_cull_is_exact() {
        let mut model = bare_model();
        model.group_footprints = two_group_footprints();
        // Over group 0's face; the unculled baseline first.
        let probe = [-1.0, -1.0, 2.0];
        let unculled = footprint_sample(&model, probe);
        assert_eq!(unculled, Some((0, [10, 20, 30], false)));
        model.group_footprint_bounds = footprint_tri_bounds(&model.group_footprints);
        assert_eq!(footprint_sample(&model, probe), unculled);
        // Between the groups: both agree on a miss.
        model.group_footprint_bounds = Vec::new();
        assert_eq!(footprint_sample(&model, [50.0, 50.0, 2.0]), None);
        model.group_footprint_bounds = footprint_tri_bounds(&model.group_footprints);
        assert_eq!(footprint_sample(&model, [50.0, 50.0, 2.0]), None);
        // Below every face: the min-z leg and the scan agree on a miss.
        model.group_footprint_bounds = Vec::new();
        assert_eq!(footprint_sample(&model, [-1.0, -1.0, -1.0]), None);
        model.group_footprint_bounds = footprint_tri_bounds(&model.group_footprints);
        assert_eq!(footprint_sample(&model, [-1.0, -1.0, -1.0]), None);
    }

    /// The containment attach's upward retry (`0x6a908d`): after a downward miss the nearest face
    /// above answers, with the group, MOCV and MOPY it gives from below.
    #[test]
    fn the_upward_retry_finds_the_floor_a_sunk_object_sits_under() {
        let mut model = bare_model();
        model.group_footprints = two_group_footprints();
        model.group_footprint_bounds = footprint_tri_bounds(&model.group_footprints);

        // 2 cm under group 0's face at z = 0, as a portcullis spawned below its own slab sits.
        let sunk = [-1.0, -1.0, -0.02];
        assert_eq!(
            footprint_sample(&model, sunk),
            None,
            "the downward leg has nothing below it — this is the whole defect"
        );
        assert_eq!(
            footprint_sample_above(&model, sunk),
            Some((0, [10, 20, 30], false)),
            "the retry finds the slab it is sunk into, and reads the same bake"
        );
        // From above the legs swap roles: the retry must not become a second downward scan.
        let over = [-1.0, -1.0, 2.0];
        assert_eq!(
            footprint_sample(&model, over),
            Some((0, [10, 20, 30], false))
        );
        assert_eq!(
            footprint_sample_above(&model, over),
            None,
            "nothing above the probe, so the retry declines rather than re-finding the floor"
        );
        // The upward leg honours the broad phase: group 1's face (z = 5) is off this column in XY.
        assert_eq!(footprint_sample_above(&model, [-1.0, -1.0, 1.0]), None);
    }

    /// The derived bounds cover only vertices a face references: an orphan must not inflate them.
    #[test]
    fn footprint_bounds_ignore_unreferenced_vertices() {
        let mut fps = two_group_footprints();
        if let Some(fp) = fps[0].as_mut() {
            fp.positions.push([1000.0, 1000.0, 1000.0]); // orphan
        }
        let bounds = footprint_tri_bounds(&fps);
        let (min, max) = bounds[0].expect("group 0 has faces");
        assert!(max[0] <= 5.0 + 1e-6 && max[1] <= 5.0 + 1e-6);
        assert_eq!(min[2], 0.0);
    }
}
