//! The bar's click intents onto the wire, and the state a press latches locally: vmangos answers
//! no command or reaction press (`PetHandler.cpp:81-99`) and no autocast toggle
//! (`PetHandler.cpp:203-300`), and sends `SMSG_PET_MODE` only from `Pet::SetEnabled`
//! (`Pet.cpp:2362-2377`), so the client applies them itself until the next `SMSG_PET_SPELLS`.

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PET_ACT_COMMAND, PET_COMMAND_ATTACK, PET_COMMAND_FOLLOW, PET_COMMAND_STAY,
};

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::target::{is_possessed_by, Selection};
use crate::ui_action::Spells;

use super::bar::active_aura_press;
use super::{PetBar, PetUnit};

/// What a pet bar press reads and writes: `CastPetAction`, `PetAttack` and the other orders, and
/// the pet book's `CastSpell(id, "pet")`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetPress<'w, 's> {
    pub(crate) bar: ResMut<'w, PetBar>,
    pub(crate) selection: ResMut<'w, Selection>,
    pub(crate) commands: Res<'w, NetCommands>,
    pub(crate) pet: PetUnit<'w, 's>,
    pub(crate) spells: Option<Res<'w, Spells>>,
    /// The talent modifiers the pet GCD's op 21 goes through: the player's tables, which gate on
    /// the spell's family and never on the caster (`0x6e6b30`).
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    ui_errors: Res<'w, crate::ui_action::UiErrorKeys>,
    pick: crate::target::AttackPick<'w, 's>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
}

/// A press that passed everything ahead of its packet.
pub(crate) struct PetSend {
    pet_guid: u64,
    packed: u32,
    target_guid: u64,
}

/// A spell press the bar hands to the attempt with its unit as the caster, sending no
/// `CMSG_PET_ACTION` (`0x4bd378`-`0x4bd385`: `0x6e4b60(unit, spell, 0, target)`).
pub(crate) struct PetCast {
    spell_id: u32,
    caster: u64,
    target_guid: u64,
}

/// Where a press goes once it has passed the dispatcher's gates (`0x4bd1d0`).
pub(crate) enum PetStep {
    /// The bar's own packet, [`PetPress::finish_press`].
    Order(PetSend),
    /// The attempt, [`cast_pet_press`].
    Cast(PetCast),
}

/// The cast arm's hand-off (`0x4bd385`): the attempt with the bar's unit as its caster, at the
/// press's target, the selection as the press found it (`0x4bd212`). Its packet is
/// `CMSG_PET_CAST_SPELL`, its GCD the pet's ([`crate::spell::CastLadder::send_pet`]).
pub(crate) fn cast_pet_press(cast: &mut crate::spell::ScriptCast, press: PetCast) {
    let crate::spell::ScriptCast { targeting, ladder } = cast;
    let target = Some(press.target_guid).filter(|&g| g != 0);
    let Some(ctx) = targeting.context_for_caster(press.caster, target) else {
        debug!(
            "ui_pet: cast {} — the caster is not streamed",
            press.spell_id
        );
        return;
    };
    debug!(
        "ui_pet: cast {} through the attempt, caster {:#x}, target {:#x}",
        press.spell_id, press.caster, press.target_guid
    );
    ladder.send_pet(press.spell_id, &ctx, press.caster);
}

impl PetPress<'_, '_> {
    /// `CastPetAction(slot)`, a 1-based bar slot, for a press that ends in its packet; one the
    /// attempt takes needs the ladder, which the call host lends ([`cast_pet_press`]).
    #[cfg(test)]
    pub(crate) fn press_slot(&mut self, slot: u32) {
        match self.begin_slot(slot) {
            Some(PetStep::Order(send)) => self.finish_press(send),
            Some(PetStep::Cast(_)) => panic!("slot {slot} casts through the attempt"),
            None => {}
        }
    }

