//! The app-side **pet paper-doll feed** (decision 1057) — `PetPaperDollFrame`'s data, the
//! [`crate::ui_char`] pattern pointed at the pet.
//!
//! Two jobs, each frame, in the feed phase before the VM ticks ([`crate::ui_script::UiFeed`]):
//!
//! - **The pet's [`UnitCombatStats`]**, built through [`crate::ui_char::unit_combat_stats`] — the
//!   descriptor-only core, which is all a creature has. It is the *same* snapshot type and the
//!   *same* bindings the character sheet reads, because that is the reference's own arrangement:
//!   `PetPaperDollFrame_Update` calls `PaperDollFrame_SetDamage/_SetAttackPower/_SetArmor/…` with
//!   `unit = "pet"` (ref `PetPaperDollFrame.lua:73-81`) and its own two setters read
//!   `UnitStat("pet", i)` / `UnitResistance("pet", id)`. Nothing here is a parallel API.
//! - **Pointing the body booth**: [`PetDollBooth`]'s `unit` gets the resolved pet entity, and its
//!   `yaw` the facing the stock pane's rotate buttons wrote onto `PetModelFrame` itself
//!   (`UiScript::model_pane_facing` — decision 1751).
//!
//! **Why a module of its own rather than more of [`crate::ui_pet_stats`]**, which already resolves
//! the same pet: these are the *shared* paper-doll surface (every value passes through a binding
//! the character sheet uses too, and every event is one `PaperDollFrame` fires for the player),
//! while that file is the hunter-only block behind the `0x6116e0` class gate. One diff over both
//! would fire the shared repaint events off happiness drift, and the hunter gate would sit in the
//! path of numbers a warlock's imp has too.
//!
//! Events: [`crate::ui_char::fire_stat_transitions`] with `arg1 = "pet"` — the eight groups the
//! ref's page registers (`PetPaperDollFrame.lua:12-20`) that also have a source. `UNIT_LEVEL` is
//! already fired for `"pet"` by [`crate::ui_pet`]'s token feed, and `UNIT_PET` /
//! `UNIT_PET_EXPERIENCE` / `UNIT_PET_TRAINING_POINTS` by that file and
//! [`crate::ui_pet_stats`] — none is re-fired here. `PET_UI_UPDATE`/`PET_UI_CLOSE` have no source
//! anywhere yet (a named deferral in 1057), and `UNIT_DEFENSE` has none either while
//! `UnitDefense("pet")` is the INTERIM `(0, 0)`.

use bevy::prelude::*;

use benilla_ui::script::{UiScript, UnitCombatStats};

use crate::portrait::PetDollBooth;
use crate::ui_char::{fire_stat_transitions, unit_combat_stats};
use crate::ui_pet::{PetBar, PetUnit};
use crate::ui_unit::UnitFeed;

pub(crate) struct UiPetDollPlugin;

impl Plugin for UiPetDollPlugin {
    fn build(&self, app: &mut App) {
        // Rides the unit feed beside the pet bar's and the stat block's, and before the VM ticks —
        // the whole page repaints out of the one pass that pushes the pet's health.
        app.add_systems(Update, feed_pet_doll.in_set(UnitFeed));
    }
}

