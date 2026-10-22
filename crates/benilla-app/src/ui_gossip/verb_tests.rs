//! The gossip verbs inside their calls, through the whole client's call host: each packet goes
//! out before the verb returns, so a chunk's packets leave in call order.

use benilla_protocol::messages::{GossipOption, NpcTextBlock};
use bevy::prelude::*;

use super::GossipState;
use crate::ui_quest::verb_tests::Client;

const NPC: u64 = 0xF130_0000_0000_0007;
const ROLL: &str = "RandomRoll { min: 1, max: 100 }";

/// `NPC`'s menu, its greeting drawn: option 1 is wire index 3, then active quest 5 (icon 3) and
/// available quest 6 (icon 5).
fn menu(c: &mut Client) {
    let world = c.app.world_mut();
    {
        let mut state = world.resource_mut::<GossipState>();
        state.remember_record(
            7,
            vec![NpcTextBlock {
                probability: 1.0,
                male: "Hello".into(),
                female: "Hello".into(),
            }],
        );
        state.open_menu(
            NPC,
            7,
            vec![GossipOption {
                index: 3,
                icon: 1,
                coded: false,
                message: "Browse".into(),
            }],
            vec![(5, 3, 1, "Active".into()), (6, 5, 1, "Available".into())],
            0,
        );
    }
    world
        .run_system_cached(super::feed_gossip)
        .expect("the feed runs");
}

/// `SelectGossipOption` (`0x4e2a30` → `0x4e2320`, the send at `0x4e23ef`) and the two quest-row
/// selects (`0x4e24b0`'s sends at `0x4e253c` and `0x4e2554`, `0x4e2600`'s at `0x4e267f`) send
/// inside their calls, so a roll on the chunk's next line follows them on the wire.
#[test]
fn each_gossip_verb_is_on_the_wire_before_the_next_line() {
    let mut c = Client::new(0);
    for (verb, packet) in [
        (
            "SelectGossipOption(1)",
            format!("GossipSelectOption {{ guid: {NPC}, option: 3, code: None }}"),
        ),
        (
            "SelectGossipAvailableQuest(1)",
            format!("QuestgiverQuery {{ npc: {NPC}, quest: 6 }}"),
        ),
        (
            "SelectGossipActiveQuest(1)",
            format!("QuestgiverComplete {{ npc: {NPC}, quest: 5 }}"),
        ),
    ] {
        menu(&mut c);
        c.sent();
        c.run(&format!("{verb} RandomRoll(1, 100)")).unwrap();
        assert_eq!(c.sent(), [packet, ROLL.to_string()], "{verb}");
    }
}

/// Records `GOSSIP_SHOW` and `GOSSIP_ENTER_CODE` with their first argument.
const RECORDER: &str = "SEEN = {} local f = CreateFrame('Frame') \
    f:RegisterEvent('GOSSIP_SHOW') f:RegisterEvent('GOSSIP_ENTER_CODE') \
    f:SetScript('OnEvent', function() table.insert(SEEN, event .. ':' .. tostring(arg1)) end)";

/// `NPC`'s menu with `options` and the two quest rows, its text cached when `drawn`.
fn menu_of(c: &mut Client, options: Vec<GossipOption>, drawn: bool) {
    let world = c.app.world_mut();
    {
        let mut state = world.resource_mut::<GossipState>();
        if drawn {
            state.remember_record(
                7,
                vec![NpcTextBlock {
                    probability: 1.0,
                    male: "Hello".into(),
                    female: "Hello".into(),
                }],
            );
        }
        state.open_menu(
            NPC,
            7,
            options,
            vec![(5, 3, 1, "Active".into()), (6, 5, 1, "Available".into())],
            0,
        );
    }
    world
        .run_system_cached(super::feed_gossip)
        .expect("the feed runs");
}

fn option(index: u32, coded: bool) -> GossipOption {
    GossipOption {
        index,
        icon: 0,
        coded,
        message: format!("Option {index}"),
    }
}

/// After its send `SelectGossipOption` resets the slots (`0x4e23f4` → `0x4e26a0`): every option's
/// index to -1, which the select refuses (`0x4e235f`) and `GetGossipOptions` skips, and every quest
/// id to 0, which ends both quest scans. The text stays, and no event fires.
#[test]
fn an_option_select_resets_the_menus_slots() {
    let mut c = Client::new(0);
    menu_of(&mut c, vec![option(3, false), option(4, false)], true);
    c.run(RECORDER).unwrap();
    c.sent();
    c.run("SelectGossipOption(1) SelectGossipOption(2) SelectGossipAvailableQuest(1)")
        .unwrap();
    assert_eq!(
        c.sent(),
        [format!(
            "GossipSelectOption {{ guid: {NPC}, option: 3, code: None }}"
        )]
    );
    assert_eq!(
        c.eval(
            "local function n(...) return arg.n end \
             return n(GetGossipOptions()) .. ',' .. n(GetGossipAvailableQuests()) .. ',' .. \
             n(GetGossipActiveQuests()) .. ',' .. GetGossipText()"
        ),
        "0,0,0,Hello"
    );
    c.app
        .world_mut()
        .run_system_cached(super::feed_gossip)
        .expect("the feed runs");
    assert_eq!(c.eval("return table.concat(SEEN, ',')"), "");
}

