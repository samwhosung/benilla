//! Battleground teammate and flag positions on the world map: `MSG_BATTLEGROUND_PLAYER_POSITIONS`
//! kept as sent, and the view behind `GetBattlefieldPosition`, `GetBattlefieldFlagPosition` and
//! `GetBattlefieldMapIconScale`, which `WorldMapFrame.lua` and the battlefield minimap poll.
//!
//! The reference's getter (`0x4abf90`) does the work per call: it skips the player, the party and
//! the raid roster, prefers a streamed object's live position, and projects under the active queue
//! slot's map, answering `(0, 0)` off it. The host does the same as each getter asks
//! ([`crate::ui_battlefield::BattlefieldLive`]). A name a getter's call missed fires
//! `WORLD_MAP_NAME_UPDATE` at its answer, as in the reference.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::messages::{BattlefieldPosition, BattlefieldPositions as PositionsPacket};

use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, NetEntity};
use crate::query_cache::Ask;

/// `RequestBattlefieldPositions` (`0x4aa5c0`) sends at most once per 5000 ms (stamp
/// `[0xb6ebe0]`), and only with an active queue slot; without one it clears the list.
pub(crate) const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// The last positions packet and the request stamp.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldPositions {
    packet: Option<PositionsPacket>,
    last_request: Option<Instant>,
    /// The callbacks `0x4ac190` the getter's calls left, one per call that missed, each the
    /// guid whose name it waits on; the name cache keeps them, so no list clear drops them.
    name_callbacks: Vec<u64>,
}

impl BattlefieldPositions {
    /// `MSG_BATTLEGROUND_PLAYER_POSITIONS` in: kept as sent, as the handler (`0x4aad40`) does.
    pub(crate) fn apply(&mut self, packet: PositionsPacket) {
        self.packet = Some(packet);
    }

    /// `GetBattlefieldPosition`'s lookup of a nameless teammate, inside its call
    /// ([`benilla_ui::script::RecordAsk::BattlefieldName`]): the cache asks a miss, a refused name
    /// again, and the call leaves its callback, with no dedupe (`0x4ac104`-`0x4ac118`).
    pub(crate) fn wait_for_name(&mut self, names: &NameCache, commands: &NetCommands, guid: u64) {
        if names.resolve_player(guid, Ask::Event(commands)).is_none() {
            self.name_callbacks.push(guid);
        }
    }

    /// The callbacks waiting on `guid`'s name, spent by its answer, a record or a refusal: each
    /// signals `WORLD_MAP_NAME_UPDATE` with no status test (`0x4ac190`-`0x4ac195`).
    pub(crate) fn name_answered(&mut self, guid: u64) -> usize {
        let before = self.name_callbacks.len();
        self.name_callbacks.retain(|&g| g != guid);
        before - self.name_callbacks.len()
    }

    /// The positions request's clear with no active slot (`0x4aa5df`), and a zero-map status's
    /// (`0x4aaae6`): the teammate count `[0xb6ebdc]` alone, the carrier kept.
    pub(crate) fn clear_teammates(&mut self) {
        if let Some(packet) = self.packet.as_mut() {
            packet.players.clear();
        }
    }

    /// The world-enter cascade's clear (`0x4a9db0`): everything the last packet left, the teammate
    /// count among it (`0x4a9e6d`), and the request stamp (`0x4a9e93`).
    pub(crate) fn world_enter(&mut self) {
        self.packet = None;
        self.last_request = None;
    }

    /// The carrier the last packet left, if any.
    pub(crate) fn carrier(&self) -> Option<&BattlefieldPosition> {
        self.packet.as_ref()?.carrier.as_ref()
    }

    /// The stored teammates, in wire order.
    pub(crate) fn teammates(&self) -> &[BattlefieldPosition] {
        self.packet.as_ref().map_or(&[], |p| &p.players)
    }

    /// The throttle: `true`, and the stamp moves, when a request may go out now.
    pub(crate) fn request_due(&mut self, now: Instant) -> bool {
        let due = self
            .last_request
            .is_none_or(|t| now.saturating_duration_since(t) >= REQUEST_THROTTLE);
        if due {
            self.last_request = Some(now);
        }
        due
    }
}

/// The getter's position source (`0x4abf90`): a streamed object's live position, else the
/// packet's, as wow `(x, y)`.
pub(crate) fn live_or_packet(
    p: &BattlefieldPosition,
    guids: Option<&GuidIndex>,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
) -> (f32, f32) {
    match guids
        .and_then(|g| g.0.get(&p.guid))
        .and_then(|e| unit_pos.get(*e).ok())
    {
        Some(tf) => {
            let w = bevy_to_wow(tf.translation());
            (w[0], w[1])
        }
        None => (p.x, p.y),
    }
}

/// The flag texture token by the player's side (`0x4aa670`, `0x5efe00`): the friendly carrier
/// holds the other side's flag; no side, no token.
pub(crate) fn flag_token(side: i8) -> Option<&'static str> {
    match side {
        1 => Some("HordeFlag"),
        0 => Some("AllianceFlag"),
        _ => None,
    }
}

/// The battlefield map's packet handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::BattlefieldPositions;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::BattlefieldPositions, on_positions);
    }

    fn on_positions(In(ev): In<SessionEvent>, mut positions: ResMut<BattlefieldPositions>) {
        if let SessionEvent::BattlefieldPositions(packet) = ev {
            positions.apply(packet);
        }
    }
}

pub(crate) struct BattlefieldPositionsPlugin;

impl Plugin for BattlefieldPositionsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // The world entry clears it in the cascade's battlefield init
        // ([`crate::ui_battlefield::reset_on_world_enter`]).
        app.init_resource::<BattlefieldPositions>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_throttle_is_the_references_five_seconds() {
        let mut s = BattlefieldPositions::default();
        let t0 = Instant::now();
        assert!(s.request_due(t0), "the first request goes out");
        assert!(!s.request_due(t0 + Duration::from_millis(4999)));
        assert!(s.request_due(t0 + REQUEST_THROTTLE));
        assert!(!s.request_due(t0 + REQUEST_THROTTLE + Duration::from_millis(1)));
    }

    #[test]
    fn the_flag_token_is_the_other_sides() {
        assert_eq!(flag_token(1), Some("HordeFlag"), "an Alliance player's");
        assert_eq!(flag_token(0), Some("AllianceFlag"), "a Horde player's");
        assert_eq!(flag_token(-1), None);
    }
}
