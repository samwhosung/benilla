//! Group session state: the `SMSG_GROUP_LIST` mirror and the party system lines. vmangos sends no
//! text for party events, so, as the reference does through `DisplayError` (`0x496720`), each
//! opcode composes its own line and `SMSG_GROUP_LIST` composes joins and leaves from what the list
//! finds held: the party slots (`0x5e6a40`) and the raid roster (`0x4ba5f0`, `0x4ba550`).

use std::collections::HashMap;

use crate::ui_action::UiError;
use benilla_protocol::messages::{
    party_operation, party_result, GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo,
};
use bevy::prelude::*;

use crate::ui_script::{UiFeed, UiInput};

mod feed;
pub(crate) mod net;
pub(crate) use feed::{
    raid_row_guid, synthetic_raid, synthetic_roster, GROUPTYPE_RAID, GROUP_MEMBER_SUBGROUP,
    PARTY_TOKENS, RAID_TOKENS,
};

pub(crate) struct UiPartyPlugin;

impl Plugin for UiPartyPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // A UI-only harness has no sound stack; the list handler queues the join chime.
        app.init_resource::<GroupState>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(
                Update,
                (
                    feed::feed_party.in_set(UiFeed),
                    feed::drain_party.after(UiInput),
                ),
            );
    }
}

/// The group session mirror: `SMSG_GROUP_LIST` as sent plus its side state, reset on disconnect.
#[derive(Resource, Default)]
pub struct GroupState {
    /// True from any `SMSG_GROUP_LIST` naming a leader until the all-zero "you left" list.
    pub in_group: bool,
    /// 0 party, 1 raid (vmangos `GroupType`, `Group/Group.h:116-120`).
    pub group_type: u8,
    /// Our subgroup (bits 0-2) and raid-assistant bit (`0x80`).
    pub own_flags: u8,
    /// The other members in wire order; the list never contains the recipient.
    pub members: Vec<GroupMemberEntry>,
    /// The leave ack, `SMSG_GROUP_UNINVITE` and `SMSG_GROUP_DESTROYED` empty the reference's party
    /// slots and leader (`0x4e84a0`, `0x4e8250(0)`) and keep its raid roster; the next list then
    /// finds no slot held.
    pub slots_emptied: bool,
    pub leader: u64,
    /// The loot tail, present whenever the list has members.
    pub loot: Option<GroupLootInfo>,
    /// The inviter's name while the invite popup is up.
    pub pending_invite: Option<String>,
    /// Each member's record by guid, seated at 1/1 and patched by `SMSG_PARTY_MEMBER_STATS`.
    pub stats: HashMap<u64, PartyMemberStatsInfo>,
    /// The marked guid per raid-target icon 0-7 (star to skull), 0 when unset.
    pub raid_targets: [u64; 8],
    /// `/partytest` sandbox: the drain applies group intents to this mirror; a real list clears it.
    pub test: bool,
    /// Our raid lockouts, the character's rather than the group's: leaving the group keeps them.
    pub saved_instances: Vec<benilla_protocol::messages::RaidInstanceEntry>,
    /// How many `SMSG_RAID_INSTANCE_INFO` answers arrived, empty ones included. The reference fires
    /// `UPDATE_INSTANCE_INFO` once per packet (`0x49e1a7`, straight from `0x49e0d8` when empty)
    /// and `RaidFrame.lua:43` ignores the first, so the Raid Info button is decided on the second.
    pub saved_instances_answers: u32,
    /// Bumped by each ready check that pops for us; a count, so a second one still fires.
    pub ready_check: u32,
    /// Bumped by every ready-check open, ours as leader included, where [`Self::ready_check`] skips
    /// the leader's (`0x4ba360`'s leader arm never reaches the `READY_CHECK` fire at `0x4ba53a`).
    pub ready_check_requests: u32,
    /// The answers forwarded to us as leader, append-only until the next request clears them.
    pub ready_check_answers: Vec<(u64, bool)>,
}

/// What one `SMSG_GROUP_LIST` shows: its lines, and whether it plays `igPlayerInviteAccept`
/// (`0x5e6c83`-`0x5e6c8f`), the join's only sound, since `ERR_JOINED_GROUP_S`'s row has none.
#[derive(Debug, Default, PartialEq)]
pub struct ListOutcome {
    pub lines: Vec<UiError>,
    pub invite_accept: bool,
}

