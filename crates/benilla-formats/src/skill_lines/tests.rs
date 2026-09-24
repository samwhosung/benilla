//! Skill-line catalog tests: real-data pins, which skip without client data, and synthetic rank
//! chains.

use super::*;

/// Line ids as vmangos `SharedDefines.h`'s `SkillType` has them (Frost 6, Fire 8).
#[test]
fn real_skill_line_catalog_resolves_known_spells() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load SkillLine/SkillLineAbility");
    assert!(
        cat.len() > 50,
        "a real skill-line table has hundreds of rows"
    );

    // Fireball (133) -> the Fire line (SKILL_FIRE = 8).
    let fire_line = cat.spell_to_line(133).expect("Fireball has a skill line");
    assert_eq!(fire_line, 8);
    let fire = cat.line(fire_line).expect("the Fire line resolves");
    assert_eq!(fire.name, "Fire");
    assert!(fire.icon.is_some(), "the Fire tab has an icon");

    // Frost Armor (168) -> the Frost line (SKILL_FROST = 6).
    let frost_line = cat
        .spell_to_line(168)
        .expect("Frost Armor has a skill line");
    assert_eq!(frost_line, 6);
    let frost = cat.line(frost_line).expect("the Frost line resolves");
    assert_eq!(frost.name, "Frost");
    assert!(frost.icon.is_some(), "the Frost tab has an icon");

    assert_eq!(cat.spell_to_line(0), None);

    // Column 12: a profession's own flavour sentence, the weapon lines' shared one.
    let smithing = cat.line(164).expect("Blacksmithing resolves");
    assert!(
        smithing.description.to_lowercase().contains("blacksmith"),
        "Blacksmithing's description names the trade: {:?}",
        smithing.description
    );
    let swords = cat.line(43).expect("Swords resolves");
    assert_eq!(
        swords.description, "Higher weapon skill increases your chance to hit.",
        "the weapon lines' shared byte-exact sentence"
    );
}

/// A human warrior against a human mage: one Fireball is General for the first, Fire for the other.
#[test]
fn real_spell_tab_collapses_general_by_race_and_class() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");
    const HUMAN: u8 = 1;
    const WARRIOR: u8 = 1;
    const MAGE: u8 = 8;

    // Charge, Heroic Strike and Rend on Arms (26), Battle Shout on Fury (256).
    for (id, line) in [(100u32, 26u32), (78, 26), (772, 26), (6673, 256)] {
        assert_eq!(cat.spell_to_line(id), Some(line));
        assert_eq!(
            cat.spell_tab(id, HUMAN, WARRIOR),
            line,
            "warrior class ability {id} keeps its own tab {line}"
        );
    }

    // Perception, on line 754 `Racial - Human`.
    assert_eq!(cat.spell_to_line(20600), Some(754));
    assert_eq!(
        cat.spell_tab(20600, HUMAN, WARRIOR),
        0,
        "a human racial routes to General"
    );

    // A warrior has no Fire row; a mage's has no sort flag.
    assert_eq!(cat.spell_to_line(133), Some(8));
    assert_eq!(
        cat.spell_tab(133, HUMAN, WARRIOR),
        0,
        "a warrior's cross-class Fireball collapses to General"
    );
    assert_eq!(
        cat.spell_tab(133, HUMAN, MAGE),
        8,
        "a mage's Fireball keeps its Fire tab"
    );

    // Race and class 0 keep the raw line.
    assert_eq!(cat.spell_tab(133, 0, 0), 8);
}

/// `flags & 0x402` silences the lines 1.12 shows silent; expected flags read off the raw file.
#[test]
fn real_skill_up_announce_gate_matches_the_archives() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");
    const HUMAN: u8 = 1;
    const NIGHT_ELF: u8 = 4;
    const WARRIOR: u8 = 1;
    const HUNTER: u8 = 3;

    // A night-elf hunter's 0x402 rows: raw flags 0x492 (hidden) and 0x410 (mono).
    for (id, name) in [
        (183u32, "GENERIC (DND)"),
        (51, "Survival"),
        (126, "Night Elf Racial"),
        (163, "Marksmanship"),
        (50, "Beast Mastery"),
        (118, "Dual Wield"),
    ] {
        assert!(
            !cat.announces_skill_ups(id, NIGHT_ELF, HUNTER),
            "{name} ({id}) must be silent"
        );
    }
    // Raw flags 0x080/0x0a0: weapons, Defense, secondary skills.
    for (id, name) in [
        (43u32, "Swords"),
        (95, "Defense"),
        (185, "Cooking"),
        (129, "First Aid"),
        (356, "Fishing"),
    ] {
        assert!(
            cat.announces_skill_ups(id, NIGHT_ELF, HUNTER),
            "{name} ({id}) must announce"
        );
    }
    // A warrior's own spec line is silent too (Arms 26, raw 0x410).
    assert!(
        !cat.announces_skill_ups(26, HUMAN, WARRIOR),
        "Arms must be silent"
    );
    // Fist Weapons (473, raw 0x082), the one silent weapon line, on the 0x2 bit alone.
    assert!(
        !cat.announces_skill_ups(473, HUMAN, WARRIOR),
        "Fist Weapons must be silent"
    );
    // No admitting row (a mage line for a warrior): the reference skips too (`0x5de352`).
    assert!(
        !cat.announces_skill_ups(6, HUMAN, WARRIOR),
        "a row-less line is silent"
    );
}

