//! The pet action bar's **feed** — ten packed words in, ten `PetActionView`s out, once a frame.
//!
//! **The shape of this system is the stance bar's mirror image, and that is the thing to hold on
//! to.** `crate::ui_shapeshift` derives its whole bar locally, from the known-spell set × the
//! `Spell.dbc` catalog, and the server never has an opinion about it. Here the server owns the
//! bar's CONTENTS — which slots exist and what is in them — and hands them over whole in
//! `SMSG_PET_SPELLS`. So the feed does no admission and no ordering: it *renders* the ten words
//! the last packet delivered. The slot law it renders them by is [`super`]'s.

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

/// `GetPetActionsUsable()` — may the bar be used at all (the predicate `0x4bcf70`).
///
/// benilla's earlier reading — "the enabled-flags byte's `0x8`" — was REFUTED as the
/// *whole* answer, but it survives as one of the seven steps: the client tests bit 27 of the state
/// dword, which is that same byte's `0x8` (see `PET_STATE_BAR_DISABLED`). The step it was missing
/// is the pet's own crowd-control state — a **stunned, confused or feared** pet cannot be ordered,
/// and its bar greys until it recovers.
///
/// The remaining four steps are ownership identity (the player resolves, the pet resolves, the pet
/// is ours, we are not ourselves charmed). Holding a bar at all already means the server named us
/// this pet's controller, so they are structurally true here; the two that can actually change
/// while a bar is on screen are the two we test.
///
/// This is the same predicate that gates whether a press does anything, so a false answer greys
/// the icons *and* is the honest reason a click would be ignored.
pub(super) fn actions_usable(bar: &PetBar, pet_flags: Option<u32>) -> bool {
    !bar.spells.bar_disabled()
        && pet_flags.is_none_or(|f| f & benilla_protocol::messages::PET_UNUSABLE_UNIT_FLAGS == 0)
}

/// What the feed last pushed, so `PET_BAR_UPDATE` fires on a real change rather than every frame
/// (the [`crate::ui_shapeshift`] memory pattern).
///
/// The leading `u32` is [`PetBar::bar_signals`], and it is what keeps the dedup from swallowing a
/// press that changes nothing — see that field for why the reference cannot afford to skip one.
#[derive(Default)]
pub(super) struct PetBarMemory {
    pushed: Option<(u32, bool, bool, bool, Vec<PetActionView>)>,
    /// The ten slots' cooldown triples as last pushed — `PET_BAR_UPDATE_COOLDOWN`'s own edge.
    cooldowns: Vec<Option<(i64, u32, bool)>>,
}

/// The command tokens' `(GlobalStrings key, texture-global name)` pair.
///
/// Both are returned as **the names of globals**, not values — the reference's token convention.
/// The name keys ship in `GlobalStrings.lua` (`PET_ACTION_ATTACK` = "Attack" at l.3029-3032); the
/// texture globals are declared by `PetActionBar.xml` — our own code, under the names and with the
/// art paths the reference's `PetActionBarFrame.lua:6-12` specifies (1260).
pub(super) fn command_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_COMMAND_STAY => ("PET_ACTION_WAIT", "PET_WAIT_TEXTURE"),
        PET_COMMAND_FOLLOW => ("PET_ACTION_FOLLOW", "PET_FOLLOW_TEXTURE"),
        PET_COMMAND_ATTACK => ("PET_ACTION_ATTACK", "PET_ATTACK_TEXTURE"),
        PET_COMMAND_DISMISS => ("PET_ACTION_DISMISS", "PET_DISMISS_TEXTURE"),
        _ => return None,
    })
}

/// The reaction tokens' pair, same convention. `PET_MODE_*` ship in `GlobalStrings.lua`
/// (l.3045-3047) — deliberately the `PET_MODE_*` keys, which name the state the pet is IN, not the
/// `PET_AGGRESSIVE`/`PET_DEFENSIVE`/`PET_PASSIVE` keys, which are the right-click menu's
/// imperatives. Both read identically in enUS ("Aggressive"), so only a localized client can tell
/// them apart — which is exactly why the key matters rather than the string.
pub(super) fn reaction_token(action: u32) -> Option<(&'static str, &'static str)> {
    Some(match action {
        PET_REACT_PASSIVE => ("PET_MODE_PASSIVE", "PET_PASSIVE_TEXTURE"),
        PET_REACT_DEFENSIVE => ("PET_MODE_DEFENSIVE", "PET_DEFENSIVE_TEXTURE"),
        PET_REACT_AGGRESSIVE => ("PET_MODE_AGGRESSIVE", "PET_AGGRESSIVE_TEXTURE"),
        _ => return None,
    })
}

