//! The group loot rolls' getters as a listener reads them, and their signals at their packets. The
//! getters find the roll by id (`0x61b2e0`) and look its item up at the call (`0x4c3050`-
//! `0x4c3360`); `START_LOOT_ROLL` fires inside the start packet that finds its record cached
//! (`0x61b310` → `0x61b430`), and a result's line and `CANCEL_LOOT_ROLL` inside the result's
//! packet (`0x61b870` → `0x61b9e0`).

use benilla_protocol::messages::{roll_vote, LootRollWon, LootStartRoll};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, fall, frame};

/// A fall's `UNIT_COMBAT`, as [`client`]'s spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";
const CORPSE: u64 = 0xF130_0000_0044_00AA;
const ROLLED: u32 = 17182;
const WINNER: u64 = 0x51;

/// [`ROLLED`] cached as Sulfuras, [`WINNER`] named Bob, the won line's template in the strings,
/// and a spy on the roll events and the loot line, `START_LOOT_ROLL` with the name and the time
/// left its getters read as it fires.
fn rolls_client() -> App {
    let mut app = client();
    let world = app.world_mut();
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(ROLLED, Some(crate::items::test_template("Sulfuras")));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(WINNER, "Bob".into(), None);
    world
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            LOOT_ROLL_WON = "%s won: %s|Hitem:%d:%d:%d:%d|h[%s]|h%s"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"START_LOOT_ROLL", "CANCEL_LOOT_ROLL", "CHAT_MSG_LOOT"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                if event == "START_LOOT_ROLL" then
                    local _, name = GetLootRollItemInfo(arg1)
                    table.insert(Seen, event .. ":" .. arg1 .. ":" .. arg2 .. ":"
                        .. tostring(name))
                elseif event == "CHAT_MSG_LOOT" then
                    table.insert(Seen, event)
                else
                    table.insert(Seen, event .. ":" .. arg1)
                end
            end)
            "#,
        )
        .unwrap();
    app
}

fn start(slot: u32) -> SessionEvent {
    SessionEvent::LootStartRoll(LootStartRoll {
        looted_target: CORPSE,
        item_slot: slot,
        item_id: ROLLED,
        random_property_id: 0,
        suffix_factor: 0,
        countdown_ms: 60_000,
    })
}

fn won(slot: u32) -> SessionEvent {
    SessionEvent::LootRollWon(LootRollWon {
        looted_target: CORPSE,
        item_slot: slot,
        item_id: ROLLED,
        random_property_id: 0,
        suffix_factor: 0,
        winner: WINNER,
        roll_number: 84,
        roll_type: roll_vote::NEED,
    })
}

/// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, live: bool, chunk: &str) -> T {
    if live {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval::<T>(chunk).expect("runs")
        })
    } else {
        crate::script_calls::eval_in_vm(app.world(), chunk)
    }
}

const READ: &str = "local _, name, _, quality = GetLootRollItemInfo(1) \
    return tostring(name) .. '/' .. quality .. '/' .. tostring(GetLootRollItemLink(1) ~= nil) \
    .. '/' .. tostring(GetLootRollTimeLeft(1) > 59000)";

/// With no frame after the start packet, the getters read the engine's roll and its record through
/// the host; the VM's copy, which nothing pushes, holds none.
#[test]
fn the_roll_getters_read_the_list_at_the_call() {
    let mut app = rolls_client();
    crate::net::handlers::dispatch(app.world_mut(), vec![start(0)]);
    assert_eq!(eval::<String>(&mut app, true, READ), "Sulfuras/1/true/true");
    assert_eq!(eval::<String>(&mut app, false, READ), "nil/1/false/false");
}

/// The start's `START_LOOT_ROLL` fires at its packet's place between a fall's `UNIT_COMBAT` on
/// each side, its listener reading the roll's item.
#[test]
fn start_loot_roll_fires_between_neighbouring_packets_events() {
    let mut app = rolls_client();
    let seen = frame(&mut app, vec![fall(5), start(0), fall(5)]);
    assert_eq!(seen, [FELL, "START_LOOT_ROLL:1:60000:Sulfuras", FELL]);
}

/// A roll started and won in one frame: its start, then the win's line and cancel, each at its
/// packet's place; the frame leaves no roll listed, nothing on the lane, and no vote to send, and
/// the next frame signals nothing.
#[test]
fn a_roll_started_and_cancelled_in_one_frame_leaves_no_stale_state() {
    let mut app = rolls_client();
    let seen = frame(&mut app, vec![start(0), fall(5), won(0)]);
    assert_eq!(
        seen[0].split(':').take(3).collect::<Vec<_>>(),
        ["START_LOOT_ROLL", "1", "60000"]
    );
    assert_eq!(&seen[1..], [FELL, "CHAT_MSG_LOOT", "CANCEL_LOOT_ROLL:1"]);
    let rolls = app.world().resource::<crate::ui_loot_roll::LootRolls>();
    assert!(!rolls.holds(1));
    assert_eq!(eval::<String>(&mut app, true, READ), "nil/1/false/false");
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    eval::<()>(&mut app, true, "RollOnLoot(1, 0)");
    assert!(rx.try_iter().next().is_none(), "no roll to vote on");
    assert!(frame(&mut app, vec![]).is_empty());
}
