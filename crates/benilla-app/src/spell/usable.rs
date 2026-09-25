//! The usable walk, `Spell_C::IsSpellUsableNow 0x6e3d60`: the ordered gates behind
//! `IsUsableAction`'s grey tint. Any tripped gate answers `(usable, oom) = (false, false)`; only
//! the power leg, the last, sets `notEnoughMana`.
//!
//! Built: the TRADE_SKILL early-out, dead (leg 1), reagents and totems (3), the equipped item (4),
//! combo points (5), shapeshift form (6), only-stealthed (7), not-in-combat (8), CasterAuraState
//! (9), TargetAuraState and its CanAttack/CanAssist fork (10/10b, the only legs that read the
//! current target), the bit-25 cooldown fold (11) and power (12).
//!
//! Not built, each answering usable: leg 2's caster aura-immunity helpers (`0x6e9f20/40/60`), leg
//! 4's `AttributesEx3` sub-conditions. CanAssist in leg 10b is a reaction-rank test (friendly or
//! better), not the reference's `0x6066f0`.

use benilla_protocol::messages::ItemUseSpell;

use benilla_formats::{
    SpellDisplay, ATTR_CASTABLE_WHILE_DEAD, ATTR_NOT_IN_COMBAT, ATTR_ONLY_STEALTHED,
    SPELL_EFFECT_TRADE_SKILL,
};

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, Reputations};
use crate::spell::Cooldowns;
use crate::spell::{SpellModifiers, OP_COST};
use crate::target::{can_attack, ring_reaction, Factions};

use crate::ui_action::Spells;

/// Leg 8's caster unit flag.
use crate::player::UNIT_FLAG_IN_COMBAT;

/// The implicit targets leg 10b forks on (`0x6e3f8a`/`0x6e3fa2`): 6, single enemy, asks
/// `CanAttack 0x606980`; 21, single friend, asks `CanAssist 0x6066f0`.
const IMPLICIT_TARGET_ENEMY: u32 = 6;
const IMPLICIT_TARGET_FRIEND: u32 = 21;

/// Everything the walk reads besides the spell. `target_store` is the current target's (leg 10
/// reads the current-target global `0xb4e2d8`, not an explicit cast target).
pub(crate) struct UsableCtx<'a> {
    pub(crate) store: &'a ObjectStore,
    pub(crate) target_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
    pub(crate) cooldowns: &'a Cooldowns,
    /// The talent spell-modifier tables leg 12's cost goes through.
    pub(crate) spell_mods: &'a SpellModifiers,
    /// Every carried entry's count for the frame ([`crate::ui_items::carried_counts`]).
    pub(crate) carried: &'a std::collections::HashMap<u32, u32>,
}

/// The search covers equipment indices `0..=22` (`0x5f0c50`'s `cmp ebx,0x17; jl`): the 19 worn
/// slots and the four equipped bags.
const EQUIPMENT_SLOTS: u8 = 23;

/// `AttributesEx3`'s hand restrictions, the source of the search's slot mask (`0x5f0c50`'s
/// callers): `0x400` main hand only (mask `0x8000`), `0x1000000` off hand only (mask `0x10000`).
const ATTR_EX3_MAIN_HAND_ONLY: u32 = 0x0000_0400;
const ATTR_EX3_OFF_HAND_ONLY: u32 = 0x0100_0000;

/// `ITEM_FLAG_DEPRECATED` (vmangos `ItemPrototype.h`): a worn item carrying it never satisfies
/// the search.
const ITEM_FLAG_DEPRECATED: u32 = 0x0000_0010;

/// `TARGET_FLAG_ITEM` (`Targets`, column 13): the `EquippedItem*` columns describe the clicked
/// item, not the caster's gear, so the search passes (`0x6e40e0`).
const TARGET_FLAG_ITEM: u32 = 0x0000_0010;

/// The search's slot mask from `AttributesEx3` (`0x6e4136`..`0x6e4153`): bit 10 index 15 alone,
/// else bit 24 index 16 alone, else every slot.
fn hand_mask(d: &SpellDisplay) -> u32 {
    if d.attributes_ex3 & ATTR_EX3_MAIN_HAND_ONLY != 0 {
        1 << crate::items::EQUIPMENT_SLOT_MAINHAND
    } else if d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0 {
        1 << crate::items::EQUIPMENT_SLOT_OFFHAND
    } else {
        u32::MAX
    }
}

