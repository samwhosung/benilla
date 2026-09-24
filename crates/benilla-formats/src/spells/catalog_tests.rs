//! The spell catalog's column pins, checked end to end against the real 5875 `Spell.dbc`.

use super::*;

/// A trainer's learn wrapper is never in `SkillLineAbility`, so the tree groups it by the spell
/// it teaches: Heroic Strike 78 via 1605, Charge 100 via 1738, Battle Shout 6673 via 6674.
#[test]
fn real_learn_spell_hop_resolves_the_taught_ability() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let skills = crate::skill_lines::load_skill_line_catalog(&mut chain).expect("load skill lines");

    for (wrapper, ability, line) in [(1605u32, 78u32, 26u32), (1738, 100, 26), (6674, 6673, 256)] {
        assert_eq!(
            spells.learned_spell(wrapper),
            Some(ability),
            "learn wrapper {wrapper} teaches ability {ability}"
        );
        assert_eq!(
            skills.spell_to_line(wrapper),
            None,
            "the wrapper {wrapper} is not itself in SkillLineAbility (the bug's root)"
        );
        assert_eq!(
            skills.spell_to_line(ability),
            Some(line),
            "the taught ability {ability} groups under skill line {line}"
        );
        assert!(
            spells.get(ability).is_some_and(|d| !d.name.is_empty()),
            "the taught ability carries the display name"
        );
    }
}

/// Values from the vmangos `spell_template` rows.
#[test]
fn real_spell_catalog_reads_ranged_attributes() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Auto Shot: ranged (0x2) and auto-repeat (0x20); no visual, the wire ammo is the missile.
    let auto_shot = cat.get(75).expect("Auto Shot");
    assert_eq!(auto_shot.attributes, 0x50012);
    assert_eq!(auto_shot.attributes_ex2, 0x20);
    assert!(auto_shot.ranged_attack());
    assert_eq!(auto_shot.visual, 0, "the shot spells have no SpellVisual");
    assert_eq!(auto_shot.speed, 40.0);

    // Wand Shoot: the 0x18&0x2 side of the client gate, plus auto-repeat.
    let shoot = cat.get(5019).expect("Shoot (wand)");
    assert_eq!(shoot.attributes, 0x12);
    assert_eq!(shoot.attributes_ex2, 0x20);
    assert!(shoot.ranged_attack());

    // Throw: ranged but not auto-repeat, and still arms the ranged stance.
    let throw = cat.get(2764).expect("Throw");
    assert_eq!(throw.attributes, 0x410012);
    assert_eq!(throw.attributes_ex2, 0);
    assert!(throw.ranged_attack());

    // Fireball: neither bit.
    let fireball = cat.get(133).expect("Fireball");
    assert_eq!(fireball.attributes, 0x10000);
    assert_eq!(fireball.attributes_ex2, 0);
    assert!(!fireball.ranged_attack());

    // Effect[0] (column 61): 6603 "Attack" carries SPELL_EFFECT_ATTACK (78).
    let attack = cat.get(6603).expect("Attack");
    assert_eq!(
        attack.effects[0], 78,
        "6603 Effect[0] == SPELL_EFFECT_ATTACK"
    );
    assert!(attack.is_melee_auto_attack());
    assert!(!fireball.is_melee_auto_attack());
    assert!(
        !auto_shot.is_melee_auto_attack(),
        "Auto Shot is ranged (Effect 58), not melee"
    );
}

/// The buff bar's cache builder (`PlayerAuras_Update` `0x4e4170`) refuses `NO_AURA_ICON` and
/// `DO_NOT_DISPLAY` (0x80), reading `Attributes` as a byte (columns 6 and 7).
#[test]
fn real_spell_catalog_hides_stances_from_the_aura_bar() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Battle Stance carries NO_AURA_ICON | CAST_WHEN_LEARNED, the other two NO_AURA_ICON alone.
    let battle = cat.get(2457).expect("Battle Stance");
    assert_eq!(battle.attributes_ex, 0x9000_0000);
    assert!(battle.hidden_from_aura_bar());
    let defensive = cat.get(71).expect("Defensive Stance");
    assert_eq!(defensive.attributes_ex, 0x1000_0000);
    assert!(defensive.hidden_from_aura_bar());
    let berserker = cat.get(2458).expect("Berserker Stance");
    assert_eq!(berserker.attributes_ex, 0x1000_0000);
    assert!(berserker.hidden_from_aura_bar());

    // Defensive State 5302 rides a visible wire slot, but DO_NOT_DISPLAY keeps it off the bar.
    let def_state = cat.get(5302).expect("Defensive State");
    assert_eq!(def_state.attributes, 0x2000_0190);
    assert!(def_state.hidden_from_aura_bar());
    let def_state_dnd = cat.get(5301).expect("Defensive State (DND)");
    assert_eq!(def_state_dnd.attributes, 0x1d0);
    assert!(def_state_dnd.hidden_from_aura_bar());

    let shout = cat.get(6673).expect("Battle Shout");
    assert!(!shout.hidden_from_aura_bar());
    let fortitude = cat.get(1243).expect("Power Word: Fortitude");
    assert!(!fortitude.hidden_from_aura_bar());

    // The dword sign bit (`NO_AURA_CANCEL`) filters nothing: Echoes of Lordaeron still shows.
    let echoes = cat.get(1386).expect("Echoes of Lordaeron");
    assert_eq!(echoes.attributes, 0x8800_0100);
    assert!(!echoes.hidden_from_aura_bar());
}

#[test]
fn real_spell_catalog_reads_rank_and_passive() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Fireball's first three ranks; even rank 1 carries the literal "Rank 1".
    assert_eq!(cat.get(133).unwrap().rank.as_deref(), Some("Rank 1"));
    assert_eq!(cat.get(143).unwrap().rank.as_deref(), Some("Rank 2"));
    assert_eq!(cat.get(145).unwrap().rank.as_deref(), Some("Rank 3"));
    assert_eq!(cat.get(168).unwrap().rank.as_deref(), Some("Rank 1")); // Frost Armor
    assert_eq!(cat.get(172).unwrap().rank.as_deref(), Some("Rank 1")); // Corruption
    assert_eq!(cat.get(2136).unwrap().rank.as_deref(), Some("Rank 1")); // Fire Blast

    // None of the above is passive; a weapon skill (One-Handed Swords, 201) is.
    assert!(!cat.get(133).unwrap().passive);
    assert!(
        cat.get(201).unwrap().passive,
        "One-Handed Swords is passive"
    );
}

#[test]
fn real_spell_catalog_gates_the_spellbook() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Shown: Fireball's ranks, Frostbolt, Polymorph (0x80 clear, castUI 0).
    for id in [133, 143, 145, 116, 118] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert!(
            d.in_spellbook(),
            "spell {id} should show ({:#x})",
            d.attributes
        );
        assert_eq!(d.attributes & 0x80, 0, "spell {id} is not DO_NOT_DISPLAY");
        assert_eq!(d.cast_ui, 0, "an ordinary spell reads castUI 0");
    }

    // Hidden: a language, armor and weapon proficiencies, each `0xC0 = PASSIVE | DO_NOT_DISPLAY`.
    for (id, what) in [
        (668u32, "Language: Common"),
        (9078, "Cloth"),
        (9077, "Leather"),
        (196, "One-Handed Axes"),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id} {what}"));
        assert_eq!(
            d.attributes & 0xC0,
            0xC0,
            "{what} ({id}) is PASSIVE|DO_NOT_DISPLAY"
        );
        assert!(!d.in_spellbook(), "{what} ({id}) is hidden from the book");
    }
}

