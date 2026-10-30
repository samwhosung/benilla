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
    pub(super) rx: Receiver<ClientCommand>,
}

/// The whole client, headless, holding our player, [`CORPSE`] and [`LOCKBOX`], with the three
/// templates and a VM that notes the loot events in `SEEN`, defines the loot error keys as
/// themselves and the money line's strings as the stock ones.
pub(super) fn client() -> Loot {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(bevy::prelude::State::new(
        crate::client_state::ClientState::InWorld,
    ));
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
            LOOT_MONEY_SPLIT = "Your share of the loot is %s."
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

/// Run `lua` as a key's body, the hardware event raised for the movement gate (`0x494a50(0)`).
fn bound(l: &mut Loot, lua: &str) {
    with_hosted_vm(l.app.world_mut(), |s| s.expect("a VM").run_binding(lua))
        .expect("the chunk runs");
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

/// `SMSG_LOOT_ALL_PASSED` and `SMSG_LOOT_ROLL_WON` both run `ClearLootRollState` (`0x61b5dd`,
/// `0x61b97c` → `0x5ebcf0`), which zeroes the type of the open source's row: the next click on it
/// takes, where the pending row refused. Another source's packet, or another slot's, frees nothing.
#[test]
fn a_resolved_roll_frees_the_open_row() {
    let packet = |won: bool, guid: u64, slot: u32| {
        if won {
            SessionEvent::LootRollWon(benilla_protocol::messages::LootRollWon {
                looted_target: guid,
                item_slot: slot,
                item_id: LINEN,
                random_property_id: 0,
                suffix_factor: 0,
                winner: THRALL,
                roll_number: 50,
                roll_type: 2,
            })
        } else {
            SessionEvent::LootAllPassed(benilla_protocol::messages::LootAllPassed {
                looted_target: guid,
                item_slot: slot,
                item_id: LINEN,
                random_property_id: 0,
                suffix_factor: 0,
            })
        }
    };
    for won in [false, true] {
        let mut l = client();
        let (mut first, mut second) = (row(0, LINEN), row(1, WOOL));
        first.slot_type = slot_type::ROLL_ONGOING;
        second.slot_type = slot_type::ROLL_ONGOING;
        open(&mut l, CORPSE, 1, 0, vec![first, second]);
        let _ = (seen(&mut l), sent(&l));
        packets(&mut l, vec![packet(won, GONE, 0), packet(won, CORPSE, 7)]);
        click(&mut l, 1);
        assert_eq!(
            seen(&mut l),
            "UI_ERROR_MESSAGE:ERR_LOOT_ROLL_PENDING",
            "won: {won}"
        );
        packets(&mut l, vec![packet(won, CORPSE, 0)]);
        let _ = seen(&mut l);
        click(&mut l, 1);
        assert!(
            matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 0 }]),
            "won: {won}"
        );
        click(&mut l, 2);
        assert_eq!(
            seen(&mut l),
            "UI_ERROR_MESSAGE:ERR_LOOT_ROLL_PENDING",
            "the other row"
        );
    }
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

/// A movement start's tick closes the loot inside its call (`0x60dc6a` → `0x60e990` →
/// `0x48f200(1, 1, 0)` at `0x60e9dd`), and so does `Jump` (`0x60deae`, after its gates): the release
/// is out and `LOOT_CLOSED` has run on the next line. A turn under no mouse button starts too
/// (`0x60dd55`), and an item's loot stays (`0x48f2ab`). Main closed at the frame's input pass, and
/// a jump never closed.
#[test]
fn a_movement_start_closes_the_loot_inside_its_call() {
    for start in ["MoveForwardStart()", "TurnLeftStart()", "Jump()"] {
        let mut l = client();
        let world = l.app.world_mut();
        let me = world.resource::<crate::net::GuidIndex>().0[&ME];
        world.entity_mut(me).insert(crate::net::Embodied);
        world.init_resource::<crate::player::CameraControl>();
        world.init_resource::<crate::player::Player>();
        world.resource_mut::<crate::player::Player>().active = true;
        open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
        let _ = (seen(&mut l), sent(&l));
        bound(&mut l, &format!("{start} N = GetNumLootItems()"));
        assert!(
            matches!(sent(&l)[..], [ClientCommand::LootRelease { guid: CORPSE }]),
            "{start}"
        );
        assert_eq!(seen(&mut l), "LOOT_CLOSED", "{start}");
        assert_eq!(eval::<i64>(&mut l, "return N"), 0, "{start}");
        bound(&mut l, "MoveForwardStop() TurnLeftStop()");
        open(&mut l, LOCKBOX, 1, 0, vec![row(0, LINEN)]);
        let _ = (seen(&mut l), sent(&l));
        bound(&mut l, "StrafeLeftStart()");
        assert!(sent(&l).is_empty(), "an item's loot stays");
    }
}

