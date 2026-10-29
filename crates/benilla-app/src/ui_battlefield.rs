//! The battleground list window's app half: what the stock `BattlefieldFrame.lua` and the
//! minimap's queue icon need from the wire, the clock and Map.dbc. The queue slots live in
//! [`BattlefieldQueue`]; the scoreboard is [`crate::ui_battlefield_score`]'s.
//!
//! The getters read the list, the slots, the board and the positions through the lent host at the
//! call ([`BattlefieldLive`]), and the handlers' events fire on the battleground lane at their
//! packets' places ([`BattlefieldLane`]). A list (`SMSG_BATTLEFIELD_LIST`, `0x4aa6c0`) fires
//! `BATTLEFIELDS_SHOW` and anchors the leash (`0x4aa090`), which fires `BATTLEFIELDS_CLOSED`, the
//! window's only engine-side close, once the player is more than 5.5556 yd from the anchor.

use std::time::Instant;

use bevy::prelude::*;

use benilla_assets::MapCatalogRes;
use benilla_formats::MapCatalog;
use benilla_protocol::messages::{BattlefieldList, BattlefieldStatus};
use benilla_ui::script::{
    BattlefieldCall, BattlefieldListView, BattlefieldMapInfo, BattlefieldQueueSlot, UiScript,
};

use crate::net::{ClientCommand, NetCommands};
use crate::player::Player;
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_party::GroupState;
use crate::ui_script::UiFeed;

mod lane;
mod live;

pub(crate) use lane::{BattlefieldEvent, BattlefieldLane, BattlefieldNameAnswers};
pub(crate) use live::BattlefieldLive;

/// The leash radius, the `.rdata` f32 at `0x806574`, which the reference squares at startup.
const LEASH_RADIUS_YD: f32 = 5.555_555_3;

/// An anchor this close to the origin means no list is open (`fcomp [0x8029d4]`).
const DEGENERATE_ANCHOR: f32 = 2.384_185_8e-7;

/// `SMSG_GROUP_JOINED_BATTLEGROUND`'s deserters sentinel (`cmp eax,-2` in `0x4aacc0`).
const GROUP_JOIN_DESERTERS: u32 = 0xFFFF_FFFE;

/// The list window's state: the last list, and the joined and left notices waiting on a name.
#[derive(Resource, Default)]
pub(crate) struct Battlefield {
    list: Option<BattlefieldList>,
    /// Joined (`true`) or left guids whose notice waits for the name's answer, each the callback
    /// its packet's lookup left (`0x4aae2f`, `0x4aae36`).
    players: Vec<(u64, bool)>,
}

impl Battlefield {
    pub(crate) fn apply_list(&mut self, list: BattlefieldList) {
        self.list = Some(list);
    }

