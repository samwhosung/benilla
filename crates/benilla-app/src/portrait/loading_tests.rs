//! A round portrait whose unit the client holds but whose model is still loading.

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_protocol::{EntityKind, ObjectFields};

use super::{
    new_target_image, player_temporary_portrait, sync_portraits, Booth, BoothLight, BoothPanes,
    Booths, DisplayBakes, PortraitBakes, PortraitImages, PortraitPart, PortraitSource,
};
use crate::net::{NetEntity, ObjectStore, SelfPlayer};
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
    let target = booth_target(&app, "player");
    assert_eq!(
        shown(&app, "player"),
        Some(PortraitSource::Live(target.clone())),
        "the empty booth, not a TemporaryPortrait"
    );
    assert!(!baked(&app, "player"));

    body(&mut app, me);
    sync(&mut app);
    assert!(baked(&app, "player"), "the model loaded: the bake");
    assert_eq!(shown(&app, "player"), Some(PortraitSource::Live(target)));
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
    assert_eq!(
        shown(&app, "target"),
        Some(PortraitSource::Live(booth_target(&app, "target")))
    );
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
