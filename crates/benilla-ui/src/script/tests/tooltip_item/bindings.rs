//! The `Set*` entry points: the vendor compare, `SetInventoryItem` outside it, `SetHyperlink`'s
//! item filter, and the tradeskill and merchant-compare row indexing.

use std::collections::HashMap;

use super::script;
use crate::script::*;

/// The compare as 1.12.1 drives it, from the vendor row's `SetMerchantCompareItem`
/// (`MerchantFrame.xml:63-80`): the worn item's ordinary tooltip under a gray
/// `CURRENTLY_EQUIPPED` header (p5). Both callers pass p4 zero, so the name keeps its quality
/// colour and nothing is cut at `0x52e14c`. A bag hover seats no plate, shift or not:
/// `SHOW_COMPARE_TOOLTIP` has no fire site in the reference.
#[test]
fn merchant_compare_renders_the_compare_shape_and_no_hover_seats_a_plate() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut inv: InventorySlots = Default::default();
    inv[11] = Some(InvSlotView {
        durability: None,
        item_id: 7000,
        name: Some("Old Loop".into()),
        // Epic: at quality 1 a white name and a quality name are the same pixel.
        quality: 4,
        ..Default::default()
    });
    inv[12] = Some(InvSlotView {
        durability: None,
        item_id: 7001,
        name: Some("Older Loop".into()),
        quality: 1,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_item_template(
        7000,
        ItemTemplateView {
            name: "Old Loop".into(),
            quality: 4,
            inventory_type: 11,
            description: "Round.".into(),
            ..Default::default()
        },
    );
    s.set_item_template(
        7001,
        ItemTemplateView {
            name: "Older Loop".into(),
            quality: 1,
            inventory_type: 11,
            ..Default::default()
        },
    );
    s.set_item_template(
        7002,
        ItemTemplateView {
            name: "New Loop".into(),
            quality: 2,
            inventory_type: 11,
            ..Default::default()
        },
    );
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: None,
            count: 1,
            quality: Some(2),
            item_id: 7002,
            link: Some("|cff1eff00|Hitem:7002:0:0:0|h[New Loop]|h|r".into()),
            locked: false,
            equip_slots: vec![11, 12],
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 4,
            slots,
        }),
    );
    // The plates are ordinary GameTooltips the FrameXML owns; the reference never names them.
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("LEFT", 0, 0); a:SetWidth(10); a:SetHeight(10)
        -- CreateFrame'd frames start SHOWN; the shipped XML instances are hidden="true".
        CreateFrame("GameTooltip", "GameTooltip"):Hide()
        CreateFrame("GameTooltip", "ShoppingTooltip1"):Hide()
        CreateFrame("GameTooltip", "ShoppingTooltip2"):Hide()
        -- A ref-shaped SHOW_COMPARE_TOOLTIP listener, kept as the NEGATIVE control: 5875 never
        -- signals event 377 (zero fire sites in the whole image), so nothing
        -- benilla does may reach this handler.
        compare_calls = {}
        local watcher = CreateFrame("Frame", "CompareWatcher")
        watcher:RegisterEvent("SHOW_COMPARE_TOOLTIP")
        watcher:SetScript("OnEvent", function()
            table.insert(compare_calls, arg1 .. ":" .. arg2)
        end)
    "#,
    )
    .unwrap();

    // ── A hover is not a compare, with shift or without ────────────────────────────────────
    s.run(
        r#"
        GameTooltip:SetOwner(Slot, "ANCHOR_RIGHT")
        GameTooltip:SetBagItem(0, 1)
        assert(not ShoppingTooltip1:IsShown(), "a bag hover seats no plate")
    "#,
    )
    .unwrap();
    s.set_modifiers(true, false, false);
    s.run(
        r#"
        assert(table.getn(compare_calls) == 0, "the dead event never fires")
        assert(not ShoppingTooltip1:IsShown() and not ShoppingTooltip2:IsShown(),
               "shift over a hover seats no plate either — 1.12 has no hover compare")
        assert(GameTooltip:IsShown(), "and the hover itself is untouched")
    "#,
    )
    .unwrap();
    s.set_modifiers(false, false, false);

    // ── The vendor row, which is where the compare actually lives ──────────────────────────
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("New Loop".into()),
            item_id: 7002,
            ..Default::default()
        }],
        ..Default::default()
    }));
    s.run(
        r#"
        -- MerchantFrame.xml:67-78, in miniature: fill as the predicate, seat, fill again, show.
        assert(ShoppingTooltip1:SetMerchantCompareItem(1, 1), "the worn ring is a candidate")
        ShoppingTooltip1:SetOwner(GameTooltip, "ANCHOR_NONE")
        ShoppingTooltip1:ClearAllPoints()
        ShoppingTooltip1:SetPoint("TOPLEFT", "GameTooltip", "TOPRIGHT", 0, -10)
        ShoppingTooltip1:SetMerchantCompareItem(1, 1)
        ShoppingTooltip1:Show()
        assert(ShoppingTooltip1:IsShown())
        assert(ShoppingTooltip1TextLeft1:GetText() == "[CURRENTLY_EQUIPPED]")
        assert(ShoppingTooltip1TextLeft2:GetText() == "Old Loop")
        -- p4 = 0: there is NO compact cut here. The description prints, exactly as it does on a
        -- plain SetInventoryItem of the same ring.
        local described = nil
        for i = 1, ShoppingTooltip1:NumLines() do
            if getglobal("ShoppingTooltip1TextLeft" .. i):GetText() == "\"Round.\"" then
                described = i
            end
        end
        assert(described, "the compare tooltip is the FULL tooltip — the description prints")
        assert(table.getn(compare_calls) == 0, "and still no dead event")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let color_of = |txt: &str| {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == txt => *color,
                _ => None,
            })
            .unwrap_or([0.0; 4])
    };
    let gray = color_of("[CURRENTLY_EQUIPPED]");
    assert!(
        (gray[0] - 128.0 / 255.0).abs() < 0.01 && (gray[1] - 128.0 / 255.0).abs() < 0.01,
        "CURRENTLY_EQUIPPED is gray, got {gray:?}"
    );
    let name = color_of("Old Loop");
    assert!(
        (name[0] - 0.639).abs() < 0.01
            && (name[1] - 0.208).abs() < 0.01
            && (name[2] - 0.933).abs() < 0.01,
        "the compare name wears the item's own quality color (epic purple), got {name:?}"
    );
    assert!(s.take_errors().is_empty());
}