/// The OPEN_LOCK effect (column 61 == 0x21) and its `LockType` (`EffectMiscValue`, column 106): a
/// Copper Vein's `Lock.dbc` skill slot names LockType 3, which Mining (2575) opens.
#[test]
fn real_spell_catalog_reads_open_lock_types() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    assert_eq!(
        cat.get(2575).unwrap().open_lock_type(),
        Some(3),
        "Mining opens LockType 3"
    );
    assert_eq!(
        cat.get(2366).unwrap().open_lock_type(),
        Some(2),
        "Herb Gathering opens LockType 2"
    );
    assert_eq!(
        cat.get(1804).unwrap().open_lock_type(),
        Some(1),
        "Pick Lock opens LockType 1"
    );
    assert_eq!(
        cat.get(133).unwrap().open_lock_type(),
        None,
        "Fireball opens no lock"
    );

    // LockType 13 (lock 43, the ground containers) has two openers every character knows; the
    // resolver takes the first sufficient one in known-spell order, and the cast bar prints it.
    assert_eq!(cat.get(6478).unwrap().open_lock_type(), Some(13));
    assert_eq!(cat.get(6478).unwrap().name, "Opening");
    assert_eq!(cat.get(22810).unwrap().open_lock_type(), Some(13));
    assert_eq!(
        cat.get(22810).unwrap().name,
        "Opening - No Text",
        "the placeholder is Blizzard's own Spell.dbc string, not a formatting bug of ours"
    );

    // The cast bar shows no name for `AttributesEx3 & 0x4` (`SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`),
    // the one column separating 22810 from 6478; three shipped rows carry it.
    assert!(cat.get(22810).unwrap().no_casting_bar_text());
    assert!(!cat.get(6478).unwrap().no_casting_bar_text());
    let silent: Vec<u32> = {
        let mut v: Vec<u32> = cat
            .iter()
            .filter(|(_, d)| d.no_casting_bar_text())
            .map(|(id, _)| id)
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(
        silent,
        vec![6477, 22810, 26380],
        "6477 Opening / 22810 Opening - No Text / 26380 zzOLDSummon Mouth Tentacle Visual"
    );

    // The tool and reagent columns the pre-send possession check reads (`0x6e4000`): totems at
    // columns 40-41, reagents at 42-49 with their counts at 50-57.
    assert_eq!(
        cat.get(2575).unwrap().totems,
        [2901, 0],
        "Mining requires the Mining Pick (2901)"
    );
    assert_eq!(
        cat.get(8613).unwrap().totems,
        [7005, 0],
        "Skinning requires the Skinning Knife (7005)"
    );
    let slow_fall = cat.get(130).unwrap();
    assert_eq!(
        slow_fall.reagents[0],
        (17056, 1),
        "Slow Fall consumes one Light Feather (17056)"
    );
    assert_eq!(cat.get(133).unwrap().totems, [0, 0]);
}

/// The two `Effect[0]` values the client latches at learn time (`0x4b25e0` into `[0xb700e4]` and
/// `[0xb700e8]`); the cursor's skin leg shows the knife only when one of them is in the book.
#[test]
fn real_spell_catalog_pins_the_skin_latch_effects() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Skinning (8613): `0x4b2623: cmp [esi+0xf4], 0x5f`.
    assert_eq!(
        cat.get(8613).unwrap().effects[0],
        crate::SPELL_EFFECT_SKINNING,
        "Skinning carries SPELL_EFFECT_SKINNING (95 == 0x5f)"
    );
    // Remove Insignia (22027), the `[0xb700e8]` half: `0x4b2632: cmp [esi+0xf4], 0x74`.
    assert_eq!(
        cat.get(22027).unwrap().effects[0],
        0x74,
        "Remove Insignia carries SPELL_EFFECT_SKIN_PLAYER_CORPSE (116 == 0x74)"
    );
    // Nothing an ordinary caster starts with does, so a non-skinner's latch stays empty.
    assert_ne!(
        cat.get(133).unwrap().effects[0],
        crate::SPELL_EFFECT_SKINNING
    );
    assert_ne!(
        cat.get(6247).unwrap().effects[0],
        crate::SPELL_EFFECT_SKINNING
    );
}

/// The left side of the client's lock test (`0x5f850f`), with the player's skill as the level
/// term (`0x5ea690`). Its columns (27, 28, 64, 67, 70, 73, 76) are pinned by result, against
/// values known from the game; the below-cap rows are the ones a caster-level term would miss.
#[test]
fn real_spell_catalog_computes_the_lock_skill_an_opener_provides() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Pick Lock (1804): `4 + 1 + 5.0×(skill/5 − 1)`, the skill itself.
    assert_eq!(cat.get(1804).unwrap().open_lock_skill(300), Some(300));
    assert_eq!(cat.get(1804).unwrap().open_lock_skill(225), Some(225));
    assert_eq!(cat.get(1804).unwrap().open_lock_skill(150), Some(150));
    // Mining (2575), Herb Gathering (2366): `−1 + 1 + 5.0×(skill/5)`; 1 Mining provides 0.
    assert_eq!(cat.get(2575).unwrap().open_lock_skill(300), Some(300));
    assert_eq!(cat.get(2575).unwrap().open_lock_skill(100), Some(100));
    assert_eq!(cat.get(2575).unwrap().open_lock_skill(1), Some(0));
    assert_eq!(cat.get(2366).unwrap().open_lock_skill(300), Some(300));
    // Seaforium Charges (4056, 4075): a flat 150 and 250; lock 92 asks for Blasting 150.
    assert_eq!(cat.get(4056).unwrap().open_lock_skill(0), Some(150));
    assert_eq!(cat.get(4056).unwrap().open_lock_skill(300), Some(150));
    assert_eq!(cat.get(4075).unwrap().open_lock_skill(0), Some(250));
    // The "Opening"/"Closing" family every character knows is a flat 100, so the Action gate,
    // not the value test, keeps them off a padlocked door.
    for id in [3365, 6233, 6246, 6247, 6477, 6478, 21651, 21652] {
        assert_eq!(
            cat.get(id).unwrap().open_lock_skill(0),
            Some(100),
            "spell {id} is a flat-100 opener"
        );
        assert_eq!(cat.get(id).unwrap().open_lock_skill(300), Some(100));
    }
    assert_eq!(cat.get(133).unwrap().open_lock_skill(300), None);
}

