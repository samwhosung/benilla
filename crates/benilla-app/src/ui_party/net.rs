//! The group packet handlers. [`GroupState`] names the messages each packet implies and these
//! queue them by key, as the reference's `DisplayError` (`0x496720`) takes a message id: the
//! catalog row gives the text, the surface and the sound. [`member_deactivated`] and
//! [`roster_deactivated`] are the object layer's hooks on a roster member's stream-out.

use benilla_protocol::messages::{GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};

use super::{GroupState, RosterRelease, GROUPTYPE_RAID};
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfGuid};
use crate::query_cache::Ask;
use crate::sound::MessageSounds;
use crate::target::DeselectGuid;
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_quest::QuestGiver;

/// The join chime, played by name (`0x840404` through `0x458030`).
const INVITE_ACCEPT_SOUND: &str = "igPlayerInviteAccept";

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
        .net_handler(K::PlayerName, on_player_name)
        .net_handler(K::Disconnected, on_session_end)
        .add_message::<DeselectGuid>();
}

/// The popup's signal; the feed shows its line after the event ([`GroupState::apply_invited`]).
fn on_invite(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::GroupInvite { inviter } = ev {
        group.apply_invited(&inviter);
    }
}

fn on_decline(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>, errors: Res<UiErrorKeys>) {
    if let SessionEvent::GroupDecline { name } = ev {
        declined(&mut group, &errors, &name);
    }
}

fn on_uninvited(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    mut teardown: RosterTeardown,
) {
    if let SessionEvent::GroupUninvited = ev {
        uninvited(&mut group, &errors);
        teardown.run(&mut group);
    }
}

fn on_leader_changed(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
) {
    if let SessionEvent::GroupLeaderChanged { name } = ev {
        leader_changed(&mut group, &errors, &name, &self_guid, &names);
    }
}

fn on_destroyed(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    mut teardown: RosterTeardown,
) {
    if let SessionEvent::GroupDestroyed = ev {
        destroyed(&mut group, &errors);
        teardown.run(&mut group);
    }
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    sounds: Res<MessageSounds>,
    mut quest: ResMut<QuestGiver>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    index: Res<GuidIndex>,
    commands: Res<NetCommands>,
    tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    mut teardown: RosterTeardown,
) {
    if let SessionEvent::GroupList {
        group_type,
        own_flags,
        members,
        leader,
        loot,
    } = ev
    {
        // The raid roster writer opens on Raid Groups for a rebuild with rows (`0x4ba604`,
        // `0x4ba60d`), every one; the tutorial's own bit makes it fire once.
        if group_type == GROUPTYPE_RAID && !members.is_empty() {
            if let Some(mut tutorials) = tutorials {
                tutorials.write(crate::tutorial::TutorialEvent::trigger(
                    crate::tutorial::id::RAID_GROUPS,
                ));
            }
        }
        list(
            &mut group, &errors, &sounds, &mut quest, group_type, own_flags, members, leader, loot,
            &self_guid, &names, &index, &commands,
        );
        teardown.run(&mut group);
    }
}

fn on_command_result(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    mut teardown: RosterTeardown,
) {
    if let SessionEvent::PartyCommandResult {
        operation,
        member,
        result,
    } = ev
    {
        command_result(&mut group, &errors, operation, &member, result);
        teardown.run(&mut group);
    }
}

/// The selection teardowns (`0x493910(guid, 1)`) a list or an ending raised
/// ([`GroupState::take_released`]), resolved while the objects stand as the packet found them and
/// handed to the guid-scoped deselect ([`DeselectGuid`]). A harness with no object index holds no
/// object, and one with no selection drops them.
#[derive(bevy::ecs::system::SystemParam)]
struct RosterTeardown<'w, 's> {
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    deselect: Option<MessageWriter<'w, DeselectGuid>>,
}

impl RosterTeardown<'_, '_> {
    fn run(&mut self, group: &mut GroupState) {
        let released = group.take_released();
        let guids: Vec<u64> = released
            .into_iter()
            .filter_map(|release| self.released_guid(release))
            .collect();
        if let Some(deselect) = &mut self.deselect {
            deselect.write_batch(guids.into_iter().map(DeselectGuid));
        }
    }

    /// The guid a release tears down. Each lookup asks for a unit (typemask 8: the member at
    /// `0x4e84d7` and `0x5e6d50`, the pet at `0x4e8523`, `0x5e6da1` and `0x4baa64`), but the raid's
    /// for its member asks for a player (`0x4baa18`), which a row's member is.
    fn released_guid(&self, release: RosterRelease) -> Option<u64> {
        let held = |guid: u64| {
            self.index
                .as_deref()
                .and_then(|index| index.0.get(&guid))
                .and_then(|&e| self.stores.get(e).ok())
                .filter(|s| s.is_unit())
        };
        match release {
            RosterRelease::Offline(guid) => Some(guid),
            RosterRelease::Departed(member) => match held(member) {
                None => Some(member),
                // `CHARM`, else `SUMMON` (`0x4e84e6`-`0x4e84f3`, `0x5e6d60`-`0x5e6d6d`).
                Some(store) => store.0.unit_pet_guid().filter(|&pet| held(pet).is_none()),
            },
        }
    }
}

/// The record lands, then the unit events of what it changed go out (`0x5e54fa`), the old record
/// the stored one or, for a member neither list holds, an empty one (`0x5e519b`).
fn on_member_stats(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut notices: ResMut<crate::field_watch::NoticeLane>,
) {
    if let SessionEvent::PartyMemberStats { guid, full, info } = ev {
        let old = group.stats.get(&guid).cloned().unwrap_or_default();
        group.apply_stats(guid, full, *info);
        if let Some(new) = group.stats.get(&guid) {
            notices.write_batch(super::records::record_notices(guid, &old, new));
        }
    }
}