/// A coded option and no code fires `GOSSIP_ENTER_CODE` with the 1-based index and sends nothing
/// (`0x4e2365`-`0x4e237e`); with a code, a string or a number (`0x4e2a60`), the packet carries it
/// (`0x4e23e4`), and an uncoded option's packet never does (`0x4e23d4`).
#[test]
fn a_coded_option_asks_for_its_code_and_sends_it() {
    let mut c = Client::new(0);
    menu_of(&mut c, vec![option(4, false), option(3, true)], true);
    c.run(RECORDER).unwrap();
    c.sent();
    c.run("SelectGossipOption(2)").unwrap();
    assert!(c.sent().is_empty());
    assert_eq!(
        c.eval("return table.concat(SEEN, ',')"),
        "GOSSIP_ENTER_CODE:2"
    );
    for (lua, packet) in [
        (
            "SelectGossipOption(2, 'open sesame')",
            format!(
                "GossipSelectOption {{ guid: {NPC}, option: 3, code: Some({:?}) }}",
                b"open sesame".to_vec()
            ),
        ),
        (
            "SelectGossipOption(2, 77)",
            format!(
                "GossipSelectOption {{ guid: {NPC}, option: 3, code: Some({:?}) }}",
                b"77".to_vec()
            ),
        ),
        (
            "SelectGossipOption(1, 'unused')",
            format!("GossipSelectOption {{ guid: {NPC}, option: 4, code: None }}"),
        ),
    ] {
        menu_of(&mut c, vec![option(4, false), option(3, true)], true);
        c.sent();
        c.run(lua).unwrap();
        assert_eq!(c.sent(), [packet], "{lua}");
    }
}

/// A slot whose index is -1 is unused (`0x4e235f`), and only 16 slots exist (`0x4e2343`).
#[test]
fn an_unused_slot_and_a_slot_past_sixteen_send_nothing() {
    let mut c = Client::new(0);
    menu_of(&mut c, vec![option(u32::MAX, false)], true);
    c.sent();
    c.run("SelectGossipOption(1)").unwrap();
    assert!(c.sent().is_empty());
    menu_of(&mut c, (0..17).map(|i| option(i, false)).collect(), true);
    c.sent();
    c.run("SelectGossipOption(17)").unwrap();
    assert!(c.sent().is_empty());
}

/// The quest rows' selects test only for the active player (`0x4e250b`, `0x4e2657`), not the
/// pending greeting the option select waits on, and read the rows the message wrote.
#[test]
fn a_quest_row_selects_while_the_text_is_pending_but_not_without_a_player() {
    let mut c = Client::new(0);
    menu_of(&mut c, vec![option(3, false)], false);
    c.sent();
    c.run("SelectGossipOption(1) SelectGossipActiveQuest(1) SelectGossipAvailableQuest(1)")
        .unwrap();
    assert_eq!(
        c.sent(),
        [
            format!("QuestgiverComplete {{ npc: {NPC}, quest: 5 }}"),
            format!("QuestgiverQuery {{ npc: {NPC}, quest: 6 }}"),
        ]
    );
    c.drop_player();
    c.run("SelectGossipActiveQuest(1)").unwrap();
    assert!(c.sent().is_empty());
}

/// The code keeps its bytes up to its first NUL (`0x4e2a70`, `0x418430`), not a lossy text.
#[test]
fn a_code_keeps_its_bytes_up_to_its_nul() {
    let mut c = Client::new(0);
    for (lua, code) in [
        ("SelectGossipOption(1, 'ab\\0cd')", b"ab".to_vec()),
        ("SelectGossipOption(1, '\\255x')", vec![255, b'x']),
    ] {
        menu_of(&mut c, vec![option(3, true)], true);
        c.sent();
        c.run(lua).unwrap();
        assert_eq!(
            c.sent(),
            [format!(
                "GossipSelectOption {{ guid: {NPC}, option: 3, code: Some({code:?}) }}"
            )],
            "{lua}"
        );
    }
}

/// `CloseGossip` (`0x4e2b20` → `0x4e1fa0`) zeroes the cell inside its call (`0x4e1fcc`) and
/// leaves the slots: a select on the chunk's next line sends for guid 0, and so does one after
/// the drain's `GOSSIP_CLOSED`, whose getters still read the slots.
#[test]
fn close_gossip_zeroes_the_cell_and_keeps_the_slots() {
    let mut c = Client::new(0);
    menu_of(&mut c, vec![option(3, false)], true);
    c.sent();
    c.run("CloseGossip() SelectGossipOption(1)").unwrap();
    assert_eq!(
        c.sent(),
        ["GossipSelectOption { guid: 0, option: 3, code: None }"]
    );

    menu_of(&mut c, vec![option(3, false)], true);
    c.run(RECORDER).unwrap();
    c.run(
        "local g = CreateFrame('Frame') g:RegisterEvent('GOSSIP_CLOSED') \
         g:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
    )
    .unwrap();
    c.sent();
    c.run("CloseGossip()").unwrap();
    c.app
        .world_mut()
        .run_system_cached(super::drain_gossip)
        .expect("the drain runs");
    c.app
        .world_mut()
        .run_system_cached(super::feed_gossip)
        .expect("the feed runs");
    assert_eq!(
        c.eval("local function n(...) return arg.n end return n(GetGossipOptions()) .. ',' .. table.concat(SEEN, ',')"),
        "2,GOSSIP_CLOSED"
    );
    c.run("SelectGossipOption(1)").unwrap();
    assert_eq!(
        c.sent(),
        ["GossipSelectOption { guid: 0, option: 3, code: None }"]
    );
}
