//! The bar's click **intents** → the wire, and the state a press latches locally.
//!
//! **The bar's STATE — the lit command, the lit reaction, the autocast bit — is applied LOCALLY,
//! and that is forced rather than chosen.** The server answers none of those three presses:
//! `HandlePetAction`'s command and reaction arms send nothing back,
//! `HandlePetSpellAutocastOpcode` sets its bits and returns, and `SMSG_PET_MODE` is emitted from
//! exactly one place in the whole server (`Pet::SetEnabled` — the enabled flag, nothing else). A
//! client that waited for confirmation would show a bar whose reaction buttons never light and
//! whose autocast ring never appears. Verified live against vmangos, 2026-08-05: a
//! `TogglePetAutocast` and a Follow press drew no reply packet at all. [`drain_pet_actions`]
//! carries that half, and the next `SMSG_PET_SPELLS` re-authorities it.

use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PET_COMMAND_ATTACK, PET_COMMAND_FOLLOW, PET_COMMAND_STAY,
};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::target::Selection;
use crate::ui_action::Spells;

use super::bar::active_aura_press;
use super::{PetBar, PetUnit};

/// `UNIT_FIELD_FLAGS` bit 24 — `UNIT_FLAG_POSSESSED` (vmangos `UnitDefines.h:515`), read by the
/// reference as the descriptor byte `[[pet+0x110]+0xA3] & 1`. Gates the pet bar's **drag** and
/// nothing else: a possessed unit's buttons still work, its layout is just not
/// yours to rearrange.
pub(super) const UNIT_FLAG_POSSESSED: u32 = 0x0100_0000;

