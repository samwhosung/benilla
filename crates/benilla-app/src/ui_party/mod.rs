//! Group session state: the `SMSG_GROUP_LIST` mirror and the party system lines. vmangos sends no
//! text for party events, so, as the reference does through `DisplayError` (`0x496720`), each
//! opcode composes its own line and `SMSG_GROUP_LIST` composes joins and leaves from what the list
//! finds held: the party slots (`0x5e6a40`) and the raid roster (`0x4ba5f0`, `0x4ba550`).

use std::collections::HashMap;

use crate::ui_action::{FillArg, UiError};
use benilla_protocol::messages::{
    party_operation, party_result, patch_auras, GroupLootInfo, GroupMemberEntry,
    PartyMemberStatsInfo,
};
use bevy::prelude::*;

use crate::ui_script::UiFeed;

mod feed;
pub(crate) mod net;
mod pets;
mod records;
pub(crate) use feed::{
    drain_party, raid_row_guid, raid_row_guids, synthetic_raid, synthetic_roster, GROUPTYPE_RAID,
    GROUP_MEMBER_SUBGROUP,
};
#[cfg(test)]
pub(crate) use feed::{feed_party, PARTY_PET_TOKENS};
pub(crate) use records::record_notices;

pub(crate) struct UiPartyPlugin;

impl Plugin for UiPartyPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // A UI-only harness has no sound stack; the list handler queues the join chime, the stats
        // handler the unit events of a record.
        app.init_resource::<GroupState>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<crate::field_watch::FieldNotice>()
            .add_systems(
                Update,
                (
                    // Ahead of the field events, whose handlers read the group tokens, and after
                    // the unit snapshots: TargetFrame answers RAID_TARGET_UPDATE with
                    // GetRaidTargetIndex("target"), which reads the target snapshot's
                    // `raid_target`, so a new mark landed on the target frame only when the target
                    // changed if this ran first.
                    feed::feed_party
                        .after(crate::ui_unit::UnitSnapshots)
                        .in_set(UiFeed)
                        .in_set(crate::ui_unit::field_events::FieldEventPushes),
                    feed::drain_party.in_set(crate::ui_script::UiDrain),
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
    /// Our own raid row's guid while the raid roster holds rows: a raid list that names a member
    /// appends us after them, at row `count` (`0x5e6e6f`-`0x5e6e9c`), and one that names nobody
    /// leaves the roster empty (`0x5e6e71`, the writer's count at `0x4ba8a4`), as does any list
    /// that is not a raid (`0x4ba550`). The guid is the session's own, known from the login on,
    /// and a worldport keeps it. The reference writes the active player's (`0x5e6e73`), 0 while
    /// none is set, as in a worldport's new object manager (`0x401bc0` into `0x464ff0`) until our
    /// create, and the world entry fills that zero row (`0x4ba140`). vmangos sends our create
    /// ahead of any list at login (`Map.cpp:432`, `Group.cpp:1454`), so only a list that lands
    /// mid-worldport takes that path, which is not built.
    pub own_raid_row: Option<u64>,
    /// Whether any `SMSG_GROUP_LIST` landed this session, the all-zero one included: what a fresh
    /// VM's catch-up reads (`feed::catch_up`).
    pub listed: bool,
    /// The group events the handlers signalled, in the order they signalled them.
    pub(crate) signals: SignalLog,
    /// The name lookups the party seats left waiting, one per seat whose name the cache missed:
    /// each list seats its slots through `AddPartyMember`, whose lookup (`0x4e8477`) registers
    /// the callback `0x4e8490` with no dedupe, and nothing cancels one. An answer runs every one
    /// its guid holds.
    pub(crate) party_name_waits: Vec<u64>,
    /// The raid roster's pending names (`[0xb713f0]`): the rows of the last rebuild whose lookup
    /// missed (`0x4ba9d0`), which each answer counts down (`0x4bad90`).
    pub(crate) raid_names_pending: u32,
    /// The lookups those rows left waiting, one per miss (`0x4ba1b0`, registered at `0x4ba9ba`).
    /// A rebuild with names pending cancels every one the old rows hold (`0x4ba631`-`0x4ba64b`,
    /// `0x55f870`) before it counts afresh (`0x4ba89e`); a roster that a list that is not a raid
    /// drops keeps its own (`0x4ba550` touches neither), so their answers still count down.
    pub(crate) raid_name_waits: Vec<u64>,
    /// The leave ack, `SMSG_GROUP_UNINVITE` and `SMSG_GROUP_DESTROYED` empty the reference's party
    /// slots and leader (`0x4e84a0`, `0x4e8250(0)`) and keep its raid roster: from then
    /// [`Self::party_slots`] is empty, and the next list finds no slot held.
    pub slots_emptied: bool,
    /// The leader cell (`[0xbc75f8]`), which the endings zero.
    pub leader: u64,
    /// The leader the raid roster's ranks were written against at its last rebuild (`0x4ba8f9`),
    /// which an ending's zeroing of the cell leaves alone.
    pub raid_leader: u64,
    /// The loot tail, present whenever the list has members.
    pub loot: Option<GroupLootInfo>,
    /// The loot setter's stored method, master and threshold (`0xbc7600`, `0xbc7608`,
    /// `0xbc7610`): the last tail a list carried, which the next one is compared against. Nothing
    /// in the reference clears them, neither the end of a group nor a new login.
    pub loot_stored: GroupLootInfo,
    /// The inviter's name while the invite popup is up.
    pub pending_invite: Option<String>,
    /// Each member's record by guid, seated at 1/1, patched by `SMSG_PARTY_MEMBER_STATS`, and given
    /// its online bit again by each `SMSG_GROUP_LIST` that lists the member.
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

/// A group event as a packet's handler signals it; none of the five carries an argument
/// (benilla-re `tables/event_args.tsv`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupSignal {
    /// `PARTY_MEMBERS_CHANGED`: every list (`0x5e6c66`), a party seat's name answer (`0x4e8495`),
    /// the raid's last pending name (`0x4badb0`) and every ending (`0x4e85c5`).
    MembersChanged,
    /// `PARTY_LEADER_CHANGED`: the leader store (`0x4e82a9`, `0x4e82c2`).
    LeaderChanged,
    /// `PARTY_LOOT_METHOD_CHANGED`: the loot store (`0x4e87fa`).
    LootMethodChanged,
    /// `RAID_ROSTER_UPDATE`: a rebuild with no name pending (`0x4babef`), a drop (`0x4ba57b`) and
    /// the last pending name (`0x4bada6`).
    RaidRosterUpdate,
    /// `RAID_TARGET_UPDATE`: every `MSG_RAID_TARGET_UPDATE` (`0x4ba348`) and every list that
    /// clears the board (`0x4ba5db`).
    RaidTargetUpdate,
}

/// The group events signalled, oldest first. The reference signals each inside its packet's
/// handler (`FrameScript_SignalEvent`, `0x703e50`), so one drain's packets signal in wire order
/// and the feed fires them in that order, once each (`feed::feed_party`). Sequence numbers run on
/// across trims and sessions, so a cursor into the log stays good. An entry lives two ages, as a
/// bevy message lives two updates: `feed::drain_party` ages the log every frame, and the feed,
/// which also runs every frame, fires everything logged before the last age before the next one
/// drops it. A frame with no VM drops what no VM heard; the next VM is a fresh one, and its
/// catch-up stands in.
#[derive(Default)]
pub(crate) struct SignalLog {
    /// The sequence number of `entries[0]`.
    base: u64,
    entries: Vec<GroupSignal>,
    /// The log's end at the last [`Self::age`].
    aged: u64,
}

impl SignalLog {
    fn push(&mut self, signal: GroupSignal) {
        self.entries.push(signal);
    }

    /// One past the newest entry's sequence number.
    pub(crate) fn end(&self) -> u64 {
        self.base + self.entries.len() as u64
    }

    /// The entries from sequence number `cursor` on, oldest first.
    pub(crate) fn since(&self, cursor: u64) -> &[GroupSignal] {
        let skip = cursor
            .saturating_sub(self.base)
            .min(self.entries.len() as u64);
        &self.entries[skip as usize..]
    }

    /// Drop the entries logged before the previous age.
    pub(crate) fn age(&mut self) {
        let fired = self
            .aged
            .saturating_sub(self.base)
            .min(self.entries.len() as u64);
        self.entries.drain(..fired as usize);
        self.base += fired;
        self.aged = self.end();
    }

    /// Drop every entry; the numbering runs on.
    fn clear(&mut self) {
        self.base = self.end();
        self.entries.clear();
        self.aged = self.base;
    }

    /// Every entry, which leaves the log empty.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<GroupSignal> {
        let all = self.entries.clone();
        self.clear();
        all
    }
}

