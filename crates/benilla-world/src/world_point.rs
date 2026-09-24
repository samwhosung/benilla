//! [`WorldPoint`]: what is at a point and where a subject is, asked of the world rather than of one
//! subsystem: liquid and its surface height, whose room it is, what the eye is submerged in, the
//! nearest audible water. The subject is an [`Entity`] or a role, never an engine component.
//! `terrain_height_under` reads the MCNK heightfield, not the physics trimesh that
//! `WorldCollision` traces, and the two may differ.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::liquid::{
    camera_claim, describe_at, liquid_at, player_claim, unit_claim, water_surface_at, LiquidClaim,
    LiquidHit, LiquidSoundSource, RoomPlacements, Underwater, WaterChunkInfo,
};
use crate::surface::SurfaceUnderfoot;
use crate::terrain_stream::CurrentArea;
use crate::wmo_portal::{
    CameraInteriorClaim, CurrentAreaInterior, CurrentWmoInterior, PlayerWmoRoom, UnitWmoRoom,
};

/// Whose question it is: inside a building only that placement's MLIQ answers, outdoors only the
/// ADT's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Subject {
    /// The player's own body, from the interior down-ray `wmo_portal` runs each frame.
    Player,
    /// The camera eye: the reference's `[0xc7b748]` branch of the environment probe `0x6809c0`.
    /// It differs from the body when the camera is outside the feet's room, and decides the
    /// submerged view.
    Eye,
    /// A streamed unit; its room is looked up here.
    Unit(Entity),
}

/// The world, asked about a point; read-only, so it never conflicts with a caller's parameters.
#[derive(SystemParam)]
pub struct WorldPoint<'w, 's> {
    liquids: Query<'w, 's, &'static WaterChunkInfo>,
    /// The XY grid over loaded surfaces: a point query walks only its cell's surfaces.
    index: Res<'w, crate::liquid::WaterIndex>,
    /// Liquid surfaces that also carry a sound class: the ambient-loop scan's population.
    sound_sources: Query<'w, 's, (&'static LiquidSoundSource, &'static WaterChunkInfo)>,
    /// The placed buildings a room's whole-group submersion override resolves against.
    placements: RoomPlacements<'w, 's>,
    player_room: Res<'w, PlayerWmoRoom>,
    eye_room: Res<'w, CameraInteriorClaim>,
    unit_rooms: Query<'w, 's, &'static UnitWmoRoom>,
    underwater: Res<'w, Underwater>,
    surface: SurfaceUnderfoot<'w, 's>,
    /// Where the player is, as three answers that disagree on purpose: the finest MCNK area (the
    /// client's own `GetAreaID`), the render-ray interior claim and the zone-text one.
    area: Res<'w, CurrentArea>,
    interior: Res<'w, CurrentWmoInterior>,
    area_interior: Res<'w, CurrentAreaInterior>,
    wmo_areas: Option<Res<'w, crate::wmo_portal::WmoAreas>>,
}

/// Seeds a headless `World` (a `RunSystemOnce` harness runs no startup plugins) with every resource
/// [`WorldPoint`] needs, as the empty world; a resource added to [`WorldPoint`] is added here too.
pub fn init_world_point_resources(world: &mut bevy::prelude::World) {
    world.init_resource::<crate::liquid::WaterIndex>();
    world.init_resource::<Underwater>();
    world.init_resource::<crate::liquid::SubmergedEye>();
    world.init_resource::<PlayerWmoRoom>();
    world.init_resource::<CameraInteriorClaim>();
    world.init_resource::<bevy::prelude::Assets<benilla_assets::WmoModel>>();
    world.init_resource::<bevy::prelude::Assets<benilla_assets::AdtTile>>();
    world.init_resource::<crate::terrain_stream::TerrainStreamer>();
    world.init_resource::<CurrentArea>();
    world.init_resource::<CurrentWmoInterior>();
    world.init_resource::<CurrentAreaInterior>();
}

/// The nearest wet point of one liquid sound class ([`WorldPoint::nearest_liquid_per_class`]).
pub struct NearestLiquid {
    /// Squared distance from the query point, in WoW yards.
    pub(crate) dist_sq: f32,
    /// The wet point, WoW space: the slew target.
    pub point: [f32; 3],
    /// The surface's sound-class nibble (`class = n & 3`, `FluidSpeed = n & 0xc`).
    pub nibble: u8,
}

