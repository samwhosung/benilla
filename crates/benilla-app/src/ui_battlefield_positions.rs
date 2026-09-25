//! Battleground teammate and flag positions on the world map: `MSG_BATTLEGROUND_PLAYER_POSITIONS`
//! resolved into the view behind `GetBattlefieldPosition`, `GetBattlefieldFlagPosition` and
//! `GetBattlefieldMapIconScale`, which `WorldMapFrame.lua` and the battlefield minimap poll.
//!
//! The reference's getter (`0x4abf90`) does the work per call: it skips the player, the party and
//! the raid roster, prefers a streamed object's live position, and projects under the active queue
//! slot's map, answering `(0, 0)` off it. The app resolves the list that way each frame. A name
//! landing fires `WORLD_MAP_NAME_UPDATE`, as in the reference.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_assets::MapCatalogRes;
use benilla_protocol::messages::{BattlefieldPosition, BattlefieldPositions as PositionsPacket};
use benilla_ui::script::{BattlefieldFlagView, BattlefieldPositionView, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, EnteredWorldMessage, GuidIndex, NetCommands, NetEntity, SelfGuid};
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_party::GroupState;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_world_map::{project_on_displayed, WorldMapUiData};

/// `RequestBattlefieldPositions` (`0x4aa5c0`) sends at most once per 5000 ms (stamp
/// `[0xb6ebe0]`), and only with an active queue slot; without one it clears the list.
pub(crate) const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// The last positions packet and the request stamp.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldPositions {
    packet: Option<PositionsPacket>,
    last_request: Option<Instant>,
    /// Guids whose name the cache has not answered yet.
    pending_names: Vec<u64>,
}

impl BattlefieldPositions {
    /// `MSG_BATTLEGROUND_PLAYER_POSITIONS` in: kept as sent, as the handler (`0x4aad40`) does.
    pub(crate) fn apply(&mut self, packet: PositionsPacket) {
        self.packet = Some(packet);
    }

    fn clear(&mut self) {
        self.packet = None;
        self.pending_names.clear();
    }

    /// The throttle: `true`, and the stamp moves, when a request may go out now.
    fn request_due(&mut self, now: Instant) -> bool {
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
fn live_or_packet(
    p: &BattlefieldPosition,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
) -> (f32, f32) {
    match guids.0.get(&p.guid).and_then(|e| unit_pos.get(*e).ok()) {
        Some(tf) => {
            let w = bevy_to_wow(tf.translation());
            (w[0], w[1])
        }
        None => (p.x, p.y),
    }
}

/// The flag texture token: the friendly carrier holds the other side's flag.
fn flag_token(faction: Option<&str>) -> Option<&'static str> {
    match faction {
        Some("Alliance") => Some("HordeFlag"),
        Some("Horde") => Some("AllianceFlag"),
        _ => None,
    }
}

fn feed_battlefield_positions(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<BattlefieldPositions>,
    queue: Res<BattlefieldQueue>,
    data: Option<Res<WorldMapUiData>>,
    maps: Option<Res<MapCatalogRes>>,
    me: Res<SelfGuid>,
    group: Res<GroupState>,
    guids: Res<GuidIndex>,
    unit_pos: Query<&GlobalTransform, With<NetEntity>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    // The last empty push: with no list the engine is told on change, with one every frame.
    mut last_empty: Local<crate::ui_script::VmMemo<Option<(bool, u32)>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let active = queue.active_map();
    // `GetBattlefieldMapIconScale`: the active slot's `Map.dbc` row, map 0's with no slot.
    let icon_scale = maps
        .as_deref()
        .and_then(|m| m.0.battleground(active.unwrap_or(0)))
        .map_or(1.0, |b| b.minimap_icon_scale);
    let BattlefieldPositions {
        packet,
        pending_names,
        ..
    } = &mut *state;
    let (Some(map), Some(packet)) = (active, packet.as_ref()) else {
        let key = Some((false, icon_scale.to_bits()));
        let last = last_empty.get(&script);
        if *last != key {
            *last = key;
            script.set_battlefield_positions(Vec::new(), None, icon_scale);
        }
        return;
    };
    *last_empty.get(&script) = Some((true, icon_scale.to_bits()));

    let selection = script.world_map_selection();
    let project = |x: f32, y: f32| {
        data.as_deref()
            .and_then(|d| project_on_displayed(d, selection, map, x, y))
            .unwrap_or((0.0, 0.0))
    };
    let mut name_landed = false;
    let mut players = Vec::with_capacity(packet.players.len());
    for p in &packet.players {
        if me.0 == Some(p.guid) || group.members.iter().any(|m| m.guid == p.guid) {
            continue;
        }
        let (x, y) = live_or_packet(p, &guids, &unit_pos);
        let name = names.resolve(p.guid, &commands).map(str::to_string);
        match name {
            None => {
                if !pending_names.contains(&p.guid) {
                    pending_names.push(p.guid);
                }
            }
            Some(_) => {
                if let Some(i) = pending_names.iter().position(|g| *g == p.guid) {
                    pending_names.swap_remove(i);
                    name_landed = true;
                }
            }
        }
        players.push(BattlefieldPositionView {
            uv: project(x, y),
            name,
        });
    }
    let flag = packet.carrier.as_ref().map(|c| {
        let (x, y) = live_or_packet(c, &guids, &unit_pos);
        let faction = script
            .eval::<Option<String>>(r#"return (UnitFactionGroup("player"))"#)
            .ok()
            .flatten();
        BattlefieldFlagView {
            uv: project(x, y),
            token: flag_token(faction.as_deref()).map(str::to_string),
        }
    });
    script.set_battlefield_positions(players, flag, icon_scale);
    if name_landed {
        script.fire_event("WORLD_MAP_NAME_UPDATE", Vec::new());
    }
}

fn drain_battlefield_positions(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<BattlefieldPositions>,
    queue: Res<BattlefieldQueue>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_battlefield_position_requests() == 0 {
        return;
    }
    if queue.active_map().is_none() {
        state.clear();
        return;
    }
    if state.request_due(Instant::now()) {
        let _ = commands.0.send(ClientCommand::RequestBattlefieldPositions);
    }
}

fn reset_on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    mut state: ResMut<BattlefieldPositions>,
) {
    if entered.read().next().is_none() {
        return;
    }
    state.clear();
    state.last_request = None;
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
        app.init_resource::<BattlefieldPositions>().add_systems(
            Update,
            (
                reset_on_world_enter.before(feed_battlefield_positions),
                feed_battlefield_positions.in_set(UiFeed),
                drain_battlefield_positions.after(UiInput),
            ),
        );
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
        assert_eq!(flag_token(Some("Alliance")), Some("HordeFlag"));
        assert_eq!(flag_token(Some("Horde")), Some("AllianceFlag"));
        assert_eq!(flag_token(None), None);
    }
}
