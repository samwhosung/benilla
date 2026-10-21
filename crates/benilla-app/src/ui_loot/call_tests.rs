//! The loot window's verbs and packets through the whole client's host, each read on the line or
//! the packet after it: `LootSlot`, `CloseLoot` and `GiveMasterLoot` apply inside their calls
//! (`0x4c2e70`, `0x4c2ec0`, `0x4c2940`), a row's click takes inside its input record after its
//! `<OnClick>` (`0x4c1820`), the last row's removal and the coin line's clear close inside their
//! packets (`0x4c1f8d`, `0x4c2785`), the auto-loot copier sweeps before it signals (`0x4c1ed3` →
//! `0x4c1fa0`), the count is the last occupied row's (`0x4c2260`), and a response for an object
//! the client lacks opens nothing (`0x5ebb3f`).

use benilla_protocol::messages::{slot_type, ItemInfo, ItemPushResult, LootItem, ObjectFields};
use benilla_protocol::{EntityKind, SessionEvent};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_ui::script::UiScript;

use super::{AutoLootByte, LootLatch, LootState};
use crate::items::Items;
use crate::net::{
    ClientCommand, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfGuid, SelfPlayer,
};
use crate::script_calls::with_hosted_vm;
use crate::ui_action::{UiError, UiErrorKeys};

/// A dead creature the object manager holds: the loot source.
pub(super) const CORPSE: u64 = 0xF130_0000_0000_00AB;
/// A guid no object carries.
pub(super) const GONE: u64 = 0xF130_0000_0000_0FFF;
pub(super) const ME: u64 = 0x10;
/// A lockbox in our bags, an item object the object manager holds.
pub(super) const LOCKBOX: u64 = 0x4000_0000_0000_0007;
/// A white, a bind-on-pickup epic, and a second white.
pub(super) const LINEN: u32 = 2589;
pub(super) const FELSTRIKER: u32 = 12590;
pub(super) const WOOL: u32 = 2592;
/// The display id the cloths share, which has an icon, so their pushes signal `ITEM_PUSH`.
const CLOTH_DISPLAY: u32 = 5;
/// A master-loot candidate.
pub(super) const THRALL: u64 = 0x22;

pub(super) struct Loot {
    pub(super) app: App,
    rx: Receiver<ClientCommand>,
}

