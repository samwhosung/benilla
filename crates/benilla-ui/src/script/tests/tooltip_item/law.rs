//! Line order and the red requirement checks: level, class and reputation against player state,
//! and the slot and type cells' separate proficiency reds.

use std::collections::HashMap;

use super::{axe, lines_of, right_color, script};
use crate::script::*;

#[test]
fn item_line_law_and_red_requirements() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(871, axe());
    s.set_player_req_state(PlayerReqState {
        level: 25,
        class_id: 1, // Warrior: allowed
        race_id: 1,
        skills: HashMap::new(),
        ..Default::default()
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:BenillaSetItemById(871)
        assert(tt:IsShown(), "BenillaSetItemById shows")
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "Ravager",
            "[ITEM_BIND_ON_EQUIP]",
            "[INVTYPE_2HWEAPON]",
            // School 0 has no school word. The first damage line takes the plain key, later ones
            // `PLUS_`, and a slot with a school the `_WITH_SCHOOL` arm, the word inside it.
            "[DMG 68 - 103]",
            "[+DMGS 2 - 4 [SCHOOL5]]",
            "[DPS 25.3]",
            // Display order, not field order: `0x808e88` prints STR, AGI, STA, INT, SPI, HP, MANA.
            "[MOD_STRENGTH +9]",
            "[MOD_STAMINA +12]",
            "[RESIST_SINGLE +10 [SCHOOL5]]",
            "[DURABILITY 90/90]",
            "[CLASSES Warrior, Rogue]",
            "[MIN_LEVEL 37]",
            "[ONPROC] Ravager",
            "\"A wicked axe of the Scarlet Crusade.\"",
        ],
        "the verified line order (0276)"
    );
    assert_eq!(lines[0].1, [0.0, 0.439, 0.867, 1.0], "rare-blue name");
    let class_line = &lines[10];
    assert_eq!(
        class_line.1,
        [1.0, 1.0, 1.0, 1.0],
        "class list passes → white"
    );
    let level_line = &lines[11];
    assert_eq!(
        level_line.1,
        [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0],
        "level 25 < 37 → red"
    );
    let ty: String = s.eval("return TTTextRight3:GetText()").unwrap();
    assert_eq!(ty, "Axe");
    let speed: String = s.eval("return TTTextRight4:GetText()").unwrap();
    assert_eq!(speed, "Speed 3.50");
    assert!(s.take_errors().is_empty());
}

