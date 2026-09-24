//! **What you are standing on** — the `TerrainType` id under a unit's feet, for every consumer of
//! it.
//!
//! The reference keeps this as one cached dword per unit, `CGUnit+0xc60`: an environment-node
//! down-ray (`0x6a8a20`) resolves it on movement, and four readers consume that single value — the
//! `$FSD` footstep sound (`0x62341d`), the footprint decal (`0x5fc06e`), the footstep spray
//! (`0x5fc20f`) and `0x623749`. They cannot disagree about the ground, and neither can ours: this
//! module is the one place the question is answered.
//!
//! **Two legs, arbitrated by nearest hit.** The client races a terrain probe (`0x69c320`) against a
//! WMO probe (`0x6a8840`) over the building's COLLISION faces; whichever surface is nearer under
//! the foot supplies the type. That arbitration is already ours — it is the same faces-only,
//! terrain-raced down-ray that publishes [`UnitWmoRoom`], so a room claim *is* "the building won
//! this column".
//!
//! - **WMO leg.** The claim names a group; [`surface_terrain_sample`] re-casts the column over that
//!   group's RENDER faces and reads the hit face's `MOPY.material_id → MOMT+0x20` — a
//!   `TerrainType.dbc` id carried by the surface itself, with no `GroundEffectTexture` hop.
//! - **ADT leg.** No claim: the MCNK dominant layer → `GroundEffectTexture` → `TerrainType`, the
//!   chain that was already here.
//!
//! `None` is the client's `−1` sentinel and means **silent, and no print** — not "try the other
//! leg". A building that won the column but whose group has no render face under the probe is
//! silent; falling back to the ground beneath the floor is exactly the bug this module exists to
//! kill (B236's sequel: snow crunching inside the Kharanos inn, and snow prints on its floorboards,
//! because the ADT leg answered indoors).
//!
//! **Known gap — exterior groups.** Both the claim and the render-face set are built for INTERIOR
//! groups only, so a WMO surface authored on an *exterior* group (bridges and docks, `Wood`) still
//! falls to the ADT leg. Wrong in the same direction as before this module, never newly wrong; the
//! fix is widening the claim, not special-casing here.

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

/// The world state the surface question needs, as one `SystemParam` — the placements it rays, the
/// terrain it races them against. Threaded as a query rather than read into a resource for the
/// same reason `liquid::RoomPlacements` is: the answer must not outlive the placement.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SurfaceUnderfoot<'w, 's> {
    wmos: Res<'w, Assets<WmoModel>>,
    instances: Query<'w, 's, &'static WmoPortalInstance>,
    streamer: Res<'w, TerrainStreamer>,
    adt_tiles: Res<'w, Assets<AdtTile>>,
}

impl SurfaceUnderfoot<'_, '_> {
    /// The `TerrainType` id under `pos` (Bevy space) for a unit holding `room`, or `None` for the
    /// client's `−1` — silent, no print. See the module docs for why `None` never retries.
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
                // The SAME origin lift the claim's own ray used: feet rest on the floor plane, and
                // a coplanar `z <= probe.z` test must not lose the floor to a rounding hair. Using
                // a different origin here could resolve a different face than the one that won.
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

    /// The MCNK `areaId` under `pos` (Bevy space) — the *outdoor* leg of "where is this", with no
    /// WMO-interior claim. The player's own area is [`crate::terrain_stream::CurrentArea`], which
    /// includes that claim; this is what every other body gets.
    pub fn area_id_under(&self, pos: Vec3) -> Option<u32> {
        area_id_under(&self.streamer, &self.adt_tiles, pos)
    }

    /// The terrain height under `pos` (Bevy space), from the MCNK heightfield — `None` off the
    /// streamed set. **Not** the physics trimesh: the two are allowed to differ, which is why
    /// tracing a body through the world is a separate question (1164 item 11).
    pub fn terrain_height_under(&self, pos: Vec3) -> Option<f32> {
        terrain_height_under(&self.streamer, &self.adt_tiles, pos)
    }

    /// Is `feet_world` inside a building? The faces-only down-ray, raced against the terrain —
    /// standing on the grass above a mine's tunnels is not standing in the mine.
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
