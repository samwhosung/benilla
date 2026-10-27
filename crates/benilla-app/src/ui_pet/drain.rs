//! The bar's presses onto the wire, and the state a press latches locally: vmangos answers no
//! command or reaction press (`PetHandler.cpp:81-99`) and no autocast toggle
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
use super::{PetAttackLatch, PetBar, PetUnit};

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
    /// The press wrote the state word, which signals `PET_BAR_UPDATE` ahead of the send
    /// (`0x4bc956`, `0x4bc979`).
    signals: bool,
}

impl PetSend {
    /// Whether the press wrote the state word ([`latch_press`]), so its listeners run before the
    /// packet.
    pub(crate) fn signals(&self) -> bool {
        self.signals
    }
}

/// A spell press the bar hands to the attempt with its unit as the caster, sending no
/// `CMSG_PET_ACTION` (`0x4bd378`-`0x4bd385`: `0x6e4b60(unit, spell, 0, target)`).
pub(crate) struct PetCast {
    spell_id: u32,
    caster: u64,
    target_guid: u64,
}

/// An Attack press past the dispatcher's gates, at its validator (`0x4bd40d`): the pick
/// ([`PetPress::pick_attack`]), whose `SetSelection` signals inside it, then the rest
/// ([`PetPress::finish_attack`]).
pub(crate) struct PetAttack {
    slot: u32,
    entry: PetActionEntry,
    passed: Option<u64>,
    pet_guid: u64,
}

