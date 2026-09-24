//! What the streamed world answers about a position: the spawn-time MCSH ground-shade lookup,
//! the ground-effect/height queries the sound + clutter systems make against resident tiles, and
//! the area authority (the `AreaTable.dbc` leaf under the player's feet, decision 0070).

use benilla_assets::coords::bevy_to_wow;
use benilla_assets::AdtTile;
use benilla_formats::{mcsh_shadowed_at, world_to_tile};
use bevy::prelude::*;

use super::TerrainStreamer;

/// The `AreaTable.dbc` id under the player's feet — the zone/subzone, from the containing
/// resident chunk's MCNK `areaId` (decision 0070: drives zone music/ambience/reverb; later the
/// minimap zone text). `None` until the ground tile is resident (or off-terrain). Written each
/// frame by [`update_current_area`]; consumers change-detect on the inner value.
///
/// **It is scoped to the character session, not to the process** (decision 2130). It goes back to
/// `None` the moment there is no avatar to measure from, so "we have not answered yet" is never
/// spelled the same way as "here is the character before this one".
#[derive(Resource, Default, PartialEq, Eq)]
pub struct CurrentArea(pub Option<u32>);

/// Ordering handle on [`update_current_area`] — the leaf-authority write. The zone-text feed
/// (`crate::area`) orders after it (which itself orders after the interior claim), so leaf +
/// indoor bit + names always come from one coherent frame — the client's single-pass resolve.
///
/// **Every consumer that ACTS on the area belongs after it**, and the zone-channel walk is the
/// case that proves it (decision 2130): unordered, it read whatever the previous frame had
/// published and paid for the difference in `CMSG_JOIN_CHANNEL`/`CMSG_LEAVE_CHANNEL` traffic for
/// zones the player was never in.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AreaAuthoritySet;

/// Outcome of the spawn-time MCSH ground-shade lookup.
pub enum ShadeResolve {
    /// The shade is known: `true` = the doodad's base sits on MCSH-shadowed terrain (sun ×0.5 in the
    /// lobe), `false` = lit (×2.5, or a deliberate fallback when the ground tile isn't resident).
    Ready(bool),
    /// The doodad's own ground tile is requested but hasn't decoded yet — defer the spawn one frame so a
    /// straddling tree isn't baked lit before its true tile lands.
    Pending,
}

/// Resolve a doodad's terrain ground-shade the reference way: a GLOBAL world-position → tile → chunk
/// MCSH lookup at the doodad's origin (the client's `0x69b350`), NOT a sample of whichever tile happened
/// to register the placement — the latter is load-order/timing dependent and left straddling trees wrongly
/// lit. `bevy_pos` is the doodad's origin (its `Transform.translation`).
///
/// - Ground tile resident + decoded → the true MCSH bit under the footprint.
/// - Ground tile requested but still decoding → [`ShadeResolve::Pending`] (caller defers the spawn).
/// - Ground tile not in the loaded set (a doodad straddling in from just beyond the ring) → `Ready(false)`:
///   show it lit rather than hide it, matching the reference (a null tile reads lit until it streams in).
///   (A doodad whose origin tile is *requested but never decodes* — a missing map-edge ADT — would defer
///   indefinitely; that can't arise for a real placement, whose origin always lies in an existing tile.)
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

/// The MCNK `areaId` under a **Bevy-space** position on the resident terrain — the OUTDOOR leg of
/// the client's `GetAreaID 0x670250`, for any unit rather than the player.
///
/// [`CurrentArea`] is the player's authority and races a WMO interior claim ahead of this
/// ([`update_current_area`]); there is no per-unit equivalent of that claim, so a caller asking
/// about a *remote* unit gets the terrain answer alone. `None` off-terrain / mid-stream, and an
/// `areaId` of 0 (unassigned in the data) reads as a miss, same as the player's resolver.
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

/// The `GroundEffectTexture` id under a **Bevy-space** position on the resident terrain — the
/// footstep terrain-type source (decision 0070 slice 3; the same global position→tile→chunk
/// lookup shape as [`doodad_ground_shade`]). `None` off-terrain / mid-stream / no-effect cell —
/// the footstep resolver falls back to its Dirt default.
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

/// The terrain surface height (raw WoW `z`) under a **Bevy-space** position on the resident terrain —
/// the terrain leg of the client's down-ray arbitration (`FUN_006821f0`'s `0x69c320` probe, and the
/// identical one in `GetAreaID 0x670250`), which races the WMO probe and wins the column whenever the
/// ground is strictly nearer under the eye. Same global position→tile→chunk lookup shape as
/// [`ground_effect_under`].
///
/// `None` means **no terrain surface in this column**, which the race must read as "no terrain hit",
/// never as "ground at 0": off the streamed ring, mid-decode, or — the load-bearing case — the column
/// falls in an MCNK hole, the cut-out through which a mine or cave entrance reaches its WMO interior.
pub fn terrain_height_under(
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    bevy_pos: Vec3,
) -> Option<f32> {
    terrain_height_under_cached(streamer, adt_tiles, bevy_pos, &mut None)
}