/// Drain the bar's three intents onto the wire.
///
/// Every one carries the pet's guid from [`PetBar`] — never a guid the VM supplied — so an intent
/// queued in the frame a pet was dismissed dies here rather than naming a unit that is gone.
///
/// `CMSG_PET_ACTION` echoes the slot's **own packed word**: the server re-splits it and dispatches
/// on the type byte, so command, reaction and cast all leave through this one send. The target is
/// our current selection, which is what makes "select a mob, press Attack" work; a slot that wants
/// no target simply has the server ignore it (`HandlePetAction`'s `explicitlySelectedTarget` fork
/// drops a target the spell does not want).
///
/// **This drain is also where the bar's STATE moves, and that is not an optimisation — it is
/// forced.** The server answers none of these three: `HandlePetAction`'s command and reaction arms
/// send nothing back, and `HandlePetSpellAutocastOpcode` sets its bits and returns
/// (`PetHandler.cpp:451-478`). `SMSG_PET_MODE` is emitted from exactly ONE place in the whole
/// server — `Pet::SetEnabled`, i.e. only when the bar is enabled or disabled
/// (`Pet.cpp:2362-2377`). So a client that waited for confirmation would show a pet bar whose
/// reaction buttons never light and whose autocast ring never appears; the reference plainly does
/// not, so it must apply all three locally, and so do we. Verified live, 2026-08-05: a
/// `TogglePetAutocast` + a Follow press against vmangos drew no packet whatsoever in reply.
///
/// The optimism is bounded by the same packet that owns everything else — the next
/// `SMSG_PET_SPELLS` (a re-summon, a learn, a stable swap) replaces state and contents together.
pub(super) fn drain_pet_actions(
    script: Option<NonSendMut<UiScript>>,
    mut bar: ResMut<PetBar>,
    mut selection: ResMut<Selection>,
    commands: Res<NetCommands>,
    pet: PetUnit,
    spells: Option<Res<Spells>>,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    scan: crate::target::TargetScan,
    mut seam: crate::creature_anim::AttackSeam,
) {
    let Some(mut script) = script else {
        return;
    };
    let pressed = script.take_pet_actions();
    let orders = script.take_pet_orders();
    let toggles = script.take_pet_autocast_toggles();
    let stops = script.take_pet_stop_attacks();
    let writes = script.take_pet_set_actions();
    if pressed.is_empty()
        && orders.is_empty()
        && toggles.is_empty()
        && stops == 0
        && writes.is_empty()
    {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        debug!("ui_pet: dropping queued pet intents — the bar is gone");
        return;
    }
    // NOTE, so it is not "fixed" again (decision 1030 reverting 1027): the repaint is
    // [`latch_press`]'s alone, and it is deliberately NOT unconditional on the click.
    // `PetActionButton_OnClick`'s `this:SetChecked(0)` runs for every click, but the reference only
    // signals `PET_BAR_UPDATE` from the writes that changed something — so a **refused**
    // `TogglePetAutocast` (`0x4bcbf7`: a token is not autocastable, which is every right-click on
    // Follow, Stay or a reaction) leaves that button's ring off until the next repaint from
    // anywhere. That is the reference's own behaviour, checked there by the director, and
    // reproduced.
    //
    // The pet's own descriptor — the ATTACK arm's validator reads it, and so does the spell arm's
    // showing-active test. Neither is about the bar; both are about the pet.
    let pet_store = pet.store(pet_guid);
    // `0x4bd420`'s compare, hoisted once: is the bar's unit the one we are *driving*? Only then may
    // an ATTACK press latch (see [`possessing`]) — which for an ordinary pet is never.
    let possessing = possessing(pet_store, pet.self_guid.0);

    // A pressed slot and a one-shot order (`PetAttack` & co., 1958) are the same press: the
    // slot's own packed word, or the word the verb synthesized — both re-enter the reference's
    // one dispatcher (`0x4bd1d0`), so they share one loop. The slot number only ever labelled logs.
    let presses: Vec<(u32, PetActionEntry)> = pressed
        .into_iter()
        .filter_map(|slot| slot_entry(&bar, slot).map(|e| (slot, e)))
        .chain(
            orders
                .into_iter()
                .map(|packed| (0, PetActionEntry::from(packed))),
        )
        .collect();
    for (slot, entry) in presses {
        // The spell arm's early exit (`0x4bd240`–`0x4bd2ad`): a press on a spell the
        // pet is already running takes the aura OFF and **returns** — `CMSG_PET_ACTION` never
        // leaves, so it is a cancel, not a re-cast. Nothing is latched locally either: the icon
        // goes back when the pet's `UNIT_FIELD_AURA` says the aura is gone, which is the honest
        // order (the server can refuse — a dead pet gets `FEEDBACK_PET_DEAD` instead).
        let display = entry
            .is_spell()
            .then(|| spells.as_ref().and_then(|s| s.catalog.get(entry.action())))
            .flatten();
        if let Some(spell_id) = active_aura_press(entry, pet_store, display) {
            debug!("ui_pet: slot {slot} cancels its own aura (spell {spell_id}) — no PetAction");
            let _ = commands
                .0
                .send(ClientCommand::PetCancelAura { pet_guid, spell_id });
            continue;
        }
        // `0x612df0`, the ATTACK order's validator, and its actor is the **pet**. Phase A is the
        // pet's own eligibility; Phase B is the target, and it can move your selection — see
        // [`crate::target::attack_order_target`], which is the whole of "press Attack at nothing
        // and the pet goes for whatever is in front of you".
        //
        // Every other arm sends the selection untouched: `CastPetAction` substitutes it for a nil
        // Lua argument at `0x4bd212` and only the ATTACK arm ever calls the validator, so a Follow
        // or a spell carries whatever you happened to have.
        let mut target_guid = selection.guid.unwrap_or(0);
        let refused = if is_attack_order(entry) {
            crate::ui_action::attack_actor_refusal(pet_store, pet.self_guid.0, &mut ui_errors)
                || match crate::target::attack_order_target(
                    &scan,
                    &mut selection,
                    &mut seam,
                    &mut ui_errors,
                ) {
                    Some(guid) => {
                        target_guid = guid;
                        false
                    }
                    None => true,
                }
        } else {
            false
        };
        if !commit_press(&mut bar, entry, refused, possessing) {
            debug!("ui_pet: slot {slot} refused by the attack validator — no packet");
            continue;
        }
        debug!(
            "ui_pet: press slot {slot} (action {} kind {:#04x}) at {target_guid:#x}",
            entry.action(),
            entry.kind()
        );
        let _ = commands.0.send(ClientCommand::PetAction {
            pet_guid,
            packed: entry.packed,
            target_guid,
        });
    }
    for slot in toggles {
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
            // The wire's slot positions are 0-based (vmangos bounds them `< 10`); the Lua's are
            // 1-based. This subtraction is the whole of the conversion and cannot underflow —
            // `slot_entry` already rejected 0.
            entries: vec![(slot - 1, flipped.packed)],
        });
    }
    for _ in 0..stops {
        if stop_pet_attack(&mut bar, &commands) {
            debug!("ui_pet: stop attack");
        }
    }
    // The drag's writes. The engine ran the assign core against its own mirror and
    // handed back the `(0-based position, word)` pairs; the authoritative ten words live *here*, so
    // the app's whole job is to mirror each pair and put the batch on the wire **whole** — the
    // server tells the one-pair form from the two-pair form by body size, so a relocation and its
    // write must not be split into two sends.
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
}

