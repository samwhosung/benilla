//! The group/party packet handlers (decision 0434 §D2, superseded by 0440; in the net handler
//! table since 2321, moved out of the drain's group arm file). [`GroupState`] mirrors the wire and
//! names the **messages** its roster diff implies; these are the shims that put them on the
//! shared by-key queue. Two bodies here are not packet handlers but the object layer's hook on a
//! roster member's stream-out ([`member_deactivated`], [`roster_deactivated`]); the drain's
//! object arms still call them, and they become listeners on the object kinds once those are
//! peeled.
//!
//! **They are message ids, not sentences** (decisions 2045/2054). The reference reaches all twelve
//! through one `CGGameUI::DisplayError(msgId)`, and the catalog row that id names answers three
//! questions at once — the text, the surface, and the sound. Composing the English here answered
//! only the first, and got the third wrong for free: `ERR_DECLINE_GROUP_S` carries the
//! `igPlayerInviteDecline` cue, which a straight chat push had no way to play.

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

/// Register the group handlers — called from [`super::UiPartyPlugin`]. One per kind, plus the
/// session-end listener.
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

/// The group dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut group: ResMut<GroupState>) {
    group.clear_session();
}

/// Queue the messages a `GroupState::apply_*` named. `ui_action::feed_actions` resolves each key
/// against the VM's own `GlobalStrings.lua` and puts the line on the surface its catalog row
/// names — `ui_guild`'s `push_lines` (decision 2054), one window over.
fn push_group_lines(errors: &mut UiErrorKeys, lines: Vec<UiError>) {
    errors.0.extend(lines);
}

/// `MSG_RAID_READY_CHECK`, the open form (decision 1989): our own echo as leader takes the
/// response-collection arm and prints nothing; as anyone else we print the leader's line and take
/// the popup ticket. The leader test is the reference's guid compare (`0x4ba3a0`).
fn ready_check_request(group: &mut GroupState, errors: &mut UiErrorKeys, self_guid: &SelfGuid) {
    let we_lead = self_guid.0 == Some(group.leader);
    push_group_lines(errors, group.apply_ready_check_request(we_lead));
}

/// `SMSG_GROUP_INVITE` — someone asked us into their group.
fn invited(group: &mut GroupState, errors: &mut UiErrorKeys, inviter: &str) {
    push_group_lines(errors, group.apply_invited(inviter));
}

/// `SMSG_GROUP_DECLINE` — our invitee said no (sent to the inviter only).
fn declined(group: &mut GroupState, errors: &mut UiErrorKeys, name: &str) {
    push_group_lines(errors, group.apply_declined(name));
}

/// `SMSG_GROUP_UNINVITE` — we were kicked.
fn uninvited(group: &mut GroupState, errors: &mut UiErrorKeys) {
    push_group_lines(errors, group.apply_uninvited());
}

/// `SMSG_GROUP_DESTROYED` — the group is gone outright.
fn destroyed(group: &mut GroupState, errors: &mut UiErrorKeys) {
    push_group_lines(errors, group.apply_destroyed());
}

/// `SMSG_GROUP_SET_LEADER` — the line reads differently when the new leader is us, so the composer
/// needs our own name. It is cache-seeded at login (`session::connected`), so this never asks.
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

/// `SMSG_GROUP_LIST` — the roster echo (and the join/leave diff's line source). Roster changes move
/// shared-quest availability, so the questgiver sweep re-asks from here (0654).
///
/// **A roster entry is a sighting** (decision 1564): every member guid is warmed into the
/// [`NameCache`] here, the same ask-once discipline `net::objects` applies the moment a unit
/// streams in. The roster wire carries a member's *name*, so this is not asked for the name — it is
/// asked for the `(race, class, gender)` triple that rides the same answer, and which is the ONLY
/// source of those three for a member we never see: their descriptor never arrives. Two surfaces
/// read them and both were empty for an out-of-area member before this — the raid grid's class
/// column (`ui_party::feed::raid_roster`, whose own-row twin of this hole 1549 §7 found live), and
/// the party frame's 2D portrait stand-in (`portrait::temporary_portrait`, report B315).
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
    // The roster BEFORE the apply, so the "new to this roster" test below is the slot writer's
    // own `srcRec == 0` (see `seat_new_records`), not a re-read of what we just stored.
    let seats: Vec<(u64, bool)> = members
        .iter()
        .map(|m| (m.guid, m.status & member_status::ONLINE != 0))
        .collect();
    let lines = group.apply_list(group_type, own_flags, members, leader, loot);
    push_group_lines(errors, lines);
    seat_new_records(group, &seats, index, net_commands);
    quest.bump_reask();
}

