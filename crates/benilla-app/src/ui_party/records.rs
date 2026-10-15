//! The roster records' readers for a guid the object manager does not hold: the member record
//! (`0x496400`), the pet record (`0x496420`) and the party and raid pets' guids (`0x4e81d0`,
//! `0x491960`); and the unit events a stats packet's change of a record raises (`0x5e5720`).

use benilla_protocol::messages::{ObjectFields, PartyMemberStatsInfo};

use super::GroupState;
use crate::field_watch::{id, FieldNotice};

/// `0x5e5510(old, new)`: the bits of what the stats packet changed in the member's record. The
/// record's zero is an absent field here.
fn record_changes(old: &PartyMemberStatsInfo, new: &PartyMemberStatsInfo) -> u32 {
    fn differ<T: PartialEq + Default + Copy>(a: Option<T>, b: Option<T>) -> bool {
        a.unwrap_or_default() != b.unwrap_or_default()
    }
    fn auras(a: &Option<Vec<(u8, u16)>>, b: &Option<Vec<(u8, u16)>>) -> bool {
        a.as_deref().unwrap_or_default() != b.as_deref().unwrap_or_default()
    }
    let arms: [(bool, u32); 21] = [
        (differ(old.status, new.status), 0x1),
        (differ(old.cur_hp, new.cur_hp), 0x2),
        (differ(old.max_hp, new.max_hp), 0x4),
        (differ(old.power_type, new.power_type), 0x8),
        (differ(old.cur_power, new.cur_power), 0x10),
        (differ(old.max_power, new.max_power), 0x20),
        (differ(old.level, new.level), 0x40),
        (differ(old.zone, new.zone), 0x80),
        (differ(old.position, new.position), 0x100),
        (auras(&old.auras, &new.auras), 0x200),
        (auras(&old.auras_negative, &new.auras_negative), 0x400),
        (differ(old.pet_guid, new.pet_guid), 0x800),
        (
            old.pet_name.as_deref().unwrap_or_default()
                != new.pet_name.as_deref().unwrap_or_default(),
            0x1000,
        ),
        (differ(old.pet_model_id, new.pet_model_id), 0x2000),
        (differ(old.pet_cur_hp, new.pet_cur_hp), 0x4000),
        (differ(old.pet_max_hp, new.pet_max_hp), 0x8000),
        (differ(old.pet_power_type, new.pet_power_type), 0x10000),
        (differ(old.pet_cur_power, new.pet_cur_power), 0x20000),
        (differ(old.pet_max_power, new.pet_max_power), 0x40000),
        (auras(&old.pet_auras, &new.pet_auras), 0x80000),
        (
            auras(&old.pet_auras_negative, &new.pet_auras_negative),
            0x100000,
        ),
    ];
    arms.iter()
        .filter(|(moved, _)| *moved)
        .fold(0, |mask, (_, bit)| mask | bit)
}