    /// [`Self::press_slot`]'s first half, up to the packet or the attempt. The attack validator
    /// in it (`0x612df0`) may hand a unit to a standing targeting cursor, whose cast goes out
    /// inside the validator, ahead of the packet: a caller that owns the cursor binds between the
    /// halves.
    pub(crate) fn begin_slot(&mut self, slot: u32) -> Option<PetStep> {
        let entry = slot_entry(&self.bar, slot)?;
        self.begin_press(slot, entry, None)
    }

    /// [`Self::order`]'s first half; see [`Self::begin_slot`].
    pub(crate) fn begin_order(&mut self, packed: u32) -> Option<PetStep> {
        self.begin_press(0, PetActionEntry::from(packed), None)
    }

    /// TryCast's attack pick for a press the bar handed the attempt (`0x6e4efb`:
    /// `0x612df0(unit, &guid)`), the validator with that unit as the actor and the press's target
    /// as its candidate: the guid the cast goes on at, or `None` once refused.
    pub(crate) fn pick_for_unit_cast(&mut self, press: crate::spell::UnitPress) -> Option<u64> {
        let actor = self.pet.store(press.caster);
        if crate::ui_action::attack_actor_refusal(actor, self.pet.self_guid.0, &self.ui_errors) {
            return None;
        }
        self.pick.target(
            press.target,
            true,
            &mut self.selection,
            &mut self.seam,
            &self.ui_errors,
        )
    }

