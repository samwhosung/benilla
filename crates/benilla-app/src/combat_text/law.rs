//! The WORLDTEXTSTRING tables and formulas: categories, words, colours, fade, scale and the
//! emitter splits.

use bevy::prelude::*;

/// One config-table row (`0xce8828`, stride 0x1c, filled by `0x6c79a0`): rise in world units over
/// the whole life, fade-in end, fade-out start and duration in ms, the scale pair (equal except
/// the crit row, whose keyframes ramp `valueHi`) and the ARGB default colour (`0x4a2c10`).
pub(super) struct Category {
    pub(super) rise: f32,
    fade_in_ms: f32,
    fade_out_ms: f32,
    pub(super) dur_ms: f32,
    value_lo: f32,
    value_hi: f32,
    pub(super) color: u32,
}

/// The category scale values, bit-exact: `0.018333` and `0.0275`.
const VALUE_NORMAL: f32 = f32::from_bits(0x3c96_2fc9);
const VALUE_CRIT: f32 = f32::from_bits(0x3ce1_47ad);

/// Rows: 0 number, 1 ABSORB word, 2 crit number, 3 miss/dodge/parry word, 4 XP, 5 honor. Row 1's
/// `fade_out(90) < fade_in(150)` is real, a quick flicker.
pub(super) const CATEGORIES: [Category; 6] = [
    Category {
        rise: 2.0,
        fade_in_ms: 150.0,
        fade_out_ms: 760.0,
        dur_ms: 1500.0,
        value_lo: VALUE_NORMAL,
        value_hi: VALUE_NORMAL,
        color: 0xFFFF_FFFF,
    },
    Category {
        rise: 2.0,
        fade_in_ms: 150.0,
        fade_out_ms: 90.0,
        dur_ms: 1500.0,
        value_lo: VALUE_NORMAL,
        value_hi: VALUE_NORMAL,
        color: 0xFFFF_FFFF,
    },
    Category {
        rise: 0.0,
        fade_in_ms: 150.0,
        fade_out_ms: 1000.0,
        dur_ms: 1500.0,
        value_lo: 0.0,
        value_hi: VALUE_CRIT,
        color: 0xFFFF_FFFF,
    },
    Category {
        rise: 2.0,
        fade_in_ms: 150.0,
        fade_out_ms: 1000.0,
        dur_ms: 1500.0,
        value_lo: VALUE_NORMAL,
        value_hi: VALUE_NORMAL,
        color: 0xFFFF_FFFF,
    },
    Category {
        rise: 0.0,
        fade_in_ms: 500.0,
        fade_out_ms: 2000.0,
        dur_ms: 4500.0,
        value_lo: VALUE_NORMAL,
        value_hi: VALUE_NORMAL,
        color: 0x8094_008B,
    },
    Category {
        rise: 0.0,
        fade_in_ms: 500.0,
        fade_out_ms: 2000.0,
        dur_ms: 4500.0,
        value_lo: VALUE_NORMAL,
        value_hi: VALUE_NORMAL,
        color: 0xFFE0_CA0A,
    },
];

/// The crit pop keyframes (`0x8112dc`, category 2): `{t0, t1, s0, s1}` factors on `valueHi`, up to
/// 2x by 10% of life and back to 1x by 20%.
const CRIT_KEYFRAMES: [(f32, f32, f32, f32); 3] = [
    (0.0, 0.1, 0.1, 2.0),
    (0.1, 0.2, 2.0, 1.0),
    (0.2, 1.0, 1.0, 1.0),
];

/// The outcome words by code 1-11 (`0x86582c` keys for `FrameScript_GetText`), vmangos
/// `SpellMissInfo` (`SpellDefines.h:160`); the enUS `GlobalStrings.lua` values.
const WORDS: [&str; 11] = [
    "Miss", "Resist", "Dodge", "Parry", "Block", "Evade", "Immune", "Immune", "Deflect", "Absorb",
    "Reflect",
];

/// Outcome code 1-11 to `(word, category)`: category 3, except ABSORB's 1 (`0x80c48c`).
pub(crate) fn miss_word(code: u8) -> Option<(&'static str, u8)> {
    let word = *WORDS.get((code as usize).checked_sub(1)?)?;
    Some((word, if code == 10 { 1 } else { 3 }))
}

