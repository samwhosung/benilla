//! The gates a click, a target command and a hand-held payload answer to, on the whole client
//! headless: what 1.12.1 lets each of them do, read from the entry the case comes in by.

use std::collections::HashMap;

use benilla_protocol::field::{
    FIELD_UNIT_BYTES_1, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH,
    FIELD_UNIT_NPC_FLAGS,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{ContainerSlot, ContainerState, MerchantItem, MerchantState, UiScript};
use benilla_world::interact::WorldRightClick;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use super::front_tests::{fix, Fix, ME, MOB, OLD};
use super::{by_name, click, cursor_mode, Hovered, PickOcclusion, PressPick};
use crate::net::{ClientCommand, GuidIndex, ObjectStore};
use crate::target::TargetByNameRequest;
use crate::ui_action::UiErrorKeys;
use crate::vplates::PlateClicks;

/// A gossip NPC, and a hostile unit.
const NPC: u64 = 0x51;
const FOE: u64 = 0x52;
/// `UNIT_NPC_FLAG_GOSSIP`.
const GOSSIP: u32 = 0x1;

/// What the cursor holds before the click.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// A bag item (payload mode 1).
    Item,
    /// A vendor row (payload mode 5), a preview payload (`[0xb4b41c]` set).
    Row,
}

/// What the press was over.
#[derive(Clone, Copy, Debug)]
enum Under {
    Nothing,
    Unit,
    Corpse,
}

impl Fix {
    fn merge_mine(&mut self, pairs: &[(u16, u32)]) {
        let me = self.app.world().resource::<GuidIndex>().0[&ME];
        self.app
            .world_mut()
            .entity_mut(me)
            .get_mut::<ObjectStore>()
            .expect("our descriptor")
            .0
            .merge(ObjectFields::from_pairs(pairs));
    }

    /// A unit `x` yards along, with `fields` over a living unit's.
    fn unit_with(&mut self, guid: u64, x: f32, fields: &[(u16, u32)]) -> Entity {
        let unit = self.scannable_at(guid, x);
        self.app
            .world_mut()
            .entity_mut(unit)
            .get_mut::<ObjectStore>()
            .expect("its descriptor")
            .0
            .merge(ObjectFields::from_pairs(fields));
        unit
    }

    /// The press `latch_press_pick` leaves for a click begun on `unit`, the classifier's cursor.
    fn press_on(&mut self, unit: Entity, guid: u64, attack: bool) {
        self.latch(
            unit,
            guid,
            attack,
            cursor_mode::WorldCursor {
                kind: if attack {
                    cursor_mode::CursorKind::Attack
                } else {
                    cursor_mode::CursorKind::Speak
                },
                unable: false,
            },
        );
    }

    /// The same press with a spell cursor up: the cursor the press latched is the spell's, and
    /// `unable` is its verdict on the unit, whatever the interaction's own range says.
    fn press_under_spell(&mut self, unit: Entity, guid: u64, attack: bool, unable: bool) {
        self.latch(
            unit,
            guid,
            attack,
            cursor_mode::WorldCursor {
                kind: cursor_mode::CursorKind::Cast,
                unable,
            },
        );
    }

    fn latch(&mut self, unit: Entity, guid: u64, attack: bool, cursor: cursor_mode::WorldCursor) {
        *self.app.world_mut().resource_mut::<PressPick>() = PressPick {
            hovered: Hovered {
                target: Some(unit),
                guid: Some(guid),
                distance: 3.0,
                ..Hovered::default()
            },
            cursor,
            attack_fork: cursor_mode::AttackFork(attack),
            ..PressPick::default()
        };
    }

    /// The world's right-click on what `press_on` latched.
    fn right_click(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        self.app
            .world_mut()
            .run_system_once(click::act_on_right_click)
            .expect("the right-click runs");
    }