/// The whole client, headless, holding our player, [`CORPSE`] and [`LOCKBOX`], with the three
/// templates and a VM that notes the loot events in `SEEN`, defines the loot error keys as
/// themselves and the money line's strings as the stock ones.
pub(super) fn client() -> Loot {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = world
        .spawn((
            Guid(ME),
            SelfPlayer,
            NetEntity {
                kind: EntityKind::Player,
                display_id: None,
                scale: 1.0,
            },
            Transform::default(),
            GlobalTransform::default(),
            // `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH` 100.
            ObjectStore(ObjectFields::from_pairs(&[(22, 100), (28, 100)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    // `OBJECT_FIELD_TYPE` (2): `TYPEMASK_OBJECT | TYPEMASK_ITEM`.
    let lockbox = world
        .spawn((
            Guid(LOCKBOX),
            ObjectStore(ObjectFields::from_pairs(&[(2, 0x3)])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(LOCKBOX, lockbox);
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
            ERR_LOOT_DIDNT_KILL = "ERR_LOOT_DIDNT_KILL"
            YOU_LOOT_MONEY = "You loot %s"
            GOLD = "Gold" SILVER = "Silver" COPPER = "Copper"
            SEEN = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"LOOT_OPENED", "LOOT_SLOT_CLEARED", "LOOT_CLOSED",
                    "LOOT_BIND_CONFIRM", "OPEN_MASTER_LOOT_LIST", "ITEM_PUSH",
                    "UI_ERROR_MESSAGE", "ITEM_LOCK_CHANGED"}) do
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

pub(super) fn row(slot: u8, entry: u32) -> LootItem {
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
pub(super) fn packets(l: &mut Loot, events: Vec<SessionEvent>) {
    crate::net::handlers::dispatch(l.app.world_mut(), events);
}

/// The answer to our `CMSG_LOOT` on `guid`, which armed the latch: the response, then the feed
/// that shows it.
pub(super) fn open(l: &mut Loot, guid: u64, loot_type: u8, gold: u32, items: Vec<LootItem>) {
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

pub(super) fn feed(l: &mut Loot) {
    super::feed_loot_once(l.app.world_mut());
}

/// Run `lua` with the host lent, as every entry that runs Lua lends it.
pub(super) fn run(l: &mut Loot, lua: &str) {
    with_hosted_vm(l.app.world_mut(), |s| s.expect("a VM").run(lua)).expect("the chunk runs");
}

pub(super) fn eval<T: benilla_ui::mlua::FromLuaMulti>(l: &mut Loot, lua: &str) -> T {
    with_hosted_vm(l.app.world_mut(), |s| s.expect("a VM").eval::<T>(lua)).expect("the chunk runs")
}

/// The events noted since the last read.
pub(super) fn seen(l: &mut Loot) -> String {
    eval(l, "local s = table.concat(SEEN, ',') SEEN = {} return s")
}

/// The loot packets sent since the last read, in order.
pub(super) fn sent(l: &Loot) -> Vec<ClientCommand> {
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
pub(super) fn click(l: &mut Loot, slot: u32) {
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

/// With the byte the interaction stored set (`0x5df468`), the copier sweeps first (`0x4c1ec7`,
/// `0x4c1ed3` → `0x4c1fa0`): the stores and the bind confirm, then `LOOT_OPENED` while anything is
/// left (`0x4c2240`), sent rows included; an empty window closes with release instead
/// (`0x4c224f`) and never signals it. The response reads the stored byte, never the keys.
#[test]
fn an_auto_loot_open_sweeps_before_it_signals() {
    let mut l = client();
    l.app.world_mut().resource_mut::<AutoLootByte>().0 = true;
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
    run(&mut l, "LootSlot(1)");
    assert!(
        matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 5 }]),
        "the sweep stashed the confirmed row after its listeners (`0x4c21dc`)"
    );

    let mut l = client();
    l.app.world_mut().resource_mut::<AutoLootByte>().0 = true;
    open(&mut l, CORPSE, 1, 0, Vec::new());
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::LootRelease { guid: CORPSE }]
    ));
    assert_eq!(seen(&mut l), "LOOT_CLOSED");

    let mut l = client();
    open(&mut l, CORPSE, 1, 0, Vec::new());
    assert!(sent(&l).is_empty(), "without the byte nothing closes it");
    assert_eq!(seen(&mut l), "LOOT_OPENED", "`0x4c1ee6` signals it empty");
}

/// `SMSG_LOOT_CLEAR_MONEY` (`0x5ec030` → `0x4c2750`) on a window with nothing else left closes
/// with release inside its packet (`0x4c2777`, `0x4c2785`), after its `LOOT_SLOT_CLEARED` 1.
#[test]
fn the_coin_lines_clear_closes_an_emptied_window_at_its_packet() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 150, Vec::new());
    let _ = (seen(&mut l), sent(&l));
    packets(&mut l, vec![SessionEvent::LootClearMoney]);
    assert!(
        matches!(sent(&l)[..], [ClientCommand::LootRelease { guid: CORPSE }]),
        "released at the packet"
    );
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1,LOOT_CLOSED");
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

/// `IsFishingLoot` (`0x4c2740`) reads the type cell `0xb71bbc` with no guard, and a close leaves
/// it (`0x48f200` → `0x4c1cb0(0)` zeroes only the source, `0x4c1da0`): after the close it still
/// answers the last window's type.
#[test]
fn is_fishing_loot_reads_the_last_windows_type_after_its_close() {
    let mut l = client();
    // A fishing answer (type 3), which a cold latch admits (`0x5eb953`).
    packets(
        &mut l,
        vec![SessionEvent::LootResponse {
            guid: CORPSE,
            loot_type: 3,
            gold: 0,
            items: vec![row(0, LINEN)],
        }],
    );
    feed(&mut l);
    let fishing = |l: &mut Loot| eval::<bool>(l, "return IsFishingLoot() == 1");
    assert!(fishing(&mut l));
    run(&mut l, "CloseLoot()");
    assert!(fishing(&mut l), "the type cell outlives the close");
    feed(&mut l);
    assert!(fishing(&mut l), "and the next frame's feed");
}

/// `SMSG_LOOT_CLEAR_MONEY` tests no latch (`0x5ec030` → `0x4c2750`), and the close keeps the money
/// cell, so a clear that lands after the close still takes the coin row and signals
/// `LOOT_SLOT_CLEARED` 1 (`0x4c2765`, `0x4c276f`); the close it then asks for finds no window
/// (`0x4c2785` → `0x48f23f`) and sends nothing.
#[test]
fn a_coin_clear_after_the_close_still_signals_its_row() {
    let mut l = client();
    open(&mut l, CORPSE, 1, 150, vec![row(0, LINEN)]);
    run(&mut l, "CloseLoot()");
    let _ = (seen(&mut l), sent(&l));
    packets(&mut l, vec![SessionEvent::LootClearMoney]);
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1");
    assert!(sent(&l).is_empty());
}

/// `CloseLoot(1)` on a lockbox's window (`0x48f200`): `ERR_LOOT_NO_UI` first (`0x48f27b`), then
/// the close's unlock (`0x48f2bd`) and the teardown's (`0x4c1d36`), then `LOOT_CLOSED`
/// (`0x4c1dac`).
#[test]
fn close_loot_failed_shows_its_line_before_the_teardown() {
    let mut l = client();
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, LINEN)]);
    let _ = seen(&mut l);
    run(&mut l, "CloseLoot(1)");
    assert_eq!(
        seen(&mut l),
        "UI_ERROR_MESSAGE:ERR_LOOT_NO_UI,ITEM_LOCK_CHANGED,ITEM_LOCK_CHANGED,LOOT_CLOSED"
    );
}

/// The last row's removal on a lockbox (`0x4c1f00`): `LOOT_SLOT_CLEARED` (`0x4c1f77`), then the
/// close it asks for (`0x4c1f8d` → `0x48f200`) with its two unlocks (`0x48f2bd`, `0x4c1d36`) and
/// `LOOT_CLOSED` (`0x4c1dac`), all inside the packet.
#[test]
fn a_lockboxs_last_row_signals_its_unlocks_between_the_clear_and_the_close() {
    let mut l = client();
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, LINEN)]);
    let _ = seen(&mut l);
    packets(&mut l, vec![SessionEvent::LootRemoved { slot: 0 }]);
    feed(&mut l);
    assert_eq!(
        seen(&mut l),
        "LOOT_SLOT_CLEARED:1,ITEM_LOCK_CHANGED,ITEM_LOCK_CHANGED,LOOT_CLOSED"
    );
    assert!(
        !l.app.world().resource::<LootState>().owes_unlocks(),
        "signalled in their place, none left for the inventory pass"
    );
}

