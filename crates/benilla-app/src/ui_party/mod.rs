//! Group session state: the `SMSG_GROUP_LIST` mirror and the party system lines. vmangos sends no
//! text for party events, so, as the reference does through `DisplayError` (`0x496720`), each
//! opcode composes its own line and the `SMSG_GROUP_LIST` roster diff composes joins and leaves.

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
        app.init_resource::<GroupState>().add_systems(
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

impl GroupState {
    /// Apply one `SMSG_GROUP_LIST` and return its roster diff's lines. As in the reference
    /// (`0x5e6c19`, `0x5e6d37`) the diff starts from an empty cache and runs both ways, the
    /// all-zero list included; kick, leave (`0x5e690b`) and disband lines come from their opcodes.
    pub fn apply_list(
        &mut self,
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
    ) -> Vec<UiError> {
        let mut lines = Vec::new();
        // The all-zero list means "not in a group" (vmangos `Server/Packets/Group.h:257`); only
        // `leader == 0` tells it apart, since a solo leader's list is empty but names a leader.
        let leaving = leader == 0;
        // Raid wording when either side of the transition is a raid. The reference's raid lines
        // come from its raid roster rebuild `0x4ba5f0` and raid-leave leg `0x4ba550`, whose
        // triggers this diff does not follow.
        let (added, removed) = if group_type == 1 || (leaving && self.group_type == 1) {
            ("ERR_RAID_MEMBER_ADDED_S", "ERR_RAID_MEMBER_REMOVED_S")
        } else {
            ("ERR_JOINED_GROUP_S", "ERR_LEFT_GROUP_S")
        };
        for m in &members {
            if !self.members.iter().any(|old| old.guid == m.guid) {
                lines.push(UiError::s(added, &m.name));
            }
        }
        for old in &self.members {
            if !members.iter().any(|m| m.guid == old.guid) {
                lines.push(UiError::s(removed, &old.name));
            }
        }
        if leaving {
            self.leave_group();
            return lines;
        }
        // Entering a raid prints `ERR_RAID_YOU_JOINED`, as the reference does when its raid
        // roster was empty (`0x4ba83a`).
        if group_type == 1 && self.group_type != 1 {
            lines.push(UiError::key("ERR_RAID_YOU_JOINED"));
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
        self.leader = leader;
        self.loot = loot;
        // A real list ends the sandbox; `synthetic_roster` re-raises the flag after its own call.
        self.test = false;
        lines
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

    /// `SMSG_GROUP_UNINVITE`: we were kicked; prints unconditionally (`0x5e6850`).
    pub fn apply_uninvited(&mut self) -> Vec<UiError> {
        vec![UiError::key("ERR_UNINVITE_YOU")]
    }

    /// `SMSG_GROUP_DESTROYED`: prints only while grouped (`0x5e6880` tests `0x4e86d0() != 0`).
    /// vmangos's two-member collapse sends none (`Group/Group.cpp:533`).
    pub fn apply_destroyed(&mut self) -> Vec<UiError> {
        if self.in_group {
            vec![UiError::key("ERR_GROUP_DISBANDED")]
        } else {
            Vec::new()
        }
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
    /// than invite or leave, and an invite ack with no name (`0x5e6923`).
    pub fn apply_command_result(
        &self,
        operation: u32,
        member: &str,
        result: u32,
    ) -> Option<UiError> {
        let named = |key: &'static str| UiError::s(key, member);
        match result {
            party_result::OK => match operation {
                // The reference's guard: an invite ack with no name prints nothing.
                party_operation::INVITE if !member.is_empty() => Some(named("ERR_INVITE_PLAYER_S")),
                party_operation::LEAVE => Some(UiError::key("ERR_LEFT_GROUP_YOU")),
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

        g.apply_list(0, 0, vec![], 0, None);

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

    #[test]
    fn join_lines_come_from_roster_diffs() {
        let mut g = GroupState::default();
        // Our first list prints Alice's join: the reference's cache starts empty (`0x5e6c19`).
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], 1, None),
            vec![UiError::s("ERR_JOINED_GROUP_S", "Alice")]
        );
        assert!(g.in_group);
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1), member("Carol", 3)], 1, None),
            vec![UiError::s("ERR_JOINED_GROUP_S", "Carol")]
        );
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], 1, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Carol")]
        );
        // An unchanged resync (loot/status churn re-sends the list) prints nothing.
        assert!(g
            .apply_list(0, 0, vec![member("Alice", 1)], 1, None)
            .is_empty());
    }

    #[test]
    fn leave_kick_disband_lines_stack_per_opcode() {
        // Voluntary: the leave ack prints; the empty list adds the leave lines.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(
            g.apply_command_result(party_operation::LEAVE, "Us", party_result::OK),
            Some(UiError::key("ERR_LEFT_GROUP_YOU"))
        );
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")]
        );
        assert!(!g.in_group);

        // Kicked: `SMSG_GROUP_UNINVITE` prints unconditionally.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(g.apply_uninvited(), vec![UiError::key("ERR_UNINVITE_YOU")]);
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")]
        );

        // Destroyed while grouped: it prints, and the empty list still runs its diff.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(
            g.apply_destroyed(),
            vec![UiError::key("ERR_GROUP_DISBANDED")]
        );
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")]
        );
        // Ungrouped, it is silent (the `0x4e86d0` gate).
        assert!(g.apply_destroyed().is_empty());

        // vmangos's two-member collapse sends no `SMSG_GROUP_DESTROYED`: just the leave line.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")]
        );
    }

    #[test]
    fn raid_wording_and_conversion() {
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        // Party to raid: same roster, the type flips.
        assert_eq!(
            g.apply_list(1, 0, vec![member("Alice", 1)], 1, None),
            vec![UiError::key("ERR_RAID_YOU_JOINED")]
        );
        assert_eq!(
            g.apply_list(1, 0, vec![member("Alice", 1), member("Dave", 4)], 1, None),
            vec![UiError::s("ERR_RAID_MEMBER_ADDED_S", "Dave")]
        );
        let mut lines = g.apply_list(1, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(
            lines.pop(),
            Some(UiError::s("ERR_RAID_MEMBER_REMOVED_S", "Dave"))
        );
        // Leaving the raid: the empty list's diff keeps the departed raid's wording.
        assert_eq!(
            g.apply_command_result(party_operation::LEAVE, "Us", party_result::OK),
            Some(UiError::key("ERR_LEFT_GROUP_YOU"))
        );
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_RAID_MEMBER_REMOVED_S", "Alice")]
        );
    }

    #[test]
    fn party_slots_filter_to_own_subgroup() {
        let mut g = GroupState::default();
        let mut m2 = member("Bob", 2);
        m2.flags = 0x01; // subgroup 1
        let mut m3 = member("Carol", 3);
        m3.flags = 0x80; // subgroup 0, assistant: the filter ignores 0x80
        g.apply_list(1, 0x00, vec![member("Alice", 1), m2, m3], 1, None);
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
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
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

        g.apply_list(0, 0, Vec::new(), 0, None);
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
        g.apply_list(1, 0, vec![member("Ally", 0x22)], 0x22, None);
        g.apply_raid_target(7, 0x22);
        assert_eq!(g.raid_targets[7], 0x22, "marked while a raid");

        g.apply_list(
            1,
            0,
            vec![member("Ally", 0x22), member("Bee", 0x33)],
            0x22,
            None,
        );
        assert_eq!(
            g.raid_targets[7], 0x22,
            "a roster change inside a raid keeps the marks"
        );

        g.apply_list(0, 0, vec![member("Ally", 0x22)], 0x22, None);
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
        let g = GroupState::default();
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
        let ok = |op| g.apply_command_result(op, "Zed", party_result::OK);
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
        let g = GroupState::default();
        let kind = |r| {
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
        let g = GroupState::default();
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

        let g = GroupState::default();
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
        lines.extend(g.apply_list(0, 0, vec![member("Alice", 1), member("Bob", 2)], 1, None));
        lines.extend(g.apply_list(0, 0, vec![member("Alice", 1)], 1, None));
        lines.extend(g.apply_list(1, 0, vec![member("Alice", 1), member("Dave", 4)], 1, None));
        lines.extend(g.apply_list(1, 0, vec![member("Alice", 1)], 1, None));
        lines.extend(g.apply_invited("Bob"));
        lines.extend(g.apply_declined("Carol"));
        lines.extend(g.apply_leader_changed("Alice", Some("Us")));
        lines.extend(g.apply_leader_changed("Us", Some("Us")));
        lines.extend(g.apply_uninvited());
        lines.extend(g.apply_destroyed());
        g.leader = 1;
        lines.extend(g.apply_ready_check_request(false));

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
                "ERR_UNINVITE_YOU",
            ],
            "all twelve keys this window can raise are exercised below"
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
