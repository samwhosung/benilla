//! The battleground **list window**'s app half (decision 1974): what
//! the stock `BattlefieldFrame.lua` and the minimap's queue icon need from the wire, the clock and
//! the Map.dbc rows. The verbs themselves are `benilla_ui::script::battlefield_queue`'s; the
//! queue slots live in [`BattlefieldQueue`]; the scoreboard is [`crate::ui_battlefield_score`]'s.
//!
//! - **The list** (`SMSG_BATTLEFIELD_LIST`, `0x4aa6c0`) is the battlemaster's guid, the map, the
//!   bracket index and the instance ids. The handler derives the bracket's level pair off the map
//!   row (`bracket · span + minLevel`, max clamped at 60), fires `BATTLEFIELDS_SHOW`, and only THEN
//!   anchors the player's position — the leash below measures from it.
//! - **The leash** (`0x4aa090`): a world tick fires `BATTLEFIELDS_CLOSED` once the player is
//!   strictly more than 5.5556 yd from where the list was opened, and zeroes the anchor first. The
//!   stock window hides itself on the event; there is no other closer.
//! - **The queue view**: every frame the three slots go to the VM with their clock-shaped fields
//!   reduced — the port deadline, the raw estimated wait, the time waited — and the map names
//!   resolved (map 0 included: the reference looks a cleared slot's id up like any other).
//! - **The join** (`0x4a9f60`): `JoinBattlefield`'s instance and group flag come back from the VM;
//!   the group leg is refused client-side with message 442 when the map's `MaxPlayers` is below
//!   either the party count or the raid count, and the opcode is chosen by the cached
//!   battlemaster guid — `CMSG_BATTLEMASTER_JOIN` with it, `CMSG_BATTLEFIELD_JOIN` without.
//! - **The three speaking handlers** (`0x4aacc0`/`0x4aae10`) print and store nothing: the group
//!   verdict (deserters / joined `%s` / failed) and the joined-or-left player lines, the latter
//!   once the name cache answers — the reference's fallback callback, in the shape this app
//!   already uses.
//! - **World enter** (`0x4a9db0`) clears the list scalars and the selection (not the queue slots)
//!   and sends the bodyless `CMSG_BATTLEFIELD_STATUS`, which the server answers slot by slot.

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

/// The leash radius, squared: the CRT initialiser's `fld [0x806574]; fmul` of the `.rdata` f32
/// `5.55555534362793`.
const LEASH_RADIUS_YD: f32 = 5.555_555_3;

/// The leash's degenerate-anchor refusal (`fcomp [0x8029d4]`, `2.384185791015625e-07`): an
/// anchor this close to the origin is "no list open", never a place to measure from.
const DEGENERATE_ANCHOR: f32 = 2.384_185_8e-7;

/// `SMSG_GROUP_JOINED_BATTLEGROUND`'s deserters sentinel (`cmp eax,-2` in `0x4aacc0`).
const GROUP_JOIN_DESERTERS: u32 = 0xFFFF_FFFE;

/// The list window's state: the last list, the leash anchor, and what the speaking handlers
/// still owe the screen.
#[derive(Resource, Default)]
pub(crate) struct Battlefield {
    /// The last `SMSG_BATTLEFIELD_LIST` — the battlemaster, the map, the bracket, the ids.
    list: Option<BattlefieldList>,
    /// `BATTLEFIELDS_SHOW` owed for a list that just landed.
    show: bool,
    /// The list view needs re-pushing (a new list, or the world-enter reset).
    dirty: bool,
    /// Where the list was opened (`[0xb6e870..78]`), the leash's origin; `None` once it fired.
    anchor: Option<Vec3>,
    /// `SMSG_GROUP_JOINED_BATTLEGROUND` results awaiting their line.
    verdicts: Vec<u32>,
    /// Joined (`true`) / left guids awaiting a name.
    players: Vec<(u64, bool)>,
}

impl Battlefield {
    /// `SessionEvent::BattlefieldList` — replaces the list; the event and the anchor follow on
    /// the next feed, in that order.
    pub(crate) fn apply_list(&mut self, list: BattlefieldList) {
        self.list = Some(list);
        self.show = true;
        self.dirty = true;
    }