/// The message each loot method 0 to 4 shows when it changes, `0xe6` to `0xea` through the jump
/// table `0x4e880c`.
const LOOT_METHOD_KEYS: [&str; 5] = [
    "ERR_SET_LOOT_FREEFORALL",
    "ERR_SET_LOOT_ROUNDROBIN",
    "ERR_SET_LOOT_MASTER",
    "ERR_SET_LOOT_GROUP",
    "ERR_SET_LOOT_NBG",
];

/// Master loot, the only method whose looter the setter names (`0x4e8773`).
const LOOT_METHOD_MASTER: u8 = 2;

impl GroupState {
    /// Apply one `SMSG_GROUP_LIST` and return what it shows, in the reference's order: the party
    /// slots' joins, the new-group line, the party slots' leaves, then the raid roster's lines.
    /// Kick, leave (`0x5e690b`) and disband lines come from their opcodes. Every name is taken as
    /// held, as the empty string: [`Self::apply_list_awaiting`] is the one that says otherwise.
    #[cfg(test)]
    pub fn apply_list(
        &mut self,
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
        self_guid: Option<u64>,
    ) -> ListOutcome {
        self.apply_list_awaiting(
            group_type,
            own_flags,
            members,
            leader,
            loot,
            self_guid,
            &|_| Some(String::new()),
            &|_| Some(String::new()),
        )
    }

    /// [`Self::apply_list`] told what the name cache holds: `lookup` is the handler's lookup with a
    /// callback, the name the cache holds or `None` while it is still to come, and `cached_name`
    /// the loot master's, looked up with no callback (`0x4e878f`): the record if held, never
    /// asking. Each party seat looks
    /// its member up (`0x4e8477`), and a raid list's rebuild each row, ours included
    /// (`0x4ba9b6`-`0x4ba9c7`); a raid row that misses holds `RAID_ROSTER_UPDATE` back
    /// (`0x4babdf`) until its answer.
    pub fn apply_list_awaiting(
        &mut self,
        group_type: u8,
        own_flags: u8,
        members: Vec<GroupMemberEntry>,
        leader: u64,
        loot: Option<GroupLootInfo>,
        self_guid: Option<u64>,
        lookup: &dyn Fn(u64) -> Option<String>,
        cached_name: &dyn Fn(u64) -> Option<String>,
    ) -> ListOutcome {
        let missed = |guid: u64| lookup(guid).is_none();
        // Every path of the handler reaches the event (`0x5e6c66`), the leave list and a party's
        // list alike, and then the leader store (`0x5e6c7e` into `0x4e8250`), against a cell the
        // handler's own emptier zeroed first (`0x5e6b19` into `0x4e84a0(0)`, `0x4e85a9`): it
        // signals for every list naming a leader, the same one included, and returns on the
        // all-zero list's 0 (`0x4e8267`).
        self.listed = true;
        self.signal(GroupSignal::MembersChanged);
        if leader != 0 {
            self.signal(GroupSignal::LeaderChanged);
        }
        let mut out = ListOutcome::default();
        let raid = group_type == GROUPTYPE_RAID;
        // The two stores as this list finds them: the party slots unless an opcode emptied them,
        // and the raid roster, which only a list that is not a raid drops.
        let held_slots: Vec<(u64, String)> = self
            .party_slots()
            .map(|m| (m.guid, m.name.clone()))
            .collect();
        let raid_held = self.raid_held();
        // "Held a group" (`0x5e6aa9`): an occupied slot (`0x5e6afb`) or a raid roster (`0x5e6b29`).
        let held = !held_slots.is_empty() || raid_held;

        let own = own_flags & 0x7f;
        let seated: Vec<&GroupMemberEntry> = members
            .iter()
            .filter(|m| m.flags & 0x7f == own)
            .take(4)
            .collect();
        // Each seat's name lookup, whose callback signals when the answer lands.
        self.party_name_waits.extend(
            seated
                .iter()
                .map(|m| m.guid)
                .filter(|&g| g != 0 && missed(g)),
        );
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
        // The loot store (`0x5e6e52` into `0x4e86f0`) runs for a list with members, the one that
        // carries the loot tail (`0x5e6e12`): the lines of what the tail changes, then the event,
        // which it signals whatever the tail says (`0x4e87fa`), ahead of the raid legs.
        if let Some(loot) = loot {
            out.lines.extend(self.store_loot(loot, cached_name));
            self.signal(GroupSignal::LootMethodChanged);
        }
        if raid {
            // `0x4ba5f0`: with names pending, the old rows' lookups are cancelled first.
            if self.raid_names_pending != 0 {
                let old: Vec<u64> = self.raid_rows().collect();
                self.raid_name_waits.retain(|g| !old.contains(g));
            }
            // Arrivals only onto a held roster (`0x4ba725`), departures, and the joined line when
            // none was held (`0x4ba83a`).
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
                // The departures walk every old row (`0x4ba7a0`-`0x4ba7fd`), ours the last: a list
                // that names nobody rows no one, us included, so our row leaves too, under the name
                // the cache holds for it (`0x4ba7e3`) while a player is active (`0x4ba7ee`). A miss
                // defers the line to the answer (`0x4bad30`), which is not built.
                let ours = self.own_raid_row.filter(|&g| g != 0 && self_guid.is_some());
                if let Some(name) = ours.filter(|_| members.is_empty()).and_then(lookup) {
                    out.lines
                        .push(UiError::s("ERR_RAID_MEMBER_REMOVED_S", &name));
                }
            } else {
                out.lines.push(UiError::key("ERR_RAID_YOU_JOINED"));
            }
            // The rebuild counts afresh (`0x4ba89e`): each new row's lookup, ours after the
            // members' once any is listed, a zero guid asking nothing (`0x4ba9b4`). It ends in the
            // signal unless a name is pending (`0x4babdf`-`0x4babef`).
            let own_row = (!members.is_empty()).then(|| self_guid.unwrap_or(0));
            let waits: Vec<u64> = members
                .iter()
                .map(|m| m.guid)
                .chain(own_row)
                .filter(|&g| g != 0 && missed(g))
                .collect();
            self.raid_names_pending = waits.len() as u32;
            self.raid_name_waits.extend(waits);
            if self.raid_names_pending == 0 {
                self.signal(GroupSignal::RaidRosterUpdate);
            }
        } else {
            // `0x4ba550`: a list that is not a raid drops the held roster (`0x4ba55f`), and says so
            // (`0x4ba57b`); with no roster held it signals no `RAID_ROSTER_UPDATE`. Either way it
            // then clears the raid-target board and signals (`0x4ba580`-`0x4ba5db`), the all-zero
            // list's included; vmangos resends any icons after a party's list for it
            // (`Group.cpp:1358`).
            if raid_held {
                out.lines.push(UiError::key("ERR_RAID_YOU_LEFT"));
                self.signal(GroupSignal::RaidRosterUpdate);
            }
            self.raid_targets = [0; 8];
            self.signal(GroupSignal::RaidTargetUpdate);
        }

