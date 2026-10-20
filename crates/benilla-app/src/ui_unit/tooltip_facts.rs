//! What the unit tooltip builder `0x529fe0` reads of a unit beyond what the `Unit*` getters do,
//! resolved here where the caches and the object manager live: the owner line (`0x52fd30`) and
//! the studied-creature block (`0x529a40`).

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::ObjectType;
use benilla_ui::script::{
    OwnerTitle, OwnerTitleKind, PetAbility, StudiedCreature, UnitTooltipFacts,
};
use bevy::prelude::Resource;

use crate::names::NameCache;
use crate::net::ObjectStore;
use crate::query_cache::Ask;

/// What [`tooltip_facts`] reads beside the unit's own descriptor.
#[derive(Clone, Copy)]
pub(crate) struct TooltipSources<'a> {
    pub(crate) names: &'a NameCache,
    /// `Spell.dbc`, which names the spell a unit was created by and a studied creature's pet
    /// abilities; absent without game data.
    pub(crate) catalog: Option<&'a SpellCatalog>,
    /// The object manager's lookup by guid (`0x468460`): the owner line names an owner in sight.
    pub(crate) object: &'a dyn Fn(u64) -> Option<&'a ObjectStore>,
    pub(crate) study: StudySources<'a>,
}

/// `Resistances.dbc`'s armor school and `CreatureSpellData.dbc`, which the studied-creature block
/// reads beside the family tables; each part absent without its file.
#[derive(Resource, Default)]
pub(crate) struct StudyTables {
    /// `0x53f830`'s school, `None` with no row flagged ([`benilla_formats::load_armor_school`]).
    pub(crate) armor_school: Option<u8>,
    pub(crate) pet_spells: Option<benilla_formats::CreatureSpellData>,
}

/// The client tables the studied-creature block reads; absent ones resolve no diet or ability.
#[derive(Clone, Copy, Default)]
pub(crate) struct StudySources<'a> {
    pub(crate) tables: Option<&'a StudyTables>,
    /// `CreatureFamily.dbc` and `ItemPetFood.dbc`, the diet.
    pub(crate) families: Option<&'a crate::ui_pet_stats::PetFamilyTables>,
}

/// The tooltip facts of the unit `store`, whose creature type (`0x605570`) is `creature_type`.
pub(crate) fn tooltip_facts(
    store: &ObjectStore,
    creature_type: u32,
    src: &TooltipSources<'_>,
) -> UnitTooltipFacts {
    UnitTooltipFacts {
        health: store.0.unit_health().unwrap_or(0),
        owner: owner_title(store, creature_type, src),
        // The group record rides on the member's own party or raid row, where the builder finds
        // it by guid ([`UnitTooltipFacts::party`]).
        party: None,
        studied: studied(store, src),
    }
}

/// `UNIT_DYNFLAG_SPECIALINFO`, the studied flag the builder tests (`0x52a8ea`).
const UNIT_DYNFLAG_SPECIALINFO: u32 = 0x10;
/// `CreatureCache` type flag bit 0, tameable (`0x529d23`).
const CREATURE_TYPEFLAGS_TAMEABLE: u32 = 0x1;

