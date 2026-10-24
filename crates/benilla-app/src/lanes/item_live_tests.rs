//! The item and bag getters read the item cache and the item objects as they run, through the
//! lent host, as the reference's read the slot's object and its record (`GetContainerItemInfo`,
//! `0x6228a0` then `0x55ba30`): the live answer and the frame's snapshot agree on every getter
//! wherever the snapshot is current, and the cache and the object answer where it is not.

use benilla_protocol::field::{
    FIELD_CONTAINER_SLOT_1, FIELD_ITEM_STACK_COUNT, FIELD_PLAYER_INV_SLOT_HEAD,
    FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR,
};
use benilla_protocol::messages::{
    MailAttachment, MailListEntry, ObjectType, TradeItem, TradeStatus, TradeStatusExtended,
    TRADE_SLOT_COUNT,
};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// A player in view: the inspected target and the trade partner.
const BOB: u64 = 0x8;
const MAILBOX: u64 = 0xF110_0000_0000_0001;
const BANKER: u64 = 0xF130_0000_0000_0002;
/// The display every record here shows, and its icon.
const DISPLAY: u32 = 950;
const ICON: &str = "Interface\\Icons\\INV_Sword_04";
/// A rolled item's permanent enchant, `ItemRandomProperties.dbc` row and suffix factor.
const ENCHANT: u32 = 1900;
const ROLL: u32 = 862;
const SUFFIX: u32 = 20;
/// The bag in bag slot 1, and how many slots it has.
const BAG: u32 = 4245;
const BAG_SLOTS: u32 = 4;
/// `CONTAINER_FIELD_NUM_SLOTS`.
const NUM_SLOTS: u16 = 48;

/// Where an item object of ours sits: an inventory slot (equipment 0-18, bags 19-22, backpack
/// 23-38, bank 39-62), or a slot of the bag in bag slot 1.
#[derive(Clone, Copy)]
enum Seat {
    Inventory(u8),
    InBag(u8),
}

/// An item object of ours: its guid, entry, stack, durability (current, max), its own extra
/// fields and its seat.
struct Held {
    guid: u64,
    entry: u32,
    stack: u32,
    durability: (u32, u32),
    extra: &'static [(u16, u32)],
    seat: Seat,
}

/// `ITEM_FIELD_ENCHANTMENT` slot 0, `ITEM_FIELD_PROPERTY_SEED` and `ITEM_FIELD_RANDOM_PROPERTIES_ID`
/// of a rolled item.
const ROLLED: &[(u16, u32)] = &[(22, ENCHANT), (43, SUFFIX), (44, ROLL)];

/// What the agreement reads: a rolled, enchanted sword in the main hand, a broken shield in the
/// off hand; linen, a scroll and an item whose record never came in the backpack; a bag in bag
/// slot 1 with linen in it; and a rolled sword in the bank.
const HELD: [Held; 8] = [
    Held {
        guid: item(1),
        entry: 25,
        stack: 1,
        durability: (30, 40),
        extra: ROLLED,
        seat: Seat::Inventory(15),
    },
    Held {
        guid: item(2),
        entry: 2129,
        stack: 1,
        durability: (0, 35),
        extra: &[],
        seat: Seat::Inventory(16),
    },
    Held {
        guid: item(3),
        entry: 2589,
        stack: 17,
        durability: (0, 0),
        extra: &[],
        seat: Seat::Inventory(23),
    },
    Held {
        guid: item(4),
        entry: 955,
        stack: 2,
        durability: (0, 0),
        extra: &[],
        seat: Seat::Inventory(24),
    },
    Held {
        guid: item(5),
        entry: 99_999,
        stack: 1,
        durability: (0, 0),
        extra: &[],
        seat: Seat::Inventory(25),
    },
    Held {
        guid: item(6),
        entry: BAG,
        stack: 1,
        durability: (0, 0),
        extra: &[(NUM_SLOTS, BAG_SLOTS)],
        seat: Seat::Inventory(19),
    },
    Held {
        guid: item(7),
        entry: 2589,
        stack: 5,
        durability: (0, 0),
        extra: &[],
        seat: Seat::InBag(0),
    },
    Held {
        guid: item(8),
        entry: 25,
        stack: 1,
        durability: (40, 40),
        extra: ROLLED,
        seat: Seat::Inventory(39),
    },
];

