//! The spell-description `$`-token engine the 1.12 client runs over `Spell.dbc` description and
//! aura text (`0x5075f0` → `0x507710`), effect values from `0x6e3800`. Values are the flat term:
//! the reference's per-level terms (`DicePerLevel·max(0, casterLevel − baseLevel)` on the dice,
//! and `RealPointsPerLevel`) are not applied. Values print unsigned, as the client's do, and an
//! `EffectAmplitude` of 0 is the reference's 5000 ms period (`$t`, `$o`). `$g` always takes the
//! first form, as there is no gender input, and `$r`, `$u` and any unknown or unresolved token
//! stay raw.

use super::{SpellDisplay, SpellDurationCatalog, SpellRadiusCatalog};

/// The inputs one substitution runs over. `lookup` resolves cross-spell references (`$1234s1`).
pub struct TokenContext<'a> {
    pub durations: &'a SpellDurationCatalog,
    pub radii: &'a SpellRadiusCatalog,
    pub lookup: &'a dyn Fn(u32) -> Option<&'a SpellDisplay>,
    /// The `$z` token: the home-bind area's name, from `SMSG_BINDPOINTUPDATE`'s area id through
    /// `AreaTable.dbc`; `None` leaves the token raw.
    pub home_area: Option<&'a str>,
    /// Resolves a `GlobalStrings` key and fills its `%d` holes; the caller owns the string table.
    /// Integer holes only: the keys that take numbers are `INT_SPELL_*` ones, whose float twins
    /// would print "14.0 to 22.0" for "14 to 22". `None` leaves the token raw.
    pub text: &'a dyn Fn(&str, &[i64]) -> Option<String>,
}

/// The effect's `(min, max)`, flat term only (`0x6e3800`).
fn effect_bounds(d: &SpellDisplay, slot: usize) -> (i64, i64) {
    let base = i64::from(*d.effect_base_points.get(slot).unwrap_or(&0));
    let dice = i64::from(*d.effect_base_dice.get(slot).unwrap_or(&0));
    let sides = i64::from(*d.effect_die_sides.get(slot).unwrap_or(&0));
    (base + dice, base + sides * dice)
}

/// A spell's duration in ms, flat term only; -1 is permanent.
fn duration_ms(d: &SpellDisplay, ctx: &TokenContext) -> Option<i64> {
    let row = ctx.durations.get(d.duration_index)?;
    Some(i64::from(row.base_ms))
}

/// The duration in the largest whole `INT_SPELL_DURATION_*` unit, the ladder `0x52fa50` walks,
/// or `SPELL_DURATION_UNTIL_CANCELLED` when permanent. The plural pick is `GetText`'s, the `_P1`
/// twin unless exactly one; only HOURS ships one, so the others fall back to the bare key.
fn duration_text(ms: i64, ctx: &TokenContext) -> Option<String> {
    if ms < 0 {
        return (ctx.text)("SPELL_DURATION_UNTIL_CANCELLED", &[]);
    }
    let secs = ms / 1000;
    let (unit, n) = if secs < 60 {
        ("SEC", secs)
    } else if secs < 3_600 {
        ("MIN", secs / 60)
    } else if secs < 86_400 {
        ("HOURS", secs / 3_600)
    } else {
        ("DAYS", secs / 86_400)
    };
    let key = format!("INT_SPELL_DURATION_{unit}");
    (n != 1)
        .then(|| (ctx.text)(&format!("{key}_P1"), &[n]))
        .flatten()
        .or_else(|| (ctx.text)(&key, &[n]))
}

/// `INT_SPELL_POINTS_SPREAD_TEMPLATE` ("%d to %d"), not the float `SPELL_POINTS_SPREAD_TEMPLATE`,
/// which would print "14.0 to 22.0" where Fireball rank 1 says "14 to 22".
fn spread_text(min: i64, max: i64, ctx: &TokenContext) -> Option<String> {
    (ctx.text)("INT_SPELL_POINTS_SPREAD_TEMPLATE", &[min, max])
}