/// `TogglePetAutocast`'s local half, `0x4bcbb0` — flip the slot's autocast bit and write it back,
/// returning the new word for the send, or `None` when the reference no-ops silently (a slot out
/// of range, or a word that is not autocast-ALLOWED, bit 31 — `0x4bcbf1`).
///
/// The client flips bit 30 in the slot word IN PLACE and sends the whole new word — it is not a
/// "set autocast to X for spell Y" verb (`0x4bcbff`/`0x4bcc17`). The server reads the
/// direction back out of the type byte it arrives in.
///
/// Then `0x4bcc19` calls `0x4bd190(&bar[slot])` — the propagation into the **spellbook**: for a
/// spell-type slot (type 1), scan the raw pet-spell array **backwards** for the entry equal to the
/// slot under `& 0x3FFFFFFF` and copy the slot's FULL word into it. The pet book renders from that
/// array, so without the copy a bar toggle left the Pet tab's ring stale until the next
/// `SMSG_PET_SPELLS`. The book→bar direction is `ui_pet_book::flip_autocast`.
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

/// `PetStopAttack`'s **core**, `0x4bd650` — call the pet off, and the only thing besides a new pet
/// that puts the Attack button out.
///
/// It **no-ops entirely when the latch is down** (`0x4bd65e`): no packet, no repaint. That gate
/// lives here rather than in the VM because the latch does. Returns whether it actually fired,
/// which is what its two callers log.
///
/// Split out of the drain because the drain is *not* its only caller: [`pet_stop_on_old_target_clear`]
/// is the second, exactly as `0x4bd650` has a second call site of its own.
pub(super) fn stop_pet_attack(bar: &mut PetBar, commands: &NetCommands) -> bool {
    let pet_guid = bar.spells.pet_guid;
    if !bar.attacking || pet_guid == 0 {
        return false;
    }
    let _ = commands.0.send(ClientCommand::PetStopAttack { pet_guid });
    bar.attacking = false;
    true
}

/// `0x493910`'s entry gate, as a predicate on the selection transition.
///
/// The old-target clear returns immediately unless there **is** a current selection
/// (`0x493937 or edx,ecx; je epilogue`) and the guid it was handed is either zero or that same
/// selection (`0x493949`/`0x493951`) — and its two callers only ever hand it one of those two.
/// So it runs on exactly the transitions below: a selection that existed is being replaced or
/// dropped. `None → Some` is *not* one of them.
pub(super) fn old_target_cleared(previous: Option<u64>, now: Option<u64>) -> bool {
    previous.is_some() && previous != now
}

