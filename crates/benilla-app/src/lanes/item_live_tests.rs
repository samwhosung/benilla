//! The item and bag getters read the item cache and the item objects as they run, through the
//! lent host, as the reference's read the slot's object and its record (`GetContainerItemInfo`,
//! `0x6228a0` then `0x55ba30`): the live answer and the frame's snapshot agree on every getter
//! wherever the snapshot is current, and the cache and the object answer where it is not.

use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_PLAYER_INV_SLOT_HEAD};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// `PLAYER_FIELD_PACK_SLOT_1`, the backpack's first slot.
const PACK: u16 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
/// The display every record here shows, and its icon.
const DISPLAY: u32 = 950;
const ICON: &str = "Interface\\Icons\\INV_Sword_04";

/// An item object of ours: its guid, entry, stack and durability (current, max).
struct Held {
    guid: u64,
    entry: u32,
    stack: u32,
    durability: (u32, u32),
}

/// What the agreement reads: a sword in the main hand, a broken shield in the off hand, linen in
/// the backpack's first slot, a scroll in its second, and in its third an item whose record never
/// came.
const HELD: [Held; 5] = [
    Held {
        guid: item(1),
        entry: 25,
        stack: 1,
        durability: (30, 40),
    },
    Held {
        guid: item(2),
        entry: 2129,
        stack: 1,
        durability: (0, 35),
    },
    Held {
        guid: item(3),
        entry: 2589,
        stack: 17,
        durability: (0, 0),
    },
    Held {
        guid: item(4),
        entry: 955,
        stack: 2,
        durability: (0, 0),
    },
    Held {
        guid: item(5),
        entry: 99_999,
        stack: 1,
        durability: (0, 0),
    },
];

/// Where each of [`HELD`] sits: an inventory slot (main hand 15, off hand 16, backpack 23..).
const SEATS: [u8; 5] = [15, 16, 23, 24, 25];

const fn item(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The whole client, headless, in the world as [`ME`] with [`HELD`] seated.
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
            for slot = 1, 4 do
                note('GetContainerItemInfo ' .. slot, GetContainerItemInfo(0, slot))
                note('GetContainerItemLink ' .. slot, GetContainerItemLink(0, slot))
            end
            for _, slot in ipairs({{ 16, 17, 0 }}) do
                note('GetInventoryItemTexture ' .. slot, GetInventoryItemTexture('player', slot))
                note('GetInventoryItemLink ' .. slot, GetInventoryItemLink('player', slot))
                note('GetInventoryItemCount ' .. slot, GetInventoryItemCount('player', slot))
                note('GetInventoryItemQuality ' .. slot, GetInventoryItemQuality('player', slot))
                note('GetInventoryItemBroken ' .. slot, GetInventoryItemBroken('player', slot))
            end
            note('OffhandHasWeapon', OffhandHasWeapon())
            for _, entry in ipairs({{ {entries} }}) do
                note('GetItemInfo ' .. entry, GetItemInfo(entry))
                note('GetItemInfo item:' .. entry, GetItemInfo('item:' .. entry .. ':0:0:0'))
            end
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

/// A record: a one-handed sword, a shield, a stack of cloth or a readable scroll.
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
        _ => {}
    }
    t
}

fn item_create(h: &Held) -> SessionEvent {
    SessionEvent::ItemCreate {
        guid: h.guid,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, h.entry),
            (6, ME as u32),
            (8, ME as u32),
            (FIELD_ITEM_STACK_COUNT, h.stack),
            (46, h.durability.0),
            (47, h.durability.1),
        ])
        .into_created(ObjectType::Item),
    }
}

fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
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
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        for h in &HELD[..4] {
            items.insert_template(h.entry, Some(record(h.entry)));
        }
    }
    let mut c = Client { app, inbound, sent };
    let mut seats = Vec::new();
    for (h, &seat) in HELD.iter().zip(&SEATS) {
        let field = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(seat);
        seats.extend([(field, h.guid as u32), (field + 1, (h.guid >> 32) as u32)]);
    }
    assert_eq!(seats[4].0, PACK);
    let mut packets: Vec<Vec<SessionEvent>> = HELD.iter().map(|h| vec![item_create(h)]).collect();
    packets.push(vec![created(ME, EntityKind::Player, &seats)]);
    c.frame(packets);
    c.frame(vec![]);
    c
}

/// Wherever the frame's snapshots are current, every item getter answers the same off the cache
/// and the objects as off the snapshot: the bag, inventory and record fields the snapshots hold
/// are the ones the live read derives ([`crate::ui_items::live::LiveItem`]).
#[test]
fn every_item_getter_answers_the_same_live_as_off_the_snapshot() {
    let mut c = client();
    let snapshot = c.answers(false);
    let live = c.answers(true);
    // The fixture shows what it means to: a sword with its icon and link, a 17-stack, a broken
    // shield, the uncached item's white link, and a record's full answer.
    for want in [
        "GetInventoryItemTexture 16 Interface\\Icons\\INV_Sword_04",
        "GetContainerItemInfo 1 Interface\\Icons\\INV_Sword_04,17,nil,-1,nil",
        "GetContainerItemInfo 2 Interface\\Icons\\INV_Sword_04,2,nil,-1,1",
        "GetInventoryItemBroken 17 1",
        "GetContainerItemLink 3 |cffffffff|Hitem:99999:",
        "GetItemInfo 2589 Item 2589,item:2589:0:0:0,1,0,,,20,",
        "GetItemInfo 25 Item 25,item:25:0:0:0,2,5,",
    ] {
        assert!(
            snapshot.iter().any(|l| l.starts_with(want)),
            "{want}: {snapshot:#?}"
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
    let mut c = client();
    c.write_store(HELD[2].guid, &[(FIELD_ITEM_STACK_COUNT, 3)]);
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(99_999, Some(record(99_999)));
    let _ = c.item_queries();
    let chunk = "local _, count = GetContainerItemInfo(0, 1) return count, (GetItemInfo(99999))";
    let live: (i64, Option<String>) = c.eval(true, chunk);
    assert_eq!(live, (3, Some("Item 99999".into())));
    assert!(c.item_queries().is_empty(), "a cached record asks nothing");
    let snapshot: (i64, Option<String>) = c.eval(false, chunk);
    assert_eq!(snapshot, (17, None));
}

/// The bag getters' per-call cost: 10k `GetContainerItemInfo` calls over the three filled backpack
/// slots, through the lent host and off the snapshot alone.
///
/// Run with `cargo nextest run -p benilla-app --lib item_getter_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn item_getter_cost() {
    let mut c = client();
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