fn on_raid_target(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    match ev {
        SessionEvent::RaidTargetSet { icon, guid } => group.apply_raid_target_update(icon, guid),
        SessionEvent::RaidTargetList { entries } => group.apply_raid_target_list(&entries),
        _ => {}
    }
}

fn on_ready_check_request(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    self_guid: Res<SelfGuid>,
) {
    if let SessionEvent::ReadyCheckRequest = ev {
        ready_check_request(&mut group, &errors, &self_guid);
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

/// `SMSG_NAME_QUERY_RESPONSE`, found or refused: the roster's lookups that waited on it run
/// ([`GroupState::apply_player_name`]). The name cache's handler, registered ahead of every other
/// (`names::net::register`), has filed the answer by then, and a found answer's notices read that
/// record with no callback (`0x4bad6b`).
fn on_player_name(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    errors: Res<UiErrorKeys>,
    names: Res<NameCache>,
) {
    if let SessionEvent::PlayerName { guid, name, .. } = ev {
        let filed = (!name.is_empty())
            .then(|| names.resolve_player(guid, Ask::Read))
            .flatten();
        push_group_lines(&errors, group.apply_player_name(guid, filed));
    }
}

/// The group state dies with the socket, after the bridge's own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut group: ResMut<GroupState>) {
    group.clear_session();
}

/// Queue the messages an `apply_*` named; `ui_action::feed_actions` resolves and shows them.
fn push_group_lines(errors: &UiErrorKeys, lines: Vec<UiError>) {
    errors.extend(lines);
}

/// We lead when our guid is the leader's, the reference's test at `0x4ba3a0`.
fn ready_check_request(group: &mut GroupState, errors: &UiErrorKeys, self_guid: &SelfGuid) {
    let we_lead = self_guid.0 == Some(group.leader);
    push_group_lines(errors, group.apply_ready_check_request(we_lead));
}

fn declined(group: &mut GroupState, errors: &UiErrorKeys, name: &str) {
    push_group_lines(errors, group.apply_declined(name));
}

fn uninvited(group: &mut GroupState, errors: &UiErrorKeys) {
    push_group_lines(errors, group.apply_uninvited());
}

fn destroyed(group: &mut GroupState, errors: &UiErrorKeys) {
    push_group_lines(errors, group.apply_destroyed());
}

/// Our own name, cached from login (`session::connected`), picks the line; this never queries.
fn leader_changed(
    group: &mut GroupState,
    errors: &UiErrorKeys,
    name: &str,
    self_guid: &SelfGuid,
    names: &NameCache,
) {
    let own = self_guid
        .0
        .and_then(|g| names.resolve_player(g, Ask::Read).map(str::to_string));
    push_group_lines(errors, group.apply_leader_changed(name, own.as_deref()));
}

/// `SMSG_GROUP_LIST`: apply the roster, seat new members' records, ask for every raid-target icon
/// when a raid list finds no roster held, and re-ask the questgiver sweep (shared-quest
/// availability follows the roster). Every member is name-queried once: the
/// answer's race, class and gender are the only source of them for a member we never see streamed.
/// The party seats (`0x4e8477`) and a raid's rows (`0x4ba9c7`), ours among them once others are
/// listed (`0x5e6e73`), each look their name up with a callback, which [`GroupState`] keeps for
/// the answer ([`on_player_name`]).
fn list(
    group: &mut GroupState,
    errors: &UiErrorKeys,
    sounds: &MessageSounds,
    quest: &mut QuestGiver,
    group_type: u8,
    own_flags: u8,
    members: Vec<GroupMemberEntry>,
    leader: u64,
    loot: Option<GroupLootInfo>,
    self_guid: &SelfGuid,
    names: &NameCache,
    index: &GuidIndex,
    net_commands: &NetCommands,
) {
    // The lookups with a callback, which ask a refused name again; a miss waits for the answer.
    // A party list seats every member and a raid list rows every one, so each is asked.
    let lookup = |guid: u64| {
        names
            .resolve_player(guid, Ask::Event(net_commands))
            .map(str::to_string)
    };
    // The loot master's name as the setter reads it, from the cache with no callback (`0x4e878f`).
    let cached_name = |guid: u64| names.resolve_player(guid, Ask::Read).map(str::to_string);
    // Taken before `apply_list` consumes the list; `seat_new_records` then treats a member with
    // no record as new, the reference's `srcRec == 0`.
    let seats: Vec<(u64, bool)> = members
        .iter()
        .map(|m| (m.guid, m.listed_online()))
        .collect();
    let shown = group.apply_list_awaiting(
        group_type,
        own_flags,
        members,
        leader,
        loot,
        self_guid.0,
        &lookup,
        &cached_name,
    );
    push_group_lines(errors, shown.lines);
    if shown.invite_accept {
        sounds.push_cue(INVITE_ACCEPT_SOUND);
    }
    seat_new_records(group, &seats, index, net_commands);
    // The roster writer's last send, after its stats requests (`0x4bac33`).
    if shown.ask_raid_targets {
        let _ = net_commands.0.send(ClientCommand::RequestRaidTargets);
    }
    quest.bump_reask();
}

/// `SMSG_GROUP_LIST`'s record leg (`0x4e82d0`, raid twin `0x4ba5f0`): a known member's record
/// carries over, its online bit rewritten by [`GroupState::apply_list`], and nothing is sent; a new
/// member gets the 1/1 placeholder and, when we hold no object for them, a stats request
/// (`0x4e83f1`, raid `0x4bab6e`). A listed member therefore owns a record, so an unseen one shows
/// full bars, not 0/0, until their stats land.
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

/// The pet name the deactivate snapshot (`0x5f0880`) writes for a held pet: `0x609210`'s answer
/// (`0x5f0a63`), the cached `name`, else the VM's `UNKNOWNOBJECT` text as it reads at the snapshot
/// (`0x609324`-`0x60933c`, [`benilla_ui::script::UiScript::unknown_object_text`]). With no VM up,
/// as between the world-entry edge and the interface's load, there is no text to copy, and the
/// name is left empty.
pub(crate) fn snapshot_pet_name(name: Option<&str>, script: Option<&UiScript>) -> String {
    match (name, script) {
        (Some(name), _) => name.to_string(),
        (None, Some(script)) => script.unknown_object_text(),
        (None, None) => String::new(),
    }
}

/// The despawn hook, the reference's deactivate virtual `0x5e9aa0` (object destroyed or out of
/// range): for a roster member, snapshot the live descriptor into their record (`0x5f0880`), their
/// pet's too while `held` finds it, with the name `names` holds for it or `UNKNOWNOBJECT` on a miss
/// ([`snapshot_pet_name`]), then request their stats (`0x4e8646`). Nothing happens off the roster
/// or without an object, since the hook is a virtual on the object.
pub(crate) fn member_deactivated<'a>(
    guid: u64,
    group: &mut GroupState,
    store: Option<&'a ObjectStore>,
    held: impl Fn(u64) -> Option<&'a ObjectStore>,
    names: &NameCache,
    script: Option<&UiScript>,
    net_commands: &NetCommands,
) {
    let Some(store) = store else {
        return;
    };
    if !group.members.iter().any(|m| m.guid == guid) {
        return;
    }
    let pet_guid = store.0.unit_pet_guid();
    let pet = pet_guid.and_then(held);
    let pet_name = pet_guid
        .zip(pet)
        .and_then(|(g, p)| names.resolve_unit(g, Some(p), Ask::Read));
    let pet_name = snapshot_pet_name(pet_name, script);
    group.stats.entry(guid).or_default().snapshot_descriptor(
        &store.0,
        pet.map(|p| &p.0),
        &pet_name,
    );
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
    names: &NameCache,
    script: Option<&UiScript>,
    net_commands: &NetCommands,
) {
    let streamed: Vec<u64> = group
        .members
        .iter()
        .map(|m| m.guid)
        .filter(|g| index.0.contains_key(g))
        .collect();
    let held = |g: u64| index.0.get(&g).and_then(|e| stores.get(*e).ok());
    for guid in streamed {
        member_deactivated(guid, group, held(guid), held, names, script, net_commands);
    }
}