/// A swimmer's pitch start runs the movement guard too (`0x60ddaa` → `0x60e990` → `0x48f200`): the
/// loot closes inside the call.
#[test]
fn a_swimmers_pitch_start_closes_the_loot_inside_its_call() {
    let mut l = client();
    let world = l.app.world_mut();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::Embodied);
    world.init_resource::<crate::player::CameraControl>();
    world.insert_resource(crate::player::Player::with_move_flags(
        crate::creature_anim::move_flags::SWIMMING,
    ));
    world.resource_mut::<crate::player::Player>().active = true;
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    bound(&mut l, "PitchDownStart() N = GetNumLootItems()");
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::LootRelease { guid: CORPSE }]
    ));
    assert_eq!(seen(&mut l), "LOOT_CLOSED");
    assert_eq!(eval::<i64>(&mut l, "return N"), 0);
}

/// A swimmer's pitch edge, the start and the stop alike, runs the movement guard inside its call
/// (`0x60ddaa`, `0x60ddda` → `0x60e990(1, 0)`), which stops the auto-move (`0x60e9b5`) before it
/// ends the loot window (`0x60e9dd`): a follow's `AUTOFOLLOW_END` runs ahead of `LOOT_CLOSED`.
#[test]
fn a_pitch_edge_ends_the_follow_then_closes_the_loot_inside_its_call() {
    for (before, edge) in [
        ("", "PitchDownStart()"),
        ("PitchDownStart()", "PitchDownStop()"),
    ] {
        let mut l = client();
        let world = l.app.world_mut();
        let me = world.resource::<crate::net::GuidIndex>().0[&ME];
        world.entity_mut(me).insert(crate::net::Embodied);
        world.init_resource::<crate::player::CameraControl>();
        world.insert_resource(crate::player::Player::with_move_flags(
            crate::creature_anim::move_flags::SWIMMING,
        ));
        world.resource_mut::<crate::player::Player>().active = true;
        run(
            &mut l,
            r#"local g = CreateFrame("Frame") g:RegisterEvent("AUTOFOLLOW_END")
            g:SetScript("OnEvent", function() tinsert(SEEN, event) end)"#,
        );
        if !before.is_empty() {
            bound(&mut l, before);
        }
        l.app
            .world_mut()
            .resource_mut::<crate::player::FollowState>()
            .start(0x77, "Bob".into());
        open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
        let _ = (seen(&mut l), sent(&l));
        bound(&mut l, edge);
        assert_eq!(seen(&mut l), "AUTOFOLLOW_END,LOOT_CLOSED", "{edge}");
    }
}

