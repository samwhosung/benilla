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
            format!("GossipSelectOption {{ guid: {NPC}, option: 3 }}"),
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
