//! The group getters' questions of the roster, asked of the lent host at the call
//! ([`crate::script::live::ask`]): the reference's getters read the party slots (`0xbc6f48`), the
//! leader cell (`0xbc75f8`), the loot cells and the raid rows (`0xb712a8`) as they stand when they
//! run, so a listener reads the roster its own packet left. With no host free to ask, the no-host
//! default ([`super::PartyState`]) answers: ungrouped, as the app never pushes one, unless a
//! host-less test seats it.

use mlua::Lua;

use super::live::{ask, LiveQuery};
use super::party::{PartyState, RaidMemberInfo};
use super::unit::UnitBase;

/// The cells a getter reads beside the raid rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupCells {
    /// The four party slots' guids, 0 for an empty one (`0xbc6f48`); `GetNumPartyMembers` counts
    /// the nonzero (`0x4e86d0`) and `GetPartyMember(n)` tests slot n (`0x4e81a0`).
    pub party: [u64; 4],
    /// The raid roster's count (`[0xb713e0]`), `GetNumRaidMembers`'s answer.
    pub raid_count: u32,
    /// `GetPartyLeaderIndex`: the index cell plus one (`0x4e9104`).
    pub leader_index: u32,
    /// The leader cell (`[0xbc75f8]`), which `IsRaidLeader` (`0x4bb8c0`), `IsPartyLeader`
    /// (`0x4e9130`) and `UnitIsPartyLeader` (`0x516210`) compare.
    pub leader_guid: u64,
    /// The active player's guid (`0x468550`), 0 out of world.
    pub own_guid: u64,
    /// `GetLootMethod`'s method word (`0x4e9209`), `"freeforall"` before any group.
    pub loot_method: String,
    /// `GetLootMethod`'s second return, the master looter as a party index.
    pub master_looter: Option<u32>,
    /// `GetLootThreshold` (`0x4e94e0`).
    pub loot_threshold: u32,
}

impl GroupCells {
    /// The cells off the snapshot.
    fn of(party: &PartyState) -> Self {
        let mut slots = [0; 4];
        for (slot, m) in slots.iter_mut().zip(&party.members) {
            *slot = m.guid;
        }
        Self {
            party: slots,
            raid_count: party.raid.len() as u32,
            leader_index: party.leader_index,
            leader_guid: party.leader_guid,
            own_guid: party.own_guid,
            loot_method: if party.loot_method.is_empty() {
                // Never grouped, the reference reads zero-filled cells: method 0.
                "freeforall".to_string()
            } else {
                party.loot_method.clone()
            },
            master_looter: party.master_looter,
            loot_threshold: party.loot_threshold,
        }
    }

    /// `IsRaidLeader` (`0x4bb8c0`): the cached leader guid (`[0xbc75f8]`, `[0xbc75fc]`) against the
    /// player's own (`0x468550`), both words (`0x4bb8d6`-`0x4bb8dc`), and nothing else: no raid flag,
    /// so it is true for a party leader, and no subgroup slot. Unlike `IsPartyLeader` it has no zero
    /// guard, so with no active player and no group, both zero, it answers 1.
    pub fn is_raid_leader(&self) -> bool {
        self.leader_guid == self.own_guid
    }

    /// `IsPartyLeader` (`0x4e9130`): [`Self::is_raid_leader`]'s compare behind a zero guard on the
    /// leader cell (`0x4e9141 or ecx,eax; je`), so an ungrouped player is nobody's leader.
    pub fn is_party_leader(&self) -> bool {
        self.leader_guid != 0 && self.is_raid_leader()
    }

    /// How many of the four party slots hold a guid (`0x4e86d0`).
    pub fn party_count(&self) -> u32 {
        self.party.iter().filter(|&&g| g != 0).count() as u32
    }
}

/// The cells ([`GroupCells`]).
pub struct GroupCellsQuery;

impl LiveQuery for GroupCellsQuery {
    type Answer = GroupCells;
}

/// The cells through the lent host, `None` where none answers.
pub(crate) fn live_cells(lua: &Lua) -> Option<GroupCells> {
    ask(lua, &GroupCellsQuery)
}

/// The cells, live where a host answers, else off the snapshot.
pub(crate) fn cells(lua: &Lua, snapshot: &PartyState) -> GroupCells {
    live_cells(lua).unwrap_or_else(|| GroupCells::of(snapshot))
}

/// The raid row at 0-based `row` as `GetRaidRosterInfo` reads it (`0x4bb560`): `None` past the
/// count, for an empty slot and for a name the cache lacks.
pub struct RaidRowQuery(pub usize);

impl LiveQuery for RaidRowQuery {
    type Answer = Option<RaidMemberInfo>;
}

/// The row, live where a host answers, else off the snapshot.
pub(crate) fn raid_row(lua: &Lua, snapshot: &PartyState, row: usize) -> Option<RaidMemberInfo> {
    ask(lua, &RaidRowQuery(row)).unwrap_or_else(|| snapshot.raid.get(row).cloned())
}

