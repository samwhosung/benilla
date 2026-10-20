//! The usable walk, `Spell_C::IsSpellUsableNow 0x6e3d60`: the ordered gates behind
//! `IsUsableAction`'s grey tint. Any tripped gate answers `(usable, oom) = (false, false)`; only
//! the power leg, the last, sets `notEnoughMana`.
//!
//! Built: the TRADE_SKILL early-out, dead (leg 1), lost control (2), reagents and totems (3), the
//! equipped item (4), combo points (5), shapeshift form (6), only-stealthed (7), not-in-combat
//! (8), CasterAuraState (9), TargetAuraState and its CanAttack/CanAssist fork (10/10b, the only
//! legs that read the current target), the bit-25 cooldown fold (11) and power (12).
//!
//! Not built, answering usable: leg 4's `AttributesEx3` sub-conditions. CanAssist in leg 10b is a
//! reaction-rank test (friendly or better), not the reference's `0x6066f0`.

use benilla_protocol::messages::ItemUseSpell;

use benilla_formats::{
    SpellDisplay, ATTR_CASTABLE_WHILE_DEAD, ATTR_NOT_IN_COMBAT, ATTR_ONLY_STEALTHED,
    SPELL_EFFECT_TRADE_SKILL,
};

use crate::items::Items;
use crate::net::{ObjectStore, Objects};
use crate::query_cache::Ask;
use crate::spell::Cooldowns;
use crate::spell::{SpellLevel, SpellModifiers, SpellReads, UnitField, Unrecorded, OP_COST};
use crate::target::{RelUnit, Relations};

use crate::ui_action::Spells;

/// Leg 8's caster unit flag.
use crate::player::UNIT_FLAG_IN_COMBAT;

/// The implicit targets leg 10b forks on (`0x6e3f8a`/`0x6e3fa2`): 6, single enemy, asks
/// `CanAttack 0x606980`; 21, single friend, asks `CanAssist 0x6066f0`.
const IMPLICIT_TARGET_ENEMY: u32 = 6;
const IMPLICIT_TARGET_FRIEND: u32 = 21;

/// Everything the walk reads besides the spell. `store` is the caster's, the active player's
/// ([`Relations::me`]), and `target` the current target's guid and descriptor (leg 10 reads the
/// current-target global `0xb4e2d8`, not an explicit cast target).
pub(crate) struct UsableCtx<'a> {
    pub(crate) store: &'a ObjectStore,
    pub(crate) target: Option<(u64, &'a ObjectStore)>,
    /// The relation functions leg 10b's fork asks.
    pub(crate) rel: Relations<'a>,
    pub(crate) cooldowns: &'a Cooldowns,
    /// The talent spell-modifier tables leg 12's cost goes through.
    pub(crate) spell_mods: &'a SpellModifiers,
    /// `[0xb4b3e4]` clear (`0x4958e0`): the player does not control the character. Leg 2 greys
    /// every spell its immunity scans do not exempt.
    pub(crate) control_lost: bool,
    /// Every entry's stack count at mask 0 for the frame ([`crate::ui_items::stack_counts`]):
    /// the reagent and totem counts of `0x6e4000`.
    pub(crate) stacks: &'a std::collections::HashMap<u32, u32>,
    /// The skill lines leg 12's cost finds its level through ([`SpellLevel::Player`]).
    pub(crate) skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
    /// `ItemSubClass.dbc`, whose `Flags` leg 4's ammo leg reads; absent when it did not load.
    pub(crate) sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
}

/// The search covers equipment indices `0..=22` (`0x5f0c50`'s `cmp ebx,0x17; jl`): the 19 worn
/// slots and the four equipped bags.
pub(crate) const EQUIPMENT_SLOTS: u8 = 23;

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

/// Whether the equipped-item search runs at all for `d`: the reference's short-circuits to "fits"
/// (`0x6e40e0` at `6e4103`..`6e4130`) are `EquippedItemClass < 0`, a zero subclass mask (no item
/// required, not a wildcard), and an item-targeting spell (`0x495d60` checks those).
fn needs_equipped_item(d: &SpellDisplay) -> bool {
    d.equipped_item_class >= 0
        && d.equipped_item_subclass_mask != 0
        && d.targets & TARGET_FLAG_ITEM == 0
}

/// The equipment slots [`equipped_item_fits`] reads for `d`, a bit each: none unless it searches,
/// else the slots its hand restriction leaves and both hands, which the disarm ladder reads
/// whatever the restriction (`0x5f0c69`, `0x5f0c91`).
#[cfg(test)]
pub(crate) fn worn_slots_read(d: &SpellDisplay) -> u32 {
    if needs_equipped_item(d) {
        (hand_mask(d) | HANDS) & EQUIPMENT_MASK
    } else {
        0
    }
}

/// The main and off hand, the two slots the disarm ladder tells apart.
const HANDS: u32 =
    1 << crate::items::EQUIPMENT_SLOT_MAINHAND | 1 << crate::items::EQUIPMENT_SLOT_OFFHAND;

/// Every equipment index the search covers, as a mask: the 19 worn slots and the four bags.
pub(crate) const EQUIPMENT_MASK: u32 = (1 << EQUIPMENT_SLOTS) - 1;

