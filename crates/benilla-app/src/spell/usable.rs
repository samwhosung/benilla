//! The plain-spell **usable walk** — `Spell_C::IsSpellUsableNow 0x6e3d60`, the compute behind
//! `IsUsableAction`'s grey tint beyond the power gate. Its ordered gate table, transcribed;
//! any tripped gate answers `(usable=false, oom=false)`, and ONLY the power leg (the last) sets
//! `notEnoughMana`.
//!
//! Modeled legs: the TRADE_SKILL early-out · dead (leg 1) · reagents/totems (leg 3) · required
//! equipped item (leg 4) · the combo-point gate (leg 5) · the shapeshift-form gate (leg 6,
//! [`SpellDisplay::usable_in_form`]) · only-stealthed (leg 7) · not-in-combat (leg 8) ·
//! CasterAuraState (leg 9) · TargetAuraState + its CanAttack/CanAssist fork (legs 10/10b — the
//! ONE target-dependent pair: the current-target GUID globals `0xb4e2d8/dc` appear
//! nowhere earlier in the whole function, 0879; our per-frame diff-push recomputes it on target
//! change for free, where the ref re-runs its cache on events) · the bit-25 cooldown fold
//! (leg 11) · power (leg 12).
//!
//! Deferred, named (each answers usable=true until modeled): leg 2's caster aura-immunity
//! helpers (`0x6e9f20/40/60` — silence/pacify vs the spell's school/mechanic; needs an aura-type
//! model), leg 4's `AttributesEx3` sub-conditions and the broken-durability exclusion (no
//! durability model), and the ghost state beyond plain death. CanAssist inside 10b is the
//! reaction-rank stand-in the ring/`can_attack` share, pending the true `0x6066f0` walk.

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

/// Leg 8's caster unit-flag test — the shared bit, declared once ([`crate::player`]).
use crate::player::UNIT_FLAG_IN_COMBAT;

/// The implicit-target enums leg 10b forks on (`0x6e3f8a`/`0x6e3fa2`): 6 = single enemy →
/// `CanAttack 0x606980`, 21 = single friend → `CanAssist 0x6066f0`.
const IMPLICIT_TARGET_ENEMY: u32 = 6;
const IMPLICIT_TARGET_FRIEND: u32 = 21;

/// Everything the walk reads besides the spell itself. `target_store` is the CURRENT TARGET's
/// (leg 10 resolves the current-target global `0xb4e2d8`, not an explicit cast target).
pub(crate) struct UsableCtx<'a> {
    pub(crate) store: &'a ObjectStore,
    pub(crate) target_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
    pub(crate) cooldowns: &'a Cooldowns,
    /// The talent spell-modifier tables — leg 12's cost goes through them ([`power_cost`]).
    pub(crate) spell_mods: &'a SpellModifiers,
    /// Every carried entry's count, walked ONCE by the caller for the frame
    /// ([`crate::ui_items::carried_counts`]) — leg 3 reads reagents and totems off it instead
    /// of walking the bags per reagent per slot.
    pub(crate) carried: &'a std::collections::HashMap<u32, u32>,
}

/// How many equipment indices the search covers — `0..=22` (`0x5f0c50`'s `cmp ebx,0x17; jl`): the
/// 19 worn slots plus the four equipped bags. We walked 19 before.
const EQUIPMENT_SLOTS: u8 = 23;

/// `AttributesEx3`'s two hand restrictions, which is where the reference's **slot mask** comes
/// from (`0x5f0c50`'s callers): `0x400` → main hand only (mask `0x8000`), `0x1000000` → off hand
/// only (mask `0x10000`), neither → every slot. Without this a main-hand-only ability counted a
/// weapon worn anywhere, which is what made the disarm case interesting to get right: strip the
/// hidden hand out of a mask that is already down to one bit and nothing can satisfy it.
const ATTR_EX3_MAIN_HAND_ONLY: u32 = 0x0000_0400;
const ATTR_EX3_OFF_HAND_ONLY: u32 = 0x0100_0000;

