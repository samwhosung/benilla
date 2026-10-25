//! The roster cells a group getter reads, as a borrow of the live state ([`Cells`]) or an owned
//! copy of them ([`RosterCells`]): the party slots, the raid rows, the leader cells and the loot
//! cells. A packet that moves any logs the copy it found ([`crate::ui_unit::views::ViewLog`]), so
//! a listener reads its own packet's cells ([`crate::ui_unit::views::ViewStores::roster`]).

use benilla_protocol::messages::{GroupLootInfo, GroupMemberEntry};

use super::GroupState;

/// The cells a group getter reads, borrowed from the live [`GroupState`] or from a copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cells<'a> {
    pub(crate) members: &'a [GroupMemberEntry],
    pub(crate) own_flags: u8,
    pub(crate) own_raid_row: Option<u64>,
    pub(crate) slots_emptied: bool,
    pub(crate) leader: u64,
    pub(crate) leader_index: u32,
    pub(crate) raid_leader: u64,
    pub(crate) loot: Option<GroupLootInfo>,
    pub(crate) loot_stored: GroupLootInfo,
    pub(crate) test: bool,
}

/// [`Cells`] owned: what a packet found before it moved them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RosterCells {
    members: Vec<GroupMemberEntry>,
    own_flags: u8,
    own_raid_row: Option<u64>,
    slots_emptied: bool,
    leader: u64,
    leader_index: u32,
    raid_leader: u64,
    loot: Option<GroupLootInfo>,
    loot_stored: GroupLootInfo,
    test: bool,
}

impl<'a> Cells<'a> {
    pub(crate) fn to_owned(self) -> RosterCells {
        RosterCells {
            members: self.members.to_vec(),
            own_flags: self.own_flags,
            own_raid_row: self.own_raid_row,
            slots_emptied: self.slots_emptied,
            leader: self.leader,
            leader_index: self.leader_index,
            raid_leader: self.raid_leader,
            loot: self.loot,
            loot_stored: self.loot_stored,
            test: self.test,
        }
    }

    /// The `party1..party4` slots: our own subgroup in packet order, at most four entries counted
    /// (`0x5e6baa` compares the flags under `0x7f`, `0x5e6bb8` bounds the count, `0x5e6c4f`
    /// counts the entry), none once an ending emptied them. `0x4e82d0` skips a zero guid
    /// (`0x4e82eb`) and fills the first empty slot, so the slots are compact: `[A, 0, B]` puts B in
    /// `party2`.
    pub(crate) fn party_slots(self) -> impl Iterator<Item = &'a GroupMemberEntry> {
        let own = self.own_flags & 0x7f;
        let emptied = self.slots_emptied;
        self.members
            .iter()
            .filter(move |m| !emptied && m.flags & 0x7f == own)
            .take(4)
            .filter(|m| m.guid != 0)
    }

    /// Whether the raid roster holds rows (`[0xb713e0]` above 0).
    pub(crate) fn raid_held(self) -> bool {
        self.own_raid_row.is_some()
    }

    /// The raid roster's rows in order: the wire's members as listed, then us; empty while no
    /// roster is held.
    pub(crate) fn raid_rows(self) -> impl Iterator<Item = u64> + 'a {
        let held = self.own_raid_row.is_some();
        self.members
            .iter()
            .filter(move |_| held)
            .map(|m| m.guid)
            .chain(self.own_raid_row)
    }

    /// The loot cells `GetLootMethod` and `GetLootThreshold` read (`0x4e91b6`, `0x4e91fb`,
    /// `0x4e94e0`): the stored cells, or in the `/partytest` sandbox the sandbox's own tail.
    pub(crate) fn loot_cells(self) -> GroupLootInfo {
        match self.loot {
            Some(sandbox) if self.test => sandbox,
            _ => self.loot_stored,
        }
    }
}

impl RosterCells {
    pub(crate) fn cells(&self) -> Cells<'_> {
        Cells {
            members: &self.members,
            own_flags: self.own_flags,
            own_raid_row: self.own_raid_row,
            slots_emptied: self.slots_emptied,
            leader: self.leader,
            leader_index: self.leader_index,
            raid_leader: self.raid_leader,
            loot: self.loot,
            loot_stored: self.loot_stored,
            test: self.test,
        }
    }
}

impl GroupState {
    /// The roster cells as they stand.
    pub(crate) fn cells(&self) -> Cells<'_> {
        Cells {
            members: &self.members,
            own_flags: self.own_flags,
            own_raid_row: self.own_raid_row,
            slots_emptied: self.slots_emptied,
            leader: self.leader,
            leader_index: self.leader_index,
            raid_leader: self.raid_leader,
            loot: self.loot,
            loot_stored: self.loot_stored,
            test: self.test,
        }
    }
}

impl GroupState {
    /// Swap the roster cells with `other`: the views lane seats the cells as of its place while it
    /// builds, and puts the live ones back.
    pub(crate) fn swap_cells(&mut self, other: &mut RosterCells) {
        std::mem::swap(&mut self.members, &mut other.members);
        std::mem::swap(&mut self.own_flags, &mut other.own_flags);
        std::mem::swap(&mut self.own_raid_row, &mut other.own_raid_row);
        std::mem::swap(&mut self.slots_emptied, &mut other.slots_emptied);
        std::mem::swap(&mut self.leader, &mut other.leader);
        std::mem::swap(&mut self.leader_index, &mut other.leader_index);
        std::mem::swap(&mut self.raid_leader, &mut other.raid_leader);
        std::mem::swap(&mut self.loot, &mut other.loot);
        std::mem::swap(&mut self.loot_stored, &mut other.loot_stored);
        std::mem::swap(&mut self.test, &mut other.test);
    }
}