/// `StopAutoMove` signals the follow's end (`0x60fb84`) and its listeners run there, before the
/// move stops (`0x60e7f0`) and the guard ends the loot window (`0x60e9dd`): a listener's packet
/// goes out ahead of the window's release.
#[test]
fn a_follows_end_listener_sends_ahead_of_the_loot_release() {
    let mut l = client();
    let world = l.app.world_mut();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::Embodied);
    world.init_resource::<crate::player::CameraControl>();
    world.insert_resource(crate::player::Player::with_move_flags(
        crate::creature_anim::move_flags::SWIMMING,
    ));
    world.resource_mut::<crate::player::Player>().active = true;
    run(
        &mut l,
        r#"local g = CreateFrame("Frame") g:RegisterEvent("AUTOFOLLOW_END")
        g:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
    );
    l.app
        .world_mut()
        .resource_mut::<crate::player::FollowState>()
        .start(0x77, "Bob".into());
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN)]);
    let _ = l.rx.try_iter().count();
    bound(&mut l, "PitchDownStart()");
    let wire: Vec<&str> =
        l.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::RandomRoll { .. } => Some("roll"),
                ClientCommand::LootRelease { .. } => Some("release"),
                _ => None,
            })
            .collect();
    assert_eq!(wire, ["roll", "release"]);
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

/// A `LOOT_SLOT_CLEARED` listener that closes the window on the last row's removal closes it once:
/// the reference runs the listener's close first (`0x4c1f77`), and the removal's own then finds
/// no window (`0x4c1f7f` → `0x4c1f8d` → `0x48f23f`). The removal's release went at its packet, so
/// the listener's close takes that close's signals in place: one release, the lockbox's two
/// unlocks and one `LOOT_CLOSED`, all inside the listener's call.
#[test]
fn a_listeners_close_on_the_last_rows_removal_closes_once() {
    let mut l = client();
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, LINEN)]);
    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_SLOT_CLEARED")
        f:SetScript("OnEvent", function() CloseLoot() end)"#,
    );
    let _ = (seen(&mut l), sent(&l));
    packets(&mut l, vec![SessionEvent::LootRemoved { slot: 0 }]);
    feed(&mut l);
    assert_eq!(
        seen(&mut l),
        "LOOT_SLOT_CLEARED:1,ITEM_LOCK_CHANGED,ITEM_LOCK_CHANGED,LOOT_CLOSED"
    );
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::LootRelease { guid: LOCKBOX }]
    ));
    assert_eq!(l.app.world().resource::<LootState>().source(), None);
    assert_eq!(l.app.world().resource::<LootLatch>().0, None);
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

/// A response for an object the client does not hold shows `ERR_LOOT_DIDNT_KILL` inside its
/// handler and returns (`0x5ebb33`-`0x5ebb57`), before the latch (`0x5ebb60`) and the open.
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
    assert_eq!(seen(&mut l), "UI_ERROR_MESSAGE:ERR_LOOT_DIDNT_KILL");
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
        l.app
            .world()
            .resource::<LootState>()
            .queued_unlocks()
            .is_empty(),
        "signalled in their place, none left on the lane"
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
        seen(&mut l);
        packets(
            &mut l,
            vec![SessionEvent::LootError {
                guid: GONE,
                error: loot_error::TOO_FAR,
            }],
        );
        assert_eq!(
            seen(&mut l),
            "ITEM_LOCK_CHANGED",
            "held {held:?}: no line, the unlock at the packet"
        );
        assert_eq!(
            l.app.world().resource::<LootLatch>().0,
            None,
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

/// A removal and the next response in one frame: the removal's `LOOT_SLOT_CLEARED` handlers run
/// inside its own packet (`0x4c1f77`), before the response's copier clears the bind stash
/// (`[0x847cec]`, `0x4c1df5`), so a `LootSlot` there takes the pending row (`0x4c2790(i, 1)`,
/// `0x4c27c0`) and sends it.
#[test]
fn a_replaced_sessions_handlers_read_its_own_bind_stash() {
    let mut l = client();
    open(
        &mut l,
        CORPSE,
        1,
        0,
        vec![row(0, FELSTRIKER), row(1, LINEN)],
    );
    click(&mut l, 1);
    assert_eq!(seen(&mut l), "LOOT_OPENED,LOOT_BIND_CONFIRM:1");
    let _ = sent(&l);
    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_SLOT_CLEARED")
        f:SetScript("OnEvent", function() LootSlot(1) end)"#,
    );
    packets(
        &mut l,
        vec![
            SessionEvent::LootRemoved { slot: 1 },
            SessionEvent::LootResponse {
                guid: CORPSE,
                loot_type: 1,
                gold: 0,
                items: vec![row(0, WOOL)],
            },
        ],
    );
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:2,LOOT_CLOSED,LOOT_OPENED");
    assert!(
        matches!(sent(&l)[..], [ClientCommand::AutostoreLootItem { slot: 0 }]),
        "the old session's pending row"
    );
}

