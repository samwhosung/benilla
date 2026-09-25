//! The group packet handlers. [`GroupState`] names the messages each packet implies and these
//! queue them by key, as the reference's `DisplayError` (`0x496720`) takes a message id: the
//! catalog row gives the text, the surface and the sound. [`member_deactivated`] and
//! [`roster_deactivated`] are the object layer's hooks on a roster member's stream-out.

use benilla_protocol::messages::{
    member_status, GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo,
};
use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};

use super::GroupState;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfGuid};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_quest::QuestGiver;

/// One handler per group kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GroupInvite, on_invite)
        .net_handler(K::GroupDecline, on_decline)
        .net_handler(K::GroupUninvited, on_uninvited)
        .net_handler(K::GroupLeaderChanged, on_leader_changed)
        .net_handler(K::GroupDestroyed, on_destroyed)
        .net_handler(K::GroupList, on_list)
        .net_handler(K::PartyCommandResult, on_command_result)
        .net_handler(K::PartyMemberStats, on_member_stats)
        .net_handler(K::RaidTargetSet, on_raid_target)
        .net_handler(K::RaidTargetList, on_raid_target)
        .net_handler(K::ReadyCheckRequest, on_ready_check_request)
        .net_handler(K::ReadyCheckAnswer, on_ready_check_answer)
        .net_handler(K::RaidInstanceInfo, on_raid_instance_info)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_invite(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::GroupInvite { inviter } = ev {
        invited(&mut group, &mut errors, &inviter);
    }
}

fn on_decline(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::GroupDecline { name } = ev {
        declined(&mut group, &mut errors, &name);
    }
}

fn on_uninvited(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::GroupUninvited = ev {
        uninvited(&mut group, &mut errors);
    }
}

fn on_leader_changed(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::GroupLeaderChanged { name } = ev {
        leader_changed(
            &mut group,
            &mut errors,
            &name,
            &self_guid,
            &names,
            &commands,
        );
    }
}

fn on_destroyed(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::GroupDestroyed = ev {
        destroyed(&mut group, &mut errors);
    }
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    mut quest: ResMut<QuestGiver>,
    names: Res<NameCache>,
    index: Res<GuidIndex>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::GroupList {
        group_type,
        own_flags,
        members,
        leader,
        loot,
    } = ev
    {
        list(
            &mut group,
            &mut errors,
            &mut quest,
            group_type,
            own_flags,
            members,
            leader,
            loot,
            &names,
            &index,
            &commands,
        );
    }
}

fn on_command_result(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::PartyCommandResult {
        operation,
        member,
        result,
    } = ev
    {
        command_result(&mut group, &mut errors, operation, &member, result);
    }
}

fn on_member_stats(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::PartyMemberStats { guid, full, info } = ev {
        group.apply_stats(guid, full, *info);
    }
}

fn on_raid_target(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    match ev {
        SessionEvent::RaidTargetSet { icon, guid } => group.apply_raid_target(icon, guid),
        SessionEvent::RaidTargetList { entries } => group.apply_raid_target_list(&entries),
        _ => {}
    }
}

fn on_ready_check_request(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    self_guid: Res<SelfGuid>,
) {
    if let SessionEvent::ReadyCheckRequest = ev {
        ready_check_request(&mut group, &mut errors, &self_guid);
    }
}

fn on_ready_check_answer(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::ReadyCheckAnswer { guid, ready } = ev {
        group.apply_ready_check_answer(guid, ready != 0);
    }
}

fn on_raid_instance_info(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::RaidInstanceInfo { entries } = ev {
        group.apply_raid_instance_info(entries);
    }
}

/// The group state dies with the socket, after the bridge's own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut group: ResMut<GroupState>) {
    group.clear_session();
}

/// Queue the messages an `apply_*` named; `ui_action::feed_actions` resolves and shows them.
fn push_group_lines(errors: &mut UiErrorKeys, lines: Vec<UiError>) {
    errors.0.extend(lines);
}