/// `ITEM_FLAG_DEPRECATED` (vmangos `ItemPrototype.h`: *"appears red icon (like when item
/// durability==0)"*) — one of the two rejects the reference's search applies to a worn item before
/// matching its class. The other is being genuinely broken.
const ITEM_FLAG_DEPRECATED: u32 = 0x0000_0010;

/// `TARGET_FLAG_ITEM` (`Targets`, column 13, bit 4) — an item-targeting spell. Its
/// `EquippedItem*` columns describe the **clicked item**, not the caster's gear, so the
/// equipped-item search short-circuits on it (`0x6e40e0`'s second gate).
const TARGET_FLAG_ITEM: u32 = 0x0000_0010;

/// The search's **slot mask**, from `AttributesEx3` (`0x6e4136`–`0x6e4153`): bit 10 → equipment
/// index 15 alone, else bit 24 → index 16 alone, else every slot.
fn hand_mask(d: &SpellDisplay) -> u32 {
    if d.attributes_ex3 & ATTR_EX3_MAIN_HAND_ONLY != 0 {
        1 << crate::items::EQUIPMENT_SLOT_MAINHAND
    } else if d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0 {
        1 << crate::items::EQUIPMENT_SLOT_OFFHAND
    } else {
        u32::MAX
    }
}

/// [`equipped_item_fits`]'s **read-only** twin, for the cast ladder's rung 7: the same search over
/// the same slots, but it never asks the server for a missing template — it runs inside a `&Items`
/// borrow. By the time a button is pressed the greying feed that shares this search has had the
/// template for many frames, and an uncached one is the shared benefit-of-the-doubt anyway.
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

/// **Which equipped-item reason the cast refuses with** — TryCast rung 7's own selection
/// (`0x6e40e0` @ `6e416e`–`6e4180`). It is keyed on `AttributesEx3` **alone**: not
/// on `EquippedItemClass`, and not on which hand's search failed.
///
/// ```text
/// 6e4171: test dh,0x4      ; bit 10 (0x400)  main-hand-only -> 0x1a
/// 6e417a: shr edx,0x17     ; else bit 24 (0x1000000) off-hand-only -> 0x1b
/// 6e4180: or  dl,0x19      ; else -> 0x19
/// ```
///
/// All three render `ERR_SPELL_FAILED_EQUIPPED_ITEM_CLASS_S` — "Must have a %s equipped" — with
/// the item **subclass** name. `0x18` ("Must have the proper item equipped") is **not** from this
/// function: it belongs to the next rung, the ranged-slot check, and the disarm gate never touches
/// slot 2. `0x2f` is this function's ammo tail, not an equipped-item reason at all.
pub(crate) fn equipped_item_reason(d: &SpellDisplay) -> u8 {
    if d.attributes_ex3 & ATTR_EX3_MAIN_HAND_ONLY != 0 {
        0x1a
    } else if d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0 {
        0x1b
    } else {
        0x19
    }
}

