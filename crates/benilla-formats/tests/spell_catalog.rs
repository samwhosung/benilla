//! Asset-gated fixture: the Spell/SpellIcon join against the real 5875 data — pins the derived
//! columns (SpellIconID = 117, SpellName enUS = 120, SpellVisualID = 115, Speed = 37; see
//! `src/spells.rs` docs) to known spells, so a schema drift or column slip fails loudly. Skips
//! (passes) without `<repo>/WoW/Data`.

use benilla_formats::{load_spell_catalog, open_chain};

#[test]
fn spell_catalog_resolves_known_spells() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let catalog = load_spell_catalog(&mut chain).expect("load spell catalog");
    assert!(
        catalog.len() > 20_000,
        "5875 ships 22357 spells, got {}",
        catalog.len()
    );

    // The column-derivation probes, now as regressions.
    let hs = catalog.get(78).expect("Heroic Strike");
    assert_eq!(hs.name, "Heroic Strike");
    assert_eq!(
        hs.icon.as_deref(),
        Some("Interface\\Icons\\Ability_Rogue_Ambush")
    );

    let bs = catalog.get(6673).expect("Battle Shout");
    assert_eq!(bs.name, "Battle Shout");
    assert_eq!(
        bs.icon.as_deref(),
        Some("Interface\\Icons\\Ability_Warrior_BattleShout")
    );

    let attack = catalog.get(6603).expect("Attack");
    assert_eq!(attack.name, "Attack");
    assert!(attack.icon.is_some(), "auto-attack has an icon");

    // The visual/speed column pins (decision 0107 data plane), cross-checked against the local
    // vmangos `spell_template` (`spellVisual1`/`speed`) — see `src/spells.rs` docs for the method.
    // **`PreventionType` (column 165)** — which crowd-control flag refuses the spell LOCALLY
    // (decision 1903): 1 silence, 2 pacify, 0 neither. Pinned here because the column has an
    // adjacent look-alike: 164 is `DmgClass`, which takes the same 0/1/2 on every one of these
    // rows. **Auto Shot separates them decisively** — it is `DmgClass = 3` (RANGED), a value
    // `PreventionType` never takes, so a one-column slip fails this test rather than passing
    // quietly. (The byte offset `SpellRec+0x294 / 4 = 165` is the other half of the pin.)
    assert_eq!(
        catalog.get(133).expect("Fireball").prevention_type,
        1,
        "Fireball is silence-preventable"
    );
    assert_eq!(
        catalog.get(78).expect("Heroic Strike").prevention_type,
        2,
        "Heroic Strike is pacify-preventable"
    );
    assert_eq!(
        catalog.get(6603).expect("Attack").prevention_type,
        0,
        "the auto-attack is neither"
    );
    assert_eq!(
        catalog.get(75).expect("Auto Shot").prevention_type,
        2,
        "Auto Shot is pacify-preventable — and its DmgClass 3 is what makes column 164 \
         distinguishable from 165 at all"
    );

    // **The crowd-control exemption's three columns** (decision 1946): `School` 1, `Mechanic` 5,
    // `EffectMechanic[0..2]` 79–81. Pinned against spells whose values are common knowledge, and
    // the per-effect pair is the convincing half — Frostbolt carries its SNARE on effect 0 and
    // Frost Nova its ROOT on effect 1, which no neighbouring column would reproduce.
    assert_eq!(catalog.get(133).expect("Fireball").school, 2, "fire");
    assert_eq!(catalog.get(116).expect("Frostbolt").school, 4, "frost");
    assert_eq!(catalog.get(585).expect("Smite").school, 1, "holy");
    assert_eq!(
        catalog.get(78).expect("Heroic Strike").school,
        0,
        "physical"
    );

    assert_eq!(
        catalog.get(118).expect("Polymorph").mechanic,
        17,
        "MECHANIC_POLYMORPH"
    );
    assert_eq!(
        catalog.get(5782).expect("Fear").mechanic,
        5,
        "MECHANIC_FEAR"
    );
    assert_eq!(
        catalog.get(133).expect("Fireball").mechanic,
        0,
        "a plain nuke carries no mechanic"
    );

    assert_eq!(
        catalog.get(116).expect("Frostbolt").effect_mechanic[0],
        11,
        "Frostbolt's slow is MECHANIC_SNARE, on effect 0"
    );
    assert_eq!(
        catalog.get(122).expect("Frost Nova").effect_mechanic[1],
        7,
        "Frost Nova's root is MECHANIC_ROOT, on effect 1 — not effect 0"
    );

    let fireball = catalog.get(133).expect("Fireball");
    assert_eq!(fireball.visual, 67, "Fireball's SpellVisual id");
    assert_eq!(fireball.speed, 24.0, "Fireball's projectile speed");

    let frostbolt = catalog.get(116).expect("Frostbolt");
    assert_eq!(frostbolt.visual, 13);
    assert_eq!(frostbolt.speed, 28.0);

    let corruption = catalog.get(172).expect("Corruption");
    assert_eq!(corruption.visual, 381);
    assert_eq!(corruption.speed, 0.0, "a DoT tick has no projectile");
}

