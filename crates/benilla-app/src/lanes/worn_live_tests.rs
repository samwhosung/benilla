//! The worn slots through the whole client: the slot getters look the item up in the player's
//! list as they run (`GetItemInSlot` `0x6228a0`; `GetInventoryItemLink` at `0x4c8cd3`), and
//! `GetWeaponEnchantInfo` its weapons and their deadlines (`0x4c97ca`, `0x5d9d00`), so a
//! listener reads the doll its own packet left: an equip's `UNIT_INVENTORY_CHANGED` reads the
//! item its packet seated, and a line between two writes reads the first.

use benilla_protocol::field::{FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR};
use benilla_protocol::messages::{ChatMessage, ObjectType, CHAT_MSG_SYSTEM};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Sender;

use super::unit_view_tests::{created, values, wire_packets};

const ME: u64 = 0x7;
/// Worn on the head, seat 0, at the start.
const HELM: (u64, u32) = (item(1), 7366);
/// In the backpack's first seat, 23, at the start.
const SPARE: (u64, u32) = (item(2), 2361);
/// In the main hand, seat 15, with a temporary enchantment of five charges.
const SWORD: (u64, u32) = (item(3), 25);
/// In the first bank bag slot at the start, and the bag that takes its place.
const BAG: (u64, u32) = (item(4), 4500);
const OTHER_BAG: (u64, u32) = (item(5), 4501);

/// `ITEM_FIELD_ENCHANTMENT` slot 1, the temporary one: its id and its charges.
const TEMP_ENCHANT: u16 = 25;
const TEMP_CHARGES: u16 = 27;
/// `ITEM_FIELD_STACK_COUNT`.
const STACK: u16 = 14;
/// `PLAYER_FIELD_BANK_BAG_SLOT_1`.
const BANK_BAG_1: u16 = 612;
/// `PLAYER_VISIBLE_ITEM_1_0`, the head's shown entry.
const VISIBLE_HEAD: u16 = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2;

const fn item(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The spy: each `UNIT_INVENTORY_CHANGED` with the head's entry as it fires, and each system
/// line with the main hand's temporary charges and the bank bag's entry as it shows.
const SPY: &str = r#"Seen = {}
    local function entry(slot)
        local _, _, id = string.find(GetInventoryItemLink("player", slot) or "", "item:(%d+)")
        return id or "none"
    end
    local f = CreateFrame("Frame")
    f:RegisterEvent("UNIT_INVENTORY_CHANGED")
    f:RegisterEvent("CHAT_MSG_SYSTEM")
    f:SetScript("OnEvent", function()
        if event == "UNIT_INVENTORY_CHANGED" then
            table.insert(Seen, "INV:" .. entry(1))
        else
            local _, _, charges = GetWeaponEnchantInfo()
            table.insert(Seen, arg1 .. ":" .. tostring(charges) .. ":" .. entry(64))
        end
    end)"#;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
}

impl Client {
    /// One frame, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        crate::script_calls::eval_in_vm(self.app.world(), "local s = Seen Seen = {} return s")
    }
}

/// One of our items, with `extra` fields.
fn item_create((guid, entry): (u64, u32), container: bool, extra: &[(u16, u32)]) -> SessionEvent {
    let mut pairs = vec![
        (2, if container { 0x7 } else { 0x3 }),
        (3, entry),
        (6, ME as u32),
        (8, ME as u32),
        (STACK, 1),
    ];
    pairs.extend_from_slice(extra);
    SessionEvent::ItemCreate {
        guid,
        container,
        fields: ObjectFields::from_pairs(&pairs).into_created(if container {
            ObjectType::Container
        } else {
            ObjectType::Item
        }),
    }
}

/// `guid` in flat seat `seat` of our array.
fn seat(seat: u16, guid: u64) -> [(u16, u32); 2] {
    let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * seat;
    [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
}

fn system(text: &str) -> SessionEvent {
    SessionEvent::Chat(ChatMessage {
        chat_type: CHAT_MSG_SYSTEM,
        language: 0,
        sender_guid: 0,
        target_guid: 0,
        sender_name: None,
        channel: None,
        text: text.into(),
        chat_tag: 0,
    })
}

/// The client in the world as [`ME`], wearing [`HELM`] and [`SWORD`], carrying [`SPARE`], with
/// [`BAG`] in the first bank bag slot and a banker open; the spy emptied.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script.run(SPY).expect("the spy");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        for (_, entry) in [HELM, SPARE, SWORD, BAG, OTHER_BAG] {
            items.insert_template(entry, Some(crate::items::test_template("Gear")));
        }
    }
    let mut c = Client { app, inbound };
    let mut me = vec![(VISIBLE_HEAD, HELM.1)];
    me.extend(seat(0, HELM.0));
    me.extend(seat(23, SPARE.0));
    me.extend(seat(15, SWORD.0));
    me.extend([
        (BANK_BAG_1, BAG.0 as u32),
        (BANK_BAG_1 + 1, (BAG.0 >> 32) as u32),
    ]);
    c.frame(vec![
        vec![item_create(HELM, false, &[])],
        vec![item_create(SPARE, false, &[])],
        vec![item_create(
            SWORD,
            false,
            &[(TEMP_ENCHANT, 7), (TEMP_CHARGES, 5)],
        )],
        vec![item_create(BAG, true, &[])],
        vec![item_create(OTHER_BAG, true, &[])],
        vec![created(ME, EntityKind::Player, &me)],
    ]);
    // The banker's cell in the VM, which the open writes beside it (`0x4f850e`), for the bank
    // bags' gate (`0x6228ad`).
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .set_bank(Some(benilla_ui::script::BankState::default()))
    });
    c.frame(vec![]);
    c.seen();
    c
}

