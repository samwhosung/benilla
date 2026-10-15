//! The pet paper-doll feed: `PetPaperDollFrame`'s combat stats. Its model pane is an ordinary
//! character pane, `PetModelFrame:SetUnit("pet")` (`PetPaperDollFrame.lua:67`).
//!
//! The pet's [`UnitCombatStats`] come from the character sheet's descriptor-only core and
//! bindings, as in the reference, whose page calls the `PaperDollFrame_Set*` setters with `"pet"`
//! (`PetPaperDollFrame.lua:73-81`). The stat events the page registers (`PetPaperDollFrame.lua:12-20`)
//! are the unit-field bridge's, fired with `arg1 = "pet"` when the pet's own fields change
//! ([`crate::ui_unit::field_events`]); this feed only pushes what their handlers read. Nothing
//! fires `UNIT_DEFENSE` for a pet: its one producer, `0x5ed990`, signals `"player"`.
//! `UNIT_LEVEL` and the `UNIT_PET*` events come from [`crate::ui_pet`] and
//! [`crate::ui_pet_stats`]; nothing fires `PET_UI_UPDATE` or `PET_UI_CLOSE`.

use bevy::prelude::*;

use benilla_ui::script::{UiScript, UnitCombatStats};

use crate::ui_char::unit_combat_stats;
use crate::ui_pet::{PetBar, PetUnit};
use crate::ui_unit::UnitFeed;

pub(crate) struct UiPetDollPlugin;

impl Plugin for UiPetDollPlugin {
    fn build(&self, app: &mut App) {
        // In the unit feed, so the page repaints in the pass that pushes the pet's health, and
        // ahead of the field events whose handlers read it.
        app.add_systems(
            Update,
            feed_pet_doll
                .in_set(UnitFeed)
                .in_set(crate::ui_unit::field_events::FieldEventPushes),
        );
    }
}

