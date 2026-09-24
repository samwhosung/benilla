//! The trainer window's **byte-transcribed laws** — the icon, the tooltip, and the group key.
//!
//! They live together because they are the same *kind* of thing (a direct transcription of one
//! binding or builder branch each, with the binary's addresses in the doc), and apart from the feed
//! because they are the part that must never drift toward "what seems consistent". The whole point
//! of putting them side by side is that they **disagree with each other on the same row**, on
//! purpose: the icon needs a trainer-type gate the tooltip does not have; the icon pins the *wire*
//! wrapper where the tooltip hops to the *taught* spell; the group key hops at three trainer types
//! and resolves nothing at the fourth. On ~806 of the shipped corpus's trainer services the
//! reference client visibly shows one spell's icon above another spell's tooltip. Keeping them
//! adjacent makes that deliberate, and makes a future "unification" obviously wrong rather than
//! obviously tidy — decision 1124 is the second time a plausible unification (the display name,
//! assumed to hop like the group key) turned out to be refuted at the bytes.
//!
//! Sources: `GetTrainerServiceIcon 0x4d8f50` (the icon), `SetTrainerService 0x5338b0` (the tooltip)
//! and the list builder `0x4d7560` (the group key).

use benilla_formats::{
    SkillLineCatalog, SpellCatalog, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL, SPELL_EFFECT_SKILL_STEP,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::{TrainerServiceCategory, TrainerTooltip, TRAINER_GROUP_KNOWN};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::NetCommands;
use crate::ui_items::item_icon;

use super::{TRAINER_TYPE_MOUNT, TRAINER_TYPE_TRADESKILL};

/// The trainer's **icon law** — `GetTrainerServiceIcon 0x4d8f50` (the binding is not a marshal,
/// the entire resolution is inlined there). Three gates, then a fallback:
///
/// 1. **The trainer type is 2** (tradeskill/profession — `[0xb73a08]`, the trainer-list packet's
///    type dword stored verbatim). A class/mount/pet trainer never substitutes.
/// 2. **The WIRE spell has a learn-wrapper effect** in any of its three slots —
///    `SPELL_EFFECT_LEARN_SPELL` or `SPELL_EFFECT_LEARN_PET_SPELL` (`0x4d8ff5`/`0x4d8ffa`). The
///    **first** matching slot wins; its `EffectTriggerSpell` is the taught spell.
/// 3. **That taught spell creates an item** (`EffectItemType[0] != 0`, `0x4d906a`) → the created
///    item's `ItemDisplayInfo` icon.
///
/// Otherwise: the **WIRE (wrapper) spell's own `SpellIconID`** (`0x4d9008`). This is the fact
/// benilla had backwards — `esi` is pinned to the wire record at `0x4d8fd7` and is never reassigned
/// on any path reaching the fallback, so **the client never paints the taught spell's own icon at a
/// trainer**. We used to, which is what put a blue `Spell_Shadow_SealOfKings` crown on Copper
/// Shortsword: spell 2756 (the wrapper the server sends) hops to 2739 (the recipe), whose own
/// `SpellIconID` is that crown — while the law wants item 2847's sword.
///
/// A gate-3 hit whose item template is not cached yet returns `None` (the client pushes Lua `nil`
/// and repaints on the cache callback, `0x4d9140` → `TRAINER_UPDATE`). Our equivalent is free: the
/// template answer changes the snapshot, so [`feed_trainer`]'s diff re-fires `TRAINER_UPDATE` on its
/// own.
pub(super) fn service_icon(
    wire_spell: u32,
    trainer_type: u32,
    spells: &SpellCatalog,
    icons: Option<&ItemDisplays>,
    items: &Items,
    commands: &NetCommands,
) -> Option<String> {
    let wire = spells.get(wire_spell)?;
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        // Gate 2: the first learn-wrapper slot wins — a miss (or a zero trigger) falls straight
        // through to the wire icon rather than scanning on, exactly as the loop's `je` does.
        let taught = wire
            .effects
            .iter()
            .position(|&e| e == SPELL_EFFECT_LEARN_SPELL || e == SPELL_EFFECT_LEARN_PET_SPELL)
            .map(|i| wire.effect_trigger_spell[i])
            .filter(|&t| t != 0);
        // Gate 3: the taught spell's product item, slot 0 only.
        if let Some(product) = taught
            .and_then(|t| spells.get(t))
            .map(|d| d.effect_item_type[0])
            .filter(|&e| e != 0)
        {
            return items
                .template(product, 0, commands)
                .map(|t| t.display_info_id)
                .and_then(|d| item_icon(icons, d));
        }
    }
    wire.icon.clone()
}

