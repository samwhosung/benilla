//! What the streamed world answers about a position: MCSH ground shade, ground effect, terrain
//! height, and the area authority (the `AreaTable.dbc` leaf under the player's feet).

use benilla_assets::coords::bevy_to_wow;
use benilla_assets::AdtTile;
use benilla_formats::{mcsh_shadowed_at, world_to_tile};
use bevy::prelude::*;

use super::TerrainStreamer;

/// The `AreaTable.dbc` id under the player's feet, written by [`update_current_area`]. `None` until
/// the ground tile is resident, and again whenever there is no avatar: it lives with the character
/// session, not the process.
#[derive(Resource, Default, PartialEq, Eq)]
pub struct CurrentArea(pub Option<u32>);

/// The set of [`update_current_area`]. Every consumer that acts on the area orders after it, so
/// leaf, indoor bit and names come from one frame (the reference resolves them in one pass).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AreaAuthoritySet;

/// Outcome of the spawn-time MCSH ground-shade lookup.
pub enum ShadeResolve {
    /// `true`: the base sits on MCSH-shadowed terrain; `false`: lit, or no loaded ground tile.
    Ready(bool),
    /// The ground tile is still decoding: defer the spawn a frame rather than bake it lit.
    Pending,
}

/// A doodad's ground shade by a global position → tile → chunk MCSH lookup at its origin (the
/// reference's `0x69b350`), not a sample of the tile that registered it. A tile outside the loaded
/// set reads lit, as the reference's null tile does.
pub fn doodad_ground_shade(
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    bevy_pos: Vec3,
) -> ShadeResolve {
    let wow = bevy_to_wow(bevy_pos);
    let (tx, ty) = world_to_tile(wow[0], wow[1]);
    match streamer.tiles.get(&(tx as i32, ty as i32)) {
        None => ShadeResolve::Ready(false),
        Some(ts) => match adt_tiles.get(&ts.handle) {
            None => ShadeResolve::Pending,
            Some(adt) => ShadeResolve::Ready(mcsh_shadowed_at(&adt.chunks, wow).unwrap_or(false)),
        },
    }
}

/// The MCNK `areaId` under a Bevy-space position: the outdoor leg of the reference's `GetAreaID`
/// (`0x670250`), for any unit, with no interior claim. An `areaId` of 0 (unassigned) is a miss.
pub fn area_id_under(
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    bevy_pos: Vec3,
) -> Option<u32> {
    let wow = bevy_to_wow(bevy_pos);
    let (tx, ty) = world_to_tile(wow[0], wow[1]);
    let ts = streamer.tiles.get(&(tx as i32, ty as i32))?;
    let adt = adt_tiles.get(&ts.handle)?;
    benilla_formats::area_id_at(&adt.chunks, wow).filter(|&id| id != 0)
}

/// The `GroundEffectTexture` id under a Bevy-space position, the footstep terrain type.
pub fn ground_effect_under(
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    bevy_pos: Vec3,
) -> Option<u32> {
    let wow = bevy_to_wow(bevy_pos);
    let (tx, ty) = world_to_tile(wow[0], wow[1]);
    let ts = streamer.tiles.get(&(tx as i32, ty as i32))?;
    let adt = adt_tiles.get(&ts.handle)?;
    benilla_formats::ground_effect_at(&adt.chunks, wow)
}

/// The terrain height (WoW `z`) under a Bevy-space position: the terrain leg of the reference's
/// down-ray arbitration (`0x6821f0`'s `0x69c320` probe, also in `GetAreaID` `0x670250`), which
/// wins the column when the ground is strictly nearer than the WMO. `None` is no terrain in the
/// column (off the ring, decoding, or an MCNK hole over a cave mouth), never ground at 0.
pub fn terrain_height_under(
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    bevy_pos: Vec3,
) -> Option<f32> {
    terrain_height_under_cached(streamer, adt_tiles, bevy_pos, &mut None)
}

/// [`terrain_height_under`] for many columns at once: the tile resolves once per tile change.
pub fn terrain_height_under_cached<'a>(
    streamer: &TerrainStreamer,
    adt_tiles: &'a Assets<AdtTile>,
    bevy_pos: Vec3,
    cache: &mut Option<((i32, i32), &'a AdtTile)>,
) -> Option<f32> {
    let wow = bevy_to_wow(bevy_pos);
    let (tx, ty) = world_to_tile(wow[0], wow[1]);
    let key = (tx as i32, ty as i32);
    let adt = match cache {
        Some((k, adt)) if *k == key => *adt,
        _ => {
            let ts = streamer.tiles.get(&key)?;
            let adt = adt_tiles.get(&ts.handle)?;
            *cache = Some((key, adt));
            adt
        }
    };
    benilla_formats::terrain_height_at(&adt.chunks, wow)
}