/// The emitter override colours (`0x5fa0b0`/`0x5fa0f0`): spell damage gold `[0xc4d8a0]`, pet melee
/// orange `[0xc4d8cc]`. No override means the row's default, white for rows 0-3.
pub(crate) const COLOR_SPELL_GOLD: u32 = 0xFFFF_DE00;
pub(crate) const COLOR_PET_MELEE_ORANGE: u32 = 0xFFFF_8400;

/// The three combat-text CVar gates: `CombatDamage` (`[0xc4d944]`), `PetMeleeDamage`
/// (`[0xc4d9cc]`) and `PetSpellDamage` (`[0xc4d99c]`), each read as the record's int `+0x28`.
/// Their only reads are in the word emitter `0x607140` and the number emitter `0x6128b0`, and a
/// failed gate suppresses the emit entirely.
///
/// `CombatDamage` is the master and silences words as well as numbers. The `Pet*` pair gate only
/// the owned-by-you branch, split by `B`: no spell record, or `AttributesEx3` bit 15 (vmangos
/// `SPELL_ATTR_EX3_NORMAL_RANGED_ATTACK`, `SpellDefines.h:924`, the basic ranged shots), goes to
/// `PetMeleeDamage`.
#[derive(Resource, Clone, Copy)]
pub(crate) struct DamageTextGates {
    pub(crate) combat_damage: bool,
    pub(crate) pet_melee: bool,
    pub(crate) pet_spell: bool,
}

impl Default for DamageTextGates {
    /// The reference's registered defaults, all `"1"`.
    fn default() -> Self {
        Self {
            combat_damage: true,
            pet_melee: true,
            pet_spell: true,
        }
    }
}

/// The `0x5efea0` source-ownership classes that may draw (`K`): the player, or a unit it summoned
/// or created. Every other source is suppressed; the caller drops the emit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DamageSource {
    Player,
    Pet,
}

/// The colour law's `B` bit: `recordPtr == 0 || sign(byte[SpellRec+0x25])`, no record or
/// `AttributesEx3` bit 15. `None` is both a pushed NULL (a melee swing, a damage shield) and a
/// missing catalog. `0x607140` and `0x6128b0` compute it alike; only the melee word site
/// (`0x624511`) pushes NULL, so a spell's miss word is gold like its number.
pub(crate) fn melee_styled(display: Option<&benilla_formats::SpellDisplay>) -> bool {
    display.is_none_or(benilla_formats::SpellDisplay::melee_white_damage)
}

/// The colour branch (`0x6128b0`, `6128f6`-`612964`) by `B` and `K`: `None` gates the emit off,
/// `Some(None)` draws in the row's default, `Some(Some(argb))` in the override. Crit never enters.
pub(crate) fn damage_color(
    gates: DamageTextGates,
    source: DamageSource,
    melee: bool,
) -> Option<Option<u32>> {
    if !gates.combat_damage {
        return None;
    }
    match (source, melee) {
        (DamageSource::Player, true) => Some(None),
        (DamageSource::Player, false) => Some(Some(COLOR_SPELL_GOLD)),
        (DamageSource::Pet, true) => gates.pet_melee.then_some(Some(COLOR_PET_MELEE_ORANGE)),
        (DamageSource::Pet, false) => gates.pet_spell.then_some(Some(COLOR_SPELL_GOLD)),
    }
}

/// The melee emitter split, `0x6243e0`'s branch order: victim states 2, 3, 5, 6, 7, 8 float their
/// word whatever the damage; otherwise damage floats the bare number (category 2 on
/// `HITINFO_CRITICALHIT 0x80`), and zero damage is "Absorb" on `0x20`, "Resist" on `0x40`, else
/// "Miss" (`HITINFO_MISS` is never tested).
pub(crate) fn melee_text(hit_info: u32, victim_state: u32, damage: u32) -> Option<(u8, String)> {
    let code = match victim_state {
        2 => 3, // DODGE
        3 => 4, // PARRY
        5 => 5, // BLOCK
        6 => 6, // EVADE
        7 => 7, // IMMUNE
        8 => 9, // DEFLECT
        _ => {
            // 0 UNAFFECTED, 1 NORMAL, 4 INTERRUPT: keyed on damage.
            if damage > 0 {
                let category = if hit_info & 0x80 != 0 { 2 } else { 0 };
                return Some((category, damage.to_string()));
            }
            if hit_info & 0x20 != 0 {
                10 // ABSORB, full; a partial one shows only the number
            } else if hit_info & 0x40 != 0 {
                2 // RESIST, full
            } else {
                1 // MISS
            }
        }
    };
    miss_word(code).map(|(w, c)| (c, w.to_string()))
}

