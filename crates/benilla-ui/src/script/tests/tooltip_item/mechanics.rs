//! Render mechanics around the line order: sell price, wrapped lines, the unit health bar,
//! durability, enchant lines and random-property rolls.

use std::collections::HashMap;

use super::script;
use crate::script::*;

/// `SetBagItem` at an open merchant fires `OnTooltipAddMoney` with SellPrice × stack, and price 0
/// prints `ITEM_UNSELLABLE`; with no merchant, neither. A template in flight renders the link name.
#[test]
fn bag_item_money_law_and_fallback() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 4,
            quality: Some(1),
            item_id: 2318,
            link: Some("|cffffffff|Hitem:2318|h[Light Leather]|h|r".into()),
            ..Default::default()
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            count: 1,
            quality: Some(1),
            item_id: 9999,
            link: Some("|cffffffff|Hitem:9999|h[Shadowforge Key]|h|r".into()),
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.set_item_template(
        2318,
        ItemTemplateView {
            name: "Light Leather".into(),
            quality: 1,
            sell_price: 13,
            ..Default::default()
        },
    );
    s.run(
        r#"
        money_fired = nil
        local a = CreateFrame("Button", "Slot3"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetScript("OnTooltipAddMoney", function() local self = this money_fired = arg1 end)
        -- No merchant: no money handler fires.
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
        assert(money_fired == nil, "no merchant, no money")
    "#,
    )
    .unwrap();
    s.set_merchant(Some(MerchantState::default()));
    s.run(
        r#"
        TT:SetOwner(Slot3, "ANCHOR_RIGHT")
        TT:SetBagItem(0, 1)
        assert(money_fired == 52, "SellPrice 13 × stack 4, got " .. tostring(money_fired))
        -- The unresolved key: the link-name fallback line + nothing else.
        TT:SetOwner(Slot3, "ANCHOR_RIGHT")
        TT:SetBagItem(0, 2)
        assert(TT:NumLines() == 1, "fallback one-liner")
        assert(TTTextLeft1:GetText() == "Shadowforge Key", "fallback carries the link name")
    "#,
    )
    .unwrap();
    assert_eq!(s.take_item_stat_asks(), vec![9999], "the miss asked");
    s.set_item_template(
        9999,
        ItemTemplateView {
            name: "Shadowforge Key".into(),
            quality: 1,
            sell_price: 0,
            ..Default::default()
        },
    );
    s.run(
        r#"
        money_fired = nil
        TT:SetOwner(Slot3, "ANCHOR_RIGHT")
        TT:SetBagItem(0, 2)
        assert(money_fired == nil, "unsellable fires no money")
        local last = getglobal("TTTextLeft" .. TT:NumLines())
        assert(last:GetText() == "[ITEM_UNSELLABLE]", "ITEM_UNSELLABLE line")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A wrapped description is sized in the builder's show (`0x52e15c` → `0x530640`): split at 230.4
/// into its widest piece, and the size holds while the hover clears and rebuilds the content every
/// frame, measuring nothing past the first.
#[test]
fn a_wrapped_description_sizes_in_the_show_and_survives_the_reenter_loop() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    s.set_text_measurer(Box::new(crate::script::tests::tooltip_size::Mono(
        calls.clone(),
    )));
    s.set_item_template(
        7,
        ItemTemplateView {
            name: "Storybook".into(),
            quality: 1,
            // ITEM_CONJURED: one row of the body font.
            flags: 0x2,
            description:
                "An exceedingly long tale of adventure and woe that would never fit on one line."
                    .into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot4"); a:SetPoint("TOPLEFT", 10, -10); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, 7)
    "#,
    )
    .unwrap();
    // The stand-in engine's 5 per byte: the quoted description, 405 wide, splits at 230.4 into
    // rows of 230 and 170, the first piece 235 with the space its wrap skipped (`0x530884`); name
    // 14 + gap 2 + conjured 12 + gap 2 + two rows of 12 + 2·10.24.
    let size =
        |s: &UiScript| -> (f64, f64) { s.eval("return TT:GetWidth(), TT:GetHeight()").unwrap() };
    let want = (
        f64::from(2.0 * 10.24f32 + 0.0 + 235.0),
        f64::from(2.0 * 10.24f32 + 54.0),
    );
    let near = |(w, h): (f64, f64)| (w - want.0).abs() < 1e-4 && (h - want.1).abs() < 1e-4;
    assert!(near(size(&s)), "sized in the show: {:?}", size(&s));
    s.tick(0.016);
    s.resolve();
    let measured = calls.get();
    for _ in 0..3 {
        s.run(
            r#"
            TT:SetOwner(Slot4, "ANCHOR_RIGHT")
            SetItemByIdForTest(TT, 7)
        "#,
        )
        .unwrap();
        s.tick(0.016);
        s.resolve();
        assert!(near(size(&s)), "re-enter holds the size: {:?}", size(&s));
    }
    assert_eq!(calls.get(), measured, "the re-enter loop re-measured");
    assert!(s.take_errors().is_empty());
}

/// The merchant's price for an item object (`0x52e418`..`0x52e473`): the sell price scaled by the
/// block-0 charges left over the template's below -1, truncated (`idiv`), less the item's own
/// repair cost (`0x5da330`, never the discounted one) down to 1, then times the stack.
#[test]
fn a_bag_items_price_scales_by_its_charges_and_loses_its_repair_cost() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let held = |guid: u64, count, charges_0| ContainerSlot {
        count,
        quality: Some(1),
        item_id: 4376,
        guid,
        spell_charges: [charges_0, 0, 0, 0, 0],
        ..Default::default()
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([
                (1, held(0x4000_0000_0000_0001, 1, -3)),
                (2, held(0x4000_0000_0000_0002, 2, -5)),
                (3, held(0x4000_0000_0000_0003, 3, -2)),
            ]),
        }),
    );
    s.set_item_template(
        4376,
        ItemTemplateView {
            name: "Flame Deflector".into(),
            quality: 1,
            sell_price: 100,
            spell_charges_0: -5,
            ..Default::default()
        },
    );
    s.set_repair_costs(RepairCosts {
        bags: HashMap::from([((0, 1), 27), ((0, 2), 18)]),
        own: HashMap::from([(0x4000_0000_0000_0001, 30), (0x4000_0000_0000_0002, 20)]),
        ..Default::default()
    });
    s.set_merchant(Some(MerchantState::default()));
    let money = |s: &mut UiScript, slot: u32| -> i64 {
        s.eval(&format!(
            r#"
            money_fired = nil
            local a = PriceSlot or CreateFrame("Button", "PriceSlot")
            a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
            local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
            tt:SetScript("OnTooltipAddMoney", function() money_fired = arg1 end)
            tt:SetOwner(a, "ANCHOR_RIGHT")
            tt:SetBagItem(0, {slot})
            return money_fired
            "#
        ))
        .unwrap()
    };
    // 3 of 5 charges: 100 × -3 / -5 = 60, less 30.
    assert_eq!(money(&mut s, 1), 30);
    // Full charges, less 20, times the stack of 2.
    assert_eq!(money(&mut s, 2), 160);
    // 100 × -2 / -5 = 40 with no cost, times 3.
    assert_eq!(money(&mut s, 3), 120);
    s.set_repair_costs(RepairCosts {
        own: HashMap::from([(0x4000_0000_0000_0003, 40)]),
        ..Default::default()
    });
    assert_eq!(
        money(&mut s, 3),
        3,
        "a cost reaching the price leaves 1 each"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The merchant's price needs an item object (a nonzero p3, `0x52e376`) held in a bag, or by the
/// player at slot index 23 or more (`0x52e3c6`..`0x52e40f`): `SetInventoryItem` prices a vault slot
/// but no worn item (`0x5332a4`), and `SetAction` prices the copy it finds in a bag but not a worn
/// one (`0x532586`).
#[test]
fn the_merchant_prices_an_object_held_past_the_worn_slots() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, name, sell_price) in [(4380, "Ring", 50), (4381, "Gem", 7), (4382, "Shard", 9)] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: name.into(),
                quality: 1,
                sell_price,
                ..Default::default()
            },
        );
    }
    let mut inv: InventorySlots = Default::default();
    inv[11] = Some(InvSlotView {
        item_id: 4380,
        guid: 0x480,
        count: 1,
        name: Some("Ring".into()),
        quality: 1,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    let held = |item_id, guid, count| ContainerSlot {
        item_id,
        guid,
        count,
        quality: Some(1),
        ..Default::default()
    };
    s.set_container(
        -1,
        Some(ContainerState {
            name: None,
            num_slots: 24,
            slots: HashMap::from([(1, held(4381, 0x481, 3))]),
        }),
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([(1, held(4382, 0x482, 2))]),
        }),
    );
    for (id, item) in [(1, 4380), (2, 4382)] {
        s.set_action(
            id,
            Some(ActionSlot {
                kind: 0x80,
                action: item,
                ..Default::default()
            }),
        );
    }
    s.set_merchant(Some(MerchantState::default()));
    let money = |s: &mut UiScript, call: &str| -> Option<i64> {
        s.eval(&format!(
            r#"
            money_fired = nil
            local a = PriceSlot or CreateFrame("Button", "PriceSlot")
            a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
            local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
            tt:SetScript("OnTooltipAddMoney", function() money_fired = arg1 end)
            tt:SetOwner(a, "ANCHOR_RIGHT")
            {call}
            return money_fired
            "#
        ))
        .unwrap()
    };
    assert_eq!(money(&mut s, r#"TT:SetInventoryItem("player", 11)"#), None);
    assert_eq!(
        money(&mut s, r#"TT:SetInventoryItem("player", 40)"#),
        Some(21),
        "the vault's first slot, index 39"
    );
    assert_eq!(money(&mut s, "TT:SetAction(1)"), None, "found worn");
    assert_eq!(money(&mut s, "TT:SetAction(2)"), Some(18), "found in a bag");
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The mouseover health bar is unit content: an item render on the same tooltip hides it.
#[test]
fn item_render_hides_the_unit_health_bar() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit(
        "mouseover",
        Some(UnitState {
            guid: 0xF130_0000_0000_0100,
            exists: true,
            name: Some("Timber Wolf".into()),
            health: 30,
            max_health: 50,
            level: 10,
            reaction: 2,
            ..Default::default()
        }),
    );
    s.set_item_template(
        9,
        ItemTemplateView {
            name: "Plain Rock".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot6"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "GameTooltip", nil, "TestTooltipTemplate"); tt:Hide()
        local bar = CreateFrame("StatusBar", "GameTooltipStatusBar", tt)
        bar:SetPoint("TOPLEFT", tt, "BOTTOMLEFT", 2, -1); bar:SetWidth(100); bar:SetHeight(8)
        -- The stock default anchor owns the plate; an unowned one's show hides it.
        tt:SetScript("OnTooltipSetDefaultAnchor", function() GameTooltip:SetOwner(Slot6, "ANCHOR_NONE") end)
    "#,
    )
    .unwrap();
    assert!(s.world_tooltip_unit("mouseover"), "the unit hover shows");
    s.run(r#"assert(GameTooltipStatusBar:IsShown(), "unit hover shows the bar")"#)
        .unwrap();
    s.run(
        r#"
        GameTooltip:SetOwner(Slot6, "ANCHOR_RIGHT")
        SetItemByIdForTest(GameTooltip, 9)
        assert(not GameTooltipStatusBar:IsShown(), "item content hides the unit bar")
        assert(GameTooltip:IsShown(), "the item tooltip itself shows")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A bag hover shows the instance's live `ITEM_FIELD_DURABILITY` (a death costs 10%, the spirit
/// healer 25%); a template hover of the same item has no object and no block, so no line
/// (`0x52cca8`).
#[test]
fn real_instance_hover_renders_live_durability() {
    let mut s = script();
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        2,
        ContainerSlot {
            count: 1,
            quality: Some(2),
            item_id: 2264,
            link: Some("|cff1eff00|Hitem:2264|h[Mantle of Doan]|h|r".into()),
            durability: Some((30, 40)), // the spirit healer's 25% off a 40-max piece
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.set_item_template(
        2264,
        ItemTemplateView {
            name: "Mantle of Doan".into(),
            quality: 2,
            max_durability: 40,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 2)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 30/40]", "live pair on a bag hover, got " .. tostring(found))
        -- The template hover of the SAME item draws none.
        SetItemByIdForTest(tt, 2264)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == nil, "a template hover has no durability line, got " .. tostring(found))
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The durability line (`0x52cc5b`..`0x52cdc3`) needs an item object with a max, or an instance
/// block with one: a template source and a block that stores none (a link and a loot row, whose
/// clear zeroes `+0x434`, `0x5326e0`) show no line, and the trade partner's block shows the wire's
/// pair (`0x5345b4`, `0x5345c0`), the current clamped to the max (`0x52cd7c`) and red at 0. A
/// wrapped gift shows none (`0x52cc60`).
#[test]
fn only_an_object_or_a_block_with_a_max_shows_durability() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        2264,
        ItemTemplateView {
            name: "Mantle of Doan".into(),
            quality: 2,
            max_durability: 40,
            ..Default::default()
        },
    );
    let durability = |s: &mut UiScript, call: &str| -> Vec<(String, [f32; 4])> {
        s.run(&format!(
            r#"
            local a = DurSlot or CreateFrame("Button", "DurSlot")
            a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
            local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
            tt:SetOwner(a, "ANCHOR_RIGHT")
            {call}
            "#
        ))
        .unwrap();
        super::lines_of(s)
            .into_iter()
            .filter(|(t, _)| t.starts_with("[DURABILITY"))
            .collect()
    };
    let texts = |lines: Vec<(String, [f32; 4])>| -> Vec<String> {
        lines.into_iter().map(|(t, _)| t).collect()
    };
    assert!(durability(&mut s, "SetItemByIdForTest(TT, 2264)").is_empty());
    assert!(durability(&mut s, r#"TT:SetHyperlink("item:2264:0:0:0")"#).is_empty());
    s.set_loot(Some(LootState {
        rows: vec![Some(LootRow {
            name: Some("Mantle of Doan".into()),
            quantity: 1,
            quality: Some(2),
            item_id: 2264,
            ..Default::default()
        })],
        ..Default::default()
    }));
    assert!(durability(&mut s, "TT:SetLootItem(1)").is_empty());
    let mantle = |durability| TradeSlotItem {
        item_id: 2264,
        name: Some("Mantle of Doan".into()),
        count: 1,
        quality: Some(2),
        durability,
        ..Default::default()
    };
    let mut trade = TradeState::default();
    trade.target.slots[0] = Some(mantle((25, 40)));
    trade.target.slots[1] = Some(mantle((0, 0)));
    trade.target.slots[2] = Some(mantle((50, 40)));
    trade.target.slots[3] = Some(mantle((0, 40)));
    s.set_trade(Some(trade));
    assert_eq!(
        texts(durability(&mut s, "TT:SetTradeTargetItem(1)")),
        ["[DURABILITY 25/40]"]
    );
    assert!(durability(&mut s, "TT:SetTradeTargetItem(2)").is_empty());
    assert_eq!(
        texts(durability(&mut s, "TT:SetTradeTargetItem(3)")),
        ["[DURABILITY 40/40]"]
    );
    assert_eq!(
        durability(&mut s, "TT:SetTradeTargetItem(4)"),
        [(
            "[DURABILITY 0/40]".to_string(),
            [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0]
        )]
    );
    let held = |flags| ContainerSlot {
        count: 1,
        quality: Some(2),
        item_id: 2264,
        durability: Some((30, 40)),
        flags,
        ..Default::default()
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([(1, held(0)), (2, held(0x8))]),
        }),
    );
    assert_eq!(
        texts(durability(&mut s, "TT:SetBagItem(0, 1)")),
        ["[DURABILITY 30/40]"]
    );
    assert!(
        durability(&mut s, "TT:SetBagItem(0, 2)").is_empty(),
        "a wrapped gift"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// Broken gear shows its true `0 / max`: a create block omits a zero `DURABILITY`, which must not
/// read as the template's max.
#[test]
fn broken_instance_hover_renders_zero_durability() {
    let mut s = script();
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(1),
            item_id: 2264,
            durability: Some((0, 40)),
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.set_item_template(
        2264,
        ItemTemplateView {
            name: "Mantle of Doan".into(),
            quality: 2,
            max_durability: 40,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 0/40]", "broken gear shows its true 0, got " .. tostring(found))
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // Red only when broken (`0x854bc4`, colour `0xc0d390`).
    let lines = super::lines_of(&mut s);
    let dur = lines
        .iter()
        .find(|(t, _)| t.starts_with("[DURABILITY"))
        .expect("the durability line renders");
    assert_eq!(
        dur.1,
        [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0],
        "broken (0) paints red"
    );
}

/// An instance's enchant prints from its resolved text, in green, between the resistances and
/// durability; a template hover of the same item has no instance and so no enchant line.
#[test]
fn an_enchanted_instance_renders_its_enchant_line_before_durability() {
    let mut s = script();
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(4),
            item_id: 22816,
            durability: Some((105, 105)),
            // `ITEM_FIELD_ENCHANTMENT` slot 0 (permanent) → enchant 2564 → its name string.
            enchants: vec![EnchantView {
                slot: 0,
                name: "Agility +15".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.set_item_template(
        22816,
        ItemTemplateView {
            name: "Hatchet of Sundered Bone".into(),
            quality: 4,
            class: 2,
            resistances: [0, 0, 7, 0, 0, 0],
            max_durability: 105,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let at = |needle: &str| {
        texts
            .iter()
            .position(|t| t.starts_with(needle))
            .unwrap_or_else(|| panic!("no {needle} line in {texts:?}"))
    };
    assert!(
        at("[RESIST_SINGLE +7 [SCHOOL3]]") < at("Agility +15")
            && at("Agility +15") < at("[DURABILITY"),
        "the enchant line sits between resistances and durability: {texts:?}"
    );
    assert_eq!(
        lines[at("Agility +15")].1,
        [0.0, 1.0, 0.0, 1.0],
        "the enchant line is green"
    );

    s.run(r#"SetItemByIdForTest(TT, 22816)"#).unwrap();
    let lines = super::lines_of(&mut s);
    assert!(
        !lines.iter().any(|(t, _)| t.starts_with("Agility")),
        "a template hover carries no instance and so no enchant line: {lines:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Enchant colour goes by slot (`0x52ca29`): slots 0 and 1 are green for a positive id and pure
/// red (`0xc0d398`, `ffff0000`, not the requirement `ffff2020`) for a negative one; 2..6 are white.
#[test]
fn enchant_line_colour_is_per_slot_and_sign() {
    let mut s = script();
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(3),
            item_id: 7777,
            enchants: vec![
                EnchantView {
                    slot: 0,
                    name: "Crusader".into(),
                    ..Default::default()
                },
                EnchantView {
                    slot: 1,
                    name: "Cursed".into(),
                    negative: true,
                    ..Default::default()
                },
                EnchantView {
                    slot: 3,
                    name: "Stamina +7".into(),
                    ..Default::default()
                },
                EnchantView {
                    slot: 4,
                    name: "Spirit +3".into(),
                    negative: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.set_item_template(
        7777,
        ItemTemplateView {
            name: "Test Blade".into(),
            quality: 3,
            class: 2,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let color_of = |needle: &str| {
        lines
            .iter()
            .find(|(t, _)| t.starts_with(needle))
            .unwrap_or_else(|| panic!("no {needle} line in {lines:?}"))
            .1
    };
    assert_eq!(color_of("Crusader"), [0.0, 1.0, 0.0, 1.0], "slot 0, id > 0");
    assert_eq!(
        color_of("Cursed"),
        [1.0, 0.0, 0.0, 1.0],
        "slot 1, id < 0 → the pure red 0xc0d398, NOT the requirement red"
    );
    assert_eq!(
        color_of("Stamina +7"),
        [1.0, 1.0, 1.0, 1.0],
        "a suffix slot is white"
    );
    assert_eq!(
        color_of("Spirit +3"),
        [1.0, 1.0, 1.0, 1.0],
        "…and stays white even with a negative id"
    );
}

/// A temporary enchant prints one line, its countdown template (`0x52fa50`) around the name, then
/// the charges joined by `" (%s)"`; with no `SMSG_ITEM_ENCHANT_TIME_UPDATE` it is the bare name.
#[test]
fn temporary_enchant_line_carries_its_countdown_and_charges() {
    let mut s = script();
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(1),
            item_id: 7777,
            enchants: vec![
                EnchantView {
                    slot: 1,
                    name: "Rockbiter Weapon".into(),
                    remaining_ms: Some(275_000), // 4 min 35 s → the MIN arm, ceiled to 5
                    charges: 5,
                    ..Default::default()
                },
                EnchantView {
                    slot: 0,
                    name: "Crusader".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.set_item_template(
        7777,
        ItemTemplateView {
            name: "Test Blade".into(),
            quality: 1,
            class: 2,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        texts.contains(&"[ENCH Rockbiter Weapon 5 m] ([CHARGES 5 x])"),
        "one line: name, countdown, charges — got {texts:?}"
    );
    assert!(
        texts.contains(&"Crusader"),
        "a slot with no packet keeps the bare name — got {texts:?}"
    );
}

/// A loot hover shows the roll, never the placeholder: `SetLootItem` (`0x533470`) builds an
/// instance block (p6 = 1) with the roll at `+0x424`, zeroed enchant slots and no object, so the
/// builder copies the suffix row into slots 2..6 (`0x52b7e0`) and never reaches
/// `ITEM_RANDOM_ENCHANT` (`0x52c991`). The name is the suffixed one `GetLootSlotInfo` returns
/// (`0x5d8b00`).
#[test]
fn a_looted_roll_shows_its_lines_and_never_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            // The template can roll, so the placeholder's absence below is the rule at work.
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_loot(Some(LootState {
        source_unit: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            name: Some("Bloodrazor of the Monkey".into()),
            texture: None,
            quantity: 1,
            quality: Some(2),
            is_coin: false,
            item_id: 8888,
            link: Some("|cff1eff00|Hitem:8888:0:584:0|h[Bloodrazor of the Monkey]|h|r".into()),
            // The row carries the roll id; the engine resolves it, as the reference reads `+0x424`.
            random_property_id: 584,
        })],
        fishing: false,
    }));
    s.set_random_properties(
        [(
            584,
            RandomPropertyView {
                suffix: "of the Monkey".into(),
                enchants: vec![
                    EnchantView {
                        slot: 2,
                        name: "Agility +7".into(),
                        ..Default::default()
                    },
                    EnchantView {
                        slot: 3,
                        name: "Stamina +7".into(),
                        ..Default::default()
                    },
                ],
            },
        )]
        .into_iter()
        .collect(),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetLootItem(1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        !texts.contains(&"[ITEM_RANDOM_ENCHANT]"),
        "a block source never reaches the placeholder arm — got {texts:?}"
    );
    assert_eq!(
        texts.first(),
        Some(&"Bloodrazor of the Monkey"),
        "the plate takes the roll's suffixed name — got {texts:?}"
    );
    for line in ["Agility +7", "Stamina +7"] {
        let l = lines
            .iter()
            .find(|(t, _)| t == line)
            .unwrap_or_else(|| panic!("the roll's slot line {line} — got {texts:?}"));
        assert_eq!(l.1, [1.0, 1.0, 1.0, 1.0], "{line} is white");
    }
}

/// A loot row with no roll shows neither enchant lines nor the placeholder, though the template can
/// roll: the fork tests the block's presence, not its contents (`0x52c9a3`).
#[test]
fn a_looted_item_with_no_roll_shows_neither_line_nor_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_loot(Some(LootState {
        source_unit: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            name: Some("Bloodrazor".into()),
            quantity: 1,
            quality: Some(2),
            item_id: 8888,
            ..Default::default()
        })],
        fishing: false,
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetLootItem(1)
    "#,
    )
    .unwrap();
    let texts: Vec<String> = super::lines_of(&mut s)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert!(
        !texts.iter().any(|t| t == "[ITEM_RANDOM_ENCHANT]"),
        "no placeholder on a block source — got {texts:?}"
    );
}

/// A chat link is a block source too (`SetHyperlink` `0x532181`, p6 = 1): its `|Hitem:` token 2 is
/// the roll, `0x52b7e0` expands it into slots 2..6, and `0x52c9a3` never reaches the placeholder.
#[test]
fn a_linked_roll_shows_its_lines_and_never_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_random_properties(
        [(
            584,
            RandomPropertyView {
                suffix: "of the Monkey".into(),
                enchants: vec![EnchantView {
                    slot: 2,
                    name: "Agility +7".into(),
                    ..Default::default()
                }],
            },
        )]
        .into_iter()
        .collect(),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetHyperlink("|cff1eff00|Hitem:8888:0:584:0|h[Bloodrazor of the Monkey]|h|r")
    "#,
    )
    .unwrap();
    let texts: Vec<String> = super::lines_of(&mut s)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert!(
        !texts.iter().any(|t| t == "[ITEM_RANDOM_ENCHANT]"),
        "a link never reaches the placeholder arm — got {texts:?}"
    );
    assert_eq!(
        texts.first().map(String::as_str),
        Some("Bloodrazor of the Monkey"),
        "the plate takes the link's own suffixed name — got {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "Agility +7"),
        "the roll's slot line — got {texts:?}"
    );
}

/// `ITEM_RANDOM_ENCHANT` (`0x52cc33`) prints, green, only for a random-property item with no
/// instance to read a roll from; an instance's known roll prints its slot lines instead.
#[test]
fn random_property_template_hover_shows_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            random_property: 42,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, 8888)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let placeholder = lines
        .iter()
        .find(|(t, _)| t == "[ITEM_RANDOM_ENCHANT]")
        .expect("the template hover shows the placeholder");
    assert_eq!(placeholder.1, [0.0, 1.0, 0.0, 1.0], "green");

    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(2),
            item_id: 8888,
            enchants: vec![EnchantView {
                slot: 2,
                name: "Stamina +7".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }),
    );
    s.run(r#"TT:SetBagItem(0, 1)"#).unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        !texts.contains(&"[ITEM_RANDOM_ENCHANT]") && texts.contains(&"Stamina +7"),
        "an instance's known roll replaces the placeholder — got {texts:?}"
    );
}

/// An item whose template is not cached ends the builder in its own hide (`0x52b72e`–`0x52b73f`),
/// which un-owns and clears: the clear zeroes the remembered item id (`0x5300dd`), so the cache
/// callback (`0x52e530`) finds nothing to rebuild (`0x52e55e`), and the stock hover loop's
/// `IsOwned` refresh (`ContainerFrame.lua:657`) stops. The plate stays down until the next hover.
#[test]
fn an_uncached_item_hides_and_un_owns_until_the_next_hover() {
    let mut s = script();
    s.run(
        r#"
        shows = 0
        UncSlot = CreateFrame("Button", "UncSlot")
        UncTip = CreateFrame("GameTooltip", "UncTip", nil, "TestTooltipTemplate")
        UncTip:SetScript("OnShow", function() shows = shows + 1 end)
        UncTip:SetOwner(UncSlot, "ANCHOR_RIGHT")
        SetItemByIdForTest(UncTip, 4242)
        assert(UncTip:NumLines() == 0, "no line for a template in flight")
        assert(not UncTip:IsShown(), "the builder's own hide")
        assert(not UncTip:IsOwned(UncSlot), "and its un-own: the hover loop stops")
        assert(shows == 0)
        "#,
    )
    .unwrap();
    s.set_item_template(
        4242,
        ItemTemplateView {
            name: "Copper Bar".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.tick(0.1);
    s.run(
        r#"
        assert(not UncTip:IsShown(), "the template's arrival rebuilds nothing")
        UncTip:SetOwner(UncSlot, "ANCHOR_RIGHT")
        SetItemByIdForTest(UncTip, 4242)
        assert(UncTip:IsShown() and UncTipTextLeft1:GetText() == "Copper Bar", "the next hover")
        "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}