/// Leg 4's own test (`0x6e40e0`), shared with the spell tooltip's requirement line: does some
/// WORN item match `EquippedItemClass` + `EquippedItemSubClassMask`? `true` when the spell asks
/// for nothing (`class < 0`). An equipped item whose template hasn't streamed yet counts as a
/// match — never grey (and never red) on missing data, the catalog-absent convention.
pub(crate) fn equipped_item_fits(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> bool {
    // The reference's four short-circuits, all answering "fits" without looking at a single slot
    // (`0x6e40e0` @ `6e4103`–`6e4130`; decision 1925 correcting 1903, which shipped only the first
    // of them):
    //
    // 1. the caster is not the active player — so a **pet** cast never takes this refusal;
    // 2. `EquippedItemClass < 0` — the spell asks for nothing;
    // 3. **`EquippedItemSubClassMask == 0`** — likewise, and this one is not the same as "every
    //    subclass matches": the reference answers fits with **no item required at all**, where
    //    treating it as a wildcard still demands something of the right class be worn. That was
    //    1903's bug, and it made a mask-less requirement refusable on an empty slot;
    // 4. `Targets & TARGET_FLAG_ITEM` — an item-targeting spell describes the CLICKED item with
    //    these columns, not the caster's gear, and a different validator (`0x495d60`) enforces
    //    them there.
    //
    // Leg 1 is ours by construction: this only ever runs for the local player.
    if d.equipped_item_class < 0
        || d.equipped_item_subclass_mask == 0
        || d.targets & TARGET_FLAG_ITEM != 0
    {
        return true;
    }
    let class = d.equipped_item_class as u32;
    // The **slot mask** first: `AttributesEx3` narrows the search to one hand for
    // an ability that names one, and to everything otherwise.
    let mut mask = hand_mask(d);
    // …then the disarm ladder strips the hidden hand's bit out of it (its citation
    // corrected by 1903: the reference does this at `0x5f0c69`/`0x5f0c91` with `visFlag = 1`
    // probes and a mask edit, NOT by `GetWeapon` returning NULL — same ladder, same outcome, one
    // hand only). A disarmed warrior's Heroic Strike greys out and its tooltip requirement line
    // turns red; a disarmed dual-wielder's off-hand weapon still satisfies a hand-agnostic one.
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
    /// The **template** flags — `ITEM_FLAG_DEPRECATED` is the reject.
    pub(crate) flags: u32,
    /// The **instance** durability pair (`[item+0x114]+0xa0`/`+0xa4`), not the template's.
    pub(crate) durability: Option<u32>,
    pub(crate) max_durability: Option<u32>,
}

/// The search body, over a per-slot resolver the caller supplies — because two callers need it
/// with different borrows: the greying leg and the tooltip hold `&mut Items` and may ASK the
/// server on a miss, while the cast ladder's rung 7 runs inside a `&Items` borrow and must not.
/// `None` from the resolver is the shared benefit-of-the-doubt: an item whose
/// template has not landed counts as a match, never a refusal on missing data.
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
            // The reference's two rejects, applied before the class match: a DEPRECATED item and
            // a genuinely BROKEN one (`MaxDurability > 0 && Durability == 0`) cannot satisfy a
            // requirement. Note this is the one place durability DOES gate something — it drives
            // no animation or model path anywhere.
            if it.flags & ITEM_FLAG_DEPRECATED != 0 {
                return false;
            }
            if it.max_durability.is_some_and(|m| m > 0) && it.durability == Some(0) {
                return false;
            }
            // A zero subclass mask never reaches here — it short-circuits above, as the
            // reference does.
            it.class == class && subclass_mask & (1 << it.subclass) != 0
        })
}