/// Leg 4 (`0x6e40e0`), shared with the tooltip's requirement line and the cast ladder's rung 7:
/// does a worn item match `EquippedItemClass` and `EquippedItemSubClassMask`? The templates are
/// only read (`0x6e428f`): one that has not streamed counts as a match. The search reports the
/// slots it reads to `reads`.
pub(crate) fn equipped_item_fits(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    reads: &mut dyn SpellReads,
) -> bool {
    if !needs_equipped_item(d) {
        return true;
    }
    // The disarm ladder reads both hands whatever the restriction (`0x5f0c69`, `0x5f0c91`), then
    // the search the slots the restriction leaves.
    reads.item_search((hand_mask(d) | HANDS) & EQUIPMENT_MASK);
    first_fitting_item(d, store, objects, items).is_some()
}

/// `FindEquippedItemForSpell 0x5f0c50`: the first worn item, in slot order, that fits `d`'s
/// requirement once the disarm ladder has hidden its hand.
fn first_fitting_item(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
) -> Option<u64> {
    let mut mask = hand_mask(d);
    if let Some(hidden) = crate::items::disarmed_equipment_slot(store, objects, items) {
        mask &= !(1u32 << hidden);
    }
    first_fitting_slot(
        store,
        mask,
        d.equipped_item_class as u32,
        d.equipped_item_subclass_mask,
        |guid| worn_item_cached(guid, objects, items),
    )
}

/// The ammo leg of `0x6e40e0` (`0x6e41f6`-`0x6e4373`). Its two callers both pass the ammo flag:
/// the cast request (`0x6e4dff`) and the usable walk (`0x6e3e67`); the spell tooltip calls the
/// worn-item search `0x5f0c50` alone (`0x52ef73`). `Some(ammo type)` refuses, `0x2f`
/// `SPELL_FAILED_NEED_AMMO` with the type as its argument (`0x6e434d`, `0x6e4372`).
/// `subclass_flags` is `ItemSubClass.dbc`'s `Flags` for a class and subclass, 0 for no row;
/// `held` is the inventory walk's count of an entry at mask 0 (`0x622420`).
pub(crate) fn ammo_refusal(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    subclass_flags: &dyn Fn(u32, u32) -> u32,
    held: &dyn Fn(u32) -> u32,
) -> Option<u32> {
    /// `ItemSubClass.dbc` `Flags & 0x10` (`0x6e4264`): a weapon that fires no ammo, in 5875 the
    /// Thrown row (2, 16) alone.
    const SUBCLASS_NO_AMMO: u32 = 0x10;
    /// `ITEM_CLASS_PROJECTILE`, the class the carried walk's callback takes (`0x6e43ab`).
    const CLASS_PROJECTILE: u32 = 6;
    if !needs_equipped_item(d) {
        return None;
    }
    let weapon = first_fitting_item(d, store, objects, items)?;
    // No record yet passes (`0x6e4296`), as no ammo type does (`0x6e42a7`).
    let template = items.template(objects.object(weapon)?.object_entry()?, 0, Ask::Read)?;
    if subclass_flags(template.class, template.subclass) & SUBCLASS_NO_AMMO != 0 {
        return None;
    }
    let ammo_type = template.ammo_type;
    if ammo_type == 0 {
        return None;
    }
    // `PLAYER_AMMO_ID` (`[+0xe68]+0x102c`), then a carried projectile of that entry and the
    // weapon's ammo type (`0x6e4331`, `0x6e43a0`).
    let ammo_id = store.0.player_ammo_id().unwrap_or(0);
    let carried = ammo_id != 0
        && items
            .template(ammo_id, 0, Ask::Read)
            .is_some_and(|t| t.class == CLASS_PROJECTILE && t.subclass == ammo_type)
        && held(ammo_id) > 0;
    (!carried).then_some(ammo_type)
}

/// The worn item `guid` names from the cached records alone: `None` while its object or its
/// template has not streamed.
fn worn_item_cached(guid: u64, objects: &Objects, items: &Items) -> Option<WornItem> {
    let obj = objects.object(guid)?;
    let t = items.template(obj.object_entry()?, 0, Ask::Read)?;
    Some(WornItem {
        class: t.class,
        subclass: t.subclass,
        flags: t.flags,
        durability: obj.item_durability(),
        max_durability: obj.item_max_durability(),
    })
}

/// What the equipped-item search learns of one equipment slot, and all it can: the feed compares
/// these frame to frame, so a view built on the search is rebuilt when a slot's answer can move,
/// and not on a durability tick that leaves an item unbroken.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum SlotItem {
    /// No guid: the search passes over the slot.
    Empty,
    /// A guid whose item object or template has not streamed: the search gives the benefit of the
    /// doubt.
    Unresolved,
    Item {
        class: u32,
        subclass: u32,
        /// `ITEM_FLAG_DEPRECATED`.
        deprecated: bool,
        /// `MaxDurability > 0 && Durability == 0`.
        broken: bool,
    },
}

impl SlotItem {
    fn of(guid: u64, worn: impl FnOnce(u64) -> Option<WornItem>) -> SlotItem {
        if guid == 0 {
            return SlotItem::Empty;
        }
        let Some(it) = worn(guid) else {
            return SlotItem::Unresolved;
        };
        SlotItem::Item {
            class: it.class,
            subclass: it.subclass,
            deprecated: it.flags & ITEM_FLAG_DEPRECATED != 0,
            broken: it.max_durability.is_some_and(|m| m > 0) && it.durability == Some(0),
        }
    }

    /// The search's verdict on the slot for a requirement.
    fn fits(self, class: u32, subclass_mask: u32) -> bool {
        match self {
            SlotItem::Empty => false,
            SlotItem::Unresolved => true, // unresolved template: benefit of the doubt
            // The reference's two rejects before the class match: deprecated, and broken.
            SlotItem::Item {
                deprecated: true, ..
            }
            | SlotItem::Item { broken: true, .. } => false,
            SlotItem::Item {
                class: c,
                subclass: sub,
                ..
            } => c == class && subclass_mask & (1 << sub) != 0,
        }
    }
}