/// Resolve one packed slot word into what the bar draws.
///
/// `cooldown` and `showing_active` are passed in rather than looked up here so the whole function
/// stays a pure (state, word) → view mapping, which is what the tests below exercise.
/// `showing_active` is [`active_aura_press`]'s predicate — the *same* answer that decides whether
/// a click cancels, because in the reference it is literally the same call (`0x4bcea0`, reached
/// from `GetPetActionInfo` at `0x4bdd2f` and from `CastPetAction` at `0x4bd24a`). Computing it
/// once and handing it to both is what keeps the icon honest: the button that shows the active art
/// is exactly the button whose next press takes the aura off.
pub(super) fn slot_view(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
    showing_active: bool,
) -> PetActionView {
    PetActionView {
        // The raw word rides EVERY slot, including the ones that draw as empty — decision 1010's
        // drag is word arithmetic and reads it. Zeroing an "empty" slot here would be wrong on the
        // wire *and* wrong in the drop core: vmangos fills its unused slots with `ACT_DISABLED` +
        // spell id 0, and that shape (type 1, low 16 zero) is precisely the relocation candidate
        // the core hunts for.
        packed: entry.packed,
        // Only a resolved spell can be passive; a token has no record and an unresolvable id has
        // no answer, and `false` is the honest reading of both.
        passive: spell.is_some_and(|s| s.passive),
        ..slot_paint(entry, bar, spell, cooldown, pet_attacking, showing_active)
    }
}

/// [`slot_view`]'s painted half — everything the button draws, with no wire word in it. Split so
/// the drag's two raw fields are stamped in exactly one place rather than on each of four returns.
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
        // A command token lights on `(state >> 8) == action` **or** on the attack latch — read at
        // the bytes at `0x4bdf01`-`0x4bdf22`, and both halves matter:
        //
        // - the compare is against the UNMASKED `state >> 8` (`PetSpells::command_state`'s own
        //   note), so a disabled bar puts every command button out;
        // - ATTACK gets the extra clause — and it is the *only* thing that can light ATTACK,
        //   because the command byte is never written for it (`0x4bc960`).
        //
        // So whether Attack ever appears lit is entirely a question about [`PetBar::attacking`],
        // and the answer for a pet bar is **never**: that latch is the possess bar's, gated on
        // `0x5ee5a0` ([`possessing`]). This expression stays faithful rather than hard-coding the
        // `false`, because under Mind Control or Eyes of the Beast the same button does light.
        //
        // `attack_active` — `IsPetAttackActive`, the click fork — is the same latch narrowed to
        // this slot (`0x4be138`-`0x4be153`: type 7, action 2, and the flag). Same input, so the
        // button that lights is exactly the button whose next press calls the unit off.
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
        // The reaction compare's left side is forced to Passive when the bar is disabled
        // (`0x4bde3c`): a pet that cannot be ordered reads as Passive rather than
        // keeping the mode light it had, which is the honest thing for it to say.
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

    // A spell slot. `is_empty` is the zero WORD (the client tests the dword); vmangos's own unused
    // middle slots are not zero and arrive here instead, where their spell id 0 misses the catalog
    // and takes the same exit — the client's own route to the same empty button.
    if !entry.is_spell() || entry.is_empty() {
        return PetActionView::default();
    }
    let Some(spell) = spell else {
        // The catalog failed to load, or the server named a spell 5875's DBC does not have. Draw
        // the slot as occupied but nameless rather than inventing a name: the button then hides,
        // which is honest, and the alternative (a "?" with no tooltip) has fooled nobody.
        return PetActionView::default();
    };
    PetActionView {
        name: Some(spell.name.clone()),
        subtext: spell.rank.clone(),
        // THE ICON SWAP (decision 1007, `0x4bdd2f`/`0x4bdd38`/`0x4bdd77`): a spell the
        // pet is currently running draws its record's `ActiveIconID` instead of its `SpellIconID`.
        // Falling back to `icon` here would be wrong — the reference looks up whichever id the
        // predicate chose and pushes **nil** if that lookup fails (`0x4bdd50`), so an unresolvable
        // active icon hides the button rather than showing the inactive art on an active spell.
        // `active_icon` is `None` on exactly that failure, so `.clone()` already says it.
        texture: if showing_active {
            spell.active_icon.clone()
        } else {
            spell.icon.clone()
        },
        is_token: false,
        spell_id: Some(action),
        // A spell slot NEVER reports isActive — nil on every path (pushed at `0x4bdd5e`), which
        // retires 0982's INTERIM. `isActive` is exclusively a token concept and `autoCast*`
        // exclusively a spell one; the two halves of the signature never overlap (`0x4bdc50`).
        //
        // "The pet is running this spell" is expressed by the icon above, not by this flag — which
        // is why 0988's hole closes without this line changing.
        active: false,
        // Autocast is bits 31/30 of the word, not the type byte (`0x4bdd65`/`0x4bdda4`) — and both
        // are additionally gated on the spell resolving in `Spell.dbc`, which the early return
        // above has already enforced by the time we get here.
        autocast_allowed: entry.autocast_allowed(),
        autocast_enabled: entry.autocast_on(),
        attack_active: false,
        cooldown,
        // `packed`/`passive` are [`slot_view`]'s to stamp — this half paints, it does not encode.
        ..Default::default()
    }
}

