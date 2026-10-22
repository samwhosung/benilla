//! The item-received line's template (`OnItemPush`, `0x491a60`).

use benilla_protocol::messages::ItemPushResult;
use benilla_ui::script::UiScript;

use super::{drain_receives, LootState, PendingReceive};

fn pending(count: u32, from_npc: bool, created: bool) -> PendingReceive {
    let mut loot = LootState::default();
    loot.push_receive(
        &ItemPushResult {
            player_guid: 0x1,
            from_npc,
            created,
            show_in_chat: true,
            bag_slot: 0xFF,
            item_slot: 0,
            item_entry: 2589,
            suffix_factor: 0,
            random_property_id: 0,
            count,
        },
        true,
    );
    loot.take_push().expect("queued")
}

/// `OnItemPush` reads its template through `FrameScript_GetText` (`0x491c25`, `0x491c81`): created
/// first (`0x491c04`), then pushed by an NPC, then looted, each with a `_MULTIPLE` form for a count
/// above 1 (`0x491bfb`, `jg`), filled with the item's link (`0x52adb0`) and the count.
#[test]
fn an_item_received_line_reads_the_installs_strings() {
    let mut script = UiScript::new().unwrap();
    script
        .run(
            r#"
            LOOT_ITEM_SELF = "Beute: %s."
            LOOT_ITEM_SELF_MULTIPLE = "Beute: %s, %d Stueck."
            LOOT_ITEM_PUSHED_SELF = "Erhalten: %s."
            LOOT_ITEM_PUSHED_SELF_MULTIPLE = "Erhalten: %s, %d Stueck."
            LOOT_ITEM_CREATED_SELF = "Hergestellt: %s."
            LOOT_ITEM_CREATED_SELF_MULTIPLE = "Hergestellt: %s, %d Stueck."
        "#,
        )
        .unwrap();
    let mut items = crate::items::Items::default();
    items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
    let chat = crate::ui_chat::ChatLog::default();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let left = drain_receives(
        vec![
            pending(1, false, false),
            pending(3, false, false),
            pending(1, true, false),
            pending(2, true, false),
            pending(1, true, true),
            pending(5, false, true),
        ],
        &items,
        None,
        &crate::net::NetCommands(tx),
        &chat,
        &mut script,
        crate::items::RollCatalogs {
            props: None,
            enchants: None,
        },
    );
    assert!(left.is_empty());
    let link = "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r";
    assert_eq!(
        chat.pending_lines(),
        [
            format!("Beute: {link}."),
            format!("Beute: {link}, 3 Stueck."),
            format!("Erhalten: {link}."),
            format!("Erhalten: {link}, 2 Stueck."),
            format!("Hergestellt: {link}."),
            format!("Hergestellt: {link}, 5 Stueck."),
        ]
    );
}