/// [`terrain_height_under`] for a caller that asks many columns of the same tile in one go —
/// the sun-flare march asks ~96 a frame along two rays (decision 1979): the tile resolution
/// (a streamer map lookup and an asset lookup) is done once per tile change, not per column.
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

/// Track the `AreaTable` id under the player's feet. Faithful to the client's GetAreaID resolver
/// (`GetAreaID 0x670250`): a WMO interior takes precedence over the terrain chunk when the
/// down-ray keeps the WMO nearer — [`CurrentAreaInterior`] is exactly that player-position
/// **faces-only** down-ray (`0x6a8a20`; the portal-legged render seed flipped in the abbey
/// yard), so an indoor city (Ironforge, Undercity) reports its OWN area via
/// `WMOAreaTable.AreaTableID` rather than the enclosing zone's terrain. Outdoors (no interior
/// claim, or a group with no area row) it falls to the containing chunk's MCNK `areaId`. Holds
/// the previous value while the tile is still decoding, so a tile-edge crossing never flickers
/// through `None`.
pub(super) fn update_current_area(
    mut area: ResMut<CurrentArea>,
    focus: Res<crate::terrain_stream::ViewFocus>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    interior: Res<crate::wmo_portal::CurrentAreaInterior>,
    wmo_areas: Option<Res<crate::wmo_portal::WmoAreas>>,
) {
    let Some(wow) = focus.body_pos() else {
        // **No avatar — and the authority DIES with the character session** (decision 2130).
        //
        // The hold below is a within-session convenience (a tile-edge crossing must not flicker
        // through `None`). Holding across the *session* boundary is a different thing entirely:
        // this resource had exactly one writer, which only ever assigned `Some`, so once set it
        // could never return to `None` for the life of the process — and the next character's
        // login read the PREVIOUS character's zone until their own tiles decoded. The zone-channel
        // walk believed it and joined `General - Stormwind City` for a character standing in the
        // Eastern Plaguelands, then left it again a beat later; `sound/mod.rs`'s `world_audio_live`
        // already carried a hand-written guard against the same staleness, which is the tell that
        // the defect was the lifetime and not either consumer.
        //
        // `body_pos()` is `Some` only for a live avatar (`ViewFocus::body`/`detached`) and `None`
        // for both glue-screen focuses, so this is the session edge without a state transition to
        // subscribe to. A recoverable disconnect keeps the body as the local puppet (0065), so the
        // area correctly survives one.
        if area.0.is_some() {
            *area = CurrentArea(None);
        }
        return;
    };
    if !focus.body_settled() {
        // …and one whose own world is still arriving has no area worth publishing: the leaf under
        // it is not yet the leaf it is standing in. Hold the last real answer instead — the
        // reference is behind a loading screen for exactly this window
        // ([`ViewFocus::body_settled`], 1287).
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

    /// A world with every resource [`update_current_area`] reads, and no terrain at all — so the
    /// only thing under test is what the system does about the *body*, not what it finds under one.
    fn bare_world() -> World {
        let mut w = World::new();
        w.init_resource::<CurrentArea>();
        w.init_resource::<crate::terrain_stream::ViewFocus>();
        w.init_resource::<TerrainStreamer>();
        w.init_resource::<Assets<AdtTile>>();
        w.init_resource::<crate::wmo_portal::CurrentAreaInterior>();
        w
    }

    /// **The area authority dies with the character session** (decision 2130).
    ///
    /// It had exactly one writer, which only ever assigned `Some`, and nothing reset it — so once
    /// a character had published a zone it stayed published for the life of the *process*. The
    /// next login read it before its own tiles decoded, and the zone-channel walk turned that into
    /// `CMSG_JOIN_CHANNEL` for the previous character's capital: the director logged into a
    /// character in the Eastern Plaguelands and watched Stormwind City's three channels join and
    /// then leave again.
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

        // …and then logged out. `ViewFocus` carries no body at either glue screen.
        w.insert_resource(crate::terrain_stream::ViewFocus::camera());
        w.run_system_once(update_current_area).unwrap();
        assert_eq!(
            w.resource::<CurrentArea>().0,
            None,
            "the authority follows the character, and there is none — a held value here is the \
             NEXT character's login reading THIS character's zone"
        );

        // And the entry-window focus (decision 0777) is the same: a picked row is not a body.
        w.insert_resource(CurrentArea(Some(1519)));
        w.insert_resource(crate::terrain_stream::ViewFocus::entry(
            0,
            [-8900.0, -130.0, 80.0],
        ));
        w.run_system_once(update_current_area).unwrap();
        assert_eq!(w.resource::<CurrentArea>().0, None);
    }

    /// A **recoverable** disconnect keeps the avatar as the local puppet (0065), so the area must
    /// survive one: nothing about the world under the player's feet changed when the socket died.
    /// This is why the clear hangs off "is there a body" rather than off the net session.
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