/// Values from vmangos `spell_template`, each entry at its highest build up to 5875.
#[test]
fn real_spell_catalog_reads_cooldown_cost_and_range_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Fireball r1: no cooldown, the ordinary GCD (category 133, 1500 ms), 30 mana, range row 35.
    let fireball = cat.get(133).unwrap();
    assert_eq!(
        (
            fireball.category,
            fireball.recovery_ms,
            fireball.category_recovery_ms
        ),
        (0, 0, 0)
    );
    assert_eq!(
        (fireball.start_recovery_category, fireball.start_recovery_ms),
        (133, 1500)
    );
    assert_eq!((fireball.power_type, fireball.mana_cost), (0, 30));
    assert_eq!(fireball.range_index, 35);
    assert!(!fireball.cooldown_on_event());

    // Charge: category 44 with a 15 s category cooldown, rage (1), no GCD pair, range row 95.
    let charge = cat.get(100).unwrap();
    assert_eq!(
        (
            charge.category,
            charge.recovery_ms,
            charge.category_recovery_ms
        ),
        (44, 0, 15000)
    );
    assert_eq!(
        (charge.start_recovery_category, charge.start_recovery_ms),
        (0, 0)
    );
    assert_eq!(charge.power_type, 1, "Charge costs rage");
    assert_eq!(charge.range_index, 95);

    // Feign Death: a 30 s RecoveryTime and SPELL_ATTR_COOLDOWN_ON_EVENT (bit 25 of 0x2151400).
    let feign = cat.get(5384).unwrap();
    assert_eq!(feign.recovery_ms, 30_000);
    assert!(
        feign.cooldown_on_event(),
        "Feign Death is cooldown-on-event"
    );

    // Lay on Hands: the hour-long category cooldown (56 / 3_600_000).
    let loh = cat.get(633).unwrap();
    assert_eq!((loh.category, loh.category_recovery_ms), (56, 3_600_000));

    // ManaCostPercentage: Purge r1 (370) is 10, Dispel Magic r1 (527) 18.
    assert_eq!(cat.get(370).unwrap().mana_cost_pct, 10);
    assert_eq!(cat.get(527).unwrap().mana_cost_pct, 18);
}

/// [`COL_TARGETS`] (13) and [`COL_IMPLICIT_TARGET_A1`] (82): one row per `0x6e5250` arm or bit.
#[test]
fn real_spell_catalog_reads_cast_targeting_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Implicit targets: 6 single enemy (hostile bit), 1 self, 21 single friend (assist bit), 20
    // party around the caster (a no-op arm, so the mask stays 0).
    assert_eq!(cat.get(133).unwrap().implicit_target_a1, 6, "Fireball");
    assert_eq!(cat.get(7302).unwrap().implicit_target_a1, 1, "Ice Armor");
    assert_eq!(cat.get(5384).unwrap().implicit_target_a1, 1, "Feign Death");
    assert_eq!(
        cat.get(1459).unwrap().implicit_target_a1,
        21,
        "Arcane Intellect"
    );
    assert_eq!(
        cat.get(6673).unwrap().implicit_target_a1,
        20,
        "Battle Shout"
    );

    // The `Targets` seed mask: 0 for ordinary casts; Resurrection carries the corpse-ally bit 15,
    // Skinning unit bit 1 and the requires-explicit-selection bit 10.
    assert_eq!(cat.get(133).unwrap().targets, 0);
    assert_eq!(cat.get(6673).unwrap().targets, 0);
    assert_eq!(cat.get(2006).unwrap().targets, 0x8000, "Resurrection");
    assert_eq!(cat.get(8613).unwrap().targets, 0x402, "Skinning");
}

/// The usable walk's columns (`0x6e3d60`), one row per gate family, and the real form flags.
#[test]
fn real_spell_catalog_reads_usable_walk_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let forms = load_shapeshift_forms(&mut chain).expect("load SpellShapeshiftForm");

    // Claw: cat form (1). Ambush: stealth (form 30), a dagger and the only-stealthed attribute.
    // Execute: battle or berserker stance and the target's below-20% aura state. Revenge: the
    // caster's defense state.
    let claw = cat.get(1082).unwrap();
    assert_eq!(claw.stances, 0x1);
    let ambush = cat.get(8676).unwrap();
    assert_eq!(ambush.stances, 0x2000_0000);
    assert_ne!(ambush.attributes & ATTR_ONLY_STEALTHED, 0);
    assert_eq!(
        (
            ambush.equipped_item_class,
            ambush.equipped_item_subclass_mask
        ),
        (2, 0x8000)
    );
    let execute = cat.get(5308).unwrap();
    assert_eq!((execute.stances, execute.target_aura_state), (0x50000, 2));
    assert_eq!(cat.get(6572).unwrap().caster_aura_state, 1, "Revenge");

    // The combo-point gate (`0x6e3e7a`): Overpower has no aura state; its window rides
    // `AttributesEx` bit 20 (`FINISHING_MOVE_DAMAGE`) like the finishers.
    for rank in [7384, 7887, 11584, 11585] {
        let op = cat.get(rank).unwrap();
        assert!(op.needs_combo_points(), "Overpower {rank}");
        assert_eq!(
            (op.caster_aura_state, op.target_aura_state),
            (0, 0),
            "Overpower {rank} has no aura-state gate — the combo-point gate is all that holds it"
        );
    }
    for finisher in [
        2098, /* Eviscerate */
        1943, /* Rupture */
        5171, /* Slice and Dice */
    ] {
        assert!(
            cat.get(finisher).unwrap().needs_combo_points(),
            "{finisher}"
        );
    }
    for plain in [
        78,   /* Heroic Strike */
        5308, /* Execute */
        6572, /* Revenge */
    ] {
        assert!(!cat.get(plain).unwrap().needs_combo_points(), "{plain}");
    }

    // Auto Shot: bows/guns/crossbows. Slow Fall: one Light Feather.
    let auto_shot = cat.get(75).unwrap();
    assert_eq!(
        (
            auto_shot.equipped_item_class,
            auto_shot.equipped_item_subclass_mask
        ),
        (2, 0x4000c)
    );
    assert_eq!(cat.get(130).unwrap().reagents[0], (17056, 1), "Slow Fall");

    // Battle Stance (17) is a stance (flags1 bit 0), Cat Form (1) a true shapeshift.
    assert!(forms.get(&17).unwrap().is_stance());
    assert!(!forms.get(&1).unwrap().is_stance());
    assert_eq!(forms.get(&1).unwrap().bonus_bar, 1);

    // Claw in cat only; Fireball unshifted and in a stance, not in Cat Form; Execute in Battle
    // (17), not in Defensive (18).
    assert!(claw.usable_in_form(1, false));
    assert!(!claw.usable_in_form(0, false));
    let fireball = cat.get(133).unwrap();
    assert!(fireball.usable_in_form(0, false));
    assert!(fireball.usable_in_form(17, true));
    assert!(!fireball.usable_in_form(1, false));
    assert!(execute.usable_in_form(17, true));
    assert!(!execute.usable_in_form(18, true));

    // Ghost Wolf: form 16 is a cancelable true shapeshift; the spell carries NOT_SHAPESHIFT (bit
    // 16) and the stance-bar exclusion (`AttributesEx2 & 0x2`: a shaman gets no stance bar). In
    // the form an ordinary spell refuses 0x3d; a form spell out of its form refuses 0x56.
    let ghost_wolf = cat.get(2645).unwrap();
    assert_eq!(ghost_wolf.shapeshift_form, Some(16));
    assert_ne!(ghost_wolf.attributes & 0x1_0000, 0);
    assert_ne!(ghost_wolf.attributes_ex2 & 0x2, 0);
    let wolf_row = forms.get(&16).unwrap();
    assert!(!wolf_row.is_stance());
    assert!(wolf_row.cancelable());
    use crate::FormRefusal;
    let bolt = cat.get(403).unwrap();
    assert_eq!(
        bolt.form_refusal(16, false),
        Some(FormRefusal::NotShapeshift)
    );
    assert_eq!(bolt.form_refusal(0, false), None);
    assert_eq!(
        claw.form_refusal(0, false),
        Some(FormRefusal::OnlyShapeshift)
    );
    assert_eq!(
        ghost_wolf.form_refusal(16, false),
        Some(FormRefusal::NotShapeshift),
        "re-pressing Ghost Wolf in the form draws the gate too (the cancel is a separate branch)"
    );
    assert_eq!(FormRefusal::NotShapeshift.reason(), 0x3d);
    assert_eq!(FormRefusal::OnlyShapeshift.reason(), 0x56);

    // The active-action toggle (`0x4e563c`): Ghost Wolf's nonzero ActiveIconID lets a second
    // press cancel it; Battle Stance's 0 keeps a stance up on the plain paths.
    assert_ne!(ghost_wolf.active_icon_id, 0);
    assert_eq!(cat.get(2457).unwrap().active_icon_id, 0, "Battle Stance");

    // AttackIconID (column 13, `0x4e6870`): Cat Form has its own, Ghost Wolf's 0 means the weapon.
    assert_eq!(
        forms.get(&1).unwrap().attack_icon.as_deref(),
        Some("Interface\\Icons\\Ability_Druid_CatFormAttack")
    );
    assert_eq!(wolf_row.attack_icon, None);
}

