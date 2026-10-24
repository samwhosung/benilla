//! The item watchers' and the loot window's events in the packets' order. A watcher's callback
//! runs inside the notify pass of the update that moved its field (`0x465570` → `0x4655dc`), and
//! the loot packets' handlers change the window and show the pushes inside them, so the events
//! fire inside their packet's handler (`0x537aa0`), in the packets' order (`0x5384d0`): whole
//! before a later packet's events and after an earlier one's.

use benilla_protocol::field::{
    FIELD_ITEM_SPELL_CHARGES, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
    FIELD_PLAYER_INV_SLOT_HEAD,
};
use benilla_protocol::messages::{ItemPushResult, LootItem, ObjectType, ACTION_KIND_ITEM};
use benilla_protocol::{ObjectFields, SessionEvent};
use benilla_ui::script::{ActionSlot, UiScript};
use bevy::prelude::*;

const ME: u64 = 0x7;
/// Worn in the head seat, 0.
const HELM: u64 = 0x4000_0000_0000_0042;
/// In the backpack's first seat, 23.
const POTIONS: u64 = 0x4000_0000_0000_0043;
const HELM_ID: u32 = 7366;
const POTION: u32 = 118;
/// An item a trade getter asked for, not cached.
const SWORD: u32 = 4306;
/// Linen, not cached.
const LINEN: u32 = 2589;
/// A fishing bobber, a loot source the server opens with the latch cold (`0x5eb953`).
const BOBBER: u64 = 0xF110_0000_0000_0051;
/// `PLAYER_FIELD_BYTES`, whose byte 1 is the combo count (`0x5ddff0`).
const PLAYER_FIELD_BYTES: u16 = 188 + 0x1028 / 4;

/// `events` one packet each, every update block closed as its own packet.
fn wire(events: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    events
        .into_iter()
        .flat_map(|ev| {
            let block = matches!(
                ev,
                SessionEvent::ItemCreate { .. } | SessionEvent::ObjectValues { .. }
            );
            std::iter::once(ev).chain(block.then_some(SessionEvent::ObjectUpdateEnd))
        })
        .collect()
}

fn item(guid: u64, entry: u32) -> SessionEvent {
    SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x3),
            (FIELD_OBJECT_ENTRY, entry),
            (6, ME as u32),
            (7, 0),
            (8, ME as u32),
            (9, 0),
            (FIELD_ITEM_STACK_COUNT, 1),
        ])
        .into_created(ObjectType::Item),
    }
}

/// The client in the world as [`ME`], wearing the helm and holding the potions, both on the bar
/// (slots 3 and 5), and a VM whose spy records the item watchers' events and the combo count's,
/// with `arg1`.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let world = app.world_mut();
    let seat = |n: u16, guid: u64| {
        let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * n;
        [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
    };
    let pairs: Vec<(u16, u32)> = seat(0, HELM).into_iter().chain(seat(23, POTIONS)).collect();
    let me = world
        .spawn((
            crate::net::Guid(ME),
            crate::net::SelfPlayer,
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player),
            ),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    for (entry, name) in [(HELM_ID, "Helm"), (POTION, "Potion")] {
        world
            .resource_mut::<crate::items::Items>()
            .insert_template(entry, Some(crate::items::test_template(name)));
    }
    let mut script = UiScript::new().unwrap();
    for (action, entry) in [(3, HELM_ID), (5, POTION)] {
        script.set_action(
            action,
            Some(ActionSlot {
                kind: ACTION_KIND_ITEM,
                action: entry,
                ..Default::default()
            }),
        );
    }
    script
        .run(
            r#"
            LOOT_ITEM_SELF = "LOOT_ITEM_SELF %s"
            LOOT_ITEM_SELF_MULTIPLE = "LOOT_ITEM_SELF_MULTIPLE %s %d"
            Heard = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"ACTIONBAR_SLOT_CHANGED", "BAG_UPDATE_COOLDOWN",
                    "UNIT_INVENTORY_CHANGED", "TRADE_UPDATE", "PLAYER_COMBO_POINTS",
                    "LOOT_OPENED", "LOOT_SLOT_CLEARED", "LOOT_CLOSED", "CHAT_MSG_LOOT"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                if event == "CHAT_MSG_LOOT" then
                    table.insert(Heard, event)
                else
                    table.insert(Heard, event .. ":" .. tostring(arg1))
                end
            end)
            "#,
        )
        .unwrap();
    world.insert_non_send_resource(script);
    frame(&mut app, vec![item(HELM, HELM_ID), item(POTIONS, POTION)]);
    app
}