#[test]
fn red_lines_track_player_state() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(871, axe());
    s.set_player_req_state(PlayerReqState {
        level: 60,
        class_id: 8, // Mage: not in {Warrior, Rogue}
        race_id: 1,
        skills: HashMap::new(),
        ..Default::default()
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:BenillaSetItemById(871)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let find = |needle: &str| {
        lines
            .iter()
            .find(|(t, _)| t.starts_with(needle))
            .unwrap_or_else(|| panic!("no line starting {needle}"))
            .1
    };
    assert_eq!(find("[MIN_LEVEL"), [1.0, 1.0, 1.0, 1.0], "60 ≥ 37 → white");
    assert_eq!(
        find("[CLASSES"),
        [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0],
        "mage → red"
    );
    assert!(s.take_errors().is_empty());
}

/// The gated families (`0x52b650`): SIGNABLE green, UNIQUE before STARTS_QUEST, LOCKED red, six
/// equal resistances as one ALL line (Holy never prints alone), a known taught spell's red
/// "Already known", the gold description, and no openable line: the openable, readable and creator
/// tail needs an item object (`0x52e1c7`, `0x52e2e0`), and a template has none.
#[test]
fn verified_families_signable_locked_resists_known() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        5518,
        ItemTemplateView {
            name: "Sealed Charter".into(),
            quality: 1,
            flags: 0x2000 | 0x4, // signable + openable
            max_count: 1,
            start_quest: 42,
            lock_id: 7,
            resistances: [5, 5, 5, 5, 5, 5],
            spell_triggers: vec![(6, 2020, "Recipe: Stew".into())],
            description: "Sign here.".into(),
            ..Default::default()
        },
    );
    s.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![SpellSlotView {
            spell_id: 2020,
            ..Default::default()
        }],
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot5"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:BenillaSetItemById(5518)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "Sealed Charter",
            "[ITEM_SIGNABLE]",
            "[ITEM_UNIQUE]",
            "[ITEM_STARTS_QUEST]",
            "[LOCKED]",
            "[RESIST_ALL +5]",
            "[ITEM_SPELL_KNOWN]",
            "\"Sign here.\"",
        ],
        "the verified gated families in the verified order"
    );
    let color = |needle: &str| lines.iter().find(|(t, _)| t == needle).unwrap().1;
    let red = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
    assert_eq!(color("[LOCKED]"), red, "LOCKED is red");
    assert_eq!(
        color("[ITEM_SPELL_KNOWN]"),
        red,
        "SPELL_KNOWN is unconditional red"
    );
    assert_eq!(
        color("[ITEM_SIGNABLE]"),
        [0.0, 1.0, 0.0, 1.0],
        "SIGNABLE is green"
    );
    assert_eq!(
        color("\"Sign here.\""),
        [1.0, 210.0 / 255.0, 0.0, 1.0],
        "the description is the byte-verified gold"
    );
    s.set_item_template(
        5519,
        ItemTemplateView {
            name: "Blessed Trinket".into(),
            quality: 1,
            resistances: [10, 0, 0, 0, 0, 0],
            ..Default::default()
        },
    );
    s.run(
        r#"
        TT:SetOwner(Slot5, "ANCHOR_RIGHT")
        TT:BenillaSetItemById(5519)
        assert(TT:NumLines() == 1, "a lone Holy resist prints no line, got " .. TT:NumLines())
    "#,
    )
    .unwrap();
    // The five that print follow the builder, not the fields: `0x52c8ad` runs `edi` 1..5 reading
    // school `(edi == 1) ? 6 : edi`, so Arcane leads.
    s.set_item_template(
        5520,
        ItemTemplateView {
            name: "Prismatic Band".into(),
            quality: 1,
            resistances: [9, 1, 2, 3, 4, 5],
            ..Default::default()
        },
    );
    s.run(r#"TT:SetOwner(Slot5, "ANCHOR_RIGHT"); TT:BenillaSetItemById(5520)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        &texts[1..],
        [
            "[RESIST_SINGLE +5 [SCHOOL6]]",
            "[RESIST_SINGLE +1 [SCHOOL2]]",
            "[RESIST_SINGLE +2 [SCHOOL3]]",
            "[RESIST_SINGLE +3 [SCHOOL4]]",
            "[RESIST_SINGLE +4 [SCHOOL5]]",
        ],
        "Arcane, Fire, Nature, Frost, Shadow — and the +9 Holy never prints"
    );
    assert!(s.take_errors().is_empty());
}

