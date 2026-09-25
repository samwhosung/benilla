//! The battleground list window's app half: what the stock `BattlefieldFrame.lua` and the
//! minimap's queue icon need from the wire, the clock and Map.dbc. The queue slots live in
//! [`BattlefieldQueue`]; the scoreboard is [`crate::ui_battlefield_score`]'s.
//!
//! A list (`SMSG_BATTLEFIELD_LIST`, `0x4aa6c0`) fires `BATTLEFIELDS_SHOW` and anchors the leash
//! (`0x4aa090`), which fires `BATTLEFIELDS_CLOSED`, the window's only engine-side close, once the
//! player is more than 5.5556 yd from the anchor.

use std::time::Instant;

use bevy::prelude::*;

use benilla_assets::MapCatalogRes;
use benilla_formats::MapCatalog;
use benilla_protocol::messages::{BattlefieldList, BattlefieldStatus};
use benilla_ui::script::{BattlefieldListView, BattlefieldMapInfo, BattlefieldQueueSlot, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands};
use crate::player::Player;
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_party::GroupState;
use crate::ui_script::{UiFeed, UiInput};

/// The leash radius, the `.rdata` f32 at `0x806574`, which the reference squares at startup.
const LEASH_RADIUS_YD: f32 = 5.555_555_3;

/// An anchor this close to the origin means no list is open (`fcomp [0x8029d4]`).
const DEGENERATE_ANCHOR: f32 = 2.384_185_8e-7;

/// `SMSG_GROUP_JOINED_BATTLEGROUND`'s deserters sentinel (`cmp eax,-2` in `0x4aacc0`).
const GROUP_JOIN_DESERTERS: u32 = 0xFFFF_FFFE;

/// The list window's state: the last list, the leash anchor, and what the handlers still owe
/// the next feed.
#[derive(Resource, Default)]
pub(crate) struct Battlefield {
    list: Option<BattlefieldList>,
    show: bool,
    dirty: bool,
    /// Where the list was opened (`[0xb6e870..78]`); `None` once the leash fired.
    anchor: Option<Vec3>,
    verdicts: Vec<u32>,
    /// Joined (`true`) or left guids awaiting a name.
    players: Vec<(u64, bool)>,
}

impl Battlefield {
    pub(crate) fn apply_list(&mut self, list: BattlefieldList) {
        self.list = Some(list);
        self.show = true;
        self.dirty = true;
    }

    /// `SMSG_GROUP_JOINED_BATTLEGROUND` (`0x4aacc0`): one line, no state.
    pub(crate) fn apply_verdict(&mut self, result: u32) {
        self.verdicts.push(result);
    }

    /// `SMSG_BATTLEGROUND_PLAYER_JOINED`/`_LEFT` (`0x4aae10`): one line once the name resolves.
    pub(crate) fn apply_player(&mut self, guid: u64, joined: bool) {
        self.players.push((guid, joined));
    }

    fn clear_session(&mut self) {
        self.list = None;
        self.show = false;
        self.dirty = true;
        self.anchor = None;
        self.verdicts.clear();
        self.players.clear();
    }

    /// The last list's battlemaster. A character under the bracket floor gets no list at all
    /// (vmangos `BattleGroundHandler.cpp:59`).
    pub(crate) fn battlemaster(&self) -> Option<u64> {
        self.list.as_ref().map(|l| l.battlemaster)
    }

    /// The listed map (`[0xb6eba4]`); with nothing listed, 0, itself a real Map.dbc row.
    fn map_id(&self) -> u32 {
        self.list.as_ref().map_or(0, |l| l.map_id)
    }
}

/// The VM's view of the list, every value off `[0xb6eba4]`'s map row, whatever that id is.
fn list_view(
    state: &Battlefield,
    catalog: Option<&MapCatalog>,
    faction: Option<&str>,
) -> BattlefieldListView {
    let map_id = state.map_id();
    let row = catalog.and_then(|c| c.battleground(map_id));
    let (bracket_min, bracket_max) = state
        .list
        .as_ref()
        .zip(row)
        .map_or((0, 0), |(l, r)| r.bracket_levels(l.bracket));
    let info = catalog
        .and_then(|c| c.name(map_id))
        .zip(row)
        .map(|(name, r)| BattlefieldMapInfo {
            name: name.to_string(),
            // The faction-group index (`0x5efe00`): 0 Horde, 1 Alliance; both columns ship
            // the same text.
            description: match faction {
                Some("Horde") => Some(r.descriptions[0].clone()),
                Some("Alliance") => Some(r.descriptions[1].clone()),
                _ => None,
            },
            min_level: r.min_level,
            max_level: r.max_level,
            field_16: r.field_16,
            field_17: r.field_17,
            field_18: r.field_18,
        });
    BattlefieldListView {
        instances: state
            .list
            .as_ref()
            .map_or_else(Vec::new, |l| l.instances.clone()),
        bracket_min,
        bracket_max,
        info,
        group_queue: row.is_some_and(|r| r.group_queue != 0),
    }
}