impl GroupState {
    /// Apply one `SMSG_GROUP_LIST` and return what it shows, in the reference's order: the party
    /// slots' joins, the new-group line, the party slots' leaves, then the raid roster's lines.
    /// Kick, leave (`0x5e690b`) and disband lines come from their opcodes.
    pub fn apply_list(
        &mut self,
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
        self_guid: Option<u64>,
    ) -> ListOutcome {
        let mut out = ListOutcome::default();
        let raid = group_type == GROUPTYPE_RAID;
        // The two stores as this list finds them: the party slots unless an opcode emptied them,
        // and the raid roster, which only a list that is not a raid drops.
        let held_slots: Vec<(u64, String)> = if self.slots_emptied {
            Vec::new()
        } else {
            self.party_slots()
                .map(|m| (m.guid, m.name.clone()))
                .collect()
        };
        let raid_held = self.group_type == GROUPTYPE_RAID;
        // "Held a group" (`0x5e6aa9`): an occupied slot (`0x5e6afb`) or a raid roster (`0x5e6b29`).
        let held = !held_slots.is_empty() || raid_held;

        let own = own_flags & 0x7f;
        let seated: Vec<&GroupMemberEntry> = members
            .iter()
            .filter(|m| m.flags & 0x7f == own)
            .take(4)
            .collect();
        for m in &seated {
            if !held {
                // Seated into no group: no line, but the chime (`0x5e6dc6`).
                out.invite_accept = true;
            } else if !raid && !held_slots.iter().any(|(guid, _)| *guid == m.guid) {
                // New to a party we held (`0x5e6c19`, `0x5e6c24`).
                out.lines.push(UiError::s("ERR_JOINED_GROUP_S", &m.name));
                out.invite_accept = true;
            }
        }
        // A new group we lead: one line, for the last member listed (`0x5e6c94`-`0x5e6cc1`).
        if !held && self_guid.is_some_and(|me| me == leader) {
            if let Some(last) = members.last().filter(|m| !m.name.is_empty()) {
                out.lines.push(UiError::s("ERR_JOINED_GROUP_S", &last.name));
            }
        }
        // A party list's leaves: the held slots it no longer seats (`0x5e6cd7`-`0x5e6e09`).
        if !raid {
            for (guid, name) in &held_slots {
                if !seated.iter().any(|m| m.guid == *guid) {
                    out.lines.push(UiError::s("ERR_LEFT_GROUP_S", name));
                }
            }
        }
        if raid {
            // `0x4ba5f0`: arrivals only onto a held roster (`0x4ba725`), departures, and the
            // joined line when none was held (`0x4ba83a`).
            if raid_held {
                for m in &members {
                    if !self.members.iter().any(|old| old.guid == m.guid) {
                        out.lines
                            .push(UiError::s("ERR_RAID_MEMBER_ADDED_S", &m.name));
                    }
                }
                for old in &self.members {
                    if !members.iter().any(|m| m.guid == old.guid) {
                        out.lines
                            .push(UiError::s("ERR_RAID_MEMBER_REMOVED_S", &old.name));
                    }
                }
            } else {
                out.lines.push(UiError::key("ERR_RAID_YOU_JOINED"));
            }
        } else if raid_held {
            // `0x4ba550`: a list that is not a raid drops the held roster (`0x4ba55f`).
            out.lines.push(UiError::key("ERR_RAID_YOU_LEFT"));
        }

        // The all-zero list means "not in a group" (vmangos `Server/Packets/Group.h:257`); only
        // `leader == 0` tells it apart, since a solo leader's list is empty but names a leader.
        if leader == 0 {
            self.leave_group();
            return out;
        }
        // Raid to party clears the raid-target board, as the reference's `0x4ba550` does on the
        // raid-flag-clear leg; a disband clears it in `leave_group`.
        if group_type != 1 && self.group_type == 1 {
            self.raid_targets = [0; 8];
        }

        self.stats
            .retain(|guid, _| members.iter().any(|m| m.guid == *guid));

        self.in_group = true;
        self.group_type = group_type;
        self.own_flags = own_flags;
        self.members = members;
        self.slots_emptied = false;
        self.leader = leader;
        self.loot = loot;
        // A real list ends the sandbox; `synthetic_roster` re-raises the flag after its own call.
        self.test = false;
        out
    }

    /// The `party1..party4` slots: our own subgroup in packet order, at most four (`0x5e6baa`
    /// compares the flags under `0x7f`). A slot's member can change on any resync.
    pub fn party_slots(&self) -> impl Iterator<Item = &GroupMemberEntry> {
        let own = self.own_flags & 0x7f;
        self.members
            .iter()
            .filter(move |m| m.flags & 0x7f == own)
            .take(4)
    }

    /// `SMSG_GROUP_INVITE`: the popup plus a chat line, as the reference's `0x5e6730` does.
    pub fn apply_invited(&mut self, inviter: &str) -> Vec<UiError> {
        self.pending_invite = Some(inviter.to_string());
        vec![UiError::s("ERR_INVITED_TO_GROUP_S", inviter)]
    }

    /// `SMSG_GROUP_DECLINE`: our invitee said no; only the inviter gets it.
    pub fn apply_declined(&mut self, name: &str) -> Vec<UiError> {
        vec![UiError::s("ERR_DECLINE_GROUP_S", name)]
    }

    /// `SMSG_GROUP_UNINVITE`: we were kicked; empties the slots and prints unconditionally
    /// (`0x5e6850`).
    pub fn apply_uninvited(&mut self) -> Vec<UiError> {
        self.slots_emptied = true;
        vec![UiError::key("ERR_UNINVITE_YOU")]
    }

    /// `SMSG_GROUP_DESTROYED`: prints only while grouped (`0x5e6880` tests `0x4e86d0() != 0`), then
    /// empties the slots (`0x5e6893`). vmangos's two-member collapse sends none
    /// (`Group/Group.cpp:533`).
    pub fn apply_destroyed(&mut self) -> Vec<UiError> {
        let lines = if self.in_group {
            vec![UiError::key("ERR_GROUP_DISBANDED")]
        } else {
            Vec::new()
        };
        self.slots_emptied = true;
        lines
    }