#[test]
fn real_spell_catalog_reads_tooltip_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let cast_times = load_spell_cast_times(&mut chain).expect("load SpellCastTimes");
    let durations = load_spell_durations(&mut chain).expect("load SpellDuration");

    // Fireball r1 has a real 4 s damage-over-time tail ("over $d"), a 2 s tick in effect slot 1.
    let fireball = cat.get(133).unwrap();
    assert!(
        fireball
            .description
            .as_deref()
            .unwrap()
            .starts_with("Hurls a fiery ball that causes"),
        "Fireball description: {:?}",
        fireball.description
    );
    assert_eq!(fireball.casting_time_index, 16);
    assert_eq!(
        cast_times.get(16).unwrap().base_ms,
        1500,
        "Fireball's real cast time"
    );
    assert_eq!(fireball.duration_index, 35);
    assert_eq!(
        durations.get(35).unwrap().base_ms,
        4000,
        "Fireball's DoT-tail duration, not zero — it genuinely has one"
    );
    assert_eq!(
        fireball.proc_chance, 101,
        "vmangos's always-triggers sentinel"
    );
    assert_eq!(
        (fireball.effect_base_points[0], fireball.effect_die_sides[0]),
        (13, 9),
        "Fireball r1's direct-damage roll: 14-22"
    );
    assert_eq!(
        (fireball.effect_apply_aura[1], fireball.effect_amplitude[1]),
        (3, 2000),
        "Fireball's periodic-damage tail: SPELL_AURA_PERIODIC_DAMAGE ticking every 2s"
    );

    // Frost Armor: 30 minutes, instant, its description naming the chill proc 6136.
    let frost_armor = cat.get(168).unwrap();
    assert!(frost_armor
        .aura_description
        .as_deref()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(frost_armor.duration_index, 30);
    assert_eq!(durations.get(30).unwrap().base_ms, 1_800_000, "30 minutes");
    assert_eq!(frost_armor.casting_time_index, 1);
    assert_eq!(cast_times.get(1).unwrap().base_ms, 0, "instant");
    assert_eq!(
        frost_armor.effect_apply_aura[0], 22,
        "SPELL_AURA_MOD_RESISTANCE"
    );
    assert_eq!(
        frost_armor.effect_trigger_spell[1], 6136,
        "the chill proc the description text itself names"
    );

    // Fire Blast has no aura: no aura text and no duration row.
    let fire_blast = cat.get(2136).unwrap();
    assert_eq!(fire_blast.aura_description, None);
    assert_eq!(fire_blast.duration_index, 0);
    assert!(durations.get(0).is_none(), "no row 0 in SpellDuration.dbc");

    // Auto Shot and Feign Death: the signed EffectBasePoints sentinel -1, no fixed roll.
    assert_eq!(cat.get(75).unwrap().effect_base_points[0], -1, "Auto Shot");
    assert_eq!(
        cat.get(5384).unwrap().effect_base_points[0],
        -1,
        "Feign Death"
    );
}