    /// `SessionEvent::GroupJoinedBattleground` — one line, no state.
    pub(crate) fn apply_verdict(&mut self, result: u32) {
        self.verdicts.push(result);
    }

    /// `SessionEvent::BattlegroundPlayer` — one line once the name resolves, no state.
    pub(crate) fn apply_player(&mut self, guid: u64, joined: bool) {
        self.players.push((guid, joined));
    }

    /// The world-enter reset (`0x4a9db0`): the list scalars and the anchor go; the queue slots do
    /// not.
    fn clear_session(&mut self) {
        self.list = None;
        self.show = false;
        self.dirty = true;
        self.anchor = None;
        self.verdicts.clear();
        self.players.clear();
    }

    /// The guid the last `SMSG_BATTLEFIELD_LIST` came from — the join's opcode choice reads it,
    /// and [`crate::capture::ProbeBgQueuePlugin`] waits on it to know the list actually landed
    /// (a body under the bracket floor is refused at the HELLO, so the list never arrives).
    pub(crate) fn battlemaster(&self) -> Option<u64> {
        self.list.as_ref().map(|l| l.battlemaster)
    }

    /// The listed map — `[0xb6eba4]`, which is 0 (a real Map.dbc row) with nothing listed.
    fn map_id(&self) -> u32 {
        self.list.as_ref().map_or(0, |l| l.map_id)
    }
}

/// The VM's view of the list: the ids, the bracket pair the handler derived, the map row's
/// `GetBattlefieldInfo` half, and the group-queue flag — every one off `[0xb6eba4]`'s row,
/// whatever that id is.
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
            // The faction-group index (`0x5efe00`): 0 for the mask-bit-4 side, 1 for bit 2 — Horde
            // and Alliance in the emulator's naming; the shipped rows carry one text in both.
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

/// One slot's VM view at `now`: the three clock-shaped getters reduced (`0x4ab620`, `0x4ab790`,
/// `0x4ab820`), the map name resolved, the bracket pair derived (`0x4aa850`).
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

/// The as-group refusal (`0x4a9f60`): the map's `MaxPlayers` must be at least the party count (the
/// populated party slots, 0..4 — the members besides us) and the raid roster count. Which
/// members the reference's raid roster counts is INFERRED here as the wire list's members in a
/// raid and none in a party; the party count is VERIFIED.
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

/// Every frame, before the dialog feed fires `UPDATE_BATTLEFIELD_STATUS` and the score feed
/// fires `UPDATE_BATTLEFIELD_SCORE`: the list (when it changed), the queue slots (always), the
/// `BATTLEFIELDS_SHOW` event with its anchor, the leash, and the speaking handlers' lines.
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
        // The description's side is the local player's faction group (`0x5efe00(player)`)
        // — the same answer `UnitFactionGroup("player")` gives, read where it already lives.
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
        // `0x4aa6c0` writes the anchor AFTER the event, from the live player position.
        state.anchor = Some(player.pos);
    }

    // `0x4aa090`: strictly farther than the leash from where the list opened → the event, anchor
    // first.
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

/// After the script tick: `JoinBattlefield`'s sends (with the group refusal and the opcode
/// choice) and `ShowBattlefieldList`'s.
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

/// The world-enter reset (`0x4a9db0`): the list scalars, the selection and the anchor cleared, the
/// queue slots kept, and the bodyless status request sent.
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

/// The battlemaster window's packet handlers (in the net handler table since 2313).
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::Battlefield;
    use crate::net::NetHandlerApp;

    /// Register the handlers — called from [`super::BattlefieldPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldList, on_packet)
            .net_handler(K::GroupJoinedBattleground, on_packet)
            .net_handler(K::BattlegroundPlayer, on_packet);
    }

    /// One handler for the three: each is a one-line fold into the same state.
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

    /// A queued slot's view: the raw estimate, the time waited growing with the clock, the
    /// port deadline zero; a confirm slot's deadline counting down and stopping at zero.
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

    /// The group refusal: the party count and the raid count against `MaxPlayers`; no group
    /// always fits.
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

    /// The list view with nothing listed reads map 0's row — the reference resolves
    /// `[0xb6eba4] = 0` like any other id — and a listed map's bracket pair off its bracket.
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