/// The **ITEM arm** of the usable compute — `0x4e5050`'s item branch, whole. benilla answered
/// `count > 0 || equipped` here and nothing else, which left a stack of food full-colour in
/// combat; the reference runs three more gates, and the third is the whole spell walk.
///
/// 1. **the count cache** `[0xbc6390+slot*4] == 0` ⇒ `(false, false)` (`4e50ab`). `held` is our
///    stand-in: carried copies ([`crate::ui_items::InventoryScope::CARRIED`], decision 1158's documented narrowing)
///    or a copy worn on an equipment slot. The reference fills that cache from `0x4e6d20`, whose
///    mask `0x4e6d20`→`0x622439` rewrites to **`0x47`** — equipment `0..0x12`, bag slots, the
///    backpack, container contents and **the keyring**, bank excluded. So the `|| equipped` half
///    is *redundant* there rather than an addition (worn copies already count), and two narrower
///    divergences stay open and named: our `CARRIED` omits the **keyring** (a key on the bar reads
///    count 0 where the reference reads 1), and we do not model the reference's charges fork —
///    a `spellcharges[0] ∉ {0, -1}` item sums `|ITEM_FIELD_SPELL_CHARGES[0]|`, so a spent
///    0-charge copy greys at `4e50ab`. Both are count-scope work, not this arm's.
/// 2. **`IsItemOnCooldown 0x6e2fc0`** (`4e50d0`) ⇒ `(false, false)`. It resolves the item's FIRST
///    ON_USE block — `rec+0x11c[i] > 0 && rec+0x130[i] == 0`, `6e3006`–`6e3023`, the same scan
///    behind [`benilla_protocol::messages::ItemInfo::use_spell`] — and asks
///    `0x6e1690(spellId, itemId = the item ENTRY)`. That predicate is the **on-hold-record** test,
///    NOT a general on-cooldown one: it reads neither a running timed cooldown nor the
///    GCD. So a potion mid-cooldown keeps its colour under its own sweep, and nothing on the bar
///    greys for the global cooldown. **The entry is the key**, not `0`: `0x6e2fc0` is the sole
///    consumer of `0x6e1690`'s item-keyed form image-wide (`6e3037 push esi`), every other caller
///    passing `0` — so querying leg 11's `(spell, 0)` form here would never find an item's own
///    parked record, which is how our cooldown store keys it (`(use_spell, entry)`).
/// 3. **the plain-spell walk.** The resolver `0x4e5a50`'s ITEM arm hands that same first ON_USE
///    spell back with `*outType = 0` **hard-written**, so
///    `4e51ab`/`4e51b0 je 0x4e521d` is taken and the button runs the entire
///    `Spell_C::IsSpellUsableNow 0x6e3d60` gate walk — [`spell_usable`] — exactly as a spell slot
///    does. **This is where food greys in combat**: every `Food`/`Drink` row in the shipped
///    `Spell.dbc` (433/434/435, 1127/1129/1131, 1133/1135/1137, 5004–5007, 6410, …) carries
///    `Attributes = 0x18000100`, bit 28 among them — [`ATTR_NOT_IN_COMBAT`], the walk's leg 8. It
///    is not a food rule: 453 of the shipped rows carry that bit (bandages, mounts, disguises).
/// 4. an item with **no** on-use spell resolves 0 and falls into the resolver-0 leg, whose ITEM-tag
///    test answers `usable = 1` outright (`4e5127`–`4e5135`, once the item-count and item-cooldown
///    checks pass). An equipped sword on the bar is
///    full-colour, not grey — and so is an item whose template is still in flight, which is what
///    keeps a freshly-seen slot from flickering grey for a frame.
///
/// A resolved on-use spell the catalog has no row for reads **grey** with `notEnoughMana = 0` —
/// the reference's `4e5193 jg` / `4e51a0 jne` fall-through at `4e51aa`, and the same answer the
/// SPELL arm already gives an unknown id.
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
    // Gate 4's early answer: no on-use spell (or no template yet) — the ITEM tag alone lights it.
    let Some(use_spell) = use_spell else {
        return (true, false);
    };
    let spell_id = use_spell.spell_id;
    let d = spells.and_then(|s| s.catalog.get(spell_id));
    // Gate 2, keyed as `0x6e2fc0` keys it: the item ENTRY as `0x6e1690`'s `itemId`, and the
    // item's own resolved category where it has one (the `spellcategory[5]` override, `6e171b`).
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