/// A `CloseLoot` in a replaced session's `LOOT_SLOT_CLEARED` handler ends that session alone
/// (`0x48f200`): the next response, a pickpocket window the server starts on a cold target
/// (`0x5eb94b`), still waits on its rows' records and signals `LOOT_OPENED` at the last answer
/// (`0x4c2ac0`, `0x4c2afc`).
#[test]
fn a_replaced_sessions_close_keeps_the_next_copiers_end() {
    const UNCACHED: u32 = 40_000;
    let mut l = client();
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN), row(1, WOOL)]);
    let _ = (seen(&mut l), sent(&l));
    run(
        &mut l,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_SLOT_CLEARED")
        f:SetScript("OnEvent", function() CloseLoot() end)"#,
    );
    packets(
        &mut l,
        vec![
            SessionEvent::LootRemoved { slot: 0 },
            SessionEvent::LootResponse {
                guid: CORPSE,
                loot_type: 2,
                gold: 0,
                items: vec![row(0, UNCACHED)],
            },
        ],
    );
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:1,LOOT_CLOSED");
    assert!(
        matches!(sent(&l)[..], [ClientCommand::LootRelease { guid: CORPSE }]),
        "the old session's release"
    );
    let world = l.app.world_mut();
    world
        .resource_mut::<Items>()
        .insert_template(UNCACHED, Some(crate::items::test_template("Silk Cloth")));
    world.resource_scope(|world, mut loot: Mut<LootState>| {
        loot.row_answered(world.resource::<Items>())
    });
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_OPENED");
}

/// A row another member takes before its item record answers stops the window waiting: the
/// window opens on the rows left, where 1.12.1's count stays above 0 and the corpse never opens
/// (`ClearLootRow 0x4c1f00` lowers nothing), a deviation `records_pending` records.
#[test]
fn a_row_cleared_while_its_record_is_pending_does_not_hold_the_window_shut() {
    const UNCACHED: u32 = 40_000;
    let mut l = client();
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN), row(1, UNCACHED)]);
    assert_eq!(seen(&mut l), "", "the copier waits on the record");
    packets(&mut l, vec![SessionEvent::LootRemoved { slot: 1 }]);
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_SLOT_CLEARED:2");
    let world = l.app.world_mut();
    world
        .resource_mut::<Items>()
        .insert_template(UNCACHED, Some(crate::items::test_template("Silk Cloth")));
    world.resource_scope(|world, mut loot: Mut<LootState>| {
        loot.row_answered(world.resource::<Items>())
    });
    feed(&mut l);
    assert_eq!(seen(&mut l), "LOOT_OPENED");
}

/// An inventory failure ends by signalling `LOOT_OPENED` again while a window is open
/// (`0xb71b48`) and the auto-loot byte is set (`0x5e3aff`-`0x5e3b20`): a window whose copier
/// waits on a row's record keeps the byte its interaction stored (`0x4c1ea6`, before the sweep's
/// clear at `0x4c2219`). With the byte clear the failure signals no loot event.
#[test]
fn an_inventory_failure_signals_the_window_again_under_the_byte() {
    for byte in [true, false] {
        let mut l = client();
        l.app.world_mut().resource_mut::<AutoLootByte>().0 = byte;
        run(&mut l, "ERR_INV_FULL = 'ERR_INV_FULL'");
        open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN), row(1, 40_000)]);
        assert_eq!(seen(&mut l), "", "the copier waits on its record");
        packets(
            &mut l,
            vec![SessionEvent::InventoryFailure {
                reason: 50,
                required_level: None,
                item_guid: 0,
                item2_guid: 0,
                bag_slot: 0,
            }],
        );
        crate::lanes::fire_in_order(l.app.world_mut());
        let seen = seen(&mut l);
        assert!(seen.starts_with("UI_ERROR_MESSAGE"), "{seen}");
        assert_eq!(seen.ends_with(",LOOT_OPENED"), byte, "byte {byte}: {seen}");
    }
}

