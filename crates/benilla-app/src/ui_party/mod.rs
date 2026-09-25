//! Group/party session state — the `SMSG_GROUP_LIST` wire mirror and the client-composed party
//! system lines.
//!
//! The 1.12 server sends **no text** for party events (vmangos, exhaustively grepped): joins and
//! leaves appear only as roster diffs on `SMSG_GROUP_LIST`, a kick additionally announces itself
//! to the kicked player as the empty-body `SMSG_GROUP_UNINVITE`, and `SMSG_PARTY_COMMAND_RESULT`
//! acks only the acting player. The real client composes every "%s joins the party." line
//! engine-side from those packets against the GlobalStrings templates (the FrameXML never touches
//! the `ERR_*` group strings; the mechanism is the reference's errorId→GlobalStrings display,
//! `CGGameUI::DisplayError` 0x496720). benilla does the same here: [`GroupState`] mirrors the
//! wire, and its `apply_*` methods return the finished lines for the net drain to push as
//! CHAT_MSG_SYSTEM. In the mapping, each opcode prints its own line and the
//! GROUP_LIST diff prints the rest — there is NO empty-list state machine.
//!
//! The line law:
//! - GROUP_LIST runs an **ungated** two-way roster diff: new guid → "%s joins the party."
//!   (`0x5e6c19` — including everyone already there on a first roster), vanished guid →
//!   "%s leaves the party." (`0x5e6d37` — including the whole roster on our own empty list).
//! - "You leave the group." comes ONLY from `SMSG_PARTY_COMMAND_RESULT(LEAVE, ok)` (`0x5e690b`).
//! - `SMSG_GROUP_UNINVITE` (kick) → "You have been removed from the group.", unconditional.
//! - `SMSG_GROUP_DESTROYED` → "Your group has been disbanded.", gated on being grouped. The
//!   vmangos 2-man collapse sends no GROUP_DESTROYED (`Disband(hideDestroy=true)`,
//!   `Group.cpp:533`), so the survivor sees only the leave-diff line — rendered faithfully
//!   (the 0086 server-divergence precedent).

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