/// Where a press goes once it has passed the dispatcher's gates (`0x4bd1d0`).
pub(crate) enum PetStep {
    /// The bar's own packet, [`PetPress::finish_press`].
    Order(PetSend),
    /// The attempt, [`cast_pet_press`].
    Cast(PetCast),
    /// Attack, at its validator.
    Attack(PetAttack),
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
        match self.begin_slot(slot).and_then(|step| self.attack_now(step)) {
            Some(PetStep::Order(send)) => self.finish_press(send),
            Some(PetStep::Cast(_)) => panic!("slot {slot} casts through the attempt"),
            Some(PetStep::Attack(_)) | None => {}
        }
    }

    /// An Attack step's validator and rest run at once, for a press with no listener to run at
    /// its pick's signals.
    #[cfg(test)]
    fn attack_now(&mut self, step: PetStep) -> Option<PetStep> {
        let PetStep::Attack(attack) = step else {
            return Some(step);
        };
        let owed = self.pick_attack(&attack)?;
        owed.pay(&mut self.selection, &mut self.seam);
        self.finish_attack(attack)
            .map(|(send, _)| PetStep::Order(send))
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
    /// as its candidate, up to the selection's re-read: what its pick's commit owes after its
    /// signals, or `None` once an actor check refused. [`Self::settle_pick`] gives the guid.
    pub(crate) fn pick_for_unit_cast(
        &mut self,
        press: crate::spell::UnitPress,
    ) -> Option<crate::target::Owed> {
        let actor = self.pet.store(press.caster);
        if crate::ui_action::attack_actor_refusal(actor, self.pet.self_guid.0, &self.ui_errors) {
            return None;
        }
        Some(
            self.pick
                .pick(press.target, true, &mut self.selection, &mut self.seam),
        )
    }

    /// The validator from the selection's re-read on (`0x6130c1`): the guid the attempt or the
    /// attack goes on at, or `None` once refused.
    pub(crate) fn settle_pick(&mut self) -> Option<u64> {
        self.pick.settle(&self.selection, &self.ui_errors)
    }

    /// `StartAttack 0x6131a0` on a unit caster at its cast's target, TryCast's tail
    /// (`0x6e51de`), past its validator ([`Self::pick_for_unit_cast`] with the unit as the
    /// actor, `0x6131aa`): for the pet bar's unit (`0x613200`-`0x613212`), the bar's Attack at the
    /// guid it settled (`0x613217` → `0x4bd630`, word `0x07000002`), through the dispatcher a
    /// press takes. Any other unit does nothing more.
    pub(crate) fn attack_from_cast(&mut self, caster: u64, guid: u64) -> Option<PetStep> {
        if caster != self.bar.spells.pet_guid {
            return None;
        }
        let attack = PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24);
        self.begin_press(0, PetActionEntry::from(attack), Some(guid))
    }

    /// Attack's validator with the pet as its actor (`0x4bd40d`), up to the selection's re-read:
    /// the actor checks, then the pick, what its commit owes; `None` once an actor check refused,
    /// which jumps past the send (`0x4bd414`).
    pub(crate) fn pick_attack(&mut self, attack: &PetAttack) -> Option<crate::target::Owed> {
        let actor = self.pet.store(attack.pet_guid);
        if crate::ui_action::attack_actor_refusal(actor, self.pet.self_guid.0, &self.ui_errors) {
            debug!(
                "ui_pet: slot {} refused by the attack validator — no packet",
                attack.slot
            );
            return None;
        }
        Some(
            self.pick
                .pick(attack.passed, true, &mut self.selection, &mut self.seam),
        )
    }

    /// Attack past its pick's signals: the validator's re-read and final gate (`0x6130c1`), the
    /// possessed unit's latch (`0x4bd420`-`0x4bd42e`), and the packet at the target it settled,
    /// with whether the latch was raised. Possession is read past the validator, as `0x4bd420`
    /// reads it.
    pub(crate) fn finish_attack(&mut self, attack: PetAttack) -> Option<(PetSend, bool)> {
        let target = self.settle_pick();
        let possessing = possessing(
            attack.pet_guid,
            self.pet.store(attack.pet_guid),
            self.pet.player_store(),
            self.pet.self_guid.0,
        );
        if !commit_press(
            &mut self.bar,
            self.seam.pet_latch.as_deref_mut(),
            attack.entry,
            target.is_none(),
            possessing,
        ) {
            debug!(
                "ui_pet: slot {} refused by the attack validator — no packet",
                attack.slot
            );
            return None;
        }
        let target_guid = target.unwrap_or(0);
        debug!(
            "ui_pet: press slot {} (attack) at {target_guid:#x}",
            attack.slot
        );
        let send = PetSend {
            pet_guid: attack.pet_guid,
            packed: attack.entry.packed,
            target_guid,
            signals: false,
        };
        Some((send, possessing))
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
        let step = self
            .begin_order(packed)
            .and_then(|step| self.attack_now(step));
        if let Some(PetStep::Order(send)) = step {
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
            seam,
            ..
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
        // Every press but Attack sends its target as is.
        let target_guid = passed.or(selection.guid).unwrap_or(0);
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
        // Only Attack runs `0x612df0`, the attack validator with the pet as actor, whose target
        // pick can move the selection and signals there.
        if is_attack_order(entry) {
            return Some(PetStep::Attack(PetAttack {
                slot,
                entry,
                passed,
                pet_guid,
            }));
        }
        let signals = latch_press(bar, seam.pet_latch.as_deref_mut(), entry, possessing);
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
            signals,
        }))
    }
}

