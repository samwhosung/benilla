//! A unit's creature type as the reference resolves it (`0x605570`), which the spell bind's
//! creature-type gate and the tracking dots read: the shapeshift form's type when above 0
//! (`SpellShapeshiftForm.dbc` column 12, `0x60559a`), else the cached creature template's
//! (`[unit+0xb30] + 0x18`, `0x6055a8`), else the race's (`ChrRaces.dbc` column 9, `0x6055ca`).
//! 0 is a unit with no type, which a creature-type-limited spell refuses.

use std::collections::HashMap;

use benilla_formats::{CreatureTypeTable, ShapeshiftForm};

use crate::names::NameCache;
use crate::net::ObjectStore;
use crate::target::CreatureTypes;
use crate::ui_action::Spells;

/// What the resolver reads beside the unit's own fields: the creature template cache and the
/// form table, and `CreatureType.dbc` for the id's word. A source that is absent resolves nothing
/// at its stage.
#[derive(Clone, Copy, Default)]
pub(crate) struct CreatureTypeSources<'a> {
    pub(crate) names: Option<&'a NameCache>,
    pub(crate) forms: Option<&'a HashMap<u32, ShapeshiftForm>>,
    pub(crate) words: Option<&'a CreatureTypeTable>,
}

impl<'a> CreatureTypeSources<'a> {
    /// The sources a system holds as resources: the name cache, once `Spell.dbc` has loaded the
    /// form table it carries, and once `CreatureType.dbc` has loaded its words.
    pub(crate) fn of_resources(
        names: &'a NameCache,
        spells: Option<&'a Spells>,
        words: Option<&'a CreatureTypes>,
    ) -> Self {
        Self {
            names: Some(names),
            forms: spells.map(|s| &s.forms),
            words: words.map(|t| &t.0),
        }
    }

    /// The word of a creature type id: `CreatureType.dbc`'s string at the install's locale
    /// (`0x51a2dc`, `0x52a4bd`). Type 0 has no row, and neither has an id past the table, so both
    /// answer nothing (`0x51a2c1`, `0x51a2cd`).
    pub(crate) fn word(&self, creature_type: u32) -> Option<&'a str> {
        self.words?.name(creature_type)
    }

    /// The unit's creature type, 0 for none. The template stage answers whatever the record
    /// holds, 0 included, and is keyed by `OBJECT_FIELD_ENTRY` as the reference's cache is.
    pub(crate) fn of(&self, store: &ObjectStore) -> u32 {
        let fields = &store.0;
        let form = self
            .forms
            .and_then(|forms| forms.get(&u32::from(fields.unit_shapeshift_form())))
            .map(|row| row.creature_type)
            .filter(|&ty| ty > 0);
        if let Some(ty) = form {
            return ty as u32;
        }
        let record = self
            .names
            .zip(fields.object_entry())
            .and_then(|(names, entry)| names.creature_type(entry));
        if let Some(ty) = record {
            return ty;
        }
        // The race byte of `UNIT_FIELD_BYTES_0`: a player's, or 0 on a creature whose template
        // has not come back, which has no row.
        race_creature_type((fields.unit_bytes_0().unwrap_or(0) & 0xff) as u8)
    }
}

/// `ChrRaces.dbc` column 9 by race id: Humanoid on all nine shipped rows, and 0 where the table
/// has no row (`0x6055b3`-`0x6055d1`).
fn race_creature_type(race: u8) -> u32 {
    if (1..=9).contains(&race) {
        CREATURE_TYPE_HUMANOID
    } else {
        0
    }
}