    /// `StartAttack 0x6131a0` on a unit caster at its cast's target, TryCast's tail
    /// (`0x6e51de`): the attack validator with the unit as the actor (`0x6131aa`), then, for the
    /// pet bar's unit (`0x613200`-`0x613212`), the bar's Attack at the guid it settled
    /// (`0x613217` → `0x4bd630`, word `0x07000002`), through the dispatcher a press takes. Any
    /// other unit does nothing more.
    pub(crate) fn begin_attack_from_cast(&mut self, (caster, guid): (u64, u64)) -> Option<PetStep> {
        let guid = self.pick_for_unit_cast(crate::spell::UnitPress {
            caster,
            target: Some(guid),
        })?;
        if caster != self.bar.spells.pet_guid {
            return None;
        }
        let attack = PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24);
        self.begin_press(0, PetActionEntry::from(attack), Some(guid))
    }

    /// The second half of a press: its `CMSG_PET_ACTION`.
    pub(crate) fn finish_press(&mut self, send: PetSend) {
        let _ = self.commands.0.send(ClientCommand::PetAction {
            pet_guid: send.pet_guid,
            packed: send.packed,
            target_guid: send.target_guid,
        });
    }

    /// The guid the pet book's `CastSpell(id, "pet", onSelf)` aims at: the selection, or with
    /// `onSelf` the active player (`0x4b4345`). A zero guid falls back to the selection
    /// (`0x4b34af`), so a missing player leaves it.
    pub(crate) fn spell_target(&self, on_self: bool) -> u64 {
        let caster = if on_self { self.pet.self_guid.0 } else { None };
        caster.or(self.selection.guid).unwrap_or(0)
    }

    /// `PetAttack` and the other one-shot orders, as their slot's packed word. A slot press and
    /// an order are one press: both reach the reference's one dispatcher (`0x4bd1d0`). The slot
    /// number only labels logs.
    #[cfg(test)]
    pub(crate) fn order(&mut self, packed: u32) {
        if let Some(PetStep::Order(send)) = self.begin_order(packed) {
            self.finish_press(send);
        }
    }

    /// Send under [`PetBar`]'s pet guid, never one from the VM, so a press queued as the pet
    /// leaves dies here. `CMSG_PET_ACTION` echoes the slot's word, which the server dispatches on
    /// its type, with `passed` as the target, else the selection (`0x4bd1ff`-`0x4bd212`);
    /// `HandlePetAction` drops a target the spell does not want. The dispatcher's first act is the
    /// usability predicate (`0x4bd1f2`), and a zero answer leaves for the epilogue (`0x4bd1f9`)
    /// ahead of everything below it.
    fn begin_press(
        &mut self,
        slot: u32,
        entry: PetActionEntry,
        passed: Option<u64>,
    ) -> Option<PetStep> {
        let Self {
            bar,
            selection,
            commands,
            pet,
            spells,
            spell_mods,
            ui_errors,
            pick,
            seam,
        } = self;
        let Some(pet_store) = pet.usable_pet(bar) else {
            debug!("ui_pet: slot {slot} refused, the pet's actions are not usable — no packet");
            return None;
        };
        let pet_guid = bar.spells.pet_guid;
        let possessing = possessing(
            pet_guid,
            Some(pet_store),
            pet.player_store(),
            pet.self_guid.0,
        );
        // A press on a spell the pet is running cancels its aura and sends no `CMSG_PET_ACTION`
        // (`0x4bd240`-`0x4bd2ad`); nothing latches, the icon follows the pet's aura field.
        let display = entry
            .is_spell()
            .then(|| spells.as_ref().and_then(|s| s.catalog.get(entry.action())))
            .flatten();
        if let Some(spell_id) = active_aura_press(entry, Some(pet_store), display) {
            debug!("ui_pet: slot {slot} cancels its own aura (spell {spell_id}) — no PetAction");
            let _ = commands
                .0
                .send(ClientCommand::PetCancelAura { pet_guid, spell_id });
            return None;
        }
        // The spell arm leaves for the epilogue, before its GCD and its send, unless the slot's
        // `Spell.dbc` record resolves (`0x4bd2e7`-`0x4bd2fe`); an id past the table's maximum has
        // none. Its lookups of the active player and the pet (`0x4bd31a`-`0x4bd324`,
        // `0x4bd346`-`0x4bd34f`) repeat the predicate's, on the same guids and typemasks, and
        // cannot fail past it. Commands and reactions have no exit of their own (`0x4bd391`,
        // `0x4bd3a3`).
        let spell = if entry.is_spell() {
            let Some(spell) = display else {
                debug!(
                    "ui_pet: slot {slot} (spell {}) has no record — no packet",
                    entry.action()
                );
                return None;
            };
            Some(spell)
        } else {
            None
        };
        // Only Attack runs `0x612df0`, the attack validator with the pet as actor, whose target
        // pick can move the selection; every other press sends its target as is.
        let mut target_guid = passed.or(selection.guid).unwrap_or(0);
        // A spell with `AttributesEx4 & 0x20` (`0x4bd355`), or any spell of the unit the player
        // possesses (`0x4bd35e`-`0x4bd365`), goes to the attempt with that unit as its caster and
        // returns: no packet of the bar's, no GCD of this arm's, nothing latched.
        if spell.is_some_and(|d| d.allows_client_targeting() || possessing) {
            return Some(PetStep::Cast(PetCast {
                spell_id: entry.action(),
                caster: pet_guid,
                target_guid,
            }));
        }
        let refused = if is_attack_order(entry) {
            crate::ui_action::attack_actor_refusal(Some(pet_store), pet.self_guid.0, ui_errors)
                || match pick.target(passed, true, selection, seam, ui_errors) {
                    Some(guid) => {
                        target_guid = guid;
                        false
                    }
                    None => true,
                }
        } else {
            false
        };
        if !commit_press(bar, entry, refused, possessing) {
            debug!("ui_pet: slot {slot} refused by the attack validator — no packet");
            return None;
        }
        if let Some(spell) = spell {
            arm_pet_gcd(bar, entry.action(), spell, spell_mods, Instant::now());
        }
        debug!(
            "ui_pet: press slot {slot} (action {} kind {:#04x}) at {target_guid:#x}",
            entry.action(),
            entry.kind()
        );
        Some(PetStep::Order(PetSend {
            pet_guid,
            packed: entry.packed,
            target_guid,
        }))
    }
}

