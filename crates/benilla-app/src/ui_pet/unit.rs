//! The **`"pet"` unit token** and the pet frame's events ([`feed_pet_unit`]).
//!
//! This lives with the pet bar rather than beside the `"player"`/`"target"` feed for one reason:
//! the token's identity is [`PetBar`]'s cached pet guid — the client's `[0xb714a0]`, which is also
//! what `UNIT_PET` fires off (`0x4bc84f`) — so the token and its repaint wire read the same word,
//! from the module that owns it.

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitState};

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_script::gate;
use crate::ui_unit::{fire_transitions, snapshot};

use super::{PetBar, PetUnit};

/// What the `"pet"` token feed last pushed — the three edges it fires on.
#[derive(Default)]
pub(super) struct PetUnitMemory {
    /// The last snapshot pushed under `"pet"`, for [`fire_transitions`]' per-field diff.
    pushed: Option<UnitState>,
    /// The last pet guid — `UNIT_PET`'s trigger. `None` until the first feed, so a login with a
    /// pet already out still announces it once.
    guid: Option<u64>,
    /// The pet's last `UNIT_FIELD_FLAGS & 0x800` — the `PET_ATTACK_START`/`PET_ATTACK_STOP` pair's
    /// trigger. `None` until the first resolved pet, so a login with a pet already fighting
    /// announces it once instead of reading as a transition from "calm".
    in_combat: Option<bool>,
    /// The last `(pet guid, UNIT_FIELD_PET_NAME_TIMESTAMP)` seen — the rename's only signal.
    /// The guid rides along because the timestamp alone would read a *different*
    /// pet's stamp as a rename of this one.
    name_stamp: Option<(u64, Option<u32>)>,
    /// The gate's counter memory (1439): the name cache by its landed counter — the token's own
    /// `resolve` miss lands frames later, and `is_changed` cannot flag it.
    names_generation: gate::Watch,
}

/// `UNIT_FIELD_FLAGS` bit `0x800` — the **pet-in-combat** flag, and the whole trigger for
/// `PET_ATTACK_START`/`PET_ATTACK_STOP` (`0x5ff75e test ah,8`).
///
/// Server-owned and server-written: nothing client-side sets it, which is exactly why it — and not
/// the local click latch — is what the reference watches.
pub(super) const UNIT_FLAG_PET_IN_COMBAT: u32 = 0x0000_0800;

