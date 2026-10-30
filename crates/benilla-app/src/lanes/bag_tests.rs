//! The bag, lock, bank-slot and money events in the packets' order. Each fires inside the run of
//! the watcher whose field moved (`0x465570` → `0x4655dc`) or inside its packet's handler
//! (`0x537aa0`), so it comes out whole before a later packet's events and after an earlier one's
//! (`0x5384d0`).

use benilla_protocol::field::{
    FIELD_CONTAINER_SLOT_1, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_FIELD_COINAGE,
    FIELD_PLAYER_INV_SLOT_HEAD,
};
use benilla_protocol::messages::{ObjectType, ACTION_KIND_ITEM};
use benilla_protocol::{ObjectFields, SessionEvent};
use benilla_ui::script::{ActionSlot, UiScript};
use bevy::prelude::*;

const ME: u64 = 0x7;
/// A stack of potions in the backpack's first seat, 23, on the bar's slot 5.
const POTIONS: u64 = 0x4000_0000_0000_0043;
/// A bag, equipped into seat 19 (bag 1) by the fixture's last packet.
const BAG: u64 = 0x4000_0000_0000_0044;
/// Linen in the bag's first slot.
const INNER: u64 = 0x4000_0000_0000_0045;
/// Linen in the bank's first seat, 39.
const BANKED: u64 = 0x4000_0000_0000_0046;
const POTION: u32 = 118;
const LINEN: u32 = 2589;
const BAG_ID: u32 = 4496;
/// `PLAYER_FIELD_BYTES`, whose byte 1 is the combo count (`0x5ddff0`).
const PLAYER_FIELD_BYTES: u16 = 188 + 0x1028 / 4;
/// `EQUIP_ERR_BAG_FULL`, whose line `ERR_BAG_FULL` the fixture's VM defines.
const REFUSAL: u8 = 4;

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

fn guid_pair(lo: u16, guid: u64) -> [(u16, u32); 2] {
    [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
}

/// The player's flat seat `n` (`PLAYER_FIELD_INV_SLOT_HEAD + 2n`).
fn seat(n: u16, guid: u64) -> [(u16, u32); 2] {
    guid_pair(FIELD_PLAYER_INV_SLOT_HEAD + 2 * n, guid)
}

/// An item of ours, held by `contained`.
fn item(guid: u64, entry: u32, contained: u64) -> SessionEvent {
    let mut pairs = vec![
        (2, 0x3),
        (FIELD_OBJECT_ENTRY, entry),
        (FIELD_ITEM_STACK_COUNT, 1),
    ];
    pairs.extend(guid_pair(6, ME));
    pairs.extend(guid_pair(8, contained));
    SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Item),
    }
}

/// The bag, four slots, [`INNER`] in the first.
fn bag() -> SessionEvent {
    let mut pairs = vec![(2, 0x7), (FIELD_OBJECT_ENTRY, BAG_ID), (48, 4)];
    pairs.extend(guid_pair(6, ME));
    pairs.extend(guid_pair(8, ME));
    pairs.extend(guid_pair(FIELD_CONTAINER_SLOT_1, INNER));
    SessionEvent::ItemCreate {
        guid: BAG,
        container: true,
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Container),
    }
}

fn values(guid: u64, pairs: &[(u16, u32)]) -> SessionEvent {
    SessionEvent::ObjectValues {
        guid,
        fields: ObjectFields::from_pairs(pairs),
    }
}

/// The combo count lands on the active player: its watcher's event (`0x5ddffa`), the other
/// packet each golden interleaves with.
fn combo(n: u32) -> SessionEvent {
    values(ME, &[(PLAYER_FIELD_BYTES, n << 8)])
}

