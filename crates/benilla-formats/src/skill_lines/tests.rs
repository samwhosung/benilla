//! The catalog's tests — the real-data pins (skip without client data) and the
//! synthetic rank-chain cases. Split from `mod.rs` for size only (one concern, two files).

use super::*;

/// Resolve real spells to their real skill lines on the build-5875 data, cross-checked
/// against vmangos `SharedDefines.h`'s `SkillType` enum (Frost=6, Fire=8 — the module doc's
/// own citation). Skips without client data.
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

    // An unknown spell id has no line.
    assert_eq!(cat.spell_to_line(0), None);

    // The description column (12): a profession line carries the flavor sentence, a weapon
    // line is blank — a column slip lands on another locale (empty for enUS data) or the
    // name flags word (a parse error) and fails loudly.
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

/// The General collapse on real build-5875 `SkillRaceClassInfo.dbc` (decision 0228), traced on
/// concrete spells for a human warrior (race 1, class 1) vs. a human mage (race 1, class 8):
/// class-native combat lines keep their own tab, racials collapse to General, and a cross-class
/// spell (a warrior's cheated Fireball) collapses to General while the SAME spell keeps its Fire
/// tab for a mage — the class/race dependence, the whole point of the routing. Skips without
/// client data.
#[test]
fn real_spell_tab_collapses_general_by_race_and_class() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");
    const HUMAN: u8 = 1;
    const WARRIOR: u8 = 1;
    const MAGE: u8 = 8;

    // Warrior class abilities keep their own class-line tabs (flag clear for a warrior):
    // Charge/Heroic Strike/Rend on Arms (26), Battle Shout on Fury (256).
    for (id, line) in [(100u32, 26u32), (78, 26), (772, 26), (6673, 256)] {
        assert_eq!(cat.spell_to_line(id), Some(line));
        assert_eq!(
            cat.spell_tab(id, HUMAN, WARRIOR),
            line,
            "warrior class ability {id} keeps its own tab {line}"
        );
    }

    // A human racial (Perception, line 754 "Racial - Human") collapses to General (0).
    assert_eq!(cat.spell_to_line(20600), Some(754));
    assert_eq!(
        cat.spell_tab(20600, HUMAN, WARRIOR),
        0,
        "a human racial routes to General"
    );

    // Fireball (Fire line 8): class-dependent. A warrior has no Fire race/class row → General;
    // a mage has one, flag clear → its own Fire tab. Same spell, different tab by class.
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

    // Unknown character (race/class 0): the collapse is skipped — the raw line stands.
    assert_eq!(cat.spell_tab(133, 0, 0), 8);
}

/// The skill-up message gate on the real build-5875 `SkillRaceClassInfo.dbc` (decision 1309,
/// bugs B19/B245): `flags & 0x402` silences exactly the lines the 1.11/1.12 archives show
/// silent. Expected flags read straight off the raw file this session (a struct-unpack
/// dump): the B245 shot's announced-but-shouldn't-be lines for a night-elf hunter, the
/// announce controls, and the class dependence. Skips without client data.
#[test]
fn real_skill_up_announce_gate_matches_the_archives() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");
    const HUMAN: u8 = 1;
    const NIGHT_ELF: u8 = 4;
    const WARRIOR: u8 = 1;
    const HUNTER: u8 = 3;

    // The B245 shot, verbatim — GENERIC (DND), Survival, Night Elf Racial and Marksmanship
    // all announced on one ding — plus the hunter's other 0x402-flagged rows (raw flags
    // 0x492 hidden / 0x410 mono).
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
    // The lines skill-ups exist FOR (raw flags 0x080/0x0a0): weapons, Defense, secondary.
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
    // The class dependence: a warrior's own spec line is silent too (Arms 26, raw 0x410) —
    // B19's "magic skill-ups" generalized to every class's spec lines.
    assert!(
        !cat.announces_skill_ups(26, HUMAN, WARRIOR),
        "Arms must be silent"
    );
    // Fist Weapons (473, raw 0x082): the one silent WEAPON line — the 0x2 half of the mask
    // alone, corroborated by vmangos giving exactly this line SKILL_RANGE_MONO.
    assert!(
        !cat.announces_skill_ups(473, HUMAN, WARRIOR),
        "Fist Weapons must be silent"
    );
    // No admitting row (a mage-only line for a warrior): the real watcher's empty resolve takes
    // the same skip branch as the flag test (`0x5de352` — decision 1314).
    assert!(
        !cat.announces_skill_ups(6, HUMAN, WARRIOR),
        "a row-less line is silent"
    );
}

