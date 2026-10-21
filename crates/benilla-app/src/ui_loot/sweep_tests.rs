//! The auto-loot sweep `0x4c1fa0` through the whole client's host: its gates (`0x4c1fb3`,
//! `0x4c1fd9`), its money (`0x4c1ff4`-`0x4c2170`, `0xb71bc4`), the disenchant hold that runs it
//! (`0x48f245`-`0x48f264`), and a frame's responses each ending their own copier (`0x4c1ea8`).

use benilla_protocol::messages::GroupMemberEntry;
use benilla_protocol::SessionEvent;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use super::call_tests::{
    client, eval, feed, open, packets, row, run, seen, sent, Loot, CORPSE, LINEN, ME, THRALL, WOOL,
};
use super::{AutoLootByte, LootCloseAtRest, LootLatch};
use crate::net::{ClientCommand, GuidIndex, SelfPlayer};

/// Disenchanting: a window `CloseLoot` sweeps while rows are left.
const DISENCHANT: u8 = 4;
/// An entry the item cache has no record of.
const UNCACHED: u32 = 40_000;

/// A recorder for the money lines: `CHAT_MSG_MONEY` noted in `SEEN` in its place among the loot
/// events, and its text in `MONEY`.
fn note_money(l: &mut Loot) {
    run(
        l,
        r#"MONEY = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("CHAT_MSG_MONEY")
        f:SetScript("OnEvent", function() tinsert(SEEN, event) tinsert(MONEY, arg1) end)"#,
    );
}

