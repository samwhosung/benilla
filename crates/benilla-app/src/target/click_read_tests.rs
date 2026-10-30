//! The reader's legs of the interaction dispatchers through the whole client's host. A text
//! object's use opens the reader locally (`0x5f58c0` → `0x4e32e0`), inside whatever reaches its
//! dispatcher: a world click, an interaction, an arrival. Its `ITEM_TEXT_BEGIN` and
//! `ITEM_TEXT_READY` are heard there, and nothing is left for a later click to open.

use benilla_protocol::messages::ObjectFields;
use benilla_protocol::EntityKind;
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::net::{ActiveMover, NetCommands, NetEntity, ObjectStore, SelfGuid};

const ME: u64 = 0x10;
/// A TEXT object (type 9) in reach, whose template's first page is [`PAGE`].
const PLAQUE: u64 = 0xF110_0000_4321_0001;
const ENTRY: u32 = 4321;
const PAGE: u32 = 358;

/// The whole client, headless: our player beside the plaque, its page cached, with a VM noting
/// the reader's events in `SEEN`.
fn client() -> (App, Entity) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    // What the dispatchers and the cursor read that a headless build leaves to its plugins.
    app.init_resource::<PressPick>()
        .init_resource::<Messages<Interact>>()
        .init_resource::<crate::player::Approach>()
        .init_resource::<crate::player::FollowState>()
        .init_resource::<crate::player::Player>()
        .init_resource::<crate::player::OwnControlLost>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Messages<crate::player::StandStateRequest>>();
    let world = app.world_mut();
    let spawn = |world: &mut World, guid: u64, kind, at: Vec3, fields: &[(u16, u32)]| {
        let e = world
            .spawn((
                Guid(guid),
                NetEntity {
                    kind,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_translation(at),
                GlobalTransform::default(),
                ObjectStore(ObjectFields::from_pairs(fields)),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
        e
    };
    let me = spawn(
        world,
        ME,
        EntityKind::Player,
        Vec3::ZERO,
        &[(2, 0x19), (22, 100), (28, 100)],
    );
    world.entity_mut(me).insert((SelfPlayer, ActiveMover));
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    // `OBJECT_FIELD_TYPE` object and GameObject, its entry, `GAMEOBJECT_TYPE_ID` TEXT.
    let plaque = spawn(
        world,
        PLAQUE,
        EntityKind::GameObject,
        Vec3::new(0.0, 0.0, 2.0),
        &[(2, 0x21), (3, ENTRY), (21, 9)],
    );
    let mut data = [0; 24];
    data[0] = PAGE as i32;
    world
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(ENTRY, 9, "Plaque".into(), &data);
    world
        .resource_mut::<crate::ui_item_text::PageTexts>()
        .insert(PAGE, "Page one.".into(), 0);
    let script = benilla_ui::script::UiScript::new().expect("a VM");
    script
        .run(
            r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("ITEM_TEXT_BEGIN") f:RegisterEvent("ITEM_TEXT_READY")
            f:SetScript("OnEvent", function() tinsert(SEEN, event) end)"#,
        )
        .expect("the recorder installs");
    world.insert_non_send_resource(script);
    (app, plaque)
}

/// The reader as the turn left it: its object, and the events heard.
fn read(app: &App) -> (Option<u64>, Vec<String>) {
    let world = app.world();
    let object = world
        .resource::<crate::ui_item_text::ItemTextOpen>()
        .object();
    let seen = world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .eval::<Vec<String>>("return SEEN")
        .expect("runs");
    (object, seen)
}

const OPENED: [&str; 2] = ["ITEM_TEXT_BEGIN", "ITEM_TEXT_READY"];

/// A right-click on the plaque opens the reader inside the click.
#[test]
fn a_text_objects_read_opens_inside_the_click() {
    let (mut app, plaque) = client();
    let world = app.world_mut();
    *world.resource_mut::<PressPick>() = PressPick {
        object: HoveredObject {
            target: Some(plaque),
            guid: Some(PLAQUE),
            distance: 2.0,
        },
        cursor: WorldCursor {
            kind: cursor_mode::CursorKind::Interact,
            unable: false,
        },
        ..PressPick::default()
    };
    super::release_click(world, benilla_ui::script::ClickMode::Action);
    let (object, seen) = read(&app);
    assert_eq!(object, Some(PLAQUE));
    assert_eq!(seen, OPENED);
}

/// A crate's [`Interact`] on the plaque opens the reader inside its turn (`0x5f8660`), with no
/// click after it.
#[test]
fn a_text_objects_read_opens_inside_an_interaction() {
    let (mut app, plaque) = client();
    let world = app.world_mut();
    world
        .resource_mut::<Messages<Interact>>()
        .write(Interact(plaque));
    crate::script_calls::run_cached(world, act_on_interact).unwrap();
    let (object, seen) = read(&app);
    assert_eq!(object, Some(PLAQUE));
    assert_eq!(seen, OPENED);
}

/// An approach that arrives at the plaque for Use runs its dispatcher (`0x60fa20`), which opens
/// the reader inside its turn.
#[test]
fn a_text_objects_read_opens_inside_an_arrival() {
    let (mut app, _plaque) = client();
    let world = app.world_mut();
    world.resource_mut::<crate::player::Approach>().arrived =
        Some((crate::player::ApproachVerb::Use, PLAQUE));
    world.run_system_once(act_on_arrival).unwrap();
    let (object, seen) = read(&app);
    assert_eq!(object, Some(PLAQUE));
    assert_eq!(seen, OPENED);
}