/// [`equipped_item_fits`] for the cast ladder's rung 7: the same search, never querying a
/// missing template; an uncached one counts as a match.
pub(crate) fn equipped_item_fits_cached(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
) -> bool {
    if d.equipped_item_class < 0
        || d.equipped_item_subclass_mask == 0
        || d.targets & TARGET_FLAG_ITEM != 0
    {
        return true;
    }
    let mut mask = hand_mask(d);
    if let Some(hidden) = crate::items::disarmed_equipment_slot_cached(store, objects, items) {
        mask &= !(1u32 << hidden);
    }
    equipped_slots_match(
        store,
        mask,
        d.equipped_item_class as u32,
        d.equipped_item_subclass_mask,
        |guid| {
            let obj = objects.object(guid)?;
            let t = items.template_cached(obj.object_entry()?)?;
            Some(WornItem {
                class: t.class,
                subclass: t.subclass,
                flags: t.flags,
                durability: obj.item_durability(),
                max_durability: obj.item_max_durability(),
            })
        },
    )
}

/// The equipped-item refusal reason, keyed on `AttributesEx3` alone (`0x6e40e0` at
/// `6e416e`..`6e4180`): main-hand-only `0x1a`, off-hand-only `0x1b`, else `0x19`. All three render
/// "Must have a %s equipped" with the item subclass name.
pub(crate) fn equipped_item_reason(d: &SpellDisplay) -> u8 {
    if d.attributes_ex3 & ATTR_EX3_MAIN_HAND_ONLY != 0 {
        0x1a
    } else if d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0 {
        0x1b
    } else {
        0x19
    }
}