/// A refusal (`loot_type` 0) whose guid is not the held loot target goes to `0x5eb963`: no line,
/// the target cleared (`0x5eb9d2`) and `UnlockItem` on the refused guid (`0x5eb9de`). With no
/// target held it takes the same leg (`0x5eb944`).
#[test]
fn a_refusal_for_another_session_shows_nothing_and_drops_the_target() {
    use benilla_protocol::messages::loot_error;
    for held in [Some(CORPSE), None] {
        let mut l = client();
        open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
        l.app.world_mut().resource_mut::<LootLatch>().0 = held;
        packets(
            &mut l,
            vec![SessionEvent::LootError {
                guid: GONE,
                error: loot_error::TOO_FAR,
            }],
        );
        let world = l.app.world();
        assert!(
            world.resource::<UiErrorKeys>().snapshot().is_empty(),
            "held {held:?}: no line"
        );
        assert_eq!(world.resource::<LootLatch>().0, None, "held {held:?}");
        assert!(
            world.resource::<LootState>().owes_unlocks(),
            "held {held:?}"
        );
    }
}

/// A response over an open window runs its copier's teardown first (`0x4c1cb0`): the source is
/// zeroed (`0x4c1da0`) and `LOOT_CLOSED` signalled (`0x4c1dac`) before the new source is stored
/// (`0x4c1dc2`), so a `CloseLoot` in its handlers finds no window (`0x48f23f`) and releases
/// nothing; the new window opens.
#[test]
fn a_replacing_responses_close_finds_no_window() {
    let mut l = client();
    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_CLOSED")
        f:SetScript("OnEvent", function() CloseLoot() end)"#,
    );
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, WOOL)]);
    assert!(sent(&l).is_empty(), "no release");
    assert_eq!(seen(&mut l), "LOOT_CLOSED,LOOT_OPENED");
    assert_eq!(
        l.app.world().resource::<LootState>().source(),
        Some(LOCKBOX)
    );
    assert_eq!(l.app.world().resource::<LootLatch>().0, Some(LOCKBOX));
    assert_eq!(
        eval::<String>(
            &mut l,
            "local _, name = GetLootSlotInfo(1) return name or ''"
        ),
        "Wool Cloth"
    );
}