    fn clear_session(&mut self) {
        self.list = None;
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

    /// The notices waiting on `guid`'s name, spent.
    fn take_waits(&mut self, guid: u64) -> Vec<bool> {
        let mut joined = Vec::new();
        self.players.retain(|&(g, j)| {
            if g == guid {
                joined.push(j);
            }
            g != guid
        });
        joined
    }
}

/// Where the list was opened (`[0xb6e870..78]`), stored after its `BATTLEFIELDS_SHOW`
/// (`0x4aa7ff`); `None` once the leash fired.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldLeash(Option<Vec3>);

/// The VM's view of the list, every value off `[0xb6eba4]`'s map row, whatever that id is, the
/// description by the player's faction group (`0x5efe00`: 0 Horde, 1 Alliance, -1 neither).
fn list_view(state: &Battlefield, catalog: Option<&MapCatalog>, side: i8) -> BattlefieldListView {
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
            // Both columns ship the same text.
            description: match side {
                0 => Some(r.descriptions[0].clone()),
                1 => Some(r.descriptions[1].clone()),
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
    // Status 1: the raw estimate, and `[slot+0x1c] = now − Δ` read back as `now − stamp`, which is
    // the time since the status plus Δ, taken forwards so no `Instant` precedes the clock's origin.
    if let Some((estimate, waited)) = status.queued {
        view.estimated_wait_ms = estimate;
        let since = now.saturating_duration_since(*at);
        view.time_waited_ms = ms(since + std::time::Duration::from_millis(u64::from(waited)));
    }
    view
}

/// The as-group refusal (`0x4a9f60`): the map's `MaxPlayers` must cover the party count (the
/// members besides us, 0..4, `0x4e86d0`) and the raid roster's count, us included (`[0xb713e0]`,
/// `0x4a9fa0`), 0 while no roster is held.
fn group_fits(catalog: Option<&MapCatalog>, map_id: u32, group: Option<&GroupState>) -> bool {
    let max = catalog
        .and_then(|c| c.battleground(map_id))
        .map_or(0, |r| r.max_players) as usize;
    let Some(group) = group else {
        return true;
    };
    let party = group.party_slots().count();
    let raid = group.raid_rows().count();
    max >= party && max >= raid
}

/// The leash (`0x4aa090`), polled once a frame from the world tick (`0x4818d5`): strictly beyond
/// the radius of the stored anchor, the anchor clears, then `BATTLEFIELDS_CLOSED`.
fn drive_battlefield_leash(
    mut vm: crate::script_calls::HostedVm,
    mut leash: ResMut<BattlefieldLeash>,
    player: Res<Player>,
) {
    let Some(anchor) = leash.0 else {
        return;
    };
    if anchor.length_squared() <= DEGENERATE_ANCHOR
        || player.pos.distance_squared(anchor) <= LEASH_RADIUS_YD * LEASH_RADIUS_YD
    {
        return;
    }
    leash.0 = None;
    vm.run(|script| {
        if let Some(script) = script {
            script.fire_event("BATTLEFIELDS_CLOSED", vec![]);
        }
    });
}

/// The battleground verbs inside their calls ([`BattlefieldCall`]): the list window's, the queue
/// slots', the scoreboard's and the teammate map's.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BattlefieldSteps<'w> {
    /// Absent in a world with no battleground windows, which drops the calls.
    state: Option<Res<'w, Battlefield>>,
    queue: Option<Res<'w, BattlefieldQueue>>,
    board: Option<ResMut<'w, crate::ui_battlefield_score::BattlefieldScoreboard>>,
    positions: Option<ResMut<'w, crate::ui_battlefield_positions::BattlefieldPositions>>,
    maps: Option<Res<'w, MapCatalogRes>>,
    group: Option<Res<'w, GroupState>>,
    /// The names and race tables the board's rebuild reads.
    reads: crate::ui_battlefield_score::BoardReads<'w>,
    net: Res<'w, NetCommands>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl BattlefieldSteps<'_> {
    /// `call` inside its verb, with what the call signals as it returns.
    pub(crate) fn apply(
        &mut self,
        script: &mut UiScript,
        call: BattlefieldCall,
    ) -> Option<benilla_ui::script::CallTail> {
        match call {
            BattlefieldCall::Join {
                instance_id,
                as_group,
            } => self.join(script, instance_id, as_group),
            BattlefieldCall::List { map_id } => {
                let _ = self.net.0.send(ClientCommand::BattlefieldList { map_id });
            }
            BattlefieldCall::Port { index, accept } => {
                if let Some(map_id) = self.queue.as_deref().and_then(|q| q.map_id(index)) {
                    let _ = self
                        .net
                        .0
                        .send(ClientCommand::BattlefieldPort { map_id, accept });
                }
            }
            BattlefieldCall::ScoreData => {
                let due = self
                    .board
                    .as_deref_mut()
                    .is_some_and(|b| b.request_due(Instant::now()));
                if due {
                    let _ = self.net.0.send(ClientCommand::RequestBattlefieldScoreData);
                }
            }
            // The rebuild's `UPDATE_BATTLEFIELD_SCORE` signals inside the call (`0x4aa308`).
            BattlefieldCall::ScoreFaction(filter) => {
                let rebuilt = self
                    .board
                    .as_deref_mut()
                    .is_some_and(|b| b.set_filter(filter, &self.reads));
                if rebuilt {
                    return Some(Box::new(|script: &mut UiScript| {
                        script.fire_event("UPDATE_BATTLEFIELD_SCORE", vec![]);
                    }));
                }
            }
            // A negative slot sends map 0 (`0x4abe72`).
            BattlefieldCall::Leave => {
                let map_id = self
                    .queue
                    .as_deref()
                    .and_then(BattlefieldQueue::active_map)
                    .unwrap_or(0);
                let _ = self.net.0.send(ClientCommand::LeaveBattlefield { map_id });
            }
            BattlefieldCall::Positions => {
                let positions = self.positions.as_deref_mut()?;
                if self
                    .queue
                    .as_deref()
                    .and_then(BattlefieldQueue::active_map)
                    .is_none()
                {
                    positions.clear_teammates();
                } else if positions.request_due(Instant::now()) {
                    let _ = self.net.0.send(ClientCommand::RequestBattlefieldPositions);
                }
            }
        }
        None
    }

    /// `0x4a9f60`: the as-group refusal (`0x4a9fa8`), then `CMSG_BATTLEMASTER_JOIN` with a stored
    /// battlemaster or `CMSG_BATTLEFIELD_JOIN` without (`0x4a9fea`), for the listed map.
    fn join(&mut self, script: &mut UiScript, instance_id: u32, as_group: bool) {
        let state = self.state.as_deref();
        let map_id = state.map_or(0, Battlefield::map_id);
        let catalog = self.maps.as_deref().map(|m| &m.0);
        if as_group && !group_fits(catalog, map_id, self.group.as_deref()) {
            let input = script.input_events();
            if let Some(line) =
                crate::ui_action::keyed_line(script, "ERR_GROUP_JOIN_BATTLEGROUND_TOO_MANY")
            {
                crate::ui_action::show_now(
                    script,
                    &mut self.sink,
                    "ui_battlefield",
                    line.under(input),
                );
            }
            return;
        }
        let battlemaster = state
            .and_then(|s| s.list.as_ref())
            .map_or(0, |l| l.battlemaster);
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
        let _ = self.net.0.send(command);
    }
}

/// The world-enter cascade's battlefield init (`0x4909fb` → `0x4a9db0`), after the mail query, in
/// the cascade's place ([`crate::ui_world_enter`]): the list, the notices waiting on names and the
/// anchor clear, the queue slots stay; the selected instance id the VM holds clears with them
/// (`[0xb6eba0]`, `0x4a9dc1`), and so do the score board's scalars and the teammate positions with
/// their request stamp (`0x4a9e55`-`0x4a9e93`, [`crate::ui_battlefield_score`],
/// [`crate::ui_battlefield_positions`]); then the bodyless `CMSG_BATTLEFIELD_STATUS` goes out
/// (`0x4a9eba`), answered slot by slot. It fires no event.
#[allow(clippy::type_complexity)]
pub(crate) fn reset_on_world_enter(
    mut vm: crate::script_calls::VmWith<(
        Option<ResMut<'static, Battlefield>>,
        Option<ResMut<'static, BattlefieldLeash>>,
        Option<ResMut<'static, crate::ui_battlefield_positions::BattlefieldPositions>>,
        Option<ResMut<'static, crate::ui_battlefield_score::BattlefieldScoreboard>>,
    )>,
    commands: Res<NetCommands>,
) {
    vm.turn(|script, (state, leash, positions, board)| {
        if let Some(script) = script {
            script.reset_battlefield_selection();
        }
        if let Some(mut state) = state {
            state.clear_session();
        }
        if let Some(mut leash) = leash {
            leash.0 = None;
        }
        if let Some(mut board) = board {
            *board = Default::default();
        }
        if let Some(mut positions) = positions {
            positions.world_enter();
        }
    });
    let _ = commands.0.send(ClientCommand::BattlefieldStatusRequest);
}

/// The battlemaster window's and the queue's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{Battlefield, BattlefieldEvent, BattlefieldLane};
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp};
    use crate::query_cache::Ask;
    use crate::ui_battlefield_score::BattlefieldScoreboard;
    use crate::ui_dialog_verbs::BattlefieldQueue;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldList, on_list)
            .net_handler(K::GroupJoinedBattleground, on_verdict)
            .net_handler(K::BattlegroundPlayer, on_player)
            .net_handler(K::BattlefieldStatus, on_status);
    }

    /// `0x4aa6c0`: the ids stored, then `BATTLEFIELDS_SHOW` (`0x4aa7d1`), after which the lane
    /// stores the anchor.
    fn on_list(
        In(ev): In<SessionEvent>,
        mut battlefield: ResMut<Battlefield>,
        mut lane: ResMut<BattlefieldLane>,
    ) {
        if let SessionEvent::BattlefieldList(list) = ev {
            battlefield.apply_list(list);
            lane.push(BattlefieldEvent::Show);
        }
    }

    /// `0x4aacc0`: one line, no state.
    fn on_verdict(In(ev): In<SessionEvent>, mut lane: ResMut<BattlefieldLane>) {
        if let SessionEvent::GroupJoinedBattleground { result } = ev {
            lane.push(BattlefieldEvent::Verdict(result));
        }
    }

    /// `0x4aae10`: the player's name looked up with the callback `0x4aaef0` (joined) or `0x4aafe0`
    /// (left), which shows its notice at once for a held name and at the answer for a miss
    /// ([`super::BattlefieldNameAnswers`]).
    fn on_player(
        In(ev): In<SessionEvent>,
        mut battlefield: ResMut<Battlefield>,
        mut lane: ResMut<BattlefieldLane>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
    ) {
        if let SessionEvent::BattlegroundPlayer { guid, joined } = ev {
            match names.resolve_player(guid, Ask::Event(&commands)) {
                Some(name) => lane.push(BattlefieldEvent::Player {
                    name: name.to_string(),
                    joined,
                }),
                None => battlefield.players.push((guid, joined)),
            }
        }
    }

    /// `SMSG_BATTLEFIELD_STATUS` (`0x4aa850`): the slot written and its tutorial armed
    /// (`0x4b5390`); a status 3 builds the stat columns and runs `0x4aa5a0(-1)`, whose rebuild
    /// signals `UPDATE_BATTLEFIELD_SCORE` (`0x4aaa5a`), and a zero map clears them with the
    /// teammates; then `UPDATE_BATTLEFIELD_STATUS` (`0x4aab05`), on every path but the abort.
    fn on_status(
        In(ev): In<SessionEvent>,
        mut queue: ResMut<BattlefieldQueue>,
        mut board: ResMut<BattlefieldScoreboard>,
        mut lane: ResMut<BattlefieldLane>,
        mut positions: ResMut<crate::ui_battlefield_positions::BattlefieldPositions>,
        reads: crate::ui_battlefield_score::BoardReads,
        catalog: Option<Res<crate::world_state_ui::WorldStateUiRes>>,
        tutorials: Option<Res<crate::tutorial::Tutorials>>,
    ) {
        let SessionEvent::BattlefieldStatus(status) = ev else {
            return;
        };
        let Some(applied) = queue.apply(status) else {
            return;
        };
        if let (Some(id), Some(tutorials)) = (applied.tutorial, tutorials) {
            tutorials.trigger_now(id);
        }
        match applied.active_map {
            Some(map) => {
                if board.status_three(map, catalog.as_deref(), &reads) {
                    lane.push(BattlefieldEvent::Score);
                }
            }
            // A zero map clears the stat count and the teammate count (`0x4aaae6`).
            None if applied.cleared => {
                board.clear_columns();
                positions.clear_teammates();
            }
            None => {}
        }
        lane.push(BattlefieldEvent::Status);
    }
}