const fn item(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The whole client, headless, in the world as [`ME`] with [`HELD`] seated, [`BOB`] targeted
/// and trading, the bank and a mailbox open.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
}

impl Client {
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the snapshots alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// Every getter's answer for every slot, one line each.
    fn answers(&mut self, live: bool) -> Vec<String> {
        let entries = HELD
            .iter()
            .map(|h| h.entry.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let chunk = format!(
            "local out = {{}}
            local function note(what, ...)
                for i = 1, table.getn(arg) do arg[i] = tostring(arg[i]) end
                table.insert(out, what .. ' ' .. table.concat(arg, ','))
            end
            for _, bag in ipairs({{ 0, 1, -1 }}) do
                for slot = 1, 4 do
                    local at = bag .. ':' .. slot
                    note('GetContainerItemInfo ' .. at, GetContainerItemInfo(bag, slot))
                    note('GetContainerItemLink ' .. at, GetContainerItemLink(bag, slot))
                end
            end
            for _, unit in ipairs({{ 'player', 'target' }}) do
                for _, slot in ipairs({{ 16, 17, 0, 20 }}) do
                    local at = unit .. ':' .. slot
                    note('GetInventoryItemTexture ' .. at, GetInventoryItemTexture(unit, slot))
                    note('GetInventoryItemLink ' .. at, GetInventoryItemLink(unit, slot))
                    note('GetInventoryItemCount ' .. at, GetInventoryItemCount(unit, slot))
                    note('GetInventoryItemQuality ' .. at, GetInventoryItemQuality(unit, slot))
                    note('GetInventoryItemBroken ' .. at, GetInventoryItemBroken(unit, slot))
                end
            end
            note('OffhandHasWeapon', OffhandHasWeapon())
            for slot = 1, 2 do
                note('GetTradePlayerItemInfo ' .. slot, GetTradePlayerItemInfo(slot))
                note('GetTradePlayerItemLink ' .. slot, GetTradePlayerItemLink(slot))
                note('GetTradeTargetItemInfo ' .. slot, GetTradeTargetItemInfo(slot))
                note('GetTradeTargetItemLink ' .. slot, GetTradeTargetItemLink(slot))
            end
            note('GetInboxItem 1', GetInboxItem(1))
            note('GetSendMailItem', GetSendMailItem())
            for _, entry in ipairs({{ {entries} }}) do
                note('GetItemInfo ' .. entry, GetItemInfo(entry))
                note('GetItemInfo item:' .. entry, GetItemInfo('item:' .. entry .. ':0:0:0'))
            end
            note('GetItemInfo rolled', GetItemInfo('item:25:{ENCHANT}:{ROLL}:{SUFFIX}'))
            return out"
        );
        self.eval(live, &chunk)
    }

    /// `guid`'s store, written in place with no packet or frame, as a handler leaves it before the
    /// feeds have pushed.
    fn write_store(&mut self, guid: u64, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&guid];
        let mut store = world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("a store");
        store.0.merge(ObjectFields::from_pairs(pairs));
    }

    fn item_queries(&self) -> Vec<u32> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::ItemQuery { entry, .. } => Some(entry),
                _ => None,
            })
            .collect()
    }
}