impl WorldPoint<'_, '_> {
    /// Whose liquid answers for `who`.
    pub fn claim(&self, who: Subject) -> LiquidClaim {
        match who {
            Subject::Player => player_claim(&self.player_room, &self.placements),
            Subject::Eye => camera_claim(&self.eye_room, &self.placements),
            Subject::Unit(e) => unit_claim(self.unit_rooms.get(e).ok(), &self.placements),
        }
    }

    /// The loaded surfaces whose box covers `wow`'s XY.
    fn over(&self, wow: [f32; 3]) -> impl Iterator<Item = &WaterChunkInfo> {
        self.index
            .over(wow[0], wow[1])
            .iter()
            .filter_map(|&e| self.liquids.get(e).ok())
    }

    /// Whether the room tracker has reached this subject (false only on a unit's first frame). An
    /// unsettled claim admits both liquid sources, so a state a subject can only enter (swimming)
    /// must wait for it; one it can leave must not.
    pub fn room_settled(&self, who: Subject) -> bool {
        self.claim(who) != LiquidClaim::Unknown
    }

    /// The liquid at `wow` for this subject, nearest surface first, lava and slime included.
    pub fn liquid_at(&self, who: Subject, wow: [f32; 3]) -> Option<LiquidHit> {
        liquid_at(self.over(wow), wow, self.claim(who))
    }

    /// The water surface height at `wow`: [`Self::liquid_at`] without magma and slime.
    pub fn water_surface_at(&self, who: Subject, wow: [f32; 3]) -> Option<f32> {
        water_surface_at(self.over(wow), wow, self.claim(who))
    }

    /// Every loaded liquid footprint over this XY, winner or not, for the `/liquid` command.
    pub fn describe_liquid_at(&self, who: Subject, wow: [f32; 3]) -> Vec<String> {
        describe_at(self.over(wow), wow, self.claim(who))
    }

    /// Which liquid the camera eye is in: water reads the zone's underwater slot, magma and slime
    /// fixed global rows (`0x6d2371`).
    pub fn submersion(&self) -> benilla_formats::Submersion {
        self.underwater.0
    }

    /// The `TerrainType` id under a subject's feet, read by the `$FSD` sound, the footprint decal
    /// and the spray alike; the reference caches one per unit (`CGUnit+0xc60`). `pos` is Bevy
    /// space. `None` is the client's `-1`: silent, no print, no fallback to the other leg.
    pub fn terrain_type(
        &self,
        catalog: &benilla_formats::FootstepCatalog,
        who: Subject,
        pos: Vec3,
    ) -> Option<u32> {
        self.surface.terrain_type(catalog, self.unit_room(who), pos)
    }

    /// Which WMO group a subject stands in; `None` in the open world.
    pub fn room_group(&self, who: Subject) -> Option<u16> {
        self.unit_room(who)
            .and_then(UnitWmoRoom::room)
            .map(|r| r.group)
    }

    /// Only a `Unit` subject has a room component; the player's and the eye's are resources.
    fn unit_room(&self, who: Subject) -> Option<&UnitWmoRoom> {
        match who {
            Subject::Unit(e) => self.unit_rooms.get(e).ok(),
            _ => None,
        }
    }

    /// The finest area the player stands in, the client's `GetAreaID` with the WMO-interior claim;
    /// `None` until the ground tile is resident, or off-terrain.
    pub fn area(&self) -> Option<u32> {
        self.area.0
    }

    /// The MCNK `areaId` under a point (Bevy space), outdoors only; see [`Self::area`].
    pub fn area_id_under(&self, pos: Vec3) -> Option<u32> {
        self.surface.area_id_under(pos)
    }

    /// The terrain height under a point (Bevy space), from the MCNK heightfield.
    pub fn terrain_height_under(&self, pos: Vec3) -> Option<f32> {
        self.surface.terrain_height_under(pos)
    }

    /// Whether this point is inside a building: the faces-only down-ray, raced against the terrain.
    pub fn indoors_at(&self, feet_world: Vec3) -> bool {
        self.surface.indoors_at(feet_world)
    }

