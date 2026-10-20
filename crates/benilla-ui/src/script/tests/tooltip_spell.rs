//! The engine spell and aura tooltips: name|rank, one cost|range line, one casttime|cooldown line,
//! the passive omission, the aura builder (white description, and the gold remaining line on
//! `SetPlayerBuff` only), and the selectors that delegate to them.

use super::common::script;
use crate::script::*;

/// The aura builder's key for an aura pushed with the default level and count.
fn aura_key(spell_id: u32) -> AuraKey {
    AuraKey {
        spell_id,
        ..Default::default()
    }
}

fn aura_view(name: &str, description: &str) -> AuraTooltipView {
    AuraTooltipView {
        name: name.into(),
        description: description.into(),
        ..Default::default()
    }
}

fn fireball() -> SpellTooltipView {
    SpellTooltipView {
        name: "Fireball".into(),
        rank: Some("Rank 1".into()),
        cost: Some("30 Mana".into()),
        range: Some("30 yd range".into()),
        cast_time: Some("1.5 sec cast".into()),
        cooldown: None,
        description: "Hurls a fiery ball that causes 14 to 22 Fire damage.".into(),
        ..Default::default()
    }
}

/// The spellbook hover: no rank column, cost|range on one line, cast time, gold description.
#[test]
fn spellbook_hover_renders_the_verified_shape() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(133, fireball());
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 133,
            name: "Fireball".into(),
            ..Default::default()
        }],
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "SB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetSpell(1, "spell")
        assert(tt:IsShown(), "SetSpell shows")
        assert(tt:NumLines() == 4, "name + cost|range + casttime + description, got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Fireball")
        -- Byte-verified: SetSpell passes showRank=0 — the spellbook hover shows NO rank column
        -- (only SetAction shows it).
        assert(not TTTextRight1:IsShown(), "no rank column from SetSpell")
        assert(TTTextLeft2:GetText() == "30 Mana")
        assert(TTTextRight2:GetText() == "30 yd range", "cost|range is ONE line")
        assert(TTTextLeft3:GetText() == "1.5 sec cast")
        assert(TTTextLeft4:GetText() ~= nil)
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let gold = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t.starts_with("Hurls") && (c[1] - 210.0/255.0).abs() < 1e-6)
    });
    assert!(gold, "spell description is gold");
    assert!(s.take_errors().is_empty());
}

/// Every cell's template is read at the hover (`0x703bf0` at each call, `0x52ea4c` for
/// `SPELL_RANGE`), not when the view was pushed: a string rewritten after the push reaches the
/// next hover, the description's token templates too.
#[test]
fn a_rewritten_global_string_reaches_the_next_hover() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let secs = TextPart::Keyed {
        key: "INT_SPELL_DURATION_SEC".into(),
        ordinal: Some(8),
        args: Some(vec![KeyedArg::D(8)]),
        or: "$d".into(),
    };
    s.set_spell_tooltip(
        133,
        SpellTooltipView {
            name: "Fireball".into(),
            cost: Some(TooltipText::keyed("MANA_COST", vec![KeyedArg::D(30)])),
            range: Some(TooltipText::keyed(
                "SPELL_RANGE",
                vec![KeyedArg::S("35".into())],
            )),
            reagents: Some(TooltipText::copied("SPELL_REAGENTS").then("Rune")),
            description: TooltipText(vec![TextPart::Text("Burns for ".into()), secs]),
            ..Default::default()
        },
    );
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 133,
            name: "Fireball".into(),
            ..Default::default()
        }],
    });
    s.run(
        r#"
        MANA_COST = "%d Mana"; SPELL_RANGE = "%s yd range"; SPELL_REAGENTS = "Reagents: "
        INT_SPELL_DURATION_SEC = "%d sec"; INT_SPELL_DURATION_SEC_P1 = "%d sec"
        local a = CreateFrame("Button", "SBR"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetSpell(1, "spell")
        assert(TTTextLeft2:GetText() == "30 Mana", tostring(TTTextLeft2:GetText()))
        assert(TTTextRight2:GetText() == "35 yd range")
        assert(TTTextLeft3:GetText() == "Reagents: Rune")
        assert(TTTextLeft4:GetText() == "Burns for 8 sec")
        MANA_COST = "Mana: %d"; SPELL_RANGE = "Range: %s yards"; SPELL_REAGENTS = "Uses "
        INT_SPELL_DURATION_SEC_P1 = "%d seconds"
        TT:SetSpell(1, "spell")
        assert(TTTextLeft2:GetText() == "Mana: 30", tostring(TTTextLeft2:GetText()))
        assert(TTTextRight2:GetText() == "Range: 35 yards")
        assert(TTTextLeft3:GetText() == "Uses Rune")
        assert(TTTextLeft4:GetText() == "Burns for 8 seconds")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// `SetSpell`'s `bookType` picks the book (`0x532d10` forks at `0x532e1c`, `0x532e2a`: the pet's
/// slots at `0xb6f098`, the player's at `0xb700f0`); both books hold a different spell at slot 1.
#[test]
fn a_pet_book_hover_reads_the_pets_book_not_the_players() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(133, fireball());
    // The pet book's flag is the builder's unit selector, so its hover reads the pet's view.
    s.set_pet_spell_tooltip(
        3110,
        SpellTooltipView {
            name: "Firebolt".into(),
            description: "Hurls a fiery ball.".into(),
            ..Default::default()
        },
    );
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 133,
            name: "Fireball".into(),
            ..Default::default()
        }],
    });
    s.set_pet_book(PetBookState {
        token: Some("DEMON".into()),
        slots: vec![SpellSlotView {
            spell_id: 3110,
            name: "Firebolt".into(),
            ..Default::default()
        }],
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "SB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetSpell(1, "pet")
        assert(TTTextLeft1:GetText() == "Firebolt", "pet slot 1, got " .. TTTextLeft1:GetText())
        -- The same slot id in the other book is the OTHER spell.
        tt:SetSpell(1, "spell")
        assert(TTTextLeft1:GetText() == "Fireball", "player slot 1, got " .. TTTextLeft1:GetText())
        -- The compare is case-insensitive against "pet" ALONE (0x532e13's SStrCmpI), so every
        -- other string is the player's book — including a mis-cased one being the pet's.
        tt:SetSpell(1, "PeT")
        assert(TTTextLeft1:GetText() == "Firebolt", "case-insensitive pet")
        tt:SetSpell(1, "book")
        assert(TTTextLeft1:GetText() == "Fireball", "any other string is the player's book")
        -- A number is stringified (0x6f3510 accepts it) and so is the player's book.
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetSpell(1, 7)
        assert(TTTextLeft1:GetText() == "Fireball", "a number book type is the player's book")
        -- A nil, table or boolean book type, a non-number index, and an index outside 1..1024
        -- (trunc(n - 1) outside [0, 0x400)) raise "Invalid spell slot in SetSpell" (0x855290).
        local function raises(...)
            local ok, err = pcall(tt.SetSpell, tt, unpack(arg))
            return not ok and string.find(tostring(err), "Invalid spell slot in SetSpell", 1, true)
        end
        assert(raises(1, nil), "nil book type")
        assert(raises(1, {}), "table book type")
        assert(raises(1, true), "boolean book type")
        assert(raises("x", "spell"), "non-number index")
        assert(raises(0, "spell"), "index 0")
        assert(raises(1025, "spell"), "index 1025")
        assert(raises(-3, "spell"), "negative index")
        assert(not raises(1024, "spell"), "index 1024 is in range and misses quietly")
        -- `_ftol` wraps to the low dword (0x532dd9): 4294967297 - 1 is 2^32, which is slot 1.
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetSpell(4294967297, "spell")
        assert(TTTextLeft1:GetText() == "Fireball", "a wrapped index reads slot 1")
        assert(raises(4294967297 + 1024, "spell"), "2^32 + 1024 wraps to slot 1025, past the table")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// `SetTrackingSpell` is the aura builder at level 1 and stack 1 (`0x532cde`-`0x532ce7`): a gold
/// name over the white aura description, with no cost, cast time or remaining line.
#[test]
fn tracking_hover_renders_gold_name_over_white_description() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_tracking(Some(TrackingState {
        spell_id: 2580,
        name: Some("Find Minerals".into()),
        icon: Some("Interface\\Icons\\Trade_Mining".into()),
        cancelable: true,
    }));
    s.set_spell_tooltip(
        2580,
        SpellTooltipView {
            name: "Find Minerals".into(),
            cost: Some("a cost cell that must NOT render".into()),
            cast_time: Some("nor this".into()),
            description: "Not the aura's text.".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        s.aura_tooltip_subjects(),
        vec![AuraKey {
            spell_id: 2580,
            level: 1,
            stack: 1
        }]
    );
    s.set_aura_tooltip(
        AuraKey {
            spell_id: 2580,
            level: 1,
            stack: 1,
        },
        aura_view("Find Minerals", "Finding Minerals."),
    );
    s.run(
        r#"
        local f = CreateFrame("Frame", "TRK"); f:SetPoint("CENTER", 0, 0); f:SetWidth(10); f:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(f, "ANCHOR_BOTTOMLEFT")
        tt:SetTrackingSpell()
        assert(tt:IsShown(), "SetTrackingSpell shows")
        assert(tt:NumLines() == 2, "name + description only, got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Find Minerals")
        assert(TTTextLeft2:GetText() == "Finding Minerals.")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let gold_name = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "Find Minerals" && (c[1] - 210.0/255.0).abs() < 1e-6)
    });
    let white_desc = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "Finding Minerals." && (c[1] - 1.0).abs() < 1e-6 && (c[2] - 1.0).abs() < 1e-6)
    });
    assert!(gold_name, "the tracking name line is GOLD");
    assert!(white_desc, "the description is the aura-variant white");
    assert!(s.take_errors().is_empty());
}