/// Trim a float to the client's terse style (no trailing zeros: 2.5 → "2.5", 3.0 → "3").
fn trim_float(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// One token's text and the numeric value the `$l` plural picker keys on.
fn token_value(
    letter: char,
    slot: usize,
    d: &SpellDisplay,
    ctx: &TokenContext,
    scale: f64,
) -> Option<(String, f64)> {
    let scaled = |v: i64| -> i64 {
        if scale == 1.0 {
            v
        } else {
            (v as f64 * scale).round() as i64
        }
    };
    match letter.to_ascii_lowercase() {
        's' => {
            let (min, max) = effect_bounds(d, slot);
            let (min, max) = (scaled(min.abs()), scaled(max.abs()));
            Some(if min == max {
                (min.to_string(), min as f64)
            } else {
                (spread_text(min, max, ctx)?, max as f64)
            })
        }
        'm' if letter == 'm' => {
            let (min, _) = effect_bounds(d, slot);
            let v = scaled(min.abs());
            Some((v.to_string(), v as f64))
        }
        'm' => {
            // 'M'
            let (_, max) = effect_bounds(d, slot);
            let v = scaled(max.abs());
            Some((v.to_string(), v as f64))
        }
        'o' => {
            let (min, max) = effect_bounds(d, slot);
            let period = i64::from(*d.effect_amplitude.get(slot).unwrap_or(&0)).max(0);
            let period = if period == 0 { 5000 } else { period };
            let dur = duration_ms(d, ctx).unwrap_or(0).max(0);
            let total = |v: i64| scaled((v.abs() * dur / period).max(0));
            let (tmin, tmax) = (total(min), total(max));
            Some(if tmin == tmax {
                (tmin.to_string(), tmin as f64)
            } else {
                (spread_text(tmin, tmax, ctx)?, tmax as f64)
            })
        }
        'd' => {
            let ms = duration_ms(d, ctx)?;
            let v = if ms < 0 { 0.0 } else { ms as f64 / 1000.0 };
            Some((duration_text(ms, ctx)?, v))
        }
        't' => {
            let period = i64::from(*d.effect_amplitude.get(slot).unwrap_or(&0));
            let period = if period == 0 { 5000 } else { period };
            let v = period as f64 / 1000.0;
            Some((trim_float(v), v))
        }
        'a' => {
            let idx = *d.effect_radius_index.get(slot).unwrap_or(&0);
            let r = ctx.radii.get(idx)?;
            Some((trim_float(f64::from(r.radius)), f64::from(r.radius)))
        }
        'h' => Some((d.proc_chance.to_string(), f64::from(d.proc_chance))),
        'x' => {
            let v = *d.effect_chain_targets.get(slot).unwrap_or(&0);
            Some((v.to_string(), f64::from(v)))
        }
        'e' => {
            let v = f64::from(*d.effect_multiple_value.get(slot).unwrap_or(&0.0));
            Some((trim_float(v), v))
        }
        'z' => {
            // Player state, not spell data: the home-bind area name.
            let name = ctx.home_area?;
            Some((name.to_string(), 0.0))
        }
        'r' => {
            // Left raw: the display carries only the range index.
            None
        }
        _ => None,
    }
}

/// Substitute every `$`-token in `text` against `spell`.
pub fn substitute(text: &str, spell: &SpellDisplay, ctx: &TokenContext) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut last_value: f64 = 0.0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            let ch_len = utf8_len(bytes[i]);
            out.push_str(&text[i..i + ch_len]);
            i += ch_len;
            continue;
        }
        let start = i;
        i += 1;
        // `$/N;` or `$*N;` scales the next token.
        let mut scale = 1.0f64;
        if i < bytes.len() && (bytes[i] == b'/' || bytes[i] == b'*') {
            let op = bytes[i];
            let mut j = i + 1;
            let num_start = j;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                j += 1;
            }
            if let Ok(n) = text[num_start..j].parse::<f64>() {
                if j < bytes.len() && bytes[j] == b';' {
                    j += 1;
                }
                scale = if op == b'/' { 1.0 / n } else { n };
                i = j;
            }
        }
        // Optional cross-spell id digits.
        let id_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let ref_spell: Option<u32> = if i > id_start {
            text[id_start..i].parse().ok()
        } else {
            None
        };
        // The plural / gender selectors.
        if i < bytes.len()
            && (bytes[i] == b'l' || bytes[i] == b'L' || bytes[i] == b'g' || bytes[i] == b'G')
        {
            let selector = bytes[i].to_ascii_lowercase();
            if let Some(end) = text[i + 1..].find(';') {
                let body = &text[i + 1..i + 1 + end];
                if let Some((a, b)) = body.split_once(':') {
                    let pick = match selector {
                        b'l' => {
                            if (last_value - 1.0).abs() < 1e-9 {
                                a
                            } else {
                                b
                            }
                        }
                        _ => a, // $g: the first form, as there is no gender input
                    };
                    out.push_str(pick);
                    i = i + 1 + end + 1;
                    continue;
                }
            }
        }
        // The token letter and its optional 1-based slot digit.
        let Some(&letter_b) = bytes.get(i) else {
            out.push('$');
            continue;
        };
        let letter = letter_b as char;
        if !letter.is_ascii_alphabetic() {
            // Not a token: keep `$` and the next char raw, stepping by its UTF-8 width (`$é`).
            let ch_len = utf8_len(letter_b);
            out.push_str(&text[start..i + ch_len]);
            i += ch_len;
            continue;
        }
        i += 1;
        let slot = if i < bytes.len() && bytes[i].is_ascii_digit() {
            // `$s0` is a 1-based slot below 1: saturate to slot 0 rather than wrap.
            let s = bytes[i].saturating_sub(b'1') as usize;
            i += 1;
            s.min(2)
        } else {
            0
        };
        let target: &SpellDisplay = match ref_spell {
            Some(id) => match (ctx.lookup)(id) {
                Some(s) => s,
                None => {
                    out.push_str(&text[start..i]);
                    continue;
                }
            },
            None => spell,
        };
        match token_value(letter, slot, target, ctx, scale) {
            Some((sub, val)) => {
                last_value = val;
                out.push_str(&sub);
            }
            None => out.push_str(&text[start..i]), // unknown token: keep raw
        }
    }
    out
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spells::SpellDisplay;

    /// Deliberately unlike the shipped wording, so a wrong key cannot pass by reading the same.
    fn text(key: &str, args: &[i64]) -> Option<String> {
        let n = |i: usize| args.get(i).copied().unwrap_or_default();
        Some(match key {
            "SPELL_DURATION_UNTIL_CANCELLED" => "<forever>".into(),
            "INT_SPELL_DURATION_SEC" => format!("<{}sec>", n(0)),
            "INT_SPELL_DURATION_MIN" => format!("<{}min>", n(0)),
            "INT_SPELL_DURATION_HOURS" => format!("<{}hour>", n(0)),
            "INT_SPELL_DURATION_HOURS_P1" => format!("<{}hrs>", n(0)),
            "INT_SPELL_DURATION_DAYS" => format!("<{}days>", n(0)),
            "INT_SPELL_POINTS_SPREAD_TEMPLATE" => format!("<{}..{}>", n(0), n(1)),
            _ => return None,
        })
    }

    fn ctx<'a>(
        durations: &'a SpellDurationCatalog,
        radii: &'a SpellRadiusCatalog,
        lookup: &'a dyn Fn(u32) -> Option<&'a SpellDisplay>,
    ) -> TokenContext<'a> {
        TokenContext {
            home_area: None,
            durations,
            radii,
            lookup,
            text: &text,
        }
    }

    #[test]
    fn the_duration_ladder_picks_unit_then_plural() {
        let mut durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        for (idx, ms) in [
            (1, 30_000),
            (2, 60_000),
            (3, 3_600_000),
            (4, 7_200_000),
            (5, 172_800_000),
            (6, -1),
        ] {
            durations.insert_for_tests(idx, ms);
        }
        let c = ctx(&durations, &radii, &none_lookup);
        let d = |duration_index| {
            substitute(
                "$d",
                &SpellDisplay {
                    duration_index,
                    ..Default::default()
                },
                &c,
            )
        };
        assert_eq!(d(1), "<30sec>");
        assert_eq!(d(2), "<1min>", "a single minute takes the bare token");
        assert_eq!(d(3), "<1hour>", "and so does a single hour");
        assert_eq!(d(4), "<2hrs>", "but two take the _P1 twin");
        assert_eq!(d(5), "<2days>", "the days arm the ladder used to lack");
        assert_eq!(d(6), "<forever>");
    }

    fn none_lookup<'a>(_: u32) -> Option<&'a SpellDisplay> {
        None
    }

    /// Base 13 and one 9-sided die give "14 to 22", Fireball rank 1's shape; a diceless effect
    /// prints one value.
    #[test]
    fn s_token_bounds() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let mut d = SpellDisplay {
            effect_base_points: [13, 24, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [9, 0, 0],
            ..Default::default()
        };
        let c = ctx(&durations, &radii, &none_lookup);
        assert_eq!(
            substitute("causes $s1 Fire damage and $s2 more", &d, &c),
            "causes <14..22> Fire damage and 24 more"
        );
        // Negative base points print absolute (the client's "reduces by N" phrasing).
        d.effect_base_points = [-31, 0, 0];
        d.effect_base_dice = [1, 0, 0];
        d.effect_die_sides = [1, 0, 0];
        assert_eq!(
            substitute("reduces armor by $s1", &d, &c),
            "reduces armor by 30"
        );
    }

    #[test]
    fn overtime_duration_period_scale_plural() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 18_000);
        let radii = SpellRadiusCatalog::default();
        let d = SpellDisplay {
            duration_index: 1,
            effect_base_points: [2, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [1, 0, 0],
            effect_amplitude: [3000, 0, 0],
            ..Default::default()
        };
        let c = ctx(&durations, &radii, &none_lookup);
        assert_eq!(
            substitute("Deals $o1 damage over $d, every $t1 sec.", &d, &c),
            "Deals 18 damage over <18sec>, every 3 sec."
        );
        assert_eq!(
            substitute("Restores $/2;s1 health: $l point:points;.", &d, &c),
            // 3 halved is 1.5, which rounds to 2; the plural picks "points"
            "Restores 2 health: points.".to_string()
        );
    }

    #[test]
    fn cross_spell_and_unknown_tokens() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let other = SpellDisplay {
            effect_base_points: [99, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [1, 0, 0],
            ..Default::default()
        };
        let lookup = |id: u32| -> Option<&SpellDisplay> { (id == 1234).then_some(&other) };
        let d = SpellDisplay::default();
        let c = TokenContext {
            home_area: Some("Goldshire"),
            durations: &durations,
            radii: &radii,
            lookup: &lookup,
            text: &text,
        };
        assert_eq!(
            substitute("as strong as $1234s1 hits", &d, &c),
            "as strong as 100 hits"
        );
        assert_eq!(substitute("stacks $u times", &d, &c), "stacks $u times");
        assert_eq!(
            substitute("Returns you to $z.", &d, &c),
            "Returns you to Goldshire."
        );
        let unbound = TokenContext {
            home_area: None,
            durations: &durations,
            radii: &radii,
            lookup: &lookup,
            text: &text,
        };
        assert_eq!(
            substitute("Returns you to $z.", &d, &unbound),
            "Returns you to $z."
        );
    }

    #[test]
    fn a_dollar_before_a_multibyte_char_passes_through_on_the_char_boundary() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let d = SpellDisplay {
            effect_base_points: [13, 24, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [9, 0, 0],
            ..Default::default()
        };
        let c = ctx(&durations, &radii, &none_lookup);
        assert_eq!(substitute("coûte $é or $…!", &d, &c), "coûte $é or $…!");
        assert_eq!(substitute("$é", &d, &c), "$é");
        assert_eq!(substitute("$s0", &d, &c), "<14..22>");
    }
}
