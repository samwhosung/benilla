//! The plate gate `0x60f600` on the whole client headless: which units the driver gives a plate,
//! and the name the bind sets on it.

use std::collections::HashSet;

use bevy::camera::RenderTargetInfo;
use bevy::prelude::*;

use benilla_protocol::{EntityKind, ObjectFields};
use benilla_ui::script::UiScript;
use benilla_world::view::WorldCamera;

use super::{VPlateMode, VPlates};
use crate::names::{CreatureRecord, NameCache};
use crate::net::{
    Embodied, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfGuid, SelfPlayer,
};

const ME: u64 = 0x10;
/// A creature's guid, `HIGHGUID_UNIT` over `low`.
const fn mob(low: u64) -> u64 {
    0xF130_0000_0000_0000 | low
}
/// `OBJECT_FIELD_ENTRY`, `UNIT_FIELD_HEALTH`, `UNIT_FIELD_MAXHEALTH`, `UNIT_FIELD_FLAGS`.
const ENTRY: u16 = 3;
const HEALTH: u16 = 22;
const MAX_HEALTH: u16 = 28;
const FLAGS: u16 = 46;
/// `UNIT_FLAG_IMMUNE_TO_PLAYER`: `CanAttack` from us refuses it, so its plate is friendly.
const IMMUNE_TO_PLAYER: u32 = 0x100;