pub(crate) fn feed_pet_doll(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    pet: PetUnit,
    mut last: Local<crate::ui_script::VmMemo<Option<UnitCombatStats>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let pet_guid = bar.spells.pet_guid;
    let store = (pet_guid != 0).then(|| pet.store(pet_guid)).flatten();

    let fresh = store.map(unit_combat_stats);
    // Push only on change; the page repaints off the bridge's `UNIT_*` events.
    if *last == fresh {
        return;
    }
    if let (None, Some(stats)) = (&*last, &fresh) {
        debug!(
            "ui_pet_doll: pet stats resolved — {} armor, {}-{} damage",
            stats.resistances[0], stats.min_damage, stats.max_damage
        );
    }
    script.set_pet_combat_stats(fresh.clone());
    // A pet going away fires nothing here: the page hides on `crate::ui_pet`'s `UNIT_PET`.
    *last = fresh;
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ObjectFields, ObjectType};

    use crate::net::ObjectStore;

    // The UNIT-block field indices the core reads (`benilla_protocol`'s `fields::FIELD_UNIT_*`).
    const BASEATTACKTIME: u16 = 126;
    const MINDAMAGE: u16 = 134;
    const MAXDAMAGE: u16 = 135;
    const STAT0: u16 = 150;
    const RESISTANCES0: u16 = 155;
    const ATTACK_POWER: u16 = 165;
    const ATTACK_POWER_MODS: u16 = 166;

    /// A boar's descriptor: the UNIT block alone, built as a create block, because a live pet
    /// arrives as one and a create reads an absent field as 0, not unknown.
    fn boar() -> ObjectStore {
        ObjectStore(
            ObjectFields::from_pairs(&[
                (STAT0, 63),     // strength
                (STAT0 + 1, 45), // agility
                (STAT0 + 2, 68), // stamina
                (STAT0 + 3, 32), // intellect
                (STAT0 + 4, 42), // spirit
                (RESISTANCES0, 1810),
                (RESISTANCES0 + 2, 15), // fire
                (BASEATTACKTIME, 2000),
                (MINDAMAGE, 30.5f32.to_bits()),
                (MAXDAMAGE, 44.5f32.to_bits()),
                (ATTACK_POWER, 178),
                // The MODS field is a packed signed pair: pos in the low word, neg in the high.
                (ATTACK_POWER_MODS, (((-4i16) as u16 as u32) << 16) | 12),
            ])
            .into_created(ObjectType::Unit),
        )
    }

    /// PLAYER-block values keep their defaults: the reference's pet sheet shows no buff split.
    #[test]
    fn the_core_reads_a_creatures_unit_block_and_defaults_the_rest() {
        let s = unit_combat_stats(&boar());
        assert_eq!(s.stats, [63, 45, 68, 32, 42]);
        assert_eq!(s.resistances, [1810, 0, 15, 0, 0, 0, 0]);
        assert_eq!(s.min_damage, 30.5);
        assert_eq!(s.max_damage, 44.5);
        assert_eq!(s.main_attack_time_ms, 2000);
        assert_eq!(
            (s.attack_power, s.attack_power_pos, s.attack_power_neg),
            (178, 12, -4)
        );
        assert_eq!(s.stat_pos, [0; 5]);
        assert_eq!(s.stat_neg, [0; 5]);
        assert_eq!(s.resistance_pos, [0; 7]);
        assert_eq!(s.resistance_neg, [0; 7]);
        assert_eq!((s.physical_bonus_pos, s.physical_bonus_neg), (0, 0));
        // `damage_percent` stays 1.0: the stock Lua divides the damage range by it.
        assert_eq!(s.damage_percent, 1.0);
        assert!(!s.has_offhand && !s.has_wand);
        assert_eq!(s.main_weapon_skill, (0, 0));
        assert_eq!(s.ranged_weapon_skill, (0, 0));
        assert_eq!(
            s.defense_skill,
            (0, 0),
            "a pet's snapshot has no defense pair; `UnitDefense(\"pet\")` answers level × 5 itself"
        );
    }

    #[test]
    fn an_unstreamed_pet_reads_the_absent_shape() {
        let s = unit_combat_stats(&ObjectStore(ObjectFields::from_pairs(&[])));
        assert_eq!(s.stats, [0; 5]);
        assert_eq!(s.resistances, [0; 7]);
        assert_eq!(s.damage_percent, 1.0);
        // The base-swing fallback, so `damage / speed` is finite on the first frames.
        assert_eq!(s.main_attack_time_ms, 2000);
    }

    #[test]
    fn the_feeds_composition_answers_the_pet_bindings() {
        let mut s = benilla_ui::script::UiScript::new().unwrap();
        s.set_pet_combat_stats(Some(unit_combat_stats(&boar())));

        // `PetPaperDollFrame.lua:149`'s read; stamina is the third, 1-based.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 3)"#)
                .unwrap(),
            (68, 68, 0, 0)
        );
        // `PetPaperDollFrame.lua:112`'s read; fire is school 2.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("pet", 2)"#)
                .unwrap(),
            (15, 15, 0, 0)
        );
        // `PaperDollFrame_SetArmor("pet", "Pet")`'s.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("pet")"#)
                .unwrap(),
            (1810, 1810, 1810, 0, 0)
        );
        // `PaperDollFrame_SetDamage`'s, and the `/ percent` divisor it feeds.
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("pet")"#)
                .unwrap(),
            (30.5, 44.5, 0.0, 0.0, 0, 0, 1.0)
        );
        // `PaperDollFrame_SetAttackPower`'s.
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitAttackPower("pet")"#)
                .unwrap(),
            (178, 12, -4)
        );

        // Dismissing the pet pushes `None`: every line falls back to the absent shape.
        s.set_pet_combat_stats(None);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("pet")"#)
                .unwrap(),
            (0, 0, 0, 0, 0)
        );
    }
}