/// The spell arm's tail (`0x4bd367`-`0x4bd36e`) for a press past the usability gate whose
/// `Spell.dbc` record resolved and that neither cancels an aura nor takes the attempt: it calls
/// `StartGlobalCooldown 0x6e2de0(spellId, 1)` before the send (`0x4bd444`), the spell's own
/// `StartRecovery*` pair, under op 21, into the pet's list (`0xcecaec + 0x18`). The insert moves
/// the list's generation, which carries the flush that follows it (`0x6e2e77`, `0x6e2e8e`) to
/// `bar::fire_pet_cooldown_events`.
///
/// The pet's GCD starts in three places, and this arm is one. A `SMSG_SPELL_COOLDOWN` addressed
/// to the pet's guid inserts the same pair for every entry whose spell lacks `Attributes` bit 25
/// (`0x6e9553`-`0x6e9598`, `Cooldowns::apply_wire_cooldown`). The attempt's send starts it too
/// (`0x6e58fb`), with the list picked by the caster's charmed-by, else summoned-by, guid
/// (`0x6e58c2`-`0x6e58f3`), for the presses [`cast_pet_press`] takes; it owes them to the bar
/// ([`crate::spell::PetGcdsOwed`], [`arm_owed_pet_gcds`]). The `SMSG_SPELL_GO` pet leg inserts the
/// spell's own timers and no GCD (`0x6e85f7`).
pub(super) fn arm_pet_gcd(
    bar: &mut PetBar,
    spell_id: u32,
    spell: &benilla_formats::SpellDisplay,
    mods: &crate::spell::SpellModifiers,
    now: Instant,
) {
    bar.cooldowns.start_gcd(spell_id, spell, now, mods);
}

/// The GCDs the attempt's sends owe the pet's list (`0x6e58fb`), each at its send's instant, put
/// in before the bar's feed reads the list. A world with no spell plugin owes none.
pub(crate) fn arm_owed_pet_gcds(
    owed: Option<ResMut<crate::spell::PetGcdsOwed>>,
    mut bar: ResMut<PetBar>,
    spells: Option<Res<Spells>>,
    mods: Option<Res<crate::spell::SpellModifiers>>,
) {
    let (Some(mut owed), Some(mods)) = (owed, mods) else {
        return;
    };
    for (spell_id, at) in owed.take() {
        let Some(d) = spells.as_ref().and_then(|s| s.catalog.get(spell_id)) else {
            continue;
        };
        arm_pet_gcd(&mut bar, spell_id, d, &mods, at);
    }
}

/// Send the bar's other intents under [`PetBar`]'s pet guid: the autocast toggles, the stops and
/// the drag writes. A toggle asks the usability predicate first (`0x4bcbcf`), so on an unusable
/// pet it flips nothing and sends nothing. The stops do not (`0x4bd650`); the drag's writes were
/// gated where the VM applied them (`0x4bc9d0`), on the answer the bar last pushed.
pub(crate) fn drain_pet_actions(
    mut vm: crate::script_calls::VmWith<ResMut<'static, PetBar>>,
    commands: Res<NetCommands>,
    pet: PetUnit,
) {
    vm.turn(|script, mut bar| {
        let Some(script) = script else {
            return;
        };
        let toggles = script.take_pet_autocast_toggles();
        let stops = script.take_pet_stop_attacks();
        let writes = script.take_pet_set_actions();
        if toggles.is_empty() && stops == 0 && writes.is_empty() {
            return;
        }
        let pet_guid = bar.spells.pet_guid;
        if pet_guid == 0 {
            debug!("ui_pet: dropping queued pet intents — the bar is gone");
            return;
        }
        // The repaint is `latch_press`'s alone, not every click's: a refused `TogglePetAutocast`
        // (`0x4bcbf7`: any right-click on a token) signals nothing, so after `OnClick`'s
        // `SetChecked(0)` that button stays unlit until the next repaint, as in the reference.
        for slot in toggles {
            if !pet.actions_usable(&bar) {
                debug!(
                "ui_pet: autocast toggle on slot {slot} refused, the pet's actions are not usable"
            );
                continue;
            }
            let Some(flipped) = toggle_slot_autocast(&mut bar, slot) else {
                continue;
            };
            debug!(
                "ui_pet: autocast {} for spell {} (slot {slot})",
                flipped.autocast_on(),
                flipped.action()
            );
            let _ = commands.0.send(ClientCommand::PetSetAction {
                pet_guid,
                // Lua slots are 1-based, the wire's 0-based; `slot_entry` already refused 0.
                entries: vec![(slot - 1, flipped.packed)],
            });
        }
        for _ in 0..stops {
            if stop_pet_attack(&mut bar, &commands) {
                debug!("ui_pet: stop attack");
            }
        }
        // The drag's `(0-based position, word)` pairs: mirrored here and sent as one batch, since
        // the server tells one pair from two by body size.
        for entries in writes {
            for &(position, packed) in &entries {
                if let Some(e) = bar.spells.bar.get_mut(position as usize) {
                    *e = PetActionEntry::from(packed);
                }
            }
            debug!("ui_pet: bar write {entries:?}");
            let _ = commands
                .0
                .send(ClientCommand::PetSetAction { pet_guid, entries });
        }
    })
}