        // The all-zero list means "not in a group" (vmangos `Server/Packets/Group.h:257`); only
        // `leader == 0` tells it apart, since a solo leader's list is empty but names a leader.
        if leader == 0 {
            self.leave_group();
            return out;
        }

        self.stats
            .retain(|guid, _| members.iter().any(|m| m.guid == *guid));
        // Each listed member's record takes its online bit from the row's status byte and keeps the
        // rest: a known member's record is passed back to `0x4e82d0` (`0x5e6ddb`-`0x5e6de8`), which
        // writes the bit last (`0x4e8361`-`0x4e837b`), and the raid roster's twin does the same to
        // its row's record (`0x4ba947`-`0x4ba960`). A member new to the roster has no record yet;
        // `net::list` seats its placeholder.
        for m in &members {
            if let Some(record) = self.stats.get_mut(&m.guid) {
                record.set_online(m.listed_online());
            }
        }

        self.in_group = true;
        self.group_type = group_type;
        self.own_flags = own_flags;
        self.own_raid_row = (raid && !members.is_empty()).then(|| self_guid.unwrap_or(0));
        self.raid_leader = if raid { leader } else { 0 };
        self.members = members;
        self.slots_emptied = false;
        self.leader = leader;
        self.loot = loot;
        // A real list ends the sandbox; `synthetic_roster` re-raises the flag after its own call.
        self.test = false;
        out
    }

    /// The loot setter `0x4e86f0`: each of the method, the master and the threshold that differs
    /// from its stored cell shows its message, in that order, and is stored. The method has a
    /// message for 0 to 4 (`0x4e880c`), none above. A master shows only when it is nonzero, the
    /// method is master loot (`0x4e8773`) and the cache holds its name (`0x4e8794`); the cell is
    /// stored either way (`0x4e87a6`).
    fn store_loot(
        &mut self,
        new: GroupLootInfo,
        cached_name: &dyn Fn(u64) -> Option<String>,
    ) -> Vec<UiError> {
        let old = std::mem::replace(&mut self.loot_stored, new);
        let mut lines = Vec::new();
        if new.method != old.method {
            lines.extend(
                LOOT_METHOD_KEYS
                    .get(usize::from(new.method))
                    .map(|k| UiError::key(k)),
            );
        }
        if new.master != old.master && new.master != 0 && new.method == LOOT_METHOD_MASTER {
            if let Some(name) = cached_name(new.master) {
                lines.push(UiError::s("ERR_NEW_LOOT_MASTER_S", name));
            }
        }
        if new.threshold != old.threshold {
            // The word is `ITEM_QUALITY<n>_DESC` (`0x4e87c9`, read by `0x703bf0`), which the line
            // reads from the VM that shows it: the interface's, which the reference has loaded
            // before it reads a world packet and a login's group list can beat here.
            lines.push(UiError::args(
                "ERR_SET_LOOT_THRESHOLD_S",
                vec![FillArg::Global(format!(
                    "ITEM_QUALITY{}_DESC",
                    new.threshold
                ))],
            ));
        }
        lines
    }

    /// Log one event where its handler signals it, behind every signal before it.
    pub(super) fn signal(&mut self, signal: GroupSignal) {
        self.signals.push(signal);
    }

    /// A name answer for `guid`, found or refused: a pending node runs its callbacks either way
    /// (`0x556f05`, `0x556f91`), and these two read no status. Each party seat's signals
    /// `PARTY_MEMBERS_CHANGED` (`0x4e8495`); each raid row's counts the pending names down and,
    /// when none is left, or none was counted (`0x4bad97`), signals `RAID_ROSTER_UPDATE` then
    /// `PARTY_MEMBERS_CHANGED` (`0x4bad90`-`0x4badb0`). The party seats' run first here; the order
    /// the cache runs one node's callbacks in is not derived.
    pub fn apply_player_name(&mut self, guid: u64) {
        let taken = |waits: &mut Vec<u64>| {
            let before = waits.len();
            waits.retain(|&g| g != guid);
            (before - waits.len()) as u32
        };
        let party = taken(&mut self.party_name_waits);
        let raid = taken(&mut self.raid_name_waits);
        for _ in 0..party {
            self.signal(GroupSignal::MembersChanged);
        }
        for _ in 0..raid {
            if self.raid_names_pending != 0 {
                self.raid_names_pending -= 1;
                if self.raid_names_pending != 0 {
                    continue;
                }
            }
            self.signal(GroupSignal::RaidRosterUpdate);
            self.signal(GroupSignal::MembersChanged);
        }
    }

    /// The `party1..party4` slots: our own subgroup in packet order, at most four (`0x5e6baa`
    /// compares the flags under `0x7f`), none once an ending emptied them. A slot's member can
    /// change on any resync.
    pub fn party_slots(&self) -> impl Iterator<Item = &GroupMemberEntry> {
        let own = self.own_flags & 0x7f;
        let emptied = self.slots_emptied;
        self.members
            .iter()
            .filter(move |m| !emptied && m.flags & 0x7f == own)
            .take(4)
    }

    /// `0x4e84a0` with its argument set, then `0x4e8250(0)`, as the leave acknowledgement, the kick
    /// and the disband each call them: the party slots and the leader cell empty (`0x4e858c`,
    /// `0x4e85a9`), `PARTY_MEMBERS_CHANGED` signals (`0x4e85c5`), and the leader store finds the 0
    /// already there and signals nothing (`0x4e8267`). The raid roster stays.
    fn end_party(&mut self) {
        self.slots_emptied = true;
        self.leader = 0;
        self.signal(GroupSignal::MembersChanged);
    }

    /// Whether the raid roster holds rows (`[0xb713e0]` above 0), what every raid reader and the
    /// next list's raid legs test (`0x4ba556`, `0x4ba61a`, `0x5e6e65`).
    pub fn raid_held(&self) -> bool {
        self.own_raid_row.is_some()
    }

    /// The raid roster's rows in order, the one `GetRaidRosterInfo` and `raidN` read: the wire's
    /// members as listed (`0x5e6b8e`-`0x5e6ba3`, kept row for row by `0x4ba8c6`-`0x4ba8d9`), then
    /// us; empty while no roster is held.
    pub fn raid_rows(&self) -> impl Iterator<Item = u64> + '_ {
        let held = self.own_raid_row.is_some();
        self.members
            .iter()
            .filter(move |_| held)
            .map(|m| m.guid)
            .chain(self.own_raid_row)
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

    /// `SMSG_GROUP_UNINVITE`: we were kicked; ends the party and prints unconditionally
    /// (`0x5e6850`-`0x5e6865`).
    pub fn apply_uninvited(&mut self) -> Vec<UiError> {
        self.end_party();
        vec![UiError::key("ERR_UNINVITE_YOU")]
    }

    /// `SMSG_GROUP_DESTROYED`: prints only while a party slot is occupied (`0x5e6880` tests
    /// `0x4e86d0`, the slot count), so not after a kick or in a raid whose subgroup holds only us,
    /// then ends the party (`0x5e6893`). vmangos's two-member collapse sends none
    /// (`Group/Group.cpp:533`).
    pub fn apply_destroyed(&mut self) -> Vec<UiError> {
        let lines = if self.party_slots().next().is_some() {
            vec![UiError::key("ERR_GROUP_DISBANDED")]
        } else {
            Vec::new()
        };
        self.end_party();
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
                // The leave acknowledgement ends the party (`0x5e68f6`-`0x5e6909`).
                party_operation::LEAVE => {
                    self.end_party();
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

    /// `SMSG_PARTY_MEMBER_STATS`: a delta patches the record field by field (`0x5e519d`, seeded from
    /// the stored record), `_FULL` patches an emptied one (`0x5e51b1`). An aura block is patched
    /// slot by slot ([`patch_auras`]).
    pub fn apply_stats(&mut self, guid: u64, full: bool, info: PartyMemberStatsInfo) {
        if full {
            self.stats.insert(guid, PartyMemberStatsInfo::default());
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
        if info.pet_name.is_some() {
            entry.pet_name = info.pet_name;
        }
        for (record, patch) in [
            (&mut entry.auras, info.auras),
            (&mut entry.auras_negative, info.auras_negative),
            (&mut entry.pet_auras, info.pet_auras),
            (&mut entry.pet_auras_negative, info.pet_auras_negative),
        ] {
            if let Some(patch) = patch {
                patch_auras(record, &patch);
            }
        }
    }

    /// One icon of the board, cleared by guid 0.
    pub fn apply_raid_target(&mut self, icon: u8, guid: u64) {
        if let Some(slot) = self.raid_targets.get_mut(icon as usize) {
            *slot = guid;
        }
    }

    /// `MSG_RAID_TARGET_UPDATE` mode 0: one icon, and the packet's one signal (`0x4ba348`).
    pub fn apply_raid_target_update(&mut self, icon: u8, guid: u64) {
        self.apply_raid_target(icon, guid);
        self.signal(GroupSignal::RaidTargetUpdate);
    }

    /// `MSG_RAID_TARGET_UPDATE` mode 1: the whole board, an absent icon unset (`0x4ba24b`), and the
    /// packet's one signal (`0x4ba348`).
    pub fn apply_raid_target_list(&mut self, entries: &[(u8, u64)]) {
        self.raid_targets = [0; 8];
        for (icon, guid) in entries {
            self.apply_raid_target(*icon, *guid);
        }
        self.signal(GroupSignal::RaidTargetUpdate);
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
            own_raid_row,
            slots_emptied,
            leader,
            raid_leader,
            loot,
            stats,
            raid_targets,
            test,
            // Cleared with the group. The reference's side is untraced; vmangos tells an invitee
            // nothing on a disband (`Group::RemoveAllInvites`).
            pending_invite,
            // The reference's stored loot cells, which no ending clears.
            loot_stored: _,
            // Session state the feed fires events off: kept until `clear_session`.
            listed: _,
            signals: _,
            // The name cache's callbacks, which no list ending touches (`0x4ba550`).
            party_name_waits: _,
            raid_names_pending: _,
            raid_name_waits: _,
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
        *own_raid_row = None;
        *slots_emptied = false;
        *leader = 0;
        *raid_leader = 0;
        *loot = None;
        stats.clear();
        *raid_targets = [0; 8];
        *test = false;
        *pending_invite = None;
    }

    /// Session teardown: everything resets with the socket but the signal log's numbering, so the
    /// feed's cursor into it stays good.
    pub fn clear_session(&mut self) {
        let mut signals = std::mem::take(&mut self.signals);
        signals.clear();
        // The loot cells are the process's, not the session's: the reference's only writer
        // (`0x4e86f0`) is the list handler, and nothing resets them.
        let loot_stored = self.loot_stored;
        *self = GroupState {
            signals,
            loot_stored,
            ..GroupState::default()
        };
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

    /// How many of `signals` are `signal`.
    fn count(signals: &[GroupSignal], signal: GroupSignal) -> usize {
        signals.iter().filter(|s| **s == signal).count()
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

    /// Every list signals its members change (`0x5e6c66`), whatever it changed; then the leader
    /// store's for any leader it names, against the cell the list zeroed first (`0x5e6b19`,
    /// `0x4e8250`), never for the all-zero list's 0 (`0x4e8267`); the loot store's for a list with
    /// members, the one with the tail (`0x5e6e12`, `0x4e87fa`); then a party list's board clear
    /// (`0x4ba5db`).
    #[test]
    fn every_list_signals_whatever_it_changes() {
        use GroupSignal::*;
        let mut g = GroupState::default();
        let tail = GroupLootInfo {
            method: 3,
            master: 0,
            threshold: 2,
        };
        let list = |g: &mut GroupState, members: Vec<GroupMemberEntry>, leader| {
            let loot = (!members.is_empty()).then_some(tail);
            g.apply_list(0, 0, members, leader, loot, Some(ME));
            g.signals.take()
        };
        let full = [
            MembersChanged,
            LeaderChanged,
            LootMethodChanged,
            RaidTargetUpdate,
        ];
        let alice = member("Alice", 0xA11CE);
        assert_eq!(list(&mut g, vec![alice.clone()], 0xA11CE), full);
        assert_eq!(
            list(&mut g, vec![alice.clone()], 0xA11CE),
            full,
            "the same list again"
        );
        let away = GroupMemberEntry {
            status: 0,
            ..alice.clone()
        };
        assert_eq!(list(&mut g, vec![away], 0xA11CE), full, "a status alone");
        assert_eq!(list(&mut g, vec![alice], ME), full, "a leader alone");
        assert_eq!(
            list(&mut g, vec![], ME),
            [MembersChanged, LeaderChanged, RaidTargetUpdate],
            "a leader with no members, and so no tail"
        );
        assert_eq!(
            list(&mut g, vec![], 0),
            [MembersChanged, RaidTargetUpdate],
            "the all-zero list"
        );
        assert_eq!(
            list(&mut g, vec![], 0),
            [MembersChanged, RaidTargetUpdate],
            "and another, with nobody held"
        );

        g.leave_group();
        assert_eq!(g.signals.take(), [], "leaving is not a list");
        assert!(g.listed);
        g.clear_session();
        assert!(!g.listed);
    }

    /// One list's signals in the handler's order: the members change (`0x5e6c66`), the leader
    /// store (`0x5e6c7e`), the loot store (`0x5e6e52`), then the raid legs (`0x5e6eb4`,
    /// `0x5e6ebb`), here the drop's roster update (`0x4ba57b`) and board clear (`0x4ba5db`).
    #[test]
    fn a_list_signals_in_its_handlers_order() {
        use GroupSignal::*;
        let mut g = GroupState::default();
        g.apply_list(1, 0, vec![sub("Alice", 1, 0)], ME, None, Some(ME));
        g.signals.take();
        let loot = GroupLootInfo {
            method: 2,
            master: 1,
            threshold: 3,
        };
        g.apply_list(0, 0, vec![sub("Alice", 1, 0)], 1, Some(loot), Some(ME));
        assert_eq!(
            g.signals.take(),
            [
                MembersChanged,
                LeaderChanged,
                LootMethodChanged,
                RaidRosterUpdate,
                RaidTargetUpdate
            ]
        );
    }

    /// The log keeps an entry through the age after the one that found it, so a reader between
    /// any two ages sees it, and its numbering runs on across ages and a clear.
    #[test]
    fn the_signal_log_keeps_an_entry_for_two_ages() {
        use GroupSignal::*;
        let mut log = SignalLog::default();
        log.push(MembersChanged);
        log.age();
        assert_eq!(log.since(0), [MembersChanged], "logged before this age");
        log.push(LeaderChanged);
        log.age();
        assert_eq!(log.since(0), [LeaderChanged], "the first is gone");
        assert_eq!(log.since(1), [LeaderChanged]);
        assert_eq!(log.since(2), []);
        log.clear();
        assert_eq!(log.end(), 2, "the numbering runs on");
        log.push(RaidTargetUpdate);
        assert_eq!(log.since(2), [RaidTargetUpdate]);
        assert_eq!(
            log.since(0),
            [RaidTargetUpdate],
            "an old cursor reads what is left"
        );
    }

    /// The raid roster writer signals after each rebuild (`0x4babef`), and nothing gates the signal
    /// on a difference from the roster it replaces, so a list that moves a member's status alone, or
    /// nothing, signals.
    #[test]
    fn every_raid_list_signals_a_roster_update_whatever_it_changes() {
        use benilla_protocol::messages::member_status;
        use GroupSignal::*;
        let mut g = GroupState::default();
        let raid = |g: &mut GroupState, members: Vec<GroupMemberEntry>| {
            g.apply_list(1, 0, members, 1, None, Some(ME));
            g.signals.take()
        };

        let (alice, bob) = (sub("Alice", 1, 0), sub("Bob", 2, 1));
        assert_eq!(
            raid(&mut g, vec![alice.clone(), bob.clone()]),
            [MembersChanged, LeaderChanged, RaidRosterUpdate],
            "seats the raid"
        );
        let again = [MembersChanged, LeaderChanged, RaidRosterUpdate];
        assert_eq!(
            raid(&mut g, vec![alice.clone(), bob.clone()]),
            again,
            "the same list again"
        );
        let dead = GroupMemberEntry {
            status: member_status::ONLINE | member_status::DEAD,
            ..bob
        };
        assert_eq!(
            raid(&mut g, vec![alice.clone(), dead]),
            again,
            "a status alone"
        );
        assert_eq!(raid(&mut g, vec![alice]), again, "a member gone");
        assert_eq!(
            raid(&mut g, vec![]),
            again,
            "a raid list naming nobody still rebuilds"
        );

        g.leave_group();
        assert_eq!(g.signals.take(), [], "leaving is not a list");
    }

    /// A list that is not a raid runs no roster writer, so a party's list signals nothing; one that
    /// finds a raid held drops it and signals once (`0x4ba57b`), the all-zero list among them, and
    /// with none held there is nothing to say.
    #[test]
    fn a_list_that_is_not_a_raid_signals_a_roster_update_only_for_the_raid_it_drops() {
        let mut g = GroupState::default();
        let list = |g: &mut GroupState, group_type: u8, members: Vec<GroupMemberEntry>, leader| {
            g.apply_list(group_type, 0, members, leader, None, Some(ME));
            count(&g.signals.take(), GroupSignal::RaidRosterUpdate)
        };
        let party = vec![sub("Alice", 1, 0)];

        assert_eq!(list(&mut g, 0, party.clone(), 1), 0, "a party's list");
        assert_eq!(list(&mut g, 0, party.clone(), 1), 0, "and again");
        assert_eq!(
            list(&mut g, 0, vec![], 0),
            0,
            "the all-zero list with no raid held"
        );

        assert_eq!(
            list(&mut g, 1, party.clone(), 1),
            1,
            "the party becomes a raid"
        );
        assert_eq!(
            list(&mut g, 0, party.clone(), 1),
            1,
            "the raid becomes a party"
        );
        assert_eq!(list(&mut g, 0, party.clone(), 1), 0, "nothing held to drop");

        assert_eq!(list(&mut g, 1, party, 1), 1);
        assert_eq!(
            list(&mut g, 0, vec![], 0),
            1,
            "the all-zero list ends the raid"
        );
        assert_eq!(
            list(&mut g, 0, vec![], 0),
            0,
            "and a second has none to end"
        );
    }

    /// The rebuild counts the rows whose name is still to come and signals only with none
    /// (`0x4babdf`); each answer counts one down, and the last signals `RAID_ROSTER_UPDATE` then
    /// `PARTY_MEMBERS_CHANGED` (`0x4bad90`-`0x4badb0`). A rebuild with names pending cancels the
    /// old rows' lookups (`0x55f870`) and counts afresh; a drop keeps them (`0x4ba550`), so a late
    /// answer signals with no raid held.
    #[test]
    fn a_raid_list_waits_on_its_rows_names() {
        use GroupSignal::*;
        let mut g = GroupState::default();
        // Alice shares our subgroup, so her seat waits too; Bob is in another.
        let raid = vec![sub("Alice", 1, 0), sub("Bob", 2, 1)];
        let list = |g: &mut GroupState, group_type: u8, missing: &[u64]| {
            let missing = missing.to_vec();
            g.apply_list_awaiting(
                group_type,
                0,
                raid.clone(),
                1,
                None,
                Some(ME),
                &|guid| (!missing.contains(&guid)).then(String::new),
                &|_| Some(String::new()),
            );
            g.signals.take()
        };
        let answer = |g: &mut GroupState, guid: u64| {
            g.apply_player_name(guid);
            g.signals.take()
        };

        assert_eq!(
            list(&mut g, 1, &[1, 2]),
            [MembersChanged, LeaderChanged],
            "two names pending"
        );
        assert_eq!(g.members.len(), 2, "the roster is rebuilt regardless");
        assert_eq!(answer(&mut g, 2), [], "one left");
        assert_eq!(
            answer(&mut g, 1),
            [MembersChanged, RaidRosterUpdate, MembersChanged],
            "the last: Alice's seat's, then the roster and the members"
        );
        assert_eq!(answer(&mut g, 1), [], "nothing left waiting");

        let pending = [MembersChanged, LeaderChanged];
        assert_eq!(list(&mut g, 1, &[2]), pending, "Bob pending");
        assert_eq!(
            list(&mut g, 1, &[]),
            [MembersChanged, LeaderChanged, RaidRosterUpdate],
            "every name held"
        );
        assert_eq!(answer(&mut g, 2), [], "the rebuild cancelled his");

        assert_eq!(list(&mut g, 1, &[2]), pending, "Bob pending again");
        assert_eq!(
            list(&mut g, 0, &[2]),
            [
                MembersChanged,
                LeaderChanged,
                RaidRosterUpdate,
                RaidTargetUpdate
            ],
            "the drop signals its own"
        );
        assert_eq!(
            answer(&mut g, 2),
            [RaidRosterUpdate, MembersChanged],
            "and keeps his lookup"
        );
    }

    /// Each list seats its slots and looks each member up, the callback added with no dedupe and
    /// never cancelled (`0x4e8467`-`0x4e8477`): an answer signals `PARTY_MEMBERS_CHANGED` once
    /// for every list that seated the member while the name was to come (`0x4e8495`).
    #[test]
    fn a_party_seat_waits_on_its_name_once_per_list() {
        use GroupSignal::*;
        let mut g = GroupState::default();
        let party = vec![member("Alice", 1), sub("Bob", 2, 1)];
        let list = |g: &mut GroupState| {
            g.apply_list_awaiting(0, 0, party.clone(), 1, None, Some(ME), &|_| None, &|_| {
                Some(String::new())
            });
        };
        list(&mut g);
        list(&mut g);
        g.signals.take();
        g.apply_player_name(2);
        assert_eq!(g.signals.take(), [], "Bob holds no seat of ours");
        g.apply_player_name(1);
        assert_eq!(
            g.signals.take(),
            [MembersChanged, MembersChanged],
            "one per list"
        );
        g.apply_player_name(1);
        assert_eq!(g.signals.take(), []);

        list(&mut g);
        g.apply_list(0, 0, Vec::new(), 0, None, Some(ME));
        g.signals.take();
        g.apply_player_name(1);
        assert_eq!(
            g.signals.take(),
            [MembersChanged],
            "the group's end leaves the lookup"
        );
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

    /// Each ending runs `0x4e84a0(1)` then `0x4e8250(0)` (`0x5e68f6`, `0x5e6850`, `0x5e6893`): the
    /// slots and the leader cell empty at once, `PARTY_MEMBERS_CHANGED` signals and
    /// `PARTY_LEADER_CHANGED` does not, nor does the all-zero list after it; the raid roster and
    /// its ranks stay until a list drops them.
    #[test]
    fn an_ending_empties_the_slots_and_the_leader_at_once() {
        type Ending = fn(&mut GroupState) -> Vec<UiError>;
        let endings: [Ending; 3] = [
            |g| {
                g.apply_command_result(party_operation::LEAVE, "Us", party_result::OK)
                    .into_iter()
                    .collect()
            },
            |g| g.apply_uninvited(),
            |g| g.apply_destroyed(),
        ];
        for (i, end) in endings.into_iter().enumerate() {
            let mut g = GroupState::default();
            g.apply_list(
                1,
                0,
                vec![sub("Alice", 1, 0), sub("Bob", 2, 1)],
                1,
                None,
                Some(ME),
            );
            g.signals.take();
            end(&mut g);
            assert_eq!(g.party_slots().count(), 0, "ending {i}: the slots");
            assert_eq!(g.leader, 0, "ending {i}: the leader cell");
            assert_eq!(
                g.signals.take(),
                [GroupSignal::MembersChanged],
                "ending {i}: signalled, and the leader store finds the 0 already there"
            );
            assert_eq!(
                g.raid_rows().count(),
                3,
                "ending {i}: the raid roster stays"
            );
            assert_eq!(g.raid_leader, 1, "and ranks as it was rebuilt");
            assert_eq!(
                g.apply_list(0, 0, Vec::new(), 0, None, Some(ME)).lines,
                vec![UiError::key("ERR_RAID_YOU_LEFT")],
                "ending {i}: the all-zero list drops the raid"
            );
            assert_eq!(
                count(&g.signals.take(), GroupSignal::LeaderChanged),
                0,
                "ending {i}: and leads nobody new"
            );
        }

        // A disband after a kick finds no slot held and prints nothing (`0x5e6887`); so does one in
        // a raid whose subgroup holds only us.
        let mut g = GroupState::default();
        g.apply_list(0, 0, vec![member("Alice", 1)], 1, None, Some(ME));
        g.apply_uninvited();
        assert!(g.apply_destroyed().is_empty(), "after a kick");
        let mut g = GroupState::default();
        g.apply_list(1, 0, vec![sub("Bob", 2, 1)], 2, None, Some(ME));
        assert!(g.in_group);
        assert!(g.apply_destroyed().is_empty(), "alone in our subgroup");
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

    /// The writer's departures walk every old row, ours the last (`0x4ba7a0`-`0x4ba7fd`): a raid
    /// list that names nobody rows no one, so with a roster held our own row leaves as the others
    /// do, under the name the cache holds for us (`0x4ba7e3`); a list that names anyone rows us
    /// again, and a miss shows nothing now (its line waits on `0x4bad30`).
    #[test]
    fn a_raid_list_naming_nobody_removes_our_own_row_too() {
        let removed = |name: &str| UiError::s("ERR_RAID_MEMBER_REMOVED_S", name);
        let named = |guid: u64| Some(if guid == ME { "Me" } else { "Alice" }.to_string());
        let raid = |g: &mut GroupState, members, names: &dyn Fn(u64) -> Option<String>| {
            g.apply_list_awaiting(1, 0, members, ME, None, Some(ME), names, &|_| {
                Some(String::new())
            })
            .lines
        };
        let mut g = GroupState::default();
        raid(&mut g, vec![sub("Alice", 1, 0)], &named);
        assert_eq!(
            raid(&mut g, vec![sub("Alice", 1, 0)], &named),
            [],
            "a list that rows us again"
        );
        assert_eq!(
            raid(&mut g, Vec::new(), &named),
            [removed("Alice"), removed("Me")]
        );

        let mut g = GroupState::default();
        raid(&mut g, vec![sub("Alice", 1, 0)], &named);
        let unnamed = |guid: u64| (guid != ME).then(|| "Alice".to_string());
        assert_eq!(
            raid(&mut g, Vec::new(), &unnamed),
            [removed("Alice")],
            "our name to come"
        );
    }

    /// A raid list that names nobody seats no row, ours included (`0x5e6e6f`-`0x5e6e71`), so it
    /// leaves no roster held: the next such list prints the joined line again (`0x4ba838`, on the
    /// old count of 0), and the party list after it says nothing of a raid and drops nothing
    /// (`0x4ba556`). A raid list that names a member seats them, then us (`0x5e6e73`).
    #[test]
    fn a_raid_of_one_holds_no_roster() {
        let mut g = GroupState::default();
        let alone = |g: &mut GroupState| g.apply_list(1, 0, Vec::new(), ME, None, Some(ME));
        assert_eq!(
            alone(&mut g).lines,
            vec![UiError::key("ERR_RAID_YOU_JOINED")]
        );
        assert!(!g.raid_held());
        assert_eq!(g.raid_rows().count(), 0, "not even our own row");
        assert_eq!(
            alone(&mut g).lines,
            vec![UiError::key("ERR_RAID_YOU_JOINED")],
            "no roster held, so joined again"
        );
        g.signals.take();
        assert_eq!(
            g.apply_list(0, 0, Vec::new(), ME, None, Some(ME)).lines,
            Vec::new(),
            "no raid to leave"
        );
        assert_eq!(
            count(&g.signals.take(), GroupSignal::RaidRosterUpdate),
            0,
            "and none to drop"
        );

        g.apply_list(1, 0, vec![sub("Alice", 1, 2)], ME, None, Some(ME));
        assert!(g.raid_held());
        assert_eq!(g.raid_rows().collect::<Vec<_>>(), vec![1, ME]);
        assert_eq!(
            g.apply_list(0, 0, vec![sub("Alice", 1, 2)], ME, None, Some(ME))
                .lines,
            vec![UiError::key("ERR_RAID_YOU_LEFT")],
            "a roster held is dropped"
        );
        assert_eq!(g.raid_rows().count(), 0);
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

    /// Every list without the raid flag clears the board and signals (`0x4ba580`-`0x4ba5db`), a
    /// party's to a party and the all-zero list too, with no raid ever held; a raid's list clears
    /// nothing; each `MSG_RAID_TARGET_UPDATE` signals once, whatever it changed (`0x4ba348`).
    #[test]
    fn every_party_list_clears_the_board_and_signals() {
        let mut g = GroupState::default();
        let marks = |g: &mut GroupState| count(&g.signals.take(), GroupSignal::RaidTargetUpdate);
        let party = || vec![member("Ally", 0x22)];
        g.apply_list(0, 0, party(), 0x22, None, None);
        assert_eq!(marks(&mut g), 1, "a party's first list");
        g.apply_raid_target_update(7, 0x22);
        assert_eq!(marks(&mut g), 1);
        g.apply_raid_target_update(7, 0x22);
        assert_eq!(
            marks(&mut g),
            1,
            "a packet that changes nothing still signals"
        );
        g.apply_list(0, 0, party(), 0x22, None, None);
        assert_eq!(
            g.raid_targets, [0; 8],
            "a party's list clears a party's marks"
        );
        assert_eq!(marks(&mut g), 1);

        g.apply_list(1, 0, party(), 0x22, None, None);
        g.apply_raid_target_list(&[(0, 0x22)]);
        marks(&mut g);
        g.apply_list(1, 0, party(), 0x22, None, None);
        assert_eq!(g.raid_targets[0], 0x22, "a raid's list keeps them");
        assert_eq!(marks(&mut g), 0, "and signals none");

        g.apply_list(0, 0, Vec::new(), 0, None, None);
        assert_eq!(g.raid_targets, [0; 8]);
        assert_eq!(marks(&mut g), 1, "the all-zero list signals");
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
        // The loot setter, through each method's row, a master and a threshold.
        for (method, master, threshold) in [(1, 0, 2), (2, 1, 3), (3, 1, 3), (4, 1, 3), (0, 0, 3)] {
            let loot = GroupLootInfo {
                method,
                master,
                threshold,
            };
            lines.extend(
                g.apply_list(1, 0, vec![member("Alice", 1)], 1, Some(loot), None)
                    .lines,
            );
        }
        lines.extend(g.apply_invited("Bob"));
        lines.extend(g.apply_declined("Carol"));
        lines.extend(g.apply_leader_changed("Alice", Some("Us")));
        lines.extend(g.apply_leader_changed("Us", Some("Us")));
        // The disband while a slot is held, so it prints; the kick after it prints regardless.
        lines.extend(g.apply_destroyed());
        lines.extend(g.apply_uninvited());
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
                "ERR_NEW_LOOT_MASTER_S",
                "ERR_RAID_LEADER_READY_CHECK_START_S",
                "ERR_RAID_MEMBER_ADDED_S",
                "ERR_RAID_MEMBER_REMOVED_S",
                "ERR_RAID_YOU_JOINED",
                "ERR_RAID_YOU_LEFT",
                "ERR_SET_LOOT_FREEFORALL",
                "ERR_SET_LOOT_GROUP",
                "ERR_SET_LOOT_MASTER",
                "ERR_SET_LOOT_NBG",
                "ERR_SET_LOOT_ROUNDROBIN",
                "ERR_SET_LOOT_THRESHOLD_S",
                "ERR_UNINVITE_YOU",
            ],
            "all twenty keys this window can raise are exercised below"
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
                !msg.args.is_empty(),
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

    // ── The loot setter's lines (`0x4e86f0`) ────────────────────────────────────────────────────

    const THRALL: u64 = 0x7A;

    /// The threshold line for quality `n`: the word is the VM global's, read where it is shown.
    fn threshold(n: u8) -> UiError {
        UiError::args(
            "ERR_SET_LOOT_THRESHOLD_S",
            vec![FillArg::Global(format!("ITEM_QUALITY{n}_DESC"))],
        )
    }

    /// A list of `loot` over a party Alice leads, whose cache names only [`THRALL`]: the lines
    /// and the signals it raised.
    fn loot_list(g: &mut GroupState, loot: GroupLootInfo) -> (Vec<UiError>, Vec<GroupSignal>) {
        let cached = |guid: u64| (guid == THRALL).then(|| "Thrall".to_string());
        let lines = g
            .apply_list_awaiting(
                0,
                0,
                vec![member("Alice", 1), member("Thrall", THRALL)],
                1,
                Some(loot),
                Some(ME),
                &|_| Some(String::new()),
                &cached,
            )
            .lines;
        let signals = g.signals.take();
        (lines, signals)
    }

    fn tail(method: u8, master: u64, threshold: u8) -> GroupLootInfo {
        GroupLootInfo {
            method,
            master,
            threshold,
        }
    }

    /// The cells start at zero, so a first group of group loot over green says both, in the
    /// method's row and the threshold's (`0x4e8709`, `0x4e87b5`); the same tail again says nothing,
    /// though the event fires all the same (`0x4e87fa`).
    #[test]
    fn a_tail_differing_from_the_stored_cells_says_so_before_the_event() {
        let mut g = GroupState::default();
        let (lines, signals) = loot_list(&mut g, tail(3, 0, 2));
        assert_eq!(lines, [UiError::key("ERR_SET_LOOT_GROUP"), threshold(2)]);
        assert_eq!(count(&signals, GroupSignal::LootMethodChanged), 1);

        let (lines, signals) = loot_list(&mut g, tail(3, 0, 2));
        assert_eq!(lines, []);
        assert_eq!(count(&signals, GroupSignal::LootMethodChanged), 1);

        let (lines, _) = loot_list(&mut g, tail(3, 0, 4));
        assert_eq!(lines, [threshold(4)]);
    }

    /// The jump table `0x4e880c` gives methods 0 to 4 messages `0xe6` to `0xea`; a method above 4
    /// is stored with none (`0x4e871c`, `0x4e8722`). Free-for-all is the cell's own start, so a
    /// first group at it says nothing of the method.
    #[test]
    fn each_method_has_its_row_and_an_unknown_one_none() {
        let keys = [
            "ERR_SET_LOOT_FREEFORALL",
            "ERR_SET_LOOT_ROUNDROBIN",
            "ERR_SET_LOOT_MASTER",
            "ERR_SET_LOOT_GROUP",
            "ERR_SET_LOOT_NBG",
        ];
        for (method, key) in (0u8..).zip(keys) {
            // From a method it differs from, at the threshold the cell already holds.
            let mut g = GroupState {
                loot_stored: tail(if method == 1 { 2 } else { 1 }, 0, 2),
                ..GroupState::default()
            };
            let (lines, _) = loot_list(&mut g, tail(method, 0, 2));
            assert_eq!(lines, [UiError::key(key)], "method {method}");
        }

        let mut g = GroupState::default();
        let (lines, _) = loot_list(&mut g, tail(0, 0, 0));
        assert_eq!(lines, [], "free-for-all over the zero cells");
        let (lines, signals) = loot_list(&mut g, tail(5, 0, 0));
        assert_eq!(lines, [], "a method past the table");
        assert_eq!(count(&signals, GroupSignal::LootMethodChanged), 1);
        let (lines, _) = loot_list(&mut g, tail(3, 0, 0));
        assert_eq!(
            lines,
            [UiError::key("ERR_SET_LOOT_GROUP")],
            "and it was stored"
        );
    }

    /// A new nonzero master names itself only under master loot (`0x4e8773 cmp [ebp-0xc],2`) and
    /// only when the cache holds the name (`0x4e8794`, a lookup with no callback); the cell is
    /// stored either way (`0x4e87a6`), so a name that arrives later never prints.
    #[test]
    fn a_new_master_names_itself_only_under_master_loot_with_the_name_cached() {
        let mut g = GroupState::default();
        let (lines, _) = loot_list(&mut g, tail(2, THRALL, 2));
        assert_eq!(
            lines,
            [
                UiError::key("ERR_SET_LOOT_MASTER"),
                UiError::s("ERR_NEW_LOOT_MASTER_S", "Thrall"),
                threshold(2),
            ],
            "the method, then the master, then the threshold"
        );

        // The master moves under group loot: stored, silent.
        let mut g = GroupState {
            loot_stored: tail(3, 0, 2),
            ..GroupState::default()
        };
        let (lines, _) = loot_list(&mut g, tail(3, THRALL, 2));
        assert_eq!(lines, []);
        let (lines, _) = loot_list(&mut g, tail(2, THRALL, 2));
        assert_eq!(
            lines,
            [UiError::key("ERR_SET_LOOT_MASTER")],
            "the master was already stored"
        );

        // A master the cache has no name for says nothing, now or after the name arrives.
        let mut g = GroupState {
            loot_stored: tail(2, 0, 2),
            ..GroupState::default()
        };
        let (lines, _) = loot_list(&mut g, tail(2, 0x99, 2));
        assert_eq!(lines, []);
        assert_eq!(g.loot_stored.master, 0x99);

        // No master is no line.
        g.loot_stored = tail(2, THRALL, 2);
        let (lines, _) = loot_list(&mut g, tail(2, 0, 2));
        assert_eq!(lines, []);
    }

    /// The cells are the process's: no list that ends a group clears them (`0x4e86f0` is their
    /// only writer), nor does a session's teardown, so a rejoin at the same settings is silent.
    #[test]
    fn the_stored_cells_outlive_the_group_and_the_session() {
        let mut g = GroupState::default();
        loot_list(&mut g, tail(2, THRALL, 3));
        g.apply_list(0, 0, Vec::new(), 0, None, Some(ME));
        assert!(!g.in_group);
        let (lines, _) = loot_list(&mut g, tail(2, THRALL, 3));
        assert_eq!(lines, [], "the same settings after the group ended");

        g.clear_session();
        let (lines, _) = loot_list(&mut g, tail(2, THRALL, 3));
        assert_eq!(lines, [], "and after the session ended");
        let (lines, _) = loot_list(&mut g, tail(1, THRALL, 3));
        assert_eq!(lines, [UiError::key("ERR_SET_LOOT_ROUNDROBIN")]);
    }

    /// The setter runs after the party's leaves (`0x5e6e09`) and ahead of the raid roster's
    /// legs (`0x5e6e52` before `0x5e6e6b`), so its lines sit between them.
    #[test]
    fn the_loot_lines_sit_between_the_leaves_and_the_raid_legs() {
        let mut g = GroupState::default();
        let list = |g: &mut GroupState, group_type: u8, members: Vec<GroupMemberEntry>, loot| {
            g.apply_list_awaiting(
                group_type,
                0,
                members,
                1,
                Some(loot),
                Some(ME),
                &|_| Some(String::new()),
                &|_| None,
            )
            .lines
        };
        list(
            &mut g,
            0,
            vec![member("Alice", 1), member("Bob", 2)],
            tail(3, 0, 2),
        );
        assert_eq!(
            list(&mut g, 0, vec![member("Alice", 1)], tail(1, 0, 2)),
            [
                UiError::s("ERR_LEFT_GROUP_S", "Bob"),
                UiError::key("ERR_SET_LOOT_ROUNDROBIN"),
            ]
        );

        let mut g = GroupState::default();
        assert_eq!(
            list(&mut g, 1, vec![member("Alice", 1)], tail(3, 0, 2)),
            [
                UiError::key("ERR_SET_LOOT_GROUP"),
                threshold(2),
                UiError::key("ERR_RAID_YOU_JOINED"),
            ]
        );
    }
}
