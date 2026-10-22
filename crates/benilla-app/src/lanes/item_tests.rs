//! The item watchers' events in the packets' order. A watcher's callback runs inside the notify
//! pass of the update that moved its field (`0x465570` → `0x4655dc`), so its events fire inside
//! that packet's handler (`0x537aa0`), in the packets' order (`0x5384d0`): whole before a later
//! packet's events and after an earlier one's.

use benilla_protocol::field::{
    FIELD_ITEM_SPELL_CHARGES, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
    FIELD_PLAYER_INV_SLOT_HEAD,
};
use benilla_protocol::messages::{ObjectType, ACTION_KIND_ITEM};
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
    let mut app = crate::game_plugins::schedule_tests::headless_client();
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
            Heard = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"ACTIONBAR_SLOT_CHANGED", "BAG_UPDATE_COOLDOWN",
                    "UNIT_INVENTORY_CHANGED", "TRADE_UPDATE", "PLAYER_COMBO_POINTS"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Heard, event .. ":" .. tostring(arg1))
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