/// The client with both kinds of plate on, our body at the origin and the camera looking down on
/// it from above, so every unit within 25 yards projects into view.
fn plates_app() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    world.insert_resource(VPlateMode {
        enemies: true,
        friends: true,
    });
    world.init_resource::<crate::player::CameraControl>();
    let script = UiScript::new().expect("a VM");
    script
        .run(r#"CreateFrame("Frame", "WorldFrame") UNKNOWNOBJECT = "Unknown""#)
        .unwrap();
    world.insert_non_send_resource(script);
    let mut camera = Camera::default();
    camera.computed.clip_from_view =
        Mat4::perspective_infinite_reverse_rh(1.2, 1024.0 / 768.0, 0.1);
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1024, 768),
        scale_factor: 1.0,
    });
    world.spawn((
        camera,
        Transform::from_xyz(0.0, 60.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        WorldCamera,
    ));
    // A player (`OBJECT_FIELD_TYPE` 0x19) under `UNIT_FLAG_PLAYER_CONTROLLED`, which takes
    // `CanAttack`'s player arm: with no faction table a plain unit is attackable.
    let me = world
        .spawn((
            SelfPlayer,
            Embodied,
            Guid(ME),
            NetEntity {
                kind: EntityKind::Player,
                display_id: None,
                scale: 1.0,
            },
            Transform::default(),
            GlobalTransform::default(),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, 100),
                (MAX_HEALTH, 100),
                (FLAGS, 0x8),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    part(&mut app, me, true);
    app
}

/// A mesh part under `unit`, drawn in the last render or not: the scene node's cull word
/// `[+0x94]` the gate reads through `0x6704c0`.
fn part(app: &mut App, unit: Entity, drawn: bool) -> Entity {
    use bevy::camera::visibility::SetViewVisibility;
    let world = app.world_mut();
    let part = world
        .spawn((Mesh3d::default(), ViewVisibility::HIDDEN, ChildOf(unit)))
        .id();
    if drawn {
        world.get_mut::<ViewVisibility>(part).unwrap().set_visible();
    }
    part
}

/// A living creature `guid` at `at`, with `fields` over its health.
fn unit(app: &mut App, guid: u64, at: Vec3, fields: &[(u16, u32)]) -> Entity {
    let mut pairs = vec![(2, 0x9), (HEALTH, 100), (MAX_HEALTH, 100)];
    pairs.extend_from_slice(fields);
    let world = app.world_mut();
    let e = world
        .spawn((
            Guid(guid),
            NetEntity {
                kind: EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            Transform::from_translation(at),
            GlobalTransform::from_translation(at),
            ObjectStore(ObjectFields::from_pairs(&pairs)),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
    part(app, e, true);
    e
}

/// A creature record of `creature_type` cached for `entry`, named `name`.
fn record(app: &mut App, entry: u32, name: &str, creature_type: u32) {
    app.world_mut().resource_mut::<NameCache>().insert_creature(
        entry,
        Some(CreatureRecord {
            name: name.into(),
            subname: None,
            creature_type,
            pet_family: 0,
            rank: 0,
            type_flags: 0,
            pet_spell_list: 0,
            civilian: false,
            racial_leader: false,
            display_id: 0,
        }),
    );
}

/// The units the driver gave a plate this frame.
fn plated(app: &mut App) -> HashSet<Entity> {
    super::drive_vplates_once(app.world_mut());
    app.world()
        .resource::<VPlates>()
        .0
        .iter()
        .copied()
        .collect()
}

/// The name text of every shown plate, read as an addon reads it: region 3.
fn plate_names(app: &mut App) -> Vec<String> {
    super::drive_vplates_once(app.world_mut());
    let mut names = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>(
            r#"local out = {}
            for _, c in ipairs({WorldFrame:GetChildren()}) do
                if c:IsShown() then
                    local _, _, name = c:GetRegions()
                    table.insert(out, name:GetText() or "")
                end
            end
            return out"#,
        )
        .unwrap();
    names.sort();
    names
}

/// `0x60f600` drops the camera's own unit (`0x60f68c`, the unit `camera+0x88` names at
/// `0x60f67c`) and measures the 20 yards from it (`0x60f780`-`0x60f7b8`); only the category is
/// judged from the active player (`0x60f6c3`). Attached to another unit, that unit has no plate,
/// the range follows it, and our own body is a candidate like any other.
#[test]
fn plates_are_judged_around_the_camera_unit() {
    let mut app = plates_app();
    let me = app.world().resource::<GuidIndex>().0[&ME];
    let near_me = unit(&mut app, mob(0x31), Vec3::new(-8.0, 0.0, 0.0), &[]);
    let eye = unit(&mut app, mob(0x32), Vec3::new(14.0, 0.0, 0.0), &[]);
    let near_eye = unit(&mut app, mob(0x33), Vec3::new(24.0, 0.0, 0.0), &[]);
    assert_eq!(
        plated(&mut app),
        HashSet::from([near_me, eye]),
        "on our own body: the units within 20 yards of it, never itself"
    );

    let world = app.world_mut();
    world.entity_mut(me).remove::<Embodied>();
    world.entity_mut(eye).insert(Embodied);
    assert_eq!(
        plated(&mut app),
        HashSet::from([me, near_eye]),
        "on the eye: our body and the unit within 20 yards of the eye, never the eye"
    );

    app.world_mut().entity_mut(eye).remove::<Embodied>();
    assert!(
        plated(&mut app).is_empty(),
        "no camera unit fails every unit (`0x60f77c`)"
    );
}

/// A friendly unit whose creature type `0x605570` resolves to 11, Totem, has no plate
/// (`0x60f750`-`0x60f75a`); the enemy side never makes the test (`0x60f733`), so an enemy totem
/// keeps its plate.
#[test]
fn a_friendly_totem_has_no_plate_and_an_enemy_totem_does() {
    let mut app = plates_app();
    record(&mut app, 3573, "Healing Stream Totem", 11);
    record(&mut app, 299, "Wolf", 1);
    let ours = unit(
        &mut app,
        mob(0x31),
        Vec3::X * 3.0,
        &[(ENTRY, 3573), (FLAGS, IMMUNE_TO_PLAYER)],
    );
    let theirs = unit(&mut app, mob(0x32), Vec3::X * 5.0, &[(ENTRY, 3573)]);
    let wolf = unit(
        &mut app,
        mob(0x33),
        Vec3::X * 7.0,
        &[(ENTRY, 299), (FLAGS, IMMUNE_TO_PLAYER)],
    );
    assert_eq!(
        plated(&mut app),
        HashSet::from([theirs, wolf]),
        "{ours:?} has none"
    );
}

/// The bind sets the name text from `GetUnitName` (`0x609210`, called at `0x7cb70c`), whose miss
/// is `UNKNOWNOBJECT` (`0x609324`): a creature whose record has not come back, and a player whose
/// name has not, carry it, never an empty text.
#[test]
fn an_unnamed_units_plate_reads_unknownobject() {
    let mut app = plates_app();
    record(&mut app, 299, "Wolf", 1);
    unit(&mut app, mob(0x31), Vec3::X * 3.0, &[(ENTRY, 299)]);
    unit(&mut app, mob(0x32), Vec3::X * 5.0, &[(ENTRY, 300)]);
    let player = app
        .world_mut()
        .spawn((
            Guid(0x40),
            NetEntity {
                kind: EntityKind::Player,
                display_id: None,
                scale: 1.0,
            },
            Transform::from_translation(Vec3::X * 7.0),
            GlobalTransform::from_translation(Vec3::X * 7.0),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, 100),
                (MAX_HEALTH, 100),
            ])),
        ))
        .id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(0x40, player);
    part(&mut app, player, true);
    assert_eq!(
        plate_names(&mut app),
        vec!["Unknown", "Unknown", "Wolf"],
        "the unknown creature and the unknown player"
    );
}