    /// The player's render interior claim: the `WMOAreaTable` join keys of the group the down-ray
    /// under the character finds (under the camera when there is no body), or `None` in the open
    /// world. The interior audio reads it.
    pub fn interior(&self) -> Option<crate::wmo_portal::WmoInteriorKeys> {
        self.interior.0
    }

    /// The `WMOAreaTable` row of the player's room (its `AreaTable` id and audio keys), by
    /// exact group row, then the whole-WMO default, then name set 0; `None` in the open world or
    /// without client data.
    pub fn interior_row(&self) -> Option<benilla_formats::WmoArea> {
        let keys = self.interior()?;
        let areas = self.wmo_areas.as_ref()?;
        areas
            .0
            .resolve(keys.wmo_id, keys.name_set, keys.group_area_id)
    }

    /// The two exact-key `WMOAreaTable` rows zone-text naming reads (`0x67e670`): the hit group's,
    /// which sets the subzone, and the WMO's default (`-1`) row, which may override the zone, with
    /// the claim's keys for dedup. No name-set fallback, unlike [`Self::interior_row`]: the
    /// reference names a room only by exact key.
    pub fn area_interior_rows(
        &self,
    ) -> Option<(
        crate::wmo_portal::WmoInteriorKeys,
        Option<benilla_formats::WmoArea>,
        Option<benilla_formats::WmoArea>,
    )> {
        let keys = self.area_interior()?;
        let areas = self.wmo_areas.as_ref()?;
        Some((
            keys,
            areas
                .0
                .group_row(keys.wmo_id, keys.name_set, keys.group_area_id)
                .cloned(),
            areas.0.default_row(keys.wmo_id, keys.name_set).cloned(),
        ))
    }

    /// The player's zone-text interior claim: indoors comes from a down-ray under the player alone
    /// (`0x6a87f0`), so a doorway portal under the eye does not count.
    pub fn area_interior(&self) -> Option<crate::wmo_portal::WmoInteriorKeys> {
        self.area_interior.0
    }

    /// The nearest wet point to `wow` within `radius` for each liquid sound class (`nibble & 3`),
    /// the scan behind the ambient liquid loops (reference: `0x6723f0` → `0x68af10` →
    /// `0x68b0d0`). The class split and the AABB-clamped nearest point are the world's; priority,
    /// the voice cap and the slew are the sound system's.
    pub fn nearest_liquid_per_class(
        &self,
        wow: [f32; 3],
        radius: f32,
    ) -> [Option<NearestLiquid>; 4] {
        let mut best: [Option<NearestLiquid>; 4] = [None, None, None, None];
        for (src, info) in &self.sound_sources {
            let point = info.nearest_point_wow(wow[0], wow[1]);
            let dist_sq = (point[0] - wow[0]).powi(2)
                + (point[1] - wow[1]).powi(2)
                + (point[2] - wow[2]).powi(2);
            let class = (src.nibble & 3) as usize;
            if dist_sq <= radius * radius
                && best[class].as_ref().is_none_or(|b| dist_sq < b.dist_sq)
            {
                best[class] = Some(NearestLiquid {
                    dist_sq,
                    point,
                    nibble: src.nibble,
                });
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wmo_portal::WmoRoom;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn a_unit_subject_resolves_to_that_units_own_room() {
        let mut world = World::new();
        super::init_world_point_resources(&mut world);

        let placement = world.spawn_empty().id();
        let indoors = world
            .spawn(UnitWmoRoom::claimed(WmoRoom {
                instance: placement,
                group: 7,
            }))
            .id();
        let outdoors = world.spawn(UnitWmoRoom::default()).id();
        let untracked = world.spawn_empty().id();

        world
            .run_system_once(move |point: WorldPoint| {
                assert_eq!(point.room_group(Subject::Unit(indoors)), Some(7));
                assert_eq!(point.room_group(Subject::Unit(outdoors)), None);
                assert_eq!(point.room_group(Subject::Unit(untracked)), None);
                // The player's and the eye's rooms are resources, never a unit's component.
                assert_eq!(point.room_group(Subject::Player), None);
                assert_eq!(point.room_group(Subject::Eye), None);
            })
            .unwrap();
    }
}