/// The spell and periodic emitter split (`0x5e85e0`/`0x626dd0`): damage floats a number, category
/// 2 on `SPELL_HIT_TYPE_CRIT 0x2` (periodic ticks never crit); zero damage floats ABSORB or
/// RESIST, a choice inferred from the packet's fields, as `0x5e88d1` is untraced.
pub(crate) fn spell_text(
    damage: u32,
    absorb: u32,
    resist: i32,
    crit: bool,
) -> Option<(u8, String)> {
    if damage > 0 {
        return Some((if crit { 2 } else { 0 }, damage.to_string()));
    }
    if absorb > 0 {
        return miss_word(10).map(|(w, c)| (c, w.to_string()));
    }
    if resist > 0 {
        return miss_word(2).map(|(w, c)| (c, w.to_string()));
    }
    None
}

/// The gx pixel round (`0x5c7010`/`0x5c6fa0`): add 0.5 and truncate, half away from zero.
fn round_px(t: f64) -> f32 {
    let r = if t > 0.0 { t + 0.5 } else { t - 0.5 };
    r.trunc() as f32
}

/// Scale value `v` to pixel height, constant with distance. One gx unit is the screen diagonal:
/// device space spans `[0,G44]x[0,G48]`, `G48 = 1/√(s²+1)` for `s = W/H` (live at `0x832a44/48`),
/// so `v/G48 × H = v·√(W²+H²)`.
pub(super) fn text_px(v: f32, viewport: Vec2) -> f32 {
    round_px(f64::from(v) * f64::from(viewport.x).hypot(f64::from(viewport.y)))
}

/// The shadow offset, `0xce8804` (`{0.002, 0.002}`, init `0x6c7c20`): a viewport fraction per
/// axis, rounded at draw (`0x5c8710`).
const SHADOW_OFFSET_FRAC: f32 = 0.002;

/// The rendered shadow offset, `{round(0.002·W), round(0.002·H)}` px down-right.
pub(super) fn shadow_offset_px(viewport: Vec2) -> Vec2 {
    Vec2::new(
        round_px(f64::from(SHADOW_OFFSET_FRAC) * f64::from(viewport.x)),
        round_px(f64::from(SHADOW_OFFSET_FRAC) * f64::from(viewport.y)),
    )
}

/// The anti-overlap claim box in px. `0x6c81a0` stores the halves as screen fractions (height the
/// size value ÷ G48, width the advance sum ÷ screen width plus a `round(0.002·diag)` pen seed) and
/// `0x6c7cc0` spends them as DDC lengths, inflating them by diag/dim (1.667x tall, 1.25x wide at
/// 4:3). `ink_w` stands in for the advance sum; the reference's side bearings and raster pad
/// (at most ~3 px) are untraced.
pub(super) fn claimed_box_px(ink_w: f32, size_value: f32, viewport: Vec2) -> Vec2 {
    let diag = viewport.length();
    Vec2::new(
        (ink_w + (SHADOW_OFFSET_FRAC * diag).round()) * diag / viewport.x,
        size_value * diag * diag / viewport.y,
    )
}

