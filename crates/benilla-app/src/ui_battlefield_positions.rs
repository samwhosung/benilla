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
use benilla_ui::script::{BattlefieldFlagView, BattlefieldPositionView};

use crate::names::NameCache;
use crate::net::{EnteredWorldMessage, GuidIndex, NetCommands, NetEntity, SelfGuid};
use crate::query_cache::Ask;
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_party::GroupState;
use crate::ui_script::UiFeed;
use crate::ui_world_map::{project_on_displayed, WorldMapUiData};

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

    /// The callbacks whose name the cache answered, a record or a refusal, spent: each signals
    /// `WORLD_MAP_NAME_UPDATE` with no status test (`0x4ac190`-`0x4ac195`).
    fn answered_callbacks(&mut self, names: &NameCache) -> usize {
        let before = self.name_callbacks.len();
        self.name_callbacks.retain(|&guid| {
            names.resolve_player(guid, Ask::Read).is_none() && !names.player_answered_unknown(guid)
        });
        before - self.name_callbacks.len()
    }

    /// The positions request's clear with no active slot (`0x4aa5df`): the teammate count
    /// `[0xb6ebdc]` alone, the carrier kept.
    pub(crate) fn clear_teammates(&mut self) {
        if let Some(packet) = self.packet.as_mut() {
            packet.players.clear();
        }
    }

    /// Everything the last packet left, at a world entry.
    fn clear(&mut self) {
        self.packet = None;
    }

    /// The carrier the last packet left, if any.
    #[cfg(test)]
    pub(crate) fn carrier(&self) -> Option<&BattlefieldPosition> {
        self.packet.as_ref()?.carrier.as_ref()
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

pub(crate) fn feed_battlefield_positions(
    // The positions, which the request's call clears and stamps: an engine turn, its events
    // signalled after.
    mut vm: crate::script_calls::VmWith<ResMut<'static, BattlefieldPositions>>,
    queue: Res<BattlefieldQueue>,
    data: Option<Res<WorldMapUiData>>,
    maps: Option<Res<MapCatalogRes>>,
    me: Res<SelfGuid>,
    group: Res<GroupState>,
    guids: Res<GuidIndex>,
    unit_pos: Query<&GlobalTransform, With<NetEntity>>,
    names: Res<NameCache>,
    // The last empty push: with no list the engine is told on change, with one every frame.
    mut last_empty: Local<crate::ui_script::VmMemo<Option<(bool, u32)>>>,
) {
    vm.turn(|script, mut state| {
        let Some(script) = script else {
            return;
        };
        let active = queue.active_map();
        // `GetBattlefieldMapIconScale`: the active slot's `Map.dbc` row, map 0's with no slot.
        let icon_scale = maps
            .as_deref()
            .and_then(|m| m.0.battleground(active.unwrap_or(0)))
            .map_or(1.0, |b| b.minimap_icon_scale);
        // The push only reads: the getter's calls are its name lookups with the callback
        // `0x4ac190`, which ask a nameless row inside the call (`0x4ac118`,
        // [`BattlefieldPositions::wait_for_name`]); the map's `OnUpdate` makes them every frame it
        // shows (`WorldMapFrame.lua:408`, `Blizzard_BattlefieldMinimap.lua:290`). Each callback
        // whose name answered signals once, after the push.
        let answered = state.answered_callbacks(&names);
        let signal = |script: &mut benilla_ui::script::UiScript| {
            for _ in 0..answered {
                script.fire_event("WORLD_MAP_NAME_UPDATE", Vec::new());
            }
        };
        let packet = &state.packet;
        // The getters read the count and the carrier with no slot test (`0x4abf09`, `0x4ac1c6`),
        // under the active slot's map, 0 with none.
        let map = active.unwrap_or(0);
        let Some(packet) = packet.as_ref() else {
            let key = Some((false, icon_scale.to_bits()));
            let last = last_empty.get(script);
            if *last != key {
                *last = key;
                script.set_battlefield_positions(Vec::new(), None, icon_scale);
            }
            signal(script);
            return;
        };
        *last_empty.get(script) = Some((true, icon_scale.to_bits()));

        let selection = script.world_map_selection();
        let project = |x: f32, y: f32| {
            data.as_deref()
                .and_then(|d| project_on_displayed(d, selection, map, x, y))
                .unwrap_or((0.0, 0.0))
        };
        let mut players = Vec::with_capacity(packet.players.len());
        for p in &packet.players {
            if me.0 == Some(p.guid) || group.members.iter().any(|m| m.guid == p.guid) {
                continue;
            }
            let (x, y) = live_or_packet(p, &guids, &unit_pos);
            let name = names.resolve_player(p.guid, Ask::Read).map(str::to_string);
            players.push(BattlefieldPositionView {
                uv: project(x, y),
                name,
                guid: p.guid,
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
        signal(script);
    })
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
                // Before the pass: the positions request's call reads and stamps the state.
                reset_on_world_enter
                    .before(feed_battlefield_positions)
                    .before(crate::vm_pass::VmPass::Start),
                feed_battlefield_positions.in_set(UiFeed),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

    /// The push only reads a teammate's name: the packet and the frames ask nothing, the getter's
    /// own lookup asking a nameless row inside its call (`0x4ac118`,
    /// [`benilla_ui::script::RecordAsk::PlayerName`]).
    #[test]
    fn the_push_reads_a_position_name_and_asks_nothing() {
        use benilla_protocol::messages::BattlefieldStatus;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut queue = BattlefieldQueue::default();
        queue.apply(BattlefieldStatus {
            slot: 0,
            map_id: 489,
            bracket: 0,
            instance_id: 3,
            status: 3,
            time_ms: None,
            in_progress: Some((0, 0)),
            queued: None,
        });
        app.insert_resource(NetCommands(tx))
            .insert_resource(queue)
            .init_resource::<BattlefieldPositions>()
            .init_resource::<SelfGuid>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .add_systems(Update, feed_battlefield_positions);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let packet = |app: &mut App| {
            app.world_mut()
                .resource_mut::<BattlefieldPositions>()
                .apply(PositionsPacket {
                    players: vec![BattlefieldPosition {
                        guid: 0x9,
                        x: 0.0,
                        y: 0.0,
                    }],
                    carrier: None,
                });
            app.update();
        };
        packet(&mut app);
        for _ in 0..3 {
            app.update();
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "the packet and the frames only read"
        );
    }

    /// With no active slot, as after leaving, the feed still pushes the last packet's teammates
    /// and carrier: the getters read the count and the carrier with no slot test (`0x4abf09`,
    /// `0x4ac1c6`).
    #[test]
    fn with_no_active_slot_the_last_packet_still_reads() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<BattlefieldQueue>()
            .init_resource::<BattlefieldPositions>()
            .init_resource::<SelfGuid>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .add_systems(Update, feed_battlefield_positions);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let at = |guid| BattlefieldPosition {
            guid,
            x: 0.0,
            y: 0.0,
        };
        app.world_mut()
            .resource_mut::<BattlefieldPositions>()
            .apply(PositionsPacket {
                players: vec![at(0x9)],
                carrier: Some(at(0xA)),
            });
        app.update();
        let counts: (i64, i64) = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return GetNumBattlefieldPositions(), GetNumBattlefieldFlagPositions()")
            .unwrap();
        assert_eq!(counts, (1, 1));
    }

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
