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

/// Opens a trade showing `state` and offers each `(bag slot, trade slot)` of the backpack by a
/// click, which records the item's guid in our slot (`0x4bfa70`, the table at `0xb716d0`).
fn offer(s: &mut UiScript, state: TradeState, slots: &[(u32, u32)]) {
    s.open_trade_window();
    s.set_trade(Some(state));
    for (bag_slot, trade_slot) in slots {
        s.run(&format!(
            "PickupContainerItem(0, {bag_slot}) ClickTradeButton({trade_slot})"
        ))
        .unwrap();
    }
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
    let mut trade = TradeState::default();
    trade.target.slots[0] = Some(TradeSlotItem {
        item_id: 9200,
        name: Some("Dusty Tome".into()),
        count: 1,
        quality: Some(1),
        ..Default::default()
    });
    offer(&mut s, trade, &[(1, 1)]);
    let ours = texts_after(&mut s, "TT:SetTradePlayerItem(1)");
    assert!(
        ours.contains(&read),
        "our trade slot is our object: {ours:?}"
    );
    let theirs = texts_after(&mut s, "TT:SetTradeTargetItem(1)");
    assert!(
        !theirs.contains(&read),
        "the partner's slot is a block: {theirs:?}"
    );
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

/// `SetHyperlink` stores the link's enchant as the block's first enchant (`0x5321d3`) and builds
/// with p6 = 1 (`0x532198`), so the no-object enchant loop names it from `SpellItemEnchantment`
/// (`0x52c9f2`, `0x52ca0c`..`0x52ca1e`): green for a positive id, pure red for a negative one
/// (`0x52ca38`..`0x52ca46`), ahead of the roll's slots.
#[test]
fn a_link_shows_its_enchant() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        9300,
        ItemTemplateView {
            name: "Runed Blade".into(),
            quality: 2,
            ..Default::default()
        },
    );
    s.set_enchant_names(HashMap::from([(1900, "Crusader".to_string())]));
    s.run(r#"TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")"#)
        .unwrap();
    let texts = texts_after(&mut s, r#"TT:SetHyperlink("item:9300:1900:0:0")"#);
    assert_eq!(texts, ["Runed Blade", "Crusader"]);
    let lines = lines_of(&mut s);
    assert_eq!(lines[1].1, [0.0, 1.0, 0.0, 1.0], "a positive id is green");
    let texts = texts_after(&mut s, r#"TT:SetHyperlink("item:9300:-1900:0:0")"#);
    assert_eq!(texts, ["Runed Blade", "Crusader"]);
    let lines = lines_of(&mut s);
    assert_eq!(
        lines[1].1,
        [1.0, 0.0, 0.0, 1.0],
        "a negative id is pure red"
    );
    // An id with no row adds nothing (`0x52ca06`).
    let texts = texts_after(&mut s, r#"TT:SetHyperlink("item:9300:1901:0:0")"#);
    assert_eq!(texts, ["Runed Blade"]);
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// Setting the item the shown tooltip already holds hides it (`0x5320c7`..`0x5320e9`): the shown
/// bit at `+0xd0` and the kept entry at `+0x398`, so a second click on the same item link closes
/// the plate, and the third opens it again.
#[test]
fn the_same_item_link_twice_hides_the_tooltip() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, name) in [(9301, "Iron Ring"), (9302, "Gold Ring")] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: name.into(),
                quality: 1,
                ..Default::default()
            },
        );
    }
    s.run(
        r#"
        local a = CreateFrame("Button", "LinkSlot"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetHyperlink("item:9301:0:0:0")
        assert(tt:IsShown(), "the first click shows")
        tt:SetHyperlink("item:9301:0:0:0")
        assert(not tt:IsShown(), "the same item again hides")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetHyperlink("item:9301:0:0:0")
        assert(tt:IsShown(), "and the next click shows it again")
        -- The entry decides, not the whole link: another roll of the same item hides it too.
        tt:SetHyperlink("item:9301:0:5:0")
        assert(not tt:IsShown(), "the same entry hides")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetHyperlink("item:9301:0:0:0")
        tt:SetHyperlink("item:9302:0:0:0")
        assert(tt:IsShown() and TTTextLeft1:GetText() == "Gold Ring", "another item replaces it")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The trade window's slot 7 carries the spell cast on it (`0x534397`, `0x53459a`, the instance
/// block's `+0x40c`), and the builder adds `ITEM_PROPOSED_ENCHANT` with its name in green and
/// `ITEM_ENCHANT_DISCLAIMER` in pure red after the enchant slots (`0x52cb72`..`0x52cc2c`). Every
/// other slot leaves the id 0.
#[test]
fn the_trade_enchant_slot_shows_the_proposed_enchant() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        ITEM_PROPOSED_ENCHANT   = "[PROPOSED %s]"
        ITEM_ENCHANT_DISCLAIMER = "[DISCLAIMER]"
        TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        "#,
    )
    .unwrap();
    s.set_item_template(
        9400,
        ItemTemplateView {
            name: "Plain Gloves".into(),
            quality: 1,
            ..Default::default()
        },
    );
    let slot = |enchantment: Option<&str>| TradeSlotItem {
        item_id: 9400,
        name: Some("Plain Gloves".into()),
        count: 1,
        quality: Some(1),
        enchantment: enchantment.map(str::to_string),
        ..Default::default()
    };
    let gloves = |guid| ContainerSlot {
        item_id: 9400,
        count: 1,
        quality: Some(1),
        guid,
        link: Some("|cffffffff|Hitem:9400:0:0:0|h[Plain Gloves]|h|r".into()),
        ..Default::default()
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([(1, gloves(0x9401)), (2, gloves(0x9402))]),
        }),
    );
    offer(&mut s, TradeState::default(), &[(1, 1), (2, 7)]);
    // The header's spell, which the app pushes with the offers (`0x5d3fa0`).
    let mut trade = TradeState::default();
    trade.player.slots[6] = Some(slot(Some("Enchant Gloves - Agility")));
    trade.target.slots[6] = Some(slot(Some("Enchant Gloves - Mining")));
    s.set_trade(Some(trade));
    let ours = texts_after(&mut s, "TT:SetTradePlayerItem(7)");
    assert_eq!(
        ours,
        [
            "Plain Gloves",
            "[PROPOSED Enchant Gloves - Agility]",
            "[DISCLAIMER]"
        ]
    );
    let lines = lines_of(&mut s);
    assert_eq!(lines[1].1, [0.0, 1.0, 0.0, 1.0], "the proposal is green");
    assert_eq!(
        lines[2].1,
        [1.0, 0.0, 0.0, 1.0],
        "the disclaimer is pure red"
    );
    let theirs = texts_after(&mut s, "TT:SetTradeTargetItem(7)");
    assert_eq!(
        theirs,
        [
            "Plain Gloves",
            "[PROPOSED Enchant Gloves - Mining]",
            "[DISCLAIMER]"
        ]
    );
    let plain = texts_after(&mut s, "TT:SetTradePlayerItem(1)");
    assert_eq!(plain, ["Plain Gloves"]);
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The partner's slot block carries the item's creator (`+0x410`, from `0xb71680`,
/// `0x53453e`..`0x53454f`), and the builder's no-object arm reads it (`0x52e1d5`); the player's
/// own slot is the item object, whose `ITEM_FIELD_CREATOR` the object arm reads (`0x52e247`). A
/// crafted item shows its maker on both sides, and an unresolved name waits for the cache.
#[test]
fn a_traded_crafted_item_names_its_maker() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")"#)
        .unwrap();
    s.set_item_template(
        9401,
        ItemTemplateView {
            name: "Runecloth Bag".into(),
            quality: 2,
            ..Default::default()
        },
    );
    let slot = |creator: Option<&str>| TradeSlotItem {
        item_id: 9401,
        name: Some("Runecloth Bag".into()),
        count: 1,
        quality: Some(2),
        creator: creator.map(str::to_string),
        creator_guid: 0x77,
        ..Default::default()
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([(
                1,
                ContainerSlot {
                    item_id: 9401,
                    count: 1,
                    quality: Some(2),
                    guid: 0x9401,
                    creator: Some("Tailor".into()),
                    creator_guid: 0x76,
                    ..Default::default()
                },
            )]),
        }),
    );
    let mut trade = TradeState::default();
    trade.target.slots[0] = Some(slot(Some("Weaver")));
    trade.target.slots[1] = Some(slot(None));
    offer(&mut s, trade, &[(1, 1)]);
    let ours = texts_after(&mut s, "TT:SetTradePlayerItem(1)");
    assert_eq!(ours, ["Runecloth Bag", "|cff00ff00[CREATED_BY Tailor]|r"]);
    let theirs = texts_after(&mut s, "TT:SetTradeTargetItem(1)");
    assert_eq!(theirs, ["Runecloth Bag", "|cff00ff00[CREATED_BY Weaver]|r"]);
    let waiting = texts_after(&mut s, "TT:SetTradeTargetItem(2)");
    assert_eq!(
        waiting,
        ["Runecloth Bag"],
        "no line while the name is in flight"
    );
    assert_eq!(
        s.take_item_name_asks(),
        [0x77],
        "the build asks the creator's name"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// A petition's signature count follows its creator line, white, with the count as the plural
/// ordinal (`0x52ba51`..`0x52ba8d`), and none at 0 (`0x52ba57`); an owner still in flight leaves
/// the count in place (`0x52b9d9` skips only the creator).
#[test]
fn a_petitions_signature_count_follows_its_creator() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"PETITION_NUM_SIGNATURES = "[SIGS %d]"
        PETITION_NUM_SIGNATURES_P1 = "[SIGS %d x]""#,
    )
    .unwrap();
    s.set_item_template(
        5863,
        ItemTemplateView {
            name: "Charter".into(),
            quality: 1,
            flags: 0x2000,
            ..Default::default()
        },
    );
    let signed = |slot: u32, owner: Option<&str>, signatures| {
        (
            slot,
            ContainerSlot {
                item_id: 5863,
                count: 1,
                quality: Some(1),
                guid: 0x4000_0000_0000_0010 + u64::from(slot),
                petition: Some(PetitionSlotView {
                    is_charter: false,
                    title: "Pact".into(),
                    owner: owner.map(str::to_string),
                    signatures,
                }),
                ..Default::default()
            },
        )
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([
                signed(1, Some("Bob"), 3),
                signed(2, Some("Bob"), 1),
                signed(3, Some("Bob"), 0),
                signed(4, None, 2),
            ]),
        }),
    );
    let pact = |rest: &[&str]| {
        let mut v = vec!["Charter", "[PETITION_TITLE Pact]"];
        v.extend_from_slice(rest);
        v.push("[ITEM_SIGNABLE]");
        v.into_iter().map(str::to_string).collect::<Vec<_>>()
    };
    let bob = "[PETITION_CREATOR Bob]";
    assert_eq!(
        texts_after(&mut s, "TT:SetBagItem(0, 1)"),
        pact(&[bob, "[SIGS 3 x]"])
    );
    let lines = lines_of(&mut s);
    assert_eq!(lines[3].1, [1.0, 1.0, 1.0, 1.0], "the count is white");
    assert_eq!(
        texts_after(&mut s, "TT:SetBagItem(0, 2)"),
        pact(&[bob, "[SIGS 1]"])
    );
    assert_eq!(texts_after(&mut s, "TT:SetBagItem(0, 3)"), pact(&[bob]));
    assert_eq!(
        texts_after(&mut s, "TT:SetBagItem(0, 4)"),
        pact(&["[SIGS 2 x]"])
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The wrap flag `TT`'s `line` carries.
fn wrapped(s: &UiScript, line: usize) -> bool {
    let model = s.model_ref();
    let h = model.arena.lookup("TT").expect("the tooltip");
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(crate::widget::KindState::Tooltip(t)) => t.wrapped[line - 1],
        _ => panic!("not a tooltip"),
    }
}