/// `CreatureType.dbc` id 7.
const CREATURE_TYPE_HUMANOID: u32 = 7;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::CreatureRecord;
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_BYTES_1` and `UNIT_FIELD_BYTES_0`, absolute descriptor indices.
    const BYTES_1: u16 = 138;
    const BYTES_0: u16 = 36;
    const OBJECT_FIELD_ENTRY: u16 = 3;

    fn store(entry: u32, race: u32, form: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (OBJECT_FIELD_ENTRY, entry),
            (BYTES_0, race),
            (BYTES_1, form << 16),
        ]))
    }

    fn names_with(entry: u32, creature_type: u32) -> NameCache {
        let mut names = NameCache::default();
        names.insert_creature(
            entry,
            Some(CreatureRecord {
                name: "Test".into(),
                subname: None,
                creature_type,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        names
    }

    fn forms() -> HashMap<u32, ShapeshiftForm> {
        let row = |creature_type| ShapeshiftForm {
            creature_type,
            ..Default::default()
        };
        // Cat form is a Beast; a warrior stance carries type 0, and Tree of Life -1.
        HashMap::from([(1, row(1)), (2, row(-1)), (17, row(0))])
    }

    /// The three stages in the reference's order: form above 0, then the template, then the
    /// race, and 0 for a unit none of them names.
    #[test]
    fn stages_resolve_in_the_references_order() {
        let names = names_with(69, 1);
        let forms = forms();
        let sources = CreatureTypeSources {
            names: Some(&names),
            forms: Some(&forms),
            ..Default::default()
        };
        // A cat-form player is a Beast; unshifted, a Humanoid by race.
        assert_eq!(sources.of(&store(0, 1, 1)), 1);
        assert_eq!(sources.of(&store(0, 1, 0)), 7);
        // A row of type 0 or -1 falls through to the race, not to a fixed answer.
        assert_eq!(sources.of(&store(0, 5, 17)), 7);
        assert_eq!(sources.of(&store(0, 5, 2)), 7);
        // A cached creature is its template's type; the form still wins over it.
        assert_eq!(sources.of(&store(69, 0, 0)), 1);
        let humanoid = names_with(69, 7);
        let sources = CreatureTypeSources {
            names: Some(&humanoid),
            forms: Some(&forms),
            ..Default::default()
        };
        assert_eq!(sources.of(&store(69, 0, 0)), 7);
        assert_eq!(
            sources.of(&store(69, 0, 1)),
            1,
            "a form outranks the template"
        );
        // The template answers whatever it holds: type 0 does not fall through to the race.
        let untyped = names_with(69, 0);
        let sources = CreatureTypeSources {
            names: Some(&untyped),
            forms: None,
            ..Default::default()
        };
        assert_eq!(sources.of(&store(69, 1, 0)), 0);
    }

    /// A creature whose template has not answered has race 0, which has no row: type 0. A form
    /// id with no row, or no form table at all, resolves nothing at that stage.
    #[test]
    fn an_uncached_creature_and_a_missing_row_read_type_zero() {
        let names = NameCache::default();
        let forms = forms();
        let sources = CreatureTypeSources {
            names: Some(&names),
            forms: Some(&forms),
            ..Default::default()
        };
        assert_eq!(sources.of(&store(69, 0, 0)), 0, "no template, race 0");
        assert_eq!(sources.of(&store(69, 0, 99)), 0, "no form row either");
        assert_eq!(sources.of(&store(0, 10, 0)), 0, "race 10 has no row");
        assert_eq!(CreatureTypeSources::default().of(&store(0, 1, 1)), 7);
        assert_eq!(CreatureTypeSources::default().of(&store(69, 0, 1)), 0);
    }

    /// The shipped `SpellShapeshiftForm.dbc` through the resolver: the forms of type 1 name a
    /// Beast, and every other row falls through to the race.
    #[test]
    fn the_shipped_forms_name_a_beast_or_fall_through_to_the_race() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let forms =
            benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc");
        let sources = CreatureTypeSources {
            names: None,
            forms: Some(&forms),
            ..Default::default()
        };
        // Cat, Travel, Aquatic, Bear, Dire Bear, Creature - Bear, Creature - Cat, Ghost Wolf.
        for id in [1, 3, 4, 5, 8, 14, 15, 16] {
            assert_eq!(forms[&id].creature_type, 1, "form {id} is type 1");
            assert_eq!(sources.of(&store(0, 1, id)), 1, "form {id}");
        }
        // Tree of Life and Battle Stance carry -1: the human race's type answers.
        for id in [2, 17] {
            assert!(forms[&id].creature_type <= 0, "form {id} carries no type");
            assert_eq!(sources.of(&store(0, 1, id)), 7, "form {id}");
        }
        // Every row by the rule: above 0 names the type, else the race.
        for (&id, row) in &forms {
            let expected = u32::try_from(row.creature_type).ok().filter(|&t| t > 0);
            assert_eq!(
                sources.of(&store(0, 1, id)),
                expected.unwrap_or(7),
                "form {id}"
            );
        }
    }

    /// The words `CreatureType.dbc` holds at some locale's column: whatever the table says, not
    /// the English ones.
    fn words() -> CreatureTypeTable {
        CreatureTypes::korean_for_tests().0
    }

    /// The unit snapshot carries the resolver's word for every unit, players included: a
    /// shapeshifted player a Beast, an unshifted one a Humanoid by race, a form of no type the
    /// same, and a creature its template's. The word is the table's own string, so a table in
    /// another language reaches `UnitCreatureType` and the tooltip as that language.
    #[test]
    fn the_snapshot_carries_the_resolved_type_word() {
        let names = names_with(69, 1);
        let forms = forms();
        let words = words();
        let sources = CreatureTypeSources {
            names: Some(&names),
            forms: Some(&forms),
            words: Some(&words),
        };
        let word = |store: &ObjectStore| {
            crate::ui_unit::snapshot(store, 0, None, 0, Default::default(), sources)
                .creature_type_name
        };
        assert_eq!(word(&store(0, 1, 1)).as_deref(), Some("야수"), "cat form");
        assert_eq!(word(&store(0, 1, 0)).as_deref(), Some("인간형"), "no form");
        assert_eq!(word(&store(0, 1, 2)).as_deref(), Some("인간형"), "type -1");
        assert_eq!(word(&store(69, 0, 0)).as_deref(), Some("야수"), "template");
        assert_eq!(word(&store(70, 0, 0)), None, "uncached: race 0, no type");
        // A type the table has no row for (4, Elemental) has no word.
        let elemental = names_with(71, 4);
        let sources = CreatureTypeSources {
            names: Some(&elemental),
            words: Some(&words),
            ..Default::default()
        };
        assert_eq!(
            crate::ui_unit::snapshot(&store(71, 0, 0), 0, None, 0, Default::default(), sources)
                .creature_type_name,
            None
        );
        // Without the table, as before `CreatureType.dbc` loads, no unit has a word.
        let sources = CreatureTypeSources {
            names: Some(&names),
            forms: Some(&forms),
            ..Default::default()
        };
        assert_eq!(
            crate::ui_unit::snapshot(&store(0, 1, 1), 0, None, 0, Default::default(), sources)
                .creature_type_name,
            None
        );
    }

    /// Type 10, "Not specified", has a row and a word, and the snapshot carries both: the id for
    /// the tooltip's own gate and the word `UnitCreatureType` answers (`0x51a2dc`).
    #[test]
    fn type_ten_carries_its_word_and_its_id() {
        let names = names_with(72, 10);
        let words = words();
        let sources = CreatureTypeSources {
            names: Some(&names),
            words: Some(&words),
            ..Default::default()
        };
        assert_eq!(sources.word(10), Some("지정되지 않음"));
        let snap =
            crate::ui_unit::snapshot(&store(72, 0, 0), 0, None, 0, Default::default(), sources);
        assert_eq!(snap.creature_type, 10);
        assert_eq!(snap.creature_type_name.as_deref(), Some("지정되지 않음"));
    }

    /// The shipped `CreatureType.dbc` through the resolver at the install's locale: the eleven
    /// words the stock tooltip and `UnitCreatureType` print, and enGB's are enUS's.
    #[test]
    fn the_shipped_words_are_the_installs_own() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let table = benilla_formats::load_creature_type_table(&mut chain).expect("CreatureType");
        let sources = CreatureTypeSources {
            words: Some(&table),
            ..Default::default()
        };
        if chain.locale().index() == 0 {
            for (id, word) in [
                (1, "Beast"),
                (2, "Dragonkin"),
                (3, "Demon"),
                (4, "Elemental"),
                (5, "Giant"),
                (6, "Undead"),
                (7, "Humanoid"),
                (8, "Critter"),
                (9, "Mechanical"),
                (10, "Not specified"),
                (11, "Totem"),
            ] {
                assert_eq!(sources.word(id), Some(word), "type {id}");
            }
        }
        assert_eq!(sources.word(0), None);
        assert_eq!(sources.word(12), None);
    }

    /// The frozen race table against the shipped `ChrRaces.dbc`.
    #[test]
    fn race_creature_type_matches_the_shipped_table() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let rows = benilla_formats::load_race_creature_types(&mut chain).expect("ChrRaces walk");
        assert_eq!(rows.len(), 9, "ChrRaces.dbc row count");
        for race in 0..=u8::MAX {
            assert_eq!(
                race_creature_type(race),
                rows.get(&race).copied().unwrap_or(0),
                "race {race}"
            );
        }
    }
}