/// The trainer's **tooltip law** — `SetTrainerService 0x5338b0`. It is a *selector*, not a
/// renderer: the binding emits no tooltip line of its own (verified negative — none of the four
/// AddLine helpers appears in its extent) and hands one of the two shared builders a subject. That
/// is why this returns a [`TrainerTooltip`] rather than any text.
///
/// ```text
/// for i in 0..3:
///     if WIRE.Effect[i] in {36 LEARN_SPELL, 57 LEARN_PET_SPELL}:
///         t = WIRE.EffectTriggerSpell[i]
///         if t resolves to a Spell.dbc row:              # else: NEXT SLOT, not abandon
///             if TAUGHT.Attributes & 0x20:               # the ITEM route
///                 return Item(TAUGHT.EffectItemType[ TAUGHT.Effect[0]==24 ? i : 0 ])
///             return Spell(t, altCaster = WIRE.Effect[i] == 57)
/// return Spell(WIRE.id, altCaster = false)               # the only path describing the wrapper
/// ```
///
/// Three things a re-implementation gets wrong by default, all of them ours to get right here:
///
/// - **The scan is stricter than the icon's.** A slot whose `EffectTriggerSpell[i]` is out of range
///   or resolves to a NULL row advances to the *next* slot (`jl`/`jg` → `0x5339f0`); the icon
///   binding's first match wins outright. So the two can pick different slots on the same row.
/// - **`Effect[0] == 24` is NOT the item-vs-spell decision** — `Attributes & 0x20` alone is. The
///   effect test only picks *which slot* the item id comes from, because the spell builder
///   `0x52e610` re-applies the same Attributes bit itself at `0x52e6d2` and redirects to the item
///   builder with `EffectItemType[0]`. A client that treats `Effect[0]==24` as the gate renders a
///   spell tooltip where the reference renders an item one. (Divergence population on the shipped
///   `Spell.dbc`: 1161 spells carry the bit, 1159 with `Effect[0]==24`; the two that don't — 2479
///   and 7920 — have `EffectItemType` all-zero, so both routes end at an empty tooltip.)
/// - **It hops where the icon pins.** [`service_icon`] describes the WIRE wrapper on its fallback;
///   this describes the TAUGHT spell on every path that resolves one, and has no trainer-type gate
///   at all. On ~806 shipped services that means one spell's icon over another spell's tooltip —
///   the reference client's own behaviour, not a bug to reconcile.
pub(super) fn service_tooltip(wire_spell: u32, spells: &SpellCatalog) -> TrainerTooltip {
    let wire_only = TrainerTooltip::Spell {
        spell_id: wire_spell,
        alt_caster: false,
    };
    let Some(wire) = spells.get(wire_spell) else {
        return wire_only;
    };
    for i in 0..3 {
        let effect = wire.effects[i];
        if effect != SPELL_EFFECT_LEARN_SPELL && effect != SPELL_EFFECT_LEARN_PET_SPELL {
            continue;
        }
        let trigger = wire.effect_trigger_spell[i];
        // Unresolvable trigger → the NEXT slot, not the fallback (the binding's `jl`/`jg`).
        let Some(taught) = spells.get(trigger) else {
            continue;
        };
        if taught.attributes & SPELL_ATTR_IS_TRADESKILL != 0 {
            let slot = if taught.effects[0] == SPELL_EFFECT_CREATE_ITEM {
                i
            } else {
                0
            };
            return TrainerTooltip::Item(taught.effect_item_type[slot]);
        }
        return TrainerTooltip::Spell {
            spell_id: trigger,
            alt_caster: effect == SPELL_EFFECT_LEARN_PET_SPELL,
        };
    }
    wire_only
}

