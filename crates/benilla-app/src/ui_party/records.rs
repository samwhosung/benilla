//! The roster records' readers for a guid the object manager does not hold: the member record
//! (`0x496400`), the pet record (`0x496420`) and the party and raid pets' guids (`0x4e81d0`,
//! `0x491960`).

use benilla_protocol::messages::{member_status, ObjectFields, PartyMemberStatsInfo};

use super::GroupState;

impl GroupState {
    /// The member record for `guid`, party or raid (`0x496400`).
    fn member_record(&self, guid: u64) -> Option<&PartyMemberStatsInfo> {
        self.stats.get(&guid)
    }

    /// The record whose pet block names `guid` (`0x496420`); a pet is found only while its owner's
    /// record reads online, party (`0x4e8860`) or raid (`0x4bb130`).
    pub(crate) fn pet_record(&self, guid: u64) -> Option<&PartyMemberStatsInfo> {
        self.stats
            .values()
            .find(|r| guid != 0 && r.pet_guid == Some(guid) && online(r))
    }

    /// Whether the roster fallback of `UnitExists` (`0x491900`) names `member`'s pet, which the
    /// object manager may not hold and which `partypetN` or `raidpetN` resolved to a guid: a held
    /// member answers off its descriptor's `CHARM`/`SUMMON` (`0x4e814d`, `0x4bb022`), an unheld one
    /// off its record's pet guid while the record reads online (`0x4e816b`, `0x4bb040`). That test
    /// is the party pet's own online gate already, but not the raid pet's guid, which the resolver
    /// reads off the record with none (`0x4919ae`).
    pub(crate) fn roster_names_pet(&self, member: u64, member_held: bool) -> bool {
        member_held || self.member_record(member).is_some_and(online)
    }

    /// `partypetN`'s guid for the member `member` (`0x4e81d0`): `CHARM`, else `SUMMON`, off the
    /// member's descriptor while it is held (`0x4e8204`-`0x4e821a`), else the record's pet guid
    /// while the record reads online (`0x4e8227`-`0x4e8236`).
    pub(crate) fn party_pet_guid(&self, member: u64, live: Option<&ObjectFields>) -> Option<u64> {
        self.pet_guid_of(member, live, true)
    }

    /// `raidpetN`'s guid for the raid member `member` (`0x491960`): `CHARM`, else `SUMMON`, off the
    /// member's descriptor while it is held (`0x491993`-`0x4919a9`), else the record's pet guid
    /// with no online test (`0x4919ae`), where `partypetN` has one.
    pub(crate) fn raid_pet_guid(&self, member: u64, live: Option<&ObjectFields>) -> Option<u64> {
        self.pet_guid_of(member, live, false)
    }

    fn pet_guid_of(
        &self,
        member: u64,
        live: Option<&ObjectFields>,
        online_only: bool,
    ) -> Option<u64> {
        match live {
            Some(fields) => fields.unit_pet_guid(),
            None => self
                .member_record(member)
                .filter(|r| !online_only || online(r))
                .and_then(|r| r.pet_guid)
                .filter(|&g| g != 0),
        }
    }

    /// The aura slots `UnitBuff` and `UnitDebuff` walk for a guid with no live object
    /// (`0x519735`, `0x519b25`), ascending, buffs then debuffs: a member record's block (`+0x1a`)
    /// unless it reads offline, which answers nothing and never tries the pet records
    /// (`0x519741`); on a miss, the block of the pet record naming the guid (`+0xe4`, `0x519760`).
    pub(crate) fn roster_aura_slots(&self, guid: u64) -> impl Iterator<Item = (u8, u16)> + '_ {
        let blocks = match self.member_record(guid) {
            Some(r) if !online(r) => None,
            Some(r) => Some((&r.auras, &r.auras_negative)),
            None => self
                .pet_record(guid)
                .map(|r| (&r.pet_auras, &r.pet_auras_negative)),
        };
        blocks
            .into_iter()
            .flat_map(|(buffs, debuffs)| buffs.iter().chain(debuffs).flatten().copied())
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
        let slots = |g: &GroupState, guid| g.roster_aura_slots(guid).collect::<Vec<_>>();
        assert_eq!(
            slots(&g, MEMBER),
            [(0, 1126), (4, 21562), (32, 589)],
            "buffs then debuffs, ascending slot"
        );
        assert_eq!(slots(&g, PET), [(2, 1126), (40, 770)]);
        assert!(slots(&g, 0x9999).is_empty(), "a miss everywhere");

        let g = group_with(record(member_status::OFFLINE));
        assert!(slots(&g, MEMBER).is_empty());
        assert!(
            slots(&g, PET).is_empty(),
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

    /// `raidpetN` reads the same fields as `partypetN`, but its record leg has no online test
    /// (`0x4919ae`, against `partypetN`'s `0x4e8227`).
    #[test]
    fn the_raid_pet_guid_reads_the_record_whatever_its_online_bit() {
        let g = group_with(record(member_status::ONLINE));
        assert_eq!(g.raid_pet_guid(MEMBER, None), Some(PET));
        let summon = ObjectFields::from_pairs(&[(8, 0x55), (9, 0)]);
        assert_eq!(g.raid_pet_guid(MEMBER, Some(&summon)), Some(0x55));
        assert_eq!(
            g.raid_pet_guid(MEMBER, Some(&ObjectFields::default())),
            None
        );

        let g = group_with(record(member_status::OFFLINE));
        assert_eq!(g.raid_pet_guid(MEMBER, None), Some(PET));
        assert!(
            g.roster_aura_slots(PET).next().is_none(),
            "named, but the pet record is not found behind an offline owner"
        );
    }
}