/// The attack-start masks, rows from vmangos: [`SpellDisplay::on_next_swing`] (`0x404`),
/// [`SpellDisplay::initiates_auto_attack`] (adding `AttributesEx & 0x200`) and
/// [`SpellDisplay::initiates_auto_attack_at_go`] (`AttributesEx2 & 0x100000`).
#[test]
fn real_spell_catalog_classifies_combat_initiation() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // (spell, on_next_swing, initiates_auto_attack, initiates_auto_attack_at_go)
    for (id, name, next_swing, initiates, at_go) in [
        (78u32, "Heroic Strike", true, true, false), // Attributes 0x50014
        (845, "Cleave", true, true, false),          // Attributes 0x50014, Ex 0x200
        (2973, "Raptor Strike", true, true, false),  // Attributes 0x50404
        (772, "Rend", false, true, false),           // Ex 0x8000200
        (7386, "Sunder Armor", false, true, false),  // Ex 0x8000200
        (1464, "Slam", false, true, false),          // Ex 0x8000200
        (100, "Charge", false, false, false),        // Ex 0x400: neither bit, bit 20 clear
        (6673, "Battle Shout", false, false, false), // Ex 0x0
        (6603, "Attack", false, false, false),       // the auto-attack pseudo-spell itself
        (133, "Fireball", false, false, false),      // an ordinary cast
        // The GO-deferred class: bit 20 set, so the attack starts at `SMSG_SPELL_GO` (`0x6e83c0`).
        (53, "Backstab", false, false, true), // Attributes 0x50010, Ex 0x8000200, Ex2 0x100000
        (703, "Garrote", false, false, true),
        (8676, "Ambush", false, false, true),
        (1833, "Cheap Shot", false, false, true),
        (5221, "Shred", false, false, true),
        (6785, "Ravage", false, false, true),
        (9005, "Pounce", false, false, true),
        (20271, "Judgement", false, false, true), // Ex 0x0: bit 20 is its only initiation bit
        // The hunter's instant shots carry `AttributesEx2` bit 17 (`DO_NOT_RESET_COMBAT_TIMERS`),
        // not bit 20, and no other initiation bit, so neither attack start fires for them.
        (1978, "Serpent Sting", false, false, false),
        (3044, "Arcane Shot", false, false, false),
        (2643, "Multi-Shot", false, false, false),
        (75, "Auto Shot", false, false, false), // the auto-repeat itself: Ex2 0x20, not 0x100000
    ] {
        let d = cat
            .get(id)
            .unwrap_or_else(|| panic!("{name} ({id}) in the catalog"));
        assert_eq!(
            d.on_next_swing(),
            next_swing,
            "{name} ({id}) on_next_swing (Attributes {:#x})",
            d.attributes
        );
        assert_eq!(
            d.initiates_auto_attack(),
            initiates,
            "{name} ({id}) initiates_auto_attack (Attributes {:#x}, Ex {:#x}, Ex2 {:#x})",
            d.attributes,
            d.attributes_ex,
            d.attributes_ex2
        );
        assert_eq!(
            d.initiates_auto_attack_at_go(),
            at_go,
            "{name} ({id}) initiates_auto_attack_at_go (Ex2 {:#x})",
            d.attributes_ex2
        );
        // Bit 20 suppresses the send-time start, so no spell starts the attack twice.
        assert!(
            !(d.initiates_auto_attack() && d.initiates_auto_attack_at_go()),
            "{name} ({id}) may not start the auto-attack at BOTH the send and the GO"
        );
    }

    // 36 shipped rows carry bit 20, every one an opener, a positional strike or Judgement.
    let deferred: Vec<(u32, &str)> = cat
        .iter()
        .filter(|(_, d)| d.initiates_auto_attack_at_go())
        .map(|(id, d)| (id, d.name.as_str()))
        .collect();
    assert_eq!(
        deferred.len(),
        36,
        "5875 ships 36 INITIATE_COMBAT_POST_CAST rows, got {}: {:?}",
        deferred.len(),
        deferred
    );
    let mut names: Vec<&str> = deferred.iter().map(|&(_, n)| n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names,
        [
            "Ambush",
            "Backstab",
            "Cheap Shot",
            "Garrote",
            "Judgement",
            "Pounce",
            "Ravage",
            "Shred",
            "Test Stab R50",
        ],
        "the deferred-start class is the openers, the positional strikes and Judgement"
    );
}

/// `modalNextSpell`, column 38, which `0x6e7447` reads to start Auto Shot after a hunter shot:
/// nonzero on 57 of 22357 rows, 52 of them naming spell 75.
#[test]
fn real_spell_catalog_reads_modal_next_spell() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Rank 1 of each hunter shot.
    for (id, name, next) in [
        (1978u32, "Serpent Sting", 75u32),
        (3044, "Arcane Shot", 75),
        (2643, "Multi-Shot", 75),
        (5116, "Concussive Shot", 75),
        (19434, "Aimed Shot", 75),
        (3034, "Viper Sting", 75),
        (3043, "Scorpid Sting", 75),
        (3674, "Black Arrow", 75),
        (14274, "Distracting Shot", 75),
        // Auto Shot's own column 38 is 0: the chain is one hop and cannot loop.
        (75, "Auto Shot", 0),
        // A melee strike and an ordinary cast carry nothing here.
        (78, "Heroic Strike", 0),
        (133, "Fireball", 0),
    ] {
        let d = cat
            .get(id)
            .unwrap_or_else(|| panic!("{name} ({id}) in the catalog"));
        assert_eq!(
            d.modal_next_spell, next,
            "{name} ({id}) modalNextSpell (column 38)"
        );
    }

    let chained: Vec<(u32, u32)> = cat
        .iter()
        .filter(|(_, d)| d.modal_next_spell != 0)
        .map(|(id, d)| (id, d.modal_next_spell))
        .collect();
    assert_eq!(
        chained.len(),
        57,
        "5875 ships 57 rows with a non-zero column 38, got {}",
        chained.len()
    );
    let to_auto_shot = chained.iter().filter(|&&(_, next)| next == 75).count();
    assert_eq!(
        to_auto_shot, 52,
        "52 of the 57 name Auto Shot (the rest: three (TEST) bow shot rows → 59, two Minigun → 23675)"
    );
    // Nothing points at a spell that chains onward: one hop, in data.
    for &(id, next) in &chained {
        let onward = cat.get(next).map(|d| d.modal_next_spell).unwrap_or(0);
        assert!(
            onward == 0 || onward == next,
            "spell {id} chains {next}, which itself chains {onward} — the chain must not walk"
        );
    }
}

/// `EffectItemType` (columns 103-105) and `RequiresSpellFocus` (15); values from vmangos.
#[test]
fn real_crafting_columns_read_created_item_and_focus() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // (recipe spell, created item, focus): Bolt of Linen Cloth, Minor Healing Potion,
    // Copper Axe, Crafted Light Shot, Charred Wolf Meat.
    for (spell, item, focus) in [
        (2963u32, 2996u32, 0u32),
        (2330, 118, 0),
        (2738, 2845, 1), // Blacksmithing needs the Anvil (focus 1)
        (3920, 8067, 0),
        (2538, 2679, 4), // Cooking needs a Cooking Fire (focus 4)
    ] {
        let d = cat.get(spell).expect("recipe in the catalog");
        assert_eq!(
            d.effects[0], SPELL_EFFECT_CREATE_ITEM,
            "spell {spell} creates"
        );
        assert_eq!(d.effect_item_type[0], item, "spell {spell} created item");
        assert_eq!(d.requires_spell_focus, focus, "spell {spell} focus");
    }

    // Crafted Light Shot makes 200 a craft: BasePoints 199 plus DieSides 1.
    let shots = cat.get(3920).expect("Crafted Light Shot");
    assert_eq!(shots.effect_base_points[0], 199);
    assert_eq!(shots.effect_die_sides[0], 1);

    // The openers carry effect 47 and no product: Tailoring 3908, Enchanting 7411.
    for opener in [3908u32, 7411] {
        let d = cat.get(opener).expect("opener in the catalog");
        assert_eq!(d.effects[0], SPELL_EFFECT_TRADE_SKILL, "opener {opener}");
        assert_eq!(
            d.effect_item_type, [0; 3],
            "opener {opener} creates nothing"
        );
    }
}

#[test]
fn the_tooltip_gates_read_effect_and_mask() {
    // `0x52eb15`: the cast and cooldown line is omitted for the passive bit or Effect[0] 47 or 78.
    let plain = SpellDisplay::default();
    assert!(!plain.tooltip_omits_cast_line());
    let attribute_passive = SpellDisplay {
        passive: true,
        ..Default::default()
    };
    assert!(attribute_passive.tooltip_omits_cast_line());
    // 6603 "Attack"'s shape: Effect[0] 78, attributes 0x10, not the passive bit.
    let auto_attack = SpellDisplay {
        effects: [78, 0, 0],
        attributes: 0x10,
        ..Default::default()
    };
    assert!(!auto_attack.passive, "the attribute bit is clear");
    assert!(auto_attack.tooltip_omits_cast_line(), "the Effect[0] leg");
    let trade_skill = SpellDisplay {
        effects: [47, 0, 0],
        ..Default::default()
    };
    assert!(trade_skill.tooltip_omits_cast_line());
}