/// A record: a one-handed sword, a shield, a stack of cloth, a readable scroll or a bag.
fn record(entry: u32) -> benilla_protocol::messages::ItemInfo {
    let mut t = crate::items::test_template(&format!("Item {entry}"));
    t.display_info_id = DISPLAY;
    match entry {
        25 => {
            (t.class, t.subclass, t.inventory_type, t.quality) = (2, 7, 13, 2);
            t.required_level = 5;
        }
        2129 => (t.class, t.subclass, t.inventory_type) = (4, 6, 14),
        2589 => (t.class, t.stackable) = (7, 20),
        955 => (t.class, t.page_text, t.quality) = (0, 7, 0),
        BAG => (t.class, t.inventory_type, t.container_slots) = (1, 18, BAG_SLOTS),
        _ => {}
    }
    t
}

fn item_create(h: &Held) -> SessionEvent {
    let mut pairs = vec![
        (2, 0x3),
        (3, h.entry),
        (6, ME as u32),
        (8, ME as u32),
        (FIELD_ITEM_STACK_COUNT, h.stack),
        (46, h.durability.0),
        (47, h.durability.1),
    ];
    pairs.extend_from_slice(h.extra);
    let container = h.entry == BAG;
    if container {
        pairs[0].1 = 0x7;
        let inner = HELD.iter().filter_map(|i| match i.seat {
            Seat::InBag(s) => Some((s, i.guid)),
            Seat::Inventory(_) => None,
        });
        for (slot, guid) in inner {
            let field = FIELD_CONTAINER_SLOT_1 + 2 * u16::from(slot);
            pairs.extend([(field, guid as u32), (field + 1, (guid >> 32) as u32)]);
        }
    }
    SessionEvent::ItemCreate {
        guid: h.guid,
        container,
        fields: ObjectFields::from_pairs(&pairs).into_created(if container {
            ObjectType::Container
        } else {
            ObjectType::Item
        }),
    }
}

/// The partner's trade slot 1: a rolled, enchanted sword.
fn partners_offer() -> SessionEvent {
    let mut slots = [None; TRADE_SLOT_COUNT];
    slots[0] = Some(TradeItem {
        entry: 25,
        display_id: DISPLAY,
        count: 1,
        wrapped: false,
        gift_creator: 0,
        perm_enchant: ENCHANT,
        creator: 0,
        charges: 0,
        suffix_factor: SUFFIX,
        random_prop_id: ROLL,
        lock_id: 0,
        max_durability: 40,
        durability: 30,
    });
    SessionEvent::TradeStatusExtended {
        state: Box::new(TradeStatusExtended {
            their_window: true,
            sequence: 1,
            flag: 7,
            gold: 0,
            enchant_spell_id: 0,
            slots,
        }),
    }
}

/// A letter carrying a rolled, enchanted sword.
fn letter() -> SessionEvent {
    SessionEvent::MailList {
        mails: vec![MailListEntry {
            message_id: 1,
            message_type: 0,
            sender_guid: Some(BOB),
            sender_id: None,
            subject: "A sword".into(),
            item_text_id: 0,
            package: 0,
            stationery: 41,
            item: Some(MailAttachment {
                entry: 25,
                perm_enchant: ENCHANT,
                random_prop_id: ROLL,
                suffix_factor: SUFFIX,
                count: 1,
                charges: 0,
                durability_max: 40,
                durability: 40,
            }),
            money: 0,
            cod: 0,
            checked: 0x1,
            expire_days: 30.0,
            mail_template_id: 0,
        }],
    }
}

/// The NPC or player window the fixture opens beside the bank: one at a time, as the interaction
/// latch holds one.
#[derive(Clone, Copy, PartialEq)]
enum Window {
    Trade,
    Mail,
}