/// Equipment slot `slot`'s [`SlotItem`] from the cached records, without an ask.
pub(crate) fn slot_item_cached(
    store: &ObjectStore,
    slot: u8,
    objects: &Objects,
    items: &Items,
) -> SlotItem {
    SlotItem::of(store.0.player_inv_slot(slot).unwrap_or(0), |guid| {
        worn_item_cached(guid, objects, items)
    })
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

/// The search body over a caller-supplied resolver, the guid of the first slot that fits; `None`
/// from the resolver (template not landed) counts as a match.
fn first_fitting_slot(
    store: &ObjectStore,
    mask: u32,
    class: u32,
    subclass_mask: u32,
    mut worn: impl FnMut(u64) -> Option<WornItem>,
) -> Option<u64> {
    (0..EQUIPMENT_SLOTS)
        .filter(|slot| mask & (1u32 << slot) != 0)
        .map(|slot| store.0.player_inv_slot(slot).unwrap_or(0))
        .find(|&guid| SlotItem::of(guid, &mut worn).fits(class, subclass_mask))
}

/// The item arm of the usable compute (`0x4e5050`), gate by gate:
///
/// 1. a held count of 0 (the cache at `0xbc6390`) answers `(false, false)` (`4e50ab`). `held` is
///    that cached count nonzero: the slot's stored count, which [`crate::ui_items::action_count`]
///    (`0x4e6d20`) wrote at the last recount.
/// 2. `IsItemOnCooldown 0x6e2fc0` (`4e50d0`) answers `(false, false)`. It asks `0x6e1690` with
///    the item's first on-use spell and the item entry as the key: an on-hold-record test, so a
///    running cooldown or the GCD never greys an item.
/// 3. the item's first on-use spell runs the whole plain-spell walk, [`spell_usable`] (`0x4e5a50`
///    hands it back with type 0; `4e51b0`). This is where food greys in combat: its rows carry
///    [`ATTR_NOT_IN_COMBAT`], leg 8.
/// 4. an item with no on-use spell, or no template yet, is usable (`4e5127`..`4e5135`) while the
///    player has control (`4e50f4`).
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
) -> (bool, bool) {
    if !held {
        return (false, false);
    }
    // Gate 4: no on-use spell, or no template yet.
    let Some(use_spell) = use_spell else {
        // The spell-less leg (`0x4e50f4`): lost control greys it before the item word is read.
        return (!ctx.control_lost, false);
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
    spell_usable(spell_id, d, spells, ctx, objects, items)
}

/// The walk; returns the `IsUsableAction` pair `(usable, not_enough_mana)`.
pub(crate) fn spell_usable(
    spell_id: u32,
    d: &SpellDisplay,
    spells: &Spells,
    ctx: &UsableCtx,
    objects: &Objects,
    items: &Items,
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
    // Leg 2 (`0x6e3dc3`-`0x6e3e4c`): without control, only a spell whose immunity covers the aura
    // that took it (fear, confusion, charm) passes.
    if ctx.control_lost
        && !crate::ui_unit::control_gate::exempts_lost_control(&spells.catalog, d, &ctx.store.0)
    {
        return (false, false);
    }
    // Leg 3 (`0x6e4000`): every reagent count stacks, every totem tool present.
    let stacks = |entry: u32| ctx.stacks.get(&entry).copied().unwrap_or(0);
    for &(entry, count) in &d.reagents {
        if entry != 0 && stacks(entry) < count {
            return (false, false);
        }
    }
    for &totem in &d.totems {
        if totem != 0 && stacks(totem) == 0 {
            return (false, false);
        }
    }
    // Leg 4 (`0x6e40e0`): some worn item matches the class and subclass mask, and its ammo leg,
    // the walk passing the ammo flag (`0x6e3e67`), finds a loaded and carried projectile.
    if !equipped_item_fits(d, ctx.store, objects, items, &mut Unrecorded) {
        return (false, false);
    }
    let flags = |class, subclass| ctx.sub_classes.map_or(0, |c| c.flags(class, subclass));
    if ammo_refusal(d, ctx.store, objects, items, &flags, &stacks).is_some() {
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
        let Some((target_guid, target)) = ctx.target else {
            return (false, false);
        };
        if target.0.unit_aura_state() & (1 << (d.target_aura_state - 1)) == 0 {
            return (false, false);
        }
        // The caster is the active player's object; without its guid there is no unit to ask.
        let Some(me) = ctx.rel.me else {
            return (false, false);
        };
        let caster = RelUnit::of(me, ctx.store);
        let target = RelUnit::of(target_guid, target);
        match d.implicit_target_a1 {
            // `CanAttack 0x606980`.
            IMPLICIT_TARGET_ENEMY if !ctx.rel.can_attack(&caster, &target) => {
                return (false, false);
            }
            // `CanAssist 0x6066f0`, the caster as `this` (`0x6e3f98`-`0x6e3f9b`).
            IMPLICIT_TARGET_FRIEND if !ctx.rel.can_assist(&caster, &target) => {
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
    if !can_afford(d, ctx.store, ctx.skill_lines, ctx.spell_mods) {
        return (false, true);
    }
    (true, false)
}

/// The resolved power cost, `GetPowerCost 0x6e31b0`: `manaCost`, plus the signed
/// `(level - baseLevel) * manaCostPerlevel` (only creature spells carry it), plus
/// `ManaCostPercentage` of the reference's per-type basis (`0x612c50`): base health for health,
/// base mana for mana, 1000 for rage, 100 for focus and energy, and 0 otherwise. Then the caster's
/// `POWER_COST_MODIFIER` and `POWER_COST_MULTIPLIER` for the spell's school, the clamp at 0, and
/// `SPELLMOD_COST` through `0x6e6af0` last (`0x6e32f0`), whose result is not clamped.
///
/// One cost for leg 12, the press-path gate and the tooltip, as `0x6e31b0` serves all six of its
/// callers. The press-path gate refuses an unaffordable cast without sending anything
/// (`0x609657`), so an unmodified cost refuses casts a talented character can afford.
pub(crate) fn power_cost(
    d: &SpellDisplay,
    store: &ObjectStore,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    mods: &SpellModifiers,
    reads: &mut dyn SpellReads,
) -> i32 {
    let level = SpellLevel::Player {
        me: Some(store),
        skill_lines,
    };
    power_cost_at(d, &store.0, level, mods, reads)
}

/// [`power_cost`] against `unit`'s fields at the level term's `level`, the value `0x6e31b0` reads
/// through the caster's `[vtbl+0xa8]` over 5 (`6e3242`-`6e3254`). With the unit selector set, the
/// tooltip's caster is the player's charm, else its summon (`6e31fe`-`6e3228`), whose own basis
/// `0x612c50` is called (`6e327d`). `reads` hears the level when the cost has a level term, and
/// the fields it reads.
pub(crate) fn power_cost_at(
    d: &SpellDisplay,
    unit: &benilla_protocol::ObjectFields,
    level: SpellLevel,
    mods: &SpellModifiers,
    reads: &mut dyn SpellReads,
) -> i32 {
    let base = match cost_basis(d) {
        CostBasis::None => 0,
        CostBasis::BaseMana => {
            reads.unit(UnitField::BaseMana);
            unit.unit_base_mana().unwrap_or(0)
        }
        CostBasis::BaseHealth => {
            reads.unit(UnitField::BaseHealth);
            unit.unit_base_health().unwrap_or(0)
        }
        CostBasis::Fixed(value) => value,
    };
    // Only a spell with a per-level cost, a creature's, reads the level. The sums are the
    // reference's 32-bit ones (`0x6e325f`-`0x6e32a1`).
    let level_term = if d.mana_cost_per_level != 0 {
        level
            .read(d, reads)
            .wrapping_sub(d.base_level)
            .wrapping_mul(d.mana_cost_per_level) as i32
    } else {
        0
    };
    let pct = (base as i32).wrapping_mul(d.mana_cost_pct as i32) / 100;
    let cost = (d.mana_cost as i32)
        .wrapping_add(level_term)
        .wrapping_add(pct);
    // The school's flat term (`0x6e32ad`), then its factor: `(1 + multiplier) × cost` on the x87
    // stack, stored as an `f32`, then `fistp`, which rounds to nearest even and stores the
    // integer indefinite for a NaN or a value out of range (`0x6e32c0`-`0x6e32d6`).
    reads.unit(UnitField::PowerCostModifier);
    reads.unit(UnitField::PowerCostMultiplier);
    let cost = cost.wrapping_add(unit.unit_power_cost_modifier(d.school).unwrap_or(0));
    let factor = f64::from(unit.unit_power_cost_multiplier(d.school).unwrap_or(0.0)) + 1.0;
    let scaled = f64::from((factor * f64::from(cost)) as f32).round_ties_even();
    let cost = if (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&scaled) {
        scaled as i32
    } else {
        i32::MIN
    };
    // The clamp at 0 (`0x6e32de`-`0x6e32ea`), then the talent modifier, which can take the cost
    // below 0 again (`0x6e32f0`).
    mods.apply(d, OP_COST, cost.max(0))
}

/// The per-second cost the tooltip's `_PER_TIME` cell shows, `0x6e3300`: `manaPerSecond` plus
/// `(level - baseLevel) * manaPerSecondPerLevel` (`0x6e3323`-`0x6e3335`), in 32-bit arithmetic,
/// `level` the value `0x6e3130` reads. `reads` hears the level when the term has a factor.
pub(crate) fn per_second_cost(
    d: &SpellDisplay,
    level: SpellLevel,
    reads: &mut dyn SpellReads,
) -> i32 {
    let level_term = if d.mana_per_second_per_level != 0 {
        level
            .read(d, reads)
            .wrapping_sub(d.base_level)
            .wrapping_mul(d.mana_per_second_per_level)
    } else {
        0
    };
    d.mana_per_second.wrapping_add(level_term) as i32
}

/// What [`power_cost_at`] scales `d`'s percentage cost by: `0x612c50`, a jump table on the power
/// type plus 2 (`612c53`-`612c5e`), on the caster's fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CostBasis {
    /// No percentage, or a type the table sends to 0 (`612c97`): the cost reads no unit field.
    None,
    /// Mana: `UNIT_FIELD_BASE_MANA`, unit block `+0x270` (`612c65`-`612c6b`).
    BaseMana,
    /// Health (-2): `UNIT_FIELD_BASE_HEALTH`, unit block `+0x274` (`612c87`-`612c8d`).
    BaseHealth,
    /// A constant, not the pool: 1000 for rage (`612c7e`), 100 for focus and energy (`612c75`).
    Fixed(u32),
}

fn cost_basis(d: &SpellDisplay) -> CostBasis {
    if d.mana_cost_pct == 0 {
        return CostBasis::None;
    }
    match d.power_type as i32 {
        -2 => CostBasis::BaseHealth,
        0 => CostBasis::BaseMana,
        1 => CostBasis::Fixed(1000),
        2 | 3 => CostBasis::Fixed(100),
        _ => CostBasis::None,
    }
}

/// Whether the caster can afford `d`, for leg 12 (`0x6e3fba`) and the press-path gate
/// (`0x60962c`): `UNIT_FIELD_POWER[type]`, or `UNIT_FIELD_HEALTH` for any negative PowerType
/// (`0x609631`; Bloodrage's -2), against [`power_cost`], both compares signed (`0x6e3fe2`,
/// `0x60965f`), so a cost a talent took below 0 is always affordable.
pub(crate) fn can_afford(
    d: &SpellDisplay,
    store: &ObjectStore,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    mods: &SpellModifiers,
) -> bool {
    let power_type = d.power_type as i32;
    let avail = if power_type < 0 {
        store.0.unit_health().unwrap_or(0)
    } else {
        store.0.unit_power(power_type as u8).unwrap_or(0)
    };
    avail as i32 >= power_cost(d, store, skill_lines, mods, &mut Unrecorded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reputations;
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
            deps.with_objects(|objects, items, _| {
                equipped_item_fits(&needs_a_weapon, &store, objects, items, &mut Unrecorded)
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
            deps.with_objects(|objects, items, _| {
                equipped_item_fits(d, &store, objects, items, &mut Unrecorded)
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
            deps.with_objects(|objects, items, _| {
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
                    &mut Unrecorded,
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

    /// `0x6e40e0`'s ammo leg: a weapon whose record names an ammo type (`+0x114`) fires only with
    /// `PLAYER_AMMO_ID` set (`0x6e42db`) and a carried projectile of that entry, class 6 and that
    /// subclass (`0x6e43a0`); an `ItemSubClass.dbc` row with flag `0x10` (Thrown), a weapon of
    /// ammo type 0 and a record not cached pass (`0x6e4264`, `0x6e42a7`, `0x6e4296`).
    #[test]
    fn a_ranged_weapon_needs_its_ammo_loaded_and_carried() {
        use crate::items::TestDeps;
        use benilla_protocol::messages::ItemInfo;

        const INV_RANGED: u16 = 486 + 2 * 17;
        const INV_BACKPACK: u16 = 486 + 2 * 23;
        const AMMO_ID: u16 = 1223;
        const BOW: u64 = 0x2a;
        const ARROWS: u64 = 0x2b;
        const BOW_ENTRY: u32 = 2504;
        const ARROW_ENTRY: u32 = 2512;

        let shot = SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 2 | 1 << 3 | 1 << 16 | 1 << 18,
            ..Default::default()
        };
        struct Case {
            ammo_type: u32,
            ammo_id: u32,
            carried: bool,
            projectile: (u32, u32),
            flags: u32,
            cached: bool,
        }
        let base = Case {
            ammo_type: 2,
            ammo_id: ARROW_ENTRY,
            carried: true,
            projectile: (6, 2),
            flags: 0,
            cached: true,
        };
        let refusal = |c: Case| {
            let mut deps = TestDeps::new();
            deps.spawn_item(BOW, ObjectFields::from_pairs(&[(3, BOW_ENTRY)]));
            if c.cached {
                deps.items.insert_template(
                    BOW_ENTRY,
                    Some(ItemInfo {
                        class: 2,
                        subclass: 2,
                        ammo_type: c.ammo_type,
                        ..crate::items::test_template("Bow")
                    }),
                );
            }
            deps.items.insert_template(
                ARROW_ENTRY,
                Some(ItemInfo {
                    class: c.projectile.0,
                    subclass: c.projectile.1,
                    ..crate::items::test_template("Arrow")
                }),
            );
            let mut pairs = vec![(INV_RANGED, BOW as u32), (AMMO_ID, c.ammo_id)];
            if c.carried {
                deps.spawn_item(ARROWS, ObjectFields::from_pairs(&[(3, ARROW_ENTRY)]));
                pairs.push((INV_BACKPACK, ARROWS as u32));
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            let flags = c.flags;
            deps.with_objects(|objects, items, _| {
                let held = |entry| {
                    crate::ui_items::count_of(
                        &store.0,
                        objects,
                        entry,
                        crate::ui_items::InventoryScope::DEFAULT,
                    )
                };
                let flags = |class, subclass| {
                    if (class, subclass) == (2, 2) {
                        flags
                    } else {
                        0
                    }
                };
                ammo_refusal(&shot, &store, objects, items, &flags, &held)
            })
        };

        assert_eq!(refusal(Case { ..base }), None, "loaded and carried");
        assert_eq!(
            refusal(Case { ammo_id: 0, ..base }),
            Some(2),
            "nothing loaded"
        );
        assert_eq!(
            refusal(Case {
                carried: false,
                ..base
            }),
            Some(2),
            "loaded but none carried"
        );
        assert_eq!(
            refusal(Case {
                projectile: (6, 3),
                ..base
            }),
            Some(2),
            "bullets for a bow"
        );
        assert_eq!(
            refusal(Case {
                projectile: (15, 2),
                ..base
            }),
            Some(2),
            "not a projectile"
        );
        assert_eq!(
            refusal(Case {
                ammo_id: 0,
                flags: 0x10,
                ..base
            }),
            None,
            "a subclass flagged 0x10"
        );
        assert_eq!(
            refusal(Case {
                ammo_id: 0,
                ammo_type: 0,
                ..base
            }),
            None,
            "no ammo type"
        );
        assert_eq!(
            refusal(Case {
                ammo_id: 0,
                cached: false,
                ..base
            }),
            None,
            "no record yet"
        );
    }

    /// The usable walk runs the equipped-item check with the ammo flag (`0x6e3e67`), so a bow
    /// with no arrows loaded greys the shot, and loaded arrows the bags hold light it.
    #[test]
    fn a_shot_without_ammo_greys_its_button() {
        use crate::items::TestDeps;
        use benilla_protocol::messages::ItemInfo;
        const INV_RANGED: u16 = 486 + 2 * 17;
        const INV_BACKPACK: u16 = 486 + 2 * 23;
        const AMMO_ID: u16 = 1223;
        const BOW: u64 = 0x2a;
        const ARROWS: u64 = 0x2b;
        const BOW_ENTRY: u32 = 2504;
        const ARROW_ENTRY: u32 = 2512;

        let shot = SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 2,
            ..Default::default()
        };
        let lit = |ammo_id: u32| {
            let mut deps = TestDeps::new();
            deps.spawn_item(BOW, ObjectFields::from_pairs(&[(3, BOW_ENTRY)]));
            deps.spawn_item(
                ARROWS,
                ObjectFields::from_pairs(&[(3, ARROW_ENTRY), (14, 200)]),
            );
            deps.items.insert_template(
                BOW_ENTRY,
                Some(ItemInfo {
                    class: 2,
                    subclass: 2,
                    ammo_type: 2,
                    ..crate::items::test_template("Bow")
                }),
            );
            deps.items.insert_template(
                ARROW_ENTRY,
                Some(ItemInfo {
                    class: 6,
                    subclass: 2,
                    ..crate::items::test_template("Arrow")
                }),
            );
            let store = player(&[
                (INV_RANGED, BOW as u32),
                (INV_BACKPACK, ARROWS as u32),
                (AMMO_ID, ammo_id),
            ]);
            let cooldowns = Cooldowns::default();
            let reputations = Reputations::default();
            let spells = Spells::empty_for_tests();
            let spell_mods = SpellModifiers::default();
            deps.with_objects(|objects, items, _| {
                let stacks = crate::ui_items::stack_counts(&store.0, objects);
                let ctx = ctx(&store, &cooldowns, &reputations, &stacks, &spell_mods);
                spell_usable(3044, &shot, &spells, &ctx, objects, items)
            })
        };
        assert_eq!(lit(0), (false, false), "nothing loaded");
        assert_eq!(lit(ARROW_ENTRY), (true, false), "loaded and carried");
    }

    /// The spell tooltip's requirement line searches the worn items alone (`0x52ef73`): an empty
    /// quiver leaves the bow's requirement met.
    #[test]
    fn the_worn_item_search_reads_no_ammo() {
        use crate::items::TestDeps;
        let mut deps = TestDeps::new();
        deps.spawn_item(0x2a, ObjectFields::from_pairs(&[(3, 2504)]));
        deps.items.insert_template(
            2504,
            Some(benilla_protocol::messages::ItemInfo {
                class: 2,
                subclass: 2,
                ammo_type: 2,
                ..crate::items::test_template("Bow")
            }),
        );
        let store = ObjectStore(ObjectFields::from_pairs(&[(486 + 2 * 17, 0x2a)]));
        let shot = SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 2,
            ..Default::default()
        };
        deps.with_objects(|objects, items, _| {
            assert!(equipped_item_fits(
                &shot,
                &store,
                objects,
                items,
                &mut Unrecorded
            ));
        });
    }

    /// The relation functions with nothing held and no catalog: every faction leg neutral.
    fn no_relations(reputations: &Reputations) -> Relations<'_> {
        Relations {
            factions: None,
            reputations,
            me: None,
            roster: None,
            units: &crate::target::relations::NoUnits,
        }
    }

    fn ctx<'a>(
        store: &'a ObjectStore,
        cooldowns: &'a Cooldowns,
        reputations: &'a Reputations,
        stacks: &'a std::collections::HashMap<u32, u32>,
        spell_mods: &'a SpellModifiers,
    ) -> UsableCtx<'a> {
        UsableCtx {
            store,
            target: None,
            rel: no_relations(reputations),
            cooldowns,
            stacks,
            spell_mods,
            control_lost: false,
            skill_lines: None,
            sub_classes: None,
        }
    }

    fn walk(d: &SpellDisplay, store: &ObjectStore) -> (bool, bool) {
        let cooldowns = Cooldowns::default();
        let reputations = Reputations::default();
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let stacks = crate::ui_items::stack_counts(&store.0, &objects);
        let spell_mods = SpellModifiers::default();
        spell_usable(
            1,
            d,
            &spells,
            &ctx(store, &cooldowns, &reputations, &stacks, &spell_mods),
            &objects,
            &items,
        )
    }

    /// Bloodrage (2687) takes 20% of base health, even when gear raises max health.
    #[test]
    fn bloodrage_cost_uses_base_health_for_cost_and_gates() {
        let bloodrage = SpellDisplay {
            power_type: (-2i32) as u32,
            mana_cost_pct: 20,
            ..Default::default()
        };
        let mods = SpellModifiers::default();
        let with_gear = player(&[(22, 400), (28, 4_000), (163, 1_689)]);
        assert_eq!(
            power_cost(&bloodrage, &with_gear, None, &mods, &mut Unrecorded),
            337
        );
        assert!(can_afford(&bloodrage, &with_gear, None, &mods));
        assert_eq!(walk(&bloodrage, &with_gear), (true, false));

        let below_cost = player(&[(22, 336), (28, 4_000), (163, 1_689)]);
        assert!(!can_afford(&bloodrage, &below_cost, None, &mods));
        assert_eq!(walk(&bloodrage, &below_cost), (false, true));

        let without_gear = player(&[(22, 400), (28, 1_689), (163, 1_689)]);
        assert_eq!(
            power_cost(&bloodrage, &without_gear, None, &mods, &mut Unrecorded),
            337
        );
    }

    /// `0x612c50`'s jump table (`0x612ca0`, indexed by the type plus 2): base health for -2, base
    /// mana, fixed bases for rage, focus and energy, and 0 for -1, happiness and past the table.
    #[test]
    fn percentage_cost_bases_follow_power_type() {
        let store = player(&[
            (28, 4_000), // max health
            (29, 2_000), // max mana
            (30, 800),   // max rage
            (31, 80),    // max focus
            (32, 120),   // max energy
            (33, 500),   // max happiness
            (162, 1_500),
            (163, 1_689),
        ]);
        let mods = SpellModifiers::default();
        let cases = [
            (-3, 0),
            (-2, 337),
            (-1, 0),
            (0, 300),
            (1, 200),
            (2, 20),
            (3, 20),
            (4, 0),
            (5, 0),
        ];
        for (power_type, cost) in cases {
            let spell = SpellDisplay {
                power_type: power_type as u32,
                mana_cost_pct: 20,
                ..Default::default()
            };
            assert_eq!(
                power_cost(&spell, &store, None, &mods, &mut Unrecorded),
                cost,
                "type {power_type}"
            );
        }
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
            power_cost(&d, &store, None, &mods, &mut Unrecorded),
            100,
            "no packet yet — the flat DBC cost"
        );
        assert!(
            !can_afford(&d, &store, None, &mods),
            "80 mana does not buy a 100 mana spell"
        );

        mods.set(false, 5, OP_COST, -30);
        assert_eq!(power_cost(&d, &store, None, &mods, &mut Unrecorded), 70);
        assert!(
            can_afford(&d, &store, None, &mods),
            "the talent brings it inside the pool, so TryCast must send the cast"
        );

        // Another class's family takes nothing.
        let other_class = SpellDisplay {
            spell_family: 4,
            ..d
        };
        assert_eq!(
            power_cost(&other_class, &store, None, &mods, &mut Unrecorded),
            100
        );
    }

    /// The school's flat term and factor (`0x6e32ad`, `0x6e32c0`-`0x6e32d6`), then the clamp at
    /// 0, then the talent modifier, which no clamp follows (`0x6e32de`-`0x6e32f0`).
    #[test]
    fn the_school_cost_terms_come_before_the_clamp_and_the_talent_after() {
        // A 100-mana Fire spell (school 2) of family 3, bit 5.
        let d = SpellDisplay {
            school: 2,
            spell_family: 3,
            spell_family_flags: 1 << 5,
            mana_cost: 100,
            ..Default::default()
        };
        let cost = |fields: &[(u16, u32)], flat: i32| {
            let mut mods = SpellModifiers::default();
            mods.set_class_family(3);
            mods.set(true, 5, OP_COST, flat);
            let store = player(fields);
            (
                power_cost(&d, &store, None, &mods, &mut Unrecorded),
                can_afford(&d, &store, None, &mods),
            )
        };
        let f = |v: f32| v.to_bits();
        assert_eq!(cost(&[], 0).0, 100);
        assert_eq!(cost(&[(175, 20)], 0).0, 120, "Fire's modifier");
        assert_eq!(cost(&[(174, 20), (181, f(1.0))], 0).0, 100, "Holy's");
        assert_eq!(cost(&[(182, f(-0.25))], 0).0, 75, "Fire's factor");
        assert_eq!(
            cost(&[(175, 20), (182, f(0.5))], 0).0,
            180,
            "the modifier, then the factor"
        );
        assert_eq!(cost(&[(175, 1), (182, f(-0.5))], 0).0, 50, "50.5 to even");
        assert_eq!(
            cost(&[(182, f(f32::NAN))], 0).0,
            0,
            "indefinite, then clamped"
        );
        let below = (-150i32) as u32;
        assert_eq!(cost(&[(175, below)], 0), (0, true), "clamped at 0");
        assert_eq!(
            cost(&[(175, below)], -10),
            (-10, true),
            "the talent after the clamp"
        );
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

    /// Leg 10b's friend arm is `CanAssist(caster, target)` (`0x6e3f98`-`0x6e3f9b`), not the
    /// reaction: a friendly creature the caster may not assist, being no PvP-flagged one
    /// (`0x6067bd`), greys the spell, and the same creature flagged lights it; a not-selectable
    /// friend greys it whatever its flag (`0x606706`).
    #[test]
    fn the_friend_arm_asks_can_assist_of_the_caster() {
        use crate::target::relations::fixture::{factions, Scene, HUMAN};
        let me = player(&[(2, 0x19), (35, HUMAN)]);
        let cooldowns = Cooldowns::default();
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let stacks = crate::ui_items::stack_counts(&me.0, &objects);
        // An implicit-21 spell gated on the target's aura state 2.
        let friend_spell = SpellDisplay {
            target_aura_state: 2,
            implicit_target_a1: 21,
            ..Default::default()
        };
        let scene = Scene::new(RelUnit::of(1, &me), Some(factions()));
        let friend = |flags: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x09),
                (22, 100),
                (35, HUMAN),
                (46, flags),
                (125, 0x2),
            ]))
        };
        for (flags, usable) in [(0, false), (0x1000, true), (0x1000 | (1 << 25), false)] {
            let target = friend(flags);
            let ctx = UsableCtx {
                store: &me,
                target: Some((2, &target)),
                rel: scene.rel(),
                cooldowns: &cooldowns,
                stacks: &stacks,
                spell_mods: &SpellModifiers::default(),
                control_lost: false,
                skill_lines: None,
                sub_classes: None,
            };
            assert_eq!(
                spell_usable(1, &friend_spell, &spells, &ctx, &objects, &items).0,
                usable,
                "flags {flags:#x}"
            );
        }
    }

    /// Leg 10: the aura-state bit gates, and CanAttack passes on the default neutral reaction.
    #[test]
    fn target_aura_state_reads_the_current_target() {
        let me = player(&[]);
        let cooldowns = Cooldowns::default();
        let reputations = Reputations::default();
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let execute = SpellDisplay {
            target_aura_state: 2,
            implicit_target_a1: 6,
            ..Default::default()
        };

        let healthy = ObjectStore(ObjectFields::from_pairs(&[(22, 100), (125, 0)]));
        let low = ObjectStore(ObjectFields::from_pairs(&[(22, 10), (125, 0x2)]));
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let stacks = crate::ui_items::stack_counts(&me.0, &objects);
        // Without the active player's guid there is no caster to ask the relation of, so the fork
        // refuses even the low target: no relation unit is built with guid 0.
        for (me_guid, target, expect) in [
            (Some(1), &healthy, false),
            (Some(1), &low, true),
            (None, &low, false),
        ] {
            let ctx = UsableCtx {
                store: &me,
                target: Some((2, target)),
                rel: Relations {
                    me: me_guid,
                    ..no_relations(&reputations)
                },
                cooldowns: &cooldowns,
                stacks: &stacks,
                spell_mods: &SpellModifiers::default(),
                control_lost: false,
                skill_lines: None,
                sub_classes: None,
            };
            assert_eq!(
                spell_usable(5308, &execute, &spells, &ctx, &objects, &items),
                (expect, false),
                "{me_guid:?}"
            );
        }
    }

    /// The walk with the control flag set as given, against `spells`' catalog.
    fn walk_lost(
        d: &SpellDisplay,
        store: &ObjectStore,
        spells: &Spells,
        control_lost: bool,
    ) -> (bool, bool) {
        let cooldowns = Cooldowns::default();
        let reputations = Reputations::default();
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let stacks = crate::ui_items::stack_counts(&store.0, &objects);
        let spell_mods = SpellModifiers::default();
        let mut c = ctx(store, &cooldowns, &reputations, &stacks, &spell_mods);
        c.control_lost = control_lost;
        spell_usable(1, d, spells, &c, &objects, &items)
    }

    /// `UNIT_FIELD_FLAGS` (46) bit 23 and the first `UNIT_FIELD_AURA` slot (47).
    const FLEEING: u32 = 0x0080_0000;

    /// Leg 2 (`0x6e3dc3`-`0x6e3e4c`): with `[0xb4b3e4]` clear every spell greys, but the one whose
    /// immunity covers the fear that took control (Will of the Forsaken's shape) stays lit.
    #[test]
    fn lost_control_greys_every_spell_an_immunity_scan_does_not_exempt() {
        const FEAR_AURA: u32 = 5782;
        let mut fear = SpellDisplay {
            mechanic: 5,
            ..Default::default()
        };
        fear.effects[0] = 6;
        fear.effect_apply_aura[0] = 7;
        let mut immunity = SpellDisplay {
            attributes_ex: 0x8000,
            ..Default::default()
        };
        immunity.effects[0] = 6;
        immunity.effect_apply_aura[0] = 77;
        immunity.effect_misc_value[0] = 5;
        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays([(FEAR_AURA, fear)].into());
        let plain = SpellDisplay::default();

        let feared = player(&[(46, (1 << 3) | FLEEING), (47, FEAR_AURA)]);
        assert_eq!(walk_lost(&plain, &feared, &spells, false), (true, false));
        assert_eq!(
            walk_lost(&plain, &feared, &spells, true),
            (false, false),
            "a plain spell greys while control is lost"
        );
        assert_eq!(
            walk_lost(&immunity, &feared, &spells, true),
            (true, false),
            "the fear immunity passes the scan"
        );
        // The same spell without the fear flag asks no scan: control lost for another reason.
        let rooted = player(&[(47, FEAR_AURA)]);
        assert_eq!(walk_lost(&immunity, &rooted, &spells, true), (false, false));
        // A charm reads `UNIT_FIELD_CHARMEDBY` (10) with no aura of its type on the caster: no
        // aura found, no exemption.
        let charmed = player(&[(10, 0x77)]);
        assert_eq!(
            walk_lost(&immunity, &charmed, &spells, true),
            (false, false)
        );
    }

    /// The spell-less leg (`0x4e50f4`) of the item arm: an item with no on-use spell greys while
    /// control is lost.
    #[test]
    fn an_item_with_no_on_use_spell_greys_without_control() {
        let cooldowns = Cooldowns::default();
        let reputations = Reputations::default();
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let store = player(&[]);
        let stacks = std::collections::HashMap::new();
        let spell_mods = SpellModifiers::default();
        let mut c = ctx(&store, &cooldowns, &reputations, &stacks, &spell_mods);
        let usable = |c: &UsableCtx| item_usable(6948, None, true, c, None, &objects, &items);
        assert_eq!(usable(&c), (true, false));
        c.control_lost = true;
        assert_eq!(usable(&c), (false, false));
    }
}
