//! The `"pet"` unit token, the unit our charm, else our summon, names (`0x515a5e`-`0x515a6e`).
//! Its `UNIT_PET`s are the bar writer's (`0x4bc84f`, `crate::ui_pet::net`, the world entry's
//! reset) and our two guid fields' watch (`crate::field_watch`), each at its own place.

use bevy::prelude::*;

use benilla_ui::script::UnitState;

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_script::gate;
use crate::ui_unit::snapshot;

use super::PetUnit;
use crate::query_cache::Ask;

/// What the `"pet"` feed last pushed, for its edges.
#[derive(Default)]
pub(crate) struct PetUnitMemory {
    /// The last snapshot pushed under `"pet"`.
    pushed: Option<UnitState>,
    /// The name cache's counter, which catches a `resolve` answer landing frames later.
    names_generation: gate::Watch,
}

/// Feed the `"pet"` token and the rename event. The token is the unit our `UNIT_FIELD_CHARM`,
/// else our `UNIT_FIELD_SUMMON`, names (`0x515a5e`-`0x515a6e`). The reaction is 0: the pet frame
/// reads none.
pub(crate) fn feed_pet_unit(
    mut vm: crate::script_calls::VmWith<ResMut<'static, NameCache>>,
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
    vm.feed(|script, mut names, fires| {
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
        let stores_changed = !changed_stores.is_empty();
        let stores_removed = !removed_stores.is_empty();
        gate::trace(
            "feed_pet_unit",
            &[
                ("vm_reset", vm_reset),
                ("names", names_moved),
                ("stores", stores_changed),
                ("removed", stores_removed),
            ],
        );
        let group_changed = group.as_ref().is_some_and(|g| g.is_changed());
        let gate = gate::Gate::new(
            vm_reset || names_moved || stores_changed || stores_removed || group_changed,
        );
        removed_stores.clear();
        if gate.skip() {
            return;
        }
        let pet_guid = crate::ui_unit::pet_token_guid(pet.player_store());
        let object = |g: u64| pet.store(g);
        let no_group = crate::ui_party::GroupState::default();
        let src = PetSources {
            names: &names,
            commands: &commands,
            types: CreatureTypeSources::of_resources(
                &names,
                spells.as_deref(),
                creature_types.as_deref(),
            ),
            catalog: spells.as_deref().map(|s| &s.catalog),
            study: crate::ui_unit::tooltip_facts::StudySources {
                tables: study.0.as_deref(),
                families: study.1.as_deref(),
            },
            group: group.as_deref().unwrap_or(&no_group),
            object: &object,
        };
        let fresh = pet_state(pet_guid, pet.store(pet_guid), &src);

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
    })
}

/// What the `"pet"` snapshot reads beside the pet's descriptor: the feed's frame-final sources, or
/// the views lane's as a packet left them ([`crate::ui_unit::views`]).
pub(crate) struct PetSources<'a> {
    pub(crate) names: &'a NameCache,
    pub(crate) commands: &'a NetCommands,
    /// The creature-type resolver's sources. No `ChrClasses.dbc`: the reference reads a class only
    /// for TYPEMASK_PLAYER.
    pub(crate) types: CreatureTypeSources<'a>,
    /// `Spell.dbc`, which the tooltip's owner line reads a creating spell from.
    pub(crate) catalog: Option<&'a benilla_formats::SpellCatalog>,
    pub(crate) study: crate::ui_unit::tooltip_facts::StudySources<'a>,
    pub(crate) group: &'a crate::ui_party::GroupState,
    /// The object manager's lookup (`0x468460`).
    pub(crate) object: &'a dyn Fn(u64) -> Option<&'a ObjectStore>,
}

