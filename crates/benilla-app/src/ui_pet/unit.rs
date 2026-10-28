//! The `"pet"` unit token, the unit our charm, else our summon, names (`0x515a5e`-`0x515a6e`),
//! which the getters read at the call (`crate::ui_unit::live`). Its `UNIT_PET`s are the bar
//! writer's (`0x4bc84f`, `crate::ui_pet::net`, the world entry's reset) and our two guid fields'
//! watch (`crate::field_watch`), each at its own place; the pet's name stamp watch is here.

use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::NetCommands;

use super::PetUnit;
use crate::query_cache::Ask;

/// `UNIT_FIELD_PET_NAME_TIMESTAMP`, the field whose watcher `0x604400` registers on every unit
/// (unit block offset `0x218`).
const UNIT_FIELD_PET_NAME_TIMESTAMP: u16 = 140;

/// The callbacks of one field edge: the name stamp's watch (`0x604aa0`), a type watcher's, at
/// the edge's own place inside the update packet ([`crate::field_callbacks`]).
pub(crate) fn on_field_edge(world: &mut World, e: crate::net::FieldChanged) {
    if !e.unit_field(UNIT_FIELD_PET_NAME_TIMESTAMP) {
        return;
    }
    crate::lanes::firing(e.at, || {
        if let Err(err) = world.run_system_cached_with(name_timestamp_moved, e.guid) {
            debug!("ui_pet: the name stamp's watch did not run: {err}");
        }
    });
}

/// [`watch_name_timestamp`] on `guid`, its `LOCALPLAYER_PET_RENAMED` signalled bare (`0x604b12`,
/// `0x604b17` → `0x703e50`) on the notice lane.
fn name_timestamp_moved(
    In(guid): In<u64>,
    pet: PetUnit,
    mut names: ResMut<NameCache>,
    commands: Res<NetCommands>,
    mut lane: ResMut<crate::field_watch::NoticeLane>,
) {
    if watch_name_timestamp(guid, &pet, &mut names, &commands) {
        lane.write(crate::field_watch::FieldNotice::Global {
            id: crate::field_watch::id::LOCALPLAYER_PET_RENAMED,
        });
    }
}

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

    /// Us held, summoning `summon`, and the bar caching `bar`; a spy on `UNIT_PET`.
    fn pet_app(summon: u64, bar: u64) -> App {
        let mut app = App::new();
        app.add_message::<crate::net::FieldChanged>()
            .init_resource::<GuidIndex>()
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_guild::GuildState>()
            .insert_resource(PetBar {
                spells: PetSpells {
                    pet_guid: bar,
                    ..Default::default()
                },
                ..Default::default()
            });
        let (tx, rx) = crossbeam_channel::unbounded();
        // Kept alive for the run: a dropped receiver would fail the name query.
        std::mem::forget(rx);
        app.insert_resource(NetCommands(tx));
        let me = app
            .world_mut()
            .spawn((
                crate::net::SelfPlayer,
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

    /// `lua`'s answer with the host lent, as the getters read the token at the call.
    fn answer(app: &mut App, lua: &str) -> Option<f64> {
        crate::ui_unit::live::eval_lent_objects::<Option<f64>>(app.world_mut(), lua).0
    }

    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Vec<String>>("local s = SEEN SEEN = {} return s")
            .unwrap()
    }

    /// `"pet"` names the unit our `UNIT_FIELD_SUMMON` names (`0x515a5e`-`0x515a6e`), not the bar's
    /// guid: with no bar the summon exists, and a bar naming a unit we neither charm nor summon
    /// names no pet. A frame signals no `UNIT_PET`: the bar writer's and the field watch's are at
    /// their own places.
    #[test]
    fn the_pet_token_follows_our_summon_not_the_bar() {
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