/// The failure's tail reads whether a window is open at its own packet (`0x5e3af3`-`0x5e3aff`):
/// a response later in the same drain opens its window after it, so the tail signals nothing,
/// though the byte is set and that window's copier waits on its record.
#[test]
fn an_inventory_failure_reads_the_window_at_its_own_packet() {
    let mut l = client();
    run(&mut l, "ERR_INV_FULL = 'ERR_INV_FULL'");
    l.app.world_mut().resource_mut::<AutoLootByte>().0 = true;
    l.app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
    packets(
        &mut l,
        vec![
            SessionEvent::InventoryFailure {
                reason: 50,
                required_level: None,
                item_guid: 0,
                item2_guid: 0,
                bag_slot: 0,
            },
            SessionEvent::LootResponse {
                guid: CORPSE,
                loot_type: 1,
                gold: 0,
                items: vec![row(0, 40_000)],
            },
        ],
    );
    crate::lanes::fire_in_order(l.app.world_mut());
    assert_eq!(seen(&mut l), "UI_ERROR_MESSAGE:ERR_INV_FULL");
    assert!(l.app.world().resource::<AutoLootByte>().0, "no sweep ran");
}

/// Our own push of `entry` into the backpack, shown in chat.
fn pushed(entry: u32) -> ItemPushResult {
    ItemPushResult {
        player_guid: ME,
        from_npc: false,
        created: false,
        show_in_chat: true,
        bag_slot: benilla_protocol::messages::BAG_PLAYER_INVENTORY,
        item_slot: 23,
        item_entry: entry,
        suffix_factor: 0,
        random_property_id: 0,
        count: 1,
    }
}

/// `SetLootPortrait` answers 1 only while the loot source is a unit object we hold
/// (`0x4c2bd1`-`0x4c2be9`): a creature's corpse, never an item or a closed window.
#[test]
fn the_loot_portrait_follows_a_unit_source_only() {
    let mut l = client();
    // `OBJECT_FIELD_TYPE` (2): `TYPEMASK_OBJECT | TYPEMASK_UNIT`, which the portrait asks for.
    let world = l.app.world_mut();
    let corpse = world.resource::<GuidIndex>().0[&CORPSE];
    world
        .entity_mut(corpse)
        .insert(ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x9),
            (22, 0),
            (28, 100),
        ])));
    run(
        &mut l,
        r#"CreateFrame("Frame", "LootHost"):CreateTexture("LootTex")"#,
    );
    let portrait = |l: &mut Loot| eval::<bool>(l, "return SetLootPortrait(LootTex) == 1");
    assert!(!portrait(&mut l), "no window");
    open(&mut l, CORPSE, 1, 5, Vec::new());
    assert!(portrait(&mut l), "a corpse");
    run(&mut l, "CloseLoot()");
    open(&mut l, LOCKBOX, 1, 5, Vec::new());
    assert!(!portrait(&mut l), "an item");
}

/// The response's tail closes a window that opened on the run, our body moving as it lands, after
/// the copier's open and sweep (`0x5ebc3a`-`0x5ebc51`), so `LOOT_OPENED`'s handlers read the open
/// window, its coin row included, and `LOOT_CLOSED`'s the closed one.
#[test]
fn a_window_opened_on_the_run_is_open_to_its_loot_opened_handlers() {
    let mut l = client();
    run(
        &mut l,
        r#"COUNTS = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_OPENED")
        f:RegisterEvent("LOOT_CLOSED")
        f:SetScript("OnEvent", function()
            table.insert(COUNTS, event .. ":" .. GetNumLootItems())
        end)"#,
    );
    l.app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
    l.app
        .world_mut()
        .insert_resource(crate::player::Player::with_move_flags(
            crate::creature_anim::move_flags::FORWARD,
        ));
    packets(
        &mut l,
        vec![SessionEvent::LootResponse {
            guid: CORPSE,
            loot_type: 1,
            gold: 5,
            items: Vec::new(),
        }],
    );
    feed(&mut l);
    assert_eq!(
        eval::<String>(&mut l, "return table.concat(COUNTS, ',')"),
        "LOOT_OPENED:1,LOOT_CLOSED:0"
    );
}