/// The unit events a stats packet raises for `member`, whose record went from `old` to `new`
/// (`0x5e54fa`): `0x5e5720` through `0x515e50`, in its order. A status change signals the health
/// and the power and drops their own bits (`0x5e572f`-`0x5e574a`); then the level, health, maximum,
/// display power, power and maximum power, `UNIT_FACTION` for the PvP and free-for-all status bits
/// (`0x5e57c8`), `UNIT_AURA` for either aura block and `UNIT_PET` for a new pet (`0x5e584f`).
/// With a pet the record names, its own: the name, health, maximum, display power, power, maximum
/// power and auras (`0x5e586c`-`0x5e5900`). The power events add the new record's power type.
pub(crate) fn record_notices(
    member: u64,
    old: &PartyMemberStatsInfo,
    new: &PartyMemberStatsInfo,
) -> Vec<FieldNotice> {
    let mut mask = record_changes(old, new);
    let mut out = Vec::new();
    let mut signal = |guid: u64, id: u16| out.push(FieldNotice::Signal { guid, id });
    let power = u16::from(new.power_type.unwrap_or(0));
    if mask & 0x1 != 0 {
        signal(member, id::UNIT_HEALTH);
        signal(member, id::POWER + power);
        mask &= !0x12;
    }
    for (bit, event) in [
        (0x40, id::UNIT_LEVEL),
        (0x2, id::UNIT_HEALTH),
        (0x4, id::UNIT_MAXHEALTH),
        (0x8, id::UNIT_DISPLAYPOWER),
        (0x10, id::POWER + power),
        (0x20, id::MAX_POWER + power),
    ] {
        if mask & bit != 0 {
            signal(member, event);
        }
    }
    if (old.status.unwrap_or(0) ^ new.status.unwrap_or(0)) & 0x12 != 0 {
        signal(member, id::UNIT_FACTION);
    }
    if mask & 0x600 != 0 {
        signal(member, id::UNIT_AURA);
    }
    if mask & 0x800 != 0 {
        signal(member, id::UNIT_PET);
    }
    let pet = new.pet_guid.unwrap_or(0);
    if pet != 0 {
        let power = u16::from(new.pet_power_type.unwrap_or(0));
        // 0x2000, the display, refreshes the portrait alone (`0x5e587f`).
        for (bit, event) in [
            (0x1000, id::UNIT_NAME_UPDATE),
            (0x4000, id::UNIT_HEALTH),
            (0x8000, id::UNIT_MAXHEALTH),
            (0x10000, id::UNIT_DISPLAYPOWER),
            (0x20000, id::POWER + power),
            (0x40000, id::MAX_POWER + power),
            (0x180000, id::UNIT_AURA),
        ] {
            if mask & bit != 0 {
                signal(pet, event);
            }
        }
    }
    out
}

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
            .find(|r| guid != 0 && r.pet_guid == Some(guid) && r.is_online())
    }

    /// Whether the roster fallback of `UnitExists` (`0x491900`) names `member`'s pet, which the
    /// object manager may not hold and which `partypetN` or `raidpetN` resolved to a guid: a held
    /// member answers off its descriptor's `CHARM`/`SUMMON` (`0x4e814d`, `0x4bb022`), an unheld one
    /// off its record's pet guid while the record reads online (`0x4e816b`, `0x4bb040`). That test
    /// is the party pet's own online gate already, but not the raid pet's guid, which the resolver
    /// reads off the record with none (`0x4919ae`).
    pub(crate) fn roster_names_pet(&self, member: u64, member_held: bool) -> bool {
        member_held
            || self
                .member_record(member)
                .is_some_and(PartyMemberStatsInfo::is_online)
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
                .filter(|r| !online_only || r.is_online())
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
            Some(r) if !r.is_online() => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{member_status, GroupMemberEntry};

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

    /// `0x4e82d0` copies the saved record back and then sets or clears bit 0 of its status byte
    /// from the row's (`0x4e8361`-`0x4e837b`); the raid roster's twin does the same to its row's
    /// record (`0x4ba947`-`0x4ba960`). Both test the row's byte as non-zero.
    #[test]
    fn a_list_rewrites_a_known_members_online_bit_and_keeps_the_rest_of_the_record() {
        use crate::ui_party::GROUPTYPE_RAID;
        for group_type in [0, GROUPTYPE_RAID] {
            let mut g = group_with(record(
                member_status::ONLINE | member_status::DEAD | member_status::PVP,
            ));
            let relist = |g: &mut GroupState, status: u8| {
                let row = |guid, name: &str| GroupMemberEntry {
                    name: name.into(),
                    guid,
                    status,
                    flags: 0,
                };
                g.apply_list(
                    group_type,
                    0,
                    vec![row(MEMBER, "Brisca"), row(0x5678, "Aldwyn")],
                    MEMBER,
                    None,
                    None,
                );
            };

            relist(&mut g, member_status::OFFLINE);
            assert_eq!(
                g.stats[&MEMBER],
                record(member_status::DEAD | member_status::PVP),
                "the bit clears and the rest of the record stays as it was"
            );
            assert!(g.roster_aura_slots(MEMBER).next().is_none());
            assert_eq!(g.party_pet_guid(MEMBER, None), None);

            relist(&mut g, member_status::ONLINE | member_status::AFK);
            assert_eq!(
                g.stats[&MEMBER],
                record(member_status::ONLINE | member_status::DEAD | member_status::PVP),
                "the bit sets, and the row's other bits are not copied"
            );
            assert_eq!(g.party_pet_guid(MEMBER, None), Some(PET));

            relist(&mut g, member_status::AFK);
            assert!(
                g.stats[&MEMBER].is_online(),
                "a non-zero byte is online, bit 0 or not"
            );
            assert!(
                !g.stats.contains_key(&0x5678),
                "a member new to the roster gets its record from the seat, not here"
            );
        }
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

    fn signals(pairs: &[(u64, u16)]) -> Vec<FieldNotice> {
        pairs
            .iter()
            .map(|&(guid, id)| FieldNotice::Signal { guid, id })
            .collect()
    }

    /// A delta's fields each raise their own event for the member, in `0x5e5720`'s order and not
    /// the wire's; the power events add the new power type; zone and position raise nothing.
    #[test]
    fn each_changed_field_raises_its_event_in_the_handlers_order() {
        let old = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE),
            cur_hp: Some(100),
            max_hp: Some(200),
            power_type: Some(1),
            cur_power: Some(10),
            max_power: Some(100),
            level: Some(40),
            ..Default::default()
        };
        let new = PartyMemberStatsInfo {
            cur_hp: Some(90),
            max_hp: Some(210),
            cur_power: Some(20),
            max_power: Some(110),
            level: Some(41),
            zone: Some(12),
            position: Some((5, 6)),
            ..old.clone()
        };
        // Rage, power type 1: `UNIT_RAGE` 18 and `UNIT_MAXRAGE` 24.
        assert_eq!(
            record_notices(MEMBER, &old, &new),
            signals(&[
                (MEMBER, 28),
                (MEMBER, 16),
                (MEMBER, 22),
                (MEMBER, 18),
                (MEMBER, 24)
            ])
        );
        let moved = PartyMemberStatsInfo {
            zone: Some(1),
            position: Some((0, 1)),
            ..old.clone()
        };
        assert!(record_notices(MEMBER, &old, &moved).is_empty());
        assert!(record_notices(MEMBER, &old, &old).is_empty());
    }

    /// A status change raises the health and the power and drops their own bits, so a death in one
    /// delta is one of each; the PvP and free-for-all bits add `UNIT_FACTION`, the dead bit not.
    #[test]
    fn a_status_change_raises_health_power_and_on_pvp_the_faction() {
        let old = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE),
            cur_hp: Some(100),
            power_type: Some(0),
            cur_power: Some(50),
            ..Default::default()
        };
        let dead = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::DEAD),
            cur_hp: Some(0),
            cur_power: Some(0),
            ..old.clone()
        };
        assert_eq!(
            record_notices(MEMBER, &old, &dead),
            signals(&[(MEMBER, 16), (MEMBER, 17)])
        );
        let flagged = PartyMemberStatsInfo {
            status: Some(member_status::ONLINE | member_status::PVP),
            ..old.clone()
        };
        assert_eq!(
            record_notices(MEMBER, &old, &flagged),
            signals(&[(MEMBER, 16), (MEMBER, 17), (MEMBER, 29)])
        );
    }

    /// Either aura block raises one `UNIT_AURA`; a new pet guid `UNIT_PET` for the member; the
    /// pet's own fields their events for the pet, the display none.
    #[test]
    fn the_auras_the_pet_and_its_fields_raise_theirs() {
        let old = record(member_status::ONLINE);
        let new = PartyMemberStatsInfo {
            auras: Some(vec![(0, 1126)]),
            auras_negative: Some(vec![]),
            ..old.clone()
        };
        assert_eq!(record_notices(MEMBER, &old, &new), signals(&[(MEMBER, 41)]));
        let other_pet = PartyMemberStatsInfo {
            pet_guid: Some(PET + 1),
            ..old.clone()
        };
        assert_eq!(
            record_notices(MEMBER, &old, &other_pet),
            signals(&[(MEMBER, 0)])
        );
        let pet_moved = PartyMemberStatsInfo {
            pet_name: Some("Fang".into()),
            pet_model_id: Some(903),
            pet_cur_hp: Some(5),
            pet_power_type: Some(2),
            pet_cur_power: Some(7),
            pet_auras: Some(vec![]),
            ..old.clone()
        };
        // Focus, power type 2: `UNIT_FOCUS` 19.
        assert_eq!(
            record_notices(MEMBER, &old, &pet_moved),
            signals(&[(PET, 183), (PET, 16), (PET, 30), (PET, 19), (PET, 41)])
        );
        // With no pet the record names, the pet's bits raise nothing.
        let petless = PartyMemberStatsInfo {
            pet_guid: None,
            ..pet_moved.clone()
        };
        let without = PartyMemberStatsInfo {
            pet_guid: None,
            ..old.clone()
        };
        assert!(record_notices(MEMBER, &without, &petless).is_empty());
    }
}