/// The client in the world as [`ME`] with the potions in the backpack, the linen in the bank and
/// the bag equipped, and a VM whose spy records the bag, lock, bank and money events with `arg1`.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
    let world = app.world_mut();
    let pairs: Vec<(u16, u32)> = seat(23, POTIONS)
        .into_iter()
        .chain(seat(39, BANKED))
        .collect();
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
    for (entry, name) in [(POTION, "Potion"), (LINEN, "Linen Cloth"), (BAG_ID, "Bag")] {
        let stackable = if entry == BAG_ID { 1 } else { 20 };
        world.resource_mut::<crate::items::Items>().insert_template(
            entry,
            Some(benilla_protocol::messages::ItemInfo {
                stackable,
                ..crate::items::test_template(name)
            }),
        );
    }
    crate::ui_action::hold_for_test(world, 5, ACTION_KIND_ITEM, POTION);
    let mut script = UiScript::new().unwrap();
    script.set_action(
        5,
        Some(ActionSlot {
            kind: ACTION_KIND_ITEM,
            action: POTION,
            ..Default::default()
        }),
    );
    script
        .run(
            r#"
            ERR_BAG_FULL = "Refused."
            Heard = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"BAG_UPDATE", "BAG_CLOSED", "BAG_OPEN", "ITEM_LOCK_CHANGED",
                    "PLAYERBANKSLOTS_CHANGED", "PLAYER_MONEY", "PLAYER_COMBO_POINTS",
                    "ACTIONBAR_SLOT_CHANGED", "UNIT_INVENTORY_CHANGED", "UI_ERROR_MESSAGE",
                    "CHAT_MSG_MONSTER_SAY", "LOOT_CLOSED"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Heard, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    world.insert_non_send_resource(script);
    frame(
        &mut app,
        vec![
            item(POTIONS, POTION, ME),
            item(BANKED, LINEN, ME),
            item(INNER, LINEN, BAG),
            bag(),
            values(ME, &seat(19, BAG)),
        ],
    );
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

/// The stack count watcher (`0x5d9400`) on a stackable item unlocks its entry with one
/// `ITEM_LOCK_CHANGED` first (`0x5d94af`, `0x5d94b9`), recounts the bar (`0x5d94e1`), then
/// signals `BAG_UPDATE` for the item's container, the backpack's 0 (`0x5d94ff` → `0x4f9480` →
/// `0x4f9370`), all inside the potions' update.
#[test]
fn a_restack_unlocks_recounts_and_updates_its_bag_at_its_packet() {
    let mut app = client();
    let restack = |n: u32| values(POTIONS, &[(FIELD_ITEM_STACK_COUNT, n)]);
    assert_eq!(
        frame(&mut app, vec![restack(5), combo(2)]),
        [
            "ITEM_LOCK_CHANGED:nil",
            "ACTIONBAR_SLOT_CHANGED:5",
            "BAG_UPDATE:0",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
    assert_eq!(
        frame(&mut app, vec![combo(0), restack(6)]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ITEM_LOCK_CHANGED:nil",
            "ACTIONBAR_SLOT_CHANGED:5",
            "BAG_UPDATE:0"
        ]
    );
}

/// A bag's slot watcher (`0x4f90d0`) unlocks the item now in the slot (`0x4f912c`), none for an
/// emptied one, and signals the bag's `BAG_UPDATE` from its index in the bag cache (`0x4f918e` →
/// `0x4f9370`), slot by slot inside the bag's update.
#[test]
fn a_bags_slot_watchers_unlock_the_arrival_and_update_the_bag_at_its_packet() {
    let mut app = client();
    let lo = FIELD_CONTAINER_SLOT_1;
    let moved: Vec<(u16, u32)> = guid_pair(lo, 0)
        .into_iter()
        .chain(guid_pair(lo + 2, INNER))
        .collect();
    assert_eq!(
        frame(&mut app, vec![values(BAG, &moved), combo(2)]),
        [
            "BAG_UPDATE:1",
            "ITEM_LOCK_CHANGED:nil",
            "BAG_UPDATE:1",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
    let back: Vec<(u16, u32)> = guid_pair(lo, INNER)
        .into_iter()
        .chain(guid_pair(lo + 2, 0))
        .collect();
    assert_eq!(
        frame(&mut app, vec![combo(0), values(BAG, &back)]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ITEM_LOCK_CHANGED:nil",
            "BAG_UPDATE:1",
            "BAG_UPDATE:1"
        ]
    );
}

/// A bag seat's watcher (`0x4f8ec0`) rescans the ten bags against the cache (`0x4f91a0`): a bag
/// gone fires `BAG_CLOSED` alone (`0x4f92b5`, `0x4f92cc`); one arriving fires `BAG_UPDATE`
/// (`0x4f9343`), after the inventory watcher's unlock of the arrival at the same seat
/// (`0x5ddcf0`, registered first, `0x5ddd50`).
#[test]
fn a_bag_seat_closes_and_updates_its_bag_at_its_packet() {
    let mut app = client();
    assert_eq!(
        frame(&mut app, vec![combo(2), values(ME, &seat(19, 0))]),
        ["PLAYER_COMBO_POINTS:nil", "BAG_CLOSED:1"]
    );
    assert_eq!(
        frame(&mut app, vec![values(ME, &seat(19, BAG)), combo(0)]),
        [
            "ITEM_LOCK_CHANGED:nil",
            "BAG_UPDATE:1",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
    assert_eq!(
        frame(&mut app, vec![values(ME, &seat(19, 0)), combo(2)]),
        ["BAG_CLOSED:1", "PLAYER_COMBO_POINTS:nil"]
    );
    assert_eq!(
        frame(&mut app, vec![combo(0), values(ME, &seat(19, BAG))]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ITEM_LOCK_CHANGED:nil",
            "BAG_UPDATE:1"
        ]
    );
}

/// The inventory watcher (`0x5ddcf0`) on a bank seat unlocks the item now in it (`0x5ddd50`),
/// none for an emptied one, and signals `PLAYERBANKSLOTS_CHANGED` with no argument
/// (`0x5ddd69`), seat by seat inside the player's update.
#[test]
fn a_bank_seat_signals_the_bank_slots_at_its_packet() {
    let mut app = client();
    let moved: Vec<(u16, u32)> = seat(39, 0).into_iter().chain(seat(40, BANKED)).collect();
    assert_eq!(
        frame(&mut app, vec![combo(2), values(ME, &moved)]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "PLAYERBANKSLOTS_CHANGED:nil",
            "ITEM_LOCK_CHANGED:nil",
            "PLAYERBANKSLOTS_CHANGED:nil"
        ]
    );
    let back: Vec<(u16, u32)> = seat(39, BANKED).into_iter().chain(seat(40, 0)).collect();
    assert_eq!(
        frame(&mut app, vec![values(ME, &back), combo(0)]),
        [
            "ITEM_LOCK_CHANGED:nil",
            "PLAYERBANKSLOTS_CHANGED:nil",
            "PLAYERBANKSLOTS_CHANGED:nil",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
}

/// A banked item's restack: the entry unlock (`0x5d94af`), the item's container's `BAG_UPDATE`,
/// the player's 0 (`0x5d94ff`), then `NotifyItemSlotChanged`'s bank arm,
/// `PLAYERBANKSLOTS_CHANGED` with `"player"` (`0x5d950e` → `0x4c728d`), inside its update.
#[test]
fn a_banked_items_restack_signals_the_bank_slots_with_player_at_its_packet() {
    let mut app = client();
    assert_eq!(
        frame(
            &mut app,
            vec![values(BANKED, &[(FIELD_ITEM_STACK_COUNT, 3)]), combo(2)]
        ),
        [
            "ITEM_LOCK_CHANGED:nil",
            "BAG_UPDATE:0",
            "PLAYERBANKSLOTS_CHANGED:player",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
    assert_eq!(
        frame(
            &mut app,
            vec![combo(0), values(BANKED, &[(FIELD_ITEM_STACK_COUNT, 4)])]
        ),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ITEM_LOCK_CHANGED:nil",
            "BAG_UPDATE:0",
            "PLAYERBANKSLOTS_CHANGED:player"
        ]
    );
}

/// The coinage watcher signals `PLAYER_MONEY` (`0x5ddf4b`) inside the update that moved the purse.
#[test]
fn the_coinage_watcher_signals_the_money_at_its_packet() {
    let mut app = client();
    let purse = |copper: u32| values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, copper)]);
    assert_eq!(
        frame(&mut app, vec![purse(500), combo(2)]),
        ["PLAYER_MONEY:nil", "PLAYER_COMBO_POINTS:nil"]
    );
    assert_eq!(
        frame(&mut app, vec![combo(0), purse(400)]),
        ["PLAYER_COMBO_POINTS:nil", "PLAYER_MONEY:nil"]
    );
}

/// `SMSG_OPEN_CONTAINER` signals `BAG_OPEN` for the bag's index in the cache (`0x5e3b49` →
/// `0x4f9469`) inside its handler.
#[test]
fn a_server_opened_bag_signals_at_its_packet() {
    let mut app = client();
    assert_eq!(
        frame(
            &mut app,
            vec![combo(2), SessionEvent::OpenContainer { item: BAG }]
        ),
        ["PLAYER_COMBO_POINTS:nil", "BAG_OPEN:1"]
    );
    assert_eq!(
        frame(
            &mut app,
            vec![SessionEvent::OpenContainer { item: BAG }, combo(0)]
        ),
        ["BAG_OPEN:1", "PLAYER_COMBO_POINTS:nil"]
    );
}

/// An inventory refusal shows its line (`0x496720`) and unlocks the item it names (`0x5e3ac0`)
/// inside its handler.
#[test]
fn an_inventory_refusal_shows_and_unlocks_at_its_packet() {
    let mut app = client();
    let refused = || SessionEvent::InventoryFailure {
        reason: REFUSAL,
        required_level: None,
        item_guid: POTIONS,
        item2_guid: 0,
        bag_slot: 0xff,
    };
    assert_eq!(
        frame(&mut app, vec![combo(2), refused()]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "UI_ERROR_MESSAGE:Refused.",
            "ITEM_LOCK_CHANGED:nil"
        ]
    );
    assert_eq!(
        frame(&mut app, vec![refused(), combo(0)]),
        [
            "UI_ERROR_MESSAGE:Refused.",
            "ITEM_LOCK_CHANGED:nil",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
}

/// A refusal for a specialised bag names the bag's family as its handler finds the bag
/// (`0x5e3a1e` → `0x5ede00`), not as a later packet of the frame leaves the seat.
#[test]
fn a_wrong_bag_refusal_names_the_bag_its_packet_found() {
    let mut app = client();
    let world = app.world_mut();
    world.insert_resource(crate::ui_items::ItemBagFamilies(
        benilla_formats::ItemBagFamilyCatalog::from_names(std::collections::HashMap::from([(
            1,
            "Arrows".to_string(),
        )])),
    ));
    world.resource_mut::<crate::items::Items>().insert_template(
        BAG_ID,
        Some(benilla_protocol::messages::ItemInfo {
            stackable: 1,
            bag_family: 1,
            ..crate::items::test_template("Quiver")
        }),
    );
    world
        .non_send_resource_mut::<UiScript>()
        .run(
            "ERR_WRONG_BAG_TYPE = 'Wrong bag.' \
             ERR_WRONG_BAG_TYPE_SUBCLASS = 'Only %s can be placed in that.'",
        )
        .unwrap();
    let refused = SessionEvent::InventoryFailure {
        reason: 16,
        required_level: None,
        item_guid: 0,
        item2_guid: 0,
        bag_slot: 19,
    };
    let lines: Vec<String> = frame(&mut app, vec![refused, values(ME, &seat(19, 0))])
        .into_iter()
        .filter(|e| e.starts_with("UI_ERROR_MESSAGE"))
        .collect();
    assert_eq!(
        lines,
        ["UI_ERROR_MESSAGE:Only Arrows can be placed in that."]
    );
}

/// A refused sale unlocks the item it names inside its handler (`0x5dd26b`).
#[test]
fn a_refused_sales_unlock_fires_at_its_packet() {
    let mut app = client();
    let refused = || SessionEvent::VendorSellFailed {
        vendor: 0xF130_0000_0000_0001,
        item_guid: POTIONS,
        reason: 1,
    };
    assert_eq!(
        frame(&mut app, vec![combo(2), refused()]),
        ["PLAYER_COMBO_POINTS:nil", "ITEM_LOCK_CHANGED:nil"]
    );
    assert_eq!(
        frame(&mut app, vec![refused(), combo(0)]),
        ["ITEM_LOCK_CHANGED:nil", "PLAYER_COMBO_POINTS:nil"]
    );
}

/// A loot error for a guid the window is not on takes the refusal leg (`0x5eb944` → `0x5eb963`)
/// and unlocks the guid inside its handler (`0x5eb9de`).
#[test]
fn a_loot_refusals_unlock_fires_at_its_packet() {
    let mut app = client();
    let refused = || SessionEvent::LootError {
        guid: POTIONS,
        error: 8,
    };
    assert_eq!(
        frame(&mut app, vec![combo(2), refused()]),
        ["PLAYER_COMBO_POINTS:nil", "ITEM_LOCK_CHANGED:nil"]
    );
    assert_eq!(
        frame(&mut app, vec![refused(), combo(0)]),
        ["ITEM_LOCK_CHANGED:nil", "PLAYER_COMBO_POINTS:nil"]
    );
}

/// A window open on an item of ours, the potions, as an opened lockbox's is, latched; what the
/// open signalled is spent.
fn loot_open_on_the_potions(app: &mut App) {
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_loot::LootState>()
        .open(POTIONS, 1, 0, Vec::new());
    world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(POTIONS);
    let _ = frame(app, Vec::new());
}

/// The server's release of a window open on an item closes it inside its handler (`0x5ec090` →
/// `0x48f200`): the close's `UnlockItem` (`0x48f2bd`), the teardown's (`0x4c1d36`), then the
/// teardown's `LOOT_CLOSED` (`0x4c1dac`), between the packets around it.
#[test]
fn a_released_item_windows_two_unlocks_fire_at_its_packet() {
    let mut app = client();
    loot_open_on_the_potions(&mut app);
    let released = SessionEvent::LootReleaseResponse {
        guid: POTIONS,
        result: 1,
    };
    assert_eq!(
        frame(&mut app, vec![combo(2), released, combo(0)]),
        [
            "PLAYER_COMBO_POINTS:nil",
            "ITEM_LOCK_CHANGED:nil",
            "ITEM_LOCK_CHANGED:nil",
            "LOOT_CLOSED:nil",
            "PLAYER_COMBO_POINTS:nil"
        ]
    );
}

/// A monster's line with no `$`-macro, which shows inside its own packet's handler (`0x49dac2`).
fn boar_says() -> SessionEvent {
    SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: 0,
        sender_name: Some("Boar".into()),
        channel: None,
        text: "Grr".into(),
        chat_tag: 0,
    })
}

/// A restack's events and another packet's chat line come out in the packets' order, whichever
/// lane holds each.
#[test]
fn a_restacks_events_and_a_chat_line_keep_their_packets_order() {
    let mut app = client();
    let restack = |n: u32| values(POTIONS, &[(FIELD_ITEM_STACK_COUNT, n)]);
    assert_eq!(
        frame(&mut app, vec![boar_says(), restack(5)]),
        [
            "CHAT_MSG_MONSTER_SAY:Grr",
            "ITEM_LOCK_CHANGED:nil",
            "ACTIONBAR_SLOT_CHANGED:5",
            "BAG_UPDATE:0"
        ]
    );
    assert_eq!(
        frame(&mut app, vec![restack(6), boar_says()]),
        [
            "ITEM_LOCK_CHANGED:nil",
            "ACTIONBAR_SLOT_CHANGED:5",
            "BAG_UPDATE:0",
            "CHAT_MSG_MONSTER_SAY:Grr"
        ]
    );
}

/// `chunk` on the VM with the host lent, as a getter runs inside a frame.
fn hosted_eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").eval::<T>(chunk).expect("runs")
    })
}

/// A bag getter reads the bags as its listener's packet left them (`GetContainerItemInfo` finds
/// the slot in the player's list at the call, `0x6228a0`): the potions leave the backpack's first
/// seat and come back in one frame, and each seat's `BAG_UPDATE` reads its own packet's seat, not
/// the frame's last.
#[test]
fn a_bag_updates_listener_reads_the_seat_its_packet_left() {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("BAG_UPDATE")
            f:SetScript("OnEvent", function()
                if arg1 == 0 then
                    table.insert(Seen, GetContainerItemLink(0, 1) and "held" or "empty")
                end
            end)
            "#,
        )
        .unwrap();
    frame(
        &mut app,
        vec![
            values(ME, &seat(23, 0)),
            combo(2),
            values(ME, &seat(23, POTIONS)),
        ],
    );
    let seen: Vec<String> = hosted_eval(&mut app, "return Seen");
    assert_eq!(seen, ["empty", "held"]);
}

/// What the text `expr` reads before the logout, and then inside a `PLAYER_LOGOUT` handler as the
/// logout runs it: `SMSG_LOGOUT_COMPLETE` and the disconnect through the handler table, which
/// free the objects, then the session's end, whose shutdown signals the event with the host lent.
fn before_and_at_logout(app: &mut App, expr: &str) -> (String, String) {
    use std::cell::RefCell;
    use std::rc::Rc;
    let before: String = hosted_eval(app, &format!("return {expr}"));
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = seen.clone();
    {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let note = script
            .lua()
            .create_function(move |_, text: String| {
                sink.borrow_mut().push(text);
                Ok(())
            })
            .unwrap();
        script.lua().globals().set("Note", note).unwrap();
        script
            .run(&format!(
                "local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_LOGOUT') \
                 f:SetScript('OnEvent', function() \
                     local ok, v = pcall(function() return {expr} end) \
                     Note(ok and v or ('raised ' .. tostring(v))) \
                 end)"
            ))
            .unwrap();
    }
    frame(
        app,
        vec![
            SessionEvent::LoggedOut,
            SessionEvent::Disconnected {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
            },
        ],
    );
    let world = app.world_mut();
    let held = world
        .query_filtered::<(), With<crate::net::SelfPlayer>>()
        .iter(world)
        .count();
    assert_eq!(held, 0, "the avatar is gone");
    assert!(
        world.resource::<crate::net::GuidIndex>().0.is_empty(),
        "every object is gone"
    );
    crate::ui_script::end_ui_session(world);
    let at = seen.borrow().clone();
    assert_eq!(at.len(), 1, "one PLAYER_LOGOUT: {at:?}");
    (before, at[0].clone())
}

/// A logout's shutdown finds no bags: `SMSG_LOGOUT_COMPLETE` (`0x5aaeb0`) tears the game down
/// through `0x401ee0`, which destroys the object manager (`0x402011` → `0x467700`, storing 0 at
/// `0xb41414`, `0x46771e`) before the interface shutdown (`0x402039` → `0x491180` →
/// `0x490bd0`), so `PLAYER_LOGOUT` (`0x490c2a`) runs with no active player (`0x468550`), and
/// `GetContainerNumSlots` returns 0 for every bag (`0x4f95ab`).
#[test]
fn the_logouts_shutdown_reads_no_bags() {
    let mut app = client();
    let (before, at) = before_and_at_logout(
        &mut app,
        "tostring(((function() local _, n = GetContainerItemInfo(0, 1) return n end)())) .. ' ' .. \
         GetContainerNumSlots(0) .. ' ' .. GetContainerNumSlots(1) .. ' ' .. \
         tostring((GetContainerItemLink(1, 1)))",
    );
    assert!(before.starts_with("1 16 4 |c"), "{before}");
    assert_eq!(at, "nil 0 0 nil");
}

/// The same shutdown finds no inventory: the slot getters ask `CanReadUnitInventory`
/// (`0x4c8ca7`), which finds no active player (`0x4944af` → `0x468460`, false at `0x4944c9`), so
/// `GetInventoryItemLink` returns nothing (`0x4c8cae` → `0x4c8c56`). `UnitName("player")` reads
/// the selected character's name, no object (`0x51708c` → `0x5abdc0`), and `UnitHealth`
/// finds no unit, 0.
#[test]
fn the_logouts_shutdown_reads_no_inventory_but_the_name() {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_player_record(benilla_ui::script::PlayerRecord {
            name: "Onehunter".into(),
            ..Default::default()
        });
    frame(
        &mut app,
        vec![values(
            ME,
            &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 70)],
        )],
    );
    let (before, at) = before_and_at_logout(
        &mut app,
        "tostring((GetInventoryItemLink('player', 20))) .. ' ' .. \
         tostring((UnitName('player'))) .. ' ' .. UnitHealth('player')",
    );
    assert!(before.starts_with("|c"), "{before}");
    assert!(before.ends_with(" Onehunter 70"), "{before}");
    assert_eq!(at, "nil Onehunter 0");
}

/// `OffhandHasWeapon` looks the active player up first (`0x4c9578` → `0x468460`) and pushes nil
/// when none is held (`0x4c9590`, `je 0x4c95c4`): at `PLAYER_LOGOUT`, after the object manager is
/// gone (`0x467700`, `0x46771e`), a weapon in the off hand still reads nil.
#[test]
fn the_logouts_shutdown_finds_no_off_hand_weapon() {
    const SWORD: u64 = 0x4000_0000_0000_0049;
    let mut app = client();
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(
            25,
            Some(benilla_protocol::messages::ItemInfo {
                class: 2,
                ..crate::items::test_template("Sword")
            }),
        );
    // The VM's record copy, which the snapshot getter reads once the objects are gone.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_item_template(
            25,
            benilla_ui::script::ItemTemplateView {
                class: 2,
                ..Default::default()
            },
        );
    frame(
        &mut app,
        vec![item(SWORD, 25, ME), values(ME, &seat(16, SWORD))],
    );
    let (before, at) = before_and_at_logout(&mut app, "tostring(OffhandHasWeapon())");
    assert_eq!(before, "1");
    assert_eq!(at, "nil");
}

