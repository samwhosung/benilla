//! **The guard's directions** — the marker `SMSG_GOSSIP_POI` drops on the minimap and the world
//! map when you ask a city guard where the warrior trainer is (or the bank, or the inn).
//!
//! The wire is volunteered, never requested: a gossip option carrying an `action_poi_id` makes the
//! server send `{flags, x, y, icon, data, name}` (vmangos `Player::OnGossipSelect` →
//! `PlayerMenu::SendPointOfInterest`, `GossipDef.cpp:253`). Every 5875-era row ships
//! `icon = 6` (`ICON_POI_REDFLAG` — the red flag with the yellow `!`) and `flags = 99`.
//!
//! **The reference does NOT give this its own drawing code.** It builds a synthetic `AreaPOI`
//! record out of the packet and appends it to the minimap's landmark candidate list — one fixed
//! static slot, so a new marker overwrites the old. Byte law and packet→record map (1516): handler
//! `0x4e2840`, `set_blip 0x6dac10` writing static slot **1** at `0xcea7d4`, and the minimap's
//! candidate array, which appends the static slots *unconditionally* (`0x6d8fa8`), bypassing
//! the DBC scan's `ContinentID`/`Flags & 1` gate. So it is a landmark
//! in every way that follows — the 0.8 in/out split, the `POIIcons` cell picked by `Icon`, the rim
//! arrow, the nearest-3 `Importance` rank, the 694.444-yd rank cut (the marker gets **no**
//! exemption from it — that belongs to the corpse slot `0xcea848`), the hover tooltip on both the
//! icon and the arrow. This module holds only what is *specific* to the marker: the record, and
//! its lifetime. The drawing is [`crate::minimap::blips`] and the world map's POI pool.
//!
//! **The lifetime is four ways to lose it, whichever comes first**:
//! - **8 minutes.** `set_blip` stamps a deadline of `time() + 480` — *seconds*, not the
//!   milliseconds a `GetTickCount` reading would suggest (`0x429580` is a cached `time()`; the
//!   tick count is only its 500 ms cache check).
//! - **Arriving.** `minimap_update 0x6d93a0` clears it once `(player − marker)² < 100`
//!   (`0x806b10`) — strictly inside 10 yards, the boundary excluded.
//! - **A replacing packet.** One slot; the next set of directions overwrites this one.
//! - **World entry.** The world-enter path runs `zone_rebuild`, which stamps the slot clear. Ours
//!   maps that to a worldport and to logging out — the two ways a benilla session re-enters.
//!
//! The record keeps the map it was given on, because the wire has no map field and the world map
//! must project the marker through *some* map's rect. The reference re-derives the same thing from
//! its live current-map global at `set_blip` time (`+0x1c ContinentID ← [0x86f694]`).

use benilla_formats::AreaPoi;
use bevy::prelude::*;
use bevy::time::Real;

use benilla_assets::coords::bevy_to_wow;

use crate::net::{LoggedOutMessage, WorldportMessage};
use crate::player::Player;

/// The arrival clear's radius, squared, in yards² — `0x806b10` = 100 = (10 yd)², VERIFIED, and the
/// compare is strict (`<`), so a marker exactly 10 yd away survives.
const ARRIVE_CLEAR_YD_SQ: f32 = 100.0;
/// How long a set of directions lasts before it drops itself: `time() + 480` **seconds**
/// (`set_blip 0x6dac10`'s deadline stamp; 480 ms would be a marker you could never walk to).
const MARKER_TTL_SECS: f64 = 480.0;

/// The one point of interest a guard's directions left on the map — a synthetic `AreaPOI` record,
/// exactly as the reference builds it, or `None` when no directions are live.
///
/// Read by the minimap's landmark pass ([`crate::minimap::blips`]) and the world map's feed
/// ([`crate::ui_world_map`]); written only by the `SMSG_GOSSIP_POI` arm and the clears below.
#[derive(Resource, Default)]
pub(crate) struct PoiMarker {
    /// The record. Its `continent_id` is the map the directions were given on, and its `pos` z is
    /// `0.0`: the wire carries x/y only, and every law that reads it measures a 2-D distance (the
    /// reference leaves `+0x18 Z` unwritten for the same reason).
    pub(crate) poi: Option<AreaPoi>,
    /// `Time<Real>` seconds at which the marker expires — meaningless while `poi` is `None`. Real,
    /// not virtual: this is a wall-clock span like the corpse reclaim delay (decision 0846).
    expires_at: f64,
}