/// Where a guid stands in the group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Membership {
    /// The active player's guid or a party slot's (`0x4e7f70`); a zero guid is neither.
    pub party: bool,
    /// On the raid roster below its count (`0x4baee0`); a zero guid is not.
    pub raid: bool,
    /// The raid row's rank (`[row+0xc]`), `None` off the roster.
    pub rank: Option<u32>,
}

/// Where `guid` stands ([`Membership`]).
pub struct MemberQuery(pub u64);

impl LiveQuery for MemberQuery {
    type Answer = Membership;
}

/// `guid`'s place in the group through the lent host, `None` where none answers.
pub(crate) fn live_membership(lua: &Lua, guid: u64) -> Option<Membership> {
    ask(lua, &MemberQuery(guid))
}

/// Whether `guid` is a member of the party slots or of the raid roster, or the pet of one
/// (`0x4e8fa0`, `0x4bafd0`): the member's own guid, else its object's charm or summon while the
/// object is held, else its record's pet while the record reads online. A zero guid is nobody, and
/// the party's test never matches the active player, who is no slot; the raid's roster holds us.
pub struct MemberOrPetQuery {
    pub guid: u64,
    /// The raid roster's test, else the party slots'.
    pub raid: bool,
}

impl LiveQuery for MemberOrPetQuery {
    type Answer = bool;
}

/// [`MemberOrPetQuery`] through the lent host, `None` where none answers.
pub(crate) fn live_member_or_pet(lua: &Lua, guid: u64, raid: bool) -> Option<bool> {
    ask(lua, &MemberOrPetQuery { guid, raid })
}

/// A group row the unit-token resolver reads (`0x515970`): `partyN`'s guid (`0x4e81a0`), `raidN`'s
/// (`0x491940`), and for the pet tokens the member's record pet while its object is not held
/// (`0x4e81d0`, `0x491960`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowQuery(pub UnitBase);

/// The row's member and the pet its record names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupRow {
    pub member: u64,
    pub pet_record: u64,
}

impl LiveQuery for RowQuery {
    type Answer = GroupRow;
}

/// Every group row the token walk reads as an event names its unit (`0x515c50`): party rows 1 to
/// 4 (`0x491920`, `0x491930`) and raid rows 1 to 40 (`0x491940`, `0x491960`), each as
/// [`RowQuery`] answers it, at the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupRowsQuery;

/// The party rows and the raid rows, an empty row past the roster's end.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupRows {
    pub party: [GroupRow; 4],
    pub raid: Vec<GroupRow>,
}

impl LiveQuery for GroupRowsQuery {
    type Answer = GroupRows;
}

/// `guid`'s place in the group, live where a host answers, else off the snapshot: the active
/// player or a party slot (`0x4e7f70`), a raid row below the count (`0x4baee0`) with its rank.
pub(crate) fn membership(lua: &Lua, snapshot: &PartyState, guid: u64) -> Membership {
    if let Some(live) = live_membership(lua, guid) {
        return live;
    }
    let rank = snapshot
        .raid
        .iter()
        .find(|m| guid != 0 && m.guid == guid)
        .map(|m| m.rank);
    Membership {
        party: guid != 0
            && (guid == snapshot.own_guid
                || snapshot.members.iter().take(4).any(|m| m.guid == guid)),
        raid: rank.is_some(),
        rank,
    }
}

/// The raid rows below the count (`[0xb712a8]`, `[0xb713e0]`) as `GetRaidRosterInfo` reads them,
/// live where a host answers, else off the snapshot.
pub(crate) fn raid_rows(lua: &Lua, snapshot: &PartyState) -> Vec<RaidMemberInfo> {
    let Some(cells) = live_cells(lua) else {
        return snapshot.raid.clone();
    };
    (0..cells.raid_count as usize)
        .filter_map(|row| ask(lua, &RaidRowQuery(row)).flatten())
        .collect()
}

/// The raid rows' guids below the count, live where a host answers, else off the snapshot.
pub(crate) fn raid_guids(lua: &Lua, snapshot: &PartyState) -> Vec<u64> {
    let Some(cells) = live_cells(lua) else {
        return snapshot.raid.iter().map(|m| m.guid).collect();
    };
    (0..cells.raid_count)
        .filter_map(|row| ask(lua, &RowQuery(UnitBase::Raid(row))).map(|r| r.member))
        .collect()
}

/// One group member as the by-name resolver's roster passes see it (`0x493b22`, `0x493ba7`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterName {
    /// The member's name; empty for none, which the matcher skips (`0x493c68`).
    pub name: String,
    pub guid: u64,
    /// The object manager holds a unit under the guid (`0x493b46`, `0x493bde`).
    pub held: bool,
}

/// The party slots and the raid rows below the count, as the roster passes walk them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RosterNames {
    pub party: Vec<RosterName>,
    pub raid: Vec<RosterName>,
}

/// The roster passes' members ([`RosterNames`]).
pub struct RosterNamesQuery;

impl LiveQuery for RosterNamesQuery {
    type Answer = RosterNames;
}

/// The roster passes' members through the lent host, `None` where none answers.
pub(crate) fn live_roster_names(lua: &Lua) -> Option<RosterNames> {
    ask(lua, &RosterNamesQuery)
}