fn command_result(
    group: &mut GroupState,
    errors: &crate::ui_action::UiErrorKeys,
    operation: u32,
    member: &str,
    result: u32,
) {
    // By key, so the catalog row picks the surface; `None` is the reference's silence.
    errors.extend(group.apply_command_result(operation, member, result));
}

#[cfg(test)]
mod tests {
    use super::super::GroupSignal;
    use super::*;
    use crate::net::ClientCommand;
    use benilla_protocol::guid;
    use benilla_protocol::messages::member_status;

    fn member(g: u64, name: &str) -> GroupMemberEntry {
        GroupMemberEntry {
            name: name.into(),
            guid: g,
            status: 1, // ONLINE
            flags: 0,
        }
    }

    /// Each list looks its members up with a callback (`0x4e8477`): a refused name is asked again
    /// by the next list, and nothing between lists asks.
    #[test]
    fn a_refused_member_is_asked_again_by_the_next_list() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let mut names = NameCache::default();
        let far = 8 | (u64::from(guid::HIGH_PLAYER) << 48);
        let mut send = |names: &NameCache| {
            list(
                &mut group,
                &errors,
                &MessageSounds::default(),
                &mut quest,
                0,
                0,
                vec![member(far, "Brisca")],
                far,
                None,
                &SelfGuid::default(),
                names,
                &GuidIndex::default(),
                &net,
            );
        };
        send(&names);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        names.insert_player(far, String::new(), None);
        assert!(crate::query_cache::queries(&rx).is_empty());
        send(&names);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next list asks again"
        );
    }

    #[test]
    fn the_roster_warms_every_member_into_the_name_cache_once() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, errors, mut quest) = (
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
            &errors,
            &MessageSounds::default(),
            &mut quest,
            0,
            0,
            vec![member(leader, "Aldwyn"), member(far, "Brisca")],
            leader,
            None,
            &SelfGuid::default(),
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
            &errors,
            &MessageSounds::default(),
            &mut quest,
            0,
            0,
            vec![member(leader, "Aldwyn"), member(far, "Brisca")],
            leader,
            None,
            &SelfGuid::default(),
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

    /// The rebuild counts the rows whose name has not landed (`0x4ba9b6`-`0x4ba9d0`), ours among
    /// them once others are listed (`0x5e6e73`), and the roster event waits for none (`0x4babdf`).
    #[test]
    fn a_raid_list_signals_the_roster_only_once_every_name_is_held() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let mut names = NameCache::default();
        let player_guid = |counter: u64| counter | (u64::from(guid::HIGH_PLAYER) << 48);
        let (me, a, b) = (player_guid(1), player_guid(7), player_guid(8));
        let mut raid = |group: &mut GroupState, names: &NameCache, group_type: u8| {
            list(
                group,
                &errors,
                &MessageSounds::default(),
                &mut quest,
                group_type,
                0,
                vec![member(a, "Aldwyn"), member(b, "Brisca")],
                a,
                None,
                &SelfGuid(Some(me)),
                names,
                &GuidIndex::default(),
                &net,
            );
            let signals = group.signals.since(0);
            signals
                .iter()
                .filter(|s| **s == GroupSignal::RaidRosterUpdate)
                .count()
        };

        assert_eq!(raid(&mut group, &names, 1), 0, "no name held");
        names.insert_player(a, "Aldwyn".into(), Some((1, 4, 1)));
        assert_eq!(
            raid(&mut group, &names, 1),
            0,
            "one member's name still to come"
        );
        names.insert_player(b, "Brisca".into(), Some((1, 5, 0)));
        assert_eq!(
            raid(&mut group, &names, 1),
            0,
            "our own row's name still to come"
        );
        names.insert_player(me, "Me".into(), None);
        assert_eq!(raid(&mut group, &names, 1), 1, "every name held");
        assert_eq!(raid(&mut group, &names, 1), 2, "and the same list again");

        // A party's list runs no roster writer; it names nothing, and drops the raid it finds.
        assert_eq!(
            raid(&mut group, &NameCache::default(), 0),
            3,
            "the raid dropped"
        );
        assert_eq!(
            raid(&mut group, &NameCache::default(), 0),
            3,
            "a party's list"
        );
    }

    #[test]
    fn a_join_queues_the_invite_accept_chime_and_a_resync_does_not() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, errors, sounds, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            MessageSounds::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let mut send = |sounds: &MessageSounds, members: Vec<GroupMemberEntry>| {
            list(
                &mut group,
                &errors,
                sounds,
                &mut quest,
                0,
                0,
                members,
                7,
                None,
                &SelfGuid(Some(9)),
                &names,
                &GuidIndex::default(),
                &net,
            );
        };

        send(&sounds, vec![member(7, "Aldwyn")]);
        assert_eq!(
            sounds.queued_cues(),
            ["igPlayerInviteAccept"],
            "our first party"
        );
        send(&sounds, vec![member(7, "Aldwyn")]);
        assert_eq!(sounds.queued_cues().len(), 1, "a resync is silent");
        send(&sounds, vec![member(7, "Aldwyn"), member(8, "Brisca")]);
        assert_eq!(sounds.queued_cues().len(), 2, "a member joining our party");
        assert_eq!(
            benilla_ui::messages::by_key("ERR_JOINED_GROUP_S").and_then(|r| r.sound),
            None,
            "the join line's own row carries no sound, so the chime is the only one"
        );
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
        group.apply_list(0, 0, members.to_vec(), members[0].guid, None, None);
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
        member_deactivated(
            guid,
            &mut group,
            Some(&store),
            |_| None,
            &NameCache::default(),
            None,
            &net,
        );

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

    /// `0x5f098d`-`0x5f09f2`: a slot is copied when its id is set and its `AURAFLAGS` nibble has an
    /// effect bit; the pet block comes off the pet's own object (`0x5f0a1f`-`0x5f0b72`).
    #[test]
    fn a_members_despawn_snapshots_their_auras_and_their_pets() {
        const AURA: u16 = 47;
        const AURAFLAGS: u16 = 95;
        const SUMMON: u16 = 8;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let guid = 0x1234;
        let pet_guid = 0xF140_0000_0000_0077;
        let mut group = grouped(&[member(guid, "Brisca")]);

        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (AURA, 1126),     // slot 0, live (nibble 0x8)
            (AURA + 1, 6673), // slot 1, a stale id: its nibble has no effect bit
            (AURA + 33, 589), // slot 33, live
            (AURAFLAGS, 0x0000_0018),
            (AURAFLAGS + 4, 0x0000_0020),
            (SUMMON, pet_guid as u32),
            (SUMMON + 1, (pet_guid >> 32) as u32),
        ]));
        // `UNIT_FIELD_PETNUMBER`, the key the pet-name cache files a pet's name under.
        const PETNUMBER: u16 = 139;
        let pet = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (HEALTH, 900),
            (PETNUMBER, 4),
            (AURA + 40, 770),
            (AURAFLAGS + 5, 0x0000_0002),
        ]));
        let held = |g: u64| (g == pet_guid).then_some(&pet);
        let mut names = NameCache::default();
        names.insert_pet(4, "Whelp".into());
        member_deactivated(guid, &mut group, Some(&store), held, &names, None, &net);

        let rec = group.stats.get(&guid).expect("the member has a record");
        assert_eq!(
            rec.auras,
            Some(vec![(0, 1126)]),
            "the stale slot 1 is dropped"
        );
        assert_eq!(rec.auras_negative, Some(vec![(33, 589)]));
        assert_eq!(rec.pet_guid, Some(pet_guid));
        assert_eq!(
            rec.pet_name.as_deref(),
            Some("Whelp"),
            "the pet's cached name rides into the record, as `0x5f0a63`-`0x5f0a70` copy it (`+0x88`)"
        );
        assert_eq!(rec.pet_cur_hp, Some(900));
        assert_eq!(rec.pet_auras, Some(vec![]));
        assert_eq!(rec.pet_auras_negative, Some(vec![(40, 770)]));

        // A pet whose name the cache misses: `0x609210` answers `UNKNOWNOBJECT` (`0x609324`) and
        // the record takes that, never the name it held (`0x5f0a70`).
        let unnamed = NameCache::default();
        let vm = benilla_ui::script::UiScript::new().unwrap();
        vm.run(r#"UNKNOWNOBJECT = "Unknown""#).unwrap();
        member_deactivated(
            guid,
            &mut group,
            Some(&store),
            held,
            &unnamed,
            Some(&vm),
            &net,
        );
        assert_eq!(group.stats[&guid].pet_name.as_deref(), Some("Unknown"));

        // The pet not held: its block is emptied, not left as it was.
        member_deactivated(guid, &mut group, Some(&store), |_| None, &names, None, &net);
        let rec = group.stats.get(&guid).unwrap();
        assert_eq!((rec.pet_guid, rec.pet_auras_negative.clone()), (None, None));
        assert_eq!(rec.pet_name, None, "and its name with it (`0x5f0a0f`)");
        assert_eq!(rec.auras, Some(vec![(0, 1126)]));
    }

    /// Raid Groups opens on every raid roster rebuild with rows (`0x4ba604`, `0x4ba60d`): a raid
    /// list that names a member asks for it, each time, and a raid of one or a party asks nothing.
    #[test]
    fn a_raid_list_with_rows_triggers_the_raid_groups_tutorial() {
        use crate::tutorial::{id, TutorialEvent};
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GroupState>()
            .init_resource::<UiErrorKeys>()
            .init_resource::<MessageSounds>()
            .init_resource::<QuestGiver>()
            .insert_resource(SelfGuid(Some(0x10)))
            .init_resource::<NameCache>()
            .init_resource::<GuidIndex>()
            .add_message::<TutorialEvent>();
        let mut triggered = |group_type: u8, members: Vec<GroupMemberEntry>| {
            app.world_mut()
                .run_system_once_with(
                    on_list,
                    SessionEvent::GroupList {
                        group_type,
                        own_flags: 0,
                        members,
                        leader: 0x10,
                        loot: None,
                    },
                )
                .expect("the group-list handler");
            app.world_mut()
                .resource_mut::<Messages<TutorialEvent>>()
                .drain()
                .filter(|e| *e == TutorialEvent::trigger(id::RAID_GROUPS))
                .count()
        };
        assert_eq!(triggered(GROUPTYPE_RAID, Vec::new()), 0, "a raid of one");
        assert_eq!(triggered(0, vec![member(0x22, "Brisca")]), 0, "a party");
        assert_eq!(triggered(GROUPTYPE_RAID, vec![member(0x22, "Brisca")]), 1);
        assert_eq!(
            triggered(GROUPTYPE_RAID, vec![member(0x22, "Brisca")]),
            1,
            "every rebuild with rows"
        );
    }

    /// The snapshot reads the VM's `UNKNOWNOBJECT` as it stands at the snapshot (`0x609324`), so a
    /// list that landed before the VM came up leaves nothing stale to copy; an empty global reads
    /// the literal (`0x60933c`), and with no VM there is no text to copy.
    #[test]
    fn a_snapshot_reads_unknownobject_off_the_vm_it_runs_beside() {
        use benilla_ui::script::UiScript;
        const SUMMON: u16 = 8;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let pet_guid = 0xF140_0000_0000_0099;
        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (HEALTH, 2400),
            (SUMMON, pet_guid as u32),
            (SUMMON + 1, (pet_guid >> 32) as u32),
        ]));
        let pet = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
            HEALTH, 900,
        )]));
        let held = |g: u64| (g == pet_guid).then_some(&pet);
        let named = |global: Option<&str>| {
            let vm = global.map(|g| {
                let vm = UiScript::new().unwrap();
                vm.run(&format!("UNKNOWNOBJECT = {g:?}")).unwrap();
                vm
            });
            // The list lands with no VM up, as in the world entry's window before the interface.
            let mut group = grouped(&[member(0x22, "Brisca")]);
            member_deactivated(
                0x22,
                &mut group,
                Some(&store),
                held,
                &NameCache::default(),
                vm.as_ref(),
                &net,
            );
            group.stats[&0x22].pet_name.clone()
        };
        assert_eq!(named(Some("Unknown")).as_deref(), Some("Unknown"));
        assert_eq!(named(Some("")).as_deref(), Some("Unknown Being"));
        assert_eq!(named(None).as_deref(), Some(""), "no VM");
    }

    /// The handlers end to end: a raid list whose member's name the cache misses asks it and holds
    /// `RAID_ROSTER_UPDATE` back (`0x4ba9c7`, `0x4babdf`); the answer's handler runs the lookup
    /// that waited, the last pending, so the roster update and the members change signal
    /// (`0x4bad90`-`0x4badb0`).
    #[test]
    fn a_raid_list_waits_on_a_name_the_answer_brings() {
        use bevy::ecs::system::RunSystemOnce;
        const ME: u64 = 0x10;
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GroupState>()
            .init_resource::<UiErrorKeys>()
            .init_resource::<MessageSounds>()
            .init_resource::<QuestGiver>()
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<NameCache>()
            .init_resource::<GuidIndex>();
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(ME, "Me".into(), None);
        let mut far = member(0x22, "Brisca");
        far.flags = 1;
        app.world_mut()
            .run_system_once_with(
                on_list,
                SessionEvent::GroupList {
                    group_type: GROUPTYPE_RAID,
                    own_flags: 0,
                    members: vec![far],
                    leader: ME,
                    loot: None,
                },
            )
            .expect("the group-list handler");
        let signals = |app: &mut App| app.world_mut().resource_mut::<GroupState>().signals.take();
        assert_eq!(
            signals(&mut app),
            [GroupSignal::MembersChanged, GroupSignal::LeaderChanged],
            "a name pending: the list's own, and no roster yet"
        );
        let queried: Vec<u64> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::NameQuery { guid } => Some(guid),
                _ => None,
            })
            .collect();
        assert_eq!(queried, vec![0x22], "and asked, once");

        app.world_mut()
            .run_system_once_with(
                on_player_name,
                SessionEvent::PlayerName {
                    guid: 0x22,
                    name: "Brisca".into(),
                    race: 1,
                    class: 1,
                    gender: 0,
                },
            )
            .expect("the name handler");
        assert_eq!(
            signals(&mut app),
            [GroupSignal::RaidRosterUpdate, GroupSignal::MembersChanged]
        );
    }

    /// The group handlers on a session holding us as [`SelfGuid`] `ME`, our name cached.
    fn handler_app(me: u64) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GroupState>()
            .init_resource::<UiErrorKeys>()
            .init_resource::<MessageSounds>()
            .init_resource::<QuestGiver>()
            .insert_resource(SelfGuid(Some(me)))
            .init_resource::<NameCache>()
            .init_resource::<GuidIndex>();
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(me, "Me".into(), None);
        (app, rx)
    }

    /// `SMSG_GROUP_LIST` through its handler.
    fn list_event(app: &mut App, group_type: u8, members: Vec<GroupMemberEntry>, leader: u64) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once_with(
                on_list,
                SessionEvent::GroupList {
                    group_type,
                    own_flags: 0,
                    members,
                    leader,
                    loot: None,
                },
            )
            .expect("the group-list handler");
    }

    /// `SMSG_NAME_QUERY_RESPONSE` through the name cache's handler, then the group's, as the
    /// table runs them; an empty name is a refusal.
    fn name_event(app: &mut App, guid: u64, name: &str) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(guid, name.into(), None);
        app.world_mut()
            .run_system_once_with(
                on_player_name,
                SessionEvent::PlayerName {
                    guid,
                    name: name.into(),
                    race: 1,
                    class: 1,
                    gender: 0,
                },
            )
            .expect("the name handler");
    }

    /// The name cache files a found answer before any callback of it runs: `0x55f310` copies the
    /// record, sets its valid byte (`0x55f413`), then walks the callbacks (`0x55f426`-`0x55f452`).
    /// So the cache's handler runs ahead of the roster's, which reads the filed record, whichever
    /// plugin registers first.
    #[test]
    fn the_name_cache_files_an_answer_ahead_of_the_rosters_callbacks() {
        let mut app = App::new();
        register(&mut app);
        crate::names::net::register(&mut app);
        let table = app
            .world()
            .resource::<crate::net::handlers::NetHandlers>()
            .census();
        let order: Vec<&str> = table[&SessionEventKind::PlayerName]
            .iter()
            .map(|name| name.trim_start_matches("benilla_app::"))
            .collect();
        assert_eq!(
            order,
            [
                "names::net::on_player_name",
                "ui_party::net::on_player_name"
            ]
        );
    }

    fn shown(app: &mut App) -> Vec<UiError> {
        app.world_mut().resource::<UiErrorKeys>().take()
    }

    /// A raid list onto a held roster looks each arrival up with the callback `0x4bacd0`
    /// (`0x4ba72e`-`0x4ba744`) and each departure with `0x4bad30` (`0x4ba7c6`-`0x4ba7e3`): a
    /// cached name shows its line at once, the cache's spelling (`0x4ba74e`, `0x4ba7f8`), and a
    /// miss shows it when the answer finds the name (`0x4bad15`, `0x4bad75`); a refusal shows
    /// nothing (`0x4bad50`).
    #[test]
    fn a_raid_notice_for_a_name_still_to_come_waits_for_its_answer() {
        const ME: u64 = 0x10;
        let (mut app, _rx) = handler_app(ME);
        let (a, b, c, d, e) = (0x21, 0x22, 0x23, 0x24, 0x25);
        for (guid, name) in [(a, "Aldwyn"), (b, "Brisca"), (d, "Dorn")] {
            app.world_mut()
                .resource_mut::<NameCache>()
                .insert_player(guid, name.into(), None);
        }
        // Joining Aldwyn's raid: no roster held, so no notices (`0x4ba725`); Eira's name never
        // lands.
        list_event(
            &mut app,
            GROUPTYPE_RAID,
            vec![member(a, "Aldwyn"), member(b, "Brisca"), member(e, "Eira")],
            a,
        );
        assert_eq!(shown(&mut app), [UiError::key("ERR_RAID_YOU_JOINED")]);

        // Cyra (uncached) and Dorn (cached, listed under another spelling) arrive; Brisca
        // (cached) and Eira (uncached) leave.
        list_event(
            &mut app,
            GROUPTYPE_RAID,
            vec![member(a, "Aldwyn"), member(c, "Cyra"), member(d, "Dornish")],
            a,
        );
        assert_eq!(
            shown(&mut app),
            [
                UiError::s("ERR_RAID_MEMBER_ADDED_S", "Dorn"),
                UiError::s("ERR_RAID_MEMBER_REMOVED_S", "Brisca"),
            ],
            "the cached names at once"
        );
        name_event(&mut app, c, "Cyra");
        assert_eq!(
            shown(&mut app),
            [UiError::s("ERR_RAID_MEMBER_ADDED_S", "Cyra")],
            "the arrival's answer"
        );
        name_event(&mut app, e, "");
        assert_eq!(shown(&mut app), [], "the departure's name refused");
        name_event(&mut app, c, "Cyra");
        assert_eq!(shown(&mut app), [], "nothing left waiting");
    }

    /// A party list's leaver is looked up with no callback (`0x5e6d15`-`0x5e6d2b`), so a name
    /// the cache lacks shows no leave line, then or when it lands.
    #[test]
    fn an_uncached_party_leaver_shows_no_line_then_or_later() {
        const ME: u64 = 0x10;
        let (mut app, _rx) = handler_app(ME);
        let (a, f) = (0x21, 0x26);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(a, "Aldwyn".into(), None);
        list_event(&mut app, 0, vec![member(a, "Aldwyn"), member(f, "Fen")], a);
        shown(&mut app);
        list_event(&mut app, 0, vec![member(a, "Aldwyn")], a);
        assert_eq!(shown(&mut app), [], "Fen's name is still to come");
        name_event(&mut app, f, "Fen");
        assert_eq!(shown(&mut app), [], "and its answer shows nothing");
    }

    /// The raid roster writer ends with `MSG_RAID_TARGET_UPDATE` 0xff, the request for every
    /// icon, when the roster held no rows on entry (`0x4ba61c` keeps that, `0x4babf4` tests it,
    /// `0x4bac01`-`0x4bac33` send), after the rebuild's own sends, whatever the new list holds; a
    /// raid list onto a held roster and a list that is not a raid (`0x4ba550`) send none.
    #[test]
    fn a_raid_list_with_no_roster_held_asks_for_every_raid_target() {
        const ME: u64 = 0x10;
        let (mut app, rx) = handler_app(ME);
        let raid = vec![member(0x21, "Aldwyn"), member(0x22, "Brisca")];
        let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<ClientCommand> {
            rx.try_iter().collect()
        };
        let asks = |sent: &[ClientCommand]| {
            sent.iter()
                .filter(|c| matches!(c, ClientCommand::RequestRaidTargets))
                .count()
        };

        list_event(&mut app, 0, raid.clone(), ME);
        assert_eq!(asks(&sent(&rx)), 0, "a party's list");
        list_event(&mut app, GROUPTYPE_RAID, raid.clone(), ME);
        let joined = sent(&rx);
        assert_eq!(asks(&joined), 1, "the party becomes a raid");
        assert!(
            matches!(joined.last(), Some(ClientCommand::RequestRaidTargets)),
            "the last send: {joined:?}"
        );
        list_event(&mut app, GROUPTYPE_RAID, raid.clone(), ME);
        assert_eq!(asks(&sent(&rx)), 0, "a roster held");
        list_event(&mut app, GROUPTYPE_RAID, Vec::new(), ME);
        assert_eq!(
            asks(&sent(&rx)),
            0,
            "a list naming nobody, onto a roster held"
        );
        list_event(&mut app, GROUPTYPE_RAID, Vec::new(), ME);
        assert_eq!(asks(&sent(&rx)), 1, "and again, onto none");
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
        member_deactivated(
            0xdead,
            &mut group,
            Some(&store),
            |_| None,
            &NameCache::default(),
            None,
            &net,
        );
        assert!(asked(&rx).is_empty());
        assert!(!group.stats.contains_key(&0xdead));

        // The object gate alone: a roster member we hold no object for asks nothing.
        member_deactivated(
            0x1234,
            &mut group,
            None,
            |_| None,
            &NameCache::default(),
            None,
            &net,
        );
        assert!(asked(&rx).is_empty());
    }

    #[test]
    fn a_roster_new_member_is_seated_at_one_one_and_asked_for_only_when_unseen() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, errors, mut quest) = (
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
                &errors,
                &MessageSounds::default(),
                &mut quest,
                0,
                0,
                members,
                near,
                None,
                &SelfGuid::default(),
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

    /// A re-sent list that moves only a member's status rewrites the online bit of the record the
    /// member already owns (`0x4e82d0`), asks nothing, and leaves the rest of the record.
    #[test]
    fn a_resent_list_moves_a_known_members_online_bit_and_keeps_its_record() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let far = 0x22u64;
        let mut send = |group: &mut GroupState, status: u8| {
            let row = GroupMemberEntry {
                status,
                ..member(far, "Brisca")
            };
            list(
                group,
                &errors,
                &MessageSounds::default(),
                &mut quest,
                0,
                0,
                vec![row],
                far,
                None,
                &SelfGuid::default(),
                &names,
                &GuidIndex::default(),
                &net,
            );
        };

        send(&mut group, member_status::ONLINE);
        group.apply_stats(
            far,
            false,
            PartyMemberStatsInfo {
                cur_hp: Some(900),
                pet_guid: Some(0xF140_0000_0000_0077),
                ..PartyMemberStatsInfo::default()
            },
        );
        assert!(group.stats[&far].is_online(), "seated from the row");
        let _ = asked(&rx);

        send(&mut group, member_status::OFFLINE);
        let rec = &group.stats[&far];
        assert!(!rec.is_online(), "the row's byte moved the record's bit");
        assert_eq!(
            (rec.cur_hp, rec.pet_guid),
            (Some(900), Some(0xF140_0000_0000_0077)),
            "and nothing else"
        );
        assert!(asked(&rx).is_empty(), "no stats request for a known member");

        send(&mut group, member_status::ONLINE);
        assert!(group.stats[&far].is_online());
    }

    /// The group feed's mapping reaches the chat gate: `SMSG_GROUP_LIST` through its handler, the
    /// roster feed into the VM, then `SendChatMessage` through its applier. We lead a raid, so RAID
    /// goes out as RAID_LEADER (`0x49f3bf`); led by another member it goes out plain; ungrouped it
    /// is refused (`0x49f379`).
    #[test]
    fn a_group_list_through_the_feed_decides_the_raid_leaders_chat_type() {
        use super::super::pets::tests::{app_with, ME};
        use crate::net::{ChatKind, Guid, SelfPlayer};
        use benilla_ui::script::{UiScript, UnitState};
        use bevy::ecs::system::RunSystemOnce;

        let run =
            |leader: u64, members: Vec<GroupMemberEntry>| -> (Vec<ChatKind>, Vec<&'static str>) {
                let mut script = UiScript::new().unwrap();
                script.set_unit(
                    "player",
                    Some(UnitState {
                        exists: true,
                        name: Some("Me".into()),
                        ..Default::default()
                    }),
                );
                let mut app = app_with(script);
                let (tx, rx) = crossbeam_channel::unbounded();
                app.insert_resource(NetCommands(tx));
                app.init_resource::<UiErrorKeys>()
                    .init_resource::<MessageSounds>()
                    .init_resource::<QuestGiver>()
                    .init_resource::<crate::ui_chat::AfkMirror>()
                    .init_resource::<crate::target::Selection>()
                    .insert_resource(SelfGuid(Some(ME)));
                let me = app
                    .world_mut()
                    .spawn((SelfPlayer, Guid(ME), ObjectStore::default()))
                    .id();
                app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
                let group_type = if members.is_empty() {
                    0
                } else {
                    GROUPTYPE_RAID
                };
                app.world_mut()
                    .run_system_once_with(
                        on_list,
                        SessionEvent::GroupList {
                            group_type,
                            own_flags: 0,
                            members,
                            leader,
                            loot: None,
                        },
                    )
                    .expect("the group-list handler");
                app.update();
                crate::ui_chat::run_chat(app.world_mut(), r#"SendChatMessage("go", "RAID")"#)
                    .expect("lua");
                let sent = rx
                    .try_iter()
                    .filter_map(|c| match c {
                        ClientCommand::Chat { kind, .. } => Some(kind),
                        _ => None,
                    })
                    .collect();
                let refused = app
                    .world()
                    .resource::<UiErrorKeys>()
                    .snapshot()
                    .iter()
                    .map(|e| e.key)
                    .collect();
                (sent, refused)
            };
        let listed = |guid: u64, name: &str| GroupMemberEntry {
            name: name.into(),
            guid,
            status: member_status::ONLINE,
            flags: 0,
        };

        let (sent, refused) = run(ME, vec![listed(0x1001, "Aldwyn")]);
        assert_eq!(sent, vec![ChatKind::RaidLeader], "{refused:?}");
        let (sent, _) = run(0x1001, vec![listed(0x1001, "Aldwyn")]);
        assert_eq!(sent, vec![ChatKind::Raid], "another member leads");
        let (sent, refused) = run(0, vec![]);
        assert!(sent.is_empty());
        assert_eq!(refused, ["ERR_NOT_IN_RAID"]);
    }

    /// A stats packet lands its record, then writes the unit events of what it changed, against
    /// the stored record; an unknown member's old record is the empty one.
    #[test]
    fn a_stats_packet_writes_the_events_of_the_record_it_changed() {
        use crate::field_watch::FieldNotice;
        use bevy::ecs::system::RunSystemOnce;
        const MATE: u64 = 0x2001;
        let mut app = App::new();
        app.init_resource::<GroupState>()
            .init_resource::<crate::field_watch::NoticeLane>();
        let mut packet = |full: bool, info: PartyMemberStatsInfo| -> Vec<FieldNotice> {
            app.world_mut()
                .run_system_once_with(
                    on_member_stats,
                    SessionEvent::PartyMemberStats {
                        guid: MATE,
                        full,
                        info: Box::new(info),
                    },
                )
                .unwrap();
            app.world_mut()
                .resource_mut::<crate::field_watch::NoticeLane>()
                .drain()
                .collect()
        };
        let signal = |id: u16| FieldNotice::Signal { guid: MATE, id };
        assert_eq!(
            packet(
                true,
                PartyMemberStatsInfo {
                    cur_hp: Some(100),
                    level: Some(60),
                    ..Default::default()
                }
            ),
            [signal(28), signal(16)]
        );
        assert_eq!(
            packet(
                false,
                PartyMemberStatsInfo {
                    cur_hp: Some(90),
                    ..Default::default()
                }
            ),
            [signal(16)]
        );
        assert!(
            packet(
                false,
                PartyMemberStatsInfo {
                    cur_hp: Some(90),
                    ..Default::default()
                }
            )
            .is_empty(),
            "an equal resend"
        );
        // A full packet starts from an empty record: what it leaves out reads as cleared.
        assert_eq!(
            packet(
                true,
                PartyMemberStatsInfo {
                    cur_hp: Some(90),
                    ..Default::default()
                }
            ),
            [signal(28)]
        );
    }

    /// A login's group list lands while the VM in hand is not the loaded interface's (the glue's,
    /// the parked one, a bare entry VM), which defines none of `ITEM_QUALITY<n>_DESC`: the
    /// threshold line keeps the global's key and takes the word from the VM that shows it
    /// (`0x4e87c9`, `0x703bf0`). The master's name is read as the packet lands, from the player
    /// cache with no callback, which a miss leaves unasked (`0x4e878f`).
    #[test]
    fn a_login_lists_loot_threshold_reads_its_word_from_the_vm_that_shows_it() {
        use super::super::pets::tests::{app_with, ME};
        use bevy::ecs::system::RunSystemOnce;

        // The bare VM the packet lands on: no GlobalStrings at all.
        let mut app = app_with(benilla_ui::script::UiScript::new().unwrap());
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<UiErrorKeys>()
            .init_resource::<MessageSounds>()
            .init_resource::<QuestGiver>()
            .insert_resource(SelfGuid(Some(ME)));
        let aldwyn = 0x1001;
        let brisca = 0x1002;
        // Listed nowhere: a lookup with a callback would ask for it.
        let stranger = 0x1003;
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(aldwyn, "Aldwyn".into(), None);
        let mut send = |master: u64| {
            app.world_mut()
                .run_system_once_with(
                    on_list,
                    SessionEvent::GroupList {
                        group_type: 0,
                        own_flags: 0,
                        members: vec![member(aldwyn, "Aldwyn"), member(brisca, "Brisca")],
                        leader: aldwyn,
                        loot: Some(GroupLootInfo {
                            method: 2,
                            master,
                            threshold: 3,
                        }),
                    },
                )
                .expect("the group-list handler");
            app.world().resource::<UiErrorKeys>().take()
        };
        let lines = send(aldwyn);
        assert_eq!(lines.len(), 3, "{lines:?}");

        // The interface comes up; the chat feed resolves the queued lines against it.
        let real = benilla_ui::script::UiScript::new().unwrap();
        real.run(
            r#"
            ERR_SET_LOOT_MASTER = "Looting changed to master looter."
            ERR_NEW_LOOT_MASTER_S = "%s is now the loot master."
            ERR_SET_LOOT_THRESHOLD_S = "Loot threshold set to %s"
            ITEM_QUALITY3_DESC = "Rare"
            "#,
        )
        .unwrap();
        let shown: Vec<Option<String>> = lines
            .iter()
            .map(|e| {
                crate::ui_action::ui_error_text(e, &|k| real.lua().globals().get::<String>(k).ok())
            })
            .collect();
        assert_eq!(
            shown,
            [
                Some("Looting changed to master looter.".to_string()),
                Some("Aldwyn is now the loot master.".to_string()),
                Some("Loot threshold set to Rare".to_string()),
            ]
        );

        // A master the cache does not hold is no line, and no query from this lookup.
        let _ = crate::query_cache::queries(&rx);
        assert_eq!(send(stranger), []);
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "the loot lookup carries no callback, so it asks nothing"
        );
    }
}