/// We lead when our guid is the leader's, the reference's test at `0x4ba3a0`.
fn ready_check_request(group: &mut GroupState, errors: &mut UiErrorKeys, self_guid: &SelfGuid) {
    let we_lead = self_guid.0 == Some(group.leader);
    push_group_lines(errors, group.apply_ready_check_request(we_lead));
}

fn invited(group: &mut GroupState, errors: &mut UiErrorKeys, inviter: &str) {
    push_group_lines(errors, group.apply_invited(inviter));
}

fn declined(group: &mut GroupState, errors: &mut UiErrorKeys, name: &str) {
    push_group_lines(errors, group.apply_declined(name));
}

fn uninvited(group: &mut GroupState, errors: &mut UiErrorKeys) {
    push_group_lines(errors, group.apply_uninvited());
}

fn destroyed(group: &mut GroupState, errors: &mut UiErrorKeys) {
    push_group_lines(errors, group.apply_destroyed());
}

/// Our own name, cached from login (`session::connected`), picks the line; this never queries.
fn leader_changed(
    group: &mut GroupState,
    errors: &mut UiErrorKeys,
    name: &str,
    self_guid: &SelfGuid,
    names: &NameCache,
    net_commands: &NetCommands,
) {
    let own = self_guid
        .0
        .and_then(|g| names.resolve(g, net_commands).map(str::to_string));
    push_group_lines(errors, group.apply_leader_changed(name, own.as_deref()));
}

/// `SMSG_GROUP_LIST`: apply the roster, seat new members' records and re-ask the questgiver sweep
/// (shared-quest availability follows the roster). Every member is name-queried once: the
/// answer's race, class and gender are the only source of them for a member we never see streamed.
fn list(
    group: &mut GroupState,
    errors: &mut UiErrorKeys,
    quest: &mut QuestGiver,
    group_type: u8,
    own_flags: u8,
    members: Vec<GroupMemberEntry>,
    leader: u64,
    loot: Option<GroupLootInfo>,
    names: &NameCache,
    index: &GuidIndex,
    net_commands: &NetCommands,
) {
    for m in &members {
        let _ = names.resolve(m.guid, net_commands);
    }
    // Taken before `apply_list` consumes the list; `seat_new_records` then treats a member with
    // no record as new, the reference's `srcRec == 0`.
    let seats: Vec<(u64, bool)> = members
        .iter()
        .map(|m| (m.guid, m.status & member_status::ONLINE != 0))
        .collect();
    let lines = group.apply_list(group_type, own_flags, members, leader, loot);
    push_group_lines(errors, lines);
    seat_new_records(group, &seats, index, net_commands);
    quest.bump_reask();
}

/// `SMSG_GROUP_LIST`'s record leg (`0x4e82d0`, raid twin `0x4ba5f0`): a known member's record
/// carries over and nothing is sent; a new member gets the 1/1 placeholder and, when we hold no
/// object for them, a stats request (`0x4e83f1`, raid `0x4bab6e`). Every member therefore owns a
/// record, so an unseen one shows full bars, not 0/0, until their stats land.
fn seat_new_records(
    group: &mut GroupState,
    seats: &[(u64, bool)],
    index: &GuidIndex,
    net_commands: &NetCommands,
) {
    for (guid, online) in seats {
        // `apply_list` kept only the staying members' records, so a missing one means new.
        if group.stats.contains_key(guid) {
            continue;
        }
        group
            .stats
            .insert(*guid, PartyMemberStatsInfo::placeholder(*online));
        if !index.0.contains_key(guid) {
            let _ = net_commands
                .0
                .send(ClientCommand::RequestPartyMemberStats { guid: *guid });
        }
    }
}