/// A passive spell omits the casttime|cooldown line whole, with no "Passive" text line.
#[test]
fn passive_omits_the_casttime_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(
        1461,
        SpellTooltipView {
            name: "Frost Warding".into(),
            rank: None,
            cost: None,
            range: None,
            cast_time: None, // passive
            cooldown: Some("never shows without a casttime".into()),
            description: "Increases your frost ward.".into(),
            ..Default::default()
        },
    );
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 1461,
            name: "Frost Warding".into(),
            ..Default::default()
        }],
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "SB2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetSpell(1, "spell")
        assert(tt:NumLines() == 2, "name + description only, got " .. tt:NumLines())
        local all = ""
        for i = 1, tt:NumLines() do all = all .. (getglobal("TTTextLeft" .. i):GetText() or "") end
        assert(not string.find(all, "Passive"), "no Passive text line")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The buff hover is the aura builder: a white description and, on this entry point only, the
/// remaining-time line computed off the aura's `GetTime` expiry.
#[test]
fn player_buff_hover_is_the_aura_variant() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(
        1459,
        SpellTooltipView {
            name: "Arcane Intellect".into(),
            rank: Some("Rank 1".into()),
            cost: Some("60 Mana".into()), // the spell builder's, never the aura's
            description: "Increases Intellect by 2.".into(),
            ..Default::default()
        },
    );
    // The record's level and stack key the view (`0x532bbf`-`0x532bd2`).
    s.set_aura_tooltip(
        AuraKey {
            spell_id: 1459,
            level: 60,
            stack: 1,
        },
        aura_view("Arcane Intellect", "Intellect increased by 2."),
    );
    s.tick(10.0); // GetTime = 10
    s.set_player_auras(vec![AuraState {
        spell_id: 1459,
        name: Some("Arcane Intellect".into()),
        count: 1,
        level: 60,
        duration: 1800.0,
        expiration_time: 100.0, // 90 s left at now = 10
        helpful: true,
        ..Default::default()
    }]);
    s.run(
        r#"
        -- The duration line's wording comes from the VM's GlobalStrings, which the app runs off
        -- the player's own install at boot. This VM has no install, so the test supplies the pair
        -- itself — deliberately NOT the shipped wording, because what is under test here is that
        -- the engine reads the string table at all, not what the strings say.
        SPELL_TIME_REMAINING_MIN    = "<%d m>"
        SPELL_TIME_REMAINING_MIN_P1 = "<%d mm>"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        -- 0, not 1: SetPlayerBuff takes a 1.12 CACHE POSITION (see `script::aura`'s header).
        tt:SetPlayerBuff(0)
        assert(tt:NumLines() == 3, "name + description + remaining, got " .. tt:NumLines())
        assert(TTTextLeft2:GetText() == "Intellect increased by 2.", "the AURA description column")
        -- 90 s left ceils to 2 minutes, and 2 takes the _P1 twin.
        assert(TTTextLeft3:GetText() == "<2 mm>", "got " .. TTTextLeft3:GetText())
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let white = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t.starts_with("Intellect increased") && *c == [1.0, 1.0, 1.0, 1.0])
    });
    assert!(white, "aura description is white, not gold");
    // The name is gold: the aura builder `0x52f880` writes it through the gold wrapper
    // `0x530380`, where the spell builder uses the plain `0x530270`.
    let gold_name = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "Arcane Intellect" && (c[1] - 210.0 / 255.0).abs() < 1e-6)
    });
    assert!(gold_name, "aura name is gold, not white");
    // So is the remaining line, the same `0xffffd200`, as in the reference's buff hover.
    let gold_remaining = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "<2 mm>" && *c == [1.0, 210.0 / 255.0, 0.0, 1.0])
    });
    assert!(gold_remaining, "duration-remaining line is gold, not white");
    assert!(s.take_errors().is_empty());
}

/// The buff builder's (`0x52f880`) right column is the dispel class, gold through the aura name's
/// wrapper, where the spell hover puts a gray rank; a `rank` on the view must not displace it.
#[test]
fn player_buff_hover_names_the_dispel_class_in_gold() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(
        168,
        SpellTooltipView {
            name: "Ice Armor".into(),
            rank: Some("Rank 1".into()), // the spell builder's column, never the aura's
            ..Default::default()
        },
    );
    s.set_aura_tooltip(
        aura_key(168),
        AuraTooltipView {
            dispel_type: Some("Magic".into()),
            ..aura_view("Ice Armor", "Encases the caster in a layer of ice.")
        },
    );
    s.set_player_auras(vec![AuraState {
        spell_id: 168,
        name: Some("Ice Armor".into()),
        helpful: true,
        ..Default::default()
    }]);
    s.run(
        r#"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetPlayerBuff(0)
        assert(TTTextRight1:GetText() == "Magic",
            "the dispel class, got " .. tostring(TTTextRight1:GetText()))
    "#,
    )
    .unwrap();
    s.resolve();
    let gold_dispel = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "Magic" && (c[1] - 210.0 / 255.0).abs() < 1e-6)
    });
    assert!(
        gold_dispel,
        "the dispel column is gold, not the rank's gray"
    );
    assert!(s.take_errors().is_empty());
}

/// `0x52e610`'s line order: cast|cooldown, requires-item, requires-form, reagents, description,
/// each requirement red while unmet.
#[test]
fn requirement_and_reagent_lines_render_in_law_order() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(
        5019,
        SpellTooltipView {
            name: "Shoot".into(),
            range: Some("30 yd range".into()),
            cast_time: Some("Instant".into()),
            requires_item: Some("Wands".into()),
            item_met: false,
            reagents: Some("Reagents: |cffff2020Light Feather|r".into()),
            description: "Attack with an equipped wand.".into(),
            ..Default::default()
        },
    );
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 5019,
            name: "Shoot".into(),
            ..Default::default()
        }],
    });
    s.run(
        r#"
        SPELL_EQUIPPED_ITEM = "Requires %s"
        local a = CreateFrame("Button", "SB2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetSpell(1, "spell")
        assert(tt:NumLines() == 6, "name + range + cast + requires + reagents + desc, got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Shoot")
        assert(TTTextLeft2:GetText() == "30 yd range", "cost absent: range moves left")
        assert(TTTextLeft3:GetText() == "Instant")
        assert(TTTextLeft4:GetText() == "Requires Wands", "the requirement sits above the reagents")
        assert(string.find(TTTextLeft5:GetText(), "Light Feather") ~= nil, "the reagents line")
        assert(TTTextLeft6:GetText() == "Attack with an equipped wand.", "description last")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let red = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "Requires Wands" && *c == [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0])
    });
    assert!(red, "an unmet equipped-item requirement is red");
    // The reagent's `|cffff2020` escape reaches the region verbatim over a white base colour; the
    // app's text layer (`ui_text::markup`) turns it into colour runs.
    let verbatim = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "Reagents: |cffff2020Light Feather|r" && *c == [1.0, 1.0, 1.0, 1.0])
    });
    assert!(
        verbatim,
        "the reagents line reaches the region escape-intact, base white"
    );
    assert!(s.take_errors().is_empty());
}

/// `SetUnitBuff`/`SetUnitDebuff` render the aura builder from the unit's sign-filtered list, with
/// no remaining line: only `SetPlayerBuff` appends it, and the 1.12 wire carries no other unit's
/// durations.
#[test]
fn unit_buff_and_debuff_hover_render_the_aura_variant_without_remaining() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_aura_tooltip(
        aura_key(589),
        aura_view(
            "Shadow Word: Pain",
            "Suffering 12 Shadow damage every 3 sec.",
        ),
    );
    s.set_aura_tooltip(
        aura_key(1126),
        aura_view("Mark of the Wild", "Armor increased by 25."),
    );
    // The target, guid 7, through the resolver.
    s.set_unit_guids(&crate::script::UnitGuids {
        target: 7,
        ..Default::default()
    });
    s.set_unit_auras(
        7,
        Some(
            vec![
                AuraState {
                    spell_id: 1126,
                    name: Some("Mark of the Wild".into()),
                    helpful: true,
                    ..Default::default()
                },
                AuraState {
                    spell_id: 589,
                    name: Some("Shadow Word: Pain".into()),
                    helpful: false,
                    ..Default::default()
                },
            ]
            .into(),
        ),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "TF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_BOTTOMRIGHT", 15, -25)
        tt:SetUnitDebuff("target", 1)
        assert(tt:NumLines() == 2, "name + description, NO remaining line, got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Shadow Word: Pain", "the sign filter picks the debuff")
        assert(TTTextLeft2:GetText() == "Suffering 12 Shadow damage every 3 sec.")
        tt:SetOwner(a, "ANCHOR_BOTTOMRIGHT", 15, -25)
        tt:SetUnitBuff("target", 1)
        assert(TTTextLeft1:GetText() == "Mark of the Wild", "the sign filter picks the buff")
        tt:SetOwner(a, "ANCHOR_BOTTOMRIGHT", 15, -25)
        tt:SetUnitDebuff("target", 2)
        assert(tt:NumLines() == 0 and not tt:IsShown(), "past the list: no empty plate")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// `SetPlayerBuff` takes a 1.12 cache position, the `GetPlayerBuff` family's index space:
/// `BuffFrame.lua:105` passes that return straight in, and a surplus filter argument is ignored.
/// After `SetOwner`'s clear, a position past the cache leaves no plate.
#[test]
fn player_buff_hover_indexes_the_cache_position_not_a_filtered_ordinal() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, name, desc) in [
        (1126, "Mark of the Wild", "Armor increased by 25."),
        (2457, "Battle Stance", "A balanced combat stance."),
        (589, "Shadow Word: Pain", "Suffering 12 Shadow damage."),
    ] {
        s.set_aura_tooltip(aura_key(id), aura_view(name, desc));
    }
    // The player's cache: two buffs then a debuff, in one insertion-ordered list.
    s.set_player_auras(vec![
        AuraState {
            spell_id: 1126,
            name: Some("Mark of the Wild".into()),
            helpful: true,
            ..Default::default()
        },
        AuraState {
            spell_id: 2457,
            name: Some("Battle Stance".into()),
            helpful: true,
            ..Default::default()
        },
        AuraState {
            spell_id: 589,
            name: Some("Shadow Word: Pain".into()),
            helpful: false,
            ..Default::default()
        },
    ]);
    s.run(
        r#"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        local function at(i, extra)
            tt:SetOwner(a, "ANCHOR_RIGHT")
            tt:SetPlayerBuff(i, extra)
            return TTTextLeft1 and TTTextLeft1:GetText()
        end
        assert(at(0) == "Mark of the Wild", "position 0 is the first aura, got " .. tostring(at(0)))
        assert(at(1) == "Battle Stance", "position 1 is the SECOND, got " .. tostring(at(1)))
        assert(at(2) == "Shadow Word: Pain", "the debuff shares the index space, got " .. tostring(at(2)))
        -- The handle really is GetPlayerBuff's return, round-tripped.
        assert(at(GetPlayerBuff(0, "HARMFUL")) == "Shadow Word: Pain", "round-trip through GetPlayerBuff")
        -- A surplus filter argument is ignored, as the reference's binding ignores it.
        assert(at(1, "HELPFUL|HARMFUL") == "Battle Stance", "surplus argument must be ignored")
        -- SetOwner clears, and a miss adds nothing (a scan that stops on a nil first line ends).
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:SetPlayerBuff(3)
        assert(tt:NumLines() == 0 and not tt:IsShown(), "past the cache: no plate")
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:SetPlayerBuff(-1)
        assert(tt:NumLines() == 0 and not tt:IsShown(), "-1 must clear, not show the first buff")
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:SetPlayerBuff(GetPlayerBuff(9, "HARMFUL"))
        assert(tt:NumLines() == 0 and not tt:IsShown(), "an unguarded -1 from GetPlayerBuff clears")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// `UnitBuff("player")` and `GameTooltip:SetUnitBuff("player")` walk the player's slots, as any
/// unit's (`0x51960f`, `0x534bfb`), at the slot's live `AURALEVELS` byte (`0x534ceb`);
/// `GetPlayerBuff` and `SetPlayerBuff` read the buff cache, in its insertion order and at the
/// level its record holds (`0x532bbf`).
#[test]
fn the_player_token_reads_the_slots_and_set_player_buff_the_cache() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit_guids(&crate::script::UnitGuids {
        player: 0x10,
        ..Default::default()
    });
    let aura = |spell_id, name: &str, level| AuraState {
        spell_id,
        name: Some(name.into()),
        level,
        count: 1,
        helpful: true,
        until_cancelled: true,
        ..Default::default()
    };
    // The cache: Mark first, then Fortitude at the level its record holds.
    s.set_player_auras(vec![
        aura(1126, "Mark of the Wild", 10),
        aura(1243, "Power Word: Fortitude", 20),
    ]);
    // The slots: Fortitude in slot 0, recast at level 40, Mark in slot 1.
    s.set_unit_auras(
        0x10,
        Some(
            vec![
                aura(1243, "Power Word: Fortitude", 40),
                aura(1126, "Mark of the Wild", 10),
            ]
            .into(),
        ),
    );
    for level in [20, 40] {
        s.set_aura_tooltip(
            AuraKey {
                spell_id: 1243,
                level,
                stack: 1,
            },
            aura_view(
                "Power Word: Fortitude",
                &format!("Stamina at level {level}."),
            ),
        );
    }
    assert_eq!(s.aura_spell_ids("player", true), [1243, 1126]);
    s.run(
        r#"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetUnitBuff("player", 1)
        assert(TTTextLeft2:GetText() == "Stamina at level 40.", "the slot's live level")
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetPlayerBuff(1)
        assert(TTTextLeft2:GetText() == "Stamina at level 20.", "the record's level")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The aura setters' misses return before the builder and leave the tooltip as it was: an index
/// out of `SetUnitBuff`'s 32 or `SetUnitDebuff`'s 16 (`0x534bcb`, `0x534f3e`) or past the unit's
/// list (`0x534c9c`, `0x53501d`), a position outside the 48 buff records (`0x532bbd`), and no
/// tracking spell (`0x532cda`). A unit that is not a string or an index that is not a number
/// raises the binding's usage line (`0x534dd2`).
#[test]
fn an_aura_setters_miss_leaves_the_tooltip_as_it_was() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, name) in [(1126, "Mark of the Wild"), (589, "Shadow Word: Pain")] {
        s.set_aura_tooltip(aura_key(id), aura_view(name, ""));
    }
    s.set_unit_guids(&crate::script::UnitGuids {
        target: 7,
        ..Default::default()
    });
    let auras = vec![
        AuraState {
            spell_id: 1126,
            name: Some("Mark of the Wild".into()),
            helpful: true,
            ..Default::default()
        },
        AuraState {
            spell_id: 589,
            name: Some("Shadow Word: Pain".into()),
            helpful: false,
            ..Default::default()
        },
    ];
    s.set_unit_auras(7, Some(auras.clone().into()));
    s.set_player_auras(auras);
    s.run(
        r#"
        local a = CreateFrame("Button", "TF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_BOTTOMRIGHT")
        tt:SetUnitDebuff("target", 1)
        local function kept(call)
            assert(tt:IsShown() and tt:NumLines() == 1, call .. ": " .. tt:NumLines() .. " lines")
            assert(TTTextLeft1:GetText() == "Shadow Word: Pain",
                call .. " drew " .. tostring(TTTextLeft1:GetText()))
        end
        kept("the debuff")
        tt:SetUnitBuff("target", 0); kept("buff 0")
        tt:SetUnitBuff("target", -3); kept("buff -3")
        tt:SetUnitBuff("target", 2); kept("past the buffs")
        tt:SetUnitBuff("target", 33); kept("buff 33")
        tt:SetUnitDebuff("target", 17); kept("debuff 17")
        tt:SetPlayerBuff(-1); kept("record -1")
        tt:SetPlayerBuff(48); kept("record 48")
        tt:SetTrackingSpell(); kept("no tracking")
        tt:SetUnitBuff("target", "1"); assert(TTTextLeft1:GetText() == "Mark of the Wild",
            "a numeric string is a number")
        assert(not pcall(tt.SetUnitBuff, tt, "target"), "a nil index raises")
        local ok, err = pcall(tt.SetUnitDebuff, tt, {}, 1)
        assert(not ok and string.find(err, 'Usage: SetUnitDebuff("unit", index [,raidFilter])', 1, true),
            tostring(err))
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The aura builder is called with the aura's level and stack (`0x534ceb`, `0x534cd1`; the
/// player's record, `0x532bbf`), and its view is keyed by them: a view built for another level or
/// count is not this aura's. A miss draws the gold name, asks for the key and waits; the answer
/// re-renders the hover.
#[test]
fn an_aura_hover_asks_for_its_own_level_and_stack() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let rend = AuraKey {
        spell_id: 3147,
        level: 25,
        stack: 1,
    };
    let sunder = AuraKey {
        spell_id: 11597,
        level: 60,
        stack: 3,
    };
    // Views at the default level and count, which neither aura is.
    s.set_aura_tooltip(aura_key(3147), aura_view("Rend Flesh", "1 damage"));
    s.set_aura_tooltip(
        aura_key(11597),
        aura_view("Sunder Armor", "Armor decreased by 450."),
    );
    s.set_player_auras(vec![AuraState {
        spell_id: 3147,
        name: Some("Rend Flesh".into()),
        level: 25,
        count: 1,
        helpful: false,
        until_cancelled: true,
        ..Default::default()
    }]);
    s.set_unit_guids(&crate::script::UnitGuids {
        target: 7,
        ..Default::default()
    });
    s.set_unit_auras(
        7,
        Some(
            vec![AuraState {
                spell_id: 11597,
                name: Some("Sunder Armor".into()),
                level: 60,
                count: 3,
                helpful: false,
                ..Default::default()
            }]
            .into(),
        ),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetPlayerBuff(0)
        assert(TT:NumLines() == 1 and TTTextLeft1:GetText() == "Rend Flesh", "the name alone")
    "#,
    )
    .unwrap();
    assert_eq!(s.take_aura_tooltip_asks(), vec![rend]);
    s.set_aura_tooltip(rend, aura_view("Rend Flesh", "32 to 33 damage"));
    assert_eq!(
        s.eval::<String>("return TTTextLeft2:GetText()").unwrap(),
        "32 to 33 damage",
        "the answer re-renders the hover"
    );
    s.run(r#"TT:SetUnitDebuff("target", 1)"#).unwrap();
    assert_eq!(s.take_aura_tooltip_asks(), vec![sunder]);
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The aura text is the only description the builder reads: an empty one is no line, with no
/// fall back to the spell's description (`0x52f929`-`0x52f938`).
#[test]
fn an_aura_with_no_text_draws_its_name_alone() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(
        2457,
        SpellTooltipView {
            name: "Battle Stance".into(),
            description: "A balanced combat stance.".into(),
            ..Default::default()
        },
    );
    s.set_aura_tooltip(aura_key(2457), aura_view("Battle Stance", ""));
    s.set_player_auras(vec![AuraState {
        spell_id: 2457,
        name: Some("Battle Stance".into()),
        helpful: true,
        until_cancelled: true,
        ..Default::default()
    }]);
    s.run(
        r#"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetPlayerBuff(0)
        assert(tt:NumLines() == 1, "the name alone, got " .. tt:NumLines())
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// A position inside the 48 records but past the occupied ones is an empty record: the builder
/// clears and leaves no line (`0x52f8ae`), and no time line follows, where the reference's rides
/// the record's stale `+0xc` (the `Deviation:` at `SetPlayerBuff`).
#[test]
fn an_empty_buff_record_shows_nothing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_aura_tooltip(
        aura_key(1126),
        aura_view("Mark of the Wild", "Armor increased by 25."),
    );
    s.set_player_auras(vec![AuraState {
        spell_id: 1126,
        name: Some("Mark of the Wild".into()),
        helpful: true,
        until_cancelled: true,
        ..Default::default()
    }]);
    s.run(
        r#"
        SPELL_TIME_REMAINING_SEC    = "%d second remaining"
        SPELL_TIME_REMAINING_SEC_P1 = "%d seconds remaining"
        local a = CreateFrame("Button", "BF1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetPlayerBuff(0)
        assert(tt:IsShown() and tt:NumLines() == 2, "the buff, got " .. tt:NumLines())
        tt:SetPlayerBuff(1)
        assert(not tt:IsShown() and tt:NumLines() == 0, "cleared, got " .. tt:NumLines())
        tt:SetPlayerBuff(0)
        tt:SetPlayerBuff(47)
        assert(not tt:IsShown() and tt:NumLines() == 0, "the last record")
    "#,
    )
    .unwrap();
    assert!(
        s.take_aura_tooltip_asks().is_empty(),
        "spell 0 asks for nothing"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// `SetAction` delegates by payload kind: a spell slot renders the spell view, and a spell the
/// store lacks shows no plate and records the ask.
#[test]
fn action_hover_delegates_by_kind() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(133, fireball());
    s.set_action(
        24,
        Some(ActionSlot {
            kind: 0x00, // SPELL
            action: 133,
            ..Default::default()
        }),
    );
    s.set_action(
        25,
        Some(ActionSlot {
            kind: 0x00,
            action: 5143, // not in the store: the ask channel fires
            ..Default::default()
        }),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "AB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetAction(24)
        assert(TTTextLeft1:GetText() == "Fireball", "spell slot renders the spell view")
        assert(TTTextRight1:GetText() == "Rank 1", "SetAction shows the rank column")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetAction(25)
        assert(tt:NumLines() == 0 and not tt:IsShown(), "a total miss shows no empty plate")
    "#,
    )
    .unwrap();
    assert_eq!(s.take_spell_tooltip_asks(), vec![5143], "the miss asked");
    assert!(s.take_errors().is_empty());
}

/// A macro slot's hover is the reference's `0x52b040`: one line, the macro's name in the normal
/// colour (`0xc0cf60` holds `0xffffffff`, white); a slot whose macro is gone shows no plate.
#[test]
fn action_hover_on_a_macro_slot_shows_its_name_in_white() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_macros(MacroState {
        account: vec![MacroView {
            name: "spawn".into(),
            texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
            body: ".spawn 16032".into(),
            local_only: false,
        }],
        character: Vec::new(),
    });
    s.set_action(
        24,
        Some(ActionSlot {
            kind: 0x40, // MACRO
            action: 1,  // macro index 1
            ..Default::default()
        }),
    );
    s.set_action(
        25,
        Some(ActionSlot {
            kind: 0x40,
            action: 7, // no such macro
            ..Default::default()
        }),
    );
    // 255 read back times the float at `0x81c6b4`; `FontString:GetTextColor` rounds green and
    // blue to `f32`s (`0x79d958`).
    s.lua()
        .globals()
        .set("ONE", crate::color::channel(255))
        .unwrap();
    s.lua()
        .globals()
        .set("ONE32", f64::from(crate::color::channel(255) as f32))
        .unwrap();
    s.run(
        r#"
        local a = CreateFrame("Button", "AB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetAction(24)
        assert(tt:IsShown(), "a macro slot shows a plate")
        assert(tt:NumLines() == 1, "exactly one line, got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "spawn", "the macro's name")
        local r, g, b = TTTextLeft1:GetTextColor()
        assert(r == ONE and g == ONE32 and b == ONE32, "white — the normal line colour, not the spell title gold")
        assert(not TTTextRight1:IsShown(), "no right column")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetAction(25)
        assert(tt:NumLines() == 0 and not tt:IsShown(), "a missing macro shows no plate")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The tooltip's left-column texts in order.
fn left_lines(s: &mut UiScript) -> Vec<String> {
    let n: i64 = s.eval("return TT:NumLines()").unwrap();
    (1..=n)
        .map(|i| {
            s.eval::<String>(&format!(
                "return getglobal('TTTextLeft{i}'):GetText() or ''"
            ))
            .unwrap()
        })
        .collect()
}

/// One trainer service fixture with an explicit tooltip subject.
fn trainer_service(spell_id: u32, name: &str, tooltip: TrainerTooltip) -> TrainerService {
    TrainerService {
        spell_id,
        tooltip,
        name: Some(name.into()),
        texture: Some("Interface\\Icons\\Temp".into()),
        category: TrainerServiceCategory::Available,
        group_key: 26,
        group_name: "Arms".into(),
        ..Default::default()
    }
}

/// `SetTrainerService` renders no line of its own: it routes to the item or spell builder the app
/// picked (`altCaster` drops the reagents), and its index is a visible row, so row 1 is a header.
#[test]
fn set_trainer_service_selects_the_builder_and_never_renders_its_own_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        2847,
        ItemTemplateView {
            name: "Copper Shortsword".into(),
            quality: 1,
            class: 2,
            subclass: 7,
            // The app resolves the type cell's word from ItemSubClass.dbc; a view without one
            // prints no type cell.
            sub_class_display: Some("Sword".into()),
            ..Default::default()
        },
    );
    let heroic_strike = SpellTooltipView {
        name: "Heroic Strike".into(),
        cost: Some("15 Rage".into()),
        reagents: Some("Reagents: Linen Cloth".into()),
        description: "A strong attack.".into(),
        ..Default::default()
    };
    s.set_spell_tooltip(200, heroic_strike.clone());
    // altCaster is the builder's unit selector, which reads the pet's view.
    s.set_pet_spell_tooltip(200, heroic_strike);
    s.set_trainer(Some(TrainerState {
        services: vec![
            trainer_service(2756, "Copper Shortsword", TrainerTooltip::Item(2847)),
            trainer_service(
                100,
                "Heroic Strike",
                TrainerTooltip::Spell {
                    spell_id: 200,
                    alt_caster: false,
                },
            ),
            trainer_service(
                101,
                "Train Growl",
                TrainerTooltip::Spell {
                    spell_id: 200,
                    alt_caster: true,
                },
            ),
        ],
        ..Default::default()
    }));

    s.run(
        r#"
        local a = CreateFrame("Button", "AB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")

        -- Row 1 is the "Arms" header: a no-op, not a shifted hit onto a neighbouring service.
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetTrainerService(1)
        assert(TT:NumLines() == 0, "a header row renders nothing")

        -- Row 2: the recipe -> the ITEM builder, full item tooltip (not the spell's name).
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetTrainerService(2)
        assert(TTTextLeft1:GetText() == "Copper Shortsword", "the item arm renders the item")
        assert(TT:NumLines() > 1, "the ITEM builder, not a one-line stub")
    "#,
    )
    .unwrap();

    // Row 3: the class-trainer service goes to the spell builder on the taught spell.
    s.run(
        r#"
        TT:SetOwner(AB1, "ANCHOR_RIGHT")
        TT:SetTrainerService(3)
    "#,
    )
    .unwrap();
    let texts = left_lines(&mut s);
    assert_eq!(
        texts[0], "Heroic Strike",
        "the spell arm hops to the taught"
    );
    assert!(
        texts.iter().any(|t| t == "Reagents: Linen Cloth"),
        "altCaster clear -> the reagents block renders: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "A strong attack."),
        "the gold description is part of the shared builder's law: {texts:?}"
    );

    // Row 4: the same view with altCaster set loses only the reagents block.
    s.run(
        r#"
        TT:SetOwner(AB1, "ANCHOR_RIGHT")
        TT:SetTrainerService(4)
    "#,
    )
    .unwrap();
    let texts = left_lines(&mut s);
    assert!(
        !texts.iter().any(|t| t == "Reagents: Linen Cloth"),
        "altCaster gates the reagents block: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "A strong attack."),
        "and gates ONLY that block: {texts:?}"
    );
    assert!(s.take_errors().is_empty());
}

/// `SetCraftSpell` is a selector like `SetTrainerService`: a rod recipe hovers the rod's item
/// tooltip while its row's icon is the spell's, which is the reference's shape, not a bug.
#[test]
fn set_craft_spell_selects_the_builder_like_the_trainer_hover_does() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        6218,
        ItemTemplateView {
            name: "Runed Copper Rod".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.set_spell_tooltip(
        7420,
        SpellTooltipView {
            name: "Enchant Bracer - Minor Health".into(),
            cast_time: Some("Instant cast".into()),
            description: "Permanently enchant bracers.".into(),
            ..Default::default()
        },
    );
    let recipe = |spell_id: u32, name: &str, tooltip: CraftTooltip| CraftRecipe {
        spell_id,
        tooltip,
        name: name.into(),
        sub_name: String::new(),
        difficulty: TradeSkillDifficulty::Optimal,
        num_available: 1,
        icon: Some("Interface\\Icons\\Spell_Holy_Heal".into()),
        description: Some("the OLD two-line stub's second line".into()),
        needs_item_target: false,
        reagents: vec![],
        tools: vec![],
        spell_level: 0,
        skill_line: None,
    };
    s.set_craft(Some(CraftState {
        name: "Enchanting".into(),
        rank: 100,
        max_rank: 150,
        craft_type: 3,
        recipes: vec![
            recipe(
                7420,
                "Enchant Bracer - Minor Health",
                CraftTooltip::Spell(7420),
            ),
            recipe(7421, "Runed Copper Rod", CraftTooltip::Item(6218)),
        ],
    }));

    s.run(
        r#"
        local a = CreateFrame("Button", "AB2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(a, "ANCHOR_RIGHT")
        TT:SetCraftSpell(1)
    "#,
    )
    .unwrap();
    let texts = left_lines(&mut s);
    assert_eq!(texts[0], "Enchant Bracer - Minor Health");
    assert!(
        texts.iter().any(|t| t == "Instant cast"),
        "the SHARED spell builder's law, not a two-line stub: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|t| t == "the OLD two-line stub's second line"),
        "the row's own `description` field is no longer the tooltip's source: {texts:?}"
    );

    s.run(
        r#"
        TT:SetOwner(AB2, "ANCHOR_RIGHT")
        TT:SetCraftSpell(2)
    "#,
    )
    .unwrap();
    let texts = left_lines(&mut s);
    assert_eq!(
        texts[0], "Runed Copper Rod",
        "a CREATE_ITEM recipe hovers the ITEM, though its icon is the spell's: {texts:?}"
    );
    assert!(s.take_errors().is_empty());
}

/// `0x532b00` skips the remaining line on `untilCancelled` (`0x532bda`, `0x532bdf`), never on
/// whether the aura has a duration yet.
#[test]
fn the_remaining_line_is_gated_on_until_cancelled_not_on_the_duration() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_aura_tooltip(
        aura_key(1459),
        aura_view("Arcane Intellect", "Intellect increased by 2."),
    );
    s.tick(10.0);
    s.run(
        r#"
        SPELL_TIME_REMAINING_MIN    = "<%d m>"
        SPELL_TIME_REMAINING_MIN_P1 = "<%d mm>"
        SPELL_TIME_REMAINING_SEC    = "<%d s>"
        SPELL_TIME_REMAINING_SEC_P1 = "<%d ss>"
        BENILLA_ANCHOR = CreateFrame("Button", "BF2")
        BENILLA_ANCHOR:SetPoint("CENTER", 0, 0); BENILLA_ANCHOR:SetWidth(10); BENILLA_ANCHOR:SetHeight(10)
        BENILLA_TIP = CreateFrame("GameTooltip", "TT2", nil, "TestTooltipTemplate")
    "#,
    )
    .unwrap();
    let hover = r#"
        BENILLA_TIP:SetOwner(BENILLA_ANCHOR, "ANCHOR_RIGHT")
        BENILLA_TIP:SetPlayerBuff(0)
        BENILLA_LINES = BENILLA_TIP:NumLines()
        BENILLA_LAST = BENILLA_LINES == 3 and TT2TextLeft3:GetText() or ""
    "#;
    let mut show = |aura: AuraState| {
        s.set_player_auras(vec![aura]);
        s.run(hover).unwrap();
        (
            s.eval::<i64>("return BENILLA_LINES").unwrap(),
            s.eval::<String>("return BENILLA_LAST").unwrap(),
        )
    };
    let base = AuraState {
        spell_id: 1459,
        name: Some("Arcane Intellect".into()),
        helpful: true,
        ..Default::default()
    };

    // 1 · Permanent, with a duration anyway, so only the flag can suppress the line.
    assert_eq!(
        show(AuraState {
            duration: 1800.0,
            expiration_time: 100.0,
            until_cancelled: true,
            ..base.clone()
        }),
        (2, String::new()),
        "untilCancelled suppresses the line even with a live duration on the record"
    );

    // 2 · Timed, before any duration packet: the reference shows the line, counting from zero.
    assert_eq!(
        show(AuraState {
            duration: 0.0,
            expiration_time: 0.0,
            until_cancelled: false,
            ..base.clone()
        }),
        (3, "<0 ss>".to_string()),
        "a timed aura's first frames still carry the line"
    );

    // 3 · Lapsed: the reference's truncating seconds arm reads 0.
    assert_eq!(
        show(AuraState {
            duration: 30.0,
            expiration_time: 9.6, // 0.4 s in the past at now = 10
            until_cancelled: false,
            ..base.clone()
        }),
        (3, "<0 ss>".to_string()),
        "a lapsed aura reads 0, it does not lose the line"
    );

    // Control: an ordinary live aura still counts down.
    assert_eq!(
        show(AuraState {
            duration: 1800.0,
            expiration_time: 100.0,
            until_cancelled: false,
            ..base
        }),
        (3, "<2 mm>".to_string()),
    );
    assert!(s.take_errors().is_empty());
}

/// `GetRewardSpell()` and `GetQuestLogRewardSpell()` answer `texture, name, isTradeskillSpell`,
/// three nils when the quest teaches nothing, and `SetQuestRewardSpell()` and
/// `SetQuestLogRewardSpell()` render that spell.
#[test]
fn quest_reward_spell_getters_and_hovers() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_spell_tooltip(133, fireball());
    s.run(
        r#"
        local a = CreateFrame("Button", "Q1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        "#,
    )
    .unwrap();

    s.set_quest(Some(QuestState::default()));
    assert_eq!(
        s.arity("GetRewardSpell()").unwrap(),
        3,
        "three values on the empty path"
    );
    assert!(s
        .eval::<bool>(
            "local t, n, ts = GetRewardSpell() return t == nil and n == nil and ts == nil"
        )
        .unwrap());
    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestRewardSpell()"#)
        .unwrap();
    assert_eq!(s.eval::<i64>("return TT:NumLines()").unwrap(), 0);

    let reward = QuestRewardSpell {
        spell_id: 133,
        name: Some("Fireball".into()),
        texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
        tradeskill: false,
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        reward_spell: Some(reward.clone()),
        ..QuestState::default()
    }));
    assert_eq!(
        s.eval::<(String, String, Option<i64>)>("return GetRewardSpell()")
            .unwrap(),
        (
            "Interface\\Icons\\Spell_Fire_FlameBolt".to_string(),
            "Fireball".to_string(),
            None
        )
    );
    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestRewardSpell()"#)
        .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<String>("return TTTextLeft1:GetText()").unwrap(),
        "Fireball"
    );

    let mut record = crate::script::quest_log::fixture::record("Q", 1, 12, "Elwynn Forest");
    record.reward_spell = Some(reward);
    crate::script::quest_log::fixture::seat(
        &mut s,
        &[(7, record, crate::script::quest_log::fixture::slot(7, 0, 0))],
    );
    // The spell is the selected quest's, so both readers go through the selection: row 2.
    s.run("SelectQuestLogEntry(2)").unwrap();
    assert_eq!(
        s.eval::<(String, String, Option<i64>)>("return GetQuestLogRewardSpell()")
            .unwrap()
            .1,
        "Fireball"
    );
    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestLogRewardSpell()"#)
        .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<String>("return TTTextLeft1:GetText()").unwrap(),
        "Fireball"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A craft recipe row whose detail icon hovers `tooltip`.
fn craft_recipe(spell_id: u32, name: &str, tooltip: CraftTooltip) -> CraftRecipe {
    CraftRecipe {
        spell_id,
        tooltip,
        name: name.into(),
        sub_name: String::new(),
        difficulty: TradeSkillDifficulty::Optimal,
        num_available: 1,
        icon: None,
        description: None,
        needs_item_target: false,
        reagents: vec![],
        tools: vec![],
        spell_level: 0,
        skill_line: None,
    }
}

/// Firebolt (3110) on a pet-bar slot.
fn firebolt_slot() -> PetActionView {
    PetActionView {
        name: Some("Firebolt".into()),
        spell_id: Some(3110),
        ..Default::default()
    }
}

fn firebolt() -> SpellTooltipView {
    SpellTooltipView {
        name: "Firebolt".into(),
        rank: Some("Rank 1".into()),
        cost: Some("10 Mana".into()),
        range: Some("30 yd range".into()),
        cast_time: Some("1 sec cast".into()),
        description: "Deals 7 to 10 Fire damage to a target.".into(),
        ..Default::default()
    }
}

/// Pet-bar tokens and craft item subjects name no spell, so they are left out; the auras are the
/// aura builder's subjects, by spell, level and stack.
#[test]
fn the_spell_subjects_are_what_the_setters_read_from_the_vm() {
    let mut s = script();
    s.set_pet_actions(
        true,
        true,
        true,
        vec![
            PetActionView {
                name: Some("PET_ACTION_ATTACK".into()),
                is_token: true,
                ..Default::default()
            },
            firebolt_slot(),
        ],
    );
    s.set_pet_book(PetBookState {
        token: Some("DEMON".into()),
        slots: vec![SpellSlotView {
            spell_id: 6307,
            name: "Blood Pact".into(),
            ..Default::default()
        }],
    });
    let reward = |spell_id| {
        Some(QuestRewardSpell {
            spell_id,
            ..Default::default()
        })
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        reward_spell: reward(133),
        ..QuestState::default()
    }));
    let mut record = crate::script::quest_log::fixture::record("Q", 1, 12, "Elwynn Forest");
    record.reward_spell = reward(116);
    crate::script::quest_log::fixture::seat(
        &mut s,
        &[(7, record, crate::script::quest_log::fixture::slot(7, 0, 0))],
    );
    s.set_craft(Some(CraftState {
        name: "Beast Training".into(),
        rank: 0,
        max_rank: 0,
        craft_type: 1,
        recipes: vec![
            craft_recipe(24599, "Bite", CraftTooltip::Spell(17253)),
            craft_recipe(7421, "Runed Copper Rod", CraftTooltip::Item(6218)),
        ],
    }));
    let aura = |spell_id, level, count| AuraState {
        spell_id,
        level,
        count,
        ..Default::default()
    };
    s.set_player_auras(vec![aura(1459, 60, 1), aura(11597, 30, 5)]);
    // The pet's, the target-of-target's and party1's lists, by guid; party1's from its roster
    // record, level 0 and stack 1.
    s.set_unit_auras(0xF140_0000_0000_0077, Some(vec![aura(172, 20, 1)].into()));
    s.set_unit_auras(0x21, Some(vec![aura(589, 60, 1)].into()));
    s.set_unit_auras(0x22, Some(vec![aura(8921, 0, 1)].into()));
    s.set_tracking(Some(TrackingState {
        spell_id: 2580,
        name: Some("Find Minerals".into()),
        icon: None,
        cancelable: true,
    }));

    let mut subjects = s.spell_tooltip_subjects();
    subjects.sort_unstable();
    assert_eq!(
        subjects,
        vec![116, 133, 17253],
        "the auras are the aura builder's"
    );
    // Each aura by the builder's arguments: its level and stack, the tracking spell's 1 and 1.
    let mut auras: Vec<_> = s
        .aura_tooltip_subjects()
        .into_iter()
        .map(|k| (k.spell_id, k.level, k.stack))
        .collect();
    auras.sort_unstable();
    assert_eq!(
        auras,
        vec![
            (172, 20, 1),
            (589, 60, 1),
            (1459, 60, 1),
            (2580, 1, 1),
            (8921, 0, 1),
            (11597, 30, 5)
        ]
    );
    // The pet's bar and book are read through the builder's unit selector: the pet's views.
    let mut pet_subjects = s.pet_spell_tooltip_subjects();
    pet_subjects.sort_unstable();
    assert_eq!(pet_subjects, vec![3110, 6307]);
}

/// The builder's unit selector picks the view built against the pet: `SetPetAction` passes it
/// (`0x532888`, `0x5328d0`), `SetSpell` on the pet book (`0x532e23`) and a pet-learn
/// `SetTrainerService` (`0x533a7f`); the player's book, a class service and a craft pass 0.
#[test]
fn the_unit_selector_setters_read_the_pet_view() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let view = |description: &str| SpellTooltipView {
        name: "Sacrifice".into(),
        description: description.into(),
        ..Default::default()
    };
    s.set_spell_tooltip(7812, view("absorb 305 damage"));
    s.set_pet_spell_tooltip(7812, view("absorb 318 damage"));
    s.set_pet_actions(
        true,
        true,
        true,
        vec![PetActionView {
            name: Some("Sacrifice".into()),
            spell_id: Some(7812),
            ..Default::default()
        }],
    );
    let slot = SpellSlotView {
        spell_id: 7812,
        name: "Sacrifice".into(),
        ..Default::default()
    };
    s.set_pet_book(PetBookState {
        token: Some("DEMON".into()),
        slots: vec![slot.clone()],
    });
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![slot],
    });
    let service = |alt_caster| {
        trainer_service(
            100,
            "Sacrifice",
            TrainerTooltip::Spell {
                spell_id: 7812,
                alt_caster,
            },
        )
    };
    s.set_trainer(Some(TrainerState {
        services: vec![service(true), service(false)],
        ..Default::default()
    }));
    s.set_craft(Some(CraftState {
        name: "Beast Training".into(),
        rank: 0,
        max_rank: 0,
        craft_type: 1,
        recipes: vec![craft_recipe(24599, "Sacrifice", CraftTooltip::Spell(7812))],
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "PB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        local function desc(set)
            TT:SetOwner(PB1, "ANCHOR_RIGHT"); set(); return TTTextLeft2:GetText()
        end
        PET_BAR = desc(function() TT:SetPetAction(1) end)
        PET_BOOK = desc(function() TT:SetSpell(1, "pet") end)
        BOOK = desc(function() TT:SetSpell(1, "spell") end)
        -- Row 1 is the group header.
        PET_SERVICE = desc(function() TT:SetTrainerService(2) end)
        SERVICE = desc(function() TT:SetTrainerService(3) end)
        CRAFT = desc(function() TT:SetCraftSpell(1) end)
    "#,
    )
    .unwrap();
    let got = |name: &str| s.eval::<String>(&format!("return {name}")).unwrap();
    assert_eq!(got("PET_BAR"), "absorb 318 damage");
    assert_eq!(got("PET_BOOK"), "absorb 318 damage");
    assert_eq!(got("PET_SERVICE"), "absorb 318 damage");
    assert_eq!(got("BOOK"), "absorb 305 damage");
    assert_eq!(got("SERVICE"), "absorb 305 damage");
    assert_eq!(got("CRAFT"), "absorb 305 damage");
    assert!(s.take_spell_tooltip_asks().is_empty());
    assert!(s.take_pet_spell_tooltip_asks().is_empty());
    assert!(s.take_errors().is_empty());
}

/// A pet view's miss asks the pet's store, and only the pet view's answer re-renders it.
#[test]
fn a_pet_view_miss_waits_on_the_pet_store() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_pet_actions(true, true, true, vec![firebolt_slot()]);
    s.run(
        r#"
        local a = CreateFrame("Button", "PB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(PB1, "ANCHOR_RIGHT")
        TT:SetPetAction(1)
    "#,
    )
    .unwrap();
    assert!(s.take_spell_tooltip_asks().is_empty());
    assert_eq!(s.take_pet_spell_tooltip_asks(), vec![3110]);
    // The player's view of the same id is not the one the selector renders.
    s.set_spell_tooltip(3110, firebolt());
    assert_eq!(left_lines(&mut s), vec!["Firebolt"]);
    s.set_pet_spell_tooltip(3110, firebolt());
    assert_eq!(
        left_lines(&mut s),
        vec![
            "Firebolt",
            "10 Mana",
            "1 sec cast",
            "Deals 7 to 10 Fire damage to a target."
        ]
    );
    assert!(s.take_errors().is_empty());
}

/// No second hover and no second `OnTooltipCleared`: the reference builds the tooltip at the call.
#[test]
fn a_missed_view_re_renders_the_tooltip_when_the_app_answers() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_pet_actions(true, true, true, vec![firebolt_slot()]);
    s.run(
        r#"
        local a = CreateFrame("Button", "PB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(PB1, "ANCHOR_RIGHT")
        CLEARED = 0
        TT:SetScript("OnTooltipCleared", function() CLEARED = CLEARED + 1 end)
        TT:SetPetAction(1)
    "#,
    )
    .unwrap();
    assert_eq!(left_lines(&mut s), vec!["Firebolt"]);
    assert_eq!(s.take_pet_spell_tooltip_asks(), vec![3110]);

    s.set_pet_spell_tooltip(3110, firebolt());
    assert_eq!(
        left_lines(&mut s),
        vec![
            "Firebolt",
            "10 Mana",
            "1 sec cast",
            "Deals 7 to 10 Fire damage to a target."
        ]
    );
    assert!(s.eval::<bool>("return TT:IsShown() == 1").unwrap());
    assert!(
        s.take_pet_spell_tooltip_asks().is_empty(),
        "the re-render found the view"
    );
    assert_eq!(
        s.eval::<i64>("return CLEARED").unwrap(),
        1,
        "one setter call, one OnTooltipCleared"
    );
    assert!(s.take_errors().is_empty());
}

/// Beast Training's icon has no fallback name, so its miss is a hidden plate.
#[test]
fn a_hidden_miss_shows_and_an_enchant_link_fills_when_answered() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_craft(Some(CraftState {
        name: "Beast Training".into(),
        rank: 0,
        max_rank: 0,
        craft_type: 1,
        recipes: vec![craft_recipe(24599, "Bite", CraftTooltip::Spell(17253))],
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "CI"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        CreateFrame("GameTooltip", "REF", nil, "TestTooltipTemplate")
        TT:SetOwner(CI, "ANCHOR_RIGHT")
        TT:SetCraftSpell(1)
        REF:SetOwner(CI, "ANCHOR_PRESERVE")
        REF:SetHyperlink("|cffffd000|Henchant:20034|h[Enchant Weapon - Crusader]|h|r")
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return TT:NumLines()").unwrap(), 0);
    assert!(!s.eval::<bool>("return TT:IsShown() == 1").unwrap());
    assert_eq!(
        s.eval::<String>("return REFTextLeft1:GetText()").unwrap(),
        "Enchant Weapon - Crusader"
    );
    assert_eq!(s.eval::<i64>("return REF:NumLines()").unwrap(), 1);

    s.set_spell_tooltip(
        17253,
        SpellTooltipView {
            name: "Bite".into(),
            description: "Bite the enemy.".into(),
            ..Default::default()
        },
    );
    s.set_spell_tooltip(
        20034,
        SpellTooltipView {
            name: "Enchant Weapon - Crusader".into(),
            cast_time: Some("5 sec cast".into()),
            description: "Permanently enchant a melee weapon.".into(),
            ..Default::default()
        },
    );
    assert!(s.eval::<bool>("return TT:IsShown() == 1").unwrap());
    assert_eq!(
        s.eval::<(String, String)>("return TTTextLeft1:GetText(), TTTextLeft2:GetText()")
            .unwrap(),
        ("Bite".to_string(), "Bite the enemy.".to_string())
    );
    assert_eq!(
        s.eval::<(i64, String)>("return REF:NumLines(), REFTextLeft3:GetText()")
            .unwrap(),
        (3, "Permanently enchant a melee weapon.".to_string())
    );
    assert!(s.take_errors().is_empty());
}

/// A new hover, a hide, an added line or a running fade leaves the tooltip as it is.
#[test]
fn new_content_or_an_added_line_ends_the_wait() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_pet_actions(true, true, true, vec![firebolt_slot()]);
    s.run(
        r#"
        local a = CreateFrame("Button", "PB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        CreateFrame("GameTooltip", "HID", nil, "TestTooltipTemplate")
        CreateFrame("GameTooltip", "ADD", nil, "TestTooltipTemplate")
        TT:SetOwner(PB1, "ANCHOR_RIGHT")
        TT:SetPetAction(1)
        TT:SetOwner(PB1, "ANCHOR_RIGHT")
        TT:SetText("Attack")
        HID:SetOwner(PB1, "ANCHOR_RIGHT")
        HID:SetPetAction(1)
        HID:Hide()
        ADD:SetOwner(PB1, "ANCHOR_RIGHT")
        ADD:SetPetAction(1)
        ADD:AddLine("an addon's line")
        CreateFrame("GameTooltip", "FAD", nil, "TestTooltipTemplate")
        FAD:SetOwner(PB1, "ANCHOR_RIGHT")
        FAD:SetPetAction(1)
        FAD:FadeOut()
    "#,
    )
    .unwrap();
    s.set_pet_spell_tooltip(3110, firebolt());
    assert_eq!(left_lines(&mut s), vec!["Attack"]);
    assert_eq!(s.eval::<i64>("return HID:NumLines()").unwrap(), 0);
    assert!(!s.eval::<bool>("return HID:IsShown() == 1").unwrap());
    assert_eq!(
        s.eval::<(i64, String)>("return ADD:NumLines(), ADDTextLeft2:GetText()")
            .unwrap(),
        (2, "an addon's line".to_string())
    );
    assert_eq!(s.eval::<i64>("return FAD:NumLines()").unwrap(), 1);
    assert!(s.take_errors().is_empty());
}

/// What Lua wrote after the miss is kept: a line rewritten in place, by `AppendText` or a cell's
/// `SetTextColor`, or new content that reads the same as the miss's line.
#[test]
fn a_line_rewritten_in_place_ends_the_wait() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_pet_actions(true, true, true, vec![firebolt_slot()]);
    s.run(
        r#"
        local a = CreateFrame("Button", "PB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "APP", nil, "TestTooltipTemplate")
        CreateFrame("GameTooltip", "COL", nil, "TestTooltipTemplate")
        APP:SetOwner(PB1, "ANCHOR_RIGHT")
        APP:SetPetAction(1)
        APP:AppendText(" (Pet)")
        COL:SetOwner(PB1, "ANCHOR_RIGHT")
        COL:SetPetAction(1)
        COLTextLeft1:SetTextColor(1, 0, 0)
        CreateFrame("GameTooltip", "SAM", nil, "TestTooltipTemplate")
        SAM:SetOwner(PB1, "ANCHOR_RIGHT")
        SAM:SetPetAction(1)
        SAM:SetOwner(PB1, "ANCHOR_RIGHT")
        SAM:SetText("Firebolt", 1, 1, 1)
    "#,
    )
    .unwrap();
    s.set_pet_spell_tooltip(3110, firebolt());
    assert_eq!(
        s.eval::<(i64, String)>("return APP:NumLines(), APPTextLeft1:GetText()")
            .unwrap(),
        (1, "Firebolt (Pet)".to_string())
    );
    assert_eq!(
        s.eval::<(i64, f32, f32)>(
            "local r, g = COLTextLeft1:GetTextColor(); return COL:NumLines(), r, g"
        )
        .unwrap(),
        (1, crate::color::quant::read32(1.0), 0.0)
    );
    assert_eq!(s.eval::<i64>("return SAM:NumLines()").unwrap(), 1);
    assert!(s.take_errors().is_empty());
}

/// A re-render's `OnShow` can move another waiting tooltip onto a new spell.
#[test]
fn a_wait_replaced_by_an_earlier_re_render_is_not_replayed() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_craft(Some(CraftState {
        name: "Beast Training".into(),
        rank: 0,
        max_rank: 0,
        craft_type: 1,
        recipes: vec![craft_recipe(24599, "Firebolt", CraftTooltip::Spell(3110))],
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "PB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "A", nil, "TestTooltipTemplate")
        CreateFrame("GameTooltip", "B", nil, "TestTooltipTemplate")
        A:SetOwner(PB1, "ANCHOR_RIGHT")
        A:SetCraftSpell(1)
        B:SetOwner(PB1, "ANCHOR_RIGHT")
        B:SetCraftSpell(1)
        A:SetScript("OnShow", function()
            B:SetOwner(PB1, "ANCHOR_RIGHT")
            B:SetHyperlink("|cffffd000|Henchant:20034|h[Enchant Weapon - Crusader]|h|r")
        end)
    "#,
    )
    .unwrap();
    s.set_spell_tooltip(3110, firebolt());
    assert_eq!(
        s.eval::<(i64, String)>("return B:NumLines(), BTextLeft1:GetText()")
            .unwrap(),
        (1, "Enchant Weapon - Crusader".to_string())
    );
    assert_eq!(
        s.eval::<String>("return ATextLeft1:GetText()").unwrap(),
        "Firebolt"
    );
    assert!(s.take_errors().is_empty());
}

/// Distinct stand-ins for the requirement keys and their compact `_NOSPACE` forms.
fn seed_requirement_strings(s: &UiScript) {
    s.run(
        r#"
        SPELL_EQUIPPED_ITEM = "[ITEM %s]"; SPELL_EQUIPPED_ITEM_NOSPACE = "[ITEM_NS %s]"
        SPELL_REQUIRED_FORM = "[FORM %s]"; SPELL_REQUIRED_FORM_NOSPACE = "[FORM_NS %s]"
        SPELL_TOTEMS = "[TOOLS] "; SPELL_REAGENTS = "[REAGENTS] "; SPELL_USE_ALL_MANA = "[ALL 100% MANA]"
        local a = CreateFrame("Button", "SBC"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        TT = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        function Lines()
            local out = {}
            for i = 1, TT:NumLines() do
                local l, r = getglobal("TTTextLeft" .. i):GetText(), getglobal("TTTextRight" .. i)
                local right = r:IsShown() and r:GetText() or nil
                table.insert(out, (l or "") .. (right and (" | " .. right) or ""))
            end
            return table.concat(out, "\n")
        end
    "#,
    )
    .unwrap();
}

/// Every line a full build can hold, the tools met, the item unmet, the form met, a reagent short.
fn holy_light() -> SpellTooltipView {
    SpellTooltipView {
        name: "Lay on Hands".into(),
        rank: Some("Rank 2".into()),
        cost: Some("0 Mana".into()),
        range: Some("40 yd range".into()),
        cast_time: Some("Instant cast".into()),
        cooldown: Some("60 min cooldown".into()),
        totems: Some(TooltipText::copied("SPELL_TOTEMS").then("Fire Totem")),
        totems_met: true,
        requires_item: Some("Wands".into()),
        item_met: false,
        requires_form: Some("Bear Form".into()),
        form_met: true,
        reagents: Some(TooltipText::copied("SPELL_REAGENTS").then("|cffff2020Rune|r")),
        reagents_met: false,
        chance: Some("5.00% chance to crit".into()),
        use_all: Some(TooltipText::copied("SPELL_USE_ALL_MANA")),
        description: "Heals a friendly target.".into(),
        ..Default::default()
    }
}

/// The full builder's order: tools after the cast line, before the item requirement
/// (`0x52ed43`-`0x52eea2`), and the drain-all line after the chance line, before the description
/// (`0x52f697`-`0x52f6e3`), copied with its `%` sign.
#[test]
fn the_tools_and_drain_all_lines_sit_in_the_builders_order() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    seed_requirement_strings(&s);
    s.set_spell_tooltip(633, holy_light());
    s.set_action(
        1,
        Some(ActionSlot {
            kind: 0,
            action: 633,
            ..Default::default()
        }),
    );
    let lines: String = s
        .eval(r#"TT:SetOwner(SBC, "ANCHOR_RIGHT"); TT:SetAction(1); return Lines()"#)
        .unwrap();
    assert_eq!(
        lines,
        "Lay on Hands | Rank 2\n0 Mana | 40 yd range\nInstant cast | 60 min cooldown\n\
         [TOOLS] Fire Totem\n[ITEM Wands]\n[FORM Bear Form]\n[REAGENTS] |cffff2020Rune|r\n\
         5.00% chance to crit\n[ALL 100% MANA]\nHeals a friendly target."
    );
    assert!(s.take_errors().is_empty());
}

/// `UberTooltips` 0 makes `SetAction` and `SetShapeshift` compact (`0x532622`-`0x532632`): the
/// name with its rank, an unmet requirement in its `_NOSPACE` form, a short reagent, and nothing
/// after the cooldown remaining (`0x52f589`); met tools and forms drop. Other setters stay full.
#[test]
fn enhanced_tooltips_off_shortens_the_action_and_stance_tooltips() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    seed_requirement_strings(&s);
    s.set_spell_tooltip(633, holy_light());
    s.set_action(
        1,
        Some(ActionSlot {
            kind: 0,
            action: 633,
            ..Default::default()
        }),
    );
    s.set_shapeshift_forms(vec![ShapeshiftFormView {
        spell_id: 633,
        name: "Lay on Hands".into(),
        ..Default::default()
    }]);
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 633,
            name: "Lay on Hands".into(),
            ..Default::default()
        }],
    });
    s.run(r#"SetCVar("UberTooltips", "0")"#).unwrap();
    let compact = "Lay on Hands | Rank 2\n[ITEM_NS Wands]\n[REAGENTS] |cffff2020Rune|r";
    for call in ["TT:SetAction(1)", "TT:SetShapeshift(1)"] {
        let lines: String = s
            .eval(&format!(
                r#"TT:SetOwner(SBC, "ANCHOR_RIGHT"); {call}; return Lines()"#
            ))
            .unwrap();
        assert_eq!(lines, compact, "{call}");
    }
    let book: String = s
        .eval(r#"TT:SetOwner(SBC, "ANCHOR_RIGHT"); TT:SetSpell(1, "spell"); return Lines()"#)
        .unwrap();
    assert!(book.ends_with("Heals a friendly target."), "{book}");
    assert!(s.take_errors().is_empty());
}

/// The reputation line sits after the form line and before the reagents (`0x52f2ae`-`0x52f38e`):
/// `ITEM_REQ_REPUTATION` of the faction and the rank's label, gendered for a female player when the
/// `_FEMALE` text is not empty (`0x612bf0`), white when met, else red; a compact build leaves a met
/// one out and keeps an unmet one whole.
#[test]
fn the_reputation_requirement_line_sits_after_the_form() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    seed_requirement_strings(&s);
    s.run(
        r#"ITEM_REQ_REPUTATION = "[REP %s - %s]"
           FACTION_STANDING_LABEL5 = "Friendly"; FACTION_STANDING_LABEL5_FEMALE = "Friendly (f)"
           FACTION_STANDING_LABEL6 = "Honored"; FACTION_STANDING_LABEL6_FEMALE = """#,
    )
    .unwrap();
    let rep = |rank, met, female| ReputationRequirement {
        faction: "Gadgetzan".into(),
        rank,
        met,
        female,
    };
    s.set_action(
        1,
        Some(ActionSlot {
            kind: 0,
            action: 633,
            ..Default::default()
        }),
    );
    let shown = |s: &mut UiScript, r: ReputationRequirement| {
        s.set_spell_tooltip(
            633,
            SpellTooltipView {
                requires_reputation: Some(r),
                ..holy_light()
            },
        );
        let lines: String = s
            .eval(r#"TT:SetOwner(SBC, "ANCHOR_RIGHT"); TT:SetAction(1); return Lines()"#)
            .unwrap();
        let rgb: (f32, f32, f32) = s
            .eval(
                r#"for i = 1, TT:NumLines() do
                       local l = getglobal("TTTextLeft" .. i)
                       if string.find(l:GetText() or "", "^%[REP") then return l:GetTextColor() end
                   end"#,
            )
            .unwrap_or((0.0, 0.0, 0.0));
        (lines, rgb)
    };
    let (lines, rgb) = shown(&mut s, rep(4, false, false));
    assert!(
        lines.contains("[FORM Bear Form]\n[REP Gadgetzan - Friendly]\n[REAGENTS]"),
        "{lines}"
    );
    assert!(
        (rgb.0 - 1.0).abs() < 1e-3 && rgb.1 < 0.2,
        "unmet is red: {rgb:?}"
    );
    let (lines, rgb) = shown(&mut s, rep(4, true, true));
    assert!(lines.contains("[REP Gadgetzan - Friendly (f)]"), "{lines}");
    assert!(rgb.1 > 0.99 && rgb.2 > 0.99, "met is white: {rgb:?}");
    let (lines, _) = shown(&mut s, rep(5, true, true));
    assert!(
        lines.contains("[REP Gadgetzan - Honored]"),
        "an empty _FEMALE: {lines}"
    );
    s.run(r#"SetCVar("UberTooltips", "0")"#).unwrap();
    let (lines, _) = shown(&mut s, rep(4, true, false));
    assert!(!lines.contains("[REP"), "compact drops a met one: {lines}");
    let (lines, _) = shown(&mut s, rep(4, false, false));
    assert!(lines.contains("[REP Gadgetzan - Friendly]"), "{lines}");
    assert!(s.take_errors().is_empty());
}

/// `SetShapeshift` and `SetPetAction` pass the rank flag (`0x532ab3`, `0x532886`), as `SetAction`
/// does; `SetSpell` does not.
#[test]
fn the_stance_and_pet_bars_show_the_rank() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    seed_requirement_strings(&s);
    let stealth = SpellTooltipView {
        name: "Stealth".into(),
        rank: Some("Rank 1".into()),
        ..Default::default()
    };
    s.set_spell_tooltip(1784, stealth.clone());
    s.set_pet_spell_tooltip(1784, stealth);
    s.set_shapeshift_forms(vec![ShapeshiftFormView {
        spell_id: 1784,
        name: "Stealth".into(),
        ..Default::default()
    }]);
    s.set_pet_actions(
        true,
        true,
        true,
        vec![PetActionView {
            name: Some("Stealth".into()),
            spell_id: Some(1784),
            ..Default::default()
        }],
    );
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 1784,
            name: "Stealth".into(),
            ..Default::default()
        }],
    });
    for (call, want) in [
        ("TT:SetShapeshift(1)", "Stealth | Rank 1"),
        ("TT:SetPetAction(1)", "Stealth | Rank 1"),
        ("TT:SetSpell(1, 'spell')", "Stealth"),
    ] {
        let lines: String = s
            .eval(&format!(
                r#"TT:SetOwner(SBC, "ANCHOR_RIGHT"); {call}; return Lines()"#
            ))
            .unwrap();
        assert_eq!(lines, want, "{call}");
    }
    assert!(s.take_errors().is_empty());
}