/// The sweep's loot leg on a lockbox: `CloseInteraction`'s unlock (`0x48f2bd`) and the
/// teardown's (`0x4c1d36`) come before the teardown zeroes the source (`0x4c1cb0`) and signals
/// `LOOT_CLOSED` (`0x4c1dac`), so each unlock's `ITEM_LOCK_CHANGED` handlers read the window
/// still open, and `LOOT_CLOSED`'s the closed one.
#[test]
fn a_closing_lockboxs_unlock_handlers_read_the_window_open() {
    let mut l = client();
    run(
        &mut l,
        r#"COUNTS = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_OPENED")
        f:RegisterEvent("ITEM_LOCK_CHANGED")
        f:RegisterEvent("LOOT_CLOSED")
        f:SetScript("OnEvent", function()
            table.insert(COUNTS, event .. ":" .. GetNumLootItems())
        end)"#,
    );
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, LINEN)]);
    crate::ui_script::sweep_test::sweep_in_place(&mut l.app);
    assert_eq!(
        eval::<String>(&mut l, "return table.concat(COUNTS, ',')"),
        "LOOT_OPENED:1,ITEM_LOCK_CHANGED:1,ITEM_LOCK_CHANGED:1,LOOT_CLOSED:0"
    );
}

/// A `CloseLoot` signals its two unlocks with the window still open and its `LOOT_CLOSED` with it
/// closed (`0x48f2bd`, `0x4c1d36`, `0x4c1da0`, `0x4c1dac`), inside the call.
#[test]
fn close_loots_unlock_handlers_read_the_window_open() {
    let mut l = client();
    run(
        &mut l,
        r#"COUNTS = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("ITEM_LOCK_CHANGED")
        f:RegisterEvent("LOOT_CLOSED")
        f:SetScript("OnEvent", function()
            table.insert(COUNTS, event .. ":" .. GetNumLootItems())
        end)"#,
    );
    open(&mut l, LOCKBOX, 1, 0, vec![row(0, LINEN)]);
    run(
        &mut l,
        "CloseLoot() table.insert(COUNTS, 'after:' .. GetNumLootItems())",
    );
    assert_eq!(
        eval::<String>(&mut l, "return table.concat(COUNTS, ',')"),
        "ITEM_LOCK_CHANGED:1,ITEM_LOCK_CHANGED:1,LOOT_CLOSED:0,after:0"
    );
}

/// `OnItemPush` signals `ITEM_PUSH` in the push's handler (`0x491be8`), which touches no loot
/// window, so its handlers read the window as the packets before it left it: none, here, for a
/// push ahead of the response.
#[test]
fn an_item_pushs_handlers_read_the_loot_before_the_response() {
    let mut l = client();
    run(
        &mut l,
        r#"COUNTS = ""
        local f = CreateFrame("Frame")
        f:RegisterEvent("ITEM_PUSH")
        f:SetScript("OnEvent", function() COUNTS = COUNTS .. GetNumLootItems() end)"#,
    );
    l.app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
    packets(
        &mut l,
        vec![
            SessionEvent::ItemPushResult(pushed(LINEN)),
            SessionEvent::LootResponse {
                guid: CORPSE,
                loot_type: 1,
                gold: 0,
                items: vec![row(0, LINEN)],
            },
        ],
    );
    feed(&mut l);
    assert_eq!(eval::<String>(&mut l, "return COUNTS"), "0");
    assert_eq!(eval::<i64>(&mut l, "return GetNumLootItems()"), 1);
}

