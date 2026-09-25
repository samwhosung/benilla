//! The `"pet"` unit token and the pet frame's events, keyed on [`PetBar`]'s pet guid: the client's
//! `[0xb714a0]`, whose writer fires `UNIT_PET` (`0x4bc84f`).

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitState};

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_script::gate;
use crate::ui_unit::{fire_transitions, snapshot};

use super::{PetBar, PetUnit};

/// What the `"pet"` feed last pushed, for its edges.
#[derive(Default)]
pub(super) struct PetUnitMemory {
    /// The last snapshot pushed under `"pet"`, for [`fire_transitions`]' per-field diff.
    pushed: Option<UnitState>,
    /// `UNIT_PET`'s trigger; `None` until the first feed, so a pet out at login announces once.
    guid: Option<u64>,
    /// The pet's last in-combat flag, `PET_ATTACK_*`'s trigger; `None` until a pet resolves, so a
    /// pet already fighting at login announces once.
    in_combat: Option<bool>,
    /// The last `(pet guid, UNIT_FIELD_PET_NAME_TIMESTAMP)`: the rename signal.
    name_stamp: Option<(u64, Option<u32>)>,
    /// The name cache's counter, which catches a `resolve` answer landing frames later.
    names_generation: gate::Watch,
}

/// `UNIT_FIELD_FLAGS` `0x800`, the server-written in-combat flag: the whole trigger for
/// `PET_ATTACK_START`/`PET_ATTACK_STOP` (`0x5ff75e`).
pub(super) const UNIT_FLAG_PET_IN_COMBAT: u32 = 0x0000_0800;

/// Feed the `"pet"` token and the pet frame's events. The token follows the bar's guid, as
/// `UNIT_PET` does (`SetPet`, `0x4bc7e0`), not our `UNIT_FIELD_SUMMON`, so a charmed unit has a
/// frame. The reaction is 0: the pet frame reads none.
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
    // An unstreamed pet pushes nothing: `UnitExists("pet")` reads false and the frame hides.
    let fresh = (pet_guid != 0)
        .then(|| pet.store(pet_guid))
        .flatten()
        .map(|store| {
            let name = names
                .resolve_unit(pet_guid, Some(store), &commands)
                .map(str::to_string);
            // No `ChrClasses.dbc`: the reference reads a class only for TYPEMASK_PLAYER.
            let mut s = snapshot(store, name, 0, None);
            s.guid = pet_guid;
            s
        });

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
        // Clearing the token fires no UNIT_* event; the frame repaints on UNIT_PET.
        None => memory.pushed = None,
    }

    // UNIT_PET("player") only on a pet guid change (`0x4bc84f`), not on a same-pet re-send.
    if memory.guid != Some(pet_guid) {
        gate.audit("feed_pet_unit", "the UNIT_PET edge");
        memory.guid = Some(pet_guid);
        debug!("ui_pet: UNIT_PET — pet is now {pet_guid:#x}");
        script.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    }

    // PET_ATTACK_START/STOP (334/335), the pet frame's attack glow, fire on the pet's
    // `UNIT_FIELD_FLAGS & 0x800` edge: a field callback (`0x5ff580`, at `0x5ff793`/`0x5ff79a`,
    // registered by offset at `0x6042e2`), not the Attack button's latch.
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
        // No pet, or not ours: forget the edge; `0x5ff580` is not called when a unit leaves.
        memory.in_combat = None;
    }
}

/// Drop our pet's cached name when its name timestamp moves, as `0x604aa0` does from the field
/// callback on offset `0x218` (`0x604400`), and fire `LOCALPLAYER_PET_RENAMED`, which stock 1.12
/// registers (`UIParent.lua:95`) but never handles. The reference watches every unit; this
/// watches only our pet, so another player's renamed pet keeps its old name.
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
    // `0x604aa0` evicts for any owner but fires the event only for our charm or summon.
    if store
        .0
        .unit_owner(benilla_protocol::OwnerFallback::SummonedBy)
        == pet.self_guid.0
    {
        script.fire_event("LOCALPLAYER_PET_RENAMED", vec![]);
    }
}

/// Whether the same pet's name timestamp moved; a new guid is never a rename.
pub(super) fn was_renamed(previous: Option<(u64, Option<u32>)>, now: (u64, Option<u32>)) -> bool {
    previous.is_some_and(|(guid, stamp)| guid == now.0 && stamp != now.1)
}

/// The in-combat flag as `0x5ff580` sees it: `None` unless the unit is ours by CHARMEDBY, else
/// SUMMONEDBY (`0x5ff780`, not `0x5ee5a0`'s CREATEDBY fallback); then the flag (`0x5ff78d`).
pub(super) fn pet_combat_flag(store: &ObjectStore, self_guid: Option<u64>) -> Option<bool> {
    (store
        .0
        .unit_owner(benilla_protocol::OwnerFallback::SummonedBy)
        == self_guid)
        .then(|| store.0.unit_flags() & UNIT_FLAG_PET_IN_COMBAT != 0)
}