    /// A right-click on `unit`'s nameplate, begun on what the press latched, through the systems
    /// in the schedule's order (`plate_right_click_runs_after_the_script_calls_head` pins it):
    /// the click's `SelectTarget`, the head of the script calls' pass that binds what it parked,
    /// then the unit's interact.
    fn plate_right_click(&mut self, unit: Entity) {
        self.app
            .world_mut()
            .resource_mut::<PlateClicks>()
            .right
            .push(unit);
        self.app
            .world_mut()
            .run_system_once(click::select_on_plate_click)
            .expect("the plate click runs");
        self.bind();
        self.app
            .world_mut()
            .run_system_once(click::act_on_interact)
            .expect("the unit's interact runs");
    }

    /// Everything sent since the last look.
    fn sent(&self) -> Vec<ClientCommand> {
        self.rx.try_iter().collect()
    }

    fn errors(&self) -> Vec<&'static str> {
        self.app
            .world()
            .resource::<UiErrorKeys>()
            .0
            .iter()
            .map(|e| e.key)
            .collect()
    }

    /// The player's control, `[0xb4b3e4]`.
    fn lose_control(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<crate::player::OwnControlLost>()
            .0 = true;
    }

    /// A VM holding `held` on its cursor.
    fn hold(&mut self, held: Held) {
        let mut script = UiScript::new().expect("a VM");
        match held {
            Held::Item => {
                script.set_container(
                    0,
                    Some(ContainerState {
                        num_slots: 16,
                        slots: HashMap::from([(
                            1,
                            ContainerSlot {
                                guid: 0x77,
                                item_id: 117,
                                count: 1,
                                ..Default::default()
                            },
                        )]),
                        ..Default::default()
                    }),
                );
                script
                    .run("PickupContainerItem(0, 1)")
                    .expect("the pickup runs");
            }
            Held::Row => {
                script.set_merchant(Some(MerchantState {
                    items: vec![MerchantItem {
                        name: None,
                        texture: None,
                        price: 25,
                        quantity: 1,
                        num_available: -1,
                        item_id: 159,
                        stats: None,
                        link: None,
                        max_stack: None,
                    }],
                    ..Default::default()
                }));
                script.run("PickupMerchantItem(1)").expect("the grab runs");
            }
        }
        assert!(
            script.cursor_payload().is_some(),
            "{held:?} is on the cursor"
        );
        self.app.world_mut().insert_non_send_resource(script);
    }

    fn held(&self) -> bool {
        self.app
            .world()
            .non_send_resource::<UiScript>()
            .cursor_payload()
            .is_some()
    }
}

fn swung_at(sent: &[ClientCommand], guid: u64) -> bool {
    sent.iter()
        .any(|c| matches!(c, ClientCommand::AttackSwing { guid: g } if *g == guid))
}

fn greeted(sent: &[ClientCommand], guid: u64) -> bool {
    sent.iter()
        .any(|c| matches!(c, ClientCommand::GossipHello { guid: g } if *g == guid))
}

/// `CGPlayer_C::InteractWithNpc` (`0x5f0130`) opens with `IsShapeShifted` (`0x60e9f0`, at
/// `0x5f0150`): a form whose `SpellShapeshiftForm` row lacks flag `0x8` shows message 351 and
/// returns (`0x5f017f`, `0x5f0185`), ahead of the range test and the walk. A form with `0x8`, a
/// stance (flag `0x1`, so not shapeshifted) and no form talk as ever.
#[test]
fn a_shapeshifted_player_cannot_talk_to_an_npc() {
    // (form byte, its row's flags1 when it has a row, refused)
    for (form, flags, refused) in [
        (0u32, None, false),
        (1, Some(0), true),
        (2, Some(0x8), false),
        (3, Some(0x1), false),
    ] {
        for far in [false, true] {
            let mut f = fix(false);
            f.app.world_mut().init_resource::<crate::player::Player>();
            let mut spells = crate::ui_action::Spells::empty_for_tests();
            if let Some(row_flags) = flags {
                spells.forms.insert(
                    form,
                    benilla_formats::ShapeshiftForm {
                        flags: row_flags,
                        ..Default::default()
                    },
                );
            }
            f.app.world_mut().insert_resource(spells);
            f.merge_mine(&[(FIELD_UNIT_BYTES_1, form << 16)]);
            let npc = f.unit_with(
                NPC,
                if far { 20.0 } else { 3.0 },
                &[(FIELD_UNIT_NPC_FLAGS, GOSSIP)],
            );
            f.press_on(npc, NPC, false);
            f.right_click();
            let what = format!("form {form}, row {flags:?}, out of reach {far}");
            if refused {
                assert_eq!(f.errors(), ["ERR_CANT_INTERACT_SHAPESHIFTED"], "{what}");
                assert!(!greeted(&f.sent(), NPC), "{what}");
            } else {
                assert!(f.errors().is_empty(), "{what}: {:?}", f.errors());
                assert_eq!(greeted(&f.sent(), NPC), !far, "{what}");
            }
        }
    }
}

