//! A unit's creature type as the reference resolves it (`0x605570`), which the spell bind's
//! creature-type gate and the tracking dots read: the shapeshift form's type when above 0
//! (`SpellShapeshiftForm.dbc` column 12, `0x60559a`), else the cached creature template's
//! (`[unit+0xb30] + 0x18`, `0x6055a8`), else the race's (`ChrRaces.dbc` column 9, `0x6055ca`).
//! 0 is a unit with no type, which a creature-type-limited spell refuses.

use std::collections::HashMap;

use benilla_formats::ShapeshiftForm;

use crate::names::NameCache;
use crate::net::ObjectStore;

/// What the resolver reads beside the unit's own fields: the creature template cache and the
/// form table. A source that is absent resolves nothing at its stage.
#[derive(Clone, Copy, Default)]
pub(crate) struct CreatureTypeSources<'a> {
    pub(crate) names: Option<&'a NameCache>,
    pub(crate) forms: Option<&'a HashMap<u32, ShapeshiftForm>>,
}

impl CreatureTypeSources<'_> {
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
        // Cat form is a Beast; a warrior stance carries type 0.
        HashMap::from([(1, row(1)), (17, row(0))])
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
        };
        // A cat-form player is a Beast; unshifted, a Humanoid by race.
        assert_eq!(sources.of(&store(0, 1, 1)), 1);
        assert_eq!(sources.of(&store(0, 1, 0)), 7);
        // A stance row of type 0 falls through to the race, not to a fixed answer.
        assert_eq!(sources.of(&store(0, 5, 17)), 7);
        // A cached creature is its template's type; the form still wins over it.
        assert_eq!(sources.of(&store(69, 0, 0)), 1);
        let humanoid = names_with(69, 7);
        let sources = CreatureTypeSources {
            names: Some(&humanoid),
            forms: Some(&forms),
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
        };
        assert_eq!(sources.of(&store(69, 0, 0)), 0, "no template, race 0");
        assert_eq!(sources.of(&store(69, 0, 99)), 0, "no form row either");
        assert_eq!(sources.of(&store(0, 10, 0)), 0, "race 10 has no row");
        assert_eq!(CreatureTypeSources::default().of(&store(0, 1, 1)), 7);
        assert_eq!(CreatureTypeSources::default().of(&store(69, 0, 1)), 0);
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