/// Feed the **`"pet"` unit token** and the pet frame's three events.
///
/// **The token resolves off the bar's cached pet guid, not off our own `UNIT_FIELD_SUMMON`**, and
/// that is the client's own choice rather than a convenience. `UNIT_PET` fires
/// from inside `SetPet 0x4bc7e0` (`0x4bc84f`: `SignalEvent(2, "%s", "player")`) — the single writer
/// of `[0xb714a0]`, the same cached guid the whole pet bar reads — and **only when that guid
/// actually changed**. Since `UNIT_PET` is the pet frame's only repaint wire, a token sourced from
/// anywhere else would repaint on the wrong edges. Reading both off one guid is what keeps them in
/// step, and it is also what makes a **possessed or charmed** unit — which has a bar but is nobody's
/// `UNIT_FIELD_SUMMON` — carry a frame at all.
///
/// The reaction argument is `0`: the pet frame reads no reaction (only `"target"` resolves one —
/// [`crate::ui_unit::feed_units`]' own note), and the party feed passes the same for the same
/// reason.
pub(super) fn feed_pet_unit(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    mut pet: PetUnit,
    changed_stores: Query<(), Changed<ObjectStore>>,
    mut removed_stores: RemovedComponents<ObjectStore>,
    mut names: ResMut<NameCache>,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<PetUnitMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    let edges = crate::net::FieldEdges::collect(&mut pet.edges);
    // The gate (1439): the bar (the token's identity), any descriptor change or DESPAWN (the
    // snapshot, the combat flag, and the rename timestamp all live on the pet's store), and the
    // name cache by its landed counter.
    let names_moved = memory.names_generation.moved(names.generation());
    let bar_changed = bar.is_changed();
    let stores_changed = !changed_stores.is_empty();
    let stores_removed = !removed_stores.is_empty();
    gate::trace(
        "feed_pet_unit",
        &[
            ("vm_reset", vm_reset),
            ("names", names_moved),
            ("bar", bar_changed),
            ("stores", stores_changed),
            ("removed", stores_removed),
        ],
    );
    let gate =
        gate::Gate::new(vm_reset || names_moved || bar_changed || stores_changed || stores_removed);
    removed_stores.clear();
    if gate.skip() {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    watch_name_timestamp(pet_guid, &mut script, &pet, &mut names, memory);
    // A bar whose unit has not streamed yet (or has left) pushes nothing: `UnitExists("pet")` then
    // reads false and the frame hides, which is honest — we have the guid but none of the fields
    // the frame draws.
    let fresh = (pet_guid != 0)
        .then(|| pet.store(pet_guid))
        .flatten()
        .map(|store| {
            let name = names
                .resolve_unit(pet_guid, Some(store), &commands)
                .map(str::to_string);
            // No `ChrClasses.dbc` here, and none needed: a pet is a UNIT, so the reference's
            // own TYPEMASK_PLAYER gate refuses it before the class byte is ever read. Passing
            // the table could not change the answer.
            let mut s = snapshot(store, name, 0, None);
            s.guid = pet_guid;
            s
        });

    // Pushed on diff (1439): an identical snapshot re-pushed is invisible to the VM.
    let dirty = match (&fresh, &memory.pushed) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if dirty {
        gate.audit("feed_pet_unit", "the pet snapshot");
        script.set_unit("pet", fresh.clone());
    }
    match &fresh {
        Some(cur) => {
            if memory.pushed.as_ref() != Some(cur) {
                // The one line worth having here, and it is about the failure that is otherwise
                // SILENT: holding a bar for a guid whose descriptor never streamed leaves
                // `UnitExists("pet")` false, so the frame simply never appears and nothing says
                // why. Naming the resolved unit on each change makes "the bar is up but the frame
                // is not" a one-grep question.
                if memory.pushed.is_none() {
                    debug!(
                        "ui_pet: \"pet\" resolved — {} ({}/{} hp)",
                        cur.name.as_deref().unwrap_or("<name pending>"),
                        cur.health,
                        cur.max_health,
                    );
                }
                fire_transitions(&mut script, "pet", memory.pushed.as_ref(), cur, &edges);
                memory.pushed = Some(cur.clone());
            }
        }
        // Clearing a token is not a UNIT_* event — the frame reacts to UNIT_PET below, exactly as
        // the target frame reacts to PLAYER_TARGET_CHANGED rather than to a cleared snapshot.
        None => memory.pushed = None,
    }

    // UNIT_PET(arg1 = "player"), fired only when the pet guid changes (`0x4bc84f`).
    // Summon, stable swap and dismiss are the three edges; a re-sent `SMSG_PET_SPELLS` for
    // the same pet (a learned spell, a mode change) is not one, which is why this diffs the guid
    // rather than riding `feed_pet_bar`'s whole-state diff.
    if memory.guid != Some(pet_guid) {
        gate.audit("feed_pet_unit", "the UNIT_PET edge");
        memory.guid = Some(pet_guid);
        debug!("ui_pet: UNIT_PET — pet is now {pet_guid:#x}");
        script.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    }

    // PET_ATTACK_START (334) / PET_ATTACK_STOP (335) — the pet frame's flashing
    // `UI-Player-AttackStatus` overlay.
    //
    // **CORRECTED.** Decision 0990 derived these from the attack latch `[0xb714b0]`'s edges and
    // said so honestly; the derivation was wrong. The real fire site is `0x5ff793`/`0x5ff79a`
    // inside `0x5ff580`, a per-field change callback registered *by field byte offset*
    // (`0x6042e2 mov edx,0xa0`), which is why walking the call graph out of the pet TU never
    // reaches it. The trigger is the unit's own server-supplied `UNIT_FIELD_FLAGS & 0x800`
    // **transition**, gated on the unit's owner guid being ours.
    //
    // The two are not the same question and they visibly diverge: the latch is a local click
    // record with exactly three writers, so a pet that disengages on its own — its target dies, it
    // runs out of range, it is feared off — clears the flag with no client-side call at all, and
    // the latch-driven version would hold the frame's combat glow lit until the player pressed
    // something. Conversely a defensive pet that retaliates unbidden raises the flag without any
    // press, and the latch never moves.
    //
    // (`PetActionBarFrame`'s Attack *button* is the other mechanism and keeps the latch — it is
    // driven by `PET_BAR_UPDATE` + `IsPetAttackActive`. Two frames, two sources, genuinely
    // independent.)
    let in_combat = fresh
        .as_ref()
        .and_then(|_| pet_combat_flag(pet.store(pet_guid)?, pet.self_guid.0));
    if let Some(now) = in_combat {
        if memory.in_combat != Some(now) {
            gate.audit("feed_pet_unit", "the pet combat-flag edge");
            memory.in_combat = Some(now);
            debug!("ui_pet: pet in-combat flag → {now}");
            script.fire_event(
                if now {
                    "PET_ATTACK_START"
                } else {
                    "PET_ATTACK_STOP"
                },
                vec![],
            );
        }
    } else {
        // No pet, or not ours: forget the edge rather than firing a STOP the reference never
        // sends. `0x5ff580` is a *change* callback — a unit going away does not call it.
        memory.in_combat = None;
    }
}

/// **Watch the pet's name timestamp, drop the cached name when it moves, and announce it**.
///
/// A pet's name is the one name in the client that can change under us. It does not ride the
/// descriptor — it is answered once by `CMSG_PET_NAME_QUERY` into `petnamecache.wdb` and keyed by
/// pet NUMBER — so after a rename `UnitName("pet")` would keep reporting the old name until relog,
/// which is exactly what a taming session does three times an hour.
///
/// **This is the reference's own mechanism, not a workaround for one.** `0x604400` registers a
/// field-change callback *by byte offset* — `mov edx,0x218`, which is
/// `UNIT_FIELD_PET_NAME_TIMESTAMP` — and `0x604aa0` evicts the cache row, re-queries, and fires
/// `LOCALPLAYER_PET_RENAMED` when the unit is ours. (Registration by offset is why no call-graph
/// walk out of the pet TU ever finds it — the same shape as `PET_ATTACK_*`'s `0x5ff580` above, and
/// the second time in this file that has hidden a mechanism.)
///
/// The **event has no handler in stock 1.12** — `UIParent.lua:95` registers it and defines nothing
/// — so the refresh is the eviction, not the event. It is fired anyway because an addon may listen,
/// and because a client that evicts silently is one where "did that fire?" has no answer.
///
/// **A changed pet guid is not a rename.** Comparing timestamps alone would read the next pet's
/// stamp as this one's having moved, so the memory holds the pair and a new guid only records.
///
/// Scope, stated because it is narrower than the reference's: this watches **our** pet, the one the
/// `"pet"` token resolves; the reference's callback is on the field itself and so covers every
/// streamed unit. Another player's pet renamed while we watch it keeps its old name in our cache
/// until it despawns. Widening it means watching the field at the descriptor apply, not here.
fn watch_name_timestamp(
    pet_guid: u64,
    script: &mut UiScript,
    pet: &PetUnit,
    names: &mut NameCache,
    memory: &mut PetUnitMemory,
) {
    let Some(store) = (pet_guid != 0).then(|| pet.store(pet_guid)).flatten() else {
        memory.name_stamp = None;
        return;
    };
    let now = store.0.unit_pet_name_timestamp();
    let previous = memory.name_stamp.replace((pet_guid, now));
    if !was_renamed(previous, (pet_guid, now)) {
        return;
    }
    if let Some(pet_number) = benilla_protocol::guid::pet_number(pet_guid) {
        debug!("ui_pet: pet {pet_guid:#x} was renamed (stamp → {now:?}) — re-asking its name");
        names.forget_pet(pet_number);
    }
    // `0x604aa0`'s own gate on the event, and only on the event: the eviction above is
    // unconditional, the announcement is for a unit charmed or summoned by us.
    if store
        .0
        .unit_owner(benilla_protocol::OwnerFallback::SummonedBy)
        == pet.self_guid.0
    {
        script.fire_event("LOCALPLAYER_PET_RENAMED", vec![]);
    }
}

/// [`watch_name_timestamp`]'s decision alone: did the SAME pet's name timestamp move?
///
/// Four cases, and only one of them is a rename: nothing seen before (a login, or the pet just
/// streamed) is not; a different guid is not, however the stamps compare; the same guid with the
/// same stamp is not; the same guid with a different stamp is.
pub(super) fn was_renamed(previous: Option<(u64, Option<u32>)>, now: (u64, Option<u32>)) -> bool {
    previous.is_some_and(|(guid, stamp)| guid == now.0 && stamp != now.1)
}

/// What the `PET_ATTACK_*` field-change callback would see for this unit: `Some(fighting)` when
/// the unit is **ours**, `None` when it is nobody's business of ours and no event may fire.
///
/// Both halves are `0x5ff580`'s, in its order: the owner test at `0x5ff780` first — CHARMEDBY when
/// set, else SUMMONEDBY, which is *not* the fallback `0x5ee5a0` uses for the same-shaped read — and
/// only then the flag at `0x5ff78d`. Reading the flag without the owner test would let any unit in
/// the world flash our pet frame.
pub(super) fn pet_combat_flag(store: &ObjectStore, self_guid: Option<u64>) -> Option<bool> {
    (store
        .0
        .unit_owner(benilla_protocol::OwnerFallback::SummonedBy)
        == self_guid)
        .then(|| store.0.unit_flags() & UNIT_FLAG_PET_IN_COMBAT != 0)
}