    /// `SMSG_GROUP_SET_LEADER`; vmangos sends the full list right after (`Group::ChangeLeader`).
    pub fn apply_leader_changed(&mut self, name: &str, own_name: Option<&str>) -> Vec<UiError> {
        if own_name == Some(name) {
            vec![UiError::key("ERR_NEW_LEADER_YOU")]
        } else {
            vec![UiError::s("ERR_NEW_LEADER_S", name)]
        }
    }

    /// `SMSG_PARTY_COMMAND_RESULT` through the reference's jump table (`0x5e6a14`): one message
    /// per result, its catalog row giving text, surface and sound (result 7 is the red error line,
    /// the rest chat). Silent, as in the reference: results past 8, `OK` for an operation other
    /// than invite or leave, and an invite ack with no name (`0x5e6923`). The leave ack also
    /// empties the slots (`0x5e68f6`).
    pub fn apply_command_result(
        &mut self,
        operation: u32,
        member: &str,
        result: u32,
    ) -> Option<UiError> {
        let named = |key: &'static str| UiError::s(key, member);
        match result {
            party_result::OK => match operation {
                // The reference's guard: an invite ack with no name prints nothing.
                party_operation::INVITE if !member.is_empty() => Some(named("ERR_INVITE_PLAYER_S")),
                party_operation::LEAVE => {
                    self.slots_emptied = true;
                    Some(UiError::key("ERR_LEFT_GROUP_YOU"))
                }
                _ => None,
            },
            party_result::BAD_PLAYER_NAME => Some(named("ERR_BAD_PLAYER_NAME_S")),
            party_result::TARGET_NOT_IN_GROUP => Some(named("ERR_TARGET_NOT_IN_GROUP_S")),
            party_result::GROUP_FULL => Some(UiError::key("ERR_GROUP_FULL")),
            party_result::ALREADY_IN_GROUP => Some(named("ERR_ALREADY_IN_GROUP_S")),
            party_result::NOT_IN_GROUP => Some(UiError::key("ERR_NOT_IN_GROUP")),
            party_result::NOT_LEADER => Some(UiError::key("ERR_NOT_LEADER")),
            party_result::WRONG_FACTION => Some(UiError::key("ERR_PLAYER_WRONG_FACTION")),
            party_result::IGNORING_YOU => Some(named("ERR_IGNORING_YOU_S")),
            _ => None,
        }
    }

    /// `SMSG_PARTY_MEMBER_STATS`: a delta merges field by field, `_FULL` replaces the record.
    pub fn apply_stats(&mut self, guid: u64, full: bool, info: PartyMemberStatsInfo) {
        if full {
            self.stats.insert(guid, info);
            return;
        }
        let entry = self.stats.entry(guid).or_default();
        macro_rules! merge {
            ($($field:ident),* $(,)?) => {
                $(if info.$field.is_some() { entry.$field = info.$field; })*
            };
        }
        merge!(
            status,
            cur_hp,
            max_hp,
            power_type,
            cur_power,
            max_power,
            level,
            zone,
            position,
            pet_guid,
            pet_model_id,
            pet_cur_hp,
            pet_max_hp,
            pet_power_type,
            pet_cur_power,
            pet_max_power,
        );
        if info.auras.is_some() {
            entry.auras = info.auras;
        }
        if info.auras_negative.is_some() {
            entry.auras_negative = info.auras_negative;
        }
        if info.pet_name.is_some() {
            entry.pet_name = info.pet_name;
        }
        if info.pet_auras.is_some() {
            entry.pet_auras = info.pet_auras;
        }
        if info.pet_auras_negative.is_some() {
            entry.pet_auras_negative = info.pet_auras_negative;
        }
    }

    /// `MSG_RAID_TARGET_UPDATE` mode 0: one icon, cleared by guid 0.
    pub fn apply_raid_target(&mut self, icon: u8, guid: u64) {
        if let Some(slot) = self.raid_targets.get_mut(icon as usize) {
            *slot = guid;
        }
    }

    /// `MSG_RAID_TARGET_UPDATE` mode 1: the whole board; an absent icon is unset.
    pub fn apply_raid_target_list(&mut self, entries: &[(u8, u64)]) {
        self.raid_targets = [0; 8];
        for (icon, guid) in entries {
            self.apply_raid_target(*icon, *guid);
        }
    }

    /// A unit's mark on the Lua `GetRaidTargetIndex` scale, 1 to 8, or 0 when unmarked.
    pub fn raid_target_index(&self, guid: u64) -> u8 {
        if guid == 0 {
            return 0;
        }
        self.raid_targets
            .iter()
            .position(|g| *g == guid)
            .map_or(0, |i| i as u8 + 1)
    }