/// The `"pet"` snapshot of `pet_guid`, the unit the token names, whose descriptor is `store`;
/// `None` for no pet.
pub(crate) fn pet_state(
    pet_guid: u64,
    store: Option<&ObjectStore>,
    src: &PetSources,
) -> Option<UnitState> {
    if pet_guid == 0 {
        return None;
    }
    let Some(store) = store else {
        // A pet whose object the client does not hold still exists (`0x515ff8` → `0x491900`,
        // whose leg `0x4e80a0` answers 1 for our own pet, `0x4e80cb`-`0x4e8108`), and each other
        // getter reads its no-object leg: the name cache by guid, the raid mark.
        let mut s =
            crate::ui_unit::unheld_state(src.names, src.group, pet_guid, Default::default());
        s.exists = true;
        return Some(s);
    };
    // The unit's create looked its name up with a callback (`0x5dec12`); a frame asks only a name
    // the cache holds nothing for, as one invalidated since, and a refused one waits for the next
    // create.
    let name = src
        .names
        .resolve_unit(pet_guid, Some(store), Ask::New(src.commands))
        .map(str::to_string);
    let mut s = snapshot(store, pet_guid, name, 0, Default::default(), src.types);
    let sources = crate::ui_unit::tooltip_facts::TooltipSources {
        names: src.names,
        catalog: src.catalog,
        object: src.object,
        study: src.study,
    };
    s.tooltip = crate::ui_unit::tooltip_facts::tooltip_facts(store, s.creature_type, &sources);
    Some(s)
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

#[cfg(test)]
mod tests {
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::UiScript;
    use bevy::prelude::*;

    use super::super::{PetBar, PetSpells};
    use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid};

    const ME: u64 = 0x77;
    const PET: u64 = 0xF140_0000_0000_002A;
    const OTHER: u64 = 0xF140_0000_0000_002B;
    /// `UNIT_FIELD_SUMMON`, a two-field guid.
    const SUMMON: u16 = 8;

    /// The feed with us held, summoning `summon`, and the bar caching `bar`; a spy on `UNIT_PET`.
    fn pet_app(summon: u64, bar: u64) -> App {
        let mut app = App::new();
        app.add_message::<crate::net::FieldChanged>()
            .init_resource::<GuidIndex>()
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<crate::names::NameCache>()
            .insert_resource(PetBar {
                spells: PetSpells {
                    pet_guid: bar,
                    ..Default::default()
                },
                ..Default::default()
            })
            .add_systems(Update, super::feed_pet_unit);
        let (tx, rx) = crossbeam_channel::unbounded();
        // Kept alive for the run: a dropped receiver would fail the name query.
        std::mem::forget(rx);
        app.insert_resource(NetCommands(tx));
        let me = app
            .world_mut()
            .spawn((
                Guid(ME),
                ObjectStore(ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (SUMMON, summon as u32),
                    (SUMMON + 1, (summon >> 32) as u32),
                ])),
            ))
            .id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_PET")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn answer(app: &mut App, lua: &str) -> Option<f64> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Option<f64>>(lua)
            .unwrap()
    }

    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Vec<String>>("local s = SEEN SEEN = {} return s")
            .unwrap()
    }

    /// The `"pet"` snapshot is the unit the token names, our `UNIT_FIELD_SUMMON` here
    /// (`0x515a5e`-`0x515a6e`), not the bar's guid: with no bar the summon exists, and a bar
    /// naming a unit we neither charm nor summon names no pet. The feed signals no `UNIT_PET`:
    /// the bar writer's and the field watch's are at their own places.
    #[test]
    fn the_pet_snapshot_follows_our_summon_not_the_bar() {
        let mut app = pet_app(PET, 0);
        app.update();
        assert_eq!(answer(&mut app, "return UnitExists('pet')"), Some(1.0));
        assert_eq!(seen(&mut app), Vec::<String>::new());

        let mut app = pet_app(0, OTHER);
        app.update();
        assert_eq!(answer(&mut app, "return UnitExists('pet')"), None);
        app.world_mut().resource_mut::<PetBar>().spells.pet_guid = PET;
        app.update();
        assert_eq!(
            answer(&mut app, "return UnitExists('pet')"),
            None,
            "a new bar guid still names no pet"
        );
        assert_eq!(seen(&mut app), Vec::<String>::new());
    }
}