/// The walk. Returns `(usable, not_enough_mana)` — the `IsUsableAction` pair.
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
    // Leg 1: dead casters use nothing without the castable-while-dead attribute.
    if ctx.store.0.unit_health() == Some(0) && d.attributes & ATTR_CASTABLE_WHILE_DEAD == 0 {
        return (false, false);
    }
    // Leg 3 (`0x6e4000`): every reagent pair in bag counts; every totem tool present.
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
    // Leg 4 (`0x6e40e0`): some worn item must match the class + subclass mask.
    if !equipped_item_fits(d, ctx.store, objects, items, commands) {
        return (false, false);
    }
    // Leg 5 (`0x6e3e7a`–`0x6e3eb2`): the combo-point gate. A
    // `test [SpellRec+0x1c], 0x500000` selects finishing moves — ONE any-of test with one jcc, so
    // `AttributesEx` b20 and b22 never fork — and then `mov al,[ecx+0x1029]; test al,al` fails the
    // leg when the caster's combo-point byte is 0. That is `PLAYER_FIELD_BYTES` byte 1 off the
    // player-block base, one byte below the honor rank the item-usable gate reads at `+0x102b`.
    // The leg's second clause (`0x6e3e83`–`0x6e3e9e`) re-checks the caster's own GUID against the
    // active player's — structurally tautological, since `esi` was found by that same GUID.
    //
    // CASTER-only, now proven rather than assumed: the current-target GUID globals `0xb4e2d8/dc`
    // appear nowhere before leg 10 in the whole of `0x6e3d60`. So a point banked on mob A with mob
    // B selected really does leave this button lit, and the server really does refuse it with
    // `SPELL_FAILED_BAD_TARGETS` — the client's own divergence, not ours. Overpower
    // is this leg's whole reason to exist: it has no aura state, so every other leg passes and the
    // button stayed lit forever without it.
    //
    // NO class gate here, and that asymmetry is the whole point: the Lua `GetComboPoints 0x51a190`
    // reads this same byte behind `cmp al,4 / cmp al,0xb`, so a warrior's Overpower point greys
    // the button through this leg while lighting no combo dot. Anything the
    // *client* gates on reads the wire, never `GetComboPoints`.
    if d.needs_combo_points() && ctx.store.0.player_combo_points().unwrap_or(0) == 0 {
        return (false, false);
    }
    // Leg 6 (`0x612480`): the shapeshift-form gate — the form's stance flag from
    // SpellShapeshiftForm.dbc decides whether it counts as "shapeshifted".
    let form = ctx.store.0.unit_shapeshift_form();
    let form_is_stance = spells
        .forms
        .get(&u32::from(form))
        .is_some_and(|f| f.is_stance());
    if !d.usable_in_form(form, form_is_stance) {
        return (false, false);
    }
    // Leg 7: only-stealthed spells need the CREEP vis flag (the stealth aura's byte).
    if d.attributes & ATTR_ONLY_STEALTHED != 0 && !ctx.store.0.unit_is_stealthed() {
        return (false, false);
    }
    // Leg 8: only-out-of-combat spells grey while UNIT_FLAG_IN_COMBAT is up.
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
    // Legs 10/10b: the target's aura state — the walk's ONE target-dependent pair (`0x6e3f58`).
    // No current target ⇒ unusable; then the aura-state bit; then the relation fork.
    if d.target_aura_state != 0 {
        let Some(target) = ctx.target_store else {
            return (false, false);
        };
        if target.0.unit_aura_state() & (1 << (d.target_aura_state - 1)) == 0 {
            return (false, false);
        }
        match d.implicit_target_a1 {
            // The shared `CanAttack 0x606980` the ring/scan transcribe.
            IMPLICIT_TARGET_ENEMY
                if !can_attack(Some(target), ctx.factions, ctx.reputations, Some(ctx.store)) =>
            {
                return (false, false);
            }
            // CanAssist stand-in: reaction rank >= friendly (module docs).
            IMPLICIT_TARGET_FRIEND
                if ring_reaction(ctx.factions, ctx.reputations, Some(target), Some(ctx.store))
                    < 4 =>
            {
                return (false, false);
            }
            _ => {}
        }
    }
    // Leg 11: ONLY a cooldown-on-event spell folds its cooldown into usable (`0x6e3fb8`) — and the
    // predicate is the corrected `0x6e1690` (an on-hold-record test): Stealth greys while its
    // record is PARKED; once the event starts the clocks — and for
    // every ordinary cooldown — the button never greys from here.
    if d.cooldown_on_event() && ctx.cooldowns.has_on_hold_record(spell_id, 0, d.category) {
        return (false, false);
    }
    // Leg 12 (`0x6e3fba`–`0x6e3feb`): the power gate — the SOLE notEnoughMana writer.
    if !can_afford(d, ctx.store, ctx.spell_mods) {
        return (false, true);
    }
    (true, false)
}

