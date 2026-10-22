//! The `"pet"` unit token and the pet frame's events, keyed on [`PetBar`]'s pet guid: the client's
//! `[0xb714a0]`, whose writer fires `UNIT_PET` (`0x4bc84f`).

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UnitState};

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
    group: Option<Res<crate::ui_party::GroupState>>,
    mut edges: MessageReader<crate::net::FieldChanged>,
    mut memory: Local<crate::ui_script::VmMemo<PetUnitMemory>>,
) {
    let renamed: Vec<u64> = edges
        .read()
        .filter(|e| e.unit_field(UNIT_FIELD_PET_NAME_TIMESTAMP))
        .map(|e| e.guid)
        .collect();
    vm.feed(|script, (bar, mut names), fires| {
        let Some(script) = script else {
            return;
        };
        for &guid in &renamed {
            if watch_name_timestamp(guid, &pet, &mut names, &commands) {
                fires.post("LOCALPLAYER_PET_RENAMED", vec![]);
            }
        }
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
        let group_changed = group.as_ref().is_some_and(|g| g.is_changed());
        let gate = gate::Gate::new(
            vm_reset
                || names_moved
                || bar_changed
                || stores_changed
                || stores_removed
                || group_changed,
        );
        removed_stores.clear();
        if gate.skip() {
            return;
        }
        let pet_guid = bar.spells.pet_guid;
        let held = |store: &ObjectStore| {
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
        };
        // A pet whose object the client does not hold still exists (`0x515ff8` → `0x491900`, whose
        // leg `0x4e80a0` answers 1 for our own pet, `0x4e80cb`-`0x4e8108`), and each other getter
        // reads its no-object leg: the name cache by guid, the raid mark.
        let no_group = crate::ui_party::GroupState::default();
        let unheld = |names: &NameCache| {
            let group = group.as_deref().unwrap_or(&no_group);
            let mut s = crate::ui_unit::unheld_state(names, group, pet_guid, Default::default());
            s.exists = true;
            Some(s)
        };
        let fresh = (pet_guid != 0)
            .then(|| match pet.store(pet_guid) {
                Some(store) => Some(held(store)),
                None => unheld(&names),
            })
            .flatten();

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

/// `UNIT_FIELD_PET_NAME_TIMESTAMP`, the field whose watcher `0x604400` registers on every unit
/// (unit block offset `0x218`).
const UNIT_FIELD_PET_NAME_TIMESTAMP: u16 = 140;

/// `0x604aa0` on a unit whose name timestamp moved: evict the pet-name cache under the unit's own
/// `UNIT_FIELD_PETNUMBER` (`0x604ac6`-`0x604ad8`, unit block `+0x214`) and name it again
/// (`0x609210` at `0x604ae1`), which asks the name anew. Whether it is the active player's charm or
/// summon (charmed-by, else summoned-by, `0x604ae6`-`0x604b10`), whose rename signals
/// `LOCALPLAYER_PET_RENAMED` (`0x604b17`), which stock 1.12 registers (`UIParent.lua:95`) but never
/// handles. A unit the object manager does not hold does nothing (`0x604ac4`).
fn watch_name_timestamp(
    guid: u64,
    pet: &PetUnit,
    names: &mut NameCache,
    commands: &NetCommands,
) -> bool {
    let Some(store) = pet.store(guid) else {
        return false;
    };
    let pet_number = store.0.unit_pet_number();
    debug!("ui_pet: unit {guid:#x} was renamed — re-asking pet name {pet_number}");
    names.forget_pet(pet_number);
    let _ = names.resolve_unit(guid, Some(store), Ask::New(commands));
    store
        .0
        .unit_owner(benilla_protocol::OwnerFallback::SummonedBy)
        == pet.self_guid.0
}