    /// The all-zero `SMSG_GROUP_LIST`: resets the group facts only, since the feed fires events on
    /// the ready-check and lockout state's edges. The destructure has no `..`, so a new field does
    /// not compile until it is sorted onto one side.
    pub(super) fn leave_group(&mut self) {
        let GroupState {
            // Group facts.
            in_group,
            group_type,
            own_flags,
            members,
            slots_emptied,
            leader,
            loot,
            stats,
            raid_targets,
            test,
            // Cleared with the group. The reference's side is untraced; vmangos tells an invitee
            // nothing on a disband (`Group::RemoveAllInvites`).
            pending_invite,
            // Session state the feed fires edges on: kept until `clear_session`.
            saved_instances: _,
            saved_instances_answers: _,
            ready_check: _,
            ready_check_requests: _,
            ready_check_answers: _,
        } = self;
        *in_group = false;
        *group_type = 0;
        *own_flags = 0;
        members.clear();
        *slots_emptied = false;
        *leader = 0;
        *loot = None;
        stats.clear();
        *raid_targets = [0; 8];
        *test = false;
        *pending_invite = None;
    }

    /// Session teardown: everything resets with the socket.
    pub fn clear_session(&mut self) {
        *self = GroupState::default();
    }

    /// `SMSG_RAID_INSTANCE_INFO`: replaces the lockouts and counts the answer, even an empty one.
    pub fn apply_raid_instance_info(
        &mut self,
        entries: Vec<benilla_protocol::messages::RaidInstanceEntry>,
    ) {
        self.saved_instances = entries;
        self.saved_instances_answers = self.saved_instances_answers.wrapping_add(1);
    }

    /// `MSG_RAID_READY_CHECK`, open form, echoed to the whole group: `0x4ba360` splits on the
    /// leader guid, and only a non-leader prints the leader's line and gets the popup. Both arms
    /// count the request and start a fresh answer log.
    pub fn apply_ready_check_request(&mut self, we_lead: bool) -> Vec<UiError> {
        self.ready_check_requests = self.ready_check_requests.wrapping_add(1);
        self.ready_check_answers.clear();
        if we_lead {
            return Vec::new();
        }
        self.ready_check = self.ready_check.wrapping_add(1);
        // The reference prints its name cache's entry for the leader; the roster holds the same.
        let leader = self
            .members
            .iter()
            .find(|m| m.guid == self.leader)
            .map(|m| m.name.as_str())
            .unwrap_or("");
        vec![UiError::s("ERR_RAID_LEADER_READY_CHECK_START_S", leader)]
    }

    /// `MSG_RAID_READY_CHECK`, answer form: one member's answer, forwarded to the leader alone.
    pub fn apply_ready_check_answer(&mut self, guid: u64, ready: bool) {
        self.ready_check_answers.push((guid, ready));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, guid: u64) -> GroupMemberEntry {
        GroupMemberEntry {
            name: name.into(),
            guid,
            status: 1,
            flags: 0,
        }
    }

    #[test]
    fn an_unchanged_lockout_list_still_counts_as_an_answer() {
        let mut g = GroupState::default();
        assert_eq!(g.saved_instances_answers, 0, "nobody has asked yet");

        g.apply_raid_instance_info(Vec::new());
        assert_eq!(g.saved_instances_answers, 1);
        g.apply_raid_instance_info(Vec::new());
        assert_eq!(
            g.saved_instances_answers, 2,
            "the second empty answer is the one the button is decided on"
        );

        g.apply_raid_instance_info(vec![benilla_protocol::messages::RaidInstanceEntry {
            map: 409,
            reset: 86_400,
            instance: 7,
        }]);
        assert_eq!(g.saved_instances_answers, 3);
        assert_eq!(g.saved_instances.len(), 1, "and the list is the new one");

        g.clear_session();
        assert_eq!(g.saved_instances_answers, 0);
        assert!(g.saved_instances.is_empty());
    }

    #[test]
    fn leaving_the_group_keeps_the_session_tickets() {
        let lockout = benilla_protocol::messages::RaidInstanceEntry {
            map: 409,
            reset: 86_400,
            instance: 7,
        };
        let mut g = GroupState {
            in_group: true,
            leader: 0xA11CE,
            members: vec![member("Alice", 0xA11CE)],
            ..Default::default()
        };
        g.raid_targets[0] = 0xA11CE;
        g.apply_ready_check_request(false);
        g.apply_raid_instance_info(Vec::new());
        g.apply_raid_instance_info(vec![lockout]);
        assert_eq!((g.ready_check, g.ready_check_requests), (1, 1));
        assert_eq!(g.saved_instances_answers, 2);

        g.apply_list(0, 0, vec![], 0, None, None);

        assert!(!g.in_group);
        assert!(g.members.is_empty());
        assert_eq!(g.leader, 0);
        assert_eq!(g.raid_targets, [0; 8]);
        assert_eq!(
            (g.ready_check, g.ready_check_requests),
            (1, 1),
            "no READY_CHECK edge"
        );
        assert_eq!(g.saved_instances_answers, 2, "no UPDATE_INSTANCE_INFO edge");
        assert_eq!(
            g.saved_instances,
            vec![lockout],
            "the lockouts are the character's"
        );

        g.clear_session();
        assert_eq!((g.ready_check, g.saved_instances_answers), (0, 0));
        assert!(g.saved_instances.is_empty());
    }