/// The despawn hook, the reference's deactivate virtual `0x5e9aa0` (object destroyed or out of
/// range): for a roster member, snapshot the live descriptor into their record (`0x5f0880`), then
/// request their stats (`0x4e8646`). Nothing happens off the roster or without an object, since
/// the hook is a virtual on the object.
pub(crate) fn member_deactivated(
    guid: u64,
    group: &mut GroupState,
    store: Option<&ObjectStore>,
    net_commands: &NetCommands,
) {
    let Some(store) = store else {
        return;
    };
    if !group.members.iter().any(|m| m.guid == guid) {
        return;
    }
    group
        .stats
        .entry(guid)
        .or_default()
        .snapshot_descriptor(&store.0);
    let _ = net_commands
        .0
        .send(ClientCommand::RequestPartyMemberStats { guid });
}

/// [`member_deactivated`] for every streamed roster member: a cross-map transfer's teardown, where
/// the reference destroys each object and runs the hook on each.
pub(crate) fn roster_deactivated(
    group: &mut GroupState,
    index: &GuidIndex,
    stores: &Query<&mut ObjectStore>,
    net_commands: &NetCommands,
) {
    let streamed: Vec<u64> = group
        .members
        .iter()
        .map(|m| m.guid)
        .filter(|g| index.0.contains_key(g))
        .collect();
    for guid in streamed {
        let store = index.0.get(&guid).and_then(|e| stores.get(*e).ok());
        member_deactivated(guid, group, store, net_commands);
    }
}

