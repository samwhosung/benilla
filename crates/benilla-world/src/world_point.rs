//! **[`WorldPoint`] — what is at this point, and where is this subject.**
//!
//! One read-only [`SystemParam`] answering the questions a caller asks *of the world* rather than
//! of one of its subsystems: is there liquid here, how high is its surface, whose room does it
//! belong to, what is the eye submerged in, where is the nearest water you can hear.
//!
//! Decision 1164 counted 23 engine items whose only reason for being public was that a gameplay
//! system had to assemble this query by hand — four `Query`s, three `Res`es and six free functions
//! threaded through each other in the right order, re-written at every call site. Assembling a
//! query by hand is also how a call site gets it *wrong*: the "swim in air" family (0634 → 0696 →
//! 0701) is three separate bugs whose common shape was a caller passing the wrong context, and
//! every one of them had to be fixed at N call sites instead of one.
//!
//! **The subject is the parameter, and it is an [`Entity`] or a role — never an engine component.**
//! Before this, a caller asking about a unit had to carry `Option<&UnitWmoRoom>` in its own query
//! and hand it in; the room tracker's component was gameplay API purely because of that. Now the
//! caller says *which unit*, and the world looks up its own bookkeeping. That is the difference
//! between an API and a wiring diagram.
//!
//! **Deliberately not merged with tracing a body through the world** (1164 item 11, `WorldCollision`):
//! `ground_normal_under` reads the physics trimesh and `terrain_height_under` reads the MCNK
//! heightfield, and they answer different questions about "the ground" that are allowed to differ.
//!
//! Two precedents this generalises, one on each side of the line: `surface::SurfaceUnderfoot` and
//! `sun::follow::FlareGate`.

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

/// Whose question it is — the liquid query's delegation + scope key (decisions 0634/0696/0701).
///
/// It is not decoration and it is not optional: inside a building only *that placement's* MLIQ
/// answers, outdoors only the ADT's. A caller that cannot say who is asking is a caller that will
/// read a lake through a ceiling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Subject {
    /// The player's own body, from the interior down-ray `wmo_portal` runs each frame.
    Player,
    /// The camera eye — the reference's `[0xc7b748]` branch in the environment probe `0x6809c0`.
    /// A separate subject from the body on purpose: the two disagree whenever the camera is out of
    /// the room the feet are in, and it is the *eye* that decides the submerged view.
    Eye,
    /// A streamed unit, by entity. Its room is looked up here; the caller does not carry it.
    Unit(Entity),
}

/// The world, asked about a point. Every field is read-only and every writer of every one of them
/// is engine-internal, so this can never conflict with a caller's own parameters.
#[derive(SystemParam)]
pub struct WorldPoint<'w, 's> {
    liquids: Query<'w, 's, &'static WaterChunkInfo>,
    /// The XY grid over the loaded surfaces ([`crate::liquid::WaterIndex`]): a point query walks
    /// the handful of surfaces over that cell, not every loaded one. The verdict is identical to
    /// the full walk by construction (a surface is registered in every cell its box overlaps —
    /// spatial.rs), and it is what keeps the per-unit askers — the swim mark, the splash line,
    /// the ground clamp, one each per moving creature per frame — off a 650-surface walk apiece
    /// (decision 1979: ~0.35 ms of a raid's main thread was those three walks).
    index: Res<'w, crate::liquid::WaterIndex>,
    /// Liquid surfaces that also carry a sound class — the ambient-loop scan's population.
    sound_sources: Query<'w, 's, (&'static LiquidSoundSource, &'static WaterChunkInfo)>,
    /// The placed buildings a room's whole-group submersion override resolves against (1000).
    placements: RoomPlacements<'w, 's>,
    player_room: Res<'w, PlayerWmoRoom>,
    eye_room: Res<'w, CameraInteriorClaim>,
    unit_rooms: Query<'w, 's, &'static UnitWmoRoom>,
    underwater: Res<'w, Underwater>,
    /// What you are standing on — the terrain/WMO surface race (decision 1161). Folded in rather
    /// than left beside: it is the same question about the same point, and 1164 named it this
    /// facade's seed.
    surface: SurfaceUnderfoot<'w, 's>,
    /// Where the player is, as the three answers that disagree on purpose: the finest MCNK area
    /// (the client's own `GetAreaID`), the *render* interior claim, and the *zone-text* one.
    area: Res<'w, CurrentArea>,
    interior: Res<'w, CurrentWmoInterior>,
    area_interior: Res<'w, CurrentAreaInterior>,
    wmo_areas: Option<Res<'w, crate::wmo_portal::WmoAreas>>,
}