/// `StartAttack` (`0x5ecb70`) returns before anything when the unit flags carry `0x20000`
/// (`0x5ecbaa`) or `[0xb4b3e4]` is clear (`0x5ecbc0`): no stand, no swing, no sheath. Its set is
/// those two and its own dead and mounted returns; the stunned, fleeing, confused and charmed
/// refusals are the Attack action's (`0x612df0`), which a right-click never reaches (`0x60c247`).
#[test]
fn a_right_click_attack_is_refused_by_start_attacks_own_gates() {
    // (what, own unit flags over PVP, control lost, swings)
    for (what, flags, lost, swings) in [
        ("in control", 0, false, true),
        ("control lost", 0, true, false),
        ("pacified", 0x2_0000, false, false),
        ("stunned, in control", 0x4_0000, false, true),
        ("fleeing, in control", 0x80_0000, false, true),
    ] {
        let mut f = fix(false);
        f.app.world_mut().init_resource::<crate::player::Player>();
        if lost {
            f.lose_control();
        }
        f.merge_mine(&[(FIELD_UNIT_FLAGS, 0x8 | flags)]);
        let foe = f.unit_with(FOE, 3.0, &[]);
        f.press_on(foe, FOE, true);
        f.right_click();
        let sent = f.sent();
        assert_eq!(swung_at(&sent, FOE), swings, "{what}: {sent:?}");
        assert!(
            sent.iter()
                .any(|c| matches!(c, ClientCommand::SetSelection { guid: FOE })),
            "{what}: the click's select comes first: {sent:?}"
        );
        let world = f.app.world_mut();
        let started = !world
            .resource_mut::<Messages<crate::player::StandStateRequest>>()
            .drain()
            .collect::<Vec<_>>()
            .is_empty()
            || !world
                .resource_mut::<Messages<crate::creature_anim::SheathRequest>>()
                .drain()
                .collect::<Vec<_>>()
                .is_empty();
        assert_eq!(
            started, swings,
            "{what}: the stand and the sheath go with the swing"
        );
    }
}

/// The world click router (`0x481f60`) reads the player's `UNIT_FIELD_CHARMEDBY` first
/// (`0x481f98`) and sends every click to the nothing leg (`0x492d30`) with zero points: nothing
/// is selected, interacted with or walked to. A nameplate's click enters at `0x4949f0` instead,
/// past that test, so its `SelectTarget` (`0x492870`) returns and the unit's interact (`0x492881`)
/// still runs.
#[test]
fn a_charmed_players_world_right_click_does_nothing_and_a_plates_still_interacts() {
    for (charmed, plate) in [(false, false), (true, false), (true, true)] {
        let mut f = fix(charmed);
        f.app.world_mut().init_resource::<crate::player::Player>();
        let npc = f.unit_with(NPC, 3.0, &[(FIELD_UNIT_NPC_FLAGS, GOSSIP)]);
        f.press_on(npc, NPC, false);
        if plate {
            f.plate_right_click(npc);
        } else {
            f.right_click();
        }
        let what = format!("charmed {charmed}, plate {plate}");
        assert_eq!(greeted(&f.sent(), NPC), !charmed || plate, "{what}");
        assert_eq!(
            f.selected(),
            if charmed { Some(OLD) } else { Some(NPC) },
            "{what}"
        );
    }
}

