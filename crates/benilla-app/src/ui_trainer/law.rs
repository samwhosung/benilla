//! The trainer window's icon, tooltip and group-key laws. They disagree on the same row as the
//! reference does: the icon falls back to the wire wrapper's art where the tooltip hops to the
//! taught spell, so about 806 shipped services show one spell's icon over another's tooltip. Do
//! not unify them.

use benilla_formats::{
    substitute, SkillLineCatalog, SpellCatalog, TokenContext, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
    SPELL_EFFECT_SKILL_STEP,
};
use benilla_protocol::messages::{trainer_spell_state, TrainerSpell};
use benilla_ui::script::{TrainerServiceCategory, TrainerTooltip, TRAINER_GROUP_KNOWN};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::ui_items::item_icon;

use super::{TRAINER_TYPE_MOUNT, TRAINER_TYPE_TRADESKILL};
use crate::query_cache::Ask;

/// `GetTrainerServiceIcon 0x4d8f50`: at a tradeskill trainer (`[0xb73a08]`), the item the wire
/// spell's first learn effect (`0x4d8ff5`/`0x4d8ffa`) teaches to create (`0x4d906a`); otherwise
/// the wire spell's own icon (`0x4d9008`), never the taught spell's (`0x4d8fd7`). An uncached
/// template reads `None` until its answer re-fires `TRAINER_UPDATE`, as the client's cache
/// callback does (`0x4d9140`).
pub(super) fn service_icon(
    wire_spell: u32,
    trainer_type: u32,
    spells: &SpellCatalog,
    icons: Option<&ItemDisplays>,
    items: &Items,
    ask: Ask<'_>,
) -> Option<String> {
    let wire = spells.get(wire_spell)?;
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        // The first learn slot wins; a zero trigger falls to the wire icon, not the next slot.
        let taught = wire
            .effects
            .iter()
            .position(|&e| e == SPELL_EFFECT_LEARN_SPELL || e == SPELL_EFFECT_LEARN_PET_SPELL)
            .map(|i| wire.effect_trigger_spell[i])
            .filter(|&t| t != 0);
        // The taught spell's product item, slot 0 only.
        if let Some(product) = taught
            .and_then(|t| spells.get(t))
            .map(|d| d.effect_item_type[0])
            .filter(|&e| e != 0)
        {
            return items
                .template(product, 0, ask)
                .map(|t| t.display_info_id)
                .and_then(|d| item_icon(icons, d));
        }
    }
    wire.icon.clone()
}

/// `SetTrainerService 0x5338b0` picks the subject a shared tooltip builder describes. An
/// unresolvable trigger moves to the next slot (`0x5339f0`), unlike the icon's first match;
/// `Attributes & 0x20` alone decides item or spell, as the spell builder `0x52e610` redirects on
/// the same bit (`0x52e6d2`), and `Effect[0] == 24` only picks the item's slot.
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
        // An unresolvable trigger moves to the next slot, not the fallback.
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

/// The `$`-token contexts a service description expands over, picked per row by the caster the
/// expander's level terms read (`0x6e3130`'s selector): the active player's, or the pet's, which
/// a `LEARN_PET_SPELL` wrapper selects.
pub(super) struct ServiceText<'a> {
    pub(super) player: &'a TokenContext<'a>,
    pub(super) pet: &'a TokenContext<'a>,
}

/// `GetTrainerServiceDescription 0x4d9b40`: the text the stock window shows under the service's
/// cost — `Spell.dbc`'s Description column with its `$`-tokens expanded (`0x5075f0` at
/// `0x4d9d6f`). Which record is expanded depends on the row: the wire spell's own text when it
/// has one (`0x4d9c32`), else the spell the first learn effect teaches — `EffectTriggerSpell` of
/// the first `LEARN_SPELL`/`LEARN_PET_SPELL` slot, whether or not its trigger resolves
/// (`0x4d9bdd`-`0x4d9c18`) — with that spell's own text, else, for a tradeskill recipe, the
/// description of the item the taught spell creates (`0x4d9cd0`-`0x4d9d22`, returned verbatim,
/// never expanded). A used row at a mount trainer skips its own text (`0x4d9c29`), and every
/// failure falls back to the wire text again (`0x4d9d3b`, probing slot 0 alone for the caster).
/// The expansion itself is the spell tooltip's description call (`0x52f717`): level override 0,
/// the Description column, and the modifiers applied. `None` is the nil the binding pushes
/// (`0x4d9d90`): the wire spell has no record (`0x4d9bd7`), or no arm found text and the wire
/// spell's own is null or empty (`0x4d9d44`, `0x4d9d49`).
pub(super) fn service_description(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    items: &Items,
    ask: Ask<'_>,
    text: &ServiceText,
) -> Option<String> {
    let d = spells.get(wire.spell)?;
    let learn = (0..3).find(|&i| {
        matches!(
            d.effects[i],
            SPELL_EFFECT_LEARN_SPELL | SPELL_EFFECT_LEARN_PET_SPELL
        )
    });
    let caster = |slot: usize| {
        if d.effects[slot] == SPELL_EFFECT_LEARN_PET_SPELL {
            text.pet
        } else {
            text.player
        }
    };
    let wire_text = d.description.as_deref().filter(|t| !t.is_empty());
    if !(trainer_type == TRAINER_TYPE_MOUNT && wire.state == trainer_spell_state::GRAY) {
        if let Some(t) = wire_text {
            return Some(substitute(t, d, learn.map_or(text.player, caster)));
        }
    }
    if let Some(slot) = learn {
        if let Some(taught) = spells.get(d.effect_trigger_spell[slot]) {
            if let Some(t) = taught.description.as_deref().filter(|t| !t.is_empty()) {
                return Some(substitute(t, taught, caster(slot)));
            }
            // The recipe's product carries the only text a `CREATE_ITEM` spell has.
            if taught.attributes & SPELL_ATTR_IS_TRADESKILL != 0
                && taught.effects[0] == SPELL_EFFECT_CREATE_ITEM
            {
                if let Some(t) = items
                    .template(taught.effect_item_type[0], 0, ask)
                    .map(|item| item.description.clone())
                    .filter(|t| !t.is_empty())
                {
                    return Some(t);
                }
            }
        }
    }
    wire_text.map(|t| substitute(t, d, caster(0)))
}

/// The list builder `0x4d7560`'s group key (`0x4d7786`). Type 2 resolves no skill line, so no row
/// drops: 1 when the wire spell steps a skill (`0x4d77b6`), else 2, labelled from the table at
/// `0x807520`. Type 1 puts a known service in the signed `-1` group (`0x4d77e8`). The rest key on
/// the taught spell's skill line (`0x4d7c60` → `0x60c920`), and 0 drops the service (`0x4d7807`).
pub(super) fn service_group(
    wire_spell: u32,
    taught: u32,
    trainer_type: u32,
    category: TrainerServiceCategory,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    // The VM's own `GlobalStrings.lua`, for the header labels.
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

/// A wire `state` byte's category; anything but GREEN or GRAY, an unexpected value too, is gated.
pub(super) fn category(state: u8) -> TrainerServiceCategory {
    match state {
        trainer_spell_state::GRAY => TrainerServiceCategory::Used,
        trainer_spell_state::GREEN => TrainerServiceCategory::Available,
        _ => TrainerServiceCategory::Unavailable,
    }
}