    #[test]
    fn the_open_form_pops_for_a_member_and_stays_quiet_for_the_leader() {
        let mut g = GroupState {
            leader: 0xA11CE,
            members: vec![member("Alice", 0xA11CE), member("Bob", 0xB0B)],
            ..Default::default()
        };
        g.apply_ready_check_answer(0xB0B, true);

        assert_eq!(
            g.apply_ready_check_request(false),
            vec![UiError::s("ERR_RAID_LEADER_READY_CHECK_START_S", "Alice")]
        );
        assert_eq!((g.ready_check, g.ready_check_requests), (1, 1));
        assert!(
            g.ready_check_answers.is_empty(),
            "a request starts a fresh log"
        );

        assert!(
            g.apply_ready_check_request(true).is_empty(),
            "the leader's echo"
        );
        assert_eq!(
            (g.ready_check, g.ready_check_requests),
            (1, 2),
            "no popup for the leader"
        );

        g.apply_ready_check_answer(0xB0B, false);
        assert_eq!(g.ready_check_answers, vec![(0xB0B, false)]);
    }

    /// Our own guid, for the lists that ask whether we lead.
    const ME: u64 = 0x5E1F;

    fn sub(name: &str, guid: u64, subgroup: u8) -> GroupMemberEntry {
        GroupMemberEntry {
            flags: subgroup,
            ..member(name, guid)
        }
    }

    fn lines(lines: Vec<UiError>, invite_accept: bool) -> ListOutcome {
        ListOutcome {
            lines,
            invite_accept,
        }
    }

    #[test]
    fn joining_a_party_names_nobody_already_in_it() {
        let mut g = GroupState::default();
        let party = vec![member("Alice", 1), member("Bob", 2)];
        assert_eq!(
            g.apply_list(0, 0, party.clone(), 1, None, Some(ME)),
            lines(vec![], true),
            "seated into no group: the chime alone"
        );
        assert!(g.in_group);

        let grown = [party.clone(), vec![member("Carol", 3)]].concat();
        assert_eq!(
            g.apply_list(0, 0, grown.clone(), 1, None, Some(ME)),
            lines(vec![UiError::s("ERR_JOINED_GROUP_S", "Carol")], true),
            "a member new to a party we held"
        );
        assert_eq!(
            g.apply_list(0, 0, party.clone(), 1, None, Some(ME)),
            lines(vec![UiError::s("ERR_LEFT_GROUP_S", "Carol")], false)
        );
        // An unchanged resync (loot/status churn re-sends the list) shows nothing.
        assert_eq!(
            g.apply_list(0, 0, party, 1, None, Some(ME)),
            ListOutcome::default()
        );
    }