fn command_result(
    group: &mut GroupState,
    errors: &mut crate::ui_action::UiErrorKeys,
    operation: u32,
    member: &str,
    result: u32,
) {
    // By key, so the catalog row picks the surface; `None` is the reference's silence.
    errors
        .0
        .extend(group.apply_command_result(operation, member, result));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_protocol::guid;

    fn member(g: u64, name: &str) -> GroupMemberEntry {
        GroupMemberEntry {
            name: name.into(),
            guid: g,
            status: 1, // ONLINE
            flags: 0,
        }
    }

    #[test]
    fn the_roster_warms_every_member_into_the_name_cache_once() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let mut names = NameCache::default();
        // A player guid, `counter | (high << 48)`, the shape `NameCache::resolve` routes on.
        let player_guid = |counter: u64| counter | (u64::from(guid::HIGH_PLAYER) << 48);
        let (leader, far) = (player_guid(7), player_guid(8));

        list(
            &mut group,
            &mut errors,
            &mut quest,
            0,
            0,
            vec![member(leader, "Aldwyn"), member(far, "Brisca")],
            leader,
            None,
            &names,
            &GuidIndex::default(),
            &net,
        );

        let asked: Vec<u64> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::NameQuery { guid } => Some(guid),
                _ => None,
            })
            .collect();
        assert_eq!(
            asked,
            vec![leader, far],
            "both members asked, in roster order"
        );

        // The answer lands for one of them; the re-sent roster asks for neither.
        names.insert_player(leader, "Aldwyn".into(), Some((1, 4, 1)));
        list(
            &mut group,
            &mut errors,
            &mut quest,
            0,
            0,
            vec![member(leader, "Aldwyn"), member(far, "Brisca")],
            leader,
            None,
            &names,
            &GuidIndex::default(),
            &net,
        );
        assert!(
            rx.try_iter()
                .all(|c| !matches!(c, ClientCommand::NameQuery { .. })),
            "a re-sent roster re-asks nothing"
        );
        assert_eq!(names.player_traits(leader), Some((1, 4, 1)));
    }

    // ── The out-of-range record ──────────────────────────────────────────────

    /// Unit descriptor field indices, build 5875.
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    /// `UNIT_FIELD_POWER2`/`MAXPOWER2`, the rage slot (`POWER1 + POWER_RAGE`).
    const POWER2: u16 = 24;
    const MAXPOWER2: u16 = 30;
    const LEVEL: u16 = 34;
    const BYTES_0: u16 = 36;

    fn asked(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::RequestPartyMemberStats { guid } => Some(guid),
                _ => None,
            })
            .collect()
    }

    fn grouped(members: &[GroupMemberEntry]) -> GroupState {
        let mut group = GroupState::default();
        group.apply_list(0, 0, members.to_vec(), members[0].guid, None);
        group
    }

    #[test]
    fn a_members_despawn_snapshots_their_descriptor_and_asks_for_their_stats() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let guid = 0x1234;
        let mut group = grouped(&[member(guid, "Brisca"), member(0x99, "Aldwyn")]);
        let _ = asked(&rx); // start from an empty queue

        // A warrior: rage is stored raw, ten times the shown value, as in the descriptor.
        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (HEALTH, 2400),
            (MAXHEALTH, 3000),
            (POWER2, 570),
            (MAXPOWER2, 1000),
            (LEVEL, 41),
            (BYTES_0, 1 << 24), // POWER_RAGE in BYTES_0 byte 3
        ]));
        member_deactivated(guid, &mut group, Some(&store), &net);

        let rec = group.stats.get(&guid).expect("the member has a record");
        assert_eq!(
            (rec.cur_hp, rec.max_hp, rec.level),
            (Some(2400), Some(3000), Some(41)),
            "the bars keep the numbers they were showing at the edge"
        );
        assert_eq!(
            (rec.power_type, rec.cur_power, rec.max_power),
            (Some(1), Some(570), Some(1000)),
            "RAW power, like the reference's record — the ÷10 happens at the read"
        );
        assert_eq!((rec.shown_power(), rec.shown_max_power()), (57, 100));
        assert_eq!(
            rec.status,
            Some(member_status::ONLINE),
            "an object you can see belongs to an online, living, unflagged player"
        );
        assert_eq!(asked(&rx), vec![guid], "and the server is asked, once");
    }

    #[test]
    fn a_despawn_that_is_not_a_party_member_asks_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut group = grouped(&[member(0x1234, "Brisca")]);
        let _ = asked(&rx);
        // A live object, so only the roster gate can stop it.
        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
            HEALTH, 40,
        )]));
        member_deactivated(0xdead, &mut group, Some(&store), &net);
        assert!(asked(&rx).is_empty());
        assert!(!group.stats.contains_key(&0xdead));

        // The object gate alone: a roster member we hold no object for asks nothing.
        member_deactivated(0x1234, &mut group, None, &net);
        assert!(asked(&rx).is_empty());
    }

    #[test]
    fn a_roster_new_member_is_seated_at_one_one_and_asked_for_only_when_unseen() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let (near, far) = (0x11u64, 0x22u64);

        // `near` is streamed, `far` is not.
        let mut index = GuidIndex::default();
        let mut world = bevy::ecs::world::World::new();
        index.0.insert(near, world.spawn_empty().id());

        let roster = vec![member(near, "Aldwyn"), member(far, "Brisca")];
        let mut send = |group: &mut GroupState, members: Vec<GroupMemberEntry>| {
            list(
                group,
                &mut errors,
                &mut quest,
                0,
                0,
                members,
                near,
                None,
                &names,
                &index,
                &net,
            );
        };

        send(&mut group, roster.clone());
        assert_eq!(
            asked(&rx),
            vec![far],
            "only the member whose object we do not hold"
        );
        for guid in [near, far] {
            let rec = group.stats.get(&guid).expect("every member owns a record");
            assert_eq!(
                (rec.cur_hp, rec.max_hp, rec.cur_power, rec.max_power),
                (Some(1), Some(1), Some(1), Some(1)),
                "a full bar, not an empty one, until the stats land"
            );
        }

        // The wire answers for `far`, then the roster is re-sent: nothing is asked again.
        group.apply_stats(
            far,
            true,
            PartyMemberStatsInfo {
                cur_hp: Some(900),
                max_hp: Some(1100),
                ..PartyMemberStatsInfo::default()
            },
        );
        send(&mut group, roster);
        assert!(
            asked(&rx).is_empty(),
            "a resync of a known roster asks nothing"
        );
        assert_eq!(
            group.stats.get(&far).map(|r| (r.cur_hp, r.max_hp)),
            Some((Some(900), Some(1100))),
            "and the record it already had survives the resync"
        );
    }
}