fn client(window: Window) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script.run("ITEM_SUFFIX_TEMPLATE = '%s %s'").unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.insert_resource(crate::entities::ItemDisplays::icons_for_tests(
        benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::from([(
            DISPLAY,
            benilla_formats::ItemDisplay {
                icon: Some(ICON.into()),
                ..Default::default()
            },
        )])),
    ));
    app.insert_resource(crate::items::RandomProperties::new(
        benilla_formats::RandomPropertyCatalog::from_rows(std::collections::HashMap::from([(
            ROLL,
            benilla_formats::RandomProperty {
                suffix: "of the Bear".into(),
                enchants: [0; 5],
            },
        )])),
    ));
    app.insert_resource(crate::items::Enchants(
        benilla_formats::EnchantCatalog::from_rows(
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        ),
    ));
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        for h in HELD.iter().filter(|h| h.entry != 99_999) {
            items.insert_template(h.entry, Some(record(h.entry)));
        }
    }
    let mut c = Client { app, inbound, sent };
    let mut seats = Vec::new();
    for h in &HELD {
        if let Seat::Inventory(seat) = h.seat {
            let field = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(seat);
            seats.extend([(field, h.guid as u32), (field + 1, (h.guid >> 32) as u32)]);
        }
    }
    // Bob shows the rolled sword in his main hand (`PLAYER_VISIBLE_ITEM_16_*`).
    let visible = FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 12 * 15;
    let bob = [
        // `OBJECT_FIELD_TYPE`: object, unit, player.
        (2, 0x19),
        (visible + 2, 25),
        (visible + 3, ENCHANT),
        (visible + 10, ROLL),
    ];
    let mut packets: Vec<Vec<SessionEvent>> = HELD.iter().map(|h| vec![item_create(h)]).collect();
    packets.push(vec![created(ME, EntityKind::Player, &seats)]);
    packets.push(vec![created(BOB, EntityKind::Player, &bob)]);
    c.frame(packets);
    let world = c.app.world_mut();
    let bob = world.resource::<crate::net::GuidIndex>().0[&BOB];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(bob),
        guid: Some(BOB),
        ..Default::default()
    };
    world
        .resource_mut::<crate::ui_bank::BankOpen>()
        .open(BANKER);
    match window {
        Window::Trade => {
            world
                .resource_mut::<crate::ui_trade::TradeSession>()
                .begin(BOB);
            c.frame(vec![vec![SessionEvent::TradeStatus {
                status: TradeStatus::OpenWindow,
            }]]);
            c.frame(vec![vec![partners_offer()]]);
            // Our own trade slot 1, off the cursor.
            c.eval::<()>(true, "PickupContainerItem(0, 1) ClickTradeButton(1)");
        }
        Window::Mail => {
            crate::ui_mail::open_mailbox(world, MAILBOX);
            c.frame(vec![vec![letter()]]);
            // The letter's attachment, off the cursor.
            c.eval::<()>(true, "PickupContainerItem(0, 2) ClickSendMailItemButton()");
        }
    }
    c.frame(vec![]);
    c.frame(vec![]);
    // Bob within inspect range, as the unit feed's reach would hold a friendly player beside us.
    c.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_unit_reach_of(
            BOB,
            benilla_ui::script::UnitReach {
                dist_sq: 16.0,
                inspectable: true,
            },
        );
    c
}