#[test]
fn shapeshift_bonus_bars_match_the_verified_table() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open vanilla patch chain");
    let forms = benilla_formats::load_shapeshift_forms(&mut chain).expect("load shapeshift rows");
    // The complete 5875 non-zero BonusActionBar set (`0x4e4fc0`: SpellShapeshiftForm
    // field 1, the exact lookup GetBonusBarOffset's cached global is filled from).
    let expect = [(1, 1), (5, 3), (8, 3), (17, 1), (18, 2), (19, 3), (30, 1)];
    let nonzero = forms.values().filter(|f| f.bonus_bar != 0).count();
    assert_eq!(nonzero, expect.len(), "exactly the seven non-zero rows");
    for (form, bar) in expect {
        assert_eq!(
            forms.get(&form).map(|f| f.bonus_bar),
            Some(bar),
            "form {form}"
        );
    }
    // flags1 (the form gate's stance bit): warrior stances + stealth are stances; cat is a
    // true shapeshift.
    for stance in [17u32, 18, 19, 30] {
        assert!(forms.get(&stance).unwrap().is_stance(), "form {stance}");
    }
    assert!(!forms.get(&1).unwrap().is_stance(), "cat is a shapeshift");
    // flags1 bit 0x2 (the stance bar's toggle-cancel BLOCK, `0x4b4963`): the
    // three warrior stances carry it (0x7 — clicking the active stance is a silent no-op); the
    // cancelable forms don't (Cat 0x70, Bear/DireBear 0x50, Ghost Wolf 0x40, Shadowform 0x9,
    // Stealth 0x1, Moonkin 0x41 — probed on the extracted file).
    for stance in [17u32, 18, 19] {
        assert!(
            !forms.get(&stance).unwrap().cancelable(),
            "warrior stance {stance} blocks the toggle-cancel"
        );
    }
    for form in [1u32, 5, 8, 16, 28, 30, 31] {
        assert!(
            forms.get(&form).unwrap().cancelable(),
            "form {form} is cancelable"
        );
    }
}

/// The stance-bar Spell.dbc columns (`0x4b2bb0` orders by StanceBarOrder, `0x4b45c0` reads
/// ActiveIconID; column pins probed on the extracted 5875 file, decision 0270): the
/// MOD_SHAPESHIFT form id, the signed StanceBarOrder (Stealth's −1 sorts last), and the druid
/// forms' ActiveIconID. Skips without client data.
#[test]
fn stance_bar_spell_columns_match_the_probed_values() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open vanilla patch chain");
    let catalog = load_spell_catalog(&mut chain).expect("load spell catalog");
    // (spell, form id, order): Battle/Defensive/Berserker Stance, Bear, Cat, Stealth, Moonkin.
    for (spell, form, order) in [
        (2457u32, 17u32, 0i32),
        (71, 18, 1),
        (2458, 19, 2),
        (5487, 5, 0),
        (768, 1, 2),
        (1784, 30, -1),
        (24858, 31, 4),
    ] {
        let d = catalog.get(spell).expect("form spell");
        assert_eq!(d.shapeshift_form, Some(form), "spell {spell} form");
        assert_eq!(d.stance_bar_order, order, "spell {spell} order");
    }
    // ActiveIconID: druid forms carry the dismiss-paw (icon 122 resolves); warrior stances 0.
    assert!(catalog.get(5487).unwrap().active_icon.is_some(), "bear");
    assert!(catalog.get(2457).unwrap().active_icon.is_none(), "battle");
    // A non-form spell reads none of it.
    let fireball = catalog.get(133).unwrap();
    assert_eq!(fireball.shapeshift_form, None);
}

/// AttributesEx3 (column 9) bit 15 — `SPELL_ATTR3_NORMAL_RANGED_ATTACK`, the combat-text
/// melee-white flip (decision 0376): set on exactly the ranged basic shots, clear on melee
/// abilities and true spells.
#[test]
fn melee_white_damage_marks_the_ranged_basic_shots() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let catalog = load_spell_catalog(&mut chain).expect("load spell catalog");
    // Auto Shot, Shoot Bow, Throw, wand Shoot: the white-damage ranged shots.
    for id in [75u32, 2480, 2764, 5019] {
        assert!(
            catalog.get(id).unwrap().melee_white_damage(),
            "spell {id} should carry AttributesEx3 & 0x8000"
        );
    }
    // Heroic Strike (a melee ability — its yellow rides the ATTACKERSTATEUPDATE spell-id rider,
    // not this bit), Fireball, and Attack itself: all clear.
    for id in [78u32, 133, 6603] {
        assert!(
            !catalog.get(id).unwrap().melee_white_damage(),
            "spell {id} should NOT carry AttributesEx3 & 0x8000"
        );
    }
}