/// The router's nothing leg for a charmed player has no points to walk along (`0x492d3f` on a
/// zero ray), and the ground leg's walk is the router's too.
#[test]
fn a_charmed_players_ground_right_click_walks_nowhere() {
    for charmed in [false, true] {
        let mut f = fix(charmed);
        f.app.world_mut().init_resource::<crate::player::Player>();
        f.app
            .world_mut()
            .resource_mut::<crate::player::Approach>()
            .enabled = true;
        *f.app.world_mut().resource_mut::<PressPick>() = PressPick {
            occlusion: PickOcclusion {
                distance: 10.0,
                point: Some(Vec3::new(0.0, 0.0, -10.0)),
                ..PickOcclusion::default()
            },
            ..PressPick::default()
        };
        f.right_click();
        assert_eq!(
            f.app.world().resource::<crate::player::Approach>().active(),
            !charmed,
            "charmed {charmed}"
        );
    }
}

/// The wire's commands that mark a plate right-click's steps, in the order they went out.
fn steps(sent: &[ClientCommand]) -> Vec<&'static str> {
    sent.iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell {
                target: Some(_), ..
            } => Some("cast"),
            ClientCommand::SetSelection { .. } => Some("select"),
            ClientCommand::AttackSwing { .. } => Some("swing"),
            ClientCommand::GossipHello { .. } => Some("gossip"),
            _ => None,
        })
        .collect()
}

/// A plate's right-click (`0x7cb910` → `0x4949f0` action 4 → `0x492820`) runs `SelectTarget`
/// (`0x492870`), which with a targeting cursor up binds its cast to the unit (`0x6e5b40`, which
/// empties the word and sends) and returns; then the unit's interact (`0x492881`) runs, and its
/// attack leg's own `SelectTarget` (`0x5eccaa`) finds no cursor, so it selects, and swings. The
/// wire reads cast, select, swing; and cast, gossip on the service leg, which selects nothing.
/// With no cursor the select comes first. No world-frame press came first to cancel the cursor
/// (`0x492c20`).
#[test]
fn a_plate_right_click_binds_then_selects_then_acts_in_the_references_order() {
    // (the leg, the cursor is up, the wire's steps)
    for (attack, cursor, want) in [
        (true, true, vec!["cast", "select", "swing"]),
        (true, false, vec!["select", "swing"]),
        (false, true, vec!["cast", "gossip"]),
        (false, false, vec!["select", "gossip"]),
    ] {
        let mut f = fix(false);
        f.app.world_mut().init_resource::<crate::player::Player>();
        if cursor {
            f.cursor_up();
        }
        let (guid, fields): (u64, &[(u16, u32)]) = if attack {
            (FOE, &[])
        } else {
            (NPC, &[(FIELD_UNIT_NPC_FLAGS, GOSSIP)])
        };
        let unit = f.unit_with(guid, 3.0, fields);
        f.press_on(unit, guid, attack);
        f.plate_right_click(unit);
        let sent = f.sent();
        let what = format!("attack leg {attack}, cursor up {cursor}: {sent:?}");
        assert_eq!(steps(&sent), want, "{what}");
        if cursor {
            assert!(
                sent.iter().any(|c| matches!(
                    c,
                    ClientCommand::CastSpell { spell_id, target: Some(t) }
                        if *spell_id == super::front_tests::HEAL && *t == guid
                )),
                "the heal is bound to the plate's unit: {what}"
            );
            assert!(!f.cursor_is_up(), "{what}");
        }
        let selected = if attack || !cursor {
            Some(guid)
        } else {
            Some(OLD)
        };
        assert_eq!(f.selected(), selected, "{what}");
    }
}

/// A unit `x` yd off under a press that latched a spell cursor with `spell_unable` as its
/// verdict, against the interact's own range: (the unit's distance, the verdict, in reach). The
/// verdict says the opposite of the range in both rows.
const REACH_CASES: [(f32, bool, bool); 2] = [(3.0, true, true), (20.0, false, false)];

const CORPSE: u64 = 0x53;