pub(crate) struct BattlefieldPlugin;

impl Plugin for BattlefieldPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lane::add_battlefield_lane(app);
        app.init_resource::<Battlefield>()
            .init_resource::<BattlefieldLeash>()
            .add_systems(
                Update,
                // The anchor is the lane's, which fires only into the in-game interface.
                drive_battlefield_leash
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The joined and left packets' handlers and the name answer's, in a world of their own.
    fn notices() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<Battlefield>()
            .init_resource::<BattlefieldLane>()
            .init_resource::<crate::names::NameCache>();
        net::register(&mut app);
        crate::names::net::register(&mut app);
        (app, rx)
    }

    fn packet(app: &mut App, ev: benilla_protocol::SessionEvent) -> Vec<BattlefieldEvent> {
        crate::net::handlers::dispatch(app.world_mut(), vec![ev]);
        app.world_mut().resource_mut::<BattlefieldLane>().take()
    }

    fn named(guid: u64, name: &str) -> benilla_protocol::SessionEvent {
        benilla_protocol::SessionEvent::PlayerName {
            guid,
            name: name.into(),
            race: 1,
            gender: 0,
            class: 1,
        }
    }

    /// A joined or left packet looks its player's name up with a callback (`0x4aae2f`): a held
    /// name's notice is its packet's, a missed one's the answer's, and a refusal spends the
    /// callback with no line (`0x4aaf2f`); the next packet that names the player asks again.
    #[test]
    fn a_notice_shows_at_its_packet_or_at_its_names_answer() {
        let (mut app, rx) = notices();
        let joined =
            |joined| benilla_protocol::SessionEvent::BattlegroundPlayer { guid: 0x9, joined };
        assert_eq!(packet(&mut app, joined(true)), []);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        assert_eq!(packet(&mut app, joined(false)), [], "a second callback");
        assert_eq!(
            packet(&mut app, named(0x9, "Rik")),
            [
                BattlefieldEvent::Player {
                    name: "Rik".into(),
                    joined: true
                },
                BattlefieldEvent::Player {
                    name: "Rik".into(),
                    joined: false
                },
            ],
            "both at the answer, in their packets' order"
        );
        assert_eq!(
            packet(&mut app, joined(true)),
            [BattlefieldEvent::Player {
                name: "Rik".into(),
                joined: true
            }],
            "a held name's at its packet"
        );
        assert!(crate::query_cache::queries(&rx).is_empty());

        assert_eq!(
            packet(
                &mut app,
                benilla_protocol::SessionEvent::BattlegroundPlayer {
                    guid: 0xA,
                    joined: true
                }
            ),
            []
        );
        assert_eq!(
            packet(&mut app, named(0xA, "")),
            [],
            "a refusal shows nothing"
        );
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        assert_eq!(
            packet(
                &mut app,
                benilla_protocol::SessionEvent::BattlegroundPlayer {
                    guid: 0xA,
                    joined: true
                }
            ),
            []
        );
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next packet asks again"
        );
    }

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
    fn a_waited_span_longer_than_any_uptime_reads_forwards() {
        let at = Instant::now();
        let mut queued = status(0, 489, 1);
        queued.queued = Some((0, u32::MAX));
        let v = slot_view(Some(&(queued.clone(), at)), None, at);
        assert_eq!(
            v.time_waited_ms,
            u32::MAX,
            "49.7 days waited, no stamp needed"
        );
        let later = at + std::time::Duration::from_secs(9);
        let v = slot_view(Some(&(queued, at)), None, later);
        assert_eq!(
            v.time_waited_ms,
            u32::MAX,
            "saturates as the getter's u32 does"
        );
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
        group.members = (1..=9).map(member).collect();
        group.rebuild_raid(0x5E1F);
        assert!(
            group_fits(Some(&catalog), 489, Some(&group)),
            "a raid of nine and us fits WSG's 10"
        );
        group.members = (1..=10).map(member).collect();
        group.rebuild_raid(0x5E1F);
        assert!(
            !group_fits(Some(&catalog), 489, Some(&group)),
            "the roster counts us: ten and us outrun WSG's 10"
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
        let v = list_view(&state, Some(&catalog), 0);
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
        let v = list_view(&state, Some(&catalog), 1);
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
        let v = list_view(&state, Some(&catalog), -1);
        assert!(
            v.info.is_some_and(|i| i.description.is_none()),
            "the -1 faction leg: no description"
        );
    }
}