/// The RESOLVED power cost — `GetPowerCost 0x6e31b0`'s data terms: the flat `manaCost`, the
/// per-level term `(level − spellLevel) · manaCostPerlevel` (signed delta, the vmangos
/// `CalculatePowerCost` shape, clamped at 0 on the way out; every nonzero row is a creature
/// spell, so the clamp choice is dormant for players), and `ManaCostPercentage` of the pool
/// 0948 chose per type (base mana for mana spells — the vmangos basis — max pool otherwise,
/// health included for negative types) — and then the talent cost cut, `SPELLMOD_COST` (op 14),
/// which `0x6e31b0` applies inside itself at `6e32e3` through the integer applier `0x6e6af0`
/// (`crate::spell::mods`). One law, every consumer: the usable walk's leg 12, the press-path
/// power gate, and the tooltip's cost cell (1074) — mirroring the byte fn's own caller
/// set (`0x609657`/`0x60968d`/`0x4e5201`/`0x6e3fdb`/`0x52e8ad`/`0x507de3`). The reference's
/// per-school unit mods are still not modeled — what remains of 0948's standing gap.
///
/// **The modifier is why this one is behavioural and not only cosmetic**: the press-path gate
/// refuses a cast it judges unaffordable and sends **no packet at all** (`0x609657`'s failure
/// block), so an unmodified cost makes benilla refuse casts a talented character can afford.
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
    // The per-level column here has ALWAYS read `+0x70` (baseLevel); whether `0x6e31b0`'s own
    // level term reads `+0x70` or `+0x74` is un-re-verified — dormant either way, since every
    // nonzero `manaCostPerlevel` row is a creature spell (the fn doc above).
    let level_delta = i64::from(store.0.unit_level().unwrap_or(0)) - i64::from(d.base_level);
    let cost = i64::from(d.mana_cost)
        + level_delta * i64::from(d.mana_cost_per_level)
        + i64::from(base) * i64::from(d.mana_cost_pct) / 100;
    // The talent cut, on the data terms and before the 0-clamp on the way out. The relative order
    // of the two is dormant for players — every nonzero `manaCostPerlevel` row is a creature spell
    // (above), so nothing reaches the modifier negative.
    let cost = cost.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    u32::try_from(mods.apply(d, OP_COST, cost)).unwrap_or(0)
}