/// The party/raid session mirror — `SMSG_GROUP_LIST` verbatim plus the invite/stats side-state.
/// Filled by the net drain's group arms, cleared on disconnect beside the other per-login
/// resources. The party *frames* (phase 2) read this through the merged view:
/// a streamed member's live `ObjectStore` wins; the [`Self::stats`] snapshot covers the rest.
#[derive(Resource, Default)]
pub struct GroupState {
    /// True while we hold a roster (any `SMSG_GROUP_LIST` with `leader != 0`; the all-zero
    /// "you left" shape flips it back off).
    pub in_group: bool,
    /// `0` party, `1` raid (`GROUPTYPE_*`, vmangos `Group.h:116-120`).
    pub group_type: u8,
    /// Our own subgroup (bits 0-2) + raid-assistant bit (`0x80`).
    pub own_flags: u8,
    /// The *other* members, wire order (the recipient never appears in its own list).
    pub members: Vec<GroupMemberEntry>,
    pub leader: u64,
    /// The loot tail — present whenever the wire list was non-empty.
    pub loot: Option<GroupLootInfo>,
    /// The inviter's name while an invite popup should be up (`SMSG_GROUP_INVITE` set it; accept/
    /// decline clears it — phase 2 wires the StaticPopup on top).
    pub pending_invite: Option<String>,
    /// Out-of-range member snapshots from `SMSG_PARTY_MEMBER_STATS(_FULL)`, keyed by guid. Delta
    /// packets merge field-wise; `_FULL` replaces. Entries for guids that leave the roster are
    /// dropped at the next list.
    pub stats: HashMap<u64, PartyMemberStatsInfo>,
    /// The raid-target icon board (`MSG_RAID_TARGET_UPDATE`): slot = icon id 0-7 (star..skull),
    /// value = marked guid, `0` = unset. Consumed by the icon renders in phase 6.
    pub raid_targets: [u64; 8],
    /// `/partytest` sandbox mode: the roster is synthetic, so the drain applies group-mutating
    /// intents to this mirror LOCALLY (mimicking the server echo that a real group would send)
    /// instead of dispatching CMSGs into a void. Any real `SMSG_GROUP_LIST` switches it off —
    /// the wire always wins.
    pub test: bool,
    /// Our saved raid lockouts (`SMSG_RAID_INSTANCE_INFO`) — the Raid tab's Raid Info panel.
    /// The answer replaces the list wholesale, empty included.
    ///
    /// A lockout is per-CHARACTER, not per-group, so this is the one field here that is not a
    /// group fact. It lives here anyway for the reason that matters: it is per-SESSION state that
    /// must die with the socket, and [`Self::clear_session`] is that guarantee — a second
    /// resource would be a second thing to remember to clear. Leaving the group does NOT clear it
    /// ([`Self::leave_group`]); nor the four ticket fields below.
    pub saved_instances: Vec<benilla_protocol::messages::RaidInstanceEntry>,
    /// **How many times the server has answered** — one per `SMSG_RAID_INSTANCE_INFO`. A TICKET,
    /// not a flag (the [`Self::ready_check`] shape), and the difference is load-bearing: the
    /// reference decides its Raid Info button on the SECOND `UPDATE_INSTANCE_INFO`, because
    /// `RaidFrame.hasRaidInfo` swallows the first. A feed that fires that event on a *diff* never
    /// reaches the second for the ordinary player — their lockout list is empty, every answer says
    /// so, nothing ever changes, and the button they should not be able to press stays live (1561).
    ///
    /// The real client fires per PACKET, not per change. The handler at
    /// `0x49e070` takes `jbe 0x49e19d` (`0x49e0d8`) when the entry count is zero — straight past
    /// the parse loop to `mov ecx, 0x21b` (539 = `UPDATE_INSTANCE_INFO`) and the one
    /// `call FrameScript_SignalEvent 0x703e50` at `0x49e1a7`, which is the function's only exit
    /// and the event's only fire site in the binary. The empty answer signals exactly like a full
    /// one.
    pub saved_instances_answers: u32,
    /// A ready-check TICKET, not a flag: every `MSG_RAID_READY_CHECK` open bumps it, so the feed
    /// fires `READY_CHECK` on a counter edge and a second check while the first popup is still up
    /// re-arms it. A boolean could not tell the two apart.
    pub ready_check: u32,
    /// The request GENERATION — bumped by BOTH arms of the open form, unlike the
    /// ticket above, which only the non-leader arm bumps: the leader never gets the popup, because
    /// `0x4ba360`'s leader arm never reaches the `READY_CHECK` fire. The feed turns an edge here
    /// into the engine's `ready_check_request` and resets its answer cursor.
    pub ready_check_requests: u32,
    /// The answers forwarded to us as leader since the last request — `{guid, ready}` in wire
    /// order. A log the feed replays into the engine from its own cursor, append-only within one
    /// check so the feed never has to mutate this state; the next request clears it.
    pub ready_check_answers: Vec<(u64, bool)>,
}