/// A spy on `PLAYER_COMBO_POINTS` that records `chunk`'s answer at each firing, as text.
fn spy_at_combo(app: &mut App, chunk: &str) {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(&format!(
            r#"AtCombo = {{}}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_COMBO_POINTS")
            f:SetScript("OnEvent", function()
                table.insert(AtCombo, tostring(({chunk})))
            end)"#
        ))
        .unwrap();
}

/// A bag's slot array as its packet left it: the linen moved out of the bag's first slot in one
/// frame and back in the next, behind a combo packet. The combo's listener finds the slot empty,
/// as the reference's `GetContainerItemLink` reads the bag object's slot at the call (`0x4f997f`
/// → `0x6228a0`), not the later packet's.
#[test]
fn a_bags_slot_reads_as_its_packet_left_it() {
    let mut app = client();
    let lo = FIELD_CONTAINER_SLOT_1;
    let moved: Vec<(u16, u32)> = guid_pair(lo, 0)
        .into_iter()
        .chain(guid_pair(lo + 2, INNER))
        .collect();
    frame(&mut app, vec![values(BAG, &moved)]);
    spy_at_combo(&mut app, "GetContainerItemLink(1, 1)");
    let back: Vec<(u16, u32)> = guid_pair(lo, INNER)
        .into_iter()
        .chain(guid_pair(lo + 2, 0))
        .collect();
    frame(&mut app, vec![combo(2), values(BAG, &back)]);
    let seen: Vec<String> = hosted_eval(&mut app, "return AtCombo");
    assert_eq!(seen, ["nil"]);
    let now: Option<String> = hosted_eval(&mut app, "return GetContainerItemLink(1, 1)");
    assert!(now.is_some(), "the frame's end reads the linen back");
}

