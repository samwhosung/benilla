//! The loot window's verbs and packets through the whole client's host, each read on the line or
//! the packet after it: `LootSlot`, `CloseLoot` and `GiveMasterLoot` apply inside their calls
//! (`0x4c2e70`, `0x4c2ec0`, `0x4c2940`), a row's click takes inside its input record after its
//! `<OnClick>` (`0x4c1820`), the last row's removal closes inside its packet (`0x4c1f8d`), the
//! auto-loot copier sweeps before it signals (`0x4c1ed3` → `0x4c1fa0`), the count is the last
//! occupied row's (`0x4c2260`), and a response for an object the client lacks opens nothing
//! (`0x5ebb3f`).

use benilla_protocol::messages::{slot_type, ItemInfo, ItemPushResult, LootItem, ObjectFields};
use benilla_protocol::{EntityKind, SessionEvent};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_ui::script::UiScript;

use super::{LootConfig, LootLatch, LootState};
use crate::items::Items;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfGuid};
use crate::script_calls::with_hosted_vm;
use crate::ui_action::{UiError, UiErrorKeys};

/// A dead creature the object manager holds: the loot source.
const CORPSE: u64 = 0xF130_0000_0000_00AB;
/// A guid no object carries.
const GONE: u64 = 0xF130_0000_0000_0FFF;
const ME: u64 = 0x10;
/// A white, a bind-on-pickup epic, and a second white.
const LINEN: u32 = 2589;
const FELSTRIKER: u32 = 12590;
const WOOL: u32 = 2592;
/// The display id the cloths share, which has an icon, so their pushes signal `ITEM_PUSH`.
const CLOTH_DISPLAY: u32 = 5;
/// A master-loot candidate.
const THRALL: u64 = 0x22;

struct Loot {
    app: App,
    rx: Receiver<ClientCommand>,
}