/// Leg 4 (`0x6e40e0`), shared with the tooltip's requirement line: does a worn item match
/// `EquippedItemClass` and `EquippedItemSubClassMask`? An item whose template has not streamed
/// counts as a match.
pub(crate) fn equipped_item_fits(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> bool {
    // The reference's short-circuits to "fits" (`0x6e40e0` at `6e4103`..`6e4130`): a caster that
    // is not the active player (always ours here), `EquippedItemClass < 0`, a zero subclass mask
    // (no item required, not a wildcard), and an item-targeting spell (`0x495d60` checks those).
    if d.equipped_item_class < 0
        || d.equipped_item_subclass_mask == 0
        || d.targets & TARGET_FLAG_ITEM != 0
    {
        return true;
    }
    let class = d.equipped_item_class as u32;
    let mut mask = hand_mask(d);
    // The disarm ladder strips the one hidden hand from the mask (`0x5f0c69`/`0x5f0c91`).
    if let Some(hidden) = crate::items::disarmed_equipment_slot(store, objects, items, commands) {
        mask &= !(1u32 << hidden);
    }
    equipped_slots_match(store, mask, class, d.equipped_item_subclass_mask, |guid| {
        let (entry, durability, max_durability) = objects.object(guid).and_then(|o| {
            Some((
                o.object_entry()?,
                o.item_durability(),
                o.item_max_durability(),
            ))
        })?;
        let t = items.template(entry, guid, commands)?;
        Some(WornItem {
            class: t.class,
            subclass: t.subclass,
            flags: t.flags,
            durability,
            max_durability,
        })
    })
}

/// One worn item, as the equipped-item search reads it.
pub(crate) struct WornItem {
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    /// The template flags.
    pub(crate) flags: u32,
    /// The instance durability pair (`[item+0x114]+0xa0`/`+0xa4`), not the template's.
    pub(crate) durability: Option<u32>,
    pub(crate) max_durability: Option<u32>,
}

/// The search body over a caller-supplied resolver; `None` from it (template not landed) counts
/// as a match.
fn equipped_slots_match(
    store: &ObjectStore,
    mask: u32,
    class: u32,
    subclass_mask: u32,
    mut worn: impl FnMut(u64) -> Option<WornItem>,
) -> bool {
    (0..EQUIPMENT_SLOTS)
        .filter(|slot| mask & (1u32 << slot) != 0)
        .any(|slot| {
            let Some(guid) = store.0.player_inv_slot(slot).filter(|&g| g != 0) else {
                return false;
            };
            let Some(it) = worn(guid) else {
                return true; // unresolved template: benefit of the doubt
            };
            // The reference's two rejects before the class match: deprecated, and broken
            // (`MaxDurability > 0 && Durability == 0`).
            if it.flags & ITEM_FLAG_DEPRECATED != 0 {
                return false;
            }
            if it.max_durability.is_some_and(|m| m > 0) && it.durability == Some(0) {
                return false;
            }
            it.class == class && subclass_mask & (1 << it.subclass) != 0
        })
}

/// The item arm of the usable compute (`0x4e5050`), gate by gate:
///
/// 1. a held count of 0 (the cache at `0xbc6390`) answers `(false, false)` (`4e50ab`). `held`
///    is carried copies ([`crate::ui_items::InventoryScope::CARRIED`]) or a worn copy. The
///    reference's count (`0x4e6d20`, mask `0x47`) also takes the keyring and sums spell charges
///    for a charged item; ours does neither, so a key reads count 0 and a spent charged copy
///    stays lit.
/// 2. `IsItemOnCooldown 0x6e2fc0` (`4e50d0`) answers `(false, false)`. It asks `0x6e1690` with
///    the item's first on-use spell and the item entry as the key: an on-hold-record test, so a
///    running cooldown or the GCD never greys an item.
/// 3. the item's first on-use spell runs the whole plain-spell walk, [`spell_usable`] (`0x4e5a50`
///    hands it back with type 0; `4e51b0`). This is where food greys in combat: its rows carry
///    [`ATTR_NOT_IN_COMBAT`], leg 8.
/// 4. an item with no on-use spell, or no template yet, is usable (`4e5127`..`4e5135`).
///
/// An on-use spell with no catalog row reads grey with `notEnoughMana = 0` (`4e51aa`).
pub(crate) fn item_usable(
    entry: u32,
    use_spell: Option<&ItemUseSpell>,
    held: bool,
    ctx: &UsableCtx,
    spells: Option<&Spells>,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> (bool, bool) {
    if !held {
        return (false, false);
    }
    // Gate 4: no on-use spell, or no template yet.
    let Some(use_spell) = use_spell else {
        return (true, false);
    };
    let spell_id = use_spell.spell_id;
    let d = spells.and_then(|s| s.catalog.get(spell_id));
    // Gate 2: the item entry as `0x6e1690`'s `itemId`, and the item's own category where it has
    // one (the `spellcategory[5]` override, `6e171b`).
    let category = match use_spell.category {
        0 => d.map_or(0, |d| d.category),
        c => c,
    };
    if ctx.cooldowns.has_on_hold_record(spell_id, entry, category) {
        return (false, false);
    }
    let (Some(d), Some(spells)) = (d, spells) else {
        return (false, false);
    };
    spell_usable(spell_id, d, spells, ctx, objects, items, commands)
}

/// The walk; returns the `IsUsableAction` pair `(usable, not_enough_mana)`.
pub(crate) fn spell_usable(
    spell_id: u32,
    d: &SpellDisplay,
    spells: &Spells,
    ctx: &UsableCtx,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> (bool, bool) {
    // Early-out (`0x6e3d99`): a tradeskill "spell" is always usable.
    if d.effects[0] == SPELL_EFFECT_TRADE_SKILL {
        return (true, false);
    }
    // Leg 1 (`0x6e3dad`): a dead or ghost caster (`0x605f30`) needs the castable-while-dead
    // attribute (`0x6e3db6`).
    if ctx.store.0.is_dead_or_ghost() && d.attributes & ATTR_CASTABLE_WHILE_DEAD == 0 {
        return (false, false);
    }
    // Leg 3 (`0x6e4000`): every reagent count carried, every totem tool present.
    let carried = |entry: u32| ctx.carried.get(&entry).copied().unwrap_or(0);
    for &(entry, count) in &d.reagents {
        if entry != 0 && carried(entry) < count {
            return (false, false);
        }
    }
    for &totem in &d.totems {
        if totem != 0 && carried(totem) == 0 {
            return (false, false);
        }
    }
    // Leg 4 (`0x6e40e0`): some worn item matches the class and subclass mask.
    if !equipped_item_fits(d, ctx.store, objects, items, commands) {
        return (false, false);
    }
    // Leg 5 (`0x6e3e7a`..`0x6e3eb2`): a finishing move (`AttributesEx & 0x500000`, one any-of
    // test) fails while the caster's combo-point byte (`PLAYER_FIELD_BYTES` byte 1) is 0. It reads
    // no target, so a point banked on another mob leaves the button lit and the server refuses the
    // cast. No class gate, unlike Lua `GetComboPoints 0x51a190`: a warrior's Overpower greys here.
    if d.needs_combo_points() && ctx.store.0.player_combo_points().unwrap_or(0) == 0 {
        return (false, false);
    }
    // Leg 6 (`0x612480`): the form's `SpellShapeshiftForm.dbc` stance flag decides whether it
    // counts as shapeshifted.
    let form = ctx.store.0.unit_shapeshift_form();
    let form_is_stance = spells
        .forms
        .get(&u32::from(form))
        .is_some_and(|f| f.is_stance());
    if !d.usable_in_form(form, form_is_stance) {
        return (false, false);
    }
    // Leg 7: only-stealthed spells need the CREEP vis flag.
    if d.attributes & ATTR_ONLY_STEALTHED != 0 && !ctx.store.0.unit_is_stealthed() {
        return (false, false);
    }
    // Leg 8: not-in-combat spells grey under `UNIT_FLAG_IN_COMBAT`.
    if d.attributes & ATTR_NOT_IN_COMBAT != 0 && ctx.store.0.unit_flags() & UNIT_FLAG_IN_COMBAT != 0
    {
        return (false, false);
    }
    // Leg 9: the caster's own aura state.
    if d.caster_aura_state != 0
        && ctx.store.0.unit_aura_state() & (1 << (d.caster_aura_state - 1)) == 0
    {
        return (false, false);
    }
    // Legs 10/10b (`0x6e3f58`): no current target is unusable; then the aura-state bit; then the
    // relation fork.
    if d.target_aura_state != 0 {
        let Some(target) = ctx.target_store else {
            return (false, false);
        };
        if target.0.unit_aura_state() & (1 << (d.target_aura_state - 1)) == 0 {
            return (false, false);
        }
        match d.implicit_target_a1 {
            // `CanAttack 0x606980`.
            IMPLICIT_TARGET_ENEMY
                if !can_attack(Some(target), ctx.factions, ctx.reputations, Some(ctx.store)) =>
            {
                return (false, false);
            }
            // Reaction friendly or better, standing in for `CanAssist 0x6066f0`.
            IMPLICIT_TARGET_FRIEND
                if ring_reaction(ctx.factions, ctx.reputations, Some(target), Some(ctx.store))
                    < 4 =>
            {
                return (false, false);
            }
            _ => {}
        }
    }
    // Leg 11 (`0x6e3fb8`): only a cooldown-on-event spell greys, while its record is on hold
    // (`0x6e1690`); a running cooldown never greys here.
    if d.cooldown_on_event() && ctx.cooldowns.has_on_hold_record(spell_id, 0, d.category) {
        return (false, false);
    }
    // Leg 12 (`0x6e3fba`..`0x6e3feb`): power, the only notEnoughMana writer.
    if !can_afford(d, ctx.store, ctx.spell_mods) {
        return (false, true);
    }
    (true, false)
}

/// The resolved power cost, `GetPowerCost 0x6e31b0`: `manaCost`, plus the signed
/// `(level - baseLevel) * manaCostPerlevel` (only creature spells carry it), plus
/// `ManaCostPercentage` of a per-type basis: base mana for a mana spell, as the reference's
/// `0x612c50`; else the max pool, where `0x612c50` takes 1000 for rage, 100 for focus and energy
/// and base health for health. Then `SPELLMOD_COST` through `0x6e6af0` (`6e32e3`), clamped at 0.
/// The reference's per-school unit modifiers are not applied.
///
/// One cost for leg 12, the press-path gate and the tooltip, as `0x6e31b0` serves all six of its
/// callers. The press-path gate refuses an unaffordable cast without sending anything
/// (`0x609657`), so an unmodified cost refuses casts a talented character can afford.
pub(crate) fn power_cost(d: &SpellDisplay, store: &ObjectStore, mods: &SpellModifiers) -> u32 {
    let power_type = d.power_type as i32;
    let base = if d.mana_cost_pct == 0 {
        0
    } else if d.power_type == 0 {
        store.0.unit_base_mana().unwrap_or(0)
    } else if power_type < 0 {
        store.0.unit_max_health().unwrap_or(0)
    } else {
        store.0.unit_max_power(power_type as u8).unwrap_or(0)
    };
    // The level term reads `baseLevel` (`+0x70`), as `0x6e31b0` does, but the reference scales a
    // caster value / 5 there (vmangos: the level / 5); only creature spells have a per-level cost.
    let level_delta = i64::from(store.0.unit_level().unwrap_or(0)) - i64::from(d.base_level);
    let cost = i64::from(d.mana_cost)
        + level_delta * i64::from(d.mana_cost_per_level)
        + i64::from(base) * i64::from(d.mana_cost_pct) / 100;
    // The talent cut applies before the clamp at 0.
    let cost = cost.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    u32::try_from(mods.apply(d, OP_COST, cost)).unwrap_or(0)
}

/// Whether the caster can afford `d`, for leg 12 (`0x6e3fba`) and the press-path gate
/// (`0x60962c`): `UNIT_FIELD_POWER[type]`, or `UNIT_FIELD_HEALTH` for any negative PowerType
/// (`0x609631`; Bloodrage's -2), against [`power_cost`].
pub(crate) fn can_afford(d: &SpellDisplay, store: &ObjectStore, mods: &SpellModifiers) -> bool {
    let power_type = d.power_type as i32;
    let avail = if power_type < 0 {
        store.0.unit_health().unwrap_or(0)
    } else {
        store.0.unit_power(power_type as u8).unwrap_or(0)
    };
    avail >= power_cost(d, store, mods)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    // Field indices, private in the protocol crate: health 22, power1 23, max health 28, flags
    // 46, aurastate 125, bytes_1 138, player flags 190.
    fn player(pairs: &[(u16, u32)]) -> ObjectStore {
        // `UNIT_FLAG_PVP_ATTACKABLE` (bit 3), which every player carries: `CanAttack 0x606980`
        // picks its arm on that bit for both parties.
        let mut base = vec![(22u16, 100u32), (23, 500), (46, 1 << 3)];
        base.extend_from_slice(pairs);
        ObjectStore(ObjectFields::from_pairs(&base))
    }

    /// The disarm ladder hides one weapon, main hand first, so a dual-wielder still qualifies.
    #[test]
    fn a_disarmed_hand_does_not_satisfy_the_equipped_item_requirement() {
        use crate::items::TestDeps;

        // `PLAYER_FIELD_INV_SLOT_HEAD + 2 * slot` for equipment slots 15 and 16.
        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        const DISARMED: u32 = 0x0020_0000;

        // A class-2 weapon requirement.
        let needs_a_weapon = SpellDisplay {
            equipped_item_class: 2,
            // Subclass 7, Sword1H; a zero mask would short-circuit to "fits".
            equipped_item_subclass_mask: 1 << 7,
            ..Default::default()
        };
        let fits = |disarmed: bool, hands: &[(u16, u64)]| {
            let mut deps = TestDeps::new();
            let mut pairs = vec![(46u16, (1 << 3) | if disarmed { DISARMED } else { 0 })];
            for (i, (field, guid)) in hands.iter().enumerate() {
                pairs.push((*field, *guid as u32));
                let entry = 500 + i as u32;
                deps.spawn_item(*guid, ObjectFields::from_pairs(&[(3, entry)]));
                deps.items.insert_template(
                    entry,
                    Some(benilla_protocol::messages::ItemInfo {
                        class: 2,
                        subclass: 7,
                        ..crate::items::test_template("Sword")
                    }),
                );
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            deps.with_objects(|objects, items, commands| {
                equipped_item_fits(&needs_a_weapon, &store, objects, items, commands)
            })
        };

        // Armed.
        assert!(fits(false, &[(INV_MAINHAND, 0x2a)]));
        assert!(!fits(true, &[(INV_MAINHAND, 0x2a)]));
        // Disarmed dual-wielder: only the main hand is hidden.
        assert!(fits(true, &[(INV_MAINHAND, 0x2a), (INV_OFFHAND, 0x2b)]));
        // With an off hand only, the ladder hides that one.
        assert!(!fits(true, &[(INV_OFFHAND, 0x2b)]));
    }

    /// `0x5f0c50`'s slot mask and its two rejects, deprecated and broken.
    #[test]
    fn the_equipped_item_search_masks_by_hand_and_rejects_broken_gear() {
        use crate::items::TestDeps;

        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        // `ITEM_FIELD_DURABILITY` / `_MAXDURABILITY`, instance fields.
        const ITEM_DURABILITY: u16 = 46;
        const ITEM_MAX_DURABILITY: u16 = 47;

        let spell = |ex3: u32| SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 7, // Sword1H; a zero mask short-circuits to "fits"
            attributes_ex3: ex3,
            ..Default::default()
        };
        // `hands`: (inv field, guid, template flags, max durability, durability).
        let fits = |d: &SpellDisplay, hands: &[(u16, u64, u32, u32, u32)]| {
            let mut deps = TestDeps::new();
            let mut pairs = vec![(46u16, 1u32 << 3)];
            for (i, (field, guid, flags, max_dur, dur)) in hands.iter().enumerate() {
                pairs.push((*field, *guid as u32));
                let entry = 500 + i as u32;
                deps.spawn_item(
                    *guid,
                    ObjectFields::from_pairs(&[
                        (3, entry),
                        (ITEM_DURABILITY, *dur),
                        (ITEM_MAX_DURABILITY, *max_dur),
                    ]),
                );
                deps.items.insert_template(
                    entry,
                    Some(benilla_protocol::messages::ItemInfo {
                        class: 2,
                        subclass: 7,
                        flags: *flags,
                        ..crate::items::test_template("Sword")
                    }),
                );
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            deps.with_objects(|objects, items, commands| {
                equipped_item_fits(d, &store, objects, items, commands)
            })
        };

        let sound = |field| (field, 0x2au64, 0u32, 0u32, 0u32);

        // Hand-agnostic: either hand satisfies it.
        assert!(fits(&spell(0), &[sound(INV_MAINHAND)]));
        assert!(fits(&spell(0), &[sound(INV_OFFHAND)]));
        // Main hand only (`0x400`).
        assert!(fits(
            &spell(ATTR_EX3_MAIN_HAND_ONLY),
            &[sound(INV_MAINHAND)]
        ));
        assert!(!fits(
            &spell(ATTR_EX3_MAIN_HAND_ONLY),
            &[sound(INV_OFFHAND)]
        ));
        // Off hand only (`0x1000000`).
        assert!(fits(&spell(ATTR_EX3_OFF_HAND_ONLY), &[sound(INV_OFFHAND)]));
        assert!(!fits(
            &spell(ATTR_EX3_OFF_HAND_ONLY),
            &[sound(INV_MAINHAND)]
        ));

        // Deprecated, and broken (`MaxDurability > 0 && Durability == 0`).
        assert!(!fits(
            &spell(0),
            &[(INV_MAINHAND, 0x2a, ITEM_FLAG_DEPRECATED, 0, 0)]
        ));
        assert!(!fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 45, 0)]));
        // Damaged still counts, as does no durability at all.
        assert!(fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 45, 12)]));
        assert!(fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 0, 0)]));
    }

    /// A main-hand-only ability has nothing left to match once the disarm hides the main hand.
    #[test]
    fn a_main_hand_only_ability_is_dead_while_that_hand_is_disarmed() {
        use crate::items::TestDeps;

        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        const DISARMED: u32 = 0x0020_0000;

        let fits = |ex3: u32, disarmed: bool| {
            let mut deps = TestDeps::new();
            let mut pairs = vec![(46u16, (1 << 3) | if disarmed { DISARMED } else { 0 })];
            for (i, field) in [INV_MAINHAND, INV_OFFHAND].iter().enumerate() {
                let guid = 0x2a + i as u64;
                pairs.push((*field, guid as u32));
                let entry = 500 + i as u32;
                deps.spawn_item(guid, ObjectFields::from_pairs(&[(3, entry)]));
                deps.items.insert_template(
                    entry,
                    Some(benilla_protocol::messages::ItemInfo {
                        class: 2,
                        subclass: 7,
                        ..crate::items::test_template("Sword")
                    }),
                );
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            deps.with_objects(|objects, items, commands| {
                equipped_item_fits(
                    &SpellDisplay {
                        equipped_item_class: 2,
                        equipped_item_subclass_mask: 1 << 7, // Sword1H
                        attributes_ex3: ex3,
                        ..Default::default()
                    },
                    &store,
                    objects,
                    items,
                    commands,
                )
            })
        };

        assert!(fits(ATTR_EX3_MAIN_HAND_ONLY, false));
        assert!(fits(ATTR_EX3_OFF_HAND_ONLY, false));
        // Disarmed: the main hand is hidden first.
        assert!(!fits(ATTR_EX3_MAIN_HAND_ONLY, true));
        assert!(fits(ATTR_EX3_OFF_HAND_ONLY, true));
        assert!(fits(0, true));
    }

    fn ctx<'a>(
        store: &'a ObjectStore,
        cooldowns: &'a Cooldowns,
        reputations: &'a Reputations,
        carried: &'a std::collections::HashMap<u32, u32>,
        spell_mods: &'a SpellModifiers,
    ) -> UsableCtx<'a> {
        UsableCtx {
            store,
            target_store: None,
            factions: None,
            reputations,
            cooldowns,
            carried,
            spell_mods,
        }
    }

    fn walk(d: &SpellDisplay, store: &ObjectStore) -> (bool, bool) {
        let cooldowns = Cooldowns::default();
        let reputations = Reputations(Vec::new());
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let carried = crate::ui_items::carried_counts(&store.0, &objects);
        let spell_mods = SpellModifiers::default();
        spell_usable(
            1,
            d,
            &spells,
            &ctx(store, &cooldowns, &reputations, &carried, &spell_mods),
            &objects,
            &items,
            &commands,
        )
    }

    /// A -30% cost cell turns an unaffordable 100-mana spell into an affordable 70 with 80 mana.
    #[test]
    fn a_talent_cost_cut_reaches_the_cost_and_the_press_path_gate() {
        // Family 3 (mage), family bit 5.
        let d = SpellDisplay {
            spell_family: 3,
            spell_family_flags: 1 << 5,
            mana_cost: 100,
            ..Default::default()
        };
        let store = ObjectStore(ObjectFields::from_pairs(&[(23, 80)]));

        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        assert_eq!(
            power_cost(&d, &store, &mods),
            100,
            "no packet yet — the flat DBC cost"
        );
        assert!(
            !can_afford(&d, &store, &mods),
            "80 mana does not buy a 100 mana spell"
        );

        mods.set(false, 5, OP_COST, -30);
        assert_eq!(power_cost(&d, &store, &mods), 70);
        assert!(
            can_afford(&d, &store, &mods),
            "the talent brings it inside the pool, so TryCast must send the cast"
        );

        // Another class's family takes nothing.
        let other_class = SpellDisplay {
            spell_family: 4,
            ..d
        };
        assert_eq!(power_cost(&other_class, &store, &mods), 100);
    }

    /// Each gate trips alone; only the power leg raises notEnoughMana (`0x6e3feb`).
    #[test]
    fn gates_trip_independently_and_only_power_sets_oom() {
        let alive = player(&[]);
        let d = SpellDisplay::default();
        assert_eq!(walk(&d, &alive), (true, false));

        // Leg 1: dead, or a ghost at health 1 (field 190 bit `0x10`); the attribute waives it.
        let dead = player(&[(22, 0), (28, 100)]);
        assert_eq!(walk(&d, &dead), (false, false));
        let ghost = player(&[(22, 1), (28, 100), (190, 0x10)]);
        assert_eq!(walk(&d, &ghost), (false, false));
        let while_dead = SpellDisplay {
            attributes: ATTR_CASTABLE_WHILE_DEAD,
            ..Default::default()
        };
        assert_eq!(walk(&while_dead, &dead), (true, false));
        assert_eq!(walk(&while_dead, &ghost), (true, false));

        // Leg 3: a missing reagent.
        let reagent = SpellDisplay {
            reagents: [
                (17056, 1),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
            ],
            ..Default::default()
        };
        assert_eq!(walk(&reagent, &alive), (false, false));

        // Leg 5: Overpower's shape with no combo points (field 1222 byte 1).
        let overpower = SpellDisplay {
            attributes_ex: 0x4810_0200,
            ..Default::default()
        };
        assert!(overpower.needs_combo_points());
        assert_eq!(walk(&overpower, &alive), (false, false));
        let dodged = player(&[(1222, 0x05_03_01_01)]);
        assert_eq!(walk(&overpower, &dodged), (true, false));
        // The neighbouring bytes are not combo points.
        let ranked = player(&[(1222, 0x05_03_00_01)]);
        assert_eq!(walk(&overpower, &ranked), (false, false));

        // Leg 6: a cat-form spell out of form.
        let claw = SpellDisplay {
            stances: 0x1,
            ..Default::default()
        };
        assert_eq!(walk(&claw, &alive), (false, false));
        let in_cat = player(&[(138, 1 << 16)]);
        assert_eq!(walk(&claw, &in_cat), (true, false));

        // Leg 7: only-stealthed vs the CREEP byte.
        let ambush = SpellDisplay {
            attributes: ATTR_ONLY_STEALTHED,
            ..Default::default()
        };
        assert_eq!(walk(&ambush, &alive), (false, false));
        let sneaking = player(&[(138, 0x2 << 24)]);
        assert_eq!(walk(&ambush, &sneaking), (true, false));

        // Leg 8: not-in-combat vs UNIT_FLAG_IN_COMBAT.
        let mount = SpellDisplay {
            attributes: ATTR_NOT_IN_COMBAT,
            ..Default::default()
        };
        assert_eq!(walk(&mount, &alive), (true, false));
        let fighting = player(&[(46, UNIT_FLAG_IN_COMBAT)]);
        assert_eq!(walk(&mount, &fighting), (false, false));

        // Leg 9: CasterAuraState (defense = 1 → bit 0).
        let revenge = SpellDisplay {
            caster_aura_state: 1,
            ..Default::default()
        };
        assert_eq!(walk(&revenge, &alive), (false, false));
        let defended = player(&[(125, 0x1)]);
        assert_eq!(walk(&revenge, &defended), (true, false));

        // Leg 10: TargetAuraState with no current target.
        let execute = SpellDisplay {
            target_aura_state: 2,
            implicit_target_a1: 6,
            ..Default::default()
        };
        assert_eq!(walk(&execute, &alive), (false, false));

        // Leg 12: power, the only oom.
        let costly = SpellDisplay {
            mana_cost: 501,
            ..Default::default()
        };
        assert_eq!(walk(&costly, &alive), (false, true));

        // The early-out beats every gate.
        let tradeskill = SpellDisplay {
            effects: [SPELL_EFFECT_TRADE_SKILL, 0, 0],
            mana_cost: 9999,
            stances: 0x1,
            ..Default::default()
        };
        assert_eq!(walk(&tradeskill, &dead), (true, false));
    }

    /// Leg 10: the aura-state bit gates, and CanAttack passes on the default neutral reaction.
    #[test]
    fn target_aura_state_reads_the_current_target() {
        let me = player(&[]);
        let cooldowns = Cooldowns::default();
        let reputations = Reputations(Vec::new());
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let execute = SpellDisplay {
            target_aura_state: 2,
            implicit_target_a1: 6,
            ..Default::default()
        };

        let healthy = ObjectStore(ObjectFields::from_pairs(&[(22, 100), (125, 0)]));
        let low = ObjectStore(ObjectFields::from_pairs(&[(22, 10), (125, 0x2)]));
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let carried = crate::ui_items::carried_counts(&me.0, &objects);
        for (target, expect) in [(&healthy, false), (&low, true)] {
            let ctx = UsableCtx {
                store: &me,
                target_store: Some(target),
                factions: None,
                reputations: &reputations,
                cooldowns: &cooldowns,
                carried: &carried,
                spell_mods: &SpellModifiers::default(),
            };
            assert_eq!(
                spell_usable(5308, &execute, &spells, &ctx, &objects, &items, &commands),
                (expect, false)
            );
        }
    }
}