/// The main-hand auto-pick family (`Attributes & 0x200`): `ArmCast` (`0x6e5250`) binds the
/// equipped main hand instead of arming the item cursor. vmangos keeps a row per build, so a
/// `GROUP BY entry` also returns pre-5302 Feedback and Omen of Clarity rows and counts 28.
#[test]
fn real_main_hand_autopick_family() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    let mut names = std::collections::BTreeSet::<String>::new();
    let mut rows = 0usize;
    for (id, d) in cat.iter().filter(|(_, d)| d.targets_main_hand_item()) {
        rows += 1;
        names.insert(d.name.clone());
        assert_eq!(
            d.targets, 0x10,
            "spell {id} ({}) carries the main-hand attribute with word {:#x} — the resolver's \
             item arm cannot discharge it",
            d.name, d.targets
        );
        assert_eq!(
            d.effects[0],
            crate::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY,
            "spell {id} ({}) is not a temporary weapon enchant",
            d.name
        );
        // The auto-pick bypasses `0x495d60`, so an equipped-item gate here would never run.
        assert_eq!(
            (
                d.equipped_item_subclass_mask,
                d.equipped_item_inventory_type_mask
            ),
            (0, 0),
            "spell {id} ({}) carries an equipped-item gate the auto-pick path never runs",
            d.name
        );
    }
    assert_eq!(rows, 22, "the whole main-hand auto-pick family");
    assert_eq!(
        names.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "Flametongue Weapon",
            "Frostbrand Weapon",
            "Rockbiter Weapon",
            "Windfury Weapon",
        ],
        "in 5875 the family is the four shaman weapon imbues and nothing else"
    );
    // Windfury Totem is not in it: a totem summon with no target word at all.
    let wf_totem = cat.get(8512).expect("Windfury Totem");
    assert!(!wf_totem.targets_main_hand_item());
    assert_eq!(wf_totem.targets, 0, "a totem summon binds nothing");
}

/// `0x495d60`'s third effect leg (`0x495df1 jne 0x495f36` into `0x495f39`, effect `0x7f`) holds
/// the only raises of `SPELL_FAILED_PROSPECT_NEED_MORE` (`0x49614e`) and `SPELL_FAILED_MIN_SKILL`
/// (`0x496128`); vmangos sends neither, so with no shipped [`SPELL_EFFECT_PROSPECTING`] spell
/// neither message can appear.
#[test]
fn real_prospecting_effect_is_absent_from_5875() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let raw = chain.read_file(SPELL).expect("Spell.dbc");
    let set = parse(&raw, spell_schema(), "Spell.dbc").expect("parse Spell.dbc");

    let mut slots = 0usize;
    for r in set.records() {
        for i in 0..3 {
            slots += 1;
            assert_ne!(
                u32_at(r, COL_EFFECT_1 + i).unwrap_or(0),
                crate::SPELL_EFFECT_PROSPECTING,
                "spell {} carries SPELL_EFFECT_PROSPECTING in effect slot {i} — `0x495d60`'s \
                 third leg is reachable now, so cast-fail reasons 0x84 PROSPECT_NEED_MORE and \
                 0x90 MIN_SKILL can raise and `ui_action::cast_fail` owes them argument arms \
                 (decision 2292)",
                u32_at(r, 0).unwrap_or(0)
            );
        }
    }
    // A scan that read nothing would pass too, so pin the row count.
    assert_eq!(
        slots,
        22357 * 3,
        "every effect slot of every 5875 row was read"
    );
}

/// `TargetingWantsItem` (`0x6e6330`) is `flag_word & 0x4010`, bits that never mix with a unit
/// bit on shipped data: `Targets` is exactly `0x10` (enchants, poisons, stones, scopes) or
/// `0x4000` (OPEN_LOCK), so the resolver forks on the bare word. `0x495d60` reads the gate columns.
#[test]
fn real_item_target_family_and_its_gate_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // The unit-shaped bits of the flag_word (`cast_target`'s `UNIT_BITS`).
    const UNIT_BITS: u32 = 0x0002 | 0x0004 | 0x0008 | 0x0080 | 0x0100 | 0x0200 | 0x0400 | 0x8000;
    let mut item_only = 0usize;
    let mut locked_only = 0usize;
    for (id, d) in cat.iter() {
        if d.targets & 0x4010 == 0 {
            continue;
        }
        assert_eq!(
            d.targets & UNIT_BITS,
            0,
            "spell {id} mixes an item bit with a unit bit — the resolver's fork would be wrong"
        );
        match d.targets {
            0x0010 => item_only += 1,
            0x4000 => locked_only += 1,
            other => panic!("spell {id}: unexpected item-family word {other:#x}"),
        }
    }
    assert_eq!(
        item_only, 363,
        "Targets == 0x10 — the enchant/poison family"
    );
    assert_eq!(locked_only, 103, "Targets == 0x4000 — the OPEN_LOCK family");

    // The OPEN_LOCK implicit arm: `cast_target_mask` ORs `0x800` for arm 23 and `TF_UNIT` for arm
    // 25, so a lock word is `0x4000` or `0x4800`, nonzero under both `& 0x4010` and `& 0x4800`,
    // and one armed cursor answers both the bag click and the world click.
    let mut arms = std::collections::BTreeMap::<u32, usize>::new();
    for (_, d) in cat.iter().filter(|(_, d)| d.targets == 0x4000) {
        *arms.entry(d.implicit_target_a1).or_default() += 1;
    }
    assert_eq!(
        arms.values().sum::<usize>(),
        103,
        "every OPEN_LOCK row is counted"
    );
    assert_eq!(
        arms.get(&25),
        Some(&1),
        "one row arms 25 (its overlay is TF_UNIT, not the GameObject bit)"
    );
    assert!(
        arms.get(&23).copied().unwrap_or(0) >= 100,
        "the family is overwhelmingly arm 23, whose overlay is TARGET_FLAG_GAMEOBJECT: {arms:?}"
    );
    // Bit 11 comes only from the implicit arm, never from the `Targets` column.
    assert_eq!(
        cat.iter().filter(|(_, d)| d.targets & 0x800 != 0).count(),
        0,
        "TARGET_FLAG_GAMEOBJECT never appears in the Targets column"
    );

    // Bracer enchant: armor (4), any subclass, InventoryType WRIST(9) only.
    let bracer = cat.get(7418).unwrap();
    assert_eq!(bracer.targets, 0x10);
    assert_eq!(bracer.equipped_item_class, 4);
    assert_eq!(bracer.equipped_item_inventory_type_mask, 1 << 9);
    // Chest enchant: CHEST(5) or ROBE(20), so a cloth robe is legal.
    let chest = cat.get(7443).unwrap();
    assert_eq!(
        chest.equipped_item_inventory_type_mask,
        (1 << 5) | (1 << 20)
    );
    // A weapon-side row gates on class+subclass and leaves the type mask alone.
    let poison = cat.get(8679).unwrap();
    assert_eq!(
        (
            poison.equipped_item_class,
            poison.equipped_item_subclass_mask,
            poison.equipped_item_inventory_type_mask
        ),
        (2, 0x2a5f3, 0)
    );

    // The reference walks all three effect slots for an enchant (`0x495de4`-`0x496050`), while
    // `SpellDisplay` keeps slot 0: no item-target row has its enchant elsewhere. Read raw.
    let raw = chain.read_file(SPELL).expect("Spell.dbc");
    let set = parse(&raw, spell_schema(), "Spell.dbc").expect("parse Spell.dbc");
    let is_enchant = |e: u32| {
        e == crate::SPELL_EFFECT_ENCHANT_ITEM || e == crate::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY
    };
    for r in set.records() {
        if u32_at(r, COL_TARGETS).unwrap_or(0) != 0x10 {
            continue;
        }
        let effects: Vec<u32> = (0..3)
            .map(|i| u32_at(r, COL_EFFECT_1 + i).unwrap_or(0))
            .collect();
        assert!(
            is_enchant(effects[0]) || !(is_enchant(effects[1]) || is_enchant(effects[2])),
            "spell {} hides its enchant effect outside slot 0 ({effects:?}) — the one-slot read \
             in `spell::targeting::item_bind_verdict` would miss it",
            u32_at(r, 0).unwrap_or(0)
        );
    }
}