/// **Seed a headless `World` with everything [`WorldPoint`] needs to be constructible.**
///
/// [`WorldPoint`] is a facade over ten resources that the world plugins insert during startup, and a
/// `RunSystemOnce` harness runs none of them — so a test that touches it fails not with an assertion
/// but with `SystemParamValidationError { message: "Resource does not exist" }`, naming whichever
/// field happens to be first. That error is the same whatever the test was about, which makes it a
/// tax on every harness that ever reaches the world facade rather than a signal about any of them.
///
/// One call here is the whole of it, and it stays correct by construction: a resource added to
/// [`WorldPoint`] is added here in the same edit, instead of being discovered by three unrelated
/// test modules failing at once. The defaults are the empty world — no rooms claimed, no liquid, no
/// area — which is what a geometry harness wants: it supplies the geometry it is about and nothing
/// else answers.
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

/// The nearest wet point of one liquid sound class — see [`WorldPoint::nearest_liquid_per_class`].
pub struct NearestLiquid {
    /// Squared distance from the query point, in WoW yards.
    pub(crate) dist_sq: f32,
    /// The wet point itself, WoW space — the slew target.
    pub point: [f32; 3],
    /// The surface's sound-class nibble (`class = n & 3`, `FluidSpeed = n & 0xc`).
    pub nibble: u8,
}

impl WorldPoint<'_, '_> {
    /// Whose liquid answers for `who` — the query's context.
    pub fn claim(&self, who: Subject) -> LiquidClaim {
        match who {
            Subject::Player => player_claim(&self.player_room, &self.placements),
            Subject::Eye => camera_claim(&self.eye_room, &self.placements),
            Subject::Unit(e) => unit_claim(self.unit_rooms.get(e).ok(), &self.placements),
        }
    }

    /// Has the room tracker reached this subject yet?
    ///
    /// `false` only on a unit's very first frame. It matters because an unsettled claim admits
    /// *both* liquid sources — the exact false positive 0696 removed — so a state a subject can
    /// only ENTER (swimming) must wait for it, while a state it can LEAVE must not.
    /// The loaded surfaces whose footprint box covers `wow`'s XY — the index's candidates,
    /// resolved to their chunk infos (a surface despawned since the index's last maintenance
    /// simply resolves to nothing).
    fn over(&self, wow: [f32; 3]) -> impl Iterator<Item = &WaterChunkInfo> {
        self.index
            .over(wow[0], wow[1])
            .iter()
            .filter_map(|&e| self.liquids.get(e).ok())
    }

    pub fn room_settled(&self, who: Subject) -> bool {
        self.claim(who) != LiquidClaim::Unknown
    }

    /// The liquid at `wow` for this subject, nearest surface first, or `None` for dry.
    ///
    /// Every liquid, not only water: **you swim in lava and slime too** (0634).
    pub fn liquid_at(&self, who: Subject, wow: [f32; 3]) -> Option<LiquidHit> {
        liquid_at(self.over(wow), wow, self.claim(who))
    }

    /// The **water** surface height at `wow` for this subject — [`Self::liquid_at`] with the
    /// fullbright kinds (magma, slime) filtered out, for the consumers that are specifically about
    /// water: the splash, the wake foam, footstep depth.
    pub fn water_surface_at(&self, who: Subject, wow: [f32; 3]) -> Option<f32> {
        water_surface_at(self.over(wow), wow, self.claim(who))
    }

    /// Every loaded liquid footprint covering this XY, one human-readable line each — the body of
    /// the `/liquid` chat instrument. Prints every candidate, not just the winner, so a surface
    /// that should not be claiming is visible next to the one that should.
    pub fn describe_liquid_at(&self, who: Subject, wow: [f32; 3]) -> Vec<String> {
        describe_at(self.over(wow), wow, self.claim(who))
    }

    /// What the camera eye is currently submerged in — **which liquid, not merely whether**. The
    /// kind is load-bearing: water reads the zone's underwater slot, magma and slime read fixed
    /// global rows (byte-verified `0x6d2371`).
    pub fn submersion(&self) -> benilla_formats::Submersion {
        self.underwater.0
    }

    /// The `TerrainType` id under a subject's feet — the `$FSD` footstep sound, the footprint
    /// decal and the footstep spray all read this one value, and the reference keeps it as one
    /// cached dword per unit for exactly that reason (`CGUnit+0xc60`). `pos` is Bevy space.
    ///
    /// `None` is the client's `−1` sentinel: **silent, and no print** — never "try the other leg".
    pub fn terrain_type(
        &self,
        catalog: &benilla_formats::FootstepCatalog,
        who: Subject,
        pos: Vec3,
    ) -> Option<u32> {
        self.surface.terrain_type(catalog, self.unit_room(who), pos)
    }

