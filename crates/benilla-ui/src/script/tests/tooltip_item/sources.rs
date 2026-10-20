//! What each `Set*Item` hands the builder, and the lines that hang off it: the item object, the
//! instance block, the link's fields and the trade window's slots.

use std::collections::HashMap;

use super::{lines_of, script};
use crate::script::*;

/// Render through `call` (a `TT` method call, `TT` owned first) and answer the lines' texts.
fn texts_after(s: &mut UiScript, call: &str) -> Vec<String> {
    s.run(&format!(
        r#"
        local a = SourceSlot or CreateFrame("Button", "SourceSlot")
        a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        {call}
    "#
    ))
    .unwrap();
    lines_of(s).into_iter().map(|(t, _)| t).collect()
}

/// A book: its template carries page text, so with an item object it reads.
fn book() -> ItemTemplateView {
    ItemTemplateView {
        name: "Dusty Tome".into(),
        quality: 1,
        page_text: 55,
        ..Default::default()
    }
}

/// The open and read hints need an item object (`0x52e2e0 test edi,edi; je 0x52e376`): a chat link
/// (`0x5321e2`), a loot row (`0x5335c9`) and the partner's trade slot (`0x5345d4`) pass a zero
/// guid, so a book shows no `<Right Click to Read>` there; a bag slot and the player's own trade
/// slot (`0x5343d5`) hold the object and do.
#[test]
fn only_a_source_with_an_item_object_reads() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(9200, book());
    let read = "[ITEM_READABLE]".to_string();
    let link = texts_after(&mut s, r#"TT:SetHyperlink("item:9200:0:0:0")"#);
    assert!(!link.contains(&read), "a link has no object: {link:?}");
    s.set_loot(Some(LootState {
        rows: vec![Some(LootRow {
            name: Some("Dusty Tome".into()),
            quantity: 1,
            quality: Some(1),
            item_id: 9200,
            ..Default::default()
        })],
        ..Default::default()
    }));
    let loot = texts_after(&mut s, "TT:SetLootItem(1)");
    assert_eq!(loot.first().map(String::as_str), Some("Dusty Tome"));
    assert!(!loot.contains(&read), "a loot row has no object: {loot:?}");
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([(
                1,
                ContainerSlot {
                    item_id: 9200,
                    count: 1,
                    quality: Some(1),
                    guid: 0x4000_0000_0000_0001,
                    ..Default::default()
                },
            )]),
        }),
    );
    let bag = texts_after(&mut s, "TT:SetBagItem(0, 1)");
    assert!(bag.contains(&read), "a bag slot holds the object: {bag:?}");
    let traded = TradeSlotItem {
        item_id: 9200,
        name: Some("Dusty Tome".into()),
        count: 1,
        quality: Some(1),
        ..Default::default()
    };
    let mut trade = TradeState::default();
    trade.player.slots[0] = Some(TradeSlotItem {
        object_flags: Some(0),
        ..traded.clone()
    });
    trade.target.slots[0] = Some(traded);
    s.set_trade(Some(trade));
    let ours = texts_after(&mut s, "TT:SetTradePlayerItem(1)");
    assert!(ours.contains(&read), "our trade slot is our object: {ours:?}");
    let theirs = texts_after(&mut s, "TT:SetTradeTargetItem(1)");
    assert!(!theirs.contains(&read), "the partner's slot is a block: {theirs:?}");
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The bank vault's slots reach `SetInventoryItem` as item objects (`0x53314e` → `0x6228a0`), so a
/// saved letter's text id picks `ITEM_WRITTEN_BY` (`0x52e21b`) and the read hint (`0x52e34e`), and
/// with no cooldown running the binding builds with p6 = 0 (`0x5332ad`), so a lootable box there
/// shows the open hint (`0x52e2f2`..`0x52e323`).
#[test]
fn a_bank_vault_item_keeps_its_text_and_its_open_hint() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        9201,
        ItemTemplateView {
            name: "Plain Letter".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.set_item_template(
        9202,
        ItemTemplateView {
            name: "Battered Chest".into(),
            quality: 1,
            flags: 0x4,
            ..Default::default()
        },
    );
    let slot = |item_id, readable, creator: Option<&str>| ContainerSlot {
        item_id,
        count: 1,
        quality: Some(1),
        guid: 0x4000_0000_0000_0100 + u64::from(item_id),
        readable,
        creator: creator.map(str::to_string),
        creator_guid: u64::from(creator.is_some()),
        ..Default::default()
    };
    s.set_container(
        -1,
        Some(ContainerState {
            name: Some("Bank".into()),
            num_slots: 24,
            slots: HashMap::from([
                (1, slot(9201, true, Some("Mailer"))),
                (2, slot(9202, false, None)),
            ]),
        }),
    );
    let letter = texts_after(&mut s, r#"TT:SetInventoryItem("player", 40)"#);
    assert_eq!(
        letter,
        ["Plain Letter", "[WRITTEN_BY Mailer]", "[ITEM_READABLE]"],
        "a letter in the vault reads as one"
    );
    let chest = texts_after(&mut s, r#"TT:SetInventoryItem("player", 41)"#);
    assert_eq!(chest, ["Battered Chest", "[ITEM_OPENABLE]"]);
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}