/// The spell arm's tail (`0x4bd367`-`0x4bd36e`) for a press past the usability gate whose
/// `Spell.dbc` record resolved and that neither cancels an aura nor takes the attempt: it calls
/// `StartGlobalCooldown 0x6e2de0(spellId, 1)` before the send (`0x4bd444`), the spell's own
/// `StartRecovery*` pair, under op 21, into the pet's list (`0xcecaec + 0x18`), and the flush
/// that follows it (`0x6e2e77`, `0x6e2e8e`), at its place on the pet's cooldown lane.
///
/// The pet's GCD starts in three places, and this arm is one. A `SMSG_SPELL_COOLDOWN` addressed
/// to the pet's guid inserts the same pair for every entry whose spell lacks `Attributes` bit 25
/// (`0x6e9553`-`0x6e9598`, `Cooldowns::apply_wire_cooldown`). The attempt's send starts it too
/// (`0x6e58fb`), with the list picked by the caster's charmed-by, else summoned-by, guid
/// (`0x6e58c2`-`0x6e58f3`), for the presses [`cast_pet_press`] takes: inside the send, through this
/// arm. The `SMSG_SPELL_GO` pet leg inserts the spell's own timers and no GCD (`0x6e85f7`).
pub(crate) fn arm_pet_gcd(
    bar: &mut PetBar,
    spell_id: u32,
    spell: &benilla_formats::SpellDisplay,
    mods: &crate::spell::SpellModifiers,
    now: Instant,
) {
    bar.cooldowns.start_gcd(spell_id, spell, now, mods);
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

/// `PetStopAttack`'s core (`0x4bd650`): send `CMSG_PET_STOP_ATTACK` for the bar's guid and lower
/// the latch (`0x4bd662`-`0x4bd6ae`), or do nothing when it is already down (`0x4bd65e`). Returns
/// whether it sent; the caller signals `PET_BAR_UPDATE` (`0x4bd6b4`).
pub(super) fn stop_pet_attack(latch: &mut PetAttackLatch, commands: &NetCommands) -> bool {
    let Some(pet_guid) = latch.0.take() else {
        return false;
    };
    let _ = commands.0.send(ClientCommand::PetStopAttack { pet_guid });
    true
}

/// The selection's release `0x493910` with the player object held: `0x5ee5a0` asked of the player
/// (`0x493a0f`), and with a unit answered, `PetStopAttack` (`0x4bd650` at `0x493a18`). `farsight`
/// is the player's `PLAYER_FARSIGHT` and `unit` that unit's descriptor; whether it sent.
pub(crate) fn call_pet_off(
    latch: &mut PetAttackLatch,
    commands: &NetCommands,
    farsight: Option<u64>,
    unit: Option<&ObjectStore>,
    player: Option<&ObjectStore>,
    me: Option<u64>,
) -> bool {
    let Some(unit_guid) = farsight else {
        return false;
    };
    if !possessing(unit_guid, unit, player, me) {
        return false;
    }
    let sent = stop_pet_attack(latch, commands);
    if sent {
        debug!("ui_pet: the selection's release called the possessed unit off (0x493a18)");
    }
    sent
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
    latch: Option<&mut PetAttackLatch>,
    entry: PetActionEntry,
    refused: bool,
    possessing: bool,
) -> bool {
    if refused {
        return false;
    }
    latch_press(bar, latch, entry, possessing);
    true
}

/// Latch a press's state, the half the server never confirms, with the client's masks, and
/// whether it wrote the state word. A reaction writes byte 0 (`0x4bc94c`). Stay and Follow, the
/// only commands to reach the write (`action <= 1` at `0x4bd3b1`), set bits 8-15 and keep byte 0
/// and bit 27, which only the server writes (`0x4bc96f`). Each write signals `PET_BAR_UPDATE`
/// (`0x4bc956`, `0x4bc979`), which the call host fires ahead of the send. Attack raises a
/// possessed unit's attack latch instead (`0x4bd42e`), behind the validator only this side runs;
/// the call host signals it (`0x4bd438`) before the send.
pub(super) fn latch_press(
    bar: &mut PetBar,
    latch: Option<&mut PetAttackLatch>,
    entry: PetActionEntry,
    possessing: bool,
) -> bool {
    let action = entry.action();
    match entry.kind() {
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_STAY || action == PET_COMMAND_FOLLOW =>
        {
            bar.spells.state = (bar.spells.state & 0x0800_00FF) | (action << 8);
            true
        }
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_ATTACK && possessing =>
        {
            // An ordinary pet never gets here.
            if let Some(latch) = latch {
                latch.0 = Some(bar.spells.pet_guid);
            }
            false
        }
        benilla_protocol::messages::PET_ACT_REACTION => {
            bar.spells.state = (bar.spells.state & 0xFFFF_FF00) | action;
            true
        }
        // Dismiss, spells and unknown types reach the send (`0x4bd444`) without a signal.
        _ => false,
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