/// A VM noting `LOOT_OPENED`, each `LOOT_SLOT_CLEARED` row and, at each `ITEM_PUSH`, which of rows
/// 1 and 2 still hold an item.
fn push_order(l: &mut Loot) {
    run(
        l,
        r#"ORDER = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_OPENED")
        f:RegisterEvent("LOOT_SLOT_CLEARED")
        f:RegisterEvent("ITEM_PUSH")
        f:SetScript("OnEvent", function()
            if event == "LOOT_OPENED" then
                tinsert(ORDER, "OPENED")
            elseif event == "LOOT_SLOT_CLEARED" then
                tinsert(ORDER, "CLEARED" .. arg1)
            else
                local rows = ""
                for i = 1, 2 do rows = rows .. ((GetLootSlotInfo(i)) and 1 or 0) end
                tinsert(ORDER, "PUSH" .. rows)
            end
        end)"#,
    );
}

fn order(l: &mut Loot) -> String {
    eval(l, "local s = table.concat(ORDER, ',') ORDER = {} return s")
}

/// Each packet's events fire inside its dispatch: `SMSG_LOOT_REMOVED` clears its row
/// (`0x5ebeb0` → `0x4c1f00`) ahead of the `SMSG_ITEM_PUSH_RESULT` vmangos sends after it
/// (`LootHandler.cpp:217`, `:230`), so each `ITEM_PUSH` reads the rows removed before it gone
/// and the rows removed after it still there.
#[test]
fn each_item_push_reads_the_rows_removed_before_it() {
    let mut l = client();
    push_order(&mut l);
    open(&mut l, CORPSE, 1, 0, vec![row(0, LINEN), row(1, WOOL)]);
    assert_eq!(order(&mut l), "OPENED");
    packets(
        &mut l,
        vec![
            SessionEvent::LootRemoved { slot: 0 },
            SessionEvent::ItemPushResult(pushed(LINEN)),
            SessionEvent::LootRemoved { slot: 1 },
            SessionEvent::ItemPushResult(pushed(WOOL)),
        ],
    );
    feed(&mut l);
    assert_eq!(order(&mut l), "CLEARED1,PUSH01,CLEARED2,PUSH00");
}

/// A push after the response in the same frame fires after the response's `LOOT_OPENED`
/// (`0x4c2ac0`), so its handlers read the window open.
#[test]
fn an_item_push_after_the_response_reads_the_window_open() {
    let mut l = client();
    push_order(&mut l);
    l.app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
    packets(
        &mut l,
        vec![
            SessionEvent::LootResponse {
                guid: CORPSE,
                loot_type: 1,
                gold: 0,
                items: vec![row(0, LINEN)],
            },
            SessionEvent::ItemPushResult(pushed(WOOL)),
        ],
    );
    feed(&mut l);
    assert_eq!(order(&mut l), "OPENED,PUSH10");
}

/// A push whose record the cache lacks waits on its answer (`0x491a93` → `0x491ee0`), and a
/// refused answer frees it unrun (`0x491ee8`, `0x491efb` → `SMemFree 0x646430`): no line and no
/// `ITEM_PUSH`, then or at a later answer for the entry.
#[test]
fn a_push_whose_record_is_refused_is_dropped() {
    const UNCACHED: u32 = 40_000;
    let mut l = client();
    let _ = seen(&mut l);
    packets(&mut l, vec![SessionEvent::ItemPushResult(pushed(UNCACHED))]);
    feed(&mut l);
    assert_eq!(l.app.world().resource::<LootState>().waiting.len(), 1);
    packets(
        &mut l,
        vec![SessionEvent::ItemTemplate {
            entry: UNCACHED,
            info: None,
        }],
    );
    feed(&mut l);
    assert!(l.app.world().resource::<LootState>().waiting.is_empty());
    packets(
        &mut l,
        vec![SessionEvent::ItemTemplate {
            entry: UNCACHED,
            info: Some(Box::new(crate::items::test_template("Silk Cloth"))),
        }],
    );
    feed(&mut l);
    assert_eq!(seen(&mut l), "");
}