/// An item's stack count as its packet left it (`ITEM_FIELD_STACK_COUNT`, `0x4f9742`): a combo
/// packet ahead of a restack reads the old count.
#[test]
fn a_stack_count_reads_as_its_packet_left_it() {
    let mut app = client();
    spy_at_combo(
        &mut app,
        "(function() local _, n = GetContainerItemInfo(0, 1) return n end)()",
    );
    frame(
        &mut app,
        vec![
            combo(2),
            values(POTIONS, &[(FIELD_ITEM_STACK_COUNT, 5)]),
            combo(0),
        ],
    );
    let seen: Vec<String> = hosted_eval(&mut app, "return AtCombo");
    assert_eq!(seen, ["1", "5"]);
}

/// A bank bag's slot array as its packet left it, with the banker open: linen put into the bank
/// bag's first slot behind a combo packet is not there to the combo's listener (`0x4f93e0` finds
/// the bag by its seat `PLAYER_FIELD_BANKBAG_SLOT_1`, `0x6228a0` reads its slot).
#[test]
fn a_bank_bags_slot_reads_as_its_packet_left_it() {
    const BANK_BAG: u64 = 0x4000_0000_0000_0047;
    const BANK_LINEN: u64 = 0x4000_0000_0000_0048;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_bank(Some(benilla_ui::script::BankState::default()));
    let mut pairs = vec![(2, 0x7), (FIELD_OBJECT_ENTRY, BAG_ID), (48, 4)];
    pairs.extend(guid_pair(6, ME));
    pairs.extend(guid_pair(8, ME));
    let bank_bag = SessionEvent::ItemCreate {
        guid: BANK_BAG,
        container: true,
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Container),
    };
    frame(
        &mut app,
        vec![
            bank_bag,
            item(BANK_LINEN, LINEN, BANK_BAG),
            values(ME, &seat(63, BANK_BAG)),
        ],
    );
    spy_at_combo(&mut app, "GetContainerItemLink(5, 1)");
    frame(
        &mut app,
        vec![
            combo(2),
            values(BANK_BAG, &guid_pair(FIELD_CONTAINER_SLOT_1, BANK_LINEN)),
        ],
    );
    let seen: Vec<String> = hosted_eval(&mut app, "return AtCombo");
    assert_eq!(seen, ["nil"]);
    let now: Option<String> = hosted_eval(&mut app, "return GetContainerItemLink(5, 1)");
    assert!(
        now.is_some(),
        "the frame's end reads the linen in the bank bag"
    );
}