    /// Which WMO group a subject stands in, if a building owns its column — the number a
    /// diagnostic prints to say *which leg answered*. `None` is the open world.
    pub fn room_group(&self, who: Subject) -> Option<u16> {
        self.unit_room(who)
            .and_then(UnitWmoRoom::room)
            .map(|r| r.group)
    }

    /// The per-unit room component, for the two engine calls that still take one. Only `Unit`
    /// subjects have one: the player's and the eye's rooms are resources of their own.
    fn unit_room(&self, who: Subject) -> Option<&UnitWmoRoom> {
        match who {
            Subject::Unit(e) => self.unit_rooms.get(e).ok(),
            _ => None,
        }
    }

    /// The finest area the player stands in — the client's own `GetAreaID`, WMO-interior claim
    /// included (decision 0232's leaf authority). `None` until the ground tile is resident, or
    /// off-terrain.
    pub fn area(&self) -> Option<u32> {
        self.area.0
    }

    /// The MCNK `areaId` under an arbitrary point (Bevy space) — the outdoor leg alone. Use
    /// [`Self::area`] for the player, whose claim is richer.
    pub fn area_id_under(&self, pos: Vec3) -> Option<u32> {
        self.surface.area_id_under(pos)
    }

    /// The terrain height under a point (Bevy space), from the MCNK heightfield.
    pub fn terrain_height_under(&self, pos: Vec3) -> Option<f32> {
        self.surface.terrain_height_under(pos)
    }

    /// Is this point inside a building? The faces-only down-ray, raced against the terrain.
    pub fn indoors_at(&self, feet_world: Vec3) -> bool {
        self.surface.indoors_at(feet_world)
    }

    /// The player's **render** interior claim — the `WMOAreaTable` join keys of the room the
    /// down-ray puts them in, or `None` in the open world. Drives the interior audio resolver and
    /// the minimap's indoor overlay.
    pub fn interior(&self) -> Option<crate::wmo_portal::WmoInteriorKeys> {
        self.interior.0
    }

    /// The `WMOAreaTable` row of the room the eye is in — the interior's own identity: its
    /// `AreaTable` id and its audio FKs (sound provider, ambience, zone music, entry fanfare).
    /// `None` in the open world, or when the client data did not load.
    ///
    /// The resolve is a three-step fallback the engine owns (exact group row → whole-WMO default →
    /// name-set-0), so a caller asks the world what room it is in and gets the room, not a catalog
    /// and a lookup rule to re-implement.
    pub fn interior_row(&self) -> Option<benilla_formats::WmoArea> {
        let keys = self.interior()?;
        let areas = self.wmo_areas.as_ref()?;
        areas
            .0
            .resolve(keys.wmo_id, keys.name_set, keys.group_area_id)
    }

    /// The two **exact-key** `WMOAreaTable` rows the zone-text indoor naming reads (`0x67e670`
    /// (d)/(d-ii)): the hit GROUP's row, which re-populates the subzone, and the WMO's default
    /// (`−1`) row, which may override the zone. Returned with the claim's own keys, because the
    /// naming law dedups on the hit group's identity.
    ///
    /// **Exact key, no name-set retry** — deliberately not [`Self::interior_row`]'s three-step
    /// resolve. The audio identity wants the nearest row it can find; the zone text wants the row
    /// the client would have found, and a fallback here invents a name the reference never shows.
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

    /// The player's **zone-text** interior claim, which is deliberately not the same question: a
    /// doorway portal under the eye seeds the render flood without making you indoors — that bit
    /// comes from a down-ray under the player alone (`0x6a87f0`).
    pub fn area_interior(&self) -> Option<crate::wmo_portal::WmoInteriorKeys> {
        self.area_interior.0
    }

    /// The nearest wet point to `wow` within `radius`, one per liquid **sound class**, indexed by
    /// class (`nibble & 3`) — the scan behind the ambient liquid loops (`0x462b50`'s
    /// nearest-liquid walk). The class split and the AABB-clamp nearest point are the world's;
    /// the priority order, the voice cap and the slew are the sound system's.
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

    /// **The one semantic change this facade makes**, pinned: a caller used to fetch
    /// `Option<&UnitWmoRoom>` in its own query and hand it in; now it names the unit and the world
    /// looks the room up. Those must be the same answer, including the two `None` cases — a unit
    /// the room tracker has not reached, and an entity that is not a unit at all. Getting this
    /// wrong is silent: every consumer would simply read the open world's liquid indoors, which is
    /// the "swim in air" family (0634/0696/0701) coming back through the front door.
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
                // The player and the eye are resources, never a unit's component — asking them for
                // a *unit's* room must not silently answer with somebody else's.
                assert_eq!(point.room_group(Subject::Player), None);
                assert_eq!(point.room_group(Subject::Eye), None);
            })
            .unwrap();
    }
}
