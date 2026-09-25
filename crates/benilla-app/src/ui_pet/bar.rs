//! The pet action bar's feed: the ten packed words of the last `SMSG_PET_SPELLS`, which the
//! server owns whole, rendered as ten `PetActionView`s each frame.

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PetSpells, PET_COMMAND_ATTACK, PET_COMMAND_DISMISS, PET_COMMAND_FOLLOW,
    PET_COMMAND_STAY, PET_REACT_AGGRESSIVE, PET_REACT_DEFENSIVE, PET_REACT_PASSIVE,
};
use benilla_ui::script::{PetActionView, UiScript};

use crate::net::{GuidIndex, ObjectStore};
use crate::ui_action::Spells;

use super::drain::UNIT_FLAG_POSSESSED;
use super::PetBar;

/// `GetPetActionsUsable()` (`0x4bcf70`): false while the state's bit 27 is set or the pet is
/// stunned, confused or fleeing. Its other four steps test ownership, which a held bar implies.
pub(super) fn actions_usable(bar: &PetBar, pet_flags: Option<u32>) -> bool {
    !bar.spells.bar_disabled()
        && pet_flags.is_none_or(|f| f & benilla_protocol::messages::PET_UNUSABLE_UNIT_FLAGS == 0)
}

/// What the feed last pushed, so `PET_BAR_UPDATE` fires on a change; the key's leading `u32` is
/// [`PetBar::bar_signals`], so a press that changes nothing still repaints.
#[derive(Default)]
pub(super) struct PetBarMemory {
    pushed: Option<(u32, bool, bool, bool, Vec<PetActionView>)>,
    /// The ten cooldown triples as last pushed: `PET_BAR_UPDATE_COOLDOWN`'s edge.
    cooldowns: Vec<Option<(i64, u32, bool)>>,
}

/// A command token's `(name, texture)` pair, both global names (`PetActionBarFrame.lua:98-104`):
/// keys from `GlobalStrings.lua:3029-3032`, textures from `PetActionBarFrame.lua:6-12`.
pub(super) fn command_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_COMMAND_STAY => ("PET_ACTION_WAIT", "PET_WAIT_TEXTURE"),
        PET_COMMAND_FOLLOW => ("PET_ACTION_FOLLOW", "PET_FOLLOW_TEXTURE"),
        PET_COMMAND_ATTACK => ("PET_ACTION_ATTACK", "PET_ATTACK_TEXTURE"),
        PET_COMMAND_DISMISS => ("PET_ACTION_DISMISS", "PET_DISMISS_TEXTURE"),
        _ => return None,
    })
}

/// A reaction token's pair: the `PET_MODE_*` keys (`GlobalStrings.lua:3045-3047`), naming the
/// pet's state, not the menu's imperative `PET_AGGRESSIVE` family, though enUS reads them alike.
pub(super) fn reaction_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_REACT_PASSIVE => ("PET_MODE_PASSIVE", "PET_PASSIVE_TEXTURE"),
        PET_REACT_DEFENSIVE => ("PET_MODE_DEFENSIVE", "PET_DEFENSIVE_TEXTURE"),
        PET_REACT_AGGRESSIVE => ("PET_MODE_AGGRESSIVE", "PET_AGGRESSIVE_TEXTURE"),
        _ => return None,
    })
}

/// One packed slot word as the bar draws it. `showing_active` is [`active_aura_press`]'s answer,
/// the one call (`0x4bcea0`) that both `GetPetActionInfo` (`0x4bdd2f`) and `CastPetAction`
/// (`0x4bd24a`) make, so the button showing active art is the one whose press cancels.
pub(super) fn slot_view(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    PetActionView {
        // Every slot carries its raw word, empty ones too: the drag's relocation candidate is
        // vmangos's unused slot, `ACT_DISABLED` with spell 0 (type 1, low 16 bits zero).
        packed: entry.packed,
        passive: spell.is_some_and(|s| s.passive),
        ..slot_paint(entry, bar, spell, cooldown, pet_attacking, showing_active)
    }
}

/// [`slot_view`]'s painted half: everything but the raw `packed` and `passive`.
pub(super) fn slot_paint(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    let kind = entry.kind();
    let action = entry.action();

    if let Some((name, texture)) = (kind == benilla_protocol::messages::PET_ACT_COMMAND)
        .then(|| command_token(action))
        .flatten()
    {
        // Lit when the unmasked `state >> 8` equals the action, so a disabled bar lights none, or
        // for Attack alone on the attack latch (`0x4bdf01`-`0x4bdf22`). `attack_active` is
        // `IsPetAttackActive`, that latch on this slot (`0x4be138`-`0x4be153`).
        let attacking = pet_attacking && action == PET_COMMAND_ATTACK;
        return PetActionView {
            name: Some(name.to_string()),
            texture: Some(texture.to_string()),
            is_token: true,
            active: bar.command_state() == action || attacking,
            attack_active: attacking,
            ..Default::default()
        };
    }

    if let Some((name, texture)) = (kind == benilla_protocol::messages::PET_ACT_REACTION)
        .then(|| reaction_token(action))
        .flatten()
    {
        // A disabled bar reads as Passive (`0x4bde3c`).
        let showing = if bar.bar_disabled() {
            benilla_protocol::messages::PET_REACT_PASSIVE
        } else {
            bar.react_state()
        };
        return PetActionView {
            name: Some(name.to_string()),
            texture: Some(texture.to_string()),
            is_token: true,
            active: showing == action,
            ..Default::default()
        };
    }

    // A zero word is empty (the client tests the dword); vmangos's unused slots are spell 0, which
    // has no record and takes the next exit, as in the client.
    if !entry.is_spell() || entry.is_empty() {
        return PetActionView::default();
    }
    let Some(spell) = spell else {
        // No `Spell.dbc` record, or no catalog: the reference returns nil and the button hides.
        return PetActionView::default();
    };
    PetActionView {
        name: Some(spell.name.clone()),
        subtext: spell.rank.clone(),
        // A running spell draws its `ActiveIconID` (`0x4bdd2f`/`0x4bdd38`/`0x4bdd77`), with no
        // fallback: the reference pushes nil when that icon does not resolve (`0x4bdd50`).
        texture: if showing_active {
            spell.active_icon.clone()
        } else {
            spell.icon.clone()
        },
        is_token: false,
        spell_id: Some(action),
        // Nil on every spell path (`0x4bdd5e`): `isActive` is a token's, `autoCast*` a spell's
        // (`0x4bdc50`).
        active: false,
        // Bits 31/30, not the type (`0x4bdd65`/`0x4bdda4`), gated on the record checked above.
        autocast_allowed: entry.autocast_allowed(),
        autocast_enabled: entry.autocast_on(),
        attack_active: false,
        cooldown,
        ..Default::default()
    }
}