/// **The GROUP_LIST slot writer's record leg** — `0x4e82d0`, the second of the two places the
/// reference asks for a member's stats.
///
/// Per member the writer has two legs, chosen on whether the member already had a record
/// (`srcRec`): a **known** member's record is copied forward and *nothing is sent* — a resync of a
/// group you are already in must not fire four queries — while a member **new to the roster** gets
/// a zeroed record with the `1/1` placeholder ([`PartyMemberStatsInfo::placeholder`]), and then, if
/// their player object is not in the object manager (`4e8398 74 4a je`), the request
/// (`4e83e4 39 7d f8 cmp [ebp-8],edi; 75 5f jne` — the second gate, on `srcRec` again).
///
/// The consequence worth stating: after this, **every roster member owns a record**, which is what
/// makes the merged view's out-of-range leg total. A member you have never seen shows a full `1/1`
/// bar rather than an empty `0/0` one until their stats land — the reference's own picture.
fn seat_new_records(
    group: &mut GroupState,
    seats: &[(u64, bool)],
    index: &GuidIndex,
    net_commands: &NetCommands,
) {
    for (guid, online) in seats {
        // `apply_list` has already dropped the records of everyone who left, and kept the rest —
        // so "no record now" is exactly the writer's `srcRec == 0`.
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

/// **The despawn edge** — CGPlayer_C vtable slot 1 `0x5e9aa0`, which `0x464920` invokes on both
/// `SMSG_DESTROY_OBJECT` and the `SMSG_UPDATE_OBJECT` OUT_OF_RANGE block. A party or raid
/// member's object is leaving the object manager, so:
/// **snapshot its live descriptor into the roster record** (`0x5f0880`), then **ask the server for
/// the member's stats** (`CMSG_REQUEST_PARTY_MEMBER_STATS`, `0x4e8646`), in that order and on that
/// same instruction sequence.
///
/// The pair is why the reference's party frame does not blank when somebody walks over the hill
/// (report B334): the snapshot means the bars keep the numbers they were showing at the edge, and
/// the request means the server answers `_FULL` rather than leaving us on whatever delta its
/// accumulated mask happens to carry next. Neither existed here before decision 1640.
///
/// **Two gates, and both are the reference's.** This runs on every despawn, so a guid that is not
/// on the roster falls straight through; and a guid we hold **no object for** falls through too —
/// the hook is a *virtual on the object*, so no object means it never ran, and a server that
/// re-announces a stream-out we have already applied must not cost a packet.
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

/// [`member_deactivated`] for **every** streamed roster member at once — the bulk teardown a
/// cross-map transfer performs, where the reference destroys the same objects one at a time and
/// runs the same hook on each.
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

/// `SMSG_PARTY_COMMAND_RESULT` — the verdict on an invite/kick/leave we asked for.
fn command_result(
    group: &mut GroupState,
    errors: &mut crate::ui_action::UiErrorKeys,
    operation: u32,
    member: &str,
    result: u32,
) {
    // By KEY, not by sentence, and into the shared queue rather than straight into chat: the
    // catalog row decides the surface, so `result == 7` reaches the red `UIErrorsFrame` line while
    // the other nine stay chat lines (`0x496720`; decision 2035's shape). `None` is the
    // reference's own silence — three inputs display nothing at all.
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

    /// The roster edge is where a member we may never SEE becomes askable. Their descriptor is the
    /// only other source of race/class/gender, and it never arrives while they are out of the local
    /// area — so without this ask the raid grid's class column and the party frame's portrait
    /// stand-in are both permanently blank for exactly the members that need them (B315).
    ///
    /// Ask-ONCE: a re-sent roster (every join, leave, loot-method change re-sends the whole list)
    /// must not re-ask, or a busy group would spam a query per member per packet.
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
        // A player guid: `counter | (high << 48)` — the shape `NameCache::resolve` routes on.
        let player_guid = |counter: u64| counter | (u64::from(guid::HIGH_PLAYER) << 48);
        let (leader, far) = (player_guid(7), player_guid(8));

        list(
            &mut group,
            &mut errors,
            &mut quest,
            0,
            0,
            vec![member(leader, "Frostshake"), member(far, "Thalyn")],
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
        names.insert_player(leader, "Frostshake".into(), Some((1, 4, 1)));
        list(
            &mut group,
            &mut errors,
            &mut quest,
            0,
            0,
            vec![member(leader, "Frostshake"), member(far, "Thalyn")],
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

    // ── The out-of-range record (decision 1640, report B334) ────────────────────────────────

    /// Descriptor field indices, spelled locally the way the other app-side descriptor tests do
    /// (`ui_unit`'s vitals block) — the private `FIELD_UNIT_*` values, build 5875.
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    /// `UNIT_FIELD_POWER2` / `MAXPOWER2` — the RAGE slot (`POWER1 + POWER_RAGE`).
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

    /// **The despawn edge is where the numbers are kept** (the deactivate virtual `0x5e9aa0`
    /// snapshots `0x5f0880` and sends `0x27f`, in that order).
    ///
    /// Falsifier for the whole report: delete the snapshot and this member's record stays whatever
    /// the wire last said — which, for a member who has never had a stats packet, is nothing, and
    /// the frame reads `0/0`. Delete the send and the server is never asked for a `_FULL`, so the
    /// record stays frozen at the snapshot until the member's health happens to change.
    #[test]
    fn a_members_despawn_snapshots_their_descriptor_and_asks_for_their_stats() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let guid = 0x1234;
        let mut group = grouped(&[member(guid, "Thalyn"), member(0x99, "Frostshake")]);
        let _ = asked(&rx); // the roster's own seat-time asks (they are unstreamed here)

        // A warrior: rage rides the wire ×10, and the record stores it raw exactly as the
        // descriptor does.
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

    /// The hook runs on **every** despawn — a mob, a totem, a stranger — and must be silent for
    /// everyone who is not on the roster. Without the guard every stream-out in a busy city would
    /// put a `CMSG_REQUEST_PARTY_MEMBER_STATS` on the wire.
    #[test]
    fn a_despawn_that_is_not_a_party_member_asks_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut group = grouped(&[member(0x1234, "Thalyn")]);
        let _ = asked(&rx);
        // A live object, so only the roster gate can stop it.
        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
            HEALTH, 40,
        )]));
        member_deactivated(0xdead, &mut group, Some(&store), &net);
        assert!(asked(&rx).is_empty());
        assert!(!group.stats.contains_key(&0xdead));

        // …and the object gate on its own: a roster member we hold no object for never
        // deactivated, so re-announcing their stream-out costs nothing.
        member_deactivated(0x1234, &mut group, None, &net);
        assert!(asked(&rx).is_empty());
    }

    /// **The roster seat** (`0x4e82d0`): a member new to the roster gets the `1/1`
    /// placeholder, and is asked for only when we hold no object for them. A resync asks nothing —
    /// the gate the reference puts on `srcRec`, and the reason a busy group does not fire four
    /// queries per GROUP_LIST.
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

        let roster = vec![member(near, "Frostshake"), member(far, "Thalyn")];
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

        // The wire answers for `far`, and then the roster is re-sent (somebody changed the loot
        // method). The cached record is restored and nothing is asked again.
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
