//! `DropItemOnUnit` on our pet, whole client headless: the pet test is the resolved unit's guid
//! against the pet's (`0x48da6c`-`0x48da7f`), whatever token names it, and the feed casts through
//! the attempt before it binds the food (`0x6ea266` → `0x6e5a90` → `TryCast 0x6e4b60`, then
//! `0x6ea274` → `BindTarget 0x6e5b40`).

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{ContainerSlot, ContainerState, CursorPayload, UiScript, UnitGuids};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const PET: u64 = 0xF140_0000_0700_002A;
/// Another player, whom a click with the food held asks to trade.
const OTHER: u64 = 0x11;
/// Our Tough Jerky, backpack slot 1.
const JERKY: u64 = 0x4000_0000_0000_0a02;
const FEED_PET: u32 = 6991;
/// `UNIT_FIELD_HEALTH`.
const HEALTH: u16 = 22;

struct Feed {
    app: App,
    rx: Receiver<ClientCommand>,
}

/// Us, alive or not, a hunter who knows Feed Pet, with Tough Jerky in the backpack and our pet
/// streamed: created by us by a spell, and `pet_number` its `UNIT_FIELD_PETNUMBER`.
fn client(alive: bool, pet_number: u32) -> Feed {
    client_with(alive, pet_number, false)
}

/// [`client`], with someone charming us when `charmed` (`UNIT_FIELD_CHARMEDBY`), and [`OTHER`]
/// streamed beside.
fn client_with(alive: bool, pet_number: u32, charmed: bool) -> Feed {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let mut mine = vec![
        (2, 0x19),
        (HEALTH, u32::from(alive) * 100),
        (28, 100),
        (532, JERKY as u32),
        (533, (JERKY >> 32) as u32),
    ];
    if charmed {
        mine.extend([(10, 0x99), (11, 0)]);
    }
    let me = ObjectFields::from_pairs(&mine).into_created(ObjectType::Player);
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
    let other = crate::items::test_spawn_object(
        world,
        OTHER,
        ObjectFields::from_pairs(&[(2, 0x19), (HEALTH, 100), (28, 100)])
            .into_created(ObjectType::Player),
    );
    world.entity_mut(other).insert(Transform::default());
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
    Feed { app, rx }
}