/// The spell id when the slot shows active: `0x4bcea0`, the pet-side twin of `0x4e55f0`
/// ([`crate::ui_action::toggle::active_action_toggle`]). It tests `ActiveIconID != 0` first
/// (`0x4bcefd`), so a press on a spell without one re-casts rather than cancels.
pub(super) fn active_aura_press(
    entry: PetActionEntry,
    pet: Option<&ObjectStore>,
    spell: Option<&benilla_formats::SpellDisplay>,
) -> Option<u32> {
    if !entry.is_spell() || entry.is_empty() {
        return None;
    }
    let spell_id = entry.action();
    crate::ui_action::toggle::active_action_toggle(spell_id, spell?, pet?).then_some(spell_id)
}

/// Push the ten slot views on a change: `PET_BAR_UPDATE`, or `PET_BAR_UPDATE_COOLDOWN` when only
/// cooldowns moved, the reference's fire from the pet's cooldown bank (`0x6e2e8e`).
pub(super) fn feed_pet_bar(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    spells: Option<Res<Spells>>,
    clock: Res<crate::ui_script::UiClock>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut memory: Local<crate::ui_script::VmMemo<PetBarMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    let now = Instant::now();
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    let has_bar = bar.has_bar();
    // An unstreamed pet leaves usability to bit 27 alone, and no slot shows active.
    let pet_store = index
        .0
        .get(&bar.spells.pet_guid)
        .and_then(|&e| stores.get(e).ok());
    let pet_flags = pet_store.map(|s| s.0.unit_flags());
    let usable = actions_usable(&bar, pet_flags);
    // `PickupPetAction`'s gate alone (`0x4be1c1`): a possessed unit's bar cannot be rearranged,
    // but its buttons work, so possession stays out of `usable`.
    let pickup_allowed = pet_flags.unwrap_or(0) & UNIT_FLAG_POSSESSED == 0;
    let pet_attacking = bar.attacking;

    let fresh: Vec<PetActionView> = if has_bar {
        bar.spells
            .bar
            .iter()
            .map(|&entry| {
                let display = entry
                    .is_spell()
                    .then(|| spells.as_ref().and_then(|s| s.catalog.get(entry.action())))
                    .flatten();
                let cooldown = display.and_then(|d| {
                    bar.cooldowns
                        .info(entry.action(), 0, Some(d), now)
                        .ui_triple(anchor, ui_now)
                });
                slot_view(
                    entry,
                    &bar.spells,
                    display,
                    cooldown,
                    pet_attacking,
                    active_aura_press(entry, pet_store, display).is_some(),
                )
            })
            .collect()
    } else {
        Vec::new()
    };

    // `bar_signals` in the key repaints a press that moved nothing (`0x4bc940`/`0x4bc960`); the
    // cooldown triples are keyed apart, their edge being the bank's.
    let cooldowns: Vec<Option<(i64, u32, bool)>> = fresh.iter().map(|s| s.cooldown).collect();
    let content: Vec<PetActionView> = fresh
        .iter()
        .cloned()
        .map(|mut s| {
            s.cooldown = None;
            s
        })
        .collect();
    let key = (bar.bar_signals, has_bar, usable, pickup_allowed, content);
    let bar_changed = memory.pushed.as_ref() != Some(&key);
    let cooldowns_changed = memory.cooldowns != cooldowns;
    if bar_changed || cooldowns_changed {
        script.set_pet_actions(has_bar, usable, pickup_allowed, fresh);
        memory.cooldowns = cooldowns;
    }
    if bar_changed {
        debug!(
            "ui_pet: bar {} ({} occupied slot(s), {}{})",
            if key.1 { "shown" } else { "hidden" },
            key.4.iter().filter(|s| s.name.is_some()).count(),
            if key.2 { "usable" } else { "disabled" },
            if key.3 { "" } else { ", possessed" },
        );
        memory.pushed = Some(key);
        script.fire_event("PET_BAR_UPDATE", vec![]);
    } else if cooldowns_changed {
        script.fire_event("PET_BAR_UPDATE_COOLDOWN", vec![]);
    }
}
