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
        &|_| None,
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

fn drain(script: &mut UiScript, batch: Vec<PendingReceive>) -> Vec<String> {
    let mut items = crate::items::Items::default();
    items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
    let chat = crate::ui_chat::ChatLog::default();
    let (tx, _rx) = crossbeam_channel::unbounded();
    // `0x468460` with mask 0x10 holds the pusher as a player (`0x491b2c`); `0x609210` names it.
    let owner = |guid: u64| match guid {
        BOB => Some(Some("Bob".to_string())),
        NAMELESS => Some(None),
        _ => None,
    };
    drain_receives(
        batch,
        &items,
        None,
        &crate::net::NetCommands(tx),
        &chat,
        script,
        crate::items::RollCatalogs {
            props: None,
            enchants: None,
        },
        &owner,
    );
    chat.pending_lines()
}

const BOB: u64 = 0x51;
const NAMELESS: u64 = 0x52;
const UNHELD: u64 = 0x53;

/// Another member's push (`0x491d9f`): `CREATED_ITEM` or `LOOT_ITEM`, `_MULTIPLE` for a count
/// above 1 (`0x491dbc`), filled with `GetUnitName`'s name for the pusher (`0x491dfe`, its miss
/// `UNKNOWNOBJECT`) and the item's link; nothing for a pusher the client does not hold.
#[test]
fn another_members_item_line_names_them() {
    let mut script = UiScript::new().unwrap();
    script
        .run(
            r#"
            UNKNOWNOBJECT = "Unbekannt"
            LOOT_ITEM = "%s bekommt: %s."
            LOOT_ITEM_MULTIPLE = "%s bekommt: %s, %d Stueck."
            CREATED_ITEM = "%s stellt her: %s."
            CREATED_ITEM_MULTIPLE = "%s stellt her: %s, %d Stueck."
        "#,
        )
        .unwrap();
    let theirs = |owner: u64, count: u32, created: bool| PendingReceive {
        owner: Some(owner),
        ..pending(count, false, created)
    };
    let link = "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r";
    assert_eq!(
        drain(
            &mut script,
            vec![
                theirs(BOB, 1, false),
                theirs(BOB, 4, false),
                theirs(BOB, 1, true),
                theirs(BOB, 2, true),
                theirs(NAMELESS, 1, false),
                theirs(UNHELD, 1, false),
            ]
        ),
        [
            format!("Bob bekommt: {link}."),
            format!("Bob bekommt: {link}, 4 Stueck."),
            format!("Bob stellt her: {link}."),
            format!("Bob stellt her: {link}, 2 Stueck."),
            format!("Unbekannt bekommt: {link}."),
        ]
    );
}

/// The count is compared signed (`0x491bfb`, `jg`): a count past `i32::MAX` takes the single form.
#[test]
fn the_receive_count_compares_signed() {
    let mut script = UiScript::new().unwrap();
    script
        .run(
            r#"
            LOOT_ITEM_SELF = "Beute: %s."
            LOOT_ITEM_SELF_MULTIPLE = "Beute: %s, %d Stueck."
        "#,
        )
        .unwrap();
    let link = "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r";
    assert_eq!(
        drain(&mut script, vec![pending(0x8000_0000, false, false)]),
        [format!("Beute: {link}.")]
    );
}