impl GroupState {
    /// Apply one `SMSG_GROUP_LIST` and compose the system lines its roster diff implies. The
    /// diff is **ungated** (the 0440 byte law, `0x5e6c19`/`0x5e6d37`): the cache starts empty,
    /// so a first roster prints a join line for every member already there, and the all-zero
    /// "you left" shape prints a leave line per stale member — every observable composite
    /// (kick, voluntary leave, disband) is per-opcode lines stacking over this one diff.
    pub fn apply_list(
        &mut self,
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
    ) -> Vec<UiError> {
        let mut lines = Vec::new();
        // The all-zero shape is "you are no longer in a group" (vmangos sends a default packet,
        // `Group.h:257`); `leader == 0` is its reliable discriminator (a live group always names
        // one, and a solo-leader group has an empty member list *with* a leader guid).
        let leaving = leader == 0;
        // Raid wording keys off whichever side of the transition was a raid. INTERIM:
        // the party line's `groupType==0` gate is pinned; the raid twin's exact trigger is not
        // known.
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
        // Our own party→raid conversion (or joining straight into a raid). INTERIM like the
        // wording pick above.
        if group_type == 1 && self.group_type != 1 {
            lines.push(UiError::key("ERR_RAID_YOU_JOINED"));
        }
        // The OTHER direction clears the raid-target board. `0x4ba550` — sole caller `0x5e6ebb`,
        // inside `SMSG_GROUP_LIST 0x7d`'s raid-flag-CLEAR leg — `rep stosd`-zeroes all eight slots
        // and refreshes every formerly-marked unit. Without it a raid→party
        // conversion leaves stale marks on screen: the icons are drawn from this board, and
        // nothing else clears it short of a `0x321` for each slot, which the server does not send.
        //
        // A full disband needs no arm here — `leaving` above resets every group fact
        // ([`Self::leave_group`]), and the board is one. 1820 recorded this as "stale icons
        // after a disband", which was wider than the truth; the conversion is the real gap.
        if group_type != 1 && self.group_type == 1 {
            self.raid_targets = [0; 8];
        }

        // Drop stats snapshots for members no longer on the roster.
        self.stats
            .retain(|guid, _| members.iter().any(|m| m.guid == *guid));

        self.in_group = true;
        self.group_type = group_type;
        self.own_flags = own_flags;
        self.members = members;
        self.leader = leader;
        self.loot = loot;
        // A list apply is the real wire by default; `synthetic_roster` re-raises the flag after
        // its own call — so a real group arriving mid-sandbox always wins.
        self.test = false;
        lines
    }

    /// The `party1..party4` slot view (byte law `0x5e6baa`): members of our OWN subgroup
    /// only, packet order, at most four. In a plain party every flags byte is 0, so the filter
    /// is a no-op; in a raid it keeps the compact frames on your own subgroup. Slot↔member
    /// association is rebuilt per packet — never assume it's stable across resyncs.
    pub fn party_slots(&self) -> impl Iterator<Item = &GroupMemberEntry> {
        let own = self.own_flags & 0x7f;
        self.members
            .iter()
            .filter(move |m| m.flags & 0x7f == own)
            .take(4)
    }

    /// `SMSG_GROUP_INVITE` — someone asked us in. Arms the pending invite (the popup rides it)
    /// and composes the chat-side notice line (CONFIRMED 0440: the real handler `0x5e6730`
    /// fires ERR_INVITED_TO_GROUP_S in addition to the popup's event).
    pub fn apply_invited(&mut self, inviter: &str) -> Vec<UiError> {
        self.pending_invite = Some(inviter.to_string());
        vec![UiError::s("ERR_INVITED_TO_GROUP_S", inviter)]
    }

    /// `SMSG_GROUP_DECLINE` — our invitee said no (sent to the inviter only).
    pub fn apply_declined(&mut self, name: &str) -> Vec<UiError> {
        vec![UiError::s("ERR_DECLINE_GROUP_S", name)]
    }

    /// `SMSG_GROUP_UNINVITE` (empty body) — we were kicked. Prints ERR_UNINVITE_YOU
    /// **unconditionally**, at the opcode (handler `0x5e6850`); the empty roster echo
    /// that follows adds its own leave-diff lines, exactly like the reference.
    pub fn apply_uninvited(&mut self) -> Vec<UiError> {
        vec![UiError::key("ERR_UNINVITE_YOU")]
    }

    /// `SMSG_GROUP_DESTROYED` — the group is gone outright. Gated on believing we're grouped
    /// (`0x5e6880` tests `0x4e86d0() != 0`); the echo list's own lines still follow.
    pub fn apply_destroyed(&mut self) -> Vec<UiError> {
        if self.in_group {
            vec![UiError::key("ERR_GROUP_DISBANDED")]
        } else {
            Vec::new()
        }
    }

    /// `SMSG_GROUP_SET_LEADER` — the broadcast leader change, by name (the guid is already in
    /// our roster; vmangos re-sends the full list right after, `Group::ChangeLeader`).
    pub fn apply_leader_changed(&mut self, name: &str, own_name: Option<&str>) -> Vec<UiError> {
        if own_name == Some(name) {
            vec![UiError::key("ERR_NEW_LEADER_YOU")]
        } else {
            vec![UiError::s("ERR_NEW_LEADER_S", name)]
        }
    }