fn feed_pet_doll(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    pet: PetUnit,
    mut booth: ResMut<PetDollBooth>,
    mut last: Local<crate::ui_script::VmMemo<Option<UnitCombatStats>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    // The pane's rotate buttons own the yaw; the booth mirrors it (the inspect pane's arrangement,
    // decision 0631 §4). Written every frame, pet or no pet — a stale yaw would snap the model the
    // moment one is summoned.
    booth.yaw = script.model_pane_facing("PetModelFrame");

    let pet_guid = bar.spells.pet_guid;
    let store = (pet_guid != 0).then(|| pet.store(pet_guid)).flatten();
    // `None` empties the booth — the same "no pet" the rest of the page shows, and the same test:
    // a bar naming a guid whose object never streamed is not a pet we can draw.
    booth.unit = (pet_guid != 0).then(|| pet.entity(pet_guid)).flatten();

    let fresh = store.map(unit_combat_stats);
    // Push only on change (the pet-bar feed's discipline): the page repaints off the UNIT_* events
    // below, so a per-frame push would be pure churn — but the DIFF is what makes it cheap, so a
    // real change still lands the frame it arrives.
    if *last == fresh {
        return;
    }
    // PUSH before firing: event dispatch runs the Lua handlers synchronously, so the snapshot must
    // already be in the VM when they repaint (the `ui_unit` rule — a fire-first ordering paints the
    // OLD values and, being transition-gated, never corrects itself).
    let prev = last.take();
    script.set_pet_combat_stats(fresh.clone());
    if let Some(stats) = &fresh {
        if prev.is_none() {
            debug!(
                "ui_pet_doll: pet stats resolved — {} armor, {}-{} damage",
                stats.resistances[0], stats.min_damage, stats.max_damage
            );
        }
        fire_stat_transitions(&mut script, "pet", prev.as_ref(), stats);
    }
    // A pet going away fires nothing: the page hides on `UNIT_PET` (which `crate::ui_pet` fires on
    // the guid edge), exactly as the character sheet's stat lines are not cleared by an event.
    *last = fresh;
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ObjectFields, ObjectType};

    use crate::net::ObjectStore;

    // The UNIT-block wire indices the core reads (`benilla_protocol`'s private
    // `fields::FIELD_UNIT_*` table; the `ui_pet_stats` fixtures' own convention).
    const BASEATTACKTIME: u16 = 126;
    const MINDAMAGE: u16 = 134;
    const MAXDAMAGE: u16 = 135;
    const STAT0: u16 = 150;
    const RESISTANCES0: u16 = 155;
    const ATTACK_POWER: u16 = 165;
    const ATTACK_POWER_MODS: u16 = 166;

    /// A boar's descriptor: the UNIT half a creature really streams, and nothing else — no PLAYER
    /// block at all, which is the whole point of the fixture.
    ///
    /// **CREATED, and that is the load-bearing half** (decision 1081). A live pet arrives as a
    /// create block, and a create is a *complete* snapshot — absent means 0, not unknown. Built
    /// bare (the fixture default), this boar answered `None` for every PLAYER field and the
    /// defaults below looked right while the live client's read `0`; the pet sheet's damage
    /// tooltip came out `inf - inf` / `nan` under a green test. Take `.into_created` off and the
    /// `damage_percent` assertion is the one that fails.
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

    /// The descriptor-only core over a real creature's field set: the UNIT values come through,
    /// and **every PLAYER-block-sourced value keeps its default** — which is what makes the ref's
    /// pet sheet plain white numbers rather than a buff decomposition.
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
        // No PLAYER block ⇒ no buff splits, no damage-done mods…
        assert_eq!(s.stat_pos, [0; 5]);
        assert_eq!(s.stat_neg, [0; 5]);
        assert_eq!(s.resistance_pos, [0; 7]);
        assert_eq!(s.resistance_neg, [0; 7]);
        assert_eq!((s.physical_bonus_pos, s.physical_bonus_neg), (0, 0));
        // …and `damage_percent` MUST stay 1.0: the ref Lua divides the damage range by it.
        assert_eq!(s.damage_percent, 1.0);
        // The equipment/skill half is the player feed's; a pet has neither.
        assert!(!s.has_offhand && !s.has_wand);
        assert_eq!(s.main_weapon_skill, (0, 0));
        assert_eq!(s.ranged_weapon_skill, (0, 0));
        assert_eq!(
            s.defense_skill,
            (0, 0),
            "INTERIM — unconfirmed in the reference"
        );
    }

    /// An empty store (a guid whose fields have not streamed) is the absent shape, not a panic and
    /// not a division-by-zero waiting to happen in the Lua.
    #[test]
    fn an_unstreamed_pet_reads_the_absent_shape() {
        let s = unit_combat_stats(&ObjectStore(ObjectFields::from_pairs(&[])));
        assert_eq!(s.stats, [0; 5]);
        assert_eq!(s.resistances, [0; 7]);
        assert_eq!(s.damage_percent, 1.0);
        // The base-swing fallback, so `damage / speed` is finite on the first frames.
        assert_eq!(s.main_attack_time_ms, 2000);
    }

    /// **The seam the gates cannot see**: the exact composition [`feed_pet_doll`] performs —
    /// descriptor → core → `set_pet_combat_stats` → the ref's own bindings — really answers the
    /// boar's numbers under `"pet"`. A feed wired to the wrong setter, or a core that dropped a
    /// field on the way, compiles and shows an all-zero pet sheet; only reading it back through
    /// the VM catches that.
    #[test]
    fn the_feeds_composition_answers_the_pet_bindings() {
        let mut s = benilla_ui::script::UiScript::new().unwrap();
        s.set_pet_combat_stats(Some(unit_combat_stats(&boar())));

        // `PetPaperDollFrame_SetStats`'s read (ref l.149) — stamina is the 3rd, 1-based.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 3)"#)
                .unwrap(),
            (68, 68, 0, 0)
        );
        // `PetPaperDollFrame_SetResistances`'s (ref l.112) — fire is school 2.
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

        // Dismissing the pet is the feed's `None` push — every line falls back to the absent
        // shape rather than freezing on the last pet's numbers.
        s.set_pet_combat_stats(None);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("pet")"#)
                .unwrap(),
            (0, 0, 0, 0, 0)
        );
    }
}