/// `0x5f0130` takes the player-to-unit distance (`0x5f01b9`-`0x5f01fd`) against 5.5556 yd, not
/// the spell cursor's verdict the press latched.
#[test]
fn the_talk_leg_measures_its_own_reach_not_the_spell_cursors() {
    for (x, spell_unable, in_reach) in REACH_CASES {
        let mut f = fix(false);
        f.app.world_mut().init_resource::<crate::player::Player>();
        let npc = f.unit_with(NPC, x, &[(FIELD_UNIT_NPC_FLAGS, GOSSIP)]);
        f.press_under_spell(npc, NPC, false, spell_unable);
        f.right_click();
        assert_eq!(
            greeted(&f.sent(), NPC),
            in_reach,
            "at {x} yd, the spell cursor unable {spell_unable}"
        );
    }
}

/// Loot rides `CanLootNow` (`0x5ec110`), the melee reach.
#[test]
fn the_loot_leg_measures_its_own_reach_not_the_spell_cursors() {
    for (x, spell_unable, in_reach) in REACH_CASES {
        let mut f = fix(false);
        f.app.world_mut().init_resource::<crate::player::Player>();
        let body = f.unit_with(
            CORPSE,
            x,
            &[(FIELD_UNIT_HEALTH, 0), (FIELD_UNIT_DYNAMIC_FLAGS, 0x1)],
        );
        f.press_under_spell(body, CORPSE, false, spell_unable);
        f.right_click();
        let looted = f
            .sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::Loot { guid: CORPSE }));
        assert_eq!(
            looted, in_reach,
            "at {x} yd, the spell cursor unable {spell_unable}"
        );
    }
}

/// Skin rides the melee reach: out of it the click walks there (`0x5f06a1`) and casts nothing, in
/// it the click casts.
#[test]
fn the_skin_leg_measures_its_own_reach_not_the_spell_cursors() {
    const SKINNING: u32 = 8613;
    for (x, spell_unable, in_reach) in REACH_CASES {
        let mut f = fix(false);
        f.app.world_mut().init_resource::<crate::player::Player>();
        f.app
            .world_mut()
            .resource_mut::<crate::player::Approach>()
            .enabled = true;
        f.app
            .world_mut()
            .resource_mut::<crate::ui_action::LearnedAbilities>()
            .skinning = Some(SKINNING);
        let body = f.unit_with(
            CORPSE,
            x,
            &[
                (FIELD_UNIT_HEALTH, 0),
                (FIELD_UNIT_FLAGS, cursor_mode::UNIT_FLAG_SKINNABLE),
            ],
        );
        f.press_under_spell(body, CORPSE, false, spell_unable);
        f.right_click();
        let cast = !f
            .app
            .world()
            .resource::<crate::ui_action::ClickCasts>()
            .0
            .is_empty();
        let walking = f.app.world().resource::<crate::player::Approach>().active();
        assert_eq!(
            (cast, walking),
            (in_reach, !in_reach),
            "at {x} yd, the spell cursor unable {spell_unable}"
        );
    }
}

/// The router's legs for a payload still held at the release (`0x481f60`): the ground and nothing
/// legs clear any payload on a right-click (`0x492c90`, `0x492d30`), the object leg a preview one
/// only, `[0xb4b41c]` set (`0x492ce0`), a charmed player's click is the nothing leg, and a corpse
/// is an object like a unit.
#[test]
fn the_router_legs_clear_the_payload_as_the_reference_does() {
    // (held, under the press, charmed, cleared)
    for (held, under, charmed, cleared) in [
        (Held::Item, Under::Nothing, false, true),
        (Held::Row, Under::Nothing, false, true),
        (Held::Item, Under::Unit, false, false),
        (Held::Row, Under::Unit, false, true),
        (Held::Item, Under::Corpse, false, false),
        (Held::Row, Under::Corpse, false, true),
        (Held::Item, Under::Unit, true, true),
    ] {
        let mut f = fix(charmed);
        f.hold(held);
        let mob = f.mob;
        let hovered = match under {
            Under::Nothing => Hovered::default(),
            Under::Unit => Hovered {
                target: Some(mob),
                guid: Some(MOB),
                ..Hovered::default()
            },
            Under::Corpse => Hovered {
                corpse: Some(mob),
                corpse_guid: Some(MOB),
                ..Hovered::default()
            },
        };
        f.app.world_mut().resource_mut::<PressPick>().hovered = hovered;
        f.app
            .world_mut()
            .resource_mut::<Messages<WorldRightClick>>()
            .write(WorldRightClick);
        f.app
            .world_mut()
            .run_system_once(click::world_right_click_payload)
            .expect("the payload leg runs");
        assert_eq!(
            !f.held(),
            cleared,
            "{held:?} under {under:?}, charmed {charmed}"
        );
    }
}