/// The whole client, headless, holding [`CORPSE`], with the three templates and a VM that notes
/// the loot events in `SEEN` and defines the loot error keys as themselves.
fn client() -> Loot {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let corpse = world
        .spawn((
            Guid(CORPSE),
            NetEntity {
                kind: EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            Transform::default(),
            GlobalTransform::default(),
            // `UNIT_FIELD_HEALTH` 0, `UNIT_FIELD_MAXHEALTH` 100.
            ObjectStore(ObjectFields::from_pairs(&[(22, 0), (28, 100)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(CORPSE, corpse);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    world.insert_resource(crate::entities::ItemDisplays::icons_for_tests(
        benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::from([(
            CLOTH_DISPLAY,
            benilla_formats::ItemDisplay {
                icon: Some("CLOTH".into()),
                ..Default::default()
            },
        )])),
    ));
    let mut items = world.resource_mut::<Items>();
    for (entry, name, quality, bonding) in [
        (LINEN, "Linen Cloth", 1, 0),
        (FELSTRIKER, "Felstriker", 4, 1),
        (WOOL, "Wool Cloth", 1, 0),
    ] {
        let mut t = crate::items::test_template(name);
        t.display_info_id = CLOTH_DISPLAY;
        items.insert_template(
            entry,
            Some(ItemInfo {
                quality,
                bonding,
                ..t
            }),
        );
    }
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"ERR_LOOT_ROLL_PENDING = "ERR_LOOT_ROLL_PENDING"
            ERR_LOOT_NO_UI = "ERR_LOOT_NO_UI"
            SEEN = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"LOOT_OPENED", "LOOT_SLOT_CLEARED", "LOOT_CLOSED",
                    "LOOT_BIND_CONFIRM", "OPEN_MASTER_LOOT_LIST", "ITEM_PUSH",
                    "UI_ERROR_MESSAGE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                if event == "LOOT_SLOT_CLEARED" or event == "LOOT_BIND_CONFIRM"
                        or event == "UI_ERROR_MESSAGE" then
                    tinsert(SEEN, event .. ":" .. tostring(arg1))
                else
                    tinsert(SEEN, event)
                end
            end)
            ROW = CreateFrame("LootButton", "Row", UIParent)
            ROW:SetPoint("BOTTOMLEFT", 100, 100) ROW:SetWidth(50) ROW:SetHeight(50)
            ROW:EnableMouse(true)"#,
        )
        .expect("the recorder installs");
    world.insert_non_send_resource(script);
    Loot { app, rx }
}

fn row(slot: u8, entry: u32) -> LootItem {
    LootItem {
        slot,
        item_id: entry,
        count: 1,
        display_info_id: CLOTH_DISPLAY,
        random_property_id: 0,
        slot_type: slot_type::ALLOW_LOOT,
    }
}

/// The packets, dispatched as one net drain in wire order.
fn packets(l: &mut Loot, events: Vec<SessionEvent>) {
    crate::net::handlers::dispatch(l.app.world_mut(), events);
}

/// The answer to our `CMSG_LOOT` on `guid`, which armed the latch: the response, then the feed
/// that shows it.
fn open(l: &mut Loot, guid: u64, loot_type: u8, gold: u32, items: Vec<LootItem>) {
    l.app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
    packets(
        l,
        vec![SessionEvent::LootResponse {
            guid,
            loot_type,
            gold,
            items,
        }],
    );
    feed(l);
}

fn feed(l: &mut Loot) {
    super::feed_loot_once(l.app.world_mut());
}

/// Run `lua` with the host lent, as every entry that runs Lua lends it.
fn run(l: &mut Loot, lua: &str) {
    with_hosted_vm(l.app.world_mut(), |s| s.expect("a VM").run(lua)).expect("the chunk runs");
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(l: &mut Loot, lua: &str) -> T {
    with_hosted_vm(l.app.world_mut(), |s| s.expect("a VM").eval::<T>(lua)).expect("the chunk runs")
}

/// The events noted since the last read.
fn seen(l: &mut Loot) -> String {
    eval(l, "local s = table.concat(SEEN, ',') SEEN = {} return s")
}

/// The loot packets sent since the last read, in order.
fn sent(l: &Loot) -> Vec<ClientCommand> {
    l.rx.try_iter()
        .filter(|c| {
            matches!(
                c,
                ClientCommand::AutostoreLootItem { .. }
                    | ClientCommand::LootMoney
                    | ClientCommand::LootRelease { .. }
                    | ClientCommand::LootMasterGive { .. }
            )
        })
        .collect()
}

/// A hardware left click on the `LootButton` slotted at `slot`, its own input record.
fn click(l: &mut Loot, slot: u32) {
    with_hosted_vm(l.app.world_mut(), |s| {
        let s = s.expect("a VM");
        s.set_screen_size(1024.0, 768.0);
        s.run(&format!("ROW:SetSlot({slot}) ROW:Show()"))
            .expect("the row is slotted");
        s.resolve();
        s.mouse_button(125.0, 125.0, "LeftButton", true);
        s.mouse_button(125.0, 125.0, "LeftButton", false);
    });
}

/// The row click takes inside its input record, after its `<OnClick>` (`0x4c1820` → `0x4c2790(slot,
/// 0)`): the send is out and the bind confirm signalled when the click returns, and a `LootSlot` in
/// the confirm's own handler finds no stash (`0x4c290f` before `0x4c2917`).
#[test]
fn a_rows_click_takes_inside_its_input_record() {
    let mut l = client();
    open(
        &mut l,
        CORPSE,
        1,
        0,
        vec![row(3, LINEN), row(4, FELSTRIKER)],
    );
    assert_eq!(seen(&mut l), "LOOT_OPENED");
    let _ = sent(&l);

    click(&mut l, 1);
    assert!(
        matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 3 }]),
        "the white's store is out as the click returns"
    );

    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_BIND_CONFIRM")
        f:SetScript("OnEvent", function() LootSlot(arg1) end)"#,
    );
    click(&mut l, 2);
    assert_eq!(
        seen(&mut l),
        "LOOT_BIND_CONFIRM:2",
        "the confirm is signalled inside the click"
    );
    assert!(sent(&l).is_empty(), "its handler's accept finds no stash");
    run(&mut l, "LootSlot(2)");
    assert!(
        matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 4 }]),
        "an accept after the click sends the row (`0x4c27c0`)"
    );
}