/// `TogglePetAutocast`'s local half (`0x4bcbb0`): flip bit 30 in place and return the whole word
/// to send (`0x4bcbff`/`0x4bcc17`), or `None` for a slot out of range or without bit 31
/// (`0x4bcbf1`). `0x4bcc19` then calls `0x4bd190`, which copies a spell slot's word over the last
/// spellbook entry equal to it under `& 0x3FFFFFFF`.
pub(super) fn toggle_slot_autocast(bar: &mut PetBar, slot: u32) -> Option<PetActionEntry> {
    let entry = slot_entry(bar, slot).filter(|e| e.autocast_allowed())?;
    let flipped = entry.with_autocast(!entry.autocast_on());
    *slot_entry_mut(bar, slot)? = flipped;
    if flipped.kind() == 1 {
        let key = flipped.packed & 0x3FFF_FFFF;
        if let Some(book) = bar
            .spells
            .spells
            .iter_mut()
            .rev()
            .find(|w| w.packed & 0x3FFF_FFFF == key)
        {
            *book = flipped;
        }
    }
    Some(flipped)
}

/// `PetStopAttack`'s core (`0x4bd650`): send `CMSG_PET_STOP_ATTACK` and lower the latch, or do
/// nothing when it is already down (`0x4bd65e`). Returns whether it sent.
pub(super) fn stop_pet_attack(bar: &mut PetBar, commands: &NetCommands) -> bool {
    let pet_guid = bar.spells.pet_guid;
    if !bar.attacking || pet_guid == 0 {
        return false;
    }
    let _ = commands.0.send(ClientCommand::PetStopAttack { pet_guid });
    bar.attacking = false;
    true
}

/// `0x493910`'s entry gate: the old-target clear runs only when a selection existed (`0x493937`)
/// and is being replaced or dropped (`0x493949`/`0x493951`).
pub(super) fn old_target_cleared(previous: Option<u64>, now: Option<u64>) -> bool {
    previous.is_some() && previous != now
}

/// The old-target clear's `PetStopAttack` (`0x493910`, at `0x493a18` when `0x5ee5a0` finds a
/// possessed unit at `0x493a0f`). It sits before the notify branch, so every selection writer runs
/// it; a re-select runs no clear (`0x493540`).
pub(crate) fn pet_stop_on_old_target_clear(
    selection: Res<Selection>,
    mut previous: Local<Option<u64>>,
    mut bar: ResMut<PetBar>,
    commands: Res<NetCommands>,
) {
    let now = selection.guid;
    if *previous == now {
        return;
    }
    let cleared = old_target_cleared(*previous, now);
    *previous = now;
    if cleared && stop_pet_attack(&mut bar, &commands) {
        debug!("ui_pet: the old-target clear called the pet off (0x493a18)");
    }
}

