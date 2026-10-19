//! What the unit tooltip builder `0x529fe0` reads of a unit beyond what the `Unit*` getters do,
//! resolved here where the caches and the object manager live: the owner line (`0x52fd30`).

use benilla_formats::SpellCatalog;
use benilla_ui::script::{OwnerTitle, OwnerTitleKind, UnitTooltipFacts};

use crate::names::NameCache;
use crate::net::ObjectStore;
use crate::query_cache::Ask;

/// What [`tooltip_facts`] reads beside the unit's own descriptor.
#[derive(Clone, Copy)]
pub(crate) struct TooltipSources<'a> {
    pub(crate) names: &'a NameCache,
    /// `Spell.dbc`, which names the spell a unit was created by; absent without game data.
    pub(crate) catalog: Option<&'a SpellCatalog>,
    /// The object manager's lookup by guid (`0x468460`): the owner line names an owner in sight.
    pub(crate) object: &'a dyn Fn(u64) -> Option<&'a ObjectStore>,
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
    }
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
        };
        let kind = |pairs: &[(u16, u32)], ty: u32| {
            owner_title(&unit(pairs), ty, &src).map(|o| o.kind)
        };
        let charmed = [(CHARMEDBY, OWNER as u32)];
        assert_eq!(kind(&charmed, 1), Some(OwnerTitleKind::Pet));
        assert_eq!(kind(&charmed, 3), Some(OwnerTitleKind::Minion));
        let created = |spell: u32| [(CREATEDBY, OWNER as u32), (CREATED_BY_SPELL, spell)];
        assert_eq!(kind(&created(10), 1), Some(OwnerTitleKind::Pet), "summon pet, a beast");
        assert_eq!(kind(&created(10), 3), Some(OwnerTitleKind::Minion), "a demon");
        assert_eq!(kind(&created(11), 11), Some(OwnerTitleKind::Creation), "a totem slot");
        assert_eq!(kind(&created(12), 7), Some(OwnerTitleKind::Guardian));
        assert_eq!(kind(&created(13), 7), Some(OwnerTitleKind::Creation));
        assert_eq!(kind(&created(14), 7), Some(OwnerTitleKind::Creation));
        assert_eq!(kind(&created(15), 7), Some(OwnerTitleKind::Minion), "108 is past the range");
        assert_eq!(kind(&created(9999), 1), Some(OwnerTitleKind::Pet), "no record");
        // The charmer wins over the creator.
        let both = [(CHARMEDBY, OWNER as u32), (CREATEDBY, OWNER as u32), (CREATED_BY_SPELL, 12)];
        assert_eq!(kind(&both, 1), Some(OwnerTitleKind::Pet));
        // No owner, or one out of sight.
        assert_eq!(kind(&[], 1), None);
        assert_eq!(kind(&[(CHARMEDBY, 0x99)], 1), None);
    }
}
