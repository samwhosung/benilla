//! A round portrait whose unit the client holds but whose model is still loading.

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_protocol::{EntityKind, ObjectFields};

use super::{
    new_target_image, player_temporary_portrait, sync_portraits, Booth, BoothLight, BoothPanes,
    Booths, DisplayBakes, PortraitBakes, PortraitImages, PortraitMarks, PortraitPart,
    PortraitSource,
};
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::target::Selection;

const STRANGER: u64 = 0x51;

/// One booth per slot the tests draw, and the world `sync_portraits` reads.
fn app() -> App {
    let mut app = App::new();
    app.init_resource::<PortraitImages>()
        .init_resource::<BoothLight>()
        .init_resource::<Selection>()
        .init_resource::<crate::ui_session::InteractNpc>()
        .init_resource::<crate::ui_party::GroupState>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<benilla_world::rig_palette::RigPalettes>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<BoothPanes>()
        .init_resource::<PortraitBakes>()
        .init_resource::<DisplayBakes>()
        .init_resource::<PortraitMarks>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<WowModelMaterial>>();
    let mut booths = Booths::default();
    for (i, token) in ["player", "target"].into_iter().enumerate() {
        let target = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(new_target_image(1));
        let root = app.world_mut().spawn(Transform::IDENTITY).id();
        booths.0.insert(
            token.to_string(),
            Booth::new(RenderLayers::layer(2 + i), root, target),
        );
    }
    app.insert_resource(booths);
    app
}

/// A player the object manager holds, its body not built yet: a night elf woman.
fn player(app: &mut App) -> Entity {
    let fields = ObjectFields::from_pairs(&[(36, 0x0001_0004)]);
    app.world_mut()
        .spawn((
            NetEntity {
                kind: EntityKind::Player,
                display_id: None,
                scale: 1.0,
            },
            ObjectStore(fields),
        ))
        .id()
}

/// A creature the object manager holds under display 903, its body not built yet.
fn creature(app: &mut App) -> Entity {
    let fields = ObjectFields::from_pairs(&[(131, 903)]);
    app.world_mut()
        .spawn((
            NetEntity {
                kind: EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            ObjectStore(fields),
        ))
        .id()
}

/// Let `token`'s bake finish drawing, as its camera would over the next frames.
fn settle(app: &mut App, token: &str) {
    let mut booths = app.world_mut().resource_mut::<Booths>();
    let booth = booths.0.get_mut(token).unwrap();
    booth.wake = 0;
    booth.pending.clear();
    booth.pipes_settling = false;
}

fn body(app: &mut App, unit: Entity) {
    app.world_mut().spawn((
        ChildOf(unit),
        PortraitPart {
            static_mesh: Handle::default(),
            skinned_mesh: None,
            material: Handle::default(),
        },
    ));
}

fn sync(app: &mut App) {
    app.world_mut().run_system_once(sync_portraits).unwrap();
    app.world_mut().flush();
}

fn shown(app: &App, token: &str) -> Option<PortraitSource> {
    app.world()
        .resource::<PortraitImages>()
        .0
        .get(token)
        .cloned()
}

fn booth_target(app: &App, token: &str) -> Handle<Image> {
    app.world().resource::<Booths>().0[token].target.clone()
}

fn baked(app: &App, token: &str) -> bool {
    app.world().resource::<Booths>().0[token].baked.is_some()
}

/// `SetPortraitTexture` empties the texture first (`0x519fc1`-`0x519fd3`), and for a unit it
/// holds calls the bake (`0x51a02b`), which binds nothing while the model is not ready
/// (`0x5250f5`-`0x52515d` → `0x525874`): the circle stays empty, with no stand-in, and the bake
/// fills it once the model loads.
#[test]
fn a_held_unit_still_loading_shows_an_empty_portrait_until_its_bake() {
    let mut app = app();
    let me = player(&mut app);
    app.world_mut().entity_mut(me).insert(SelfPlayer);
    sync(&mut app);
    assert_eq!(
        shown(&app, "player"),
        Some(PortraitSource::Empty),
        "no image, not a TemporaryPortrait nor the booth's backdrop"
    );

    body(&mut app, me);
    sync(&mut app);
    assert!(baked(&app, "player"), "the model loaded: the bake");
    assert_eq!(
        shown(&app, "player"),
        Some(PortraitSource::Live(booth_target(&app, "player")))
    );
}

/// A new unit still loading empties a booth that holds another unit's bake, which a texture the
/// call emptied would not show (`0x519fcc`).
#[test]
fn a_new_unit_still_loading_empties_the_last_units_bake() {
    let mut app = app();
    let first = player(&mut app);
    body(&mut app, first);
    let next = player(&mut app);
    app.world_mut().resource_mut::<Selection>().target = Some(first);
    sync(&mut app);
    assert!(baked(&app, "target"));

    app.world_mut().resource_mut::<Selection>().target = Some(next);
    sync(&mut app);
    assert!(!baked(&app, "target"), "the first unit's face is gone");
    assert_eq!(shown(&app, "target"), Some(PortraitSource::Empty));
}

/// The 2D stand-ins are for a unit the client does not hold (`0x525ba0` from `0x51a0b4`): a
/// selection with no object keeps the face of its name record's race and sex.
#[test]
fn a_unit_not_held_keeps_its_stand_in() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(STRANGER, "Brisca".into(), Some((4, 1, 1)));
    app.world_mut().resource_mut::<Selection>().guid = Some(STRANGER);
    sync(&mut app);
    assert_eq!(
        shown(&app, "target"),
        Some(PortraitSource::File(player_temporary_portrait(
            Some(4),
            Some(1)
        )))
    );
}