/// Wherever the frame's snapshots are current, every item getter answers the same off the cache
/// and the objects as off the snapshot, across the bags, the bank, the worn and inspected slots,
/// the trade and the mail: the fields the snapshots hold are the ones the live read derives
/// ([`crate::ui_items::live::LiveItem`]).
#[test]
fn every_item_getter_answers_the_same_live_as_off_the_snapshot() {
    let mut c = client(Window::Trade);
    let (mut snapshot, mut live) = (c.answers(false), c.answers(true));
    let mut c = client(Window::Mail);
    snapshot.extend(c.answers(false));
    live.extend(c.answers(true));
    // The fixture shows what it means to on every surface.
    let rolled = format!("Hitem:25:{ENCHANT}:{ROLL}:{SUFFIX}|h[Item 25 of the Bear]");
    for want in [
        "GetContainerItemInfo 0:1 Interface\\Icons\\INV_Sword_04,17,1,-1,nil",
        "GetContainerItemInfo 0:2 Interface\\Icons\\INV_Sword_04,2,1,-1,1",
        "GetContainerItemLink 0:3 |cffffffff|Hitem:99999:",
        "GetContainerItemInfo 1:1 Interface\\Icons\\INV_Sword_04,5,",
        "GetInventoryItemTexture player:16 Interface\\Icons\\INV_Sword_04",
        "GetInventoryItemBroken player:17 1",
        "GetInventoryItemTexture player:20 Interface\\Icons\\INV_Sword_04",
        "GetInventoryItemTexture target:16 Interface\\Icons\\INV_Sword_04",
        // Visible gear carries no suffix factor.
        "GetInventoryItemLink target:16 |cff1eff00|Hitem:25:1900:862:0|h[Item 25 of the Bear]",
        "GetItemInfo 25 Item 25,item:25:0:0:0,2,5,",
        "GetItemInfo rolled Item 25 of the Bear,",
        "GetTradePlayerItemInfo 1 Item 2589,",
        "GetTradeTargetItemInfo 1 Item 25 of the Bear,",
        "GetInboxItem 1 Item 25 of the Bear,",
        "GetSendMailItem Item 955,",
    ] {
        assert!(
            snapshot.iter().any(|l| l.starts_with(want)),
            "{want}: {snapshot:#?}"
        );
    }
    for at in [
        "GetContainerItemLink -1:1",
        "GetInventoryItemLink player:16",
        "GetTradeTargetItemLink 1",
    ] {
        assert!(
            snapshot
                .iter()
                .any(|l| l.starts_with(at) && l.contains(&rolled)),
            "{at} links the roll: {snapshot:#?}"
        );
    }
    assert_eq!(snapshot.len(), live.len());
    let differ: Vec<(&String, &String)> =
        snapshot.iter().zip(&live).filter(|(s, l)| s != l).collect();
    assert!(differ.is_empty(), "snapshot vs live: {differ:#?}");
}

/// A stack a packet changed after the frame's pushes, and a record that landed after them, read
/// as the object and the cache hold them now, through the host; the snapshot still holds the
/// push, and the live hit asks for nothing.
#[test]
fn an_item_getter_reads_the_object_and_the_cache_as_it_runs() {
    let mut c = client(Window::Trade);
    c.write_store(HELD[2].guid, &[(FIELD_ITEM_STACK_COUNT, 3)]);
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(99_999, Some(record(99_999)));
    let _ = c.item_queries();
    let chunk = "local _, count = GetContainerItemInfo(0, 1) \
                 local _, _, traded = GetTradePlayerItemInfo(1) \
                 return count, traded, (GetItemInfo(99999))";
    let live: (i64, i64, Option<String>) = c.eval(true, chunk);
    assert_eq!(live, (3, 3, Some("Item 99999".into())));
    assert!(c.item_queries().is_empty(), "a cached record asks nothing");
    let snapshot: (i64, i64, Option<String>) = c.eval(false, chunk);
    assert_eq!(snapshot, (17, 17, None));
}

/// The bag getters' per-call cost: 10k `GetContainerItemInfo` calls over the three filled backpack
/// slots, through the lent host and off the snapshot alone.
///
/// Run with `cargo nextest run -p benilla-app --lib item_getter_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn item_getter_cost() {
    let mut c = client(Window::Trade);
    let chunk = r#"
        local sum = 0
        for n = 1, 9999 do
            local _, count = GetContainerItemInfo(0, math.mod(n, 3) + 1)
            sum = sum + count
        end
        return sum"#;
    let mut median = |live: bool| {
        let mut times = Vec::new();
        for _ in 0..50 {
            let t = std::time::Instant::now();
            let sum: i64 = c.eval(live, chunk);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(sum, 3333 * (17 + 2 + 1));
        }
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let (live, snapshot) = (median(true), median(false));
    eprintln!(
        "item getters: 10k GetContainerItemInfo calls, median {live:.3} ms with the host lent, \
         {snapshot:.3} ms off the snapshots alone"
    );
}