/// One slot's VM view at `now`: the three clock getters reduced (`0x4ab620`, `0x4ab790`,
/// `0x4ab820`) and the bracket levels derived (`0x4aa850`).
fn slot_view(
    slot: Option<&(BattlefieldStatus, Instant)>,
    catalog: Option<&MapCatalog>,
    now: Instant,
) -> BattlefieldQueueSlot {
    let ms = |d: std::time::Duration| d.as_millis().min(u128::from(u32::MAX)) as u32;
    let map_id = slot.map_or(0, |(s, _)| s.map_id);
    let mut view = BattlefieldQueueSlot {
        map_id,
        map_name: catalog.and_then(|c| c.name(map_id)).map(str::to_string),
        ..Default::default()
    };
    let Some((status, at)) = slot else {
        return view;
    };
    view.status = status.status;
    view.instance_id = status.instance_id;
    if let Some(row) = catalog.and_then(|c| c.battleground(map_id)) {
        (view.min_level, view.max_level) = row.bracket_levels(status.bracket);
    }
    // Status 2: `[slot+0x14] = Δ ? now + Δ : 0`; read as `deadline − now`, 0 once past.
    if let Some(delta) = status.time_ms.filter(|&d| d != 0) {
        let deadline = *at + std::time::Duration::from_millis(u64::from(delta));
        view.port_expiration_ms = ms(deadline.saturating_duration_since(now));
    }
    // Status 1: the raw estimate, and `[slot+0x1c] = now − Δ` read back as `now − stamp`.
    if let Some((estimate, waited)) = status.queued {
        view.estimated_wait_ms = estimate;
        let stamp = *at - std::time::Duration::from_millis(u64::from(waited));
        view.time_waited_ms = ms(now.saturating_duration_since(stamp));
    }
    view
}

/// The as-group refusal (`0x4a9f60`): the map's `MaxPlayers` must cover the party count (the
/// members besides us, 0..4) and the raid roster count. The raid count here is the wire list in a
/// raid and 0 in a party; which members the reference's raid roster counts is untraced.
fn group_fits(catalog: Option<&MapCatalog>, map_id: u32, group: Option<&GroupState>) -> bool {
    let max = catalog
        .and_then(|c| c.battleground(map_id))
        .map_or(0, |r| r.max_players) as usize;
    let Some(group) = group else {
        return true;
    };
    let party = group.party_slots().count();
    let raid = if group.group_type == 1 {
        group.members.len()
    } else {
        0
    };
    max >= party && max >= raid
}