fn run(f: &mut Feed, lua: &str) {
    with_hosted_vm(f.app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
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

/// Feed Pet's attempt signals inside the drop (`0x6e5026`, `0x6e5333`), and so does its bind of the
/// food (`0x6e60cb`); the drop clears the cursor only once the bind has sent (`0x48da8d`), so their
/// listeners see the food still held.
#[test]
fn feed_pets_signals_see_the_food_still_held() {
    let mut f = client(true, 55);
    run(
        &mut f,
        r#"local r = CreateFrame("Frame")
        r:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
        r:SetScript("OnEvent", function()
            if HELD == nil then HELD = CursorHasItem() and 1 or 0 end
        end)"#,
    );
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
    let held_then = f
        .app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<Option<i64>>("return HELD")
        .expect("the global reads");
    assert_eq!(held_then, Some(1));
    assert!(held(&f).is_none(), "the feed cleared the cursor after");
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

/// A clean left click in the world on `guid`'s unit, through `CameraOrSelectOrMoveStop`'s call.
fn world_click_on(f: &mut Feed, guid: u64) {
    let world = f.app.world_mut();
    let entity = world.resource::<crate::net::GuidIndex>().0[&guid];
    world.resource_mut::<crate::target::PressPick>().hovered = crate::target::Hovered {
        target: Some(entity),
        guid: Some(guid),
        distance: 5.0,
        ..Default::default()
    };
    crate::target::release_click(world, benilla_ui::script::ClickMode::Select);
}

fn sent(f: &Feed) -> Vec<String> {
    f.rx.try_iter().map(|c| format!("{c:?}")).collect()
}

fn selected(f: &Feed) -> Option<u64> {
    f.app.world().resource::<crate::target::Selection>().guid
}

/// A left click with the food held feeds our pet (`0x4927e8`), and on another player asks to trade
/// (`0x49278e`) and selects them (`0x49280c`).
#[test]
fn a_world_click_with_the_food_held_feeds_the_pet_and_asks_a_player_to_trade() {
    let mut f = client_with(true, 55, false);
    run(&mut f, "PickupContainerItem(0, 1)");
    world_click_on(&mut f, PET);
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
    run(&mut f, "PickupContainerItem(0, 1)");
    world_click_on(&mut f, OTHER);
    let sent = sent(&f);
    assert!(
        sent.contains(&format!(
            "{:?}",
            ClientCommand::InitiateTrade { target: OTHER }
        )),
        "the click asks to trade: {sent:?}"
    );
    assert_eq!(selected(&f), Some(OTHER));
}

/// A charmed player's left click takes the router's nothing leg (`0x481f98` → `0x48204e`): on our
/// pet or another player with the food held, nothing is fed, traded or selected.
#[test]
fn a_charmed_players_world_click_feeds_trades_and_selects_nothing() {
    let mut f = client_with(true, 55, true);
    run(&mut f, "PickupContainerItem(0, 1)");
    world_click_on(&mut f, PET);
    world_click_on(&mut f, OTHER);
    assert_eq!(sent(&f), Vec::<String>::new(), "nothing goes out");
    assert!(
        matches!(held(&f), Some(CursorPayload::Item(_))),
        "the food is still held"
    );
    assert_eq!(selected(&f), Some(PET), "the target stands");
}

/// A plate on each unit of `guids`, the world frame's children in that order.
fn plates(f: &mut Feed, guids: &[u64]) {
    let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
    crate::vplates::plates_for_test(&mut script, guids);
}

/// A nameplate's left click is the body click's object leg (`0x7cb910` → `0x4949f0` action 1 →
/// `0x4925d0`), inside the click: with the food held it feeds our pet (`0x4927e8`), and asks
/// another player to trade (`0x49278e`) ahead of the select that follows it (`0x49280c`), each
/// sent before `Click` returns.
#[test]
fn a_plate_click_with_the_food_held_feeds_the_pet_and_trades_ahead_of_its_select() {
    let mut f = client_with(true, 55, false);
    plates(&mut f, &[PET, OTHER]);
    run(
        &mut f,
        "PickupContainerItem(0, 1) local pet = WorldFrame:GetChildren() pet:Click()",
    );
    assert_eq!(
        casts(&f),
        vec![format!(
            "{:?}",
            ClientCommand::CastSpellItem {
                spell_id: FEED_PET,
                item_guid: JERKY,
            }
        )],
        "the plate's click feeds the pet"
    );
    assert!(held(&f).is_none(), "the feed cleared the cursor");
    run(
        &mut f,
        "PickupContainerItem(0, 1) local _, other = WorldFrame:GetChildren() other:Click()",
    );
    let sent = sent(&f);
    let trade = format!("{:?}", ClientCommand::InitiateTrade { target: OTHER });
    let select = format!("{:?}", ClientCommand::SetSelection { guid: OTHER });
    let at = |line: &str| sent.iter().position(|s| s == line);
    assert!(
        matches!((at(&trade), at(&select)), (Some(t), Some(s)) if t < s),
        "the trade's ask, then the select: {sent:?}"
    );
    assert_eq!(selected(&f), Some(OTHER));
}

/// The trade leg tests the targeting cursor (`0x492668`) ahead of the select's bind
/// (`0x49280c` → `0x4935d5`), so with the cursor up a plate's click asks no trade, and the bind
/// gives the cast the player instead of selecting them.
#[test]
fn a_plate_click_under_the_cursor_asks_no_trade_and_binds_the_player() {
    let mut f = client_with(true, 55, false);
    plates(&mut f, &[OTHER]);
    f.app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(2050, crate::spell::CastCommit::Spell, 0x0002);
    // A key's body, an input event, so the bind's tag 2 passes (`0x6e5ba5`).
    with_hosted_vm(f.app.world_mut(), |vm| {
        vm.expect("a VM")
            .run_binding("PickupContainerItem(0, 1) WorldFrame:GetChildren():Click()")
    })
    .expect("the chunk runs");
    let sent = sent(&f);
    assert!(
        !sent.iter().any(|s| s.starts_with("InitiateTrade")),
        "no trade under the cursor: {sent:?}"
    );
    assert!(
        sent.contains(&format!(
            "{:?}",
            ClientCommand::CastSpell {
                spell_id: 2050,
                target: Some(OTHER),
            }
        )),
        "the bind casts at the player: {sent:?}"
    );
    assert_eq!(selected(&f), Some(PET), "the target stands");
}