/// The `SlaInfo` columns on the real build-5875 `SkillLineAbility.dbc` (0437) — expected
/// values read straight off the raw file's rows this session (a struct-unpack dump, module
/// doc): recipes carry the trivial ranks, the profession openers carry zeros. A column slip
/// fails loudly. Skips without client data.
#[test]
fn real_skill_line_ability_reads_requirement_and_trivial_ranks() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Recipes: (spell, line, req, trivial_low, trivial_high) — raw rows, module doc.
    // 2963 Bolt of Linen Cloth (Tailoring 197), 2330 Minor Healing Potion (Alchemy 171),
    // 2538 Charred Wolf Meat (Cooking 185), 3920 Crafted Light Shot (Engineering 202).
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

    // The openers (effect-47 window spells) sit on their line with zero trivial ranks:
    // 3908 Tailoring → 197, 7411 Enchanting → 333.
    for (spell, line) in [(3908u32, 197u32), (7411, 333)] {
        let sla = cat.ability(spell).expect("opener has an SLA row");
        assert_eq!(
            (sla.skill_id, sla.trivial_low, sla.trivial_high),
            (line, 0, 0)
        );
    }
}

/// `SkillLineCategory.dbc` + the line→category join on the real build-5875 files (0437 phase
/// 4) — expected values read straight off the raw dump (module doc). Skips without client
/// data.
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
    // Category 12 is a real row like any other — NOT a hide bucket: the client's list build
    // drops `GENERIC (DND)` by its `flags & 0x2`, never by its category (decision 1091).
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

/// `SkillRaceClassInfo.flags & 0x20` on the real build-5875 file: professions and secondary
/// skills are abandonable, class/weapon/language lines are not — the unlearn button's real
/// data split (a human warrior, race 1 / class 1). Skips without client data.
#[test]
fn real_abandonable_split_professions_yes_weapons_no() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // PRIMARY professions carry 0xA0 (0x80 sort | 0x20 unlearnable) — the red circle-slash:
    // Blacksmithing, Tailoring, Engineering, Alchemy, Enchanting, Leatherworking, Skinning.
    for line in [164u32, 197, 202, 171, 333, 165, 393] {
        assert!(cat.abandonable(line, 1, 1), "line {line} is abandonable");
    }
    // SECONDARY skills (First Aid/Fishing/Cooking) are 0x80 only — famously NOT droppable in
    // 1.12 — and class school (Fire: no human-warrior row at all) / weapon / Defense /
    // language / riding lines aren't either.
    for line in [129u32, 356, 185, 8, 43, 95, 98, 762] {
        assert!(!cat.abandonable(line, 1, 1), "line {line} is not");
    }
    // Unknown race/class → no button (the conservative arm).
    assert!(!cat.abandonable(164, 0, 0));
}

/// `SkillRaceClassInfo.flags & 0x400` on the real build-5875 file (a night-elf hunter, race 4
/// / class 3): the class talent lines, Dual Wield, the racial and the per-mount riding lines
/// are MONO — the client reports their `skillMaxRank` as 1 and the pane draws them as gray
/// proficiencies — while every weapon line, armor proficiency, language, `Riding` and
/// profession keeps its real rank. Skips without client data.
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
    // Unknown race/class → keep the server's numbers (the conservative arm).
    assert!(!cat.mono_value(50, 0, 0));
}