/// The gate needs the unit's scene node, and its cull word clear (`0x60f63b`-`0x60f650`): the
/// scene sets the word on every node each frame (`0x698a07`) and clears it only for a node it
/// draws (`0x6833dd`-`0x6833ec`). A unit outside last frame's render, or with no model yet, has
/// no plate though it is in range and projects into view.
#[test]
fn a_unit_the_scene_did_not_draw_last_frame_has_no_plate() {
    let mut app = plates_app();
    let drawn = unit(&mut app, mob(0x31), Vec3::X * 3.0, &[]);
    let culled = unit(&mut app, mob(0x32), Vec3::X * 5.0, &[]);
    let culled_part = app.world().entity(culled).get::<Children>().unwrap()[0];
    app.world_mut()
        .entity_mut(culled_part)
        .insert(ViewVisibility::HIDDEN);
    let modelless = unit(&mut app, mob(0x33), Vec3::X * 7.0, &[]);
    let modelless_part = app.world().entity(modelless).get::<Children>().unwrap()[0];
    app.world_mut().entity_mut(modelless_part).despawn();
    assert_eq!(
        plated(&mut app),
        HashSet::from([drawn]),
        "only the unit the last render drew"
    );
    // A second part drawn is enough: the node is the whole model.
    part(&mut app, culled, true);
    assert_eq!(plated(&mut app), HashSet::from([drawn, culled]));
}

/// Under far sight the camera follows the far-sight object whatever its type (`0x5ee483`,
/// `0x5ee4ae`), and the gate resolves the camera's GUID as a unit only (`0x60f677`): a unit
/// there is the centre of the 20 yards and has no plate, and our body is a candidate; a dynamic
/// object resolves to nothing and every plate fails (`0x60f77c`).
#[test]
fn far_sight_judges_plates_around_the_cameras_object() {
    use crate::player::RemoteView;
    let mut app = plates_app();
    let me = app.world().resource::<GuidIndex>().0[&ME];
    let near_me = unit(&mut app, mob(0x31), Vec3::new(-8.0, 0.0, 0.0), &[]);
    let eye = unit(&mut app, mob(0x32), Vec3::new(14.0, 0.0, 0.0), &[]);
    let near_eye = unit(&mut app, mob(0x33), Vec3::new(24.0, 0.0, 0.0), &[]);
    let view = |app: &mut App, entity: Option<Entity>| {
        app.world_mut()
            .resource_mut::<crate::player::ViewSubject>()
            .remote = entity.map(|entity| RemoteView {
            entity,
            feet: Vec3::ZERO,
            pivot_height: 0.0,
        });
    };
    view(&mut app, Some(eye));
    assert_eq!(
        plated(&mut app),
        HashSet::from([me, near_eye]),
        "around the viewed unit: our body and the unit within 20 yards of it, never itself"
    );

    let far_sight = app
        .world_mut()
        .spawn((
            Guid(0xF110_0000_0000_0001),
            NetEntity {
                kind: EntityKind::DynamicObject,
                display_id: None,
                scale: 1.0,
            },
            Transform::from_xyz(14.0, 0.0, 0.0),
        ))
        .id();
    view(&mut app, Some(far_sight));
    assert!(
        plated(&mut app).is_empty(),
        "a dynamic object is no unit: no plates at all"
    );

    view(&mut app, None);
    assert_eq!(
        plated(&mut app),
        HashSet::from([near_me, eye]),
        "home: around our body again"
    );
}
