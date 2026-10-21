//! The loot legs of the interaction dispatchers through the whole client's host. Each stores the
//! auto-loot byte through `SetAutoLoot` (`0x5df460`) before it loots or uses, which with a loot
//! target held sweeps the open window at once (`0x5df470`, `0x5df480`); the copier reads the byte
//! the click stored (`0x4c1ec7`). `CanLootNow` (`0x5ec110`) refuses while any loot target is held
//! (`0x5ec1e1`-`0x5ec1f8`), in both loot clicks (`0x5df151`, `0x5df2c1`) and the loot cursor
//! (`0x4824fe`, `0x4827d7`), which lights only over the object being looted (`0x48250a`,
//! `0x4827e3`).

use benilla_protocol::messages::{slot_type, LootItem, ObjectFields};
use benilla_protocol::{EntityKind, SessionEvent};
use bevy::ecs::system::RunSystemOnce;
use crossbeam_channel::Receiver;

use super::*;
use crate::net::{ActiveMover, ClientCommand, NetCommands, NetEntity, ObjectStore, SelfGuid};
use crate::ui_loot::{AutoLootByte, LootLatch};

const ME: u64 = 0x10;
/// Two dead, lootable creatures in reach.
const BODY_A: u64 = 0xF130_0000_0000_00A1;
const BODY_B: u64 = 0xF130_0000_0000_00B2;
/// A skinnable creature that has been looted.
const HIDE: u64 = 0xF130_0000_0000_00C3;
const CHEST: u64 = 0xF110_0000_0000_00D4;
/// A lootable bone pile, a corpse object.
const BONES: u64 = 0xF000_0000_0000_00E5;
const LINEN: u32 = 2589;
const SKINNING: u32 = 8613;
/// `UNIT_DYNAMIC_FLAGS`: lootable.
const F_DYNAMIC_FLAGS: u16 = 143;
/// `GAMEOBJECT_TYPE_ID`; 3 is a chest.
const F_GO_TYPE: u16 = 21;

struct Client {
    app: App,
    rx: Receiver<ClientCommand>,
    bodies: [(u64, Entity); 3],
    chest: Entity,
    bones: Entity,
}

fn fields(pairs: &[(u16, u32)]) -> ObjectStore {
    ObjectStore(ObjectFields::from_pairs(pairs))
}