    /// `SMSG_PARTY_COMMAND_RESULT` — **the reference's own jump table, transcribed**.
    ///
    /// The handler `0x5e68b0` dispatches `dec eax; cmp eax,7; ja <silent>; jmp [4*eax+0x5e6a14]`,
    /// and each arm is one `CGGameUI::DisplayError(msgId)`. Returning the **message** rather than
    /// a sentence is what gets the text, the surface and the sound right at once, since all three
    /// are fields of the catalog row the id names.
    ///
    /// | `result` | msgId | key | `%s` |
    /// |---|---|---|---|
    /// | `0` + op `0` + non-empty name | `0x3a` | `ERR_INVITE_PLAYER_S` | name |
    /// | `0` + op `2` | `0x42` | `ERR_LEFT_GROUP_YOU` | — |
    /// | 1 | `0x47` | `ERR_BAD_PLAYER_NAME_S` | name |
    /// | 2 | `0x49` | `ERR_TARGET_NOT_IN_GROUP_S` | name |
    /// | 3 | `0x4a` | `ERR_GROUP_FULL` | — |
    /// | 4 | `0x3d` | `ERR_ALREADY_IN_GROUP_S` | name |
    /// | 5 | `0x48` | `ERR_NOT_IN_GROUP` | — |
    /// | 6 | `0x4b` | `ERR_NOT_LEADER` | — |
    /// | 7 | `0xff` | `ERR_PLAYER_WRONG_FACTION` | — |
    /// | 8 | `0x13d` | `ERR_IGNORING_YOU_S` | name |
    ///
    /// **`result == 7` is the odd one and we had it wrong:** its row is kind **2**, the red
    /// `UIErrorsFrame` line, where the other nine are kind 0 chat lines. benilla pushed all ten
    /// into the chat log.
    ///
    /// **Three inputs are SILENT**, and the reference means it — the default arm is four
    /// instructions, `mov eax,1; mov esp,ebp; pop ebp; ret 8`, with no call, no store and no
    /// message:
    ///
    /// 1. `result >= 9` — benilla printed an invented `"Party command failed (N)."` here, which is
    ///    worse than mis-wording a real string: the client deliberately says nothing.
    /// 2. `result == 0` with `operation ∉ {0, 2}`.
    /// 3. `result == 0`, `operation == 0`, and an **empty** name (`0x5e6923 je`) — benilla emitted
    ///    the invite line unconditionally.
    ///
    /// `operation` is read **only** on the `result == 0` path; on every other it is not touched.
    /// Takes `&self`: this arm reads the wire and names a message, and changes no state — the
    /// roster moves on `SMSG_GROUP_LIST`, not on the ack.
    pub fn apply_command_result(
        &self,
        operation: u32,
        member: &str,
        result: u32,
    ) -> Option<UiError> {
        let named = |key: &'static str| UiError::s(key, member);
        match result {
            party_result::OK => match operation {
                // The empty-name guard is the reference's, not defensiveness: an invite ack that
                // carries no name prints nothing at all.
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

    /// `SMSG_PARTY_MEMBER_STATS(_FULL)` — merge (delta) or replace (`full`) one member's
    /// out-of-range snapshot.
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

    /// `MSG_RAID_TARGET_UPDATE` mode 0 — one icon changed (`guid == 0` clears it).
    pub fn apply_raid_target(&mut self, icon: u8, guid: u64) {
        if let Some(slot) = self.raid_targets.get_mut(icon as usize) {
            *slot = guid;
        }
    }

    /// `MSG_RAID_TARGET_UPDATE` mode 1 — the whole board (absent icons are unset).
    pub fn apply_raid_target_list(&mut self, entries: &[(u8, u64)]) {
        self.raid_targets = [0; 8];
        for (icon, guid) in entries {
            self.apply_raid_target(*icon, *guid);
        }
    }

    /// The mark on a unit, from the board: `1..=8` (the Lua `GetRaidTargetIndex` scale), `0`
    /// unmarked. The board stores one guid per icon slot, so this is the reverse lookup the
    /// per-unit snapshots feed from.
    pub fn raid_target_index(&self, guid: u64) -> u8 {
        if guid == 0 {
            return 0;
        }
        self.raid_targets
            .iter()
            .position(|g| *g == guid)
            .map_or(0, |i| i as u8 + 1)
    }

    /// The all-zero `SMSG_GROUP_LIST` — we are no longer in a group. Resets the **group facts**
    /// and nothing else.
    ///
    /// This used to be `*self = GroupState::default()`, which is [`Self::clear_session`]'s job,
    /// and it took the per-SESSION tickets down with the roster: the feed fires on edges of those
    /// tickets, so every leave, kick or disband fired a `READY_CHECK` (a popup naming no leader,
    /// and a 30 s request armed for a check nobody started) and an `UPDATE_INSTANCE_INFO` over an
    /// emptied lockout list. In the reference `READY_CHECK` has one fire site, the
    /// `MSG_RAID_READY_CHECK` open handler, and a lockout belongs to the character.
    ///
    /// **The destructure is exhaustive on purpose** — no `..`: a field added to [`GroupState`]
    /// does not compile until someone decides here which side of that line it is on. The defaulted
    /// reset that caused this bug classified every new field silently.
    pub(super) fn leave_group(&mut self) {
        let GroupState {
            // Group facts: the roster, its side-state, the sandbox (the wire always wins).
            in_group,
            group_type,
            own_flags,
            members,
            leader,
            loot,
            stats,
            raid_targets,
            test,
            // An invite is an offer into a group we are not in yet; it is cleared with the group
            // as it always was here (UNVERIFIED either way — vmangos sends invitees nothing on a
            // disband, so the case needs a leave to race an invite popup).
            pending_invite,
            // Session tickets — the feed's edges. Kept; `clear_session` is where they die.
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

    /// Session teardown (decision 0065's lifecycle): everything resets with the socket.
    pub fn clear_session(&mut self) {
        *self = GroupState::default();
    }

    /// `SMSG_RAID_INSTANCE_INFO` — our saved lockouts, replacing the list wholesale (decision
    /// 1549). **The empty answer is the ordinary one and still counts as an answer**: it bumps the
    /// ticket like any other, which is what makes the feed fire `UPDATE_INSTANCE_INFO` for a
    /// player who has no lockouts at all — see [`GroupState::saved_instances_answers`] (1561).
    pub fn apply_raid_instance_info(
        &mut self,
        entries: Vec<benilla_protocol::messages::RaidInstanceEntry>,
    ) {
        self.saved_instances = entries;
        self.saved_instances_answers = self.saved_instances_answers.wrapping_add(1);
    }

    /// `MSG_RAID_READY_CHECK` (open form) — the leader started one, and the server echoes the
    /// request to every member including the leader. The handler `0x4ba360` splits on the leader
    /// guid: the **leader** takes the response-collection arm and neither prints
    /// nor pops; everyone **else** prints `ERR_RAID_LEADER_READY_CHECK_START_S` with the leader's
    /// name and gets the popup — the ticket the feed turns into a `READY_CHECK` event edge. Both
    /// arms bump the request generation and start a fresh answer log.
    pub fn apply_ready_check_request(&mut self, we_lead: bool) -> Vec<UiError> {
        self.ready_check_requests = self.ready_check_requests.wrapping_add(1);
        self.ready_check_answers.clear();
        if we_lead {
            return Vec::new();
        }
        self.ready_check = self.ready_check.wrapping_add(1);
        // The reference resolves the leader's name from its cache and prints whatever it holds;
        // ours is the roster's name for the leader's guid, which is the same cache's content.
        let leader = self
            .members
            .iter()
            .find(|m| m.guid == self.leader)
            .map(|m| m.name.as_str())
            .unwrap_or("");
        vec![UiError::s("ERR_RAID_LEADER_READY_CHECK_START_S", leader)]
    }

    /// `MSG_RAID_READY_CHECK` (answer form) — one member's answer, which the server forwards to
    /// the leader alone. Logged for the feed to replay into the engine's flags.
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

    /// **Every answer is an answer, including the one that says nothing changed** (1561). This is
    /// the invariant the whole Raid Info button hangs off: the reference throws its first
    /// `UPDATE_INSTANCE_INFO` away and decides the button on the second, so an empty list that
    /// answers a second time has to be distinguishable from an empty list that answered once. A
    /// flag cannot; a ticket can. Revert this to a `bool` and a player with no lockouts keeps a
    /// live button onto an empty panel — which is exactly how it shipped.
    #[test]
    fn an_unchanged_lockout_list_still_counts_as_an_answer() {
        let mut g = GroupState::default();
        assert_eq!(g.saved_instances_answers, 0, "nobody has asked yet");

        // The ordinary player: no lockouts, and the server says so every time it is asked.
        g.apply_raid_instance_info(Vec::new());
        assert_eq!(g.saved_instances_answers, 1);
        g.apply_raid_instance_info(Vec::new());
        assert_eq!(
            g.saved_instances_answers, 2,
            "the second empty answer is the one the button is decided on"
        );

        // And a list that DOES change is not counted twice for it.
        g.apply_raid_instance_info(vec![benilla_protocol::messages::RaidInstanceEntry {
            map: 409,
            reset: 86_400,
            instance: 7,
        }]);
        assert_eq!(g.saved_instances_answers, 3);
        assert_eq!(g.saved_instances.len(), 1, "and the list is the new one");

        // The socket dies, the ticket dies with it — a fresh session has not been answered.
        g.clear_session();
        assert_eq!(g.saved_instances_answers, 0);
        assert!(g.saved_instances.is_empty());
    }

    /// **Leaving the group is not the socket dying.** The all-zero `SMSG_GROUP_LIST` clears the
    /// roster — and ONLY the roster. The ready-check and saved-instance fields are per-session
    /// tickets the feed fires events on edges of: resetting them here made every leave, kick or
    /// disband look like a new `READY_CHECK` (a popup with no leader, and a stale 30 s request
    /// armed) and a fresh `UPDATE_INSTANCE_INFO` carrying an emptied lockout list — a blank Raid
    /// Info panel. In the reference `READY_CHECK` has one fire site, the `MSG_RAID_READY_CHECK`
    /// open handler; a lockout is the character's, not the group's.
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

        // The group facts are gone …
        assert!(!g.in_group);
        assert!(g.members.is_empty());
        assert_eq!(g.leader, 0);
        assert_eq!(g.raid_targets, [0; 8]);
        // … the session's tickets are not: no edge for the feed to fire on.
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

        // The socket's end is still the one place they die.
        g.clear_session();
        assert_eq!((g.ready_check, g.saved_instances_answers), (0, 0));
        assert!(g.saved_instances.is_empty());
    }

    /// The ungated diff (0440 byte law): a FIRST roster prints joins for everyone already
    /// there; later rosters diff both ways.
    /// The open form's two arms: the leader's own echo neither prints nor pops;
    /// a member prints the leader's line and takes the popup ticket. Both bump the request
    /// generation and restart the answer log.
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
        // Our first list — we just accepted; Alice was already there → her join line prints
        // (the real client's cache starts empty, 0x5e6c19).
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], 1, None),
            vec![UiError::s("ERR_JOINED_GROUP_S", "Alice")]
        );
        assert!(g.in_group);
        // Carol joins.
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1), member("Carol", 3)], 1, None),
            vec![UiError::s("ERR_JOINED_GROUP_S", "Carol")]
        );
        // Carol leaves (3→2).
        assert_eq!(
            g.apply_list(0, 0, vec![member("Alice", 1)], 1, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Carol")]
        );
        // An unchanged resync (loot/status churn re-sends the list) prints nothing.
        assert!(g
            .apply_list(0, 0, vec![member("Alice", 1)], 1, None)
            .is_empty());
    }

    /// Per-opcode lines stack over the diff — no empty-list state machine.
    #[test]
    fn leave_kick_disband_lines_stack_per_opcode() {
        // Voluntary: the LEAVE ack prints; the empty list adds the leave-diff.
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

        // Kicked: SMSG_GROUP_UNINVITE prints unconditionally at the opcode.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(g.apply_uninvited(), vec![UiError::key("ERR_UNINVITE_YOU")]);
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")]
        );

        // Destroyed: GROUP_DESTROYED prints (grouped), the echo list still runs its diff.
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
        // Ungrouped, GROUP_DESTROYED is silent (the 0x4e86d0 gate).
        assert!(g.apply_destroyed().is_empty());

        // The 2-man collapse (vmangos sends no GROUP_DESTROYED): just the leave-diff line.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_LEFT_GROUP_S", "Alice")]
        );
    }

    /// Raid wording: diffs use the raid strings, and the conversion prints the you-joined line.
    #[test]
    fn raid_wording_and_conversion() {
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None);
        // Party → raid conversion: same roster, type flips.
        assert_eq!(
            g.apply_list(1, 0, vec![member("Alice", 1)], 1, None),
            vec![UiError::key("ERR_RAID_YOU_JOINED")]
        );
        // A raid join/leave uses the raid strings.
        assert_eq!(
            g.apply_list(1, 0, vec![member("Alice", 1), member("Dave", 4)], 1, None),
            vec![UiError::s("ERR_RAID_MEMBER_ADDED_S", "Dave")]
        );
        let mut lines = g.apply_list(1, 0, vec![member("Alice", 1)], 1, None);
        assert_eq!(
            lines.pop(),
            Some(UiError::s("ERR_RAID_MEMBER_REMOVED_S", "Dave"))
        );
        // Leaving the raid: the ack prints the LEAVE line; the empty list diffs with raid
        // wording (keyed off the departed group's type).
        assert_eq!(
            g.apply_command_result(party_operation::LEAVE, "Us", party_result::OK),
            Some(UiError::key("ERR_LEFT_GROUP_YOU"))
        );
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), 0, None),
            vec![UiError::s("ERR_RAID_MEMBER_REMOVED_S", "Alice")]
        );
    }

    /// The party1..4 slot view filters to our own subgroup (`(flags^own)&0x7f`).
    #[test]
    fn party_slots_filter_to_own_subgroup() {
        let mut g = GroupState::default();
        let mut m2 = member("Bob", 2);
        m2.flags = 0x01; // subgroup 1
        let mut m3 = member("Carol", 3);
        m3.flags = 0x80; // subgroup 0, assistant — the 0x80 bit is ignored by the filter
        g.apply_list(1, 0x00, vec![member("Alice", 1), m2, m3], 1, None);
        let slots: Vec<&str> = g.party_slots().map(|m| m.name.as_str()).collect();
        assert_eq!(slots, vec!["Alice", "Carol"]);
    }

    /// Invite-side lines: the ack, the error table, the decline, the inbound invite.
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

    /// Leader lines: name match against our own name picks the YOU form.
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

    /// Stats: deltas merge field-wise, FULL replaces, roster departure drops the snapshot.
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

        // Alice leaves → her snapshot drops.
        g.apply_list(0, 0, Vec::new(), 0, None);
        assert!(g.stats.is_empty());
    }

    /// The raid-target board: deltas set/clear one slot, the list form resets the board.
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

    /// A raid→party conversion CLEARS the board; a disband does too (by replacing the state), and
    /// an ordinary roster change inside a raid does not. The reference zeroes all eight slots on
    /// `SMSG_GROUP_LIST`'s raid-flag-clear leg (`0x4ba550`) — without it the marks
    /// are drawn from a board nothing else empties, so they linger on screen.
    #[test]
    fn a_raid_to_party_conversion_clears_the_raid_target_board() {
        let mut g = GroupState::default();
        g.apply_list(1, 0, vec![member("Ally", 0x22)], 0x22, None);
        g.apply_raid_target(7, 0x22);
        assert_eq!(g.raid_targets[7], 0x22, "marked while a raid");

        // Still a raid, roster moved: the board survives.
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

        // Raid flag clears while the group lives on — the leg the reference zeroes on.
        g.apply_list(0, 0, vec![member("Ally", 0x22)], 0x22, None);
        assert_eq!(
            g.raid_targets, [0; 8],
            "the raid flag clearing empties the board"
        );
    }

    /// **The refusal table is the reference's, asserted by message id.**
    ///
    /// Each `result` must name the catalog row whose id the binary's own jump table pushes to
    /// `DisplayError` (read out of the PE at `0x5e6a14`).
    /// Asserting the *id* rather than the sentence is the point, and this family is the reason:
    /// nothing about the displayed English distinguishes a right key from a wrong one here, and a
    /// plausible-looking `ERR_WRONG_FACTION` — which exists in neither `GlobalStrings.lua` nor the
    /// catalog — would have shown **nothing at all**, silently.
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
            // The `%s` arms are exactly the arms the binary passes `&name` to, which is an
            // independent cross-check: every one of them is an `_S` key.
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

        // The OK path's two arms, where `operation` is the only thing read.
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

    /// **`result == 7` is the red line; the other nine are chat.** The one row in this family whose
    /// `+0x04` is kind 2, and benilla pushed all ten into the chat log.
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

    /// **Three inputs display NOTHING, and the reference means it.** Its default arm is four
    /// instructions — `mov eax,1; mov esp,ebp; pop ebp; ret 8` — with no call and no store.
    ///
    /// benilla printed an invented `"Party command failed (N)."` for the first of these. Inventing
    /// a sentence for a case the client is deliberately silent on is worse than mis-wording one it
    /// has a string for: there is no wording that could be right.
    #[test]
    fn the_silent_inputs_show_nothing() {
        let g = GroupState::default();
        // 1 — every result past the table's end (`dec eax; cmp eax,7; ja`, unsigned).
        for r in [9u32, 10, 42, u32::MAX] {
            assert!(
                g.apply_command_result(party_operation::INVITE, "Zed", r)
                    .is_none(),
                "result {r} must be silent"
            );
        }
        // 2 — OK with an operation that is neither invite nor leave.
        for op in [1u32, 3, 99] {
            assert!(g
                .apply_command_result(op, "Zed", party_result::OK)
                .is_none());
        }
        // 3 — an invite ack carrying no name (`0x5e6923 je`).
        assert!(g
            .apply_command_result(party_operation::INVITE, "", party_result::OK)
            .is_none());
        // …but a named one still prints, so the guard is the name and not the operation.
        assert!(g
            .apply_command_result(party_operation::INVITE, "Zed", party_result::OK)
            .is_some());
    }

    /// Every key this family raises resolves in the shipped 1.12 `GlobalStrings.lua` to the
    /// sentence the real client shows — the runtime half, since a key that is a valid catalog row
    /// can still be absent from the player's own chain. Skips without client data.
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
            // An `_S` key must actually carry the hole its fill expects.
            assert_eq!(
                text.contains("%s"),
                msg.arg_s().is_some(),
                "{} vs its fill",
                msg.key
            );
        }
    }

    /// **The twelve roster/invite/leader lines, by key** — the family that used to be twelve
    /// re-typed sentences beside twelve key names in comments, where nothing joined the two.
    /// The same three checks the command-result family gets: the key resolves in
    /// the shipped file, it is a catalog row (so its surface and sound are read rather than
    /// guessed), and an `_S` row's hole matches the fill that reaches it.
    ///
    /// **What the old chat-log path could not do**, and this one does: `ERR_DECLINE_GROUP_S`'s row
    /// names the `igPlayerInviteDecline` cue. Pushing the composed sentence straight into the chat
    /// log threw the row away and with it the sound — silently, since a missing cue looks exactly
    /// like a cue that has not been wired.
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
        // Every producer in the file, DRIVEN rather than listed — a party join and leave, then a
        // raid conversion with its own join and leave, then the per-opcode lines. Listing the keys
        // would let a new line skip this test; driving the producers cannot.
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
