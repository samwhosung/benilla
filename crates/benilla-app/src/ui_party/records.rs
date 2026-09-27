//! The roster records' readers for a guid the object manager does not hold: the member record
//! (`0x496400`), the pet record (`0x496420`) and the party pet's guid (`0x4e81d0`).

use benilla_protocol::messages::{member_status, ObjectFields, PartyMemberStatsInfo};

use super::GroupState;

impl GroupState {
    /// The member record for `guid`, party or raid (`0x496400`).
    fn member_record(&self, guid: u64) -> Option<&PartyMemberStatsInfo> {
        self.stats.get(&guid)
    }

    /// The record whose pet block names `guid` (`0x496420`); a pet is found only while its owner's
    /// record reads online (`0x4e8860`).
    fn pet_record(&self, guid: u64) -> Option<&PartyMemberStatsInfo> {
        self.stats
            .values()
            .find(|r| guid != 0 && r.pet_guid == Some(guid) && online(r))
    }

    /// `partypetN`'s guid for the member `member` (`0x4e81d0`): `CHARM`, else `SUMMON`, off the
    /// member's descriptor while it is held (`0x4e8204`-`0x4e821a`), else the record's pet guid
    /// while the record reads online (`0x4e8227`-`0x4e8236`).
    pub(crate) fn party_pet_guid(&self, member: u64, live: Option<&ObjectFields>) -> Option<u64> {
        match live {
            Some(fields) => fields.unit_pet_guid(),
            None => self
                .member_record(member)
                .filter(|r| online(r))
                .and_then(|r| r.pet_guid)
                .filter(|&g| g != 0),
        }
    }

    /// The aura slots `UnitBuff` and `UnitDebuff` walk for a guid with no live object
    /// (`0x519735`, `0x519b25`), ascending, buffs then debuffs: a member record's block (`+0x1a`)
    /// unless it reads offline, which answers nothing and never tries the pet records
    /// (`0x519741`); on a miss, the block of the pet record naming the guid (`+0xe4`, `0x519760`).
    pub(crate) fn roster_aura_slots(&self, guid: u64) -> Vec<(u8, u16)> {
        let (buffs, debuffs) = match self.member_record(guid) {
            Some(r) if !online(r) => return Vec::new(),
            Some(r) => (&r.auras, &r.auras_negative),
            None => match self.pet_record(guid) {
                Some(r) => (&r.pet_auras, &r.pet_auras_negative),
                None => return Vec::new(),
            },
        };
        buffs.iter().chain(debuffs).flatten().copied().collect()
    }
}

/// The record's status bit 0 (`[rec+8] & 1`), the online bit.
fn online(r: &PartyMemberStatsInfo) -> bool {
    r.status.unwrap_or(0) & member_status::ONLINE != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::GroupMemberEntry;

    const MEMBER: u64 = 0x1234;
    const PET: u64 = 0xF140_0000_0000_0077;

    fn group_with(record: PartyMemberStatsInfo) -> GroupState {
        let mut g = GroupState::default();
        g.apply_list(
            0,
            0,
            vec![GroupMemberEntry {
                name: "Brisca".into(),
                guid: MEMBER,
                status: member_status::ONLINE,
                flags: 0,
            }],
            MEMBER,
            None,
            None,
        );
        g.stats.insert(MEMBER, record);
        g
    }

    fn record(status: u8) -> PartyMemberStatsInfo {
        PartyMemberStatsInfo {
            status: Some(status),
            auras: Some(vec![(0, 1126), (4, 21562)]),
            auras_negative: Some(vec![(32, 589)]),
            pet_guid: Some(PET),
            pet_auras: Some(vec![(2, 1126)]),
            pet_auras_negative: Some(vec![(40, 770)]),
            ..PartyMemberStatsInfo::default()
        }
    }

    #[test]
    fn a_member_record_answers_its_own_block_and_an_offline_one_answers_nothing() {
        let g = group_with(record(member_status::ONLINE));
        assert_eq!(
            g.roster_aura_slots(MEMBER),
            [(0, 1126), (4, 21562), (32, 589)],
            "buffs then debuffs, ascending slot"
        );
        assert_eq!(g.roster_aura_slots(PET), [(2, 1126), (40, 770)]);
        assert!(g.roster_aura_slots(0x9999).is_empty(), "a miss everywhere");

        let g = group_with(record(member_status::OFFLINE));
        assert!(g.roster_aura_slots(MEMBER).is_empty());
        assert!(
            g.roster_aura_slots(PET).is_empty(),
            "an offline owner's pet is not found"
        );
    }

    #[test]
    fn the_party_pet_guid_is_the_live_charm_then_summon_else_the_records() {
        let g = group_with(record(member_status::ONLINE));
        assert_eq!(g.party_pet_guid(MEMBER, None), Some(PET));

        let summon = ObjectFields::from_pairs(&[(8, 0x55), (9, 0)]);
        assert_eq!(g.party_pet_guid(MEMBER, Some(&summon)), Some(0x55));
        let both = ObjectFields::from_pairs(&[(6, 0x66), (7, 0), (8, 0x55), (9, 0)]);
        assert_eq!(
            g.party_pet_guid(MEMBER, Some(&both)),
            Some(0x66),
            "CHARM first"
        );
        // A held member with neither is petless, whatever the record says.
        assert_eq!(
            g.party_pet_guid(MEMBER, Some(&ObjectFields::default())),
            None
        );

        let g = group_with(record(member_status::OFFLINE));
        assert_eq!(g.party_pet_guid(MEMBER, None), None);
    }
}