impl PoiMarker {
    /// Take the guard's directions: replace whatever marker was live (the reference's single
    /// slot), and start its 8-minute clock. `map_id` is the map the player is standing on — the
    /// wire has no map field, and the reference reads its own current-map global here.
    pub(crate) fn set(
        &mut self,
        wire: &benilla_protocol::messages::GossipPoi,
        map_id: u32,
        now_secs: f64,
    ) {
        self.expires_at = now_secs + MARKER_TTL_SECS;
        self.poi = Some(AreaPoi {
            // The nearest-3 rim rank key is the packet's own `data` field, verbatim (`0x6dac4e`
            // writes it to `+0x04 Importance` — 1516's correction; we had guessed a constant `0`).
            // Shipped server data sends `0` in every row, which lands the marker in the
            // first rank band: it out-ranks 28 of Kalimdor's 29 possible competitors outright and
            // ties Eastern Kingdoms' Importance-0 landmarks, winning those on distance.
            importance: wire.data,
            icon: wire.icon,
            faction_id: 0,
            pos: [wire.pos[0], wire.pos[1], 0.0],
            continent_id: map_id,
            flags: wire.flags,
            area_id: 0,
            name: wire.name.clone(),
            // Never written by the handler — and the reference's `GetMapLandmarkInfo` returns nil
            // for this landmark's description because of it.
            description: String::new(),
            world_state_id: 0,
        });
    }

    /// The marker, if it is on `map_id` — the one form both draws want.
    pub(crate) fn on_map(&self, map_id: u32) -> Option<&AreaPoi> {
        self.poi.as_ref().filter(|p| p.continent_id == map_id)
    }
}

/// The two clocks that end a marker on their own: **arriving** (strictly inside 10 yd) and the
/// **8-minute deadline**. Both are the reference's, and both are checked where it checks them —
/// once a frame, in the minimap driver's place.
fn expire_marker(mut marker: ResMut<PoiMarker>, player: Res<Player>, time: Res<Time<Real>>) {
    let Some(poi) = &marker.poi else {
        return;
    };
    if time.elapsed_secs_f64() >= marker.expires_at {
        debug!("poi: \"{}\" expired — clearing the marker", poi.name);
        marker.poi = None;
        return;
    }
    let w = bevy_to_wow(player.pos);
    let d2 = (poi.pos[0] - w[0]).powi(2) + (poi.pos[1] - w[1]).powi(2);
    if d2 < ARRIVE_CLEAR_YD_SQ {
        debug!("poi: arrived at \"{}\" — clearing the marker", poi.name);
        marker.poi = None;
    }
}

/// **World entry clears it** — the reference's fourth exit, whose path stamps the slot clear
/// through `zone_rebuild`. Benilla re-enters the world in two ways: a worldport (the cross-map
/// teleport that IS a new world entry) and a logout, after which the next login is somebody else's
/// session.
fn clear_on_world_entry(
    mut marker: ResMut<PoiMarker>,
    mut worldports: MessageReader<WorldportMessage>,
    mut logged_out: MessageReader<LoggedOutMessage>,
) {
    if worldports.read().next().is_some() || logged_out.read().next().is_some() {
        marker.poi = None;
    }
}

/// The marker's packet handler (in the net handler table since 2318, moved out of the drain's npc
/// arm file) — registered from [`PoiMarkerPlugin`].
fn on_gossip_poi(
    In(ev): In<benilla_protocol::SessionEvent>,
    mut marker: ResMut<PoiMarker>,
    current_map: Option<Res<benilla_world::world_map::CurrentMap>>,
    real_clock: Res<Time<Real>>,
) {
    if let benilla_protocol::SessionEvent::GossipPoi(poi) = ev {
        gossip_poi(
            &poi,
            &mut marker,
            current_map.as_ref().map_or(0, |m| m.0),
            real_clock.elapsed_secs_f64(),
        );
    }
}

/// The guard's directions (`SMSG_GOSSIP_POI`): drop the marker at that spot. Volunteered by the
/// server for a gossip option carrying an `action_poi_id` — it answers nothing we asked for, and
/// it does **not** end the gossip session on its own (vmangos `Player::OnGossipSelect`'s
/// `GOSSIP_OPTION_GOSSIP` arm sends the POI *before* it decides whether to move the menu on, leave
/// it, or close it).
///
/// `map_id` is the map the player is standing on, which is the only place the marker can mean
/// anything — the wire carries no map field, and the reference reads its own current-map global at
/// exactly this point. `now_secs` starts the marker's 8-minute clock (this module); it
/// is the same real clock the corpse reclaim delay is stamped against (decision 0846).
fn gossip_poi(
    poi: &benilla_protocol::messages::GossipPoi,
    marker: &mut PoiMarker,
    map_id: u32,
    now_secs: f64,
) {
    debug!(
        "net: directions to \"{}\" at ({:.1}, {:.1}) — icon {}, flags {:#x}",
        poi.name, poi.pos[0], poi.pos[1], poi.icon, poi.flags
    );
    marker.set(poi, map_id, now_secs);
}