/// `GetCooldownInfo` (`0x6e13e0`): the `SpellCategory` flags `0x2` wildcard is wand Shoot's
/// category 351, the whole-bar swing sweep the cooldown store's wildcard leg implements.
#[test]
fn gcd_wildcard_and_shape_corners_hold_on_the_real_data() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("catalog");

    let shoot = cat.get(5019).expect("wand Shoot");
    assert_eq!(shoot.category, 351);
    assert!(
        shoot.category_wildcard,
        "Shoot's category row carries flags&2"
    );
    for &(id, name) in &[
        (133u32, "Fireball"),
        (100, "Charge"),
        (6673, "Battle Shout"),
    ] {
        let d = cat.get(id).expect(name);
        assert!(
            !d.category_wildcard,
            "{name}'s category must not read wildcard"
        );
    }

    // Scroll of Armor's spell: the {cat≠0, time=0} shape, which a GCD locks, since the pressed
    // spell's own time is never consulted.
    let scroll = cat.get(8091).expect("Scroll of Armor's spell");
    assert_eq!(
        (scroll.start_recovery_category, scroll.start_recovery_ms),
        (133, 0)
    );
}

/// The health power type is -2, read as `0xFFFFFFFE`.
#[test]
fn real_spell_catalog_cost_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");

    // Life Tap, every rank: health type and no cost at all, so the cost cell stays empty; 1.12
    // states the trade in the description.
    for id in [1454u32, 1455, 1456, 11687, 11688, 11689] {
        let d = cat.get(id).unwrap();
        assert_eq!(
            (
                d.power_type,
                d.mana_cost,
                d.mana_cost_pct,
                d.mana_per_second
            ),
            (0xFFFF_FFFE, 0, 0, 0),
            "Life Tap {id}"
        );
    }
    // Bloodrage: a percentage-only health cost.
    let bloodrage = cat.get(2687).unwrap();
    assert_eq!(
        (
            bloodrage.power_type,
            bloodrage.mana_cost,
            bloodrage.mana_cost_pct
        ),
        (0xFFFF_FFFE, 0, 20)
    );
    // Health Funnel: flat health plus per second (the `_PER_TIME` line), and channeled.
    let funnel = cat.get(755).unwrap();
    assert_eq!(
        (funnel.power_type, funnel.mana_cost, funnel.mana_per_second),
        (0xFFFF_FFFE, 11, 5)
    );
    assert!(funnel.tooltip_channeled());
    // The cast cell's attribute arms: Heroic Strike (next melee; 150 on the wire is "15 Rage"),
    // Auto Shot and Throw (the ranged bit alone, so Throw reads "Attack speed"), Mind Flay
    // (channeled), Judgement (percentage-only mana, shown as the resolved number).
    let hs = cat.get(78).unwrap();
    assert_eq!((hs.power_type, hs.mana_cost), (1, 150));
    assert!(hs.on_next_swing());
    assert!(cat.get(75).unwrap().tooltip_on_next_ranged(), "Auto Shot");
    assert!(cat.get(2764).unwrap().tooltip_on_next_ranged(), "Throw");
    let mf = cat.get(15407).unwrap();
    assert!(mf.tooltip_channeled());
    assert_eq!((mf.power_type, mf.mana_cost), (0, 45));
    let judgement = cat.get(20271).unwrap();
    assert_eq!(
        (
            judgement.power_type,
            judgement.mana_cost,
            judgement.mana_cost_pct
        ),
        (0, 0, 6)
    );

    // `manaPerSecondPerLevel` (column 35) is zero on every row, so it stays unparsed;
    // `manaCostPerlevel` (33) is nonzero on 72 creature spells, so `power_cost` applies it though
    // no player tooltip shows it.
    let bytes = chain.read_file(super::SPELL).expect("reading Spell.dbc");
    let rs = super::parse(&bytes, super::spell_schema(), "Spell.dbc").expect("Spell.dbc");
    let mut per_level_rows = 0u32;
    for r in rs.records() {
        let id = super::u32_at(r, 0).unwrap_or(0);
        if super::u32_at(r, 33).unwrap_or(0) > 0 {
            per_level_rows += 1;
        }
        assert_eq!(
            super::u32_at(r, 35).unwrap_or(0),
            0,
            "manaPerSecondPerLevel on {id}"
        );
    }
    assert_eq!(per_level_rows, 72, "the manaCostPerlevel population");
    let dark_offering = cat.get(7154).unwrap();
    assert_eq!(
        (
            dark_offering.power_type,
            dark_offering.mana_cost,
            dark_offering.mana_cost_per_level,
            dark_offering.spell_level
        ),
        (0xFFFF_FFFE, 180, 9, 24),
        "Dark Offering: the per-level term's health-lane row"
    );
}

