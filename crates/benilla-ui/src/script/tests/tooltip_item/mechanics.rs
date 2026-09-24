//! Render mechanics around the line law: the sell-price money protocol (OnTooltipAddMoney +
//! ITEM_UNSELLABLE), wrapped-line measurement through the re-enter loop, and the unit health
//! bar hiding on an item render.

use std::collections::HashMap;

use super::script;
use crate::script::*;

/// The sell-price money protocol: SetBagItem at an open merchant fires OnTooltipAddMoney with
/// SellPrice × stack; price 0 prints the ITEM_UNSELLABLE line; no merchant → neither; the
/// in-flight template renders the link-name fallback and records the ask.
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
            name: Some("Backpack".into()),
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
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetScript("OnTooltipAddMoney", function(self) money_fired = arg1 end)
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
    // Zero sell price at a merchant: the ITEM_UNSELLABLE line, no money.
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

/// A wrap-flagged line carries its wrap column on the VERY FIRST measure ask (append_line pins
/// the region width at write time), so ONE round-trip answers the wrapped extent — and the
/// plate height/width converge even when the hover's re-enter loop clears + rebuilds the
/// content every frame. The old two-step (layout re-pinning after an overflowing unwrapped
/// measure) never converged under that loop: the wrapped re-measure was wiped before it could
/// land, wrap lines contributed no height, and everything after them spilled below the plate
/// (the live bread/hearthstone bug).
#[test]
fn wrap_lines_measure_wrapped_in_one_pass_and_survive_the_reenter_loop() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        7,
        ItemTemplateView {
            name: "Storybook".into(),
            quality: 1,
            charges: 1,
            description:
                "An exceedingly long tale of adventure and woe that would never fit on one line."
                    .into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot4"); a:SetPoint("TOPLEFT", 10, -10); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:BenillaSetItemById(7)
    "#,
    )
    .unwrap();
    s.resolve();
    // ONE measure pass, the frame loop's shape: the wrap-flagged description asks WITH the
    // wrap column; plain lines ask unconstrained.
    let answer = |s: &mut UiScript| {
        let reqs = s.fontstrings_needing_measure();
        let answers: Vec<(u32, f32, f32, u64)> = reqs
            .iter()
            .map(|r| {
                if r.text.starts_with('"') {
                    assert_eq!(
                        r.wrap_width,
                        Some(crate::widget::TOOLTIP_WRAP_WIDTH),
                        "a wrap line's FIRST ask carries the wrap column"
                    );
                    (r.id, 250.0, 36.0, r.key) // wrapped: 3 rows tall, inside the column
                } else {
                    (r.id, 60.0, 14.0, r.key)
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.resolve();
    };
    answer(&mut s);
    // name 14 + gap 2 + the charges line 14 + gap 2 + description 36 + 2·pad 20 = 88.
    s.run(
        r#"
        assert(TT:GetWidth() == 270, "wrap column + padding, got " .. TT:GetWidth())
        assert(TT:GetHeight() == 88, "wrap rows counted, got " .. TT:GetHeight())
    "#,
    )
    .unwrap();
    // The re-enter loop: clear + rebuild + one measure pass per frame, three frames — the size
    // must HOLD (the oscillation regression).
    for _ in 0..3 {
        s.run(
            r#"
            TT:SetOwner(Slot4, "ANCHOR_RIGHT")
            TT:BenillaSetItemById(7)
        "#,
        )
        .unwrap();
        s.resolve();
        answer(&mut s);
        s.run(
            r#"
            assert(TT:GetWidth() == 270 and TT:GetHeight() == 88,
                   "re-enter holds the wrapped size, got " .. TT:GetWidth() .. "x" .. TT:GetHeight())
        "#,
        )
        .unwrap();
    }
    assert!(s.take_errors().is_empty());
}

/// The mouseover health bar is UNIT content: an item render on the same tooltip HIDES it (the
/// live bug: the bar from the last mob hover rode under every item tooltip).
#[test]
fn item_render_hides_the_unit_health_bar() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit(
        "mouseover",
        Some(UnitState {
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
        local tt = CreateFrame("GameTooltip", "GameTooltip"); tt:Hide()
        local bar = CreateFrame("StatusBar", "GameTooltipStatusBar", tt)
        bar:SetPoint("TOPLEFT", tt, "BOTTOMLEFT", 2, -1); bar:SetWidth(100); bar:SetHeight(8)
    "#,
    )
    .unwrap();
    assert!(s.world_tooltip_unit("mouseover"), "the unit hover shows");
    s.run(r#"assert(GameTooltipStatusBar:IsShown(), "unit hover shows the bar")"#)
        .unwrap();
    s.run(
        r#"
        GameTooltip:SetOwner(Slot6, "ANCHOR_RIGHT")
        GameTooltip:BenillaSetItemById(9)
        assert(not GameTooltipStatusBar:IsShown(), "item content hides the unit bar")
        assert(GameTooltip:IsShown(), "the item tooltip itself shows")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A REAL-INSTANCE hover renders the instance's LIVE durability pair, not the template's full
/// max/max — the wire updates `ITEM_FIELD_DURABILITY` on damage (death 10%, spirit healer 25%;
/// director-reported: the 25% loss showed nowhere). A plain template/link hover keeps max/max.
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
            name: Some("Backpack".into()),
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
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 2)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 30/40]", "live pair on a bag hover, got " .. tostring(found))
        -- The template/link hover of the SAME item keeps the authored full pair.
        tt:BenillaSetItemById(2264)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 40/40]", "template hover stays full, got " .. tostring(found))
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// FULLY BROKEN gear renders its true `0 / max` pair (director-caught: gear ground to durability
/// 0 by repeated spirit rezzes read as 100% — the create block omits the zero `DURABILITY` field,
/// and the sparse `None` fell back to the template's max/max; `ObjectFields`' created semantics
/// now feed the explicit 0 through).
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
            name: Some("Backpack".into()),
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
        local tt = CreateFrame("GameTooltip", "TT")
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
    // Red iff broken (`0x854bc4`, colour `0xc0d390`): the 0/40
    // line paints the requirement red, never white.
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

/// The **enchant lines** (law line 17, decision 0915) — the director's report: an axe carrying
/// Enchant Weapon - Agility showed its green glow in the world and said nothing in the tooltip.
///
/// Three claims at once: the line renders from the instance's resolved enchant text, it sits
/// **between the resistances and the durability line** (the law's 16 → 17 → 18), and it is green.
/// The control is the template/link hover of the same item — no instance, no enchant, no line.
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
            // What the app resolved from `ITEM_FIELD_ENCHANTMENT` slot 0 → enchant 2564 →
            // `SpellItemEnchantment`'s own name string. Slot 0 = permanent, positive id.
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
            name: Some("Backpack".into()),
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
        local tt = CreateFrame("GameTooltip", "TT")
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

    // The control: the same item as a TEMPLATE hover has no instance, so no enchant line.
    s.run(r#"TT:BenillaSetItemById(22816)"#).unwrap();
    let lines = super::lines_of(&mut s);
    assert!(
        !lines.iter().any(|(t, _)| t.starts_with("Agility")),
        "a template hover carries no instance and so no enchant line: {lines:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The **enchant colour bands** (`0x52ca29`, decision 0920). The colour is per SLOT, not per
/// family: only slots 0 (permanent) and 1 (temporary) are ever coloured — green for a positive id,
/// the tooltip's OTHER red (`0xc0d398 = ffff0000`, distinct from the requirement lines' `ffff2020`)
/// for a negative one — and the random-property slots 2..6 are always white whatever the sign. Our
/// first cut painted every slot green.
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
                // A suffix slot with a NEGATIVE id is still white — the band, not the sign, rules.
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
            name: Some("Backpack".into()),
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
        local tt = CreateFrame("GameTooltip", "TT")
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

/// The temporary enchant's countdown REPLACES the name in the same line (never a second one), and
/// the charges suffix rides after it — the `0x52fa50` bucket ladder and the `" (%s)"` join. The
/// countdown's source is `SMSG_ITEM_ENCHANT_TIME_UPDATE`, so a slot with no packet shows the bare
/// name: that is the control here.
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
            name: Some("Backpack".into()),
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
        local tt = CreateFrame("GameTooltip", "TT")
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

/// **The loot hover shows the ROLL, never the placeholder** (decision 1547). `SetLootItem 0x533470`
/// writes an instance block (p6=1) whose `+0x424` is the wire's randomPropertyId and whose seven
/// enchant slots are zeroed, and passes an all-zero item GUID — so there is never an object, the
/// builder takes the suffix-row copy (`0x52b7e0`) into slots 2..6, and the `ITEM_RANDOM_ENCHANT`
/// arm is unreachable (the entry fork, `0x52c991`). The row's name is the suffix-joined one
/// (`0x5d8b00`, the same string `GetLootSlotInfo` returns), so the plate reads "… of the Monkey".
///
/// This is the reported bug's exact shape: through the template path (`BenillaSetItemById`) the same
/// hover printed the ITEM_RANDOM_ENCHANT placeholder until the item reached a bag.
#[test]
fn a_looted_roll_shows_its_lines_and_never_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            // The template CAN roll — the placeholder's own gate, so its absence below is the law
            // and not an accident of the fixture.
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_loot(Some(LootState {
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            name: Some("Bloodrazor of the Monkey".into()),
            texture: None,
            quantity: 1,
            quality: Some(2),
            is_coin: false,
            item_id: 8888,
            link: Some("|cff1eff00|Hitem:8888:0:584:0|h[Bloodrazor of the Monkey]|h|r".into()),
            // The row carries the ROLL, not resolved lines — the engine resolves it, like the
            // reference reading its own DBC store off the block's `+0x424`.
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
        local tt = CreateFrame("GameTooltip", "TT")
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
        // Slots 2..6 are always WHITE, whatever the sign (`0x52ca29`).
        assert_eq!(l.1, [1.0, 1.0, 1.0, 1.0], "{line} is white");
    }
}

/// The other half of the same law: a loot row with **no** roll shows no enchant line AND no
/// placeholder, even though the template can roll. The fork tests the block's presence, not its
/// contents (`0x52c9a3`) — which is why this cannot be expressed as "print the placeholder when the
/// slots are empty".
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
        local tt = CreateFrame("GameTooltip", "TT")
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

/// A **chat link** is a block source too (`SetHyperlink 0x532181`, p6=1): its `|Hitem:` token 2 is
/// the roll, which `0x52b7e0` expands into slots 2..6, and the placeholder arm is unreachable.
/// Decision 0920's prose said a hyperlink hover shows the placeholder; `0x52c9a3` says otherwise,
/// and this is the case that proves it — the same drop, linked in chat instead of hovered in the
/// loot window, must read identically.
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
        local tt = CreateFrame("GameTooltip", "TT")
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

/// `ITEM_RANDOM_ENCHANT` (`0x52cc33`) — the template-only placeholder: a random-property item with
/// NO instance to read a roll from prints it, green, and the per-slot lines and this one are
/// mutually exclusive by construction. The control is the same template hovered as a real enchanted
/// instance: the roll is known, so the placeholder gives way to the slot lines.
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
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:BenillaSetItemById(8888)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let placeholder = lines
        .iter()
        .find(|(t, _)| t == "[ITEM_RANDOM_ENCHANT]")
        .expect("the template hover shows the placeholder");
    assert_eq!(placeholder.1, [0.0, 1.0, 0.0, 1.0], "green");

    // With a real instance (the roll is known) the placeholder is gone and the slots print.
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
            name: Some("Backpack".into()),
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