/// `SetInventoryItem`'s third argument, `nameOnly`, is the builder's p4 and the one way to the
/// compact render: of `0x52b650`'s 31 call sites, 27 pass zero, one forwards and three are this
/// binding's, and no stock caller passes it. The render is trimmed, not bare: the bind and lock
/// lines, the stat body and everything past the cooldown line go; the white name, the slot and
/// type cell, durability, the requirements and the spell triggers stay.
#[test]
fn set_inventory_item_name_only_is_the_trimmed_build() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut inv: InventorySlots = Default::default();
    inv[16] = Some(InvSlotView {
        durability: Some((50, 90)),
        item_id: 8100,
        name: Some("Sealed Blade".into()),
        quality: 4,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_item_template(
        8100,
        ItemTemplateView {
            name: "Sealed Blade".into(),
            quality: 4,
            class: 2,
            subclass: 7,
            inventory_type: 13,
            // A line from each region p4 treats differently.
            flags: 0x2,
            bonding: 1,
            max_count: 1,
            lock_id: 7,
            armor: 100,
            max_durability: 90,
            required_level: 40,
            spell_triggers: vec![(1, 100, "Zap".into())],
            description: "Sealed.".into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot13"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")

        function has(needle)
            for i = 1, TT:NumLines() do
                local t = getglobal("TTTextLeft" .. i):GetText()
                if t and string.find(t, needle, 1, true) then return true end
            end
            return false
        end

        -- ── no third argument: the full build ──────────────────────────────────────────────
        assert(tt:SetInventoryItem("player", 16) == 1)
        assert(has("[ITEM_CONJURED]") and has("[ITEM_BIND_ON_PICKUP]")
               and has("[ITEM_UNIQUE]") and has("[LOCKED]"), "full: the bind/lock region")
        assert(has("[ARMOR 100]"), "full: the stat body")
        assert(has("[INVTYPE_WEAPON]") and has("[DURABILITY 50/90]")
               and has("[MIN_LEVEL 40]") and has("Zap"), "full: the kept lines")
        assert(has("\"Sealed.\""), "full: the description")
        local r, g, b = TTTextLeft1:GetTextColor()
        assert(math.abs(r - 0.639) < 0.01 and math.abs(g - 0.208) < 0.01
               and math.abs(b - 0.933) < 0.01, "full: the name is epic purple")

        -- ── nameOnly = 1: trimmed, not bare ───────────────────────────────────────────────
        assert(tt:SetInventoryItem("player", 16, 1) == 1, "nameOnly still answers 1")
        assert(TTTextLeft1:GetText() == "Sealed Blade")
        local r2, g2, b2 = TTTextLeft1:GetTextColor()
        assert(r2 == 1 and g2 == 1 and b2 == 1, "nameOnly: the NAME goes white (0x52b8b3)")
        assert(not has("[ITEM_CONJURED]") and not has("[ITEM_BIND_ON_PICKUP]")
               and not has("[ITEM_UNIQUE]") and not has("[LOCKED]"),
               "nameOnly: the bind/lock region is cut (0x52bac3)")
        assert(not has("[ARMOR 100]"), "nameOnly: the stat body is cut (0x52c225)")
        assert(not has("\"Sealed.\""), "nameOnly: the early return (0x52e14e) drops the tail")
        assert(has("[INVTYPE_WEAPON]"), "nameOnly KEEPS the slot/type cell — between the two cuts")
        assert(has("[DURABILITY 50/90]") and has("[MIN_LEVEL 40]") and has("Zap"),
               "nameOnly KEEPS durability, the requirements and the triggers")

        -- ── the gate's polarity: is-number AND strictly > 0 (0x53304a) ────────────────────
        -- Absent, nil, 0, a negative and a non-numeric string all leave the flag at its zero
        -- seed, and none of them RAISES — this argument is not `number_arg`'s shape.
        local full = { nil, 0, -1, "nope", {}, false }
        for i = 1, 6 do
            tt:SetInventoryItem("player", 16, full[i])
            assert(has("[ARMOR 100]"), "a non-positive third argument builds the FULL tooltip")
        end
        -- A numeric STRING passes lua_isnumber, and a fraction is still > 0.
        tt:SetInventoryItem("player", 16, "1")
        assert(not has("[ARMOR 100]"), "a numeric string is a number to lua_isnumber")
        tt:SetInventoryItem("player", 16, 0.5)
        assert(not has("[ARMOR 100]"), "strictly greater than zero, not >= 1")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// Outside a compare: the full render, 1 for an occupied slot, nil for an empty or foreign one.
#[test]
fn set_inventory_item_renders_full_outside_compare() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut inv: InventorySlots = Default::default();
    inv[16] = Some(InvSlotView {
        durability: None,
        item_id: 8000,
        name: Some("Worn Axe".into()),
        quality: 2,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_item_template(
        8000,
        ItemTemplateView {
            name: "Worn Axe".into(),
            quality: 2,
            inventory_type: 21,
            description: "Chipped.".into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot9"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        assert(tt:SetInventoryItem("player", 16) == 1, "occupied slot answers 1")
        assert(TTTextLeft1:GetText() == "Worn Axe")
        local quoted = false
        for i = 1, tt:NumLines() do
            if getglobal("TTTextLeft" .. i):GetText() == "\"Chipped.\"" then quoted = true end
        end
        assert(quoted, "full render keeps the description")
        assert(tt:SetInventoryItem("player", 5) == nil, "empty slot answers nil")
        assert(tt:SetInventoryItem("target", 16) == nil, "self-only feed")

        -- THREE returns, on every leg. The reference's own PaperDollFrame.lua:741 destructures
        -- `hasItem, hasCooldown, repairCost` unconditionally and only then tests hasItem, so an
        -- empty slot answering ONE value hands its caller a nil where a number belongs. pfUI's
        -- durability scan (panel.lua:499) does `totalRep + repCost` with no guard at all and died
        -- exactly there.
        -- Counted through the implicit vararg table's `n` — 5.0's own answer, and the only one
        -- this VM has (`select` is 5.1's base library, not a 1.12 global). NOT `table.getn` on a
        -- captured table: 5.0's `luaL_getn` counts rawgeti to the first nil (decision 2102), so
        -- `{ f() }` where f answers `1, nil, 0` measures ONE — a hole, not a short return.
        local function count(...) return arg.n end
        assert(count(tt:SetInventoryItem("player", 16)) == 3,
            "occupied: three returns, got " .. count(tt:SetInventoryItem("player", 16)))
        local _, _, repairCost = tt:SetInventoryItem("player", 16)
        assert(repairCost == 0, "repairCost is a NUMBER — the reference always pushes one; 0 INTERIM")
        assert(count(tt:SetInventoryItem("player", 5)) == 3,
            "empty: three returns too, got " .. count(tt:SetInventoryItem("player", 5)))
        local hasItem, _, emptyCost = tt:SetInventoryItem("player", 5)
        assert(hasItem == nil and emptyCost == 0,
            "empty slot: no item, but still a numeric repairCost")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

#[test]
fn set_hyperlink_renders_items_and_ignores_other_links() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        7002,
        ItemTemplateView {
            name: "New Loop".into(),
            quality: 2,
            inventory_type: 11,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot10"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetHyperlink("|cff1eff00|Hitem:7002:0:0:0|h[New Loop]|h|r")
        assert(TTTextLeft1:GetText() == "New Loop", "escaped link renders")
        tt:SetHyperlink("player:Bob")
        assert(TTTextLeft1:GetText() == "New Loop", "non-item link leaves the content alone")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetHyperlink("item:7002")
        assert(TTTextLeft1:GetText() == "New Loop", "bare item: form renders")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A recipe whose product id is `spell_id + 10_000`, so the tooltip's name shows which row it hit.
fn ts_recipe(spell_id: u32, name: &str, group: (u32, u32, &str)) -> TradeSkillRecipe {
    TradeSkillRecipe {
        group: Some((group.0, group.1, group.2.to_string())),
        spell_id,
        name: name.into(),
        difficulty: TradeSkillDifficulty::Optimal,
        num_available: 1,
        icon: None,
        min_made: 1,
        max_made: 1,
        cooldown_secs: None,
        product_item: spell_id + 10_000,
        product_inv_type: 0,
        product_item_level: 0,
        reagents: vec![TradeSkillReagent {
            item: spell_id + 20_000,
            name: Some(format!("{name} Reagent")),
            icon: None,
            need: 1,
            have: 1,
        }],
        tools: vec![],
    }
}

/// `SetTradeSkillItem` takes a visible row index, headers included, not a position in `recipes`;
/// the product and reagent hovers both pass one, and a header row is a no-op.
#[test]
fn set_trade_skill_item_indexes_visible_rows_not_raw_recipes() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(r#"local tt = CreateFrame("GameTooltip", "TT")"#)
        .unwrap();

    // Two groups, two recipes each. Visible order: [1] "Bolts" header, [2] Alpha Bolt,
    // [3] Beta Bolt, [4] "Cloak" header, [5] Alpha Cloak, [6] Beta Cloak.
    s.set_trade_skill(Some(TradeSkillState {
        line: 197,
        line_name: "Tailoring".into(),
        rank: 57,
        max_rank: 75,
        recipes: vec![
            ts_recipe(1, "Beta Bolt", (1, 2, "Bolts")),
            ts_recipe(2, "Alpha Bolt", (1, 2, "Bolts")),
            ts_recipe(3, "Beta Cloak", (2, 1, "Cloak")),
            ts_recipe(4, "Alpha Cloak", (2, 1, "Cloak")),
        ],
        repeat_count: 1,
    }));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 6);

    // `TTTextLeft1` may not exist when nothing renders, so it is read defensively.
    let name_at = |s: &mut UiScript, row: i64, reagent: &str| -> String {
        s.run(&format!("TT:SetTradeSkillItem({row}{reagent})"))
            .unwrap();
        s.eval::<String>("return TTTextLeft1 and TTTextLeft1:GetText() or '<nothing rendered>'")
            .unwrap()
    };
    assert_eq!(name_at(&mut s, 5, ""), "Alpha Cloak");
    assert_eq!(name_at(&mut s, 6, ""), "Beta Cloak");
    assert_eq!(name_at(&mut s, 2, ""), "Alpha Bolt");

    assert_eq!(name_at(&mut s, 5, ", 1"), "Alpha Cloak Reagent");

    // A header row is a no-op: the previous render stands.
    assert_eq!(name_at(&mut s, 4, ""), "Alpha Cloak Reagent");
}

/// `SetMerchantCompareItem(index [, offset])` returns the number 1 or nil, one value, and the stock
/// vendor row shows each plate on it (`MerchantFrame.xml:67`).
#[test]
fn set_merchant_compare_item_answers_one_or_nil_per_candidate_slot() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);

    // Both finger slots worn, class 4 (armor) like the ring on the shelf.
    let mut inv: InventorySlots = Default::default();
    for (slot, id, name) in [(11usize, 7000u32, "Old Loop"), (12, 7001, "Older Loop")] {
        inv[slot] = Some(InvSlotView {
            item_id: id,
            name: Some(name.into()),
            quality: 1,
            ..Default::default()
        });
        s.set_item_template(
            id,
            ItemTemplateView {
                name: name.into(),
                quality: 1,
                class: 4,
                inventory_type: 11,
                ..Default::default()
            },
        );
    }
    // A shield, class 4, in the off hand makes offset 2 nil for a weapon.
    inv[17] = Some(InvSlotView {
        item_id: 7100,
        name: Some("Battered Buckler".into()),
        quality: 1,
        ..Default::default()
    });
    s.set_item_template(
        7100,
        ItemTemplateView {
            name: "Battered Buckler".into(),
            quality: 1,
            class: 4, // ARMOR
            inventory_type: 14,
            ..Default::default()
        },
    );
    inv[16] = Some(InvSlotView {
        item_id: 7101,
        name: Some("Bent Sword".into()),
        quality: 1,
        ..Default::default()
    });
    s.set_item_template(
        7101,
        ItemTemplateView {
            name: "Bent Sword".into(),
            quality: 1,
            class: 2, // WEAPON
            inventory_type: 13,
            ..Default::default()
        },
    );
    s.set_inventory_slots(inv);

    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Shiny Loop".into()),
                item_id: 8000,
                ..Default::default()
            },
            MerchantItem {
                name: Some("Sharp Sword".into()),
                item_id: 8001,
                ..Default::default()
            },
            MerchantItem {
                name: Some("Unseen Thing".into()),
                item_id: 8999, // no template: the uncached leg
                ..Default::default()
            },
        ],
        ..Default::default()
    }));
    s.set_item_template(
        8000,
        ItemTemplateView {
            name: "Shiny Loop".into(),
            quality: 3,
            class: 4,
            inventory_type: 11, // finger: two candidate slots
            ..Default::default()
        },
    );
    s.set_item_template(
        8001,
        ItemTemplateView {
            name: "Sharp Sword".into(),
            quality: 3,
            class: 2,
            inventory_type: 13, // one-hand: main hand, then off hand
            ..Default::default()
        },
    );

    s.run(
        r#"
        CreateFrame("GameTooltip", "ShoppingTooltip1"):Hide()
        CreateFrame("GameTooltip", "ShoppingTooltip2"):Hide()
    "#,
    )
    .unwrap();

    let call =
        |s: &mut UiScript, expr: &str| s.eval::<String>(&format!("return type({expr})")).unwrap();

    assert_eq!(
        s.eval::<String>(r#"return tostring(ShoppingTooltip1:SetMerchantCompareItem(1, 1))"#)
            .unwrap(),
        "1",
        "the truthy leg is lua_pushnumber(1.0), not a boolean"
    );
    assert_eq!(
        call(&mut s, "ShoppingTooltip2:SetMerchantCompareItem(1, 2)"),
        "number"
    );
    assert!(
        s.eval::<bool>(
            r#"local n = ShoppingTooltip1:NumLines()
               for i = 1, n do
                 if getglobal("ShoppingTooltip1TextLeft"..i):GetText() == "[CURRENTLY_EQUIPPED]" then
                   return true
                 end
               end
               return false"#
        )
        .unwrap(),
        "the fill is the equipped item's own tooltip with the CURRENTLY_EQUIPPED header"
    );

    // A one-hand weapon: the main hand (class 2) matches, the off-hand shield (class 4) does not.
    assert_eq!(
        call(&mut s, "ShoppingTooltip1:SetMerchantCompareItem(2, 1)"),
        "number"
    );
    assert_eq!(
        call(&mut s, "ShoppingTooltip1:SetMerchantCompareItem(2, 2)"),
        "nil",
        "a shield is not a candidate for a weapon — class, not slot"
    );

    // An absent offset is 1, not a raise.
    assert_eq!(
        call(&mut s, "ShoppingTooltip1:SetMerchantCompareItem(1)"),
        "number"
    );
    // An offset of 0 or less is nil: the counter starts below zero and only decrements.
    for bad in ["0", "-1"] {
        assert_eq!(
            call(
                &mut s,
                &format!("ShoppingTooltip1:SetMerchantCompareItem(1, {bad})")
            ),
            "nil",
            "offset {bad}"
        );
    }
    for expr in [
        "ShoppingTooltip1:SetMerchantCompareItem(0, 1)",
        "ShoppingTooltip1:SetMerchantCompareItem(99, 1)",
        "ShoppingTooltip1:SetMerchantCompareItem(3, 1)",
    ] {
        assert_eq!(call(&mut s, expr), "nil", "{expr}");
    }
    // 2.7 re-bases in f64, then truncates: 1.7 is row 1, the sword.
    assert_eq!(
        call(&mut s, "ShoppingTooltip1:SetMerchantCompareItem(2.7, 1)"),
        "number"
    );

    let e = s
        .run(r#"ShoppingTooltip1:SetMerchantCompareItem({}, 1)"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("Usage: SetMerchantCompareItem"), "{e}");
    // A numeric string passes `lua_isnumber`.
    assert_eq!(
        call(&mut s, r#"ShoppingTooltip1:SetMerchantCompareItem("1", 1)"#),
        "number"
    );
}