/// The **group-key law** — the builder `0x4d7560`'s per-trainer-type branch at `0x4d7786`
/// (decision 1124). It is the third law on this page that forks on the same dword the other two
/// do, and the one that decides what the director actually sees at the top of a profession
/// trainer's list.
///
/// **Type 2 (tradeskill) does not resolve a skill line at all.** The key defaults to `2` and becomes
/// `1` iff the **WIRE** spell's own `Spell.dbc Effect[0..2]` contains `44 SKILL_STEP` (`0x4d77b6`) —
/// no `EffectTriggerSpell` deref, no `SkillLineAbility` lookup. Two consequences that a skill-line
/// implementation gets wrong even when its ordering is right: the partition is **total**, so no row
/// is ever dropped at a tradeskill trainer; and the header vocabulary is the client's own label table
/// (`0x807520 + key * 0x40`), whose two entries are the global strings `TRADESKILL_SERVICE_STEP`
/// and `TRADESKILL_SERVICE_LEARN` — **keys**, read off the player's own table at the feed rather
/// than re-typed here (decision 2045); a header the install cannot name comes back empty, which is
/// the reference's own data-suppression face.
///
/// **Type 1** (mount — the client's "talent") is a *hybrid*, not a third predicate: an
/// already-known service (`state == 2`) goes to the signed **`-1`** group (`0x4d77e8`) under the
/// `KNOWN_TALENTS_HEADER` global string; everything else takes the same skill-line path as
/// types 0/3.
///
/// **Types 0/3 (and type 1's remainder)** key on the **TAUGHT** spell's `SkillLine`
/// (`0x4d7c60`→`0x60c920`, decision 0247 — the one hop in this file that survived 1124's audit); an
/// unresolved `0` drops the service (`0x4d7807`). Pet (3) is not special-cased anywhere.
pub(super) fn service_group(
    wire_spell: u32,
    taught: u32,
    trainer_type: u32,
    category: TrainerServiceCategory,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    // The VM's own `GlobalStrings.lua` — the two label-table entries and the known-talents header.
    get: &dyn Fn(&str) -> Option<String>,
) -> (u32, String) {
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        let step = spells
            .get(wire_spell)
            .is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SKILL_STEP));
        return if step {
            (1, get("TRADESKILL_SERVICE_STEP").unwrap_or_default())
        } else {
            (2, get("TRADESKILL_SERVICE_LEARN").unwrap_or_default())
        };
    }
    if trainer_type == TRAINER_TYPE_MOUNT && category == TrainerServiceCategory::Used {
        return (
            TRAINER_GROUP_KNOWN,
            get("KNOWN_TALENTS_HEADER").unwrap_or_default(),
        );
    }
    let line = skill_lines
        .and_then(|c| c.spell_to_line(taught))
        .unwrap_or(0);
    if line == 0 {
        return (0, String::new());
    }
    let name = skill_lines
        .and_then(|c| c.line(line))
        .map(|l| l.name.clone())
        .unwrap_or_else(|| format!("Skill {line}"));
    (line, name)
}

/// The green/red/gray colour a wire `state` byte maps to (decision 0237): GRAY → known, GREEN →
/// learnable, everything else (RED + any unexpected value) → gated.
pub(super) fn category(state: u8) -> TrainerServiceCategory {
    match state {
        trainer_spell_state::GRAY => TrainerServiceCategory::Used,
        trainer_spell_state::GREEN => TrainerServiceCategory::Available,
        _ => TrainerServiceCategory::Unavailable,
    }
}