/// The guard's directions marker — see the module doc.
pub(crate) struct PoiMarkerPlugin;

impl Plugin for PoiMarkerPlugin {
    fn build(&self, app: &mut App) {
        {
            use crate::net::NetHandlerApp;
            use benilla_protocol::SessionEventKind as K;
            app.net_handler(K::GossipPoi, on_gossip_poi);
        }
        app.init_resource::<PoiMarker>()
            .add_systems(Update, (expire_marker, clear_on_world_entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::GossipPoi;

    /// The `POIIcons.blp` cell every 5875-era `points_of_interest` row ships: `ICON_POI_REDFLAG`,
    /// which vmangos names "red flag w/ yellow !" (`GossipDef.h:113`) — and the real atlas agrees,
    /// measured rather than taken on the enum's word: decoded off the install's own 128² DXT3
    /// `POIIcons.blp`, cell 6's lit pixels mean **RGB (140, 38, 17)**, against the corpse skull's
    /// (98, 70, 67) in cell 7 and the grey AV mine's (115, 113, 115) in cell 0 as controls.
    /// Nothing hardcodes the number — the packet's own `icon` is what draws — so it lives here,
    /// naming what the data actually sends.
    const ICON_POI_REDFLAG: u32 = 6;

    fn wire(name: &str, x: f32, y: f32) -> GossipPoi {
        GossipPoi {
            flags: 99,
            pos: [x, y],
            icon: ICON_POI_REDFLAG,
            data: 0,
            name: name.into(),
        }
    }

    /// The packet becomes the synthetic AreaPOI record the landmark pipeline reads: the wire's
    /// own `flags`/`icon` (the two columns the draw laws consult), `data` as the rim rank key,
    /// z zeroed, the map stamped.
    #[test]
    fn the_wire_becomes_a_landmark_record() {
        let mut m = PoiMarker::default();
        m.set(&wire("Stormwind Warrior Trainer", -8900.0, 600.0), 0, 0.0);
        let poi = m.poi.as_ref().expect("a marker");
        assert_eq!(poi.flags & 0x1, 0x1, "a candidate (Flags bit 0)");
        assert_eq!(
            poi.flags & 0x2,
            0x2,
            "draws the in-range icon (Flags bit 1)"
        );
        assert_eq!(poi.icon, ICON_POI_REDFLAG);
        assert_eq!(poi.pos, [-8900.0, 600.0, 0.0], "the wire carries no z");
        assert_eq!(poi.name, "Stormwind Warrior Trainer");
        assert_eq!(poi.continent_id, 0, "the map it was given on");
    }

    /// The rim rank key is the packet's `data`, verbatim — not a constant of ours (`0x6dac4e`).
    #[test]
    fn the_rank_key_is_the_packets_data_field() {
        let mut w = wire("The Bank", -8900.0, 600.0);
        w.data = 7;
        let mut m = PoiMarker::default();
        m.set(&w, 0, 0.0);
        assert_eq!(m.poi.as_ref().unwrap().importance, 7);
    }

    /// One slot: the next set of directions replaces the last, it never stacks — and restarts the
    /// 8-minute clock.
    #[test]
    fn a_second_set_of_directions_replaces_the_first() {
        let mut m = PoiMarker::default();
        m.set(&wire("The Bank", -8900.0, 600.0), 0, 0.0);
        m.set(&wire("The Inn", -8800.0, 500.0), 0, 100.0);
        let poi = m.poi.as_ref().expect("a marker");
        assert_eq!(poi.name, "The Inn");
        assert_eq!(poi.pos, [-8800.0, 500.0, 0.0]);
        assert_eq!(m.expires_at, 100.0 + MARKER_TTL_SECS);
    }

    /// The deadline is 480 **seconds**, not milliseconds (`0x429580` is a cached `time()`) — long
    /// enough to walk across a capital.
    #[test]
    fn the_deadline_is_eight_minutes_from_when_it_was_given() {
        let mut m = PoiMarker::default();
        m.set(&wire("The Inn", 0.0, 0.0), 0, 12.5);
        assert_eq!(m.expires_at, 492.5);
    }

    /// The marker is drawn only on the map it was given on.
    #[test]
    fn a_marker_is_off_the_map_it_was_not_given_on() {
        let mut m = PoiMarker::default();
        m.set(&wire("The Bank", -8900.0, 600.0), 0, 0.0);
        assert!(m.on_map(0).is_some());
        assert!(m.on_map(1).is_none(), "Kalimdor sees no Stormwind flag");
    }
}