/// The coin row's click (`0x4c2826`-`0x4c287c`): `CMSG_LOOT_MONEY`, the money cell at -1 and
/// `LOOT_SLOT_CLEARED` 1 inside the click, and with nothing else left the close with release.
#[test]
fn the_coin_rows_click_clears_it_and_closes_an_emptied_window() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 150, vec![row(0, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    click(&mut l, 1);
    assert!(matches!(sent(&l)[..], [ClientCommand::LootMoney]));
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1");
    assert!(
        eval::<bool>(&mut l, "return GetLootSlotInfo(1) == nil"),
        "the coin row is a gap at once"
    );

    let mut l = client();
    open(&mut l, CORPSE, 1, 150, Vec::new());
    let _ = (seen(&mut l), sent(&l));
    click(&mut l, 1);
    assert!(
        matches!(
            sent(&l)[..],
            [
                ClientCommand::LootMoney,
                ClientCommand::LootRelease { guid: CORPSE }
            ]
        ),
        "the last row's click releases the loot inside it (`0x4c286a` → `0x48f200`)"
    );
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1,LOOT_CLOSED");
}

/// A roll-pending row refuses inside the click (`0x4c28b3`, `ERR_LOOT_ROLL_PENDING`), and a
/// master-loot row signals `OPEN_MASTER_LOOT_LIST` there (`0x4c28d1`); neither sends.
#[test]
fn a_roll_or_master_rows_click_answers_inside_it() {
    let mut l = client();
    let mut rolling = row(0, LINEN);
    rolling.slot_type = slot_type::ROLL_ONGOING;
    let mut master = row(1, WOOL);
    master.slot_type = slot_type::MASTER;
    open(&mut l, CORPSE, 1, 0, vec![rolling, master]);
    let _ = (seen(&mut l), sent(&l));
    click(&mut l, 1);
    assert_eq!(seen(&mut l), "UI_ERROR_MESSAGE:ERR_LOOT_ROLL_PENDING");
    click(&mut l, 2);
    assert_eq!(seen(&mut l), "OPEN_MASTER_LOOT_LIST");
    assert!(sent(&l).is_empty());
}

/// `CloseLoot` closes inside its call (`0x4c2ec0` → `0x48f200` → `0x4c1cb0`): the release is out,
/// `LOOT_CLOSED` has run and the window reads closed on the next line.
#[test]
fn close_loot_closes_inside_its_call() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    run(&mut l, "CloseLoot() N = GetNumLootItems()");
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::LootRelease { guid: CORPSE }]
    ));
    assert_eq!(seen(&mut l), "LOOT_CLOSED");
    assert_eq!(eval::<i64>(&mut l, "return N"), 0);
}

/// The verbs apply in call order. `GiveMasterLoot` after `CloseLoot` finds no source
/// (`0x4c2946`-`0x4c2953`); `LootSlot` after it still answers the pending row, which the close
/// keeps (`0x48f200` and `0x4c1cb0(0)` leave `0x847cec` and the rows alone), and sends after the
/// release.
#[test]
fn the_verbs_after_a_close_apply_after_it() {
    let mut l = client();
    let mut master = row(2, LINEN);
    master.slot_type = slot_type::MASTER;
    packets(
        &mut l,
        vec![SessionEvent::LootMasterList {
            candidates: vec![THRALL],
        }],
    );
    open(&mut l, CORPSE, 1, 0, vec![master]);
    let _ = sent(&l);
    run(&mut l, "CloseLoot() GiveMasterLoot(1, 1)");
    assert!(
        matches!(sent(&l)[..], [ClientCommand::LootRelease { guid: CORPSE }]),
        "no give after the close"
    );

    let mut l = client();
    open(&mut l, CORPSE, 1, 0, vec![row(4, FELSTRIKER)]);
    click(&mut l, 1);
    let _ = sent(&l);
    run(&mut l, "CloseLoot() LootSlot(1)");
    assert!(
        matches!(
            sent(&l)[..],
            [
                ClientCommand::LootRelease { guid: CORPSE },
                ClientCommand::AutostoreLootItem { slot: 4 }
            ]
        ),
        "the release, then the pending row's store"
    );
}

/// The last row's `SMSG_LOOT_REMOVED` closes inside its packet (`0x4c1f7f`, `0x4c1f8d`): the
/// release goes out at the packet, and `LOOT_CLOSED` comes after its `LOOT_SLOT_CLEARED` and before
/// the `ITEM_PUSH` vmangos sends after it (`LootHandler.cpp:217-230`).
#[test]
fn the_last_rows_removal_closes_before_its_push() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    packets(
        &mut l,
        vec![
            SessionEvent::LootRemoved { slot: 0 },
            SessionEvent::ItemPushResult(ItemPushResult {
                player_guid: ME,
                from_npc: false,
                created: false,
                show_in_chat: true,
                bag_slot: benilla_protocol::messages::BAG_PLAYER_INVENTORY,
                item_slot: 23,
                item_entry: LINEN,
                suffix_factor: 0,
                random_property_id: 0,
                count: 1,
            }),
        ],
    );
    assert!(
        matches!(sent(&l)[..], [ClientCommand::LootRelease { guid: CORPSE }]),
        "released at the packet"
    );
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1,LOOT_CLOSED,ITEM_PUSH");
}