/// Whether the caster can afford `d`'s power cost — the shared availability-vs-cost compare
/// behind BOTH the usable walk's leg 12 (`0x6e3fba`) and the press-path power gate
/// (`0x6094f0` @ `0x60962c`): raw `UNIT_FIELD_POWER[type]` — ANY negative
/// PowerType reads `UNIT_FIELD_HEALTH` instead (the ref's `jl` at `0x609631`; Bloodrage's −2) —
/// signed-compared against [`power_cost`]'s number.
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

    // Field indices mirrored from the protocol crate's own consts (private there): health 22,
    // power1 23, flags 46, aurastate 125, bytes_1 138.
    fn player(pairs: &[(u16, u32)]) -> ObjectStore {
        // `UNIT_FIELD_FLAGS` bit 3 (`UNIT_FLAG_PVP_ATTACKABLE`, behaviourally "player-controlled")
        // — every real player carries it, and `CanAttack 0x606980` selects which of its three
        // terminal arms to run on that bit for BOTH parties (1674). Without it the fixture takes
        // the NPC-vs-NPC arm, which wants a hostile reaction rather than a non-friendly one.
        let mut base = vec![(22u16, 100u32), (23, 500), (46, 1 << 3)];
        base.extend_from_slice(pairs);
        ObjectStore(ObjectFields::from_pairs(&base))
    }

    /// **The disarm ladder reaches the action bar**. The reference's own
    /// equipped-item test `0x5ea5d0` walks its three `GetWeapon` slots with `visFlag = 0`, so the
    /// hand `UNIT_FLAG_DISARMED` hides stops satisfying a weapon requirement — a disarmed
    /// warrior's Heroic Strike greys out and its tooltip requirement line turns red. Because the
    /// ladder hides exactly ONE weapon, main hand first, a dual-wielder is still armed enough.
    #[test]
    fn a_disarmed_hand_does_not_satisfy_the_equipped_item_requirement() {
        use crate::items::TestDeps;

        // `PLAYER_FIELD_INV_SLOT_HEAD + 2×slot` for equipment slots 15 and 16.
        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        const DISARMED: u32 = 0x0020_0000;

        // "Requires a melee weapon" — class 2, any subclass.
        let needs_a_weapon = SpellDisplay {
            equipped_item_class: 2,
            // Subclass 7 = Sword1H, the fixture's item. A **zero** mask would short-circuit the
            // whole search to "fits" — the reference's third gate — so it cannot be the fixture.
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

        // CONTROL — armed, the sword satisfies it.
        assert!(fits(false, &[(INV_MAINHAND, 0x2a)]));
        // Disarmed with only a main hand: the one weapon is hidden, nothing satisfies it.
        assert!(!fits(true, &[(INV_MAINHAND, 0x2a)]));
        // Disarmed dual-wielder: the main hand's claim cancels the off-hand gate, so the off-hand
        // weapon is still there and the ability stays usable.
        assert!(fits(true, &[(INV_MAINHAND, 0x2a), (INV_OFFHAND, 0x2b)]));
        // Disarmed with an off hand only: that is the hand the ladder hides.
        assert!(!fits(true, &[(INV_OFFHAND, 0x2b)]));
    }

    /// **The slot mask and its two rejects** — the rest of `0x5f0c50`, which the
    /// mislabelled census row had hidden behind a three-slot `GetWeapon` loop that does not exist.
    /// The search is equipment indices `0..=22`, narrowed by `AttributesEx3` to one hand when the
    /// ability names one, and a worn item that is DEPRECATED or genuinely BROKEN cannot satisfy it.
    #[test]
    fn the_equipped_item_search_masks_by_hand_and_rejects_broken_gear() {
        use crate::items::TestDeps;

        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        // `ITEM_FIELD_DURABILITY` / `_MAXDURABILITY` — both INSTANCE fields (46/47), which is
        // where the reference reads the pair from.
        const ITEM_DURABILITY: u16 = 46;
        const ITEM_MAX_DURABILITY: u16 = 47;

        // A spell requiring a class-2 weapon, with whatever `AttributesEx3` is passed.
        let spell = |ex3: u32| SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 7, // Sword1H — a zero mask short-circuits to "fits"
            attributes_ex3: ex3,
            ..Default::default()
        };
        // `hands`: per equipped weapon — (inv field, guid, template flags, instance max
        // durability, instance current durability).
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
        // MAIN-HAND-ONLY (`0x400`): the off-hand weapon no longer counts.
        assert!(fits(
            &spell(ATTR_EX3_MAIN_HAND_ONLY),
            &[sound(INV_MAINHAND)]
        ));
        assert!(!fits(
            &spell(ATTR_EX3_MAIN_HAND_ONLY),
            &[sound(INV_OFFHAND)]
        ));
        // OFF-HAND-ONLY (`0x1000000`): the mirror.
        assert!(fits(&spell(ATTR_EX3_OFF_HAND_ONLY), &[sound(INV_OFFHAND)]));
        assert!(!fits(
            &spell(ATTR_EX3_OFF_HAND_ONLY),
            &[sound(INV_MAINHAND)]
        ));

        // The two rejects: DEPRECATED, and broken (`MaxDurability > 0 && Durability == 0`).
        assert!(!fits(
            &spell(0),
            &[(INV_MAINHAND, 0x2a, ITEM_FLAG_DEPRECATED, 0, 0)]
        ));
        assert!(!fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 45, 0)]));
        // …and a merely damaged one still counts, as does one with no durability at all.
        assert!(fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 45, 12)]));
        assert!(fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 0, 0)]));
    }

    /// The mask and the disarm strip compose, and that composition is the Heroic Strike case: a
    /// main-hand-only ability whose one allowed bit is the hand the disarm hides has nothing left
    /// to match, whatever else is worn.
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

        // Dual-wielding, armed: both abilities are usable.
        assert!(fits(ATTR_EX3_MAIN_HAND_ONLY, false));
        assert!(fits(ATTR_EX3_OFF_HAND_ONLY, false));
        // Disarmed: the ladder hides the MAIN hand (main first), so the main-hand-only ability
        // has an empty mask and dies…
        assert!(!fits(ATTR_EX3_MAIN_HAND_ONLY, true));
        // …while the off-hand-only one is untouched, and so is a hand-agnostic one.
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

    /// **The behavioural half of the spell-modifier system, and the reason op 14 is the first op
    /// wired**: the press-path gate compares the caster's
    /// power against [`power_cost`] and, when short, refuses locally and sends **no**
    /// `CMSG_CAST_SPELL` at all (`0x609657`'s fall-through failure block). With the tables unread,
    /// a talent-discounted spell the server would happily accept is refused by the client.
    ///
    /// So: a mage with 80 mana and a 100-mana spell cannot cast it; one `SMSG_SET_PCT_SPELL_MODIFIER`
    /// for that spell's `(family bit, op 14)` cell flips both the number and the verdict.
    #[test]
    fn a_talent_cost_cut_reaches_the_cost_and_the_press_path_gate() {
        // Family 3 (mage), family bit 5 — the shape a real talent's `SpellFamilyMask` names.
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

        // The server's cell for (bit 5, op 14): −30%.
        mods.set(false, 5, OP_COST, -30);
        assert_eq!(power_cost(&d, &store, &mods), 70);
        assert!(
            can_afford(&d, &store, &mods),
            "the talent brings it inside the pool, so TryCast must send the cast"
        );

        // A warrior's copy of the same spell id takes nothing — the family gate, from this side.
        let other_class = SpellDisplay {
            spell_family: 4,
            ..d
        };
        assert_eq!(power_cost(&other_class, &store, &mods), 100);
    }

    /// Each modeled gate trips alone, and only the power leg raises notEnoughMana (`0x6e3feb`).
    #[test]
    fn gates_trip_independently_and_only_power_sets_oom() {
        let alive = player(&[]);
        let d = SpellDisplay::default();
        assert_eq!(walk(&d, &alive), (true, false));

        // Leg 1: dead — unusable, not oom; the attribute waives it.
        let dead = player(&[(22, 0)]);
        assert_eq!(walk(&d, &dead), (false, false));
        let while_dead = SpellDisplay {
            attributes: ATTR_CASTABLE_WHILE_DEAD,
            ..Default::default()
        };
        assert_eq!(walk(&while_dead, &dead), (true, false));

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

        // Leg 5: a finishing move with no combo points banked (field 1222 byte 1). Overpower's
        // own shape — no aura state anywhere, so this leg is the only thing that greys it.
        let overpower = SpellDisplay {
            attributes_ex: 0x4810_0200,
            ..Default::default()
        };
        assert!(overpower.needs_combo_points());
        assert_eq!(walk(&overpower, &alive), (false, false));
        let dodged = player(&[(1222, 0x05_03_01_01)]);
        assert_eq!(walk(&overpower, &dodged), (true, false));
        // The neighbouring bytes of that dword must not read as combo points.
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

        // Leg 12: power — the only oom.
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

    /// Leg 10 against a target store: the aura-state bit gates, and the enemy fork's CanAttack
    /// passes on the default-neutral reaction.
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