/// The studied-creature block's facts (`0x529a40`), only while the unit carries the studied flag.
/// The creature record is the unit's cached template (`[unit+0xb30]`), which a player lacks.
fn studied(store: &ObjectStore, src: &TooltipSources<'_>) -> Option<Box<StudiedCreature>> {
    let fields = &store.0;
    if fields.unit_dynamic_flags() & UNIT_DYNFLAG_SPECIALINFO == 0 {
        return None;
    }
    let resistances: [i32; 7] =
        std::array::from_fn(|i| fields.unit_resistance(i as u8).unwrap_or(0));
    let armor_school = src.study.tables.and_then(|t| t.armor_school);
    let armor = match armor_school {
        Some(school) => resistances[usize::from(school)],
        // Index -1 (`0x529ae6`): the dword before `UNIT_FIELD_RESISTANCES`, `UNIT_FIELD_STAT4`.
        None => fields.unit_stat(4).unwrap_or(0) as i32,
    };
    let record = fields
        .object_entry()
        .filter(|_| fields.object_type() != Some(ObjectType::Player))
        .and_then(|entry| src.names.creature_record(entry));
    // The family's diet (`0x529c01`-`0x529cd1`); no family row, or an empty mask, no foods.
    let diet = src
        .study
        .families
        .zip(record)
        .and_then(|(t, r)| Some((t, t.families.get(r.pet_family)?)))
        .map(|(t, family)| {
            t.foods
                .for_mask(family.pet_food_mask)
                .into_iter()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let tameable = record
        .filter(|r| r.type_flags & CREATURE_TYPEFLAGS_TAMEABLE != 0 && r.pet_family != 0)
        .map(|r| pet_abilities(r.pet_spell_list, src));
    Some(Box::new(StudiedCreature {
        min_damage: fields.unit_min_damage().unwrap_or(0.0),
        max_damage: fields.unit_max_damage().unwrap_or(0.0),
        resistances,
        armor_school,
        armor,
        diet,
        tameable,
    }))
}

/// The pet abilities of a tameable creature's `CreatureSpellData.dbc` row `list`
/// (`0x529d56`-`0x529e47`): each available slot whose spell has a `Spell.dbc` record, by name and
/// rank. No row, no abilities.
fn pet_abilities(list: u32, src: &TooltipSources<'_>) -> Vec<PetAbility> {
    let rows = src.study.tables.and_then(|t| t.pet_spells.as_ref());
    let (Some(rows), Some(catalog)) = (rows, src.catalog) else {
        return Vec::new();
    };
    rows.available_spells(list)
        .into_iter()
        .flatten()
        .filter_map(|id| catalog.get(id))
        .map(|d| PetAbility {
            name: d.name.clone(),
            rank: d.rank.clone().unwrap_or_default(),
        })
        .collect()
}

/// `CreatureType.dbc` id 1, Beast: its owner line is `UNITNAME_TITLE_PET`.
const CREATURE_TYPE_BEAST: u32 = 1;

/// `0x52fd30`: the charmer (`UNIT_FIELD_CHARMEDBY`, `0x52fd4d`), else the creator
/// (`UNIT_FIELD_CREATEDBY`, `0x52fdc9`), held as a unit (`0x52fd8b`, mask 8) and named by
/// `GetUnitName` (`0x52fd98`). A charmer's line is pet for a beast, else minion (`0x52fd6b`); a
/// creator's takes guardian or creation from its spell's first effect (`UNIT_CREATED_BY_SPELL`,
/// `0x52fdd5`; `0x52fe04`-`0x52fe44`), else the same pet or minion choice.
fn owner_title(
    store: &ObjectStore,
    creature_type: u32,
    src: &TooltipSources<'_>,
) -> Option<OwnerTitle> {
    let fields = &store.0;
    let pet_or_minion = if creature_type == CREATURE_TYPE_BEAST {
        OwnerTitleKind::Pet
    } else {
        OwnerTitleKind::Minion
    };
    let (owner, kind) = match fields.unit_charmed_by() {
        Some(charmer) => (charmer, pet_or_minion),
        None => {
            let creator = fields.unit_created_by()?;
            let effect = fields
                .unit_created_by_spell()
                .and_then(|id| src.catalog?.get(id))
                .map(|d| d.effects[0]);
            let kind = match effect {
                Some(SPELL_EFFECT_SUMMON_GUARDIAN) => OwnerTitleKind::Guardian,
                Some(50 | 74 | 87..=90 | 104..=107) => OwnerTitleKind::Creation,
                _ => pet_or_minion,
            };
            (creator, kind)
        }
    };
    let owner_store = (src.object)(owner).filter(|s| s.is_unit())?;
    let name = src
        .names
        .resolve_unit(owner, Some(owner_store), Ask::Read)
        .map(str::to_string);
    Some(OwnerTitle { kind, name })
}

/// `SPELL_EFFECT_SUMMON_GUARDIAN`, the one effect that titles a guardian (`0x52fe24`).
const SPELL_EFFECT_SUMMON_GUARDIAN: u32 = 42;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_CHARMEDBY`, `UNIT_FIELD_CREATEDBY`, `UNIT_CREATED_BY_SPELL` and `OBJECT_FIELD_TYPE`,
    /// absolute descriptor indices.
    const CHARMEDBY: u16 = 10;
    const CREATEDBY: u16 = 14;
    const CREATED_BY_SPELL: u16 = 146;
    const OBJECT_FIELD_TYPE: u16 = 2;

    fn unit(pairs: &[(u16, u32)]) -> ObjectStore {
        let mut all = vec![(OBJECT_FIELD_TYPE, 0x9)];
        all.extend_from_slice(pairs);
        ObjectStore(ObjectFields::from_pairs(&all))
    }

    fn catalog(effects: &[(u32, u32)]) -> SpellCatalog {
        SpellCatalog::from_displays(
            effects
                .iter()
                .map(|&(id, effect)| {
                    let mut d = benilla_formats::SpellDisplay {
                        id,
                        ..Default::default()
                    };
                    d.effects[0] = effect;
                    (id, d)
                })
                .collect::<HashMap<_, _>>(),
        )
    }

    /// The owner line's key: a charmer titles by the unit's own type; a creator by its spell's first
    /// effect, a guardian (42) or an object (50, 74, 87-90, 104-107), else the same type choice;
    /// no owner in sight, no line.
    #[test]
    fn the_owner_line_keys_by_charmer_type_or_creating_effect() {
        const OWNER: u64 = 0x77;
        let owner = unit(&[]);
        let names = NameCache::default();
        let spells = catalog(&[(10, 56), (11, 87), (12, 42), (13, 50), (14, 107), (15, 108)]);
        let held = |g: u64| (g == OWNER).then_some(&owner);
        let src = TooltipSources {
            names: &names,
            catalog: Some(&spells),
            object: &held,
            study: StudySources::default(),
        };
        let kind =
            |pairs: &[(u16, u32)], ty: u32| owner_title(&unit(pairs), ty, &src).map(|o| o.kind);
        let charmed = [(CHARMEDBY, OWNER as u32)];
        assert_eq!(kind(&charmed, 1), Some(OwnerTitleKind::Pet));
        assert_eq!(kind(&charmed, 3), Some(OwnerTitleKind::Minion));
        let created = |spell: u32| [(CREATEDBY, OWNER as u32), (CREATED_BY_SPELL, spell)];
        assert_eq!(
            kind(&created(10), 1),
            Some(OwnerTitleKind::Pet),
            "summon pet, a beast"
        );
        assert_eq!(
            kind(&created(10), 3),
            Some(OwnerTitleKind::Minion),
            "a demon"
        );
        assert_eq!(
            kind(&created(11), 11),
            Some(OwnerTitleKind::Creation),
            "a totem slot"
        );
        assert_eq!(kind(&created(12), 7), Some(OwnerTitleKind::Guardian));
        assert_eq!(kind(&created(13), 7), Some(OwnerTitleKind::Creation));
        assert_eq!(kind(&created(14), 7), Some(OwnerTitleKind::Creation));
        assert_eq!(
            kind(&created(15), 7),
            Some(OwnerTitleKind::Minion),
            "108 is past the range"
        );
        assert_eq!(
            kind(&created(9999), 1),
            Some(OwnerTitleKind::Pet),
            "no record"
        );
        // The charmer wins over the creator.
        let both = [
            (CHARMEDBY, OWNER as u32),
            (CREATEDBY, OWNER as u32),
            (CREATED_BY_SPELL, 12),
        ];
        assert_eq!(kind(&both, 1), Some(OwnerTitleKind::Pet));
        // No owner, or one out of sight.
        assert_eq!(kind(&[], 1), None);
        assert_eq!(kind(&[(CHARMEDBY, 0x99)], 1), None);
    }

    /// `OBJECT_FIELD_ENTRY`, `UNIT_FIELD_MINDAMAGE`, `UNIT_FIELD_MAXDAMAGE`, `UNIT_DYNAMIC_FLAGS`,
    /// `UNIT_FIELD_STAT4` and `UNIT_FIELD_RESISTANCES`, absolute descriptor indices.
    const ENTRY: u16 = 3;
    const MINDAMAGE: u16 = 134;
    const MAXDAMAGE: u16 = 135;
    const DYNAMIC_FLAGS: u16 = 143;
    const STAT4: u16 = 154;
    const RESISTANCES: u16 = 155;
    /// vmangos `creature_template` 1196, the Ice Claw Bear: a Bear (family 4) with list 5796.
    const BEAR: u32 = 1196;

    fn bear_names(type_flags: u32) -> NameCache {
        let mut names = NameCache::default();
        names.insert_creature(
            BEAR,
            Some(crate::names::CreatureRecord {
                name: "Ice Claw Bear".into(),
                subname: None,
                creature_type: 1,
                pet_family: 4,
                rank: 0,
                type_flags,
                pet_spell_list: 5796,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        names
    }

    fn bear(pairs: &[(u16, u32)]) -> ObjectStore {
        let mut all = vec![
            (ENTRY, BEAR),
            (MINDAMAGE, 10.5f32.to_bits()),
            (MAXDAMAGE, 14.2f32.to_bits()),
            (STAT4, 33),
            (RESISTANCES, 520),
            (RESISTANCES + 2, 15),
        ];
        all.extend_from_slice(pairs);
        unit(&all)
    }

    /// The block's facts only with the studied flag (`0x52a8ea`); with no armor school the armor
    /// is the dword before the resistances (`0x529ae6`, index -1); tameable is the record's type
    /// flag bit 0 and a family (`0x529d23`, `0x529d2d`), and a player has no record.
    #[test]
    fn the_studied_facts_follow_the_flag_and_the_record() {
        let none = |_: u64| None;
        let names = bear_names(1);
        let src = TooltipSources {
            names: &names,
            catalog: None,
            object: &none,
            study: StudySources::default(),
        };
        assert_eq!(studied(&bear(&[]), &src), None);
        let got = studied(&bear(&[(DYNAMIC_FLAGS, 0x10)]), &src).expect("studied");
        assert_eq!((got.min_damage, got.max_damage), (10.5, 14.2));
        assert_eq!(got.resistances, [520, 0, 15, 0, 0, 0, 0]);
        assert_eq!((got.armor_school, got.armor), (None, 33));
        assert_eq!(got.tameable, Some(Vec::new()), "no tables, no abilities");
        let untameable = bear_names(0);
        let src = TooltipSources {
            names: &untameable,
            ..src
        };
        assert_eq!(
            studied(&bear(&[(DYNAMIC_FLAGS, 0x10)]), &src).map(|s| s.tameable),
            Some(None)
        );
        let src = TooltipSources {
            names: &names,
            ..src
        };
        let player = bear(&[(DYNAMIC_FLAGS, 0x10), (OBJECT_FIELD_TYPE, 0x19)]);
        assert_eq!(studied(&player, &src).map(|s| s.tameable), Some(None));
    }

    /// The Ice Claw Bear read against the shipped tables: armor in school 0, the Bear diet in
    /// `ItemPetFood.dbc` order, and its list's one ability, Claw.
    #[test]
    fn a_studied_bear_reads_its_diet_and_claw() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let families = crate::ui_pet_stats::PetFamilyTables {
            families: benilla_formats::load_creature_families(&mut chain).expect("families"),
            foods: benilla_formats::load_pet_food_names(&mut chain).expect("foods"),
        };
        let study = StudyTables {
            armor_school: benilla_formats::load_armor_school(&mut chain).expect("resistances"),
            pet_spells: Some(
                benilla_formats::load_creature_spell_data(&mut chain).expect("pet spells"),
            ),
        };
        let spells = SpellCatalog::from_displays(HashMap::from([(
            16827,
            benilla_formats::SpellDisplay {
                id: 16827,
                name: "Claw".into(),
                rank: Some("Rank 1".into()),
                ..Default::default()
            },
        )]));
        let names = bear_names(1);
        let none = |_: u64| None;
        let src = TooltipSources {
            names: &names,
            catalog: Some(&spells),
            object: &none,
            study: StudySources {
                tables: Some(&study),
                families: Some(&families),
            },
        };
        let got = studied(&bear(&[(DYNAMIC_FLAGS, 0x10)]), &src).expect("studied");
        assert_eq!((got.armor_school, got.armor), (Some(0), 520));
        assert_eq!(
            got.diet,
            ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"]
        );
        assert_eq!(
            got.tameable,
            Some(vec![PetAbility {
                name: "Claw".into(),
                rank: "Rank 1".into(),
            }])
        );
    }
}
