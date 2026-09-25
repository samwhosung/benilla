//! The guard's directions: the marker `SMSG_GOSSIP_POI` drops on the minimap and world map.
//!
//! The server volunteers `{flags, x, y, icon, data, name}` for a gossip option with an
//! `action_poi_id` (vmangos `PlayerMenu::SendPointOfInterest`, `GossipDef.cpp:253`); every
//! 5875-era row ships `icon = 6` (`ICON_POI_REDFLAG`) and `flags = 99`.
//!
//! The reference draws it as a landmark: the handler (`0x4e2840`) builds a synthetic `AreaPOI`
//! record in static slot 1 at `0xcea7d4` (`set_blip` `0x6dac10`), which the minimap's candidate
//! list appends unconditionally (`0x6d8fa8`), past the DBC scan's `ContinentID`/`Flags & 1` gate.
//! Every landmark rule then applies, including the 694.444 yd rank cut (only the corpse slot
//! `0xcea848` is exempt). This module holds the record and its lifetime; the drawing is
//! [`crate::minimap::blips`] and the world map's POI pool.
//!
//! The marker clears on whichever comes first:
//! - 480 seconds after `set_blip` (`0x429580` is a cached `time()`, in seconds);
//! - arriving strictly inside 10 yd (`minimap_update` `0x6d93a0`, `(player - marker)^2 < 100`);
//! - the next set of directions, which overwrites the one slot;
//! - world entry (`zone_rebuild`), here a worldport or a logout.
//!
//! The wire has no map; the record takes the current map, as the reference does at `set_blip`
//! (`+0x1c ContinentID <- [0x86f694]`).

use benilla_formats::AreaPoi;
use bevy::prelude::*;
use bevy::time::Real;

use benilla_assets::coords::bevy_to_wow;

use crate::net::{LoggedOutMessage, WorldportMessage};
use crate::player::Player;

/// The arrival radius squared, `0x806b10` = 100 yd^2, compared strictly.
const ARRIVE_CLEAR_YD_SQ: f32 = 100.0;
/// The marker's lifetime, `time() + 480` seconds (`set_blip` `0x6dac10`).
const MARKER_TTL_SECS: f64 = 480.0;

/// The live directions as the reference's synthetic `AreaPOI` record, read by
/// [`crate::minimap::blips`] and [`crate::ui_world_map`].
#[derive(Resource, Default)]
pub(crate) struct PoiMarker {
    /// Its z is 0: the wire carries x/y only, and the reference leaves `+0x18 Z` unwritten.
    pub(crate) poi: Option<AreaPoi>,
    /// `Time<Real>` seconds at which the marker expires.
    expires_at: f64,
}

impl PoiMarker {
    /// Replaces the live marker and starts its 8-minute clock; `map_id` is the player's map.
    pub(crate) fn set(
        &mut self,
        wire: &benilla_protocol::messages::GossipPoi,
        map_id: u32,
        now_secs: f64,
    ) {
        self.expires_at = now_secs + MARKER_TTL_SECS;
        self.poi = Some(AreaPoi {
            // The rank key is the packet's `data` (`0x6dac4e` writes `+0x04 Importance`); shipped
            // rows send 0, the first rank band.
            importance: wire.data,
            icon: wire.icon,
            faction_id: 0,
            pos: [wire.pos[0], wire.pos[1], 0.0],
            continent_id: map_id,
            flags: wire.flags,
            area_id: 0,
            name: wire.name.clone(),
            // Never written, so `GetMapLandmarkInfo` returns a nil description.
            description: String::new(),
            world_state_id: 0,
        });
    }

    /// The marker, if it is on `map_id`.
    pub(crate) fn on_map(&self, map_id: u32) -> Option<&AreaPoi> {
        self.poi.as_ref().filter(|p| p.continent_id == map_id)
    }
}

/// Clears the marker on arrival or at its deadline, once a frame as the minimap driver does.
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

/// World entry clears the slot (`zone_rebuild`): a worldport or a logout.
fn clear_on_world_entry(
    mut marker: ResMut<PoiMarker>,
    mut worldports: MessageReader<WorldportMessage>,
    mut logged_out: MessageReader<LoggedOutMessage>,
) {
    if worldports.read().next().is_some() || logged_out.read().next().is_some() {
        marker.poi = None;
    }
}

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

/// `SMSG_GOSSIP_POI`: drops the marker. It does not end the gossip session: vmangos
/// `Player::OnGossipSelect` sends it before deciding what the menu does next.
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

    /// The `POIIcons.blp` cell every 5875-era row ships, the red flag (`GossipDef.h:113`).
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

    #[test]
    fn the_rank_key_is_the_packets_data_field() {
        let mut w = wire("The Bank", -8900.0, 600.0);
        w.data = 7;
        let mut m = PoiMarker::default();
        m.set(&w, 0, 0.0);
        assert_eq!(m.poi.as_ref().unwrap().importance, 7);
    }

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

    #[test]
    fn the_deadline_is_eight_minutes_from_when_it_was_given() {
        let mut m = PoiMarker::default();
        m.set(&wire("The Inn", 0.0, 0.0), 0, 12.5);
        assert_eq!(m.expires_at, 492.5);
    }

    #[test]
    fn a_marker_is_off_the_map_it_was_not_given_on() {
        let mut m = PoiMarker::default();
        m.set(&wire("The Bank", -8900.0, 600.0), 0, 0.0);
        assert!(m.on_map(0).is_some());
        assert!(m.on_map(1).is_none(), "Kalimdor sees no Stormwind flag");
    }
}