/// The text and rendered shadow alpha at `elapsed_ms`: `0x6c82e0`'s two lanes (255.0 at
/// `0x7ffe58`, 127.0 at `0x811310`), fade-in tested first, then fade-out, else the `(0xFF, 0x7F)`
/// plateau. The ramps divide by the duration, so fade-in ends in a step. `0x5cd650` stores the
/// shadow as `min(shadow, text)`, so the raw `[128, 255]` fade-out lane renders as the text alpha.
pub(super) fn fade_alpha(cat: &Category, elapsed_ms: f32) -> (u8, u8) {
    // MSVC __ftol truncates; `as i32` matches.
    let (text, shadow) = if elapsed_ms < cat.fade_in_ms {
        let t = (elapsed_ms / cat.dur_ms).max(0.0);
        (
            ((255.0 * t) as i32).min(0xff) as u8,
            ((127.0 * t) as i32).min(0x7f) as u8,
        )
    } else if elapsed_ms >= cat.fade_out_ms {
        let u = (elapsed_ms - cat.fade_out_ms) / (cat.dur_ms - cat.fade_out_ms);
        (
            (255.0 - (255.0 * u).clamp(0.0, 255.0)) as u8,
            (255.0 - (127.0 * u).clamp(0.0, 127.0)) as u8,
        )
    } else {
        (0xff, 0x7f)
    };
    // `0x5cd650` writes the shadow alpha as min(shadowA, mainA) every tick, after SetColor.
    (text, shadow.min(text))
}

/// The scale value at normalized life `t` (`0x6c80b0`): category 2 runs the crit keyframes on
/// `valueHi`, the rest `lo + (hi − lo)·t`; floored at 0.001.
pub(super) fn scale_value(category: u8, t: f32) -> f32 {
    let cat = &CATEGORIES[category as usize];
    let v = if category == 2 {
        let (t0, t1, s0, s1) = *CRIT_KEYFRAMES
            .iter()
            .find(|(t0, t1, _, _)| (*t0..=*t1).contains(&t))
            .unwrap_or(&CRIT_KEYFRAMES[2]);
        (s0 + (s1 - s0) * ((t - t0) / (t1 - t0))) * cat.value_hi
    } else {
        cat.value_lo + (cat.value_hi - cat.value_lo) * t
    };
    v.max(0.001)
}

