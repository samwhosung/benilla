//! The battleground **teammate and flag positions** on the world map:
//! `MSG_BATTLEGROUND_PLAYER_POSITIONS` both ways, resolved
//! into the engine's `GetBattlefieldPosition` / `GetBattlefieldFlagPosition` /
//! `GetBattlefieldMapIconScale` backing the stock `WorldMapFrame.lua` and
//! `Blizzard_BattlefieldMinimap.lua` poll every update.
//!
//! **The reference's law, and where each half lives.** The packet carries the teammates outside
//! the requester's own group as raw world floats, plus the friendly flag carrier. The handler
//! (`0x4aad40`) keeps them as sent; the *getter* (`0x4abf90`) does the work per call: it skips
//! the player itself, the four party slots and the raid roster (all of which the map already
//! draws through `GetPlayerMapPosition`), prefers the live object's position when the guid is
//! streamed to us, and normalizes through the world-map projection under the **active queue
//! slot's map** — which is why the getter answers `(0, 0)` off that map. Every one of those is an
//! app-side fact (the group, the entity index, the projection, the catalog), so the app resolves
//! the whole list here each frame and the engine holds only the finished view.
//!
//! **The name** is the name cache's answer (a `CMSG_NAME_QUERY` goes out for a guid it has not
//! seen), `nil` until it lands — and when it lands the reference fires `WORLD_MAP_NAME_UPDATE`,
//! which `WorldMapFrame_OnEvent` turns into a repaint; that is produced here, once per landing.
//!
//! **The request** (`RequestBattlefieldPositions()`) is the reference's own 5000 ms throttle
//! (`0x4ac0f0`, the `[0xb6ec00]` stamp) and goes out only with an active slot — outside a
//! battleground the list is cleared instead, so a stale roster never draws on the next map.

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

/// The reference's request throttle: `RequestBattlefieldPositions` sends at most once per 5000 ms.
pub(crate) const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// The last positions packet and the request stamp.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldPositions {
    /// The last `MSG_BATTLEGROUND_PLAYER_POSITIONS`, as sent; `None` outside a battleground.
    packet: Option<PositionsPacket>,
    /// When the last request went out — the throttle's stamp.
    last_request: Option<Instant>,
    /// Guids whose name the cache has not answered yet; a landing fires `WORLD_MAP_NAME_UPDATE`.
    pending_names: Vec<u64>,
}

impl BattlefieldPositions {
    /// `SessionEvent::BattlefieldPositions` — replaces the list.
    pub(crate) fn apply(&mut self, packet: PositionsPacket) {
        self.packet = Some(packet);
    }

    fn clear(&mut self) {
        self.packet = None;
        self.pending_names.clear();
    }

    /// The throttle: `true` (and the stamp moves) when a request may go out now.
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

/// The getter's position source (`0x4abf90`): the live object's when the guid is streamed to us,
/// else the packet's floats. Both in wow space `(x, y)`.
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

/// The flag texture token the local faction selects: the friendly carrier is carrying the OTHER
/// side's flag, so an Alliance viewer sees `HordeFlag` and a Horde viewer `AllianceFlag`.
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
    // What the empty push last carried (`(has_list, icon_scale bits)`): with no list the engine
    // is told once per change, not per frame; with one, every frame — the positions move.
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

/// The battlefield map's packet handler (in the net handler table since 2313).
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::BattlefieldPositions;
    use crate::net::NetHandlerApp;

    /// Register the handler — called from [`super::BattlefieldPositionsPlugin`].
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