/// Runs before the dialog and score feeds fire `UPDATE_BATTLEFIELD_STATUS` and
/// `UPDATE_BATTLEFIELD_SCORE`, so their handlers read this frame's list and slots.
fn feed_battlefield(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<Battlefield>,
    queue: Res<BattlefieldQueue>,
    maps: Option<Res<MapCatalogRes>>,
    player: Res<Player>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let catalog = maps.as_deref().map(|m| &m.0);
    let now = Instant::now();

    if std::mem::take(&mut state.dirty) {
        // The description's side: the player's faction group (`0x5efe00`).
        let faction = script
            .eval::<Option<String>>(r#"return (UnitFactionGroup("player"))"#)
            .ok()
            .flatten();
        let view = list_view(&state, catalog, faction.as_deref());
        script.set_battlefield_list(view);
    }

    let slots = queue
        .slots()
        .iter()
        .map(|s| slot_view(s.as_ref(), catalog, now))
        .collect();
    script.set_battlefield_queue(slots, queue.instance_expiration_ms(now));

    if std::mem::take(&mut state.show) {
        script.fire_event("BATTLEFIELDS_SHOW", vec![]);
        // The anchor is written after the event, from the live position (`0x4aa6c0`).
        state.anchor = Some(player.pos);
    }

    // The leash (`0x4aa090`): strictly beyond the radius, the anchor clears, then the event.
    if let Some(anchor) = state.anchor {
        if anchor.length_squared() > DEGENERATE_ANCHOR
            && player.pos.distance_squared(anchor) > LEASH_RADIUS_YD * LEASH_RADIUS_YD
        {
            state.anchor = None;
            script.fire_event("BATTLEFIELDS_CLOSED", vec![]);
        }
    }

    let mut lines = Vec::new();
    for result in std::mem::take(&mut state.verdicts) {
        let line = if result == GROUP_JOIN_DESERTERS {
            crate::ui_action::keyed_line(&script, "ERR_GROUP_JOIN_BATTLEGROUND_DESERTERS")
        } else if let Some(name) = catalog.and_then(|c| c.name(result)) {
            crate::ui_action::keyed_line_s(&script, "ERR_GROUP_JOIN_BATTLEGROUND_S", &[name])
        } else {
            crate::ui_action::keyed_line(&script, "ERR_GROUP_JOIN_BATTLEGROUND_FAIL")
        };
        lines.extend(line);
    }
    let pending = std::mem::take(&mut state.players);
    for (guid, joined) in pending {
        let Some(name) = names.resolve(guid, &commands).map(str::to_string) else {
            state.players.push((guid, joined));
            continue;
        };
        let line = if joined {
            crate::ui_action::keyed_line_s(&script, "ERR_BG_PLAYER_JOINED_SS", &[&name, &name])
        } else {
            crate::ui_action::keyed_line_s(&script, "ERR_BG_PLAYER_LEFT_S", &[&name])
        };
        lines.extend(line);
    }
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_battlefield", lines);
    }
}

/// `JoinBattlefield`'s sends, `CMSG_BATTLEMASTER_JOIN` with a battlemaster and
/// `CMSG_BATTLEFIELD_JOIN` without (`0x4a9f60`), and `ShowBattlefieldList`'s.
fn drain_battlefield(
    script: Option<NonSendMut<UiScript>>,
    state: Res<Battlefield>,
    maps: Option<Res<MapCatalogRes>>,
    group: Option<Res<GroupState>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let catalog = maps.as_deref().map(|m| &m.0);
    let map_id = state.map_id();
    for (instance_id, as_group) in script.take_battlefield_join_requests() {
        if as_group && !group_fits(catalog, map_id, group.as_deref()) {
            if let Some(line) =
                crate::ui_action::keyed_line(&script, "ERR_GROUP_JOIN_BATTLEGROUND_TOO_MANY")
            {
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_battlefield", [line]);
            }
            continue;
        }
        let battlemaster = state.list.as_ref().map_or(0, |l| l.battlemaster);
        let command = if battlemaster != 0 {
            ClientCommand::BattlemasterJoin {
                battlemaster,
                map_id,
                instance_id,
                as_group,
            }
        } else {
            ClientCommand::BattlefieldJoin {
                map_id,
                instance_id,
                as_group,
            }
        };
        let _ = commands.0.send(command);
    }
    for map_id in script.take_battlefield_list_requests() {
        let _ = commands.0.send(ClientCommand::BattlefieldList { map_id });
    }
}

/// World enter (`0x4a9db0`): the list, the selection and the anchor clear, the queue slots stay,
/// and the bodyless `CMSG_BATTLEFIELD_STATUS` goes out, answered slot by slot.
fn reset_on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    mut state: ResMut<Battlefield>,
    script: Option<NonSendMut<UiScript>>,
    commands: Res<NetCommands>,
) {
    if entered.read().next().is_none() {
        return;
    }
    state.clear_session();
    if let Some(mut script) = script {
        script.reset_battlefield_selection();
    }
    let _ = commands.0.send(ClientCommand::BattlefieldStatusRequest);
}

/// The battlemaster window's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::Battlefield;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldList, on_packet)
            .net_handler(K::GroupJoinedBattleground, on_packet)
            .net_handler(K::BattlegroundPlayer, on_packet);
    }

    fn on_packet(In(ev): In<SessionEvent>, mut battlefield: ResMut<Battlefield>) {
        match ev {
            SessionEvent::BattlefieldList(list) => battlefield.apply_list(list),
            SessionEvent::GroupJoinedBattleground { result } => battlefield.apply_verdict(result),
            SessionEvent::BattlegroundPlayer { guid, joined } => {
                battlefield.apply_player(guid, joined)
            }
            _ => {}
        }
    }
}

pub(crate) struct BattlefieldPlugin;