/// The same unit rebuilding its body (a shapeshift) keeps its last bake: the reference's widget
/// keeps its texture until the drain's `UNIT_PORTRAIT_UPDATE` binds again, and the drain waits on
/// the body (`0x524d42`-`0x524d8e`).
#[test]
fn a_unit_rebuilding_its_body_keeps_its_last_bake() {
    let mut app = app();
    let me = player(&mut app);
    app.world_mut().entity_mut(me).insert(SelfPlayer);
    body(&mut app, me);
    sync(&mut app);
    assert!(baked(&app, "player"));

    app.world_mut().entity_mut(me).despawn_related::<Children>();
    sync(&mut app);
    assert!(baked(&app, "player"), "the old face stays up");
    assert_eq!(
        shown(&app, "player"),
        Some(PortraitSource::Live(booth_target(&app, "player")))
    );
}

/// A player with no name record leaves the texture empty (`0x51a095`), and asks the name cache
/// with the callback `0x51a100`, which marks the portrait when the record lands.
#[test]
fn a_player_not_held_with_no_name_record_shows_nothing_and_asks_for_one() {
    let mut app = app();
    app.world_mut().resource_mut::<Selection>().guid = Some(STRANGER);
    sync(&mut app);
    assert_eq!(shown(&app, "target"), Some(PortraitSource::Empty));
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(STRANGER, "Brisca".into(), Some((4, 1, 1)));
    app.world_mut()
        .run_system_once(super::drain::mark_portraits)
        .unwrap();
    assert_eq!(
        app.world().resource::<PortraitMarks>().pending(),
        [STRANGER]
    );
}

/// The bake binds a clean cached face before its readiness gate (`0x5250f5`): a unit that is no
/// player by its display id (`0x525077`-`0x5250e4`), so a second creature of a display already
/// baked shows that face while its own model loads.
#[test]
fn a_creature_still_loading_shows_the_face_its_display_already_baked() {
    let mut app = app();
    let first = creature(&mut app);
    body(&mut app, first);
    let second = creature(&mut app);
    app.world_mut().resource_mut::<Selection>().target = Some(first);
    sync(&mut app);
    settle(&mut app, "target");
    let face = booth_target(&app, "target");

    app.world_mut().resource_mut::<Selection>().target = Some(second);
    sync(&mut app);
    assert_eq!(shown(&app, "target"), Some(PortraitSource::Live(face)));
}

/// A player by its GUID (`0x524fe2`-`0x525069`): a clean entry binds while the model loads.
#[test]
fn a_player_still_loading_shows_the_face_cached_under_its_guid() {
    let mut app = app();
    const MEMBER: u64 = 0x61;
    let face = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(new_target_image(1));
    app.world_mut()
        .resource_mut::<PortraitBakes>()
        .store(MEMBER, face.clone());
    let member = player(&mut app);
    app.world_mut().entity_mut(member).insert(Guid(MEMBER));
    app.world_mut().resource_mut::<Selection>().target = Some(member);
    sync(&mut app);
    assert_eq!(shown(&app, "target"), Some(PortraitSource::Live(face)));
}