/// The money lines signalled so far, in order.
fn money_lines(l: &mut Loot) -> Vec<String> {
    eval::<String>(l, "return table.concat(MONEY, '|')")
        .split('|')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// An auto-loot open: the byte the interaction stored, then the response.
fn auto_open(
    l: &mut Loot,
    loot_type: u8,
    gold: u32,
    items: Vec<benilla_protocol::messages::LootItem>,
) {
    l.app.world_mut().resource_mut::<AutoLootByte>().0 = true;
    open(l, CORPSE, loot_type, gold, items);
}

/// Money above 0 with the flag `0xb71bc4` clear (`0x4c2008`, `0x4c2015`): outside a party and a
/// raid, the line `YOU_LOOT_MONEY` with the coins, gold first, as `"%d %s"` joined by the
/// reference's separator tests (`0x4c2043`-`0x4c2123`), on chat type 86, added in place
/// (`0x4c2170` → `AddChatLine 0x49a870`): its `CHAT_MSG_MONEY` runs inside the sweep, ahead of the
/// rows and the sweep's `LOOT_OPENED` (`0x4c2240`). The sweep's end sets the flag (`0x4c221e`), so
/// a second sweep of the session prints none; the next session's copier clears it (`0x4c1d95`).
#[test]
fn the_sweep_prints_the_money_line_once_per_session() {
    let mut l = client();
    note_money(&mut l);
    auto_open(&mut l, DISENCHANT, 10_203, vec![row(1, LINEN)]);
    assert_eq!(money_lines(&mut l), ["You loot 1 Gold, 2 Silver, 3 Copper"]);
    assert_eq!(seen(&mut l), "CHAT_MSG_MONEY,LOOT_OPENED", "in place");
    let _ = sent(&l);
    run(&mut l, "CloseLoot()");
    assert!(
        matches!(
            sent(&l)[..],
            [
                ClientCommand::LootMoney,
                ClientCommand::AutostoreLootItem { slot: 1 }
            ]
        ),
        "the second sweep asks for the coin again (`0x4c1ffc`)"
    );
    assert_eq!(money_lines(&mut l).len(), 1, "but prints no second line");

    // The next session, a zero middle coin: both separators stand (`0x4c20c7`, `0x4c20e8`).
    auto_open(&mut l, DISENCHANT, 10_003, vec![row(1, LINEN)]);
    assert_eq!(
        money_lines(&mut l)[1..],
        ["You loot 1 Gold, , 3 Copper".to_string()]
    );
}

/// A party member in the first party slot (`[0xbc6f48]`, `0x4c2030`) or a raid roster
/// (`[0xb713e0]`, `0x4c203d`) skips the line; the coin request still goes. Alone, the next
/// session prints it.
#[test]
fn a_party_or_a_raid_skips_the_money_line() {
    for group_type in [0, 1] {
        let mut l = client();
        let member = GroupMemberEntry {
            name: "Thrall".into(),
            guid: THRALL,
            status: 1,
            flags: 0,
        };
        let group = |l: &mut Loot, members: Vec<GroupMemberEntry>| {
            l.app
                .world_mut()
                .resource_mut::<crate::ui_party::GroupState>()
                .apply_list(group_type, 0, members, ME, None, Some(ME));
        };
        note_money(&mut l);
        group(&mut l, vec![member]);
        auto_open(&mut l, 1, 500, vec![row(1, LINEN)]);
        assert!(money_lines(&mut l).is_empty(), "group type {group_type}");
        assert!(
            matches!(sent(&l)[..], [ClientCommand::LootMoney, ..]),
            "group type {group_type}"
        );
        let mut l = client();
        note_money(&mut l);
        auto_open(&mut l, 1, 500, vec![row(1, LINEN)]);
        assert_eq!(money_lines(&mut l), ["You loot 5 Silver"], "alone");
    }
}

/// While an item record is pending (`0xb71b44`) the sweep returns at once (`0x4c1fb3`): `CloseLoot`
/// on a disenchant window whose row's record has not come sends and signals nothing.
#[test]
fn a_sweep_with_a_record_pending_does_nothing() {
    let mut l = client();
    open(&mut l, CORPSE, DISENCHANT, 0, vec![row(1, UNCACHED)]);
    let _ = (seen(&mut l), sent(&l));
    run(&mut l, "CloseLoot()");
    assert!(sent(&l).is_empty());
    assert_eq!(seen(&mut l), "");
}

/// With no active player the sweep signals `LOOT_OPENED` and returns (`0x4c1fd9`, `0x4c1fe0`),
/// whatever its flag, sending nothing; the take dispatcher leaves at once (`0x4c27b6`).
#[test]
fn with_no_player_the_sweep_only_signals_and_a_take_does_nothing() {
    let mut l = client();
    open(&mut l, CORPSE, DISENCHANT, 0, vec![row(1, LINEN)]);
    let _ = (seen(&mut l), sent(&l));
    let world = l.app.world_mut();
    let me = world
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(world)
        .expect("our player");
    world.despawn(me);
    world.resource_mut::<GuidIndex>().0.remove(&ME);
    run(&mut l, "CloseLoot(1)");
    assert_eq!(seen(&mut l), "LOOT_OPENED");
    assert!(sent(&l).is_empty());
    super::call_tests::click(&mut l, 1);
    assert!(sent(&l).is_empty(), "the row's click takes nothing");
}

/// `0x48f200`'s hold asks `LootHasContents` (`0x48f24f` → `0x4c2a70`), which resolves the source
/// (`0x4c2a8c`): a disenchant window whose source has left the object manager is closed with
/// release at rest, not swept.
#[test]
fn a_disenchant_window_whose_source_is_gone_closes_at_rest() {
    let mut l = client();
    open(&mut l, CORPSE, DISENCHANT, 0, vec![row(1, LINEN)]);
    let _ = sent(&l);
    let world = l.app.world_mut();
    let corpse = world.resource_mut::<GuidIndex>().0.remove(&CORPSE).unwrap();
    world.despawn(corpse);
    world.resource_mut::<LootCloseAtRest>().0 = true;
    world.run_system_once(super::drain_loot).unwrap();
    assert!(matches!(
        sent(&l)[..],
        [ClientCommand::LootRelease { guid: CORPSE }]
    ));
    assert_eq!(l.app.world().resource::<LootLatch>().0, None);
}

/// Two responses in one frame each end their own copier, as each packet's dispatch runs it
/// (`0x4c1ea8`-`0x4c1ee6`): the first window signals `LOOT_OPENED` before the second's copier ends
/// it (`0x4c1dac`). Under the byte the first is swept, its own rows, and the sweep clears the byte
/// (`0x4c2219`), so the second opens plain.
#[test]
fn two_responses_in_one_frame_each_end_their_own_copier() {
    for byte in [false, true] {
        let mut l = client();
        l.app.world_mut().resource_mut::<AutoLootByte>().0 = byte;
        l.app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        let response = |item| SessionEvent::LootResponse {
            guid: CORPSE,
            loot_type: 1,
            gold: 0,
            items: vec![item],
        };
        packets(
            &mut l,
            vec![response(row(0, LINEN)), response(row(1, WOOL))],
        );
        feed(&mut l);
        assert_eq!(
            seen(&mut l),
            "LOOT_OPENED,LOOT_CLOSED,LOOT_OPENED",
            "byte {byte}"
        );
        let stores: Vec<ClientCommand> = sent(&l);
        if byte {
            assert!(
                matches!(stores[..], [ClientCommand::AutostoreLootItem { slot: 0 }]),
                "the first window's row: {stores:?}"
            );
        } else {
            assert!(stores.is_empty(), "{stores:?}");
        }
        assert!(!l.app.world().resource::<AutoLootByte>().0);
        assert_eq!(
            eval::<String>(
                &mut l,
                "local _, name = GetLootSlotInfo(1) return name or ''"
            ),
            "Wool Cloth",
            "the second window shows"
        );
    }
}

/// `autoLootDefault`, benilla's own option, auto-loots every window when on, with Shift held
/// opening it plain: a pickpocket or disenchant window, which no interaction stores the byte for,
/// takes it at its copier's end from the Shift held then. Off, they open plain, as in 1.12.
#[test]
fn auto_loot_default_sweeps_a_pickpocket_and_a_disenchant_window() {
    for loot_type in [2, DISENCHANT] {
        for (on, shift, swept) in [
            (true, false, true),
            (true, true, false),
            (false, false, false),
        ] {
            let mut l = client();
            l.app
                .world_mut()
                .resource_mut::<super::LootConfig>()
                .auto_loot = on;
            crate::script_calls::with_hosted_vm(l.app.world_mut(), |s| {
                s.expect("a VM").set_modifiers(shift, false, false);
            });
            open(&mut l, CORPSE, loot_type, 0, vec![row(0, LINEN)]);
            let stores = sent(&l);
            assert_eq!(
                matches!(stores[..], [ClientCommand::AutostoreLootItem { slot: 0 }]),
                swept,
                "type {loot_type}, on {on}, shift {shift}: {stores:?}"
            );
        }
    }
}