impl Plugin for BattlefieldPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<Battlefield>().add_systems(
            Update,
            (
                reset_on_world_enter
                    .in_set(crate::ui_script::UiFeed)
                    .before(feed_battlefield),
                feed_battlefield
                    .before(crate::ui_battlefield_score::feed_battlefield_score)
                    .before(crate::ui_dialog_verbs::feed_dialog_verbs)
                    .in_set(UiFeed),
                drain_battlefield.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(slot: u32, map_id: u32, status: u32) -> BattlefieldStatus {
        BattlefieldStatus {
            slot,
            map_id,
            bracket: 1,
            instance_id: 7,
            status,
            time_ms: None,
            in_progress: None,
            queued: None,
        }
    }

    #[test]
    fn the_slot_view_reduces_the_clocks() {
        let at = Instant::now();
        let mut queued = status(0, 489, 1);
        queued.queued = Some((30_000, 5_000));
        let later = at + std::time::Duration::from_millis(2_000);
        let v = slot_view(Some(&(queued, at)), None, later);
        assert_eq!((v.map_id, v.status, v.instance_id), (489, 1, 7));
        assert_eq!(v.estimated_wait_ms, 30_000, "raw, no clock");
        assert_eq!(v.time_waited_ms, 7_000, "the wire's 5 s plus the 2 s since");
        assert_eq!(v.port_expiration_ms, 0);
        assert_eq!((v.min_level, v.max_level), (0, 0), "no catalog: no bracket");
        assert!(v.map_name.is_none());

        let mut confirm = status(1, 529, 2);
        confirm.time_ms = Some(60_000);
        let v = slot_view(Some(&(confirm.clone(), at)), None, later);
        assert_eq!(v.port_expiration_ms, 58_000);
        let past = at + std::time::Duration::from_millis(61_000);
        let v = slot_view(Some(&(confirm, at)), None, past);
        assert_eq!(
            v.port_expiration_ms, 0,
            "a past deadline reads 0, never negative"
        );

        let v = slot_view(None, None, later);
        assert_eq!(
            (v.map_id, v.status),
            (0, 0),
            "an empty slot: map 0, status none"
        );
    }

    #[test]
    fn the_group_gate_reads_both_counts() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_map_catalog(&mut chain).expect("Map.dbc");
        assert!(group_fits(Some(&catalog), 489, None));
        let member = |guid: u64| benilla_protocol::messages::GroupMemberEntry {
            name: format!("m{guid}"),
            guid,
            status: 1,
            flags: 0,
        };
        let mut group = GroupState {
            in_group: true,
            ..Default::default()
        };
        group.members = (1..=4).map(member).collect();
        assert!(
            group_fits(Some(&catalog), 489, Some(&group)),
            "a full party fits WSG's 10"
        );
        group.group_type = 1;
        group.members = (1..=12).map(member).collect();
        assert!(
            !group_fits(Some(&catalog), 489, Some(&group)),
            "a 12-member raid outruns WSG's 10"
        );
        assert!(
            group_fits(Some(&catalog), 30, Some(&group)),
            "and fits AV's 40"
        );
    }

    #[test]
    fn the_list_view_resolves_the_listed_map_or_map_zero() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_map_catalog(&mut chain).expect("Map.dbc");
        let mut state = Battlefield::default();
        let v = list_view(&state, Some(&catalog), Some("Horde"));
        assert!(v.instances.is_empty());
        assert_eq!(
            v.info.as_ref().map(|i| i.name.as_str()),
            Some("Eastern Kingdoms")
        );
        assert_eq!((v.bracket_min, v.bracket_max), (0, 0));
        state.apply_list(BattlefieldList {
            battlemaster: 0x10,
            map_id: 529,
            bracket: 2,
            instances: vec![4, 9],
        });
        let v = list_view(&state, Some(&catalog), Some("Alliance"));
        assert_eq!(v.instances, vec![4, 9]);
        assert_eq!((v.bracket_min, v.bracket_max), (40, 49));
        let info = v.info.expect("Arathi Basin's row");
        assert_eq!(info.name, "Arathi Basin");
        assert!(info
            .description
            .as_deref()
            .is_some_and(|d| d.starts_with("The Arathi Basin")));
        assert_eq!(
            (info.min_level, info.max_level, info.field_16),
            (20, 60, -1)
        );
        assert!(v.group_queue);
        let v = list_view(&state, Some(&catalog), None);
        assert!(
            v.info.is_some_and(|i| i.description.is_none()),
            "the -1 faction leg: no description"
        );
    }
}