/// Tracks the `AreaTable` id under the player's feet as the reference's `GetAreaID` (`0x670250`)
/// does: a WMO interior wins when the faces-only down-ray (`0x6a8a20`) finds it nearer, by its
/// `WMOAreaTable.AreaTableID`; outdoors it is the chunk's MCNK `areaId`. Holds the last value
/// while a tile decodes.
pub(super) fn update_current_area(
    mut area: ResMut<CurrentArea>,
    focus: Res<crate::terrain_stream::ViewFocus>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    interior: Res<crate::wmo_portal::CurrentAreaInterior>,
    wmo_areas: Option<Res<crate::wmo_portal::WmoAreas>>,
) {
    let Some(wow) = focus.body_pos() else {
        // No avatar: the area dies with the character session, or the next login reads this
        // character's zone. `body_pos()` is `None` at both glue screens; a recoverable disconnect
        // keeps the body, and the area with it.
        if area.0.is_some() {
            *area = CurrentArea(None);
        }
        return;
    };
    if !focus.body_settled() {
        // A body whose world is still arriving holds the last answer: the reference is behind a
        // loading screen for this window.
        return;
    }
    // WMO interior first: the player's down-ray group resolved to its WMOAreaTable world area.
    let interior_area = interior.0.zip(wmo_areas.as_ref()).and_then(|(k, cat)| {
        cat.0
            .resolve(k.wmo_id, k.name_set, k.group_area_id)
            .map(|a| a.area_table_id)
            .filter(|&id| id != 0)
    });
    let found = interior_area.or_else(|| {
        let (tx, ty) = world_to_tile(wow[0], wow[1]);
        streamer
            .tiles
            .get(&(tx as i32, ty as i32))
            .and_then(|ts| adt_tiles.get(&ts.handle))
            .and_then(|adt| benilla_formats::area_id_at(&adt.chunks, wow))
    });
    // 0 = unassigned in the data; treat like a miss so schedulers keep the last real zone.
    let found = found.filter(|&id| id != 0);
    if found.is_some() && *area != CurrentArea(found) {
        *area = CurrentArea(found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Every resource [`update_current_area`] reads, and no terrain.
    fn bare_world() -> World {
        let mut w = World::new();
        w.init_resource::<CurrentArea>();
        w.init_resource::<crate::terrain_stream::ViewFocus>();
        w.init_resource::<TerrainStreamer>();
        w.init_resource::<Assets<AdtTile>>();
        w.init_resource::<crate::wmo_portal::CurrentAreaInterior>();
        w
    }

    #[test]
    fn no_avatar_means_no_area() {
        let mut w = bare_world();
        // A character session that published its zone…
        w.insert_resource(CurrentArea(Some(1519))); // Stormwind City
        w.insert_resource(crate::terrain_stream::ViewFocus::body(
            [-8900.0, -130.0, 80.0],
            true,
        ));
        w.run_system_once(update_current_area).unwrap();
        assert_eq!(
            w.resource::<CurrentArea>().0,
            Some(1519),
            "with a body and no resident tile the last real answer is HELD — that is the \
             tile-edge/flicker guard, and it stays"
        );

        // …then logged out: no body at either glue screen.
        w.insert_resource(crate::terrain_stream::ViewFocus::camera());
        w.run_system_once(update_current_area).unwrap();
        assert_eq!(
            w.resource::<CurrentArea>().0,
            None,
            "the authority follows the character, and there is none — a held value here is the \
             NEXT character's login reading THIS character's zone"
        );

        // And the entry-window focus is the same: a picked row is not a body.
        w.insert_resource(CurrentArea(Some(1519)));
        w.insert_resource(crate::terrain_stream::ViewFocus::entry(
            0,
            [-8900.0, -130.0, 80.0],
        ));
        w.run_system_once(update_current_area).unwrap();
        assert_eq!(w.resource::<CurrentArea>().0, None);
    }

    /// A recoverable disconnect keeps the avatar as the local puppet, and with it the area.
    #[test]
    fn a_body_that_survives_a_drop_keeps_its_area() {
        let mut w = bare_world();
        w.insert_resource(CurrentArea(Some(12)));
        w.insert_resource(crate::terrain_stream::ViewFocus::body(
            [0.0, 0.0, 0.0],
            false,
        ));
        w.run_system_once(update_current_area).unwrap();
        assert_eq!(w.resource::<CurrentArea>().0, Some(12));
    }
}
