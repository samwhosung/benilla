//! The `TerrainType` id under a unit's feet, answered in one place. The reference caches it per
//! unit at `CGUnit+0xc60`, resolved on movement by an environment down-ray (`0x6a8a20`), and four
//! readers share it: the `$FSD` footstep sound (`0x62341d`), the footprint decal (`0x5fc06e`), the
//! footstep spray (`0x5fc20f`) and `0x623749`.
//!
//! A terrain probe (`0x69c320`) races a WMO probe (`0x6a8840`) over the building's collision
//! faces, and the nearer surface supplies the type; here that race is the [`UnitWmoRoom`] claim.
//! With a claim, [`surface_terrain_sample`] re-casts over the group's render faces and reads the
//! hit face's `MOPY.material_id → MOMT+0x20`, a `TerrainType.dbc` id. Without one, the MCNK
//! dominant layer goes through `GroundEffectTexture` to `TerrainType`. `None` is the reference's
//! `−1`: silent, no print, and never a retry on the terrain under a claimed floor.
//!
//! The claim covers interior groups only, so a surface on an exterior group (bridges and docks,
//! `Wood`) still takes the terrain leg, unlike the reference; the fix belongs in the claim.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_assets::{AdtTile, WmoModel};
use benilla_formats::FootstepCatalog;

use crate::terrain_stream::{
    area_id_under, ground_effect_under, terrain_height_under, TerrainStreamer,
};
use crate::wmo_portal::{
    indoors_at, surface_terrain_sample, UnitWmoRoom, WmoPortalInstance, POSITION_PROBE_LIFT,
};

/// The placements and terrain the surface question rays, read live so no answer outlives its
/// placement.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SurfaceUnderfoot<'w, 's> {
    wmos: Res<'w, Assets<WmoModel>>,
    instances: Query<'w, 's, &'static WmoPortalInstance>,
    streamer: Res<'w, TerrainStreamer>,
    adt_tiles: Res<'w, Assets<AdtTile>>,
}

impl SurfaceUnderfoot<'_, '_> {
    /// The `TerrainType` id under `pos` (Bevy space) for a unit holding `room`; `None` is the
    /// reference's `−1`.
    pub(crate) fn terrain_type(
        &self,
        cat: &FootstepCatalog,
        room: Option<&UnitWmoRoom>,
        pos: Vec3,
    ) -> Option<u32> {
        match room.and_then(UnitWmoRoom::room) {
            Some(room) => {
                let inst = self.instances.get(room.instance).ok()?;
                let model = self.wmos.get(&inst.handle)?;
                // The claim ray's own origin lift: a coplanar `z <= probe.z` test must not lose
                // the floor, and another origin could hit a different face than the one that won.
                let probe_world = pos + Vec3::Y * POSITION_PROBE_LIFT;
                let local = inst
                    .world_from_local
                    .inverse()
                    .transform_point3(probe_world);
                surface_terrain_sample(model, usize::from(room.group), bevy_to_wow(local))
            }
            None => ground_effect_under(&self.streamer, &self.adt_tiles, pos)
                .and_then(|e| cat.terrain_of(e)),
        }
    }

    /// The MCNK `areaId` under `pos` (Bevy space), without the WMO claim that the player's own
    /// [`crate::terrain_stream::CurrentArea`] includes.
    pub fn area_id_under(&self, pos: Vec3) -> Option<u32> {
        area_id_under(&self.streamer, &self.adt_tiles, pos)
    }

    /// The MCNK heightfield's height under `pos` (Bevy space), `None` off the streamed set; not
    /// the physics trimesh, which may differ.
    pub fn terrain_height_under(&self, pos: Vec3) -> Option<f32> {
        terrain_height_under(&self.streamer, &self.adt_tiles, pos)
    }

    /// Whether `feet_world` is inside a building: the faces-only down-ray raced against the
    /// terrain, so the grass above a mine's tunnels is outdoors.
    pub fn indoors_at(&self, feet_world: Vec3) -> bool {
        indoors_at(
            &self.wmos,
            self.instances,
            &self.streamer,
            &self.adt_tiles,
            feet_world,
        )
    }
}