/// Packed ARGB (`0x4a2c10`) to straight-alpha sRGB RGBA; the alpha byte is dropped, as the fade
/// replaces it every tick.
pub(super) fn argb(c: u32) -> [f32; 4] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claimed box (`0x6c81a0`) at 1024×768: 39.1 px tall for the 23 px number, 58.7 px for the
    /// 35 px crit, and 1.25x the ink plus pen seed wide.
    #[test]
    fn claimed_box_matches_the_measured_block_anchors() {
        let ref43 = Vec2::new(1024.0, 768.0); // diag = 1280, G44 = 0.8, G48 = 0.6
        let steady = claimed_box_px(10.0, VALUE_NORMAL, ref43);
        assert!((steady.y - 39.1).abs() < 0.05, "steady box {}", steady.y);
        // ink 10 + seed round(2.56) = 3, times 1.25.
        assert!((steady.x - 16.25).abs() < 1e-3, "steady box {}", steady.x);
        let crit = claimed_box_px(10.0, VALUE_CRIT, ref43);
        assert!((crit.y - 58.7).abs() < 0.05, "crit box {}", crit.y);
    }

    #[test]
    fn text_px_matches_the_screen_to_pixel_law() {
        let ref43 = Vec2::new(1024.0, 768.0); // diag = 1280
                                              // 0.018333 × 1280 = 23.47, rounds to 23.
        assert_eq!(text_px(VALUE_NORMAL, ref43), 23.0);
        // Crit settled: 0.0275 × 1280 = 35.2.
        assert_eq!(text_px(VALUE_CRIT, ref43), 35.0);
        // Crit pop peak: 70.4.
        assert_eq!(text_px(2.0 * VALUE_CRIT, ref43), 70.0);
        // 23.5 rounds half away from zero, to 24.
        assert_eq!(text_px(23.5, Vec2::new(0.6, 0.8)), 24.0);
        // At 1920×1080 (diag ≈ 2202.9) a normal number is 40 px.
        assert_eq!(text_px(VALUE_NORMAL, Vec2::new(1920.0, 1080.0)), 40.0);
        assert_eq!(text_px(scale_value(0, 0.5), ref43), 23.0);
    }

    #[test]
    fn shadow_offset_is_a_per_axis_viewport_fraction() {
        // {round(3.84), round(2.16)}.
        assert_eq!(
            shadow_offset_px(Vec2::new(1920.0, 1080.0)),
            Vec2::new(4.0, 2.0)
        );
        // {round(2.048), round(1.536)}.
        assert_eq!(
            shadow_offset_px(Vec2::new(1024.0, 768.0)),
            Vec2::new(2.0, 2.0)
        );
    }

    #[test]
    fn config_rows_match_the_byte_table() {
        assert_eq!(VALUE_NORMAL.to_bits(), 0x3c96_2fc9);
        assert_eq!(VALUE_CRIT.to_bits(), 0x3ce1_47ad);
        assert_eq!(CATEGORIES[0].dur_ms, 1500.0);
        assert_eq!(CATEGORIES[1].fade_out_ms, 90.0); // the ABSORB flicker row
        assert_eq!(CATEGORIES[4].color, 0x8094_008B);
        assert_eq!(CATEGORIES[5].color, 0xFFE0_CA0A);
    }

    /// Row 0 (in 150, out 760, duration 1500) and row 1 at their pinned points, as rendered.
    #[test]
    fn fade_law_ramps_holds_and_mirrors() {
        let cat = &CATEGORIES[0];
        assert_eq!(fade_alpha(cat, 0.0), (0, 0));
        // 255 × 0.05 = 12.75 and 127 × 0.05 = 6.35, truncated.
        assert_eq!(fade_alpha(cat, 75.0), (12, 6));
        // 149 ms is still the ramp; 150 ms steps onto the plateau.
        assert_eq!(fade_alpha(cat, 149.0), (25, 12));
        assert_eq!(fade_alpha(cat, 150.0), (0xff, 0x7f));
        assert_eq!(fade_alpha(cat, 759.0), (0xff, 0x7f));
        // Fade-out start: both lanes 255, the shadow stepping up from 0x7f.
        assert_eq!(fade_alpha(cat, 760.0), (0xff, 0xff));
        // u = 0.5: text 127; the raw shadow lane's 191 is capped to it.
        assert_eq!(fade_alpha(cat, 1130.0), (127, 127));
        assert_eq!(fade_alpha(cat, 1500.0), (0, 0));
        // Row 1 has no plateau: fade-in is tested first, and at 150 ms fade-out is 60/1410 deep.
        let absorb = &CATEGORIES[1];
        assert_eq!(fade_alpha(absorb, 100.0), (17, 8)); // 255·t / 127·t at t = 1/15
        assert_eq!(fade_alpha(absorb, 150.0), (244, 244)); // text 255−10.85; shadow min(249, text)
                                                           // Row 4's 0x80 alpha never renders.
        assert_eq!(fade_alpha(&CATEGORIES[4], 1000.0), (0xff, 0x7f));
        // Row 4 at u = 0.8: the shadow fades with the text.
        assert_eq!(fade_alpha(&CATEGORIES[4], 4000.0), (51, 51));
    }

    #[test]
    fn crit_keyframes_pop_then_settle() {
        assert!((scale_value(2, 0.0) - 0.1 * VALUE_CRIT).abs() < 1e-7);
        assert!((scale_value(2, 0.05) - 1.05 * VALUE_CRIT).abs() < 1e-6);
        assert!((scale_value(2, 0.1) - 2.0 * VALUE_CRIT).abs() < 1e-6);
        assert!((scale_value(2, 0.15) - 1.5 * VALUE_CRIT).abs() < 1e-6);
        assert!((scale_value(2, 0.5) - VALUE_CRIT).abs() < 1e-7);
        assert_eq!(scale_value(0, 0.0), scale_value(0, 0.9));
        // A settled crit is 1.5x a normal number.
        assert!((scale_value(2, 0.5) / scale_value(0, 0.5) - 1.5).abs() < 1e-3);
    }

    /// The melee (`0x6243e0`) and spell splits; ABSORB is the one category-1 word.
    #[test]
    fn emitters_split_numbers_and_words() {
        assert_eq!(melee_text(0x2, 1, 37), Some((0, "37".into())));
        assert_eq!(melee_text(0x82, 1, 99), Some((2, "99".into()))); // crit bit 0x80
        assert_eq!(melee_text(0x10, 0, 0), Some((3, "Miss".into())));
        assert_eq!(melee_text(0x0, 2, 0), Some((3, "Dodge".into())));
        assert_eq!(melee_text(0x0, 3, 0), Some((3, "Parry".into())));
        assert_eq!(melee_text(0x0, 5, 0), Some((3, "Block".into())));
        assert_eq!(melee_text(0x0, 6, 0), Some((3, "Evade".into())));
        // A word state ignores damage.
        assert_eq!(melee_text(0x2, 3, 25), Some((3, "Parry".into())));
        assert_eq!(melee_text(0x22, 1, 0), Some((1, "Absorb".into()))); // full absorb: bit 0x20
        assert_eq!(melee_text(0x42, 1, 0), Some((3, "Resist".into()))); // full resist: bit 0x40
                                                                        // Zero damage: Miss.
        assert_eq!(melee_text(0x2, 1, 0), Some((3, "Miss".into())));
        assert_eq!(spell_text(120, 0, 0, false), Some((0, "120".into())));
        assert_eq!(spell_text(240, 0, 0, true), Some((2, "240".into())));
        assert_eq!(spell_text(0, 50, 0, false), Some((1, "Absorb".into())));
        assert_eq!(spell_text(0, 0, 80, false), Some((3, "Resist".into())));
        assert_eq!(spell_text(0, 0, 0, false), None);
        assert_eq!(miss_word(11), Some(("Reflect", 3)));
        assert_eq!(miss_word(0), None);
        assert_eq!(miss_word(12), None);
    }

    /// The `0x6128b0` B/K colour branch: self melee the row default, self spell gold, pet melee
    /// orange, pet spell gold.
    #[test]
    fn damage_color_matches_the_byte_table() {
        let on = DamageTextGates::default();
        assert_eq!(damage_color(on, DamageSource::Player, true), Some(None));
        assert_eq!(
            damage_color(on, DamageSource::Player, false),
            Some(Some(COLOR_SPELL_GOLD))
        );
        assert_eq!(
            damage_color(on, DamageSource::Pet, true),
            Some(Some(COLOR_PET_MELEE_ORANGE))
        );
        assert_eq!(
            damage_color(on, DamageSource::Pet, false),
            Some(Some(COLOR_SPELL_GOLD))
        );
        // `0x5fa0b0`/`0x5fa0f0`.
        assert_eq!(COLOR_SPELL_GOLD, 0xFFFF_DE00);
        assert_eq!(COLOR_PET_MELEE_ORANGE, 0xFFFF_8400);
    }

    /// Both read sites branch to a function epilogue, so a failed gate means no emit at all, and
    /// `CombatDamage = 0` takes the self case too.
    #[test]
    fn the_three_gates_suppress_the_emit_rather_than_recolour_it() {
        let master_off = DamageTextGates {
            combat_damage: false,
            ..Default::default()
        };
        for source in [DamageSource::Player, DamageSource::Pet] {
            for melee in [true, false] {
                assert_eq!(
                    damage_color(master_off, source, melee),
                    None,
                    "CombatDamage=0 must suppress {source:?} melee={melee}"
                );
            }
        }

        // A pet sub-gate takes only its own branch; the self case is unconditional.
        let no_pet_melee = DamageTextGates {
            pet_melee: false,
            ..Default::default()
        };
        assert_eq!(damage_color(no_pet_melee, DamageSource::Pet, true), None);
        assert_eq!(
            damage_color(no_pet_melee, DamageSource::Pet, false),
            Some(Some(COLOR_SPELL_GOLD)),
            "PetSpellDamage still governs the pet's spell branch"
        );
        assert_eq!(
            damage_color(no_pet_melee, DamageSource::Player, true),
            Some(None),
            "the self sub-case is unconditional"
        );

        let no_pet_spell = DamageTextGates {
            pet_spell: false,
            ..Default::default()
        };
        assert_eq!(damage_color(no_pet_spell, DamageSource::Pet, false), None);
        assert_eq!(
            damage_color(no_pet_spell, DamageSource::Pet, true),
            Some(Some(COLOR_PET_MELEE_ORANGE))
        );
    }
}
