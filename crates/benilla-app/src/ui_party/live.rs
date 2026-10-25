//! The group getters' questions at the call, the call host's half of
//! [`benilla_ui::script::GroupCellsQuery`] and its kin: the roster cells and the members' records
//! as of the drain's place ([`crate::ui_unit::views::ViewStores::roster`]), each member found as of
//! there too, each answer derived as the feed derives its snapshot ([`super::feed`]). The reference
//! reads the party slots, the leader cell, the loot cells and the raid rows when the getter runs
//! (`GetNumPartyMembers` `0x4e86d0`, `GetRaidRosterInfo` `0x4bb560`), so a listener reads its own
//! packet's roster.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{
    GroupCells, GroupCellsQuery, GroupRow, MemberOrPetQuery, MemberQuery, Membership,
    RaidMemberInfo, RaidRowQuery, RowQuery, UnitBase,
};

use super::feed::{loot_read, raid_row_at, rank_of, RaidSelf, RowReads};
use super::GroupState;
use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, ObjectStore, SelfPlayer};
use crate::ui_unit::views::ViewStores;

/// What the group getters read as they run.
#[derive(SystemParam)]
pub(crate) struct RosterLive<'w, 's> {
    group: Option<Res<'w, GroupState>>,
    stores: ViewStores<'w, 's>,
    index: Option<Res<'w, GuidIndex>>,
    me: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
    names: Option<Res<'w, NameCache>>,
    tables: crate::ui_unit::NameTables<'w>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    here: benilla_world::world_point::AreaUnderfoot<'w>,
}

impl RosterLive<'_, '_> {
    /// The object the manager holds under `guid`, as of here (`0x468460`).
    fn held(&self, guid: u64) -> Option<&ObjectStore> {
        let entity = self.index.as_deref().and_then(|i| i.0.get(&guid).copied());
        self.stores.at(guid, entity)
    }

    /// Our guid while the object manager holds our object, as of here (`0x468550`), else 0.
    fn own(&self) -> Option<(u64, &ObjectStore)> {
        let (entity, guid) = self.me.iter().next()?;
        Some((guid.0, self.stores.at(guid.0, Some(entity))?))
    }

    /// The cells: `None` in a harness without the roster.
    fn cells(&self) -> Option<GroupCells> {
        let group = self.group.as_deref()?;
        let cells = self.stores.roster(group);
        let own = self.own().map(|(guid, _)| guid);
        let mut party = [0; 4];
        for (slot, m) in party.iter_mut().zip(cells.party_slots()) {
            *slot = m.guid;
        }
        let (method, master_looter, loot_threshold) = loot_read(cells, own);
        Some(GroupCells {
            party,
            raid_count: cells.raid_rows().count() as u32,
            leader_index: cells.leader_index,
            leader_guid: cells.leader,
            own_guid: own.unwrap_or(0),
            loot_method: method.to_string(),
            master_looter,
            loot_threshold,
        })
    }

    /// The raid row at 0-based `index`.
    fn raid_row(&self, index: usize) -> Option<Option<RaidMemberInfo>> {
        let group = self.group.as_deref()?;
        let areas = self.areas.as_deref();
        let me = self.own().map(|(guid, store)| RaidSelf {
            guid,
            level: store.0.unit_level().unwrap_or(0),
            area: self.here.area(),
            dead: store.0.unit_is_dead(),
            class: store.0.unit_class(),
        });
        let zone_name = |area: u32| {
            let areas = areas?;
            // A member's wire zone is already a zone; our own area is the finest leaf, so walk it
            // up. `top_zone` leaves a zone as it is, so one resolver serves both.
            areas
                .0
                .name(areas.0.top_zone(area).unwrap_or(area))
                .map(str::to_string)
        };
        let held = |guid: u64| self.held(guid);
        let reads = RowReads {
            cells: self.stores.roster(group),
            records: self.stores.records(&group.stats),
            me: me.as_ref(),
            held: &held,
            names: self.names.as_deref()?,
            unit_names: self.tables.get(),
            zone_name: &zone_name,
        };
        Some(raid_row_at(&reads, index))
    }

    /// Where `guid` stands: the active player's guid or a party slot's (`0x4e7f70`), a raid row's
    /// (`0x4baee0`) with its rank.
    fn member(&self, guid: u64) -> Option<Membership> {
        let group = self.group.as_deref()?;
        let cells = self.stores.roster(group);
        let own = self.own().map_or(0, |(guid, _)| guid);
        let rank = (guid != 0)
            .then(|| {
                let member = cells.members.iter().find(|m| m.guid == guid);
                match (member, cells.own_raid_row) {
                    _ if !cells.raid_held() => None,
                    (Some(m), _) => Some(rank_of(cells, guid, m.flags)),
                    (None, Some(row)) if row == guid => Some(rank_of(cells, guid, cells.own_flags)),
                    _ => None,
                }
            })
            .flatten();
        Some(Membership {
            party: guid != 0 && (guid == own || cells.party_slots().any(|m| m.guid == guid)),
            raid: rank.is_some(),
            rank,
        })
    }

    /// `0x4e8fa0` (party slots) or `0x4bafd0` (raid rows): `guid` is a row's member, or the pet of
    /// one, which a held member's descriptor names (charm, else summon), else its record while the
    /// record reads online. A zero guid is nobody, and the party's slots hold no player.
    fn member_or_pet(&self, guid: u64, raid: bool) -> Option<bool> {
        let group = self.group.as_deref()?;
        let cells = self.stores.roster(group);
        let records = self.stores.records(&group.stats);
        let rows: Vec<u64> = if raid {
            cells.raid_rows().collect()
        } else {
            cells.party_slots().map(|m| m.guid).collect()
        };
        Some(
            guid != 0
                && rows.into_iter().any(|member| {
                    member == guid
                        || super::records::pet_guid_in(
                            records,
                            member,
                            self.held(member).map(|s| &s.0),
                            true,
                        ) == Some(guid)
                }),
        )
    }

    /// The group row a token names: its member, and the pet its record names.
    fn row(&self, base: UnitBase) -> Option<GroupRow> {
        let group = self.group.as_deref()?;
        let cells = self.stores.roster(group);
        let records = self.stores.records(&group.stats);
        let nth = |row: u32| usize::try_from(row).ok();
        let pet = |member: u64, online_only: bool| {
            super::records::pet_guid_in(records, member, None, online_only).unwrap_or(0)
        };
        let (member, pet_record) = match base {
            UnitBase::Party(row) | UnitBase::PartyPet(row) => {
                let member = nth(row)
                    .and_then(|row| cells.party_slots().nth(row))
                    .map_or(0, |m| m.guid);
                (member, pet(member, true))
            }
            UnitBase::Raid(row) | UnitBase::RaidPet(row) => {
                let member = nth(row)
                    .and_then(|row| cells.raid_rows().nth(row))
                    .unwrap_or(0);
                (member, pet(member, false))
            }
            _ => return None,
        };
        Some(GroupRow { member, pet_record })
    }
}

impl crate::script_calls::live::LiveFamily for RosterLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|_: &GroupCellsQuery| self.cells())
            || ask.answer(|q: &RaidRowQuery| self.raid_row(q.0))
            || ask.answer(|q: &MemberQuery| self.member(q.0))
            || ask.answer(|q: &RowQuery| self.row(q.0))
            || ask.answer(|q: &MemberOrPetQuery| self.member_or_pet(q.guid, q.raid))
    }
}
