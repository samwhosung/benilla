//! How `SMSG_UPDATE_OBJECT` blocks apply to the streamed world, through the handler table.
use super::*;
use benilla_assets::coords::wow_to_bevy;

const ME: u64 = 0x0000_0000_0000_0007;
const MOB: u64 = 0xF130_0000_1234_0001;

fn client() -> App {
    crate::game_plugins::schedule_tests::headless_client()
}

/// An indexed entity for `guid`, as a create leaves it.
fn indexed(world: &mut World, guid: u64) -> Entity {
    let e = world.spawn((Guid(guid), Transform::default())).id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
    e
}

/// An out-of-range block lists the active player among others: the others are released, the
/// player is not (`0x465f39`..`0x465f45` skip `0x464920`).
#[test]
fn an_out_of_range_block_never_releases_the_active_player() {
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let me = indexed(world, ME);
    let mob = indexed(world, MOB);

    super::super::handlers::dispatch(world, vec![SessionEvent::ObjectsRemoved(vec![MOB, ME])]);

    let index = &world.resource::<GuidIndex>().0;
    assert_eq!(index.get(&ME), Some(&me), "the player stays indexed");
    assert!(world.get::<Guid>(me).is_some(), "and is not torn down");
    assert!(world.get::<DespawnFade>(me).is_none(), "nor faded out");
    assert!(
        !index.contains_key(&MOB),
        "control: another guid is released"
    );
    assert!(world.get::<DespawnFade>(mob).is_some());
}

fn moved(guid: u64) -> SessionEvent {
    SessionEvent::ObjectMove {
        guid,
        position: [10.0, 20.0, 30.0],
        orientation: 0.0,
    }
}

/// A movement block names the unit this client moves, our player or a possessed unit, and is
/// dropped; any other indexed unit takes the pose (`0x4670a5`..`0x4670bb`). With control lost
/// the mover global is zero (`0x5fa600`), so the block applies.
#[test]
fn a_movement_block_for_the_unit_we_move_is_dropped() {
    let mut app = client();
    let world = app.world_mut();
    let driven = indexed(world, ME);
    world.entity_mut(driven).insert(ActiveMover);
    let possessed = indexed(world, 0xF130_0000_0000_0002);
    world.entity_mut(possessed).insert(ActiveMover);
    let other = indexed(world, MOB);
    let lost = indexed(world, 0xF130_0000_0000_0003);

    super::super::handlers::dispatch(
        world,
        vec![
            moved(ME),
            moved(0xF130_0000_0000_0002),
            moved(MOB),
            moved(0xF130_0000_0000_0003),
        ],
    );

    let at = |world: &World, e: Entity| world.get::<Transform>(e).unwrap().translation;
    assert_eq!(at(world, driven), Vec3::ZERO, "the player is not moved");
    assert_eq!(at(world, possessed), Vec3::ZERO, "nor the possessed unit");
    let pose = wow_to_bevy([10.0, 20.0, 30.0]);
    assert_eq!(
        at(world, other),
        pose,
        "control: another unit takes the pose"
    );
    assert_eq!(at(world, lost), pose, "control: an unsteered unit takes it");
}
