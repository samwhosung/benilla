//! `DropItemOnUnit` on our pet, whole client headless: the pet test is the resolved unit's guid
//! against the pet's (`0x48da6c`-`0x48da7f`), whatever token names it, and the feed casts through
//! the attempt before it binds the food (`0x6ea266` → `0x6e5a90` → `TryCast 0x6e4b60`, then
//! `0x6ea274` → `BindTarget 0x6e5b40`).

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{ContainerSlot, ContainerState, CursorPayload, UiScript, UnitGuids};
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const PET: u64 = 0xF140_0000_0700_002A;
/// Our Tough Jerky, backpack slot 1.
const JERKY: u64 = 0x4000_0000_0000_0a02;
const FEED_PET: u32 = 6991;
/// `UNIT_FIELD_HEALTH`.
const HEALTH: u16 = 22;

struct Feed {
    app: App,
    rx: Receiver<ClientCommand>,
    drop: SystemId,
}

/// Us, alive or not, a hunter who knows Feed Pet, with Tough Jerky in the backpack and our pet
/// streamed: created by us by a spell, and `pet_number` its `UNIT_FIELD_PETNUMBER`.
fn client(alive: bool, pet_number: u32) -> Feed {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = ObjectFields::from_pairs(&[
        (2, 0x19),
        (HEALTH, u32::from(alive) * 100),
        (28, 100),
        (532, JERKY as u32),
        (533, (JERKY >> 32) as u32),
    ])
    .into_created(ObjectType::Player);
    let me = world
        .spawn((
            crate::net::SelfPlayer,
            crate::net::Guid(ME),
            crate::net::ObjectStore(me),
            Transform::default(),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    let pet = crate::items::test_spawn_object(
        world,
        PET,
        ObjectFields::from_pairs(&[
            (2, 0x9),
            (12, ME as u32),
            (14, ME as u32),
            (HEALTH, 100),
            (139, pet_number),
            // `UNIT_CREATED_BY_SPELL`, Call Pet.
            (146, 883),
        ])
        .into_created(ObjectType::Unit),
    );
    world.entity_mut(pet).insert(Transform::default());
    crate::items::test_spawn_item(
        world,
        JERKY,
        ObjectFields::from_pairs(&[(3, 117), (14, 1)]),
        false,
    );
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(117, Some(crate::items::test_template("Tough Jerky")));
    world
        .resource_mut::<crate::ui_pet::PetBar>()
        .spells
        .pet_guid = PET;
    world
        .resource_mut::<crate::ui_action::LearnedAbilities>()
        .feed_pet = Some(FEED_PET);
    // Feed Pet wants an item (`Targets` 0x10), so the attempt raises the cursor for the bind.
    world.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [(
                FEED_PET,
                SpellDisplay {
                    name: "Feed Pet".into(),
                    targets: 0x10,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    {
        let mut selection = world.resource_mut::<crate::target::Selection>();
        selection.guid = Some(PET);
        selection.target = Some(pet);
    }
    let mut script = UiScript::new().expect("a VM");
    script.set_unit_guids(&UnitGuids {
        player: ME,
        pet: PET,
        target: PET,
        ..Default::default()
    });
    let mut bag = ContainerState {
        num_slots: 16,
        ..Default::default()
    };
    bag.slots.insert(
        1,
        ContainerSlot {
            item_id: 117,
            guid: JERKY,
            count: 1,
            link: Some("|cffffffff|Hitem:117:0:0:0|h[Tough Jerky]|h|r".into()),
            ..Default::default()
        },
    );
    script.set_container(0, Some(bag));
    app.world_mut().insert_non_send_resource(script);
    let drop = app
        .world_mut()
        .register_system(super::drop_item::drop_item_on_unit);
    Feed { app, rx, drop }
}

fn run(f: &mut Feed, lua: &str) {
    with_hosted_vm(f.app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
    let drop = f.drop;
    f.app.world_mut().run_system(drop).expect("the drop runs");
}

/// The casts sent since the last read, as their debug lines.
fn casts(f: &Feed) -> Vec<String> {
    f.rx.try_iter()
        .filter(|c| {
            matches!(
                c,
                ClientCommand::CastSpellItem { .. } | ClientCommand::CastSpell { .. }
            )
        })
        .map(|c| format!("{c:?}"))
        .collect()
}

fn held(f: &Feed) -> Option<CursorPayload> {
    f.app
        .world()
        .non_send_resource::<UiScript>()
        .cursor_payload()
}

/// Food dropped on the target frame with our pet targeted feeds it: `"target"` resolves to the
/// pet's unit, and the pet test compares guids (`0x48da6c`), not tokens.
#[test]
fn food_dropped_on_our_targeted_pet_feeds_it() {
    let mut f = client(true, 55);
    run(&mut f, "PickupContainerItem(0, 1) DropItemOnUnit('target')");
    assert_eq!(
        casts(&f),
        vec![format!(
            "{:?}",
            ClientCommand::CastSpellItem {
                spell_id: FEED_PET,
                item_guid: JERKY,
            }
        )]
    );
    assert!(
        held(&f).is_none(),
        "the feed cleared the cursor (`0x48da96`)"
    );
}

/// The feed runs the attempt's rungs before it binds the food (`0x6ea266`): dead, Feed Pet is
/// refused locally, nothing goes out and the food stays on the cursor (`0x48da8d`).
#[test]
fn a_dead_hunters_feed_is_refused_and_keeps_the_food() {
    let mut f = client(false, 55);
    run(&mut f, "PickupContainerItem(0, 1) DropItemOnUnit('pet')");
    assert_eq!(casts(&f), Vec::<String>::new(), "no cast leaves");
    assert!(
        matches!(held(&f), Some(CursorPayload::Item(_))),
        "the food is still held"
    );
}

/// `0x6ea1e0`'s first gate is the unit's `UNIT_FIELD_PETNUMBER` (`0x6ea1fd`, unit block `+0x214`),
/// not its creating spell: a unit we made by a spell with no pet number is not fed.
#[test]
fn a_unit_with_no_pet_number_is_not_fed() {
    let mut f = client(true, 0);
    run(&mut f, "PickupContainerItem(0, 1) DropItemOnUnit('pet')");
    assert_eq!(casts(&f), Vec::<String>::new());
    assert!(matches!(held(&f), Some(CursorPayload::Item(_))));
}