/// The same on the stock frames: `LOOT_CLOSED` hides `LootFrame` (`LootFrame.lua:53-55`), whose
/// `OnHide` calls `CloseLoot()` (`:143-145`), which finds no window at the copier's teardown; the
/// new window's `LOOT_OPENED` shows the frame again and the lockbox's loot is kept.
#[test]
fn the_stock_frames_hide_on_a_replacing_response_releases_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut l = client();
    {
        let mut s = l.app.world_mut().non_send_resource_mut::<UiScript>();
        s.set_screen_size(1024.0, 768.0);
        for f in crate::ui_script::test_ui::production_order(&[
            crate::ui_script::test_ui::LOOT_UI,
            &["Interface\\FrameXML\\LootFrame.xml"],
        ]) {
            crate::ui_script::test_ui::load_ui(&s, f);
        }
    }
    let shown = |l: &mut Loot| eval::<bool>(l, "return LootFrame:IsShown() == 1");
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    assert!(shown(&mut l), "the corpse's window");
    let _ = sent(&l);
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, WOOL)]);
    assert!(sent(&l).is_empty(), "no release");
    assert!(shown(&mut l), "the lockbox's window");
    assert_eq!(
        l.app.world().resource::<LootState>().source(),
        Some(LOCKBOX)
    );
}

/// Each function keeps its own confirm row: a take's (`0x4c2917`) and a sweep's (`0x4c21dc`). A
/// take's `LOOT_BIND_CONFIRM` whose handler closes a disenchant window runs the sweep inside it
/// (`0x48f264` → `0x4c1fa0`), which signals its own row's confirm and goes on past it; the take
/// then stores its row, which the accept sends.
#[test]
fn a_take_and_a_sweep_inside_it_keep_their_own_confirm_rows() {
    let mut l = client();
    open(
        &mut l,
        CORPSE,
        4,
        0,
        vec![row(0, FELSTRIKER), row(1, FELSTRIKER), row(2, LINEN)],
    );
    let _ = (seen(&mut l), sent(&l));
    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_BIND_CONFIRM")
        f:SetScript("OnEvent", function()
            if arg1 == 2 and not CLOSED then CLOSED = 1 CloseLoot() end
        end)"#,
    );
    click(&mut l, 2);
    assert_eq!(
        seen(&mut l),
        "LOOT_BIND_CONFIRM:2,LOOT_BIND_CONFIRM:1,LOOT_OPENED"
    );
    assert!(
        matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 2 }]),
        "the sweep went on past its confirm"
    );
    run(&mut l, "LootSlot(1)");
    assert!(sent(&l).is_empty(), "the sweep's row is not the stash");
    run(&mut l, "LootSlot(2)");
    assert!(
        matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 1 }]),
        "the take's row is"
    );
}

/// A close and the next response in one frame: the last row's removal closes inside its own
/// packet's dispatch (`0x4c1f8d` → `0x48f200`), before the response arrives, so a `CloseLoot` in
/// its `LOOT_CLOSED` handlers finds the closed window and touches nothing of the next, a
/// disenchant window the server started on a cold target (`0x5eb95b`), which opens.
#[test]
fn a_close_and_the_next_response_in_one_frame_keep_their_order() {
    let mut l = client();
    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_CLOSED")
        f:SetScript("OnEvent", function() CloseLoot() end)"#,
    );
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    packets(
        &mut l,
        vec![
            SessionEvent::LootRemoved { slot: 0 },
            SessionEvent::LootResponse {
                guid: LOCKBOX,
                loot_type: 4,
                gold: 0,
                items: vec![row(0, WOOL)],
            },
        ],
    );
    feed(&mut l);
    assert!(
        matches!(sent(&l)[..], [ClientCommand::LootRelease { guid: CORPSE }]),
        "the removal's close alone"
    );
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1,LOOT_CLOSED,LOOT_OPENED");
    assert_eq!(
        l.app.world().resource::<LootState>().source(),
        Some(LOCKBOX)
    );
}