/// A swap of the head and the backpack seat and its swap back in one frame, each packet seating
/// the item and showing its entry as an equip's does: each packet's visible-item watch signals
/// `UNIT_INVENTORY_CHANGED` there, and its listener reads the head its own packet seated, not the
/// frame's last.
#[test]
fn an_equips_listener_reads_the_item_its_packet_seated() {
    let mut c = client();
    let wear = |head: (u64, u32), spare: (u64, u32)| -> Vec<(u16, u32)> {
        let mut pairs = vec![(VISIBLE_HEAD, head.1)];
        pairs.extend(seat(0, head.0));
        pairs.extend(seat(23, spare.0));
        pairs
    };
    c.frame(vec![
        vec![values(ME, &wear(SPARE, HELM))],
        vec![values(ME, &wear(HELM, SPARE))],
    ]);
    let heard: Vec<String> = c.seen();
    assert_eq!(
        heard,
        [format!("INV:{}", SPARE.1), format!("INV:{}", HELM.1)]
    );
}

/// Two writes of the main hand's temporary charges and of the first bank bag slot, and a line
/// after each, in one frame: each line reads `GetWeaponEnchantInfo`'s charges and the bank bag's
/// item as the packets before it left them (`0x4c97ca`, `0x4c8cd3`).
#[test]
fn a_line_between_two_writes_reads_the_first() {
    let mut c = client();
    let bank_bag = |guid: u64| {
        [
            (BANK_BAG_1, guid as u32),
            (BANK_BAG_1 + 1, (guid >> 32) as u32),
        ]
    };
    c.frame(vec![
        vec![values(SWORD.0, &[(TEMP_CHARGES, 3)])],
        vec![values(ME, &bank_bag(OTHER_BAG.0))],
        vec![system("between")],
        vec![values(SWORD.0, &[(TEMP_CHARGES, 1)])],
        vec![values(ME, &bank_bag(BAG.0))],
        vec![system("after")],
    ]);
    let lines: Vec<String> = c
        .seen()
        .into_iter()
        .filter(|l| !l.starts_with("INV:"))
        .collect();
    assert_eq!(
        lines,
        [
            format!("between:3:{}", OTHER_BAG.1),
            format!("after:1:{}", BAG.1)
        ]
    );
}

/// The second spy: each system line with the main hand's temporary enchantment's time left, long
/// past five minutes or short of it.
const LEFT_SPY: &str = r#"Left = {}
    local f = CreateFrame("Frame")
    f:RegisterEvent("CHAT_MSG_SYSTEM")
    f:SetScript("OnEvent", function()
        local _, left = GetWeaponEnchantInfo()
        local span = left and (left > 300000 and "long" or "short") or "none"
        table.insert(Left, arg1 .. ":" .. span)
    end)"#;

/// Two enchant-time updates of the main hand in one frame, and a line after each: each line reads
/// `GetWeaponEnchantInfo`'s time left as the packets before it set the deadline (`0x5d9cc0` in its
/// packet, read at the call by `0x5d9d00`), not as the frame's last left it.
#[test]
fn a_line_between_two_enchant_times_reads_the_first() {
    let mut c = client();
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM").run(LEFT_SPY).expect("the spy")
    });
    let time = |seconds| SessionEvent::ItemEnchantTime {
        item_guid: SWORD.0,
        slot: 1,
        seconds,
    };
    c.frame(vec![
        vec![time(600)],
        vec![system("between")],
        vec![time(60)],
        vec![system("after")],
    ]);
    let lines: Vec<String> =
        crate::script_calls::eval_in_vm(c.app.world(), "local s = Left Left = {} return s");
    assert_eq!(lines, ["between:long", "after:short"]);
}