/// The pet spell slot that is **showing active** — the reference's `0x4bcea0`, returning the spell
/// id when it holds so the one answer can drive both of its consumers. Decision 1007.
///
/// It is not a new predicate: `0x4bcea0` is the *pet-side compiled twin* of the player's
/// `0x4e55f0`, which we already carry as [`crate::ui_action::toggle::active_action_toggle`] — same
/// three tests (nonzero raw `ActiveIconID`, the spell's own id in a live `UNIT_FIELD_AURA` slot,
/// that slot's `AURAFLAGS` nibble bit 0), different unit. So this reaches for it rather than
/// restating it, and the pet's store goes in where the player's does.
///
/// **The `ActiveIconID != 0` gate is load-bearing on the send, not just the icon.** Because the
/// binary tests it first (`0x4bcefd`) and `CastPetAction` takes its cancel arm on the whole
/// predicate, a pet spell whose record carries no active icon can never be clicked off — it
/// re-casts instead. That reads like an oversight and isn't ours to fix: the same `0` is what
/// tells the bar there is no "active" art to show, so the two halves are consistent.
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

/// Rebuild the ten slot views each frame and diff-push them, firing `PET_BAR_UPDATE` on a change
/// of the bar and `PET_BAR_UPDATE_COOLDOWN` on a change of its cooldowns alone.
///
/// The reference fires `PET_BAR_UPDATE` from nine sites, each right after a state change, and
/// `PET_BAR_UPDATE_COOLDOWN` from the cooldown subsystem when the PET bank mutates (`0x6e2e8e`).
/// Diffing the pushed state is the same edge from the other side: the slots' content and usability
/// are the bar's, the ten triples are the bank's. `UNIT_PET` is `feed_pet_unit`'s;
/// `UNIT_FLAGS`/`UNIT_AURA` for `"pet"` are the unit feed's.
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
    // The pet's own descriptor. `None` = we hold a bar for a unit whose descriptor has not arrived
    // (or has left); the usability predicate then rests on bit 27 alone rather than greying a bar
    // on missing data, and no slot can read as showing-active.
    let pet_store = index
        .0
        .get(&bar.spells.pet_guid)
        .and_then(|&e| stores.get(e).ok());
    let pet_flags = pet_store.map(|s| s.0.unit_flags());
    let usable = actions_usable(&bar, pet_flags);
    // `PickupPetAction`'s own gate, and nobody else's (`0x4be1c1`): a POSSESSED unit's bar cannot
    // be rearranged. Deliberately not folded into `usable` — the reference keeps possession out of
    // the flags that grey the bar, because a possessed unit is exactly when the buttons must work.
    // Absent flags read as not-possessed, matching `usable`'s own missing-data posture.
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

    // `bar.bar_signals` rides the key so a press the state does not move still repaints — the
    // `0x4bc940`/`0x4bc960` signal, which the button's own `SetChecked(0)` makes mandatory. The
    // cooldown triples are keyed apart: their edge is the bank's, not the bar's.
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