/// The auto-loot copier sweeps first (`0x4c1ed3` → `0x4c1fa0`): the stores and the bind confirm,
/// then `LOOT_OPENED` while anything is left (`0x4c2240`), sent rows included; an empty window
/// closes with release instead (`0x4c224f`) and never signals it.
#[test]
fn an_auto_loot_open_sweeps_before_it_signals() {
    let mut l = client();
    l.app.world_mut().resource_mut::<LootConfig>().auto_loot = true;
    open(
        &mut l,
        CORPSE,
        1,
        0,
        vec![row(5, FELSTRIKER), row(6, LINEN)],
    );
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::AutostoreLootItem { slot: 6 }]
    ));
    assert_eq!(seen(&mut l), "LOOT_BIND_CONFIRM:1,LOOT_OPENED");

    let mut l = client();
    l.app.world_mut().resource_mut::<LootConfig>().auto_loot = true;
    open(&mut l, CORPSE, 1, 0, Vec::new());
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::LootRelease { guid: CORPSE }]
    ));
    assert_eq!(seen(&mut l), "LOOT_CLOSED");
}

/// `GetNumLootItems` (`0x4c2c30` → `0x4c2260`): the last occupied row's position plus one, plus
/// one while the money cell is nonzero, a taken coin's -1 included.
#[test]
fn the_count_is_the_last_occupied_rows() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 5, vec![row(0, LINEN), row(1, WOOL)]);
    let count = |l: &mut Loot| eval::<i64>(l, "return GetNumLootItems()");
    assert_eq!(count(&mut l), 3);
    packets(&mut l, vec![SessionEvent::LootRemoved { slot: 1 }]);
    feed(&mut l);
    assert_eq!(count(&mut l), 2, "the last row taken");
    packets(&mut l, vec![SessionEvent::LootClearMoney]);
    feed(&mut l);
    assert_eq!(count(&mut l), 2, "a taken coin still counts");

    let mut l = client();
    open(&mut l, CORPSE, 1, 5, vec![row(0, LINEN), row(1, WOOL)]);
    packets(
        &mut l,
        vec![
            SessionEvent::LootRemoved { slot: 0 },
            SessionEvent::LootRemoved { slot: 1 },
        ],
    );
    feed(&mut l);
    assert_eq!(count(&mut l), 1, "the coin alone");
}

/// A response for an object the client does not hold shows `ERR_LOOT_DIDNT_KILL` and returns
/// (`0x5ebb33`-`0x5ebb57`), before the latch (`0x5ebb60`) and the open.
#[test]
fn a_response_for_an_object_we_lack_opens_nothing() {
    let mut l = client();
    // A fishing answer on a cold latch, whose bobber is gone.
    packets(
        &mut l,
        vec![SessionEvent::LootResponse {
            guid: GONE,
            loot_type: 3,
            gold: 0,
            items: vec![row(0, LINEN)],
        }],
    );
    feed(&mut l);
    let world = l.app.world();
    assert_eq!(world.resource::<LootState>().source(), None);
    assert_eq!(world.resource::<LootLatch>().0, None, "no latch written");
    assert!(world
        .resource::<UiErrorKeys>()
        .snapshot()
        .contains(&UiError::key("ERR_LOOT_DIDNT_KILL")));
    assert_eq!(seen(&mut l), "");
}

/// `CloseLoot` on a disenchant window with rows left sweeps it (`0x48f264` → `0x4c1fa0` with the
/// close's flag), and the sweep's tail signals `LOOT_OPENED` for a flag of 0 while rows are left
/// (`0x4c2237`, `0x4c2240`), so the window the close hid comes back.
#[test]
fn close_loot_on_a_disenchant_window_sweeps_and_reopens_it() {
    let mut l = client();
    open(&mut l, CORPSE, 4, 0, vec![row(1, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    run(&mut l, "CloseLoot()");
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::AutostoreLootItem { slot: 1 }]
    ));
    assert_eq!(seen(&mut l), "LOOT_OPENED");
    run(&mut l, "CloseLoot(1)");
    assert_eq!(seen(&mut l), "", "a flag of 1 signals nothing (`0x4c2237`)");
}

/// A response that replaces the open window ends its session first (`0x4c1cb0`: `LOOT_CLOSED` at
/// `0x4c1dac` for a session open), then opens the new one.
#[test]
fn a_response_over_an_open_window_closes_it_first() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = seen(&mut l);
    open(&mut l, CORPSE, 1, 0, vec![row(0, WOOL)]);
    assert_eq!(seen(&mut l), "LOOT_CLOSED,LOOT_OPENED");
}