/// A charter in the backpack's `slot`, its petition titled `title`.
fn charter(slot: u32, title: &str) -> (u32, ContainerSlot) {
    (
        slot,
        ContainerSlot {
            item_id: 5863,
            count: 1,
            quality: Some(1),
            guid: 0x4000_0000_0000_0020 + u64::from(slot),
            petition: Some(PetitionSlotView {
                is_charter: true,
                title: title.into(),
                owner: Some("Bob".into()),
                signatures: 0,
            }),
            ..Default::default()
        },
    )
}

/// The title line wraps (`0x52b9a4` pushes 1 to the line core) where the creator's does not
/// (`0x52ba24`), and it is formatted and added whatever the title (`0x52b99c`): an empty one
/// still reads its key.
#[test]
fn a_petitions_title_wraps_and_shows_when_empty() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        5863,
        ItemTemplateView {
            name: "Charter".into(),
            quality: 1,
            flags: 0x2000,
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([charter(1, "Knights"), charter(2, "")]),
        }),
    );
    assert_eq!(
        texts_after(&mut s, "TT:SetBagItem(0, 1)"),
        [
            "Charter",
            "[GUILD_CHARTER_TITLE Knights]",
            "[GUILD_CHARTER_CREATOR Bob]",
            "[ITEM_SIGNABLE]"
        ]
    );
    assert!(wrapped(&s, 2), "the title wraps");
    assert!(!wrapped(&s, 3), "the creator does not");
    assert_eq!(
        texts_after(&mut s, "TT:SetBagItem(0, 2)"),
        [
            "Charter",
            "[GUILD_CHARTER_TITLE ]",
            "[GUILD_CHARTER_CREATOR Bob]",
            "[ITEM_SIGNABLE]"
        ]
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// Our trade slot builds from the item object its guid names (`[0xb716d0 + slot*8]`, `0x5342c8`;
/// the builder's p3, `0x5343d5`), so the object's fields reach every line: a letter's text id
/// picks `ITEM_WRITTEN_BY` and the read hint (`0x52e21b`, `0x52e34e`), and a charter asks its
/// petition by that guid and shows its lines (`0x52b92f`).
#[test]
fn our_trade_slot_reads_the_item_object() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, name, flags) in [(8383, "Letter", 0), (5863, "Charter", 0x2000)] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: name.into(),
                quality: 1,
                flags,
                ..Default::default()
            },
        );
    }
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([
                (
                    1,
                    ContainerSlot {
                        item_id: 8383,
                        count: 1,
                        quality: Some(1),
                        guid: 0x8383,
                        readable: true,
                        creator: Some("Ann".into()),
                        creator_guid: 0x31,
                        ..Default::default()
                    },
                ),
                charter(2, "Knights"),
            ]),
        }),
    );
    offer(&mut s, TradeState::default(), &[(1, 1), (2, 2)]);
    assert_eq!(
        texts_after(&mut s, "TT:SetTradePlayerItem(1)"),
        ["Letter", "[WRITTEN_BY Ann]", "[ITEM_READABLE]"]
    );
    assert_eq!(
        texts_after(&mut s, "TT:SetTradePlayerItem(2)"),
        [
            "Charter",
            "[GUILD_CHARTER_TITLE Knights]",
            "[GUILD_CHARTER_CREATOR Bob]",
            "[ITEM_SIGNABLE]"
        ]
    );
    assert!(
        texts_after(&mut s, "TT:SetTradePlayerItem(3)").is_empty(),
        "an empty slot builds nothing"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}