/// Whether the bar's unit is `0x5ee5a0`'s answer, which a spell press takes to the attempt
/// (`0x4bd35e`-`0x4bd365`) and an Attack press needs to latch (`0x4bd420`-`0x4bd42e`):
/// `0x5ee5a0` reads the player's far-sight object, `PLAYER_FARSIGHT` (`[[player+0xe68]+0x830]`,
/// `0x5ee5e9`), as a unit (`0x5ee60c`), and answers it only with `UNIT_FLAG_POSSESSED`
/// (`0x5ee626`) and charmed-by, else created-by, the player (`0x5ee62f`-`0x5ee64b`),
/// [`is_possessed_by`]. vmangos sets the flag only with possession (Mind Control, Eyes of the
/// Beast, an Eye of Kilrogg), and moves the player's view to the unit with it (`Camera::SetView`).
/// The function also needs far sight's own bit on (`[player+0x1c70] & 1`, `0x5ee5bc`), which no
/// state here keeps; the field naming the unit stands for it.
pub(super) fn possessing(
    unit_guid: u64,
    unit: Option<&ObjectStore>,
    player: Option<&ObjectStore>,
    me: Option<u64>,
) -> bool {
    unit_guid != 0
        && player.and_then(|p| p.0.player_farsight()) == Some(unit_guid)
        && is_possessed_by(unit, me)
}

/// The Attack order, the one press validated before it sends: the type-7 arm branches only on
/// `action <= 1` and `action == 2`, so every other command sends as it is.
pub(super) fn is_attack_order(entry: PetActionEntry) -> bool {
    entry.kind() == benilla_protocol::messages::PET_ACT_COMMAND
        && entry.action() == PET_COMMAND_ATTACK
}

/// Apply a press and answer whether it is sent. A veto from the validator (`0x4bd40d`, the pet as
/// actor) jumps past the send (`0x4bd414 je 0x4bd4c6`): no packet, no latch, no repaint. Failing
/// the possession compare after it (`0x4bd420`) costs only the latch (`0x4bd427` to the send).
pub(super) fn commit_press(
    bar: &mut PetBar,
    entry: PetActionEntry,
    refused: bool,
    possessing: bool,
) -> bool {
    if refused {
        return false;
    }
    latch_press(bar, entry, possessing);
    true
}

/// Latch a press's state, the half the server never confirms, with the client's masks. A
/// reaction writes byte 0 (`0x4bc94c`). Stay and Follow, the only commands to reach the write
/// (`action <= 1` at `0x4bd3b1`), set bits 8-15 and keep byte 0 and bit 27, which only the server
/// writes (`0x4bc96f`). Both are the VM's latch mirrored: the verb wrote the word and signalled
/// `PET_BAR_UPDATE` in place (`0x4bc956`, `0x4bc979`). Attack raises a possessed unit's attack
/// latch instead (`0x4bd42e`), behind the validator only this side runs, so its signal
/// (`0x4bd438`) is the feed's.
pub(super) fn latch_press(bar: &mut PetBar, entry: PetActionEntry, possessing: bool) {
    let action = entry.action();
    match entry.kind() {
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_STAY || action == PET_COMMAND_FOLLOW =>
        {
            bar.spells.state = (bar.spells.state & 0x0800_00FF) | (action << 8);
        }
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_ATTACK && possessing =>
        {
            // `0x4bd42e` signals too; an ordinary pet never gets here.
            bar.attacking = true;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_REACTION => {
            bar.spells.state = (bar.spells.state & 0xFFFF_FF00) | action;
        }
        // Dismiss, spells and unknown types reach the send (`0x4bd444`) without a signal.
        _ => {}
    }
}

/// The word held for a 1-based Lua slot.
pub(super) fn slot_entry(bar: &PetBar, slot: u32) -> Option<PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get(index).copied()
}

pub(super) fn slot_entry_mut(bar: &mut PetBar, slot: u32) -> Option<&mut PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get_mut(index)
}