/// The slot and type cells red separately: a hard proficiency miss (`0xc4d4a0[class]` bit
/// `1 << subclass`) reds the type; usable only through the alternate subclass, or an off-hand
/// weapon without Dual Wield (`0x5eab70`), reds the slot. The reputation line takes the red
/// (`0xc0d390`) below the required rank, and a hidden subclass prints no type cell.
#[test]
fn proficiency_and_reputation_reds() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut item = axe(); // class 2, subclass 1 (2H axe), invtype 17 "Two-Hand"
    item.required_rep_faction = 72; // Stormwind
    item.required_rep_rank = 5; // Honored
    item.required_rep_line = Some("Requires Stormwind - Honored".into());
    s.set_item_template(871, item.clone());
    // A player who knows 2H axes (bit 1) and stands Honored: everything white.
    let mut req = PlayerReqState {
        level: 60,
        class_id: 1,
        race_id: 1,
        ..Default::default()
    };
    req.proficiency.insert(2, 1 << 1);
    req.rep_ranks.insert(72, 5);
    s.set_player_req_state(req.clone());
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot9"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:BenillaSetItemById(871)
    "#,
    )
    .unwrap();
    let white = [1.0, 1.0, 1.0, 1.0];
    let red = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
    let color = |lines: &[(String, [f32; 4])], needle: &str| {
        lines
            .iter()
            .find(|(t, _)| t == needle)
            .unwrap_or_else(|| panic!("no line {needle:?}"))
            .1
    };
    let lines = lines_of(&mut s);
    assert_eq!(
        color(&lines, "[INVTYPE_2HWEAPON]"),
        white,
        "proficient slot is white"
    );
    assert_eq!(
        right_color(&mut s, "Axe"),
        white,
        "proficient type is white"
    );
    assert_eq!(
        color(&lines, "Requires Stormwind - Honored"),
        white,
        "met reputation is white"
    );
    // A 1H-axes mask, no alternate: the type cell reds, not the slot; Friendly reds the rep line.
    let mut req2 = req;
    req2.proficiency.insert(2, 1 << 0);
    req2.rep_ranks.insert(72, 4);
    s.set_player_req_state(req2.clone());
    s.run(r#"TT:SetOwner(Slot9, "ANCHOR_RIGHT"); TT:BenillaSetItemById(871)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(
        color(&lines, "[INVTYPE_2HWEAPON]"),
        white,
        "slot survives a hard miss"
    );
    assert_eq!(right_color(&mut s, "Axe"), red, "the type cell carries it");
    assert_eq!(
        color(&lines, "Requires Stormwind - Honored"),
        red,
        "unmet reputation is red"
    );
    // With the alternate set to 1H axes, as the real (2, 1) row has it, the slot reds instead.
    let mut alt = item;
    alt.proficiency_alt = Some(0);
    s.set_item_template(871, alt);
    s.run(r#"TT:SetOwner(Slot9, "ANCHOR_RIGHT"); TT:BenillaSetItemById(871)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(
        color(&lines, "[INVTYPE_2HWEAPON]"),
        red,
        "alt-usable reds the slot"
    );
    assert_eq!(
        right_color(&mut s, "Axe"),
        white,
        "the alternate covers the type"
    );
    // An off-hand dagger reds the slot until Dual Wield (an effect-40 spell) is known.
    s.set_item_template(
        872,
        ItemTemplateView {
            name: "Left-Hand Blade".into(),
            class: 2,
            subclass: 15,
            sub_class_display: Some("Dagger".into()),
            inventory_type: 22,
            ..Default::default()
        },
    );
    let mut req3 = req2.clone();
    req3.proficiency.insert(2, 1 << 15);
    s.set_player_req_state(req3.clone());
    s.run(r#"TT:SetOwner(Slot9, "ANCHOR_RIGHT"); TT:BenillaSetItemById(872)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    // Type 22 takes `INVTYPE_WEAPONOFFHAND`, not `INVTYPE_SHIELD`, though both read "Off Hand".
    assert_eq!(
        color(&lines, "[INVTYPE_WEAPONOFFHAND]"),
        red,
        "no Dual Wield reds the slot"
    );
    assert_eq!(
        right_color(&mut s, "Dagger"),
        white,
        "the type is proficient"
    );
    req3.can_dual_wield = true;
    s.set_player_req_state(req3);
    s.run(r#"TT:SetOwner(Slot9, "ANCHOR_RIGHT"); TT:BenillaSetItemById(872)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(
        color(&lines, "[INVTYPE_WEAPONOFFHAND]"),
        white,
        "Dual Wield clears it"
    );
    // A class with no proficiency entry never reds: the map holds only what the server sent.
    s.set_item_template(
        118,
        ItemTemplateView {
            name: "Tattered Cloth Vest".into(),
            class: 4,    // armor, with no class-4 entry in req2
            subclass: 1, // cloth
            inventory_type: 5,
            ..Default::default()
        },
    );
    s.run(r#"TT:SetOwner(Slot9, "ANCHOR_RIGHT"); TT:BenillaSetItemById(118)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(
        color(&lines, "[INVTYPE_CHEST]"),
        white,
        "a class with no mask entry stays white"
    );
    // A hidden subclass (displayFlags bit 0, the Miscellaneous family) prints no type cell.
    s.set_item_template(
        889,
        ItemTemplateView {
            name: "Plain Band".into(),
            class: 4,
            subclass: 0,
            inventory_type: 11,
            hide_subclass: true,
            ..Default::default()
        },
    );
    s.run(
        r#"
        TT:SetOwner(Slot9, "ANCHOR_RIGHT"); TT:BenillaSetItemById(889)
        assert(TTTextLeft2:GetText() == "[INVTYPE_FINGER]")
        assert(TTTextRight2:GetText() == nil or TTTextRight2:GetText() == "",
               "a ring never prints its Miscellaneous type")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// `ITEM_MIN_LEVEL` prints only above 1 (`0x52d2cf`): a level-1 requirement, as starter food and
/// water carry, prints nothing.
#[test]
fn required_level_one_is_hidden() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, req) in [(11, 0u32), (12, 1), (13, 2)] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: format!("Req{req}"),
                required_level: req,
                ..Default::default()
            },
        );
    }
    s.run(
        r#"
        local a = CreateFrame("Button", "SlotR"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:BenillaSetItemById(11)
        assert(tt:NumLines() == 1, "req 0: name only, got " .. tt:NumLines())
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:BenillaSetItemById(12)
        assert(tt:NumLines() == 1, "req 1 hides like req 0, got " .. tt:NumLines())
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:BenillaSetItemById(13)
        assert(tt:NumLines() == 2, "req 2 prints, got " .. tt:NumLines())
        assert(TTTextLeft2:GetText() == "[MIN_LEVEL 2]")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A charter's guild name and master sit between the item name and the green `ITEM_SIGNABLE`
/// (`0x52b650` emits the name, the petition lines, then SIGNABLE). A plain petition takes the
/// "Petition:" and "Created by" keys, and an owner still in flight withholds only its own line.
#[test]
fn charter_lines_sit_between_the_name_and_the_signable_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        5863,
        ItemTemplateView {
            name: "Guild Charter".into(),
            quality: 1,
            flags: 0x2000, // ITEM_FLAG_CHARTER, the green line's own gate
            max_count: 1,
            bonding: 1,
            ..Default::default()
        },
    );
    let charter = |p: Option<PetitionSlotView>| ContainerSlot {
        item_id: 5863,
        count: 1,
        quality: Some(1),
        already_bound: true,
        petition: p,
        ..Default::default()
    };
    let mut slots = HashMap::new();
    slots.insert(
        1,
        charter(Some(PetitionSlotView {
            is_charter: true,
            title: "BTC".into(),
            owner: Some("Twowarrior".into()),
        })),
    );
    slots.insert(
        2,
        charter(Some(PetitionSlotView {
            is_charter: true,
            title: "BTC".into(),
            owner: None,
        })),
    );
    slots.insert(3, charter(None));
    slots.insert(
        4,
        charter(Some(PetitionSlotView {
            is_charter: false,
            title: "Something".into(),
            owner: Some("Someone".into()),
        })),
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 4,
            slots,
        }),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "SlotC"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "Guild Charter",
            "[GUILD_CHARTER_TITLE BTC]",
            "[GUILD_CHARTER_CREATOR Twowarrior]",
            "[ITEM_SIGNABLE]",
            "[ITEM_SOULBOUND]",
            "[ITEM_UNIQUE]",
        ],
        "the two guild lines sit ABOVE the green line, not below it"
    );
    assert_eq!(lines[1].1, [1.0, 1.0, 1.0, 1.0], "the title line is white");
    assert_eq!(lines[2].1, [1.0, 1.0, 1.0, 1.0], "the master line is white");
    assert_eq!(lines[3].1, [0.0, 1.0, 0.0, 1.0], "SIGNABLE is still green");

    let hover = |s: &mut UiScript, slot: u32| {
        s.run(&format!(
            r#"TT:SetOwner(getglobal("SlotC"), "ANCHOR_RIGHT"); TT:SetBagItem(0, {slot})"#
        ))
        .unwrap();
        lines_of(s)
            .into_iter()
            .map(|(t, _)| t)
            .collect::<Vec<String>>()
    };
    assert_eq!(
        hover(&mut s, 2),
        vec![
            "Guild Charter",
            "[GUILD_CHARTER_TITLE BTC]",
            "[ITEM_SIGNABLE]",
            "[ITEM_SOULBOUND]",
            "[ITEM_UNIQUE]"
        ],
        "an unresolved owner withholds ITS line only — the repaint fills it"
    );
    assert_eq!(
        hover(&mut s, 3),
        vec![
            "Guild Charter",
            "[ITEM_SIGNABLE]",
            "[ITEM_SOULBOUND]",
            "[ITEM_UNIQUE]"
        ],
        "no record yet: exactly the plate we shipped before, not a blank one"
    );
    assert_eq!(
        hover(&mut s, 4),
        vec![
            "Guild Charter",
            "[PETITION_TITLE Something]",
            "[PETITION_CREATOR Someone]",
            "[ITEM_SIGNABLE]",
            "[ITEM_SOULBOUND]",
            "[ITEM_UNIQUE]"
        ],
        "the record's charter bit picks the key family"
    );
    assert!(s.take_errors().is_empty());
}