/// A chat language reaches its skill only through the spell whose `Effect_1` is
/// `SPELL_EFFECT_LANGUAGE` (`0x4b2656`). On 5875, 813-817 all declare Common (7), so Demonic,
/// Titan, Thalassian and Kalimag garble for everyone, and a language-to-spell map would resolve
/// Common to Old Tongue; vmangos's `lang_description[]` (`ObjectMgr.cpp:84`) gives them their own.
#[test]
fn the_language_declaring_spells_cover_nine_of_thirteen_languages() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let skills = crate::skill_lines::load_skill_line_catalog(&mut chain).expect("load skill lines");

    // (spell, declared language, SkillLine.dbc id): every SPELL_EFFECT_LANGUAGE row but 25674.
    const DECLARED: &[(u32, u32, u32)] = &[
        (668, 7, 98),     // Common
        (669, 1, 109),    // Orcish
        (670, 3, 115),    // Taurahe
        (671, 2, 113),    // Darnassian
        (672, 6, 111),    // Dwarvish
        (813, 7, 137),    // "Thalassian (NYI)", declaring Common
        (814, 7, 138),    // "Draconic (NYI)", declaring Common
        (815, 7, 139),    // Demon Tongue, declaring Common
        (816, 7, 140),    // "Titan (NYI)", declaring Common
        (817, 7, 141),    // "Old Tongue (NYI)", declaring Common
        (7340, 13, 313),  // Gnomish
        (7341, 14, 315),  // Troll
        (17737, 33, 673), // Gutterspeak
    ];

    // The fourteenth, "Lesser Draconic (Language)", declares Draconic (11) but has no
    // `SkillLineAbility` row, so the gate's second hop dead-ends and Draconic always garbles.
    assert_eq!(spells.declared_language(25674), Some(11));
    assert_eq!(skills.spell_to_line(25674), None);

    assert_eq!(
        spells.declared_languages().count(),
        DECLARED.len() + 1,
        "every SPELL_EFFECT_LANGUAGE row is accounted for (the +1 is 25674, below)"
    );
    for &(spell, language, line) in DECLARED {
        assert_eq!(
            spells.declared_language(spell),
            Some(language),
            "spell {spell} -> language"
        );
        // The second hop the garble gate walks: spell -> SkillLineAbility -> SkillLine.dbc id.
        assert_eq!(
            skills.spell_to_line(spell),
            Some(line),
            "spell {spell} -> skill line"
        );
    }

    // Both hops walked, only the eight player languages can ever be understood.
    let understandable: std::collections::BTreeSet<u32> = spells
        .declared_languages()
        .filter(|(spell, _)| skills.spell_to_line(*spell).is_some())
        .map(|(_, l)| l)
        .collect();
    assert_eq!(
        understandable,
        [1, 2, 3, 6, 7, 13, 14, 33].into_iter().collect(),
        "only the eight player languages have both a declaring spell and a skill line"
    );

    // An ordinary ability declares nothing.
    assert_eq!(spells.declared_language(133), None); // Fireball
}

/// [`SpellDisplay::is_harmful`], the client's `0x6ea280 == 2`: an enemy implicit target in slot A
/// (6), or only in slot B as Frost Nova's (A 22, the caster's spot; B 15, a source-area enemy).
#[test]
fn real_is_harmful_pins() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let row = |id: u32| {
        spells
            .get(id)
            .unwrap_or_else(|| panic!("spell {id} in the catalog"))
    };
    for (id, name) in [
        (133u32, "Fireball"),
        (100, "Charge"),
        (7386, "Sunder Armor"),
        (122, "Frost Nova"),
    ] {
        assert!(row(id).is_harmful(), "{name} ({id}) targets enemies");
    }
    assert_eq!(
        (
            row(122).effect_implicit_target_a[0],
            row(122).effect_implicit_target_b[0]
        ),
        (22, 15),
        "Frost Nova is harmful through its B slot, the A slot being the caster's own spot"
    );
    for (id, name) in [
        (139u32, "Renew"),
        (5185, "Healing Touch"),
        (1459, "Arcane Intellect"),
        (6673, "Battle Shout"),
    ] {
        assert!(
            !row(id).is_harmful(),
            "{name} ({id}) does not target enemies"
        );
    }
}

/// `SpellChannelStart` (`0x6e7550`): unlike the cast bar, the channel bar says "Channeling" unless
/// a bit lets it name the spell, as 9 of the 323 channeled rows do; 2 suppress the bar.
#[test]
fn real_channel_bar_name_law() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // The generic leg: every rank of Blizzard and other everyday channels.
    for id in [
        10u32, 6141, 8427, 10185, 10186, 10187, // Blizzard, all six ranks
        5143,  // Arcane Missiles
        15407, // Mind Flay
        689,   // Drain Life
        5740,  // Rain of Fire
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert!(
            !d.channel_bar_own_name(),
            "{id} {:?} reads \"Channeling\", not its own name (AttributesEx {:#010x})",
            d.name,
            d.attributes_ex
        );
        assert!(!d.no_channel_bar(), "{id} {:?} still shows a bar", d.name);
    }

    // The named leg: Fishing and Mind Flay take opposite legs of the same `0x6e75a1`.
    for (id, name) in [
        (7620u32, "Fishing"),
        (18248, "Fishing"),
        (20578, "Cannibalize"),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert!(d.channel_bar_own_name(), "{id} names itself on the bar");
        assert_eq!(d.name, name);
    }

    // The suppressor, on these two rows only.
    for id in [24322u32, 24323] {
        assert!(
            cat.get(id).expect("Blood Siphon").no_channel_bar(),
            "{id} Blood Siphon shows no channel bar at all"
        );
    }

    let channeled: Vec<_> = cat.iter().filter(|(_, d)| d.tooltip_channeled()).collect();
    assert_eq!(channeled.len(), 323, "channeled rows in the shipped file");
    assert_eq!(
        channeled
            .iter()
            .filter(|(_, d)| d.channel_bar_own_name())
            .count(),
        9,
        "only nine channeled rows print their own name"
    );
    assert_eq!(
        channeled.iter().filter(|(_, d)| d.no_channel_bar()).count(),
        2,
        "only the two Blood Siphons suppress the bar"
    );
}

/// `SpellFamilyName` (160) and the `SpellFamilyFlags` pair (161-162): the eleven nonzero families
/// are vmangos's `SpellFamilyNames`, 322 rows set several bits and the top bit is 35, so the high
/// dword is live, and Cleanse (4987, bits 12 and 33) pins the pair's order.
#[test]
fn real_spell_family_columns_carry_the_modifier_gate() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    let mut families: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    let mut popcounts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    let mut max_bit = 0u32;
    for (_, d) in cat.iter() {
        *families.entry(d.spell_family).or_default() += 1;
        *popcounts
            .entry(d.spell_family_flags.count_ones())
            .or_default() += 1;
        if d.spell_family_flags != 0 {
            max_bit = max_bit.max(63 - d.spell_family_flags.leading_zeros());
        }
    }
    assert_eq!(
        families,
        [
            (0, 18243),
            (1, 47),
            (3, 563),
            (4, 350),
            (5, 466),
            (6, 493),
            (7, 450),
            (8, 316),
            (9, 393),
            (10, 362),
            (11, 500),
            (13, 174),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<u32, usize>>(),
        "SpellFamilyName histogram — the vmangos SpellFamilyNames set, and only it"
    );
    assert_eq!(
        popcounts.values().skip(1).sum::<usize>() - popcounts[&1],
        322,
        "rows setting MORE than one family bit — the reader's sum is live"
    );
    assert_eq!(max_bit, 35, "the highest family bit index in the file");

    // Frostbolt spans one dword, Cleanse both, Cure Poison only the high one.
    for (id, family, bits) in [
        (116u32, 3u32, &[5u32, 19, 20, 30][..]),
        (4987, 10, &[12, 33]),
        (526, 11, &[35]),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert_eq!(d.spell_family, family, "{id} {:?} family", d.name);
        let set: Vec<u32> = (0..64)
            .filter(|b| d.spell_family_flags >> b & 1 == 1)
            .collect();
        assert_eq!(set, bits, "{id} {:?} family bits", d.name);
    }
}