/// Expected values read off the raw file's rows.
#[test]
fn real_skill_line_ability_reads_requirement_and_trivial_ranks() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Bolt of Linen Cloth (Tailoring), Minor Healing Potion (Alchemy), Charred Wolf Meat
    // (Cooking), Crafted Light Shot (Engineering).
    for (spell, line, req, low, high) in [
        (2963u32, 197u32, 1u32, 25u32, 50u32),
        (2330, 171, 1, 55, 95),
        (2538, 185, 1, 45, 85),
        (3920, 202, 1, 30, 60),
    ] {
        let sla = cat.ability(spell).expect("recipe has an SLA row");
        assert_eq!(
            (
                sla.skill_id,
                sla.req_skill_value,
                sla.trivial_low,
                sla.trivial_high
            ),
            (line, req, low, high),
            "spell {spell}"
        );
    }

    // The Tailoring and Enchanting openers carry zero trivial ranks.
    for (spell, line) in [(3908u32, 197u32), (7411, 333)] {
        let sla = cat.ability(spell).expect("opener has an SLA row");
        assert_eq!(
            (sla.skill_id, sla.trivial_low, sla.trivial_high),
            (line, 0, 0)
        );
    }
}

/// Expected values read off the raw `SkillLineCategory.dbc`.
#[test]
fn real_skill_categories_name_and_order_the_pane_groups() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // The raw rows: (id, name, displayOrder).
    assert_eq!(cat.category(7), Some(("Class Skills", 2)));
    assert_eq!(cat.category(11), Some(("Professions", 3)));
    assert_eq!(cat.category(9), Some(("Secondary Skills", 4)));
    assert_eq!(cat.category(6), Some(("Weapon Skills", 5)));
    assert_eq!(cat.category(10), Some(("Languages", 7)));
    // Category 12 is an ordinary header, not a hide bucket.
    assert_eq!(cat.category(12), Some(("Not Displayed", 8)));
    assert_eq!(cat.category(0), None);

    // The join: Tailoring (197) is a Profession; First Aid (129) is Secondary; the Fire
    // school (8) is a Class Skill; Common (98) is a Language.
    for (line, category) in [(197u32, 11u32), (129, 9), (8, 7), (98, 10)] {
        assert_eq!(
            cat.line(line).map(|l| l.category_id),
            Some(category),
            "line {line}"
        );
    }
}

/// For a human warrior (race 1, class 1), primary professions are abandonable; secondary skills
/// and class, weapon and language lines are not.
#[test]
fn real_abandonable_split_professions_yes_weapons_no() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Primary professions carry 0xA0: Blacksmithing, Tailoring, Engineering, Alchemy,
    // Enchanting, Leatherworking, Skinning.
    for line in [164u32, 197, 202, 171, 333, 165, 393] {
        assert!(cat.abandonable(line, 1, 1), "line {line} is abandonable");
    }
    // First Aid, Fishing, Cooking carry 0x80 alone; Fire has no human-warrior row; then weapon,
    // Defense, language and riding lines.
    for line in [129u32, 356, 185, 8, 43, 95, 98, 762] {
        assert!(!cat.abandonable(line, 1, 1), "line {line} is not");
    }
    assert!(!cat.abandonable(164, 0, 0));
}

/// `flags & 0x400` for a night-elf hunter (race 4, class 3).
#[test]
fn real_mono_value_split_class_lines_yes_weapons_no() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Beast Mastery / Survival / Marksmanship (0x410), Dual Wield / Night Elf Racial / the
    // Tiger Riding mount line / GENERIC (DND) (0x492).
    for line in [50u32, 51, 163, 118, 126, 150, 183] {
        assert!(cat.mono_value(line, 4, 3), "line {line} is single-rank");
    }
    // Weapon lines (Axes/Bows/Daggers/Crossbows/Unarmed/Defense), armor proficiencies
    // (Cloth/Leather/Mail), languages (Common/Darnassian), Riding, First Aid.
    for line in [
        44u32, 45, 173, 226, 162, 95, 415, 414, 413, 98, 113, 762, 129,
    ] {
        assert!(!cat.mono_value(line, 4, 3), "line {line} keeps its rank");
    }
    assert!(!cat.mono_value(50, 0, 0));
}