/// The UI root's press handler runs the world input hook (`0x492b50`) before any frame, when no
/// frame holds a press: a right press with anything on the cursor clears it and is consumed
/// (`0x766370`), so the world never sees it and neither the turn nor the release's interact
/// starts. Over a unit too, which the router's own legs (`0x492ce0`) would have kept.
#[test]
fn a_right_press_with_a_payload_is_consumed_before_the_world_sees_it() {
    // (held, button, consumed)
    for (held, button, consumed) in [
        (Some(Held::Item), MouseButton::Right, true),
        (Some(Held::Row), MouseButton::Right, true),
        (None, MouseButton::Right, false),
        (Some(Held::Item), MouseButton::Left, false),
    ] {
        let mut f = fix(false);
        match held {
            Some(held) => f.hold(held),
            None => f
                .app
                .world_mut()
                .insert_non_send_resource(UiScript::new().expect("a VM")),
        }
        let world = f.app.world_mut();
        let mut window = Window {
            resolution: UVec2::new(1024, 768).into(),
            ..default()
        };
        window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
        world.spawn((window, bevy::window::PrimaryWindow));
        let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
        buttons.clear();
        buttons.press(button);
        crate::ui_script::feed_input_once(world);
        let what = format!("{held:?}, {button:?}");
        assert_eq!(
            world
                .resource::<crate::ui_script::PlayerUiPressConsumed>()
                .0,
            consumed,
            "{what}"
        );
        if consumed {
            assert!(!f.held(), "{what}: the press dropped the payload");
        } else if held.is_some() {
            assert!(f.held(), "{what}: the payload stays");
        }
    }
}

/// `TargetByName` (`0x489d60`) leaves the target alone on a miss and shows message `0x127`
/// (`ERR_UNIT_NOT_FOUND`) for a name given (`0x489dbd`, tested on the raw string's first byte, so
/// blanks count) or `0xb8` (`ERR_GENERIC_NO_TARGET`) for an empty one (`0x489dcf`); the chat
/// layer's `/target` request is the binding's body. A hit shows nothing. The Lua binding's miss is
/// pinned in `script_calls`, whose call host fires the lines inside the call.
#[test]
fn a_target_name_that_finds_nobody_shows_the_references_line() {
    let request = |f: &mut Fix, name: &str| {
        f.app
            .world_mut()
            .resource_mut::<Messages<TargetByNameRequest>>()
            .write(TargetByNameRequest { name: name.into() });
        f.app
            .world_mut()
            .run_system_once(by_name::target_by_name_requests)
            .expect("the drain runs");
    };
    for (name, line) in [
        ("Nobody", "ERR_UNIT_NOT_FOUND"),
        ("   ", "ERR_UNIT_NOT_FOUND"),
        ("", "ERR_GENERIC_NO_TARGET"),
    ] {
        let mut f = fix(false);
        request(&mut f, name);
        assert_eq!(f.errors(), [line], "{name:?}");
        assert_eq!(f.selected(), Some(OLD), "{name:?}: the target stays");
        assert!(f.sent().is_empty(), "{name:?}");
    }

    let mut f = fix(false);
    f.app
        .world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(MOB, "Mob".into(), None);
    request(&mut f, "Mob");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
    assert_eq!(f.selected(), Some(MOB));
}