/// `SkillRaceClassInfo.flags & 0x2` on the real build-5875 file — the bit that keeps a line
/// off the Skills tab entirely (decision 1091; `0x4d2cb0`'s `4d2d9f test dl,0x2`), plus
/// the `reqLevel` column the untrained gate reads. A night-elf hunter, race 4 / class 3.
/// Skips without client data.
#[test]
fn real_hidden_lines_are_the_ones_the_reference_client_never_lists() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");
    let row = |line: u32| cat.race_class(line, 4, 3).expect("an admitting row");

    // Dual Wield, Night Elf Racial, Tiger Riding, GENERIC (DND) — all 0x492, all absent from
    // the real client's pane however high the server ranks them.
    for line in [118u32, 126, 150, 183] {
        assert!(row(line).hidden(), "line {line} never gets a row");
    }
    // Everything the ref does list: the class lines, weapons, armor, languages, Riding.
    for line in [50u32, 51, 163, 44, 95, 162, 413, 414, 415, 98, 762, 129] {
        assert!(!row(line).hidden(), "line {line} is listed");
    }

    // reqLevel (column 5) reads for real: Mail 40, Dual Wield 20.
    assert_eq!(row(413).min_level, 40, "Mail");
    assert_eq!(row(118).min_level, 20, "Dual Wield");
    // The untrained gate is all but inert on this build's data, and the test says so: NO row
    // in the whole file carries 0x1, and exactly ONE carries 0x4 (line 493, reqLevel 0 — a
    // line `SkillLine.dbc` doesn't even have). So a rank-0 line is simply never listed; a
    // `reqLevel` on its own does nothing without 0x4.
    assert!(
        !row(413).displays_untrained(60),
        "Mail at rank 0 stays off the pane at any level — 0x80 carries neither gate bit"
    );
    assert!(
        row(493).displays_untrained(0),
        "the single 0x4 row shows from level 0 (its reqLevel is 0)"
    );
    // No admitting row at all → the caller drops the line (the client's own `!srci → continue`).
    assert_eq!(cat.race_class(118, 0, 0), None);
}

/// The `forward_spellid` rank graph on the real build-5875 file — the action bar's
/// rank-normalization source (decision 0883). Skips without client data.
#[test]
fn real_rank_chains_resolve_the_highest_known_rank() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Sinister Strike ranks 1..8 — the chain the bug report walked (a level-60 rogue whose
    // saved bar still held rank 1 while the book held only rank 8).
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
    // Warrior Charge (100 → 6178 → 11578) and Heroic Strike's 9 ranks — the two physical
    // families the director named.
    assert_eq!(cat.rank_successor(100), Some(6178));
    assert_eq!(cat.rank_successor(6178), Some(11578));
    assert_eq!(cat.rank_successor(11567), Some(25286));

    // From ANY rank, knowing only rank 8, the answer is rank 8 — the walk starts at the
    // chain head, so it works downward as well as upward.
    let top: std::collections::BTreeSet<u32> = [11294].into_iter().collect();
    for id in SS {
        assert_eq!(cat.highest_known_rank(id, &top), Some(11294), "from {id}");
    }
    // Knowing an intermediate rank resolves to it, not to the top of the chain.
    let mid: std::collections::BTreeSet<u32> = [1759].into_iter().collect();
    assert_eq!(cat.highest_known_rank(1752, &mid), Some(1759));
    assert_eq!(cat.highest_known_rank(11294, &mid), Some(1759));
    // No rank of the chain known → no answer (the caller leaves the slot alone).
    assert_eq!(cat.highest_known_rank(1752, &Default::default()), None);

    // Caster nukes carry NO forward link — every rank stays known and castable, which is what
    // makes vanilla down-ranking work, and what keeps normalization off them. Fireball r1/r2,
    // Frostbolt r1, Healing Touch r1, Renew r1, Immolate r1, Lightning Bolt r1.
    for id in [133u32, 143, 116, 5185, 139, 348, 403] {
        assert_eq!(cat.rank_successor(id), None, "spell {id} is not chained");
        let known: std::collections::BTreeSet<u32> = [id].into_iter().collect();
        assert_eq!(cat.highest_known_rank(id, &known), Some(id));
    }
    // A spell with no `SkillLineAbility` row at all (the auto-attack) is its own chain.
    let attack: std::collections::BTreeSet<u32> = [6603].into_iter().collect();
    assert_eq!(cat.rank_successor(6603), None);
    assert_eq!(cat.highest_known_rank(6603, &attack), Some(6603));
}

/// `KnownHigherRank` (`0x60c8d0`): walk the `forward_spellid` chain forward from the spell and
/// answer on the first known rank — the spell itself is not a "higher" rank of itself.
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