/// `flags & 0x2` and the `reqLevel` column for a night-elf hunter (race 4, class 3).
#[test]
fn real_hidden_lines_are_the_ones_the_reference_client_never_lists() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");
    let row = |line: u32| cat.race_class(line, 4, 3).expect("an admitting row");

    // Dual Wield, Night Elf Racial, Tiger Riding, GENERIC (DND): all 0x492.
    for line in [118u32, 126, 150, 183] {
        assert!(row(line).hidden(), "line {line} never gets a row");
    }
    // What the reference lists: class lines, weapons, armor, languages, Riding.
    for line in [50u32, 51, 163, 44, 95, 162, 413, 414, 415, 98, 762, 129] {
        assert!(!row(line).hidden(), "line {line} is listed");
    }

    assert_eq!(row(413).min_level, 40, "Mail");
    assert_eq!(row(118).min_level, 20, "Dual Wield");
    // No row carries 0x1 and only line 493 carries 0x4, so a rank-0 line is otherwise never
    // listed.
    assert!(
        !row(413).displays_untrained(60),
        "Mail at rank 0 stays off the pane at any level — 0x80 carries neither gate bit"
    );
    assert!(
        row(493).displays_untrained(0),
        "the single 0x4 row shows from level 0 (its reqLevel is 0)"
    );
    assert_eq!(cat.race_class(118, 0, 0), None);
}

/// The `forward_spellid` chains the action bar normalises ranks along.
#[test]
fn real_rank_chains_resolve_the_highest_known_rank() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Sinister Strike ranks 1 to 8: a saved bar can hold rank 1 while the book holds rank 8.
    const SS: [u32; 8] = [1752, 1757, 1758, 1759, 1760, 8621, 11293, 11294];
    for pair in SS.windows(2) {
        assert_eq!(
            cat.rank_successor(pair[0]),
            Some(pair[1]),
            "Sinister Strike {} → {}",
            pair[0],
            pair[1]
        );
    }
    assert_eq!(cat.rank_successor(11294), None, "rank 8 tops the chain");
    // Charge (100, 6178, 11578) and Heroic Strike's last link.
    assert_eq!(cat.rank_successor(100), Some(6178));
    assert_eq!(cat.rank_successor(6178), Some(11578));
    assert_eq!(cat.rank_successor(11567), Some(25286));

    // Knowing only rank 8, every rank resolves to it: the walk starts at the chain head.
    let top: std::collections::BTreeSet<u32> = [11294].into_iter().collect();
    for id in SS {
        assert_eq!(cat.highest_known_rank(id, &top), Some(11294), "from {id}");
    }
    let mid: std::collections::BTreeSet<u32> = [1759].into_iter().collect();
    assert_eq!(cat.highest_known_rank(1752, &mid), Some(1759));
    assert_eq!(cat.highest_known_rank(11294, &mid), Some(1759));
    assert_eq!(cat.highest_known_rank(1752, &Default::default()), None);

    // Caster spells carry no forward link: Fireball ranks 1 and 2, and rank 1 of Frostbolt,
    // Healing Touch, Renew, Immolate and Lightning Bolt.
    for id in [133u32, 143, 116, 5185, 139, 348, 403] {
        assert_eq!(cat.rank_successor(id), None, "spell {id} is not chained");
        let known: std::collections::BTreeSet<u32> = [id].into_iter().collect();
        assert_eq!(cat.highest_known_rank(id, &known), Some(id));
    }
    // Auto-attack has no `SkillLineAbility` row and is its own chain.
    let attack: std::collections::BTreeSet<u32> = [6603].into_iter().collect();
    assert_eq!(cat.rank_successor(6603), None);
    assert_eq!(cat.highest_known_rank(6603, &attack), Some(6603));
}

/// `KnownHigherRank` (`0x60c8d0`) walks forward only; the spell itself does not count.
#[test]
fn a_higher_rank_is_known_only_forward_along_the_chain() {
    use std::collections::BTreeSet;
    let row = |forward| SlaInfo {
        skill_id: 1,
        req_skill_value: 0,
        forward_spell_id: forward,
        trivial_low: 0,
        trivial_high: 0,
    };
    // 10 → 11 → 12.
    let cat = SkillLineCatalog::from_abilities([(10, row(11)), (11, row(12)), (12, row(0))]);
    let known = |ids: &[u32]| ids.iter().copied().collect::<BTreeSet<u32>>();
    assert!(cat.higher_rank_known(10, &known(&[12])));
    assert!(cat.higher_rank_known(10, &known(&[11])));
    assert!(
        !cat.higher_rank_known(10, &known(&[10])),
        "the spell itself is not a higher rank"
    );
    assert!(
        !cat.higher_rank_known(12, &known(&[10, 11])),
        "nothing is forward of the top rank"
    );
    assert!(
        !cat.higher_rank_known(99, &known(&[10, 11, 12])),
        "an unchained spell has no chain"
    );
}
