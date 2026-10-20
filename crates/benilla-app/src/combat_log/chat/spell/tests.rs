use super::{
    combat, leech_family, leech_figures, periodic_heal_worded, periodic_leech_amount, power_gain,
    rounded_product,
};

/// A rage gain words the displayed figure. `Unbridled Wrath Effect` (12964) energizes rage by
/// base points 9 + 1 = 10 on the rage field's ×10 scale, and `SendEnergizeSpellLog` sends that
/// 10 (`SpellCaster.cpp:796-813`); the line reads 1.
#[test]
fn a_rage_gain_words_the_displayed_figure_not_the_wire_one() {
    assert_eq!(power_gain(1, 10), 1, "Unbridled Wrath: one point of rage");
    assert_eq!(power_gain(1, 100), 10, "Bloodrage: ten");
    // Mana, focus and energy are one-to-one.
    assert_eq!(power_gain(0, 300), 300);
    assert_eq!(power_gain(2, 20), 20);
    assert_eq!(power_gain(3, 20), 20);
}

/// `0x627930`'s three rules, one function for both of its packets.
#[test]
fn a_leech_divides_both_figures_and_drops_a_zero_drained_line() {
    // Mana leeches one-to-one: 120 drained, half of it gained.
    assert_eq!(leech_figures(0, 120, 0.5), Some((120, 60)));
    // The product is truncated, then divided: `trunc(25 * 0.9) = 22`, `22 / 10 = 2`, not 1.
    assert_eq!(leech_figures(1, 25, 0.9), Some((2, 2)));
    // Below one displayed point, `drained == 0` drops the line.
    assert_eq!(leech_figures(1, 9, 1.0), None);
    assert_eq!(leech_figures(0, 0, 1.0), None);
    // Happiness is ×1000: 1500 raw is one displayed point.
    assert_eq!(leech_figures(4, 1500, 0.0), Some((1, 0)));
}

/// The leech/drain fork on `[0x8029d4] = 2^-22`: a multiplier exactly on it is a leech.
#[test]
fn the_leech_drain_fork_is_the_references_own_epsilon() {
    const EPS: f32 = 1.0 / 4_194_304.0;
    assert_eq!(leech_family(1.0).stem, "SPELLPOWERLEECH");
    assert_eq!(
        leech_family(EPS).stem,
        "SPELLPOWERLEECH",
        "equality leeches"
    );
    assert_eq!(leech_family(EPS / 2.0).stem, "SPELLPOWERDRAIN");
    assert_eq!(leech_family(0.0).stem, "SPELLPOWERDRAIN");
    // `fabs` first: the sign does not matter.
    assert_eq!(leech_family(-1.0).stem, "SPELLPOWERLEECH");
}

/// `0x6278f0`'s table has five entries, happiness included (`HAPPINESS_POINTS`,
/// `GlobalStrings.lua:2117`), bounded by an unsigned `cmp ecx,5; jae`. Happiness lacks only a
/// `COMBAT_TEXT_UPDATE` tag.
#[test]
fn every_wire_power_tag_has_a_noun_and_nothing_past_the_table_does() {
    for power in 0..=4 {
        assert!(combat::power_has_word(power), "power {power} has a noun");
    }
    for power in [5, 6, 99, u32::MAX] {
        assert!(!combat::power_has_word(power), "power {power} has none");
    }
}

/// The gained figure goes through an f32 product (`0x627a95`); 2^24+1 is the smallest amount
/// where the narrowing shows, rounding to even one below.
#[test]
fn the_gained_figure_goes_through_the_references_f32_product() {
    assert_eq!(rounded_product(25, 0.9), 22, "floor, not round");
    assert_eq!(rounded_product(120, 0.5), 60);
    assert_eq!(
        rounded_product(16_777_217, 1.0),
        16_777_216,
        "the `fst` narrowing"
    );
}

/// The periodic leg divides twice, as the reference does (`0x627126`/`0x627138`, then
/// `0x627930`): mana is unchanged, a wire 100 rage words as 1.
#[test]
fn a_periodic_leech_is_pre_divided_before_the_formatter_divides_again() {
    assert_eq!(periodic_leech_amount(0, 400), 400, "mana: the identity");
    assert_eq!(periodic_leech_amount(1, 100), 10, "rage: the first divide");
    // End to end: 100 rage on the wire words as 1.
    assert_eq!(
        leech_figures(1, periodic_leech_amount(1, 100), 1.0),
        Some((1, 1))
    );
    // The direct packet takes the same wire number through one divide only.
    assert_eq!(leech_figures(1, 100, 1.0), Some((10, 10)));
}

/// The duration `0x627240` tests: base plus per level, capped at the maximum, against 30 s
/// inclusive (`0x6273e7`-`0x627406`); no row, no word.
#[test]
fn a_periodic_heal_is_worded_up_to_thirty_seconds() {
    let row = |base_ms, per_level_ms, max_ms| benilla_formats::SpellDuration {
        base_ms,
        per_level_ms,
        max_ms,
    };
    assert!(!periodic_heal_worded(None, 60));
    assert!(periodic_heal_worded(Some(&row(30_000, 0, 30_000)), 60));
    assert!(!periodic_heal_worded(Some(&row(30_001, 0, 30_001)), 60));
    // The level term counts: 10 s + 25 × 1 s is 35 s.
    assert!(periodic_heal_worded(Some(&row(10_000, 1_000, 60_000)), 20));
    assert!(!periodic_heal_worded(Some(&row(10_000, 1_000, 60_000)), 25));
    // ...capped at the maximum.
    assert!(periodic_heal_worded(Some(&row(10_000, 1_000, 20_000)), 25));
}