/// One frame: the packets through the handler table, then the lanes' drain; what the spy heard.
fn frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, wire(packets));
    crate::lanes::fire_in_order(world);
    let s = world.non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Heard").unwrap();
    s.run("Heard = {}").unwrap();
    out
}

fn values(guid: u64, pairs: &[(u16, u32)]) -> SessionEvent {
    SessionEvent::ObjectValues {
        guid,
        fields: ObjectFields::from_pairs(pairs),
    }
}

/// The combo count lands on the active player: its watcher's event (`0x5ddffa`).
fn combo() -> SessionEvent {
    values(ME, &[(PLAYER_FIELD_BYTES, 2 << 8)])
}

/// The spell charges watcher (`0x5d9540`) recounts the bar (`0x5d9582`) and signals
/// `BAG_UPDATE_COOLDOWN` (`0x5d9587`) inside the potions' update.
#[test]
fn the_spell_charges_watcher_signals_at_its_packet() {
    let mut app = client();
    let charges = |n: u32| values(POTIONS, &[(FIELD_ITEM_SPELL_CHARGES, n)]);
    assert_eq!(
        frame(&mut app, vec![charges(1), combo()]),
        [
            "ACTIONBAR_SLOT_CHANGED:5",
            "BAG_UPDATE_COOLDOWN:nil",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
    assert_eq!(
        frame(
            &mut app,
            vec![values(ME, &[(PLAYER_FIELD_BYTES, 0)]), charges(2)]
        ),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ACTIONBAR_SLOT_CHANGED:5",
            "BAG_UPDATE_COOLDOWN:nil"
        ]
    );
}

/// The stack count watcher (`0x5d9400`) on a worn item recounts the bar (`0x5d94e1`), then
/// `NotifyItemSlotChanged` signals the head seat's `UNIT_INVENTORY_CHANGED` (`0x5d950e` →
/// `0x4c72e0`), all inside the helm's update.
#[test]
fn the_stack_count_watcher_recounts_then_signals_its_seat_at_its_packet() {
    let mut app = client();
    let restack = |n: u32| values(HELM, &[(FIELD_ITEM_STACK_COUNT, n)]);
    assert_eq!(
        frame(&mut app, vec![combo(), restack(2)]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ACTIONBAR_SLOT_CHANGED:3",
            "UNIT_INVENTORY_CHANGED:player"
        ]
    );
    assert_eq!(
        frame(
            &mut app,
            vec![restack(3), values(ME, &[(PLAYER_FIELD_BYTES, 0)])]
        ),
        [
            "ACTIONBAR_SLOT_CHANGED:3",
            "UNIT_INVENTORY_CHANGED:player",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
}

/// A trade getter's record callback (`0x4c0340`) signals `TRADE_UPDATE` (`0x4c034f`) inside the
/// record's answer, `SMSG_ITEM_QUERY_SINGLE_RESPONSE`, not after the frame's tick.
#[test]
fn a_trade_getters_record_callback_signals_at_the_answer() {
    let mut app = client();
    let answer = || SessionEvent::ItemTemplate {
        entry: SWORD,
        info: Some(Box::new(crate::items::test_template("Sword"))),
    };
    app.world()
        .resource::<crate::items::Items>()
        .wait_for_trade_record(SWORD);
    assert_eq!(
        frame(&mut app, vec![answer(), combo()]),
        ["TRADE_UPDATE:nil", "PLAYER_COMBO_POINTS:nil"]
    );
}

/// The bobber as a game object we hold, for the loot response's lookup (`0x5ebb33`).
fn stream_bobber(app: &mut App) {
    let world = app.world_mut();
    let e = world
        .spawn((
            crate::net::Guid(BOBBER),
            crate::net::ObjectStore(
                ObjectFields::from_pairs(&[(2, 0x21)]).into_created(ObjectType::GameObject),
            ),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(BOBBER, e);
}

/// `SMSG_LOOT_RESPONSE` for the bobber, a fishing window over `rows`.
fn fished(rows: &[(u8, u32)]) -> SessionEvent {
    SessionEvent::LootResponse {
        guid: BOBBER,
        loot_type: benilla_protocol::messages::loot_type::FISHING,
        gold: 0,
        items: rows
            .iter()
            .map(|&(slot, item_id)| LootItem {
                slot,
                item_id,
                count: 1,
                display_info_id: 0,
                random_property_id: 0,
                slot_type: benilla_protocol::messages::slot_type::ALLOW_LOOT,
            })
            .collect(),
    }
}

/// `SMSG_ITEM_PUSH_RESULT` of `entry` for us, shown in chat.
fn pushed(entry: u32) -> SessionEvent {
    SessionEvent::ItemPushResult(ItemPushResult {
        player_guid: ME,
        from_npc: false,
        created: false,
        show_in_chat: true,
        bag_slot: 0xFF,
        item_slot: 24,
        item_entry: entry,
        suffix_factor: 0,
        random_property_id: 0,
        count: 1,
    })
}

/// `entry`'s record, as `SMSG_ITEM_QUERY_SINGLE_RESPONSE` answers it.
fn answered(entry: u32, name: &str) -> SessionEvent {
    SessionEvent::ItemTemplate {
        entry,
        info: Some(Box::new(crate::items::test_template(name))),
    }
}

/// A removal's `ClearLootRow` (`0x5ebeb0` → `0x4c1f00`) signals `LOOT_SLOT_CLEARED` inside its
/// packet, and the response's copier ends with `LOOT_OPENED` inside its own (`0x4c1ea8`).
#[test]
fn the_loot_windows_steps_fire_at_their_packets() {
    let mut app = client();
    stream_bobber(&mut app);
    assert_eq!(
        frame(&mut app, vec![combo(), fished(&[(0, POTION), (1, POTION)])]),
        ["PLAYER_COMBO_POINTS:nil", "LOOT_OPENED:nil"]
    );
    let removed = |slot: u8| SessionEvent::LootRemoved { slot };
    assert_eq!(
        frame(
            &mut app,
            vec![removed(0), values(ME, &[(PLAYER_FIELD_BYTES, 0)])]
        ),
        ["LOOT_SLOT_CLEARED:1", "PLAYER_COMBO_POINTS:nil"]
    );
    // The last row's removal empties the window, which closes inside the packet (`0x4c2a70`,
    // `0x4c1f8d`).
    assert_eq!(
        frame(&mut app, vec![combo(), removed(1)]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "LOOT_SLOT_CLEARED:2",
            "LOOT_CLOSED:nil"
        ]
    );
}

/// A response whose row's record the cache lacks ends its copier inside the record's answer
/// (`0x4c2ac0`, `0x4c2afc`): behind a packet between the two.
#[test]
fn a_copier_waiting_on_a_record_ends_at_the_answer() {
    let mut app = client();
    stream_bobber(&mut app);
    assert_eq!(
        frame(
            &mut app,
            vec![
                fished(&[(0, LINEN)]),
                combo(),
                answered(LINEN, "Linen Cloth")
            ]
        ),
        ["PLAYER_COMBO_POINTS:nil", "LOOT_OPENED:nil"]
    );
}

/// `OnItemPush` (`0x491a60`) shows its line inside its packet, and one whose record the cache
/// lacks inside the record's answer (`0x491ee0`), behind a packet between the two.
#[test]
fn an_item_push_shows_at_its_packet_or_its_records_answer() {
    let mut app = client();
    assert_eq!(
        frame(&mut app, vec![combo(), pushed(POTION)]),
        ["PLAYER_COMBO_POINTS:nil", "CHAT_MSG_LOOT"]
    );
    assert_eq!(
        frame(
            &mut app,
            vec![pushed(POTION), values(ME, &[(PLAYER_FIELD_BYTES, 0)])]
        ),
        ["CHAT_MSG_LOOT", "PLAYER_COMBO_POINTS:nil"]
    );
    assert_eq!(
        frame(
            &mut app,
            vec![pushed(LINEN), combo(), answered(LINEN, "Linen Cloth")]
        ),
        ["PLAYER_COMBO_POINTS:nil", "CHAT_MSG_LOOT"]
    );
}

/// A response whose copier waits on a record ends inside the answer (`0x4c2afc`) on its own
/// window, though a later response in the frame replaced it: the second copier's teardown closes
/// what the first opened (`0x4c1ccb`), then ends its own inside its packet (`0x4c1ea8`).
#[test]
fn a_waiting_copier_ends_on_its_own_window_before_a_later_response() {
    let mut app = client();
    stream_bobber(&mut app);
    assert_eq!(
        frame(
            &mut app,
            vec![
                fished(&[(0, LINEN)]),
                answered(LINEN, "Linen Cloth"),
                fished(&[(0, POTION)])
            ]
        ),
        ["LOOT_OPENED:nil", "LOOT_CLOSED:nil", "LOOT_OPENED:nil"]
    );
}

/// A response whose rows' records are all held ends its copier inside its packet (`0x4c1ea8`), so
/// a release in the same frame closes a window that opened (`0x5ec0f3`).
#[test]
fn a_held_copier_ends_inside_its_packet_before_a_later_close() {
    let mut app = client();
    stream_bobber(&mut app);
    assert_eq!(
        frame(
            &mut app,
            vec![
                fished(&[(0, POTION)]),
                SessionEvent::LootReleaseResponse {
                    guid: BOBBER,
                    result: 1
                }
            ]
        ),
        ["LOOT_OPENED:nil", "LOOT_CLOSED:nil"]
    );
}

/// Uncached linen, on the bar's slot 7, about to be looted: a push and an item create each wait
/// on its record, the item's set-up recounting the bar and signalling its owner at the answer
/// (`0x5d8390`, `0x5d85ce`, `0x5d8606`), the push showing its line (`0x491ee0`).
fn linen_on_the_bar() -> App {
    let mut app = client();
    let world = app.world_mut();
    let mut script = world.non_send_resource_mut::<UiScript>();
    script.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        ..Default::default()
    });
    script.set_action(
        7,
        Some(ActionSlot {
            kind: ACTION_KIND_ITEM,
            action: LINEN,
            ..Default::default()
        }),
    );
    script.run("Heard = {}").unwrap();
    app
}

/// The looted linen's guid.
const LINENS: u64 = 0x4000_0000_0000_0044;

/// One answer runs its record's callbacks in the order they were asked (`0x5d8321`-`0x5d833f`):
/// a push ahead of its item's create on the wire shows its line before the item's set-up.
#[test]
fn one_answer_runs_its_callbacks_in_the_order_asked() {
    let mut app = linen_on_the_bar();
    assert_eq!(
        frame(
            &mut app,
            vec![
                pushed(LINEN),
                item(LINENS, LINEN),
                answered(LINEN, "Linen Cloth")
            ]
        ),
        [
            "CHAT_MSG_LOOT",
            "ACTIONBAR_SLOT_CHANGED:7",
            "UNIT_INVENTORY_CHANGED:player"
        ]
    );
}

/// A looted item's create comes ahead of its push (vmangos `StoreNewItem`, `Player.cpp:10403`,
/// then `SendNewItem`, `LootHandler.cpp:191`), so the set-up asks first and runs first at the
/// answer, then the push's line.
#[test]
fn a_looted_items_set_up_runs_ahead_of_its_push_at_the_answer() {
    let mut app = linen_on_the_bar();
    assert_eq!(
        frame(
            &mut app,
            vec![
                item(LINENS, LINEN),
                pushed(LINEN),
                answered(LINEN, "Linen Cloth")
            ]
        ),
        [
            "ACTIONBAR_SLOT_CHANGED:7",
            "UNIT_INVENTORY_CHANGED:player",
            "CHAT_MSG_LOOT"
        ]
    );
}

/// The close our own gather's GO owes runs inside the GO (`0x60d10b` → `0x48f200`), ahead of the
/// response behind it: it takes the window open then, or nothing (`0x48f23f`), never the one
/// the response opens.
#[test]
fn a_close_owed_ahead_of_a_response_takes_only_the_window_before_it() {
    let mut app = client();
    stream_bobber(&mut app);
    let owe = |app: &mut App| {
        app.world_mut()
            .resource_mut::<crate::ui_loot::LootCloseAtRest>()
            .0 = true
    };
    let open = |app: &App| app.world().resource::<crate::ui_loot::LootState>().source();
    owe(&mut app);
    assert_eq!(
        frame(&mut app, vec![fished(&[(0, POTION)])]),
        ["LOOT_OPENED:nil"]
    );
    assert_eq!(open(&app), Some(BOBBER));
    // Settled at the response: nothing is left owed for the frame's drain to close it with.
    assert!(!app.world().resource::<crate::ui_loot::LootCloseAtRest>().0);
    owe(&mut app);
    assert_eq!(
        frame(&mut app, vec![fished(&[(1, POTION)])]),
        ["LOOT_CLOSED:nil", "LOOT_OPENED:nil"]
    );
    assert_eq!(open(&app), Some(BOBBER));
}