/// The whole client, headless: our player, the active mover, two dead lootable creatures, a
/// looted skinnable one, a chest and a lootable corpse, all in reach, Skinning known, with a VM
/// noting `LOOT_OPENED` and `LOOT_CLOSED` in `SEEN`.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    // What the dispatchers and the cursor read that a headless build leaves to its plugins.
    app.init_resource::<PressPick>()
        .init_resource::<Messages<WorldRightClick>>()
        .init_resource::<Messages<PlateRightClick>>()
        .init_resource::<Messages<Interact>>()
        .init_resource::<crate::player::Approach>()
        .init_resource::<crate::player::FollowState>()
        .init_resource::<crate::player::Player>()
        .init_resource::<crate::player::OwnControlLost>()
        .init_resource::<crate::ui_script::CursorPayloadHeld>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Messages<crate::player::StandStateRequest>>();
    let world = app.world_mut();
    let unit = |kind| NetEntity {
        kind,
        display_id: None,
        scale: 1.0,
    };
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            ActiveMover,
            unit(EntityKind::Player),
            Transform::default(),
            GlobalTransform::default(),
            // `OBJECT_FIELD_TYPE` a player's, the cast ladder's actor.
            fields(&[(2, 0x19), (22, 100), (28, 100)]),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let body = |world: &mut World, guid: u64, x: f32, store: ObjectStore| {
        let e = world
            .spawn((
                Guid(guid),
                unit(EntityKind::Unit),
                Transform::from_xyz(x, 0.0, 0.0),
                GlobalTransform::default(),
                store,
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
        (guid, e)
    };
    let dead_lootable = || fields(&[(22, 0), (28, 100), (F_DYNAMIC_FLAGS, 0x1)]);
    let bodies = [
        body(world, BODY_A, 2.0, dead_lootable()),
        body(world, BODY_B, -2.0, dead_lootable()),
        body(
            world,
            HIDE,
            0.0,
            fields(&[
                (22, 0),
                (28, 100),
                (
                    benilla_protocol::field::FIELD_UNIT_FLAGS,
                    cursor_mode::UNIT_FLAG_SKINNABLE,
                ),
            ]),
        ),
    ];
    let chest = world
        .spawn((
            Guid(CHEST),
            unit(EntityKind::GameObject),
            Transform::from_xyz(0.0, 0.0, 2.0),
            GlobalTransform::default(),
            fields(&[(F_GO_TYPE, 3)]),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(CHEST, chest);
    let bones = world
        .spawn((
            Guid(BONES),
            Transform::from_xyz(0.0, 0.0, -1.0),
            GlobalTransform::default(),
            fields(&[(benilla_protocol::field::FIELD_CORPSE_DYNAMIC_FLAGS, 0x1)]),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(BONES, bones);
    world
        .resource_mut::<crate::ui_action::LearnedAbilities>()
        .skinning = Some(SKINNING);
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [(
            SKINNING,
            benilla_formats::SpellDisplay {
                targets: 0x402,
                implicit_target_a1: 25,
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    );
    world.insert_resource(spells);
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
    let script = benilla_ui::script::UiScript::new().expect("a VM");
    script
        .run(
            r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("LOOT_OPENED") f:RegisterEvent("LOOT_CLOSED")
            f:SetScript("OnEvent", function() tinsert(SEEN, event) end)"#,
        )
        .expect("the recorder installs");
    world.insert_non_send_resource(script);
    Client {
        app,
        rx,
        bodies,
        chest,
        bones,
    }
}

impl Client {
    fn entity(&self, guid: u64) -> Entity {
        self.bodies
            .iter()
            .find(|(g, _)| *g == guid)
            .map(|&(_, e)| e)
            .expect("a body")
    }

    fn shift(&mut self, down: bool) {
        let mut keys = self.app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        if down {
            keys.press(KeyCode::ShiftLeft);
        } else {
            keys.release(KeyCode::ShiftLeft);
        }
    }

    /// A right-click on the body `guid`, its cursor lit.
    fn right_click_body(&mut self, guid: u64) {
        let entity = self.entity(guid);
        self.right_click(PressPick {
            hovered: Hovered {
                target: Some(entity),
                guid: Some(guid),
                distance: 2.0,
                ..Hovered::default()
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Pickup,
                unable: false,
            },
            ..PressPick::default()
        });
    }

    fn right_click_chest(&mut self) {
        self.right_click(PressPick {
            object: HoveredObject {
                target: Some(self.chest),
                guid: Some(CHEST),
                distance: 2.0,
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::Interact,
                unable: false,
            },
            ..PressPick::default()
        });
    }

    /// A right-click on the corpse, its cursor as the press found it: grey while another loot is
    /// held (`0x4827d7`).
    fn right_click_bones(&mut self, unable: bool) {
        self.right_click(PressPick {
            hovered: Hovered {
                corpse: Some(self.bones),
                corpse_guid: Some(BONES),
                distance: 1.0,
                ..Hovered::default()
            },
            cursor: WorldCursor {
                kind: cursor_mode::CursorKind::LootAll,
                unable,
            },
            ..PressPick::default()
        });
    }

    /// The frame's right-click casts, sent through the cast ladder.
    fn drain_casts(&mut self) {
        self.app
            .world_mut()
            .run_system_once(crate::ui_action::drain_click_casts)
            .unwrap();
    }

    fn right_click(&mut self, press: PressPick) {
        let world = self.app.world_mut();
        *world.resource_mut::<PressPick>() = press;
        world
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        world.run_system_once(act_on_right_click).unwrap();
    }

    /// The server's answer for `guid`, then the frame's loot feed.
    fn respond(&mut self, guid: u64, items: Vec<LootItem>) {
        let world = self.app.world_mut();
        crate::net::handlers::dispatch(
            world,
            vec![SessionEvent::LootResponse {
                guid,
                loot_type: 1,
                gold: 0,
                items,
            }],
        );
        crate::ui_loot::feed_loot_once(world);
    }

    /// The window on `guid` open, as our loot click and its answer leave it.
    fn looting(&mut self, guid: u64, items: Vec<LootItem>) {
        self.app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
        self.respond(guid, items);
        let _ = (self.sent(), self.seen());
    }

    fn sent(&self) -> Vec<ClientCommand> {
        self.rx
            .try_iter()
            .filter(|c| {
                matches!(
                    c,
                    ClientCommand::Loot { .. }
                        | ClientCommand::AutostoreLootItem { .. }
                        | ClientCommand::LootRelease { .. }
                        | ClientCommand::GameObjUse { .. }
                )
            })
            .collect()
    }

    fn seen(&mut self) -> String {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            s.expect("a VM")
                .eval::<String>("local s = table.concat(SEEN, ',') SEEN = {} return s")
        })
        .expect("the chunk runs")
    }

    fn byte(&self) -> bool {
        self.app.world().resource::<AutoLootByte>().0
    }
}

fn linen(slot: u8) -> LootItem {
    LootItem {
        slot,
        item_id: LINEN,
        count: 1,
        display_info_id: 0,
        random_property_id: 0,
        slot_type: slot_type::ALLOW_LOOT,
    }
}

/// The unit's loot leg stores Shift as the click finds it (`0x60bfe4`), and the copier reads that
/// byte (`0x4c1ec7`): a Shift let go before the answer still auto-loots, and a Shift pressed after
/// a plain click does not.
#[test]
fn the_click_not_the_answer_decides_the_auto_loot() {
    for shift_at_click in [true, false] {
        let mut c = client();
        c.shift(shift_at_click);
        c.right_click_body(BODY_A);
        assert!(matches!(
            c.sent()[..],
            [ClientCommand::Loot { guid: BODY_A }]
        ));
        assert_eq!(c.byte(), shift_at_click, "stored at the click");
        c.shift(!shift_at_click);
        c.respond(BODY_A, vec![linen(0)]);
        let swept = matches!(c.sent()[..], [ClientCommand::AutostoreLootItem { slot: 0 }]);
        assert_eq!(
            swept, shift_at_click,
            "Shift at the click: {shift_at_click}"
        );
    }
}

/// A Shift-click on a second body with a window open: `SetAutoLoot(1)` sweeps the open window at
/// once (`0x5df470` → `0x5df480`), and `LootUnit` stops at `CanLootNow`, the target being held
/// (`0x5df2c1` → `0x5ec1f6`): the open window's row goes, the second body is not looted.
#[test]
fn a_shift_click_with_a_window_open_sweeps_it_and_loots_nothing_else() {
    let mut c = client();
    c.looting(BODY_A, vec![linen(0)]);
    c.shift(true);
    c.right_click_body(BODY_B);
    assert!(
        matches!(c.sent()[..], [ClientCommand::AutostoreLootItem { slot: 0 }]),
        "the sweep of the open window, and no CMSG_LOOT"
    );
    assert_eq!(c.seen(), "LOOT_OPENED", "the sweep's end (`0x4c2240`)");
    assert!(!c.byte(), "the sweep cleared it (`0x4c2219`)");
}

/// A sweep that finds nothing left closes the window with release (`0x4c224f`), which drops the
/// target (`0x48f2c9`), so the click's `LootUnit` goes on; the byte the sweep cleared opens the
/// second window plain.
#[test]
fn a_shift_click_over_an_empty_window_closes_it_then_loots_plain() {
    let mut c = client();
    c.looting(BODY_A, Vec::new());
    c.shift(true);
    c.right_click_body(BODY_B);
    assert!(
        matches!(
            c.sent()[..],
            [
                ClientCommand::LootRelease { guid: BODY_A },
                ClientCommand::Loot { guid: BODY_B }
            ]
        ),
        "the close, then the loot"
    );
    assert_eq!(c.seen(), "LOOT_CLOSED");
    c.respond(BODY_B, vec![linen(0)]);
    assert!(c.sent().is_empty(), "no sweep");
    assert_eq!(c.seen(), "LOOT_OPENED");
}

/// `CanLootNow` refuses while any loot target is held, this body's own included (`0x5ec1f4`,
/// `0x5ec1f6`): a plain click on either body sends nothing.
#[test]
fn a_loot_click_with_a_window_open_sends_nothing() {
    for guid in [BODY_B, BODY_A] {
        let mut c = client();
        c.looting(BODY_A, vec![linen(0)]);
        c.right_click_body(guid);
        assert!(c.sent().is_empty(), "{guid:#x}");
    }
}

/// The game object's interaction stores the byte when its handler allows it (`0x5f869f`), before
/// the use (`0x5f86a6`): alone, the byte waits for the chest's answer; with a window open, the
/// sweep's stores go out ahead of the use, and the sweep clears the byte.
#[test]
fn a_shift_click_on_a_chest_stores_the_byte_before_the_use() {
    let mut c = client();
    c.shift(true);
    c.right_click_chest();
    assert!(matches!(
        c.sent()[..],
        [ClientCommand::GameObjUse { guid: CHEST }]
    ));
    assert!(c.byte());

    let mut c = client();
    c.looting(BODY_A, vec![linen(0)]);
    c.shift(true);
    c.right_click_chest();
    assert!(
        matches!(
            c.sent()[..],
            [
                ClientCommand::AutostoreLootItem { slot: 0 },
                ClientCommand::GameObjUse { guid: CHEST }
            ]
        ),
        "the sweep, then the use"
    );
    assert!(!c.byte());
}

/// The skin leg's `InteractWithSpell` casts (`0x5f06dc`), then stores the byte (`0x5f06e1`-
/// `0x5f06ef`, its `set` given by the leg, `0x60c06e`): alone, the byte waits for the skinning's
/// loot; with a window open, the cast goes out ahead of the sweep's stores.
#[test]
fn a_shift_skin_casts_then_stores_the_byte() {
    let mut c = client();
    c.shift(true);
    c.right_click_body(HIDE);
    c.drain_casts();
    assert!(c.byte());

    let mut c = client();
    c.looting(BODY_A, vec![linen(0)]);
    c.shift(true);
    c.right_click_body(HIDE);
    c.drain_casts();
    let sent: Vec<ClientCommand> =
        c.rx.try_iter()
            .filter(|c| {
                matches!(
                    c,
                    ClientCommand::CastSpell { .. } | ClientCommand::AutostoreLootItem { .. }
                )
            })
            .collect();
    assert!(
        matches!(
            sent[..],
            [
                ClientCommand::CastSpell {
                    spell_id: SKINNING,
                    ..
                },
                ClientCommand::AutostoreLootItem { slot: 0 }
            ]
        ),
        "the cast, then the sweep: {sent:?}"
    );
}

/// The corpse leg stores the byte (`0x5d6c68`) before `LootCorpse` asks `CanLootNow`
/// (`0x5df151` → `0x5ec110`): a Shift-click whose sweep closes an empty window has dropped the
/// target (`0x4c224f` → `0x48f2c9`), so the corpse is looted, though the cursor was grey at the
/// press.
#[test]
fn a_shift_click_on_a_corpse_over_an_empty_window_loots_it() {
    let mut c = client();
    c.looting(BODY_A, Vec::new());
    c.shift(true);
    c.right_click_bones(true);
    let sent = c.sent();
    assert!(
        matches!(
            sent[..],
            [
                ClientCommand::LootRelease { guid: BODY_A },
                ClientCommand::Loot { guid: BONES }
            ]
        ),
        "the close, then the loot: {sent:?}"
    );

    // With the window's row left, the sweep keeps it open and the corpse is refused.
    let mut c = client();
    c.looting(BODY_A, vec![linen(0)]);
    c.shift(true);
    c.right_click_bones(true);
    assert!(matches!(
        c.sent()[..],
        [ClientCommand::AutostoreLootItem { slot: 0 }]
    ));
}

/// The loot cursor grays over a lootable body or corpse while another loot is held (`0x4824fe`,
/// `0x4827d7` → `0x5ec110`), and stays lit over the one being looted (`0x48250a`, `0x4827e3`).
#[test]
fn the_loot_cursor_lights_only_over_the_object_being_looted() {
    let mut c = client();
    let unable = |c: &mut Client, guid: u64| {
        let hovered = Hovered {
            target: Some(c.entity(guid)),
            guid: Some(guid),
            distance: 2.0,
            ..Hovered::default()
        };
        c.app
            .world_mut()
            .run_system_once(move |inputs: cursor_mode::CursorInputs| {
                cursor_mode::classify(&inputs, &hovered, &HoveredObject::default())
                    .0
                    .unable
            })
            .unwrap()
    };
    assert!(!unable(&mut c, BODY_B), "nothing held");
    c.looting(BODY_A, vec![linen(0)]);
    assert!(unable(&mut c, BODY_B), "another loot held");
    assert!(!unable(&mut c, BODY_A), "the one being looted");
}