    #[test]
    fn a_group_we_form_names_the_member_the_list_names_last() {
        let mut g = GroupState::default();
        // A solo leader's list names nobody, and holds no group.
        assert_eq!(
            g.apply_list(0, 0, vec![], ME, None, Some(ME)),
            ListOutcome::default()
        );
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], ME, None, Some(ME)),
            lines(vec![UiError::s("ERR_JOINED_GROUP_S", "Alice")], true)
        );

        let mut g = GroupState::default();
        assert_eq!(
            g.apply_list(
                0,
                0,
                vec![member("Alice", 1), member("Bob", 2)],
                ME,
                None,
                Some(ME)
            ),
            lines(vec![UiError::s("ERR_JOINED_GROUP_S", "Bob")], true),
            "one line, for the last member"
        );
        // Not ours: nobody is named.
        let mut g = GroupState::default();
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], 1, None, Some(ME)),
            lines(vec![], true)
        );
    }

    #[test]
    fn an_opcode_that_ends_the_group_leaves_the_empty_list_nobody_to_name() {
        type Ending = fn(&mut GroupState) -> Vec<UiError>;
        let endings: [(Ending, &str); 3] = [
            (
                |g| {
                    g.apply_command_result(party_operation::LEAVE, "Us", party_result::OK)
                        .into_iter()
                        .collect()
                },
                "ERR_LEFT_GROUP_YOU",
            ),
            (|g| g.apply_uninvited(), "ERR_UNINVITE_YOU"),
            (|g| g.apply_destroyed(), "ERR_GROUP_DISBANDED"),
        ];
        for (end, key) in endings {
            let mut g = GroupState::default();
            g.apply_list(
                0,
                0,
                vec![member("Alice", 1), member("Bob", 2)],
                1,
                None,
                None,
            );
            assert_eq!(end(&mut g), vec![UiError::key(key)]);
            assert_eq!(
                g.apply_list(0, 0, Vec::new(), 0, None, None),
                ListOutcome::default(),
                "{key} emptied the slots"
            );
            assert!(!g.in_group);
        }

        // Ungrouped, the disband is silent (the `0x4e86d0` gate).
        assert!(GroupState::default().apply_destroyed().is_empty());

        // vmangos's two-member collapse sends nothing first: the survivor sees the leaver go.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None, None);
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None, None),
            lines(vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")], false)
        );
    }

    #[test]
    fn joining_a_raid_prints_the_joined_line_alone() {
        let raid = vec![sub("Alice", 1, 0), sub("Bob", 2, 1), sub("Carol", 3, 1)];
        let mut g = GroupState::default();
        assert_eq!(
            g.apply_list(1, 0, raid.clone(), 1, None, Some(ME)),
            lines(vec![UiError::key("ERR_RAID_YOU_JOINED")], true),
            "Alice shares our subgroup, so the chime plays"
        );
        let mut g = GroupState::default();
        assert_eq!(
            g.apply_list(1, 2, raid, 1, None, Some(ME)),
            lines(vec![UiError::key("ERR_RAID_YOU_JOINED")], false),
            "nobody in our subgroup, no chime"
        );
    }

    #[test]
    fn a_raid_list_names_arrivals_and_departures_in_raid_words_alone() {
        let mut g = GroupState::default();
        g.apply_list(
            1,
            0,
            vec![sub("Alice", 1, 0), sub("Bob", 2, 1)],
            1,
            None,
            None,
        );
        // Dave lands in our subgroup: the raid line, no party line and no chime.
        assert_eq!(
            g.apply_list(
                1,
                0,
                vec![sub("Alice", 1, 0), sub("Bob", 2, 1), sub("Dave", 4, 0)],
                1,
                None,
                None
            ),
            lines(vec![UiError::s("ERR_RAID_MEMBER_ADDED_S", "Dave")], false)
        );
        assert_eq!(
            g.apply_list(
                1,
                0,
                vec![sub("Alice", 1, 0), sub("Dave", 4, 0)],
                1,
                None,
                None
            ),
            lines(vec![UiError::s("ERR_RAID_MEMBER_REMOVED_S", "Bob")], false)
        );
    }

    #[test]
    fn leaving_a_raid_prints_the_left_line_alone() {
        let raid = vec![sub("Alice", 1, 0), sub("Bob", 2, 1), sub("Carol", 3, 0)];
        let mut g = GroupState::default();
        g.apply_list(1, 0, raid.clone(), 1, None, None);
        assert_eq!(
            g.apply_command_result(party_operation::LEAVE, "Us", party_result::OK),
            Some(UiError::key("ERR_LEFT_GROUP_YOU"))
        );
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None, None).lines,
            vec![UiError::key("ERR_RAID_YOU_LEFT")]
        );

        // Removed with no opcode first: our subgroup's slots go in party words, then the raid.
        let mut g = GroupState::default();
        g.apply_list(1, 0, raid, 1, None, None);
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None, None).lines,
            vec![
                UiError::s("ERR_LEFT_GROUP_S", "Alice"),
                UiError::s("ERR_LEFT_GROUP_S", "Carol"),
                UiError::key("ERR_RAID_YOU_LEFT"),
            ]
        );
    }

    #[test]
    fn a_party_becoming_a_raid_and_back_prints_only_the_raid_edges() {
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None, None);
        assert_eq!(
            g.apply_list(1, 0, vec![member("Alice", 1)], 1, None, None),
            lines(vec![UiError::key("ERR_RAID_YOU_JOINED")], false)
        );
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], 1, None, None),
            lines(vec![UiError::key("ERR_RAID_YOU_LEFT")], false)
        );
    }

    #[test]
    fn party_slots_filter_to_own_subgroup() {
        let mut g = GroupState::default();
        let mut m2 = member("Bob", 2);
        m2.flags = 0x01; // subgroup 1
        let mut m3 = member("Carol", 3);
        m3.flags = 0x80; // subgroup 0, assistant: the filter ignores 0x80
        g.apply_list(1, 0x00, vec![member("Alice", 1), m2, m3], 1, None, None);
        let slots: Vec<&str> = g.party_slots().map(|m| m.name.as_str()).collect();
        assert_eq!(slots, vec!["Alice", "Carol"]);
    }

    #[test]
    fn invite_lines() {
        let mut g = GroupState::default();
        assert_eq!(
            g.apply_invited("Bob"),
            vec![UiError::s("ERR_INVITED_TO_GROUP_S", "Bob")]
        );
        assert_eq!(g.pending_invite.as_deref(), Some("Bob"));
        assert_eq!(
            g.apply_command_result(party_operation::INVITE, "Carol", party_result::OK),
            Some(UiError::s("ERR_INVITE_PLAYER_S", "Carol"))
        );
        assert_eq!(
            g.apply_command_result(
                party_operation::INVITE,
                "Carol",
                party_result::ALREADY_IN_GROUP
            ),
            Some(UiError::s("ERR_ALREADY_IN_GROUP_S", "Carol"))
        );
        assert_eq!(
            g.apply_command_result(party_operation::INVITE, "Xz", party_result::BAD_PLAYER_NAME),
            Some(UiError::s("ERR_BAD_PLAYER_NAME_S", "Xz"))
        );
        assert_eq!(
            g.apply_declined("Carol"),
            vec![UiError::s("ERR_DECLINE_GROUP_S", "Carol")]
        );
    }

    #[test]
    fn leader_lines() {
        let mut g = GroupState::default();
        assert_eq!(
            g.apply_leader_changed("Alice", Some("Benilla")),
            vec![UiError::s("ERR_NEW_LEADER_S", "Alice")]
        );
        assert_eq!(
            g.apply_leader_changed("Benilla", Some("Benilla")),
            vec![UiError::key("ERR_NEW_LEADER_YOU")]
        );
    }

    #[test]
    fn stats_merge_and_retention() {
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None, None);
        g.apply_stats(
            1,
            false,
            PartyMemberStatsInfo {
                cur_hp: Some(50),
                max_hp: Some(100),
                ..Default::default()
            },
        );
        g.apply_stats(
            1,
            false,
            PartyMemberStatsInfo {
                cur_hp: Some(60),
                ..Default::default()
            },
        );
        let s = g.stats.get(&1).unwrap();
        assert_eq!(s.cur_hp, Some(60));
        assert_eq!(s.max_hp, Some(100), "delta merge keeps unmentioned fields");

        g.apply_stats(
            1,
            true,
            PartyMemberStatsInfo {
                cur_hp: Some(70),
                ..Default::default()
            },
        );
        let s = g.stats.get(&1).unwrap();
        assert_eq!(s.cur_hp, Some(70));
        assert_eq!(s.max_hp, None, "FULL replaces the snapshot outright");

        g.apply_list(0, 0, Vec::new(), 0, None, None);
        assert!(g.stats.is_empty());
    }

    #[test]
    fn raid_target_board() {
        let mut g = GroupState::default();
        g.apply_raid_target(7, 0x99); // skull
        assert_eq!(g.raid_targets[7], 0x99);
        g.apply_raid_target(7, 0); // clear
        assert_eq!(g.raid_targets[7], 0);
        g.apply_raid_target(3, 0x11);
        g.apply_raid_target_list(&[(0, 0x22), (5, 0x33)]);
        assert_eq!(g.raid_targets[0], 0x22);
        assert_eq!(g.raid_targets[5], 0x33);
        assert_eq!(g.raid_targets[3], 0, "the list form resets absent icons");
    }

    #[test]
    fn a_raid_to_party_conversion_clears_the_raid_target_board() {
        let mut g = GroupState::default();
        g.apply_list(1, 0, vec![member("Ally", 0x22)], 0x22, None, None);
        g.apply_raid_target(7, 0x22);
        assert_eq!(g.raid_targets[7], 0x22, "marked while a raid");

        g.apply_list(
            1,
            0,
            vec![member("Ally", 0x22), member("Bee", 0x33)],
            0x22,
            None,
            None,
        );
        assert_eq!(
            g.raid_targets[7], 0x22,
            "a roster change inside a raid keeps the marks"
        );

        g.apply_list(0, 0, vec![member("Ally", 0x22)], 0x22, None, None);
        assert_eq!(
            g.raid_targets, [0; 8],
            "the raid flag clearing empties the board"
        );
    }

    /// Asserted by message id, since the English cannot tell a right key from a wrong one.
    #[test]
    fn every_party_result_names_the_message_id_the_reference_pushes() {
        const TABLE: &[(u32, u16, bool)] = &[
            (party_result::BAD_PLAYER_NAME, 0x47, true),
            (party_result::TARGET_NOT_IN_GROUP, 0x49, true),
            (party_result::GROUP_FULL, 0x4a, false),
            (party_result::ALREADY_IN_GROUP, 0x3d, true),
            (party_result::NOT_IN_GROUP, 0x48, false),
            (party_result::NOT_LEADER, 0x4b, false),
            (party_result::WRONG_FACTION, 0xff, false),
            (party_result::IGNORING_YOU, 0x13d, true),
        ];
        let mut g = GroupState::default();
        for &(result, id, takes_name) in TABLE {
            let msg = g
                .apply_command_result(party_operation::INVITE, "Zed", result)
                .unwrap_or_else(|| panic!("result {result} showed nothing"));
            let row = benilla_ui::messages::by_key(msg.key)
                .unwrap_or_else(|| panic!("result {result} named {}, not a catalog row", msg.key));
            assert_eq!(row.id, id, "result {result} -> {} (id {})", msg.key, row.id);
            // The arms the reference passes the name to are exactly the `_S` keys.
            assert_eq!(
                msg.arg_s().is_some(),
                takes_name,
                "result {result} name fill"
            );
            assert_eq!(
                msg.key.ends_with("_S"),
                takes_name,
                "result {result} _S suffix"
            );
        }

        // The two `OK` arms, where the operation decides.
        let mut ok = |op| g.apply_command_result(op, "Zed", party_result::OK);
        assert_eq!(
            benilla_ui::messages::by_key(ok(party_operation::INVITE).unwrap().key)
                .unwrap()
                .id,
            0x3a
        );
        assert_eq!(
            benilla_ui::messages::by_key(ok(party_operation::LEAVE).unwrap().key)
                .unwrap()
                .id,
            0x42
        );
    }

    #[test]
    fn the_wrong_faction_refusal_is_the_red_line_and_the_rest_are_chat() {
        use benilla_ui::messages::MsgKind;
        let mut g = GroupState::default();
        let mut kind = |r| {
            benilla_ui::messages::kind_of(
                g.apply_command_result(party_operation::INVITE, "Zed", r)
                    .unwrap()
                    .key,
            )
        };
        assert_eq!(kind(party_result::WRONG_FACTION), MsgKind::Error);
        for r in [
            party_result::BAD_PLAYER_NAME,
            party_result::TARGET_NOT_IN_GROUP,
            party_result::GROUP_FULL,
            party_result::ALREADY_IN_GROUP,
            party_result::NOT_IN_GROUP,
            party_result::NOT_LEADER,
            party_result::IGNORING_YOU,
        ] {
            assert_eq!(kind(r), MsgKind::Chat, "result {r} should be a chat line");
        }
    }

    /// Three inputs show nothing: the reference's default arm (`0x5e6a06`) returns with no call.
    #[test]
    fn the_silent_inputs_show_nothing() {
        let mut g = GroupState::default();
        // 1: every result past the table (`dec eax; cmp eax,7; ja`, unsigned).
        for r in [9u32, 10, 42, u32::MAX] {
            assert!(
                g.apply_command_result(party_operation::INVITE, "Zed", r)
                    .is_none(),
                "result {r} must be silent"
            );
        }
        // 2: `OK` for an operation other than invite or leave.
        for op in [1u32, 3, 99] {
            assert!(g
                .apply_command_result(op, "Zed", party_result::OK)
                .is_none());
        }
        // 3: an invite ack with no name (`0x5e6923`).
        assert!(g
            .apply_command_result(party_operation::INVITE, "", party_result::OK)
            .is_none());
        // A named one still prints: the guard is the name.
        assert!(g
            .apply_command_result(party_operation::INVITE, "Zed", party_result::OK)
            .is_some());
    }

    #[test]
    fn party_result_keys_resolve_to_the_real_1_12_sentences() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

        let mut g = GroupState::default();
        for r in [
            party_result::BAD_PLAYER_NAME,
            party_result::TARGET_NOT_IN_GROUP,
            party_result::GROUP_FULL,
            party_result::ALREADY_IN_GROUP,
            party_result::NOT_IN_GROUP,
            party_result::NOT_LEADER,
            party_result::WRONG_FACTION,
            party_result::IGNORING_YOU,
        ] {
            let msg = g
                .apply_command_result(party_operation::INVITE, "Zed", r)
                .unwrap();
            let text: String = s.lua().globals().get(msg.key).expect(msg.key);
            assert!(!text.is_empty(), "{} resolves empty", msg.key);
            assert_eq!(
                text.contains("%s"),
                msg.arg_s().is_some(),
                "{} vs its fill",
                msg.key
            );
        }
    }

    #[test]
    fn the_roster_lines_resolve_and_carry_their_catalog_row() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

        let mut g = GroupState::default();
        let mut lines = Vec::new();
        // Driven through every producer, so a new line cannot skip the checks below.
        let mut list =
            |g: &mut GroupState, group_type: u8, members: &[(&str, u64)], leader: u64| {
                let members = members.iter().map(|(n, guid)| member(n, *guid)).collect();
                lines.extend(
                    g.apply_list(group_type, 0, members, leader, None, None)
                        .lines,
                );
            };
        list(&mut g, 0, &[("Alice", 1)], 1);
        list(&mut g, 0, &[("Alice", 1), ("Bob", 2)], 1);
        list(&mut g, 0, &[("Alice", 1)], 1);
        list(&mut g, 1, &[("Alice", 1)], 1);
        list(&mut g, 1, &[("Alice", 1), ("Dave", 4)], 1);
        list(&mut g, 1, &[("Alice", 1)], 1);
        lines.extend(g.apply_invited("Bob"));
        lines.extend(g.apply_declined("Carol"));
        lines.extend(g.apply_leader_changed("Alice", Some("Us")));
        lines.extend(g.apply_leader_changed("Us", Some("Us")));
        lines.extend(g.apply_uninvited());
        lines.extend(g.apply_destroyed());
        g.leader = 1;
        lines.extend(g.apply_ready_check_request(false));
        lines.extend(g.apply_list(0, 0, Vec::new(), 0, None, None).lines);

        let mut keys: Vec<&str> = lines.iter().map(|m| m.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(
            keys,
            [
                "ERR_DECLINE_GROUP_S",
                "ERR_GROUP_DISBANDED",
                "ERR_INVITED_TO_GROUP_S",
                "ERR_JOINED_GROUP_S",
                "ERR_LEFT_GROUP_S",
                "ERR_NEW_LEADER_S",
                "ERR_NEW_LEADER_YOU",
                "ERR_RAID_LEADER_READY_CHECK_START_S",
                "ERR_RAID_MEMBER_ADDED_S",
                "ERR_RAID_MEMBER_REMOVED_S",
                "ERR_RAID_YOU_JOINED",
                "ERR_RAID_YOU_LEFT",
                "ERR_UNINVITE_YOU",
            ],
            "all thirteen keys this window can raise are exercised below"
        );

        for msg in &lines {
            let text: String = s.lua().globals().get(msg.key).expect(msg.key);
            assert!(!text.is_empty(), "{} resolves empty", msg.key);
            assert!(
                benilla_ui::messages::by_key(msg.key).is_some(),
                "{} is not a catalog row, so its surface and sound would be a guess",
                msg.key
            );
            assert_eq!(
                text.contains("%s"),
                msg.arg_s().is_some(),
                "{} vs its fill",
                msg.key
            );
        }
        assert_eq!(
            benilla_ui::messages::by_key("ERR_DECLINE_GROUP_S").and_then(|r| r.sound),
            Some("igPlayerInviteDecline"),
            "the cue the chat-log path was dropping"
        );
    }
}