/// **Clearing the old target calls `PetStopAttack`'s core** — the third and last way the attack
/// latch goes down, and live only on a **possess** bar (see [`possessing`]; a hunter's pet never
/// raises the latch in the first place, so [`stop_pet_attack`]'s own gate makes this a no-op for
/// it, exactly as `0x4bd65e` does in the reference).
///
/// `0x493910` — the old-target clear, run by *both* selection writers (`0x493540`'s switch calls
/// it with the outgoing guid at `ecx = 0`, the explicit clear with `{0,0}` at `ecx = 1`) — does
/// three things past its entry gate, of which benilla previously carried only the first:
///
/// ```text
/// 0x493a08  0x5ecac0(player)      ; StopAttack   -> CMSG_ATTACKSTOP   (target/scan.rs `commit`)
/// 0x493a0f  0x5ee5a0(player)      ; the unit I am POSSESSING, or null
/// 0x493a18  0x4bd650()            ; PetStopAttack's core, iff there is one   <- THIS
/// 0x493a1d  if (notifyServer) ... ; CMSG_SET_SELECTION
/// ```
///
/// The `0x5ee5a0` at `0x493a0f` is the same possession resolver `0x4bd420` uses to gate the latch,
/// which is the tidy proof that the whole latch arm belongs to one bar: the thing that raises it
/// and the thing that clears it ask the identical question. The call also sits **above** the
/// notify-server branch, so it runs on the silent switch-clear too, not only on the explicit one.
///
/// Modelled as a transition on [`Selection`] rather than a hook in the selection writers because
/// that is what the reference gets for free by routing every writer through one clear: a `Local`
/// mirror sees `/target`, a click, TAB, ESC, the death-teardown clear and the acquire alike. The
/// dedup is the reference's own (`0x493540` bails when the guid is already current, so no clear
/// runs) and falls out of comparing against the mirror.
pub(super) fn pet_stop_on_old_target_clear(
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

/// `0x5ee5a0` — **"the unit I am directly possessing"**, and the gate that decides whether an
/// ATTACK press latches anything at all.
///
/// This is the fact the pet bar was missing, and it is the answer to "why can the reference's
/// Attack button never stay lit". `0x4bd420` calls this on the *player* and compares the result
/// with the bar's own unit; only on a match does `0x4bd42e` raise `[0xb714b0]`. And the function
/// is not "resolve my pet" despite the shape — read at the bytes it is four tests in a row:
///
/// ```text
/// 0x5ee5bc  [player + 0x1c70] & 1        ; "I am driving something that is not me" — else null
/// 0x5ee5d5  the passed unit IS the local player                                    ; else guid 0
/// 0x5ee5e9  [[player + 0xe68] + 0x830]   ; the guid I am driving -> object lookup (type 8)
/// 0x5ee626  [fields + 0xa3] & 1          ; UNIT_FLAG_POSSESSED on that unit — else null
/// 0x5ee62f  charmedBy (else createdBy) == me                                       ; else null
/// ```
///
/// `[player + 0x1c70]`'s bit 0 is set and cleared by the control-transfer handler `0x5ee2xx`
/// against the active-mover guid, firing event `0x27a` with a bool — the client's own
/// "control lost / control gained". So the whole thing means *possession*: Mind Control, Eye of
/// Kilrogg, Eyes of the Beast. vmangos agrees from the other side — `UNIT_FLAG_POSSESSED` is
/// written in exactly three places (`Player::SummonPossessedMinion`, `SPELL_AURA_MOD_POSSESS`,
/// `Player::ModPossessPet`), each of them paired with `SetMover` + `UpdateControl`, and **never**
/// for an ordinary hunter or warlock pet.
///
/// Which is why `PetActionButton_OnClick`'s `IsPetAttackActive` fork and `PetStopAttack` are dead
/// code for a hunter, the way `PetActionButton_StartFlash` is dead code for everyone: the shipped
/// `PetActionBarFrame.lua` is written for both bars and a hunter only ever takes one branch.
///
/// **What benilla checks, and the gap:** the flag and the ownership, which are the two halves that
/// live in the descriptor we already stream. The active-mover halves (`+0x1c70`, the driven guid)
/// need client-control state benilla does not model yet; they only ever *narrow* this further, and
/// the server sets the flag and the mover together, so the flag standing in for both is faithful
/// until possession itself is built.
pub(super) fn possessing(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    store.is_some_and(|s| {
        s.0.unit_flags() & UNIT_FLAG_POSSESSED != 0
            && s.0.unit_owner(benilla_protocol::OwnerFallback::CreatedBy) == self_guid
    })
}

/// Is this press the ATTACK **order** — the one slot word that runs a validator before it sends?
///
/// Type 7 action 2 and nothing else. The reference's type-7 arm branches exactly twice
/// (`cmp ecx,1; jle` then `cmp ecx,2; jne`), so DISMISS, every `action >= 4`, and both mode
/// commands leave down paths that send unconditionally.
pub(super) fn is_attack_order(entry: PetActionEntry) -> bool {
    entry.kind() == benilla_protocol::messages::PET_ACT_COMMAND
        && entry.action() == PET_COMMAND_ATTACK
}

/// Commit one press against the bar and answer **whether it goes on the wire** — the composition
/// of the gate and the latch, in one place because that composition is where the last bug lived.
///
/// `refused` is the shared attack-start validator's verdict on the **pet** as actor (`0x4bd40d`
/// passes `ecx = edi`, the pet object resolved four instructions earlier). A veto costs
/// everything: `0x4bd414 je 0x4bd4c6` jumps to the **function epilogue, not the send**, so there
/// is no packet, no `[0xb714b0] = 1`, and no `PET_BAR_UPDATE`. The click is as if it never
/// happened, except for the red line the validator itself raised.
///
/// This closes the residual decision 0998 named in its own text — *"a refused attack currently
/// lights the button where the real client would not"* — and it is deliberately the one function
/// that both decides and records, because 0998's bug was invisible to thirteen tests that checked
/// the read side and the write side separately and never their join.
///
/// `possessing` is [`possessing`]'s answer, threaded rather than re-derived because it is the
/// *second* gate on the same press and it belongs beside the first: `0x4bd420` runs immediately
/// after `0x4bd40d`'s validator, and unlike it, failing costs only the latch — `0x4bd427 jne
/// 0x4bd444` lands on the **send**, so the order still goes out. That asymmetry is the whole shape
/// of the Attack button: the pet is always dispatched, the button never lights for it.
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

/// Latch a pressed slot's state locally — the half the server never confirms (the module doc's
/// "forced rather than chosen"), now with the client's own masks rather than byte assignments.
///
/// Both writes are exact transcriptions, and the masks are the point:
///
/// - **reaction** (`0x4bc94c`): `state = state & 0xFFFFFF00 | action` — byte 0 only.
/// - **command** (`0x4bc96f`): `state = state & 0x080000FF | action << 8` — bits 8–15, keeping
///   byte 0 and **bit 27**. The client deliberately preserves the disabled bit while rewriting the
///   command, which is also the proof that bit 27 is server-owned: no client path writes it.
///
/// **Only STAY and FOLLOW reach the command write.** The binary's type-7 arm gates it on
/// `action <= 1` (`0x4bd3b1`): DISMISS (3) falls straight through, and ATTACK (2) leaves down the
/// validation chain that ends at `[0xb714b0] = 1` (`0x4bd42e`) — the attack latch, never the
/// command byte. The distinction is the difference between a mode and an order: Stay and Follow
/// are what the pet is *doing until told otherwise*, and they stay lit because `isActive`'s state
/// compare keeps matching the command byte. Attack is a thing you tell it once.
///
/// Getting this wrong is visible within one click: latching ATTACK into the command byte leaves
/// the Attack button lit forever and blanks Follow and Stay.
///
/// **The ATTACK arm's latch is gated on possession** — [`possessing`], the `0x5ee5a0` compare at
/// `0x4bd420`, which no ordinary pet passes. So on a hunter's bar this match arm never fires and
/// the Attack button never checks, which is what the reference does and what the director sees.
/// The arm is still written out rather than deleted because the possess bar is the same bar: Mind
/// Control and Eyes of the Beast reach it, and everything downstream (`IsPetAttackActive`,
/// `PetStopAttack`, the old-target clear) is live for them.
///
/// The other gate on this press, `0x612df0`, ran back in [`drain_pet_actions`] and is *not*
/// symmetric with it: a validator veto costs the whole press (`0x4bd414` jumps to the epilogue,
/// no packet), while failing the possession compare costs only the latch (`0x4bd427` jumps to the
/// send). [`commit_press`] holds that asymmetry.
pub(super) fn latch_press(bar: &mut PetBar, entry: PetActionEntry, possessing: bool) {
    let action = entry.action();
    match entry.kind() {
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_STAY || action == PET_COMMAND_FOLLOW =>
        {
            // `0x4bc960` — the write, then `ecx = 0x161; jmp SignalEvent`. Both halves, and the
            // signal is unconditional: writing the SAME command still repaints the bar, which is
            // the only thing that puts back the light `OnClick`'s `SetChecked(0)` just took off.
            bar.spells.state = (bar.spells.state & 0x0800_00FF) | (action << 8);
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_COMMAND
            if action == PET_COMMAND_ATTACK && possessing =>
        {
            // `0x4bd42e` signals too (`mov ecx,0x161` immediately before the write), and it is the
            // one signal a pet bar never gets: the possession gate above it is what skips both.
            bar.attacking = true;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        benilla_protocol::messages::PET_ACT_REACTION => {
            // `0x4bc940`, the same shape one byte-mask over.
            bar.spells.state = (bar.spells.state & 0xFFFF_FF00) | action;
            bar.bar_signals = bar.bar_signals.wrapping_add(1);
        }
        // DISMISS, spells and every unknown type reach `0x4bd444` without signalling. A spell's
        // button can be checked (its active-aura ring), and it comes back the honest way: the
        // pet's aura field moves and the feed sees a real change.
        _ => {}
    }
}

/// The 1-based Lua slot index → the packed word we still hold for it.
pub(super) fn slot_entry(bar: &PetBar, slot: u32) -> Option<PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get(index).copied()
}

/// [`slot_entry`]'s mutable twin, for the autocast flip's write-back.
pub(super) fn slot_entry_mut(bar: &mut PetBar, slot: u32) -> Option<&mut PetActionEntry> {
    let index = usize::try_from(slot.checked_sub(1)?).ok()?;
    bar.spells.bar.get_mut(index)
}