#[test]
fn instance_tail_creator_and_readable() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        889,
        ItemTemplateView {
            name: "Plain Letter".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.set_item_template(
        2589,
        ItemTemplateView {
            name: "Heavy Chest".into(),
            quality: 1,
            flags: 0x4,
            lock_id: 7,
            ..Default::default()
        },
    );
    let slot = |id: u32, readable: bool, creator: Option<&str>, flags: u32| ContainerSlot {
        item_id: id,
        count: 1,
        quality: Some(1),
        readable,
        creator: creator.map(str::to_string),
        flags,
        ..Default::default()
    };
    s.set_item_template(
        7973,
        ItemTemplateView {
            name: "Small Barnacled Clam".into(),
            quality: 1,
            flags: 0x4,
            ..Default::default()
        },
    );
    let mut slots = HashMap::new();
    slots.insert(1, slot(889, true, Some("One"), 0)); // the mail letter
    slots.insert(2, slot(889, true, None, 0)); // creator name still in flight
    slots.insert(3, slot(2589, false, Some("Geoffrey"), 0)); // crafted, still locked
    slots.insert(4, slot(2589, false, None, 0x4)); // unlocked → LOCKED gone, open line on
    slots.insert(5, slot(7973, false, None, 0)); // the clam: lockless, openable outright
    slots.insert(
        6,
        ContainerSlot {
            cooldown: Some((1_000, 30_000, true)),
            ..slot(7973, false, None, 0)
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 6,
            slots,
        }),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "SlotL"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        texts,
        vec!["Plain Letter", "[WRITTEN_BY One]", "[ITEM_READABLE]"],
        "the letter: writer line + instance-gated READABLE"
    );
    assert_eq!(lines[1].1, [1.0, 1.0, 1.0, 1.0], "WRITTEN_BY is white");
    assert_eq!(lines[2].1, [0.0, 1.0, 0.0, 1.0], "READABLE is green");
    // The instance tail (`0x52e1b1`-`0x52e358`): a creator still in flight prints no writer line,
    // and READABLE stays.
    s.run(r#"TT:SetOwner(getglobal("SlotL"), "ANCHOR_RIGHT"); TT:SetBagItem(0, 2)"#)
        .unwrap();
    let texts: Vec<String> = lines_of(&mut s).into_iter().map(|(t, _)| t).collect();
    assert_eq!(texts, vec!["Plain Letter", "[ITEM_READABLE]"]);
    s.run(r#"TT:SetOwner(getglobal("SlotL"), "ANCHOR_RIGHT"); TT:SetBagItem(0, 3)"#)
        .unwrap();
    let texts: Vec<String> = lines_of(&mut s).into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        texts,
        vec![
            "Heavy Chest",
            "[LOCKED]",
            "|cff00ff00[CREATED_BY Geoffrey]|r"
        ],
        "CREATED_BY carries the string's own green escape; locked chest hides OPENABLE"
    );
    // The instance's UNLOCKED bit drops the LOCKED line and satisfies the openable lock gate.
    s.run(r#"TT:SetOwner(getglobal("SlotL"), "ANCHOR_RIGHT"); TT:SetBagItem(0, 4)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(texts, vec!["Heavy Chest", "[ITEM_OPENABLE]"]);
    assert_eq!(lines[1].1, [0.0, 1.0, 0.0, 1.0], "OPENABLE is green");
    s.run(r#"TT:SetOwner(getglobal("SlotL"), "ANCHOR_RIGHT"); TT:SetBagItem(0, 5)"#)
        .unwrap();
    let texts: Vec<String> = lines_of(&mut s).into_iter().map(|(t, _)| t).collect();
    assert_eq!(texts, vec!["Small Barnacled Clam", "[ITEM_OPENABLE]"]);
    // Mid-cooldown the clam takes `SetBagItem`'s other leg (p6 = 1, picked at `0x6e2ed0`), which
    // skips the openable tree; the reference prints `ITEM_COOLDOWN_TIME` there, unfed here.
    let has_cd: bool = s
        .eval(r#"TT:SetOwner(getglobal("SlotL"), "ANCHOR_RIGHT"); return TT:SetBagItem(0, 6)"#)
        .unwrap();
    assert!(has_cd, "the cooldown leg is what the return value reports");
    let texts: Vec<String> = lines_of(&mut s).into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        texts,
        vec!["Small Barnacled Clam"],
        "a running cooldown suppresses ITEM_OPENABLE — the two lines are exclusive here"
    );
    assert!(s.take_errors().is_empty());
}

/// Bonding `[record+0x194]` in 1..5 decides whether a bind line prints; a runtime-bound instance
/// (`0x5da2c0`: the soulbound flag, or a live enchant slot that binds) prints `ITEM_SOULBOUND`,
/// or `ITEM_BIND_QUEST` for the quest kinds; else the jump table `0x52e4fc` picks 1 picked up,
/// 2 equipped, 3 used, 4 and 5 Quest Item.
#[test]
fn a_runtime_bound_instance_overrides_the_bind_line_to_soulbound() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(871, axe()); // bonding 2: Binds when equipped
    let ring = ItemTemplateView {
        name: "Maiden's Circle".into(),
        quality: 2,
        class: 4,
        subclass: 0,
        inventory_type: 11,
        bonding: 2,
        ..Default::default()
    };
    s.set_item_template(942, ring);
    let quest_item = ItemTemplateView {
        name: "Fresh Fish".into(),
        quality: 1,
        bonding: 4,
        ..Default::default()
    };
    s.set_item_template(4913, quest_item);
    let slot = |item_id: u32, already_bound: bool| ContainerSlot {
        count: 1,
        quality: Some(2),
        item_id,
        already_bound,
        ..Default::default()
    };
    let mut slots = HashMap::new();
    slots.insert(1, slot(942, true)); // bound
    slots.insert(2, slot(942, false)); // the same ring, not yet bound
    slots.insert(3, slot(4913, true)); // a bound quest item
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
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
    let bind_line = |s: &mut UiScript| {
        lines_of(s)
            .into_iter()
            .find(|(t, _)| {
                matches!(
                    t.as_str(),
                    "[ITEM_SOULBOUND]"
                        | "[ITEM_BIND_ON_PICKUP]"
                        | "[ITEM_BIND_ON_EQUIP]"
                        | "[ITEM_BIND_ON_USE]"
                        | "[ITEM_BIND_QUEST]"
                )
            })
            .unwrap_or_else(|| panic!("no bind line at all"))
    };
    let (text, color) = bind_line(&mut s);
    assert_eq!(
        text, "[ITEM_SOULBOUND]",
        "a runtime-bound instance overrides the template's Bonding"
    );
    assert_eq!(color, [1.0, 1.0, 1.0, 1.0], "the bind line is white");

    // Controls: the same ring unbound, and a template hover, which has no instance.
    s.run(r#"TT:SetBagItem(0, 2)"#).unwrap();
    assert_eq!(bind_line(&mut s).0, "[ITEM_BIND_ON_EQUIP]");

    s.run(r#"TT:BenillaSetItemById(871)"#).unwrap();
    assert_eq!(bind_line(&mut s).0, "[ITEM_BIND_ON_EQUIP]");

    // A bound quest item stays `ITEM_BIND_QUEST`, the text 4 and 5 print anyway.
    s.run(r#"TT:SetBagItem(0, 3)"#).unwrap();
    assert_eq!(bind_line(&mut s).0, "[ITEM_BIND_QUEST]");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The damage templates (`0x52c22b`): the school arm goes by the slot's school number, the ammo
/// arm by `ItemClass == 6` alone, the single arm by equal rounded bounds, and the first/`PLUS_`
/// flag is per item, kept past a skipped slot. Bounds round as `floor(min)`, `ceil(max)`.
#[test]
fn damage_matrix_arms_and_the_first_flag() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let show = |s: &mut UiScript, id: u32, v: ItemTemplateView| -> Vec<String> {
        s.set_item_template(id, v);
        s.run(&format!(
            r#"local a = CreateFrame("Button", "S{id}"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
               if not TT then CreateFrame("GameTooltip", "TT") end
               TT:SetOwner(S{id}, "ANCHOR_RIGHT"); TT:BenillaSetItemById({id})"#
        ))
        .unwrap();
        lines_of(s).into_iter().map(|(t, _)| t).collect()
    };
    let weapon = |damages: Vec<(f32, f32, u32)>| ItemTemplateView {
        name: "Probe".into(),
        class: 2,
        subclass: 7,
        sub_class_display: Some("Sword".into()),
        inventory_type: 13,
        damages,
        delay_ms: 2600,
        ..Default::default()
    };

    // A zero middle slot is skipped; only the first line carries the Speed cell.
    let lines = show(
        &mut s,
        900,
        weapon(vec![(5.0, 9.0, 0), (0.0, 0.0, 0), (3.0, 6.0, 0)]),
    );
    assert_eq!(
        &lines[2..5],
        // (9+5)/2 + (6+3)/2 = 11.5 raw, over 2.6 s.
        ["[DMG 5 - 9]", "[+DMG 3 - 6]", "[DPS 4.4]"],
        "plain then PLUS_, the zero slot skipped: {lines:?}"
    );
    let speed: String = s
        .eval("return TTTextRight3:GetText() or ''")
        .expect("right cell 3");
    assert_eq!(speed, "Speed 2.60", "the SPEED cell is the first line's");
    let later: String = s
        .eval("return TTTextRight4:GetText() or ''")
        .expect("right cell 4");
    assert_eq!(later, "", "a later damage line renders left-only");

    // Fang of the Mystics' real bounds: `floor`/`ceil` give 38 - 86, where rounding gives 39.
    let lines = show(&mut s, 901, weapon(vec![(38.7, 85.7, 0)]));
    assert_eq!(lines[2], "[DMG 38 - 86]", "{lines:?}");

    // The single arm: equal rounded bounds, no school, not ammo.
    let lines = show(&mut s, 902, weapon(vec![(7.0, 7.0, 0)]));
    assert_eq!(lines[2], "[DMG1 7]", "{lines:?}");
    // A slot with a school and equal bounds takes `WITH_SCHOOL`: no single school template exists.
    let lines = show(&mut s, 903, weapon(vec![(7.0, 7.0, 4)]));
    assert_eq!(lines[2], "[DMGS 7 - 7 [SCHOOL4]]", "{lines:?}");

    // Ammo's `%g` is the rounded bounds' average, undivided (Rough Arrow's 1-2 reads 1.5), with
    // no Speed cell or DPS line, both gated on class 2.
    let arrow = ItemTemplateView {
        name: "Rough Arrow".into(),
        class: 6,
        subclass: 2,
        sub_class_display: Some("Arrow".into()),
        item_type: Some("Projectile".into()),
        inventory_type: 24,
        damages: vec![(1.0, 2.0, 0)],
        delay_ms: 3000,
        ..Default::default()
    };
    let lines = show(&mut s, 904, arrow.clone());
    // The type line reads "Projectile | Arrow": the left cell is the DBC name (`0x52c0bc`),
    // never a GlobalString such as the absent `INVTYPE_AMMO`.
    assert_eq!(lines[1], "Projectile", "{lines:?}");
    let ty: String = s
        .eval("return TTTextRight2:GetText() or ''")
        .expect("type cell");
    assert_eq!(
        ty, "Arrow",
        "the RIGHT cell is unchanged on the class-6 leg"
    );
    assert_eq!(lines[2], "[AMMO 1.5]", "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.starts_with("[DPS")),
        "ammo never reaches the DPS line: {lines:?}"
    );
    let right: String = s
        .eval("return TTTextRight3:GetText() or ''")
        .expect("right cell");
    assert_eq!(right, "", "ammo gets no Speed cell either");
    let lines = show(
        &mut s,
        905,
        ItemTemplateView {
            damages: vec![(1.0, 2.0, 3), (5.0, 6.0, 3)],
            ..arrow
        },
    );
    assert_eq!(
        &lines[2..4],
        ["[AMMOS 1.5 [SCHOOL3]]", "[+AMMOS 5.5 [SCHOOL3]]"],
        "{lines:?}"
    );

    // A non-weapon that carries damage takes the ordinary arms but neither class-2 gate.
    let lines = show(
        &mut s,
        906,
        ItemTemplateView {
            name: "Odd Trinket".into(),
            class: 4,
            subclass: 0,
            hide_subclass: true,
            inventory_type: 12,
            damages: vec![(4.0, 8.0, 0)],
            delay_ms: 2000,
            ..Default::default()
        },
    );
    assert_eq!(lines[2], "[DMG 4 - 8]", "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.starts_with("[DPS")),
        "DPS is weapons-only: {lines:?}"
    );
}

/// The bag line (`0x52b754`) gates on `InventoryType == 0x12` alone, and its noun is the
/// subclass's DisplayName; a row with no name prints no line.
#[test]
fn container_slots_line_names_its_subclass() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let bag = |sub: u32, name: Option<&str>, slots: u32| ItemTemplateView {
        name: "Pouch".into(),
        class: 1,
        subclass: sub,
        sub_class_display: name.map(str::to_string),
        inventory_type: 18,
        container_slots: slots,
        ..Default::default()
    };
    let show = |s: &mut UiScript, id: u32, v: ItemTemplateView| -> Vec<String> {
        s.set_item_template(id, v);
        s.run(&format!(
            r#"local a = CreateFrame("Button", "B{id}"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
               if not TT then CreateFrame("GameTooltip", "TT") end
               TT:SetOwner(B{id}, "ANCHOR_RIGHT"); TT:BenillaSetItemById({id})"#
        ))
        .unwrap();
        lines_of(s).into_iter().map(|(t, _)| t).collect()
    };

    assert_eq!(
        show(&mut s, 910, bag(0, Some("Bag"), 16))[1],
        "[SLOTS 16 Bag]"
    );
    assert_eq!(
        show(&mut s, 911, bag(1, Some("Soul Bag"), 24))[1],
        "[SLOTS 24 Soul Bag]",
        "the noun is the subclass's, not a constant"
    );
    // A quiver is InventoryType 18 like any container, so it names itself here.
    let mut quiver = bag(2, Some("Quiver"), 8);
    quiver.class = 11;
    assert_eq!(show(&mut s, 912, quiver)[1], "[SLOTS 8 Quiver]");
    assert_eq!(
        show(&mut s, 913, bag(0, Some("Bag"), 0))[1],
        "[SLOTS 0 Bag]"
    );
    // No row name: neither this line nor the slot and type line.
    let lines = show(&mut s, 914, bag(0, None, 16));
    assert_eq!(lines.len(), 1, "name line only: {lines:?}");
}
