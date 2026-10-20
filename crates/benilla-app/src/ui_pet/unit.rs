//! The `"pet"` unit token and the pet frame's events, keyed on [`PetBar`]'s pet guid: the client's
//! `[0xb714a0]`, whose writer fires `UNIT_PET` (`0x4bc84f`).

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitState};

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_script::gate;
use crate::ui_unit::snapshot;

use super::{PetBar, PetUnit};
use crate::query_cache::Ask;

/// What the `"pet"` feed last pushed, for its edges.
#[derive(Default)]
pub(crate) struct PetUnitMemory {
    /// The last snapshot pushed under `"pet"`.
    pushed: Option<UnitState>,
    /// `UNIT_PET`'s trigger; `None` until the first feed, so a pet out at login announces once.
    guid: Option<u64>,
    /// The last `(pet guid, UNIT_FIELD_PET_NAME_TIMESTAMP)`: the rename signal.
    name_stamp: Option<(u64, Option<u32>)>,
    /// The name cache's counter, which catches a `resolve` answer landing frames later.
    names_generation: gate::Watch,
}

/// Feed the `"pet"` token and the pet frame's events. The token follows the bar's guid, as
/// `UNIT_PET` does (`SetPet`, `0x4bc7e0`), not our `UNIT_FIELD_SUMMON`, so a charmed unit has a
/// frame. The reaction is 0: the pet frame reads none.
pub(crate) fn feed_pet_unit(
    mut vm: crate::script_calls::VmWith<(Res<'static, PetBar>, ResMut<'static, NameCache>)>,
    pet: PetUnit,
    changed_stores: Query<(), Changed<ObjectStore>>,
    mut removed_stores: RemovedComponents<ObjectStore>,
    commands: Res<NetCommands>,
    // The form table, the creature-type resolver's first stage, and the type words.
    spells: Option<Res<crate::ui_action::Spells>>,
    creature_types: Option<Res<crate::target::CreatureTypes>>,
    // The tooltip's studied-creature block.
    study: (
        Option<Res<crate::ui_unit::tooltip_facts::StudyTables>>,
        Option<Res<crate::ui_pet_stats::PetFamilyTables>>,
    ),
    mut memory: Local<crate::ui_script::VmMemo<PetUnitMemory>>,
) {
    vm.feed(|script, (bar, mut names), fires| {
        let Some(script) = script else {
            return;
        };
        let (memory, vm_reset) = memory.get_reset(script);
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
        let gate = gate::Gate::new(
            vm_reset || names_moved || bar_changed || stores_changed || stores_removed,
        );
        removed_stores.clear();
        if gate.skip() {
            return;
        }
        let pet_guid = bar.spells.pet_guid;
        watch_name_timestamp(pet_guid, script, &pet, &mut names, memory);
        // An unstreamed pet pushes nothing: `UnitExists("pet")` reads false and the frame hides.
        let fresh = (pet_guid != 0)
            .then(|| pet.store(pet_guid))
            .flatten()
            .map(|store| {
                // The unit's create looked its name up with a callback (`0x5dec12`); a frame asks
                // only a name the cache holds nothing for, as one invalidated since, and a refused
                // one waits for the next create.
                let name = names
                    .resolve_unit(pet_guid, Some(store), Ask::New(&commands))
                    .map(str::to_string);
                // No `ChrClasses.dbc`: the reference reads a class only for TYPEMASK_PLAYER.
                let types = CreatureTypeSources::of_resources(
                    &names,
                    spells.as_deref(),
                    creature_types.as_deref(),
                );
                let mut s = snapshot(store, pet_guid, name, 0, Default::default(), types);
                let object = |g: u64| pet.store(g);
                let sources = crate::ui_unit::tooltip_facts::TooltipSources {
                    names: &names,
                    catalog: spells.as_deref().map(|s| &s.catalog),
                    object: &object,
                    study: crate::ui_unit::tooltip_facts::StudySources {
                        tables: study.0.as_deref(),
                        families: study.1.as_deref(),
                    },
                };
                s.tooltip =
                    crate::ui_unit::tooltip_facts::tooltip_facts(store, s.creature_type, &sources);
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
                    memory.pushed = Some(cur.clone());
                }
            }
            // Pushing fires nothing: the pet's unit events are its field watches
            // (`crate::field_watch`), and the frame repaints a new pet on `UNIT_PET`.
            None => memory.pushed = None,
        }

        // UNIT_PET("player") only on a pet guid change (`0x4bc84f`), not on a same-pet re-send.
        if memory.guid != Some(pet_guid) {
            gate.audit("feed_pet_unit", "the UNIT_PET edge");
            memory.guid = Some(pet_guid);
            debug!("ui_pet: UNIT_PET — pet is now {pet_guid:#x}");
            fires.post("UNIT_PET", vec![ScriptValue::Str("player".into())]);
        }
    })
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
