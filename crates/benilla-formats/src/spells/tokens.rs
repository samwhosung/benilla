//! The spell-description `$`-token engine the 1.12 client runs over `Spell.dbc` description and
//! aura text (`0x5075f0` → `0x507710`), effect values from `GetEffectPoints 0x6e3800`. Values
//! print unsigned, as the client's do. `$g` always takes the first form, as there is no gender
//! input, and `$u` and any unknown or unresolved token stay raw.

use super::soft_float;
use super::{SpellDisplay, SpellDurationCatalog, SpellRadiusCatalog, SpellRangeCatalog};

/// The caster's spell-modifier appliers over its tables (`GetSpellModifiers 0x6e6b30`): the
/// integer `0x6e6af0`, the FPU float `0x6e6bf0` and the software float `0x6e6c30`
/// ([`super::soft_modify`]), each returning `value` unchanged when no modifier matches.
pub trait SpellMods {
    fn apply_int(&self, d: &SpellDisplay, op: u8, value: i32) -> i32;
    fn apply_float(&self, d: &SpellDisplay, op: u8, value: f32) -> f32;
    fn apply_soft(&self, d: &SpellDisplay, op: u8, value: f32) -> f32;
}

/// A value supplied to a `%` hole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TokenNumber {
    Int(i64),
    Float(f64),
}

/// The inputs one substitution runs over. `lookup` resolves cross-spell references (`$1234s1`).
pub struct TokenContext<'a> {
    pub durations: &'a SpellDurationCatalog,
    pub radii: &'a SpellRadiusCatalog,
    pub ranges: Option<&'a SpellRangeCatalog>,
    /// `0x5ea520` behind `0x6de040`: the active player's skill in a spell's `SkillLineAbility`
    /// line, bonuses included, by spell id; 0 without a player or a line. Every per-level term
    /// scales by the level derived from it ([`SpellDisplay::skill_level`]), not the character
    /// level.
    pub skill: &'a dyn Fn(u32) -> u32,
    pub lookup: &'a dyn Fn(u32) -> Option<&'a SpellDisplay>,
    /// The caster's live spell-modifier tables; absent for contexts without a caster.
    pub mods: Option<&'a dyn SpellMods>,
    /// `0x5075f0`'s last argument, set only by the aura tooltip `0x52f880` (`52f940`): the effect
    /// points skip their modifiers (`6e3925`), and so does the duration of `$d` and `$o`
    /// (`0x6ea000`'s flag, pushed at `507ccd` and `507917`).
    pub unmodified_points: bool,
    /// The `$z` token: the home-bind area's name, from `SMSG_BINDPOINTUPDATE`'s area id through
    /// `AreaTable.dbc`; `None` leaves the token raw.
    pub home_area: Option<&'a str>,
    /// `GetText 0x703bf0`: a `GlobalStrings` template by key. `None` leaves the token raw.
    pub global: &'a dyn Fn(&str) -> Option<String>,
    /// `SStrPrintf 0x64a7f0`: a template's `%` holes filled, as the CRT formats them.
    pub printf: &'a dyn Fn(&str, &[TokenNumber]) -> String,
}

/// A `GlobalStrings` template, filled.
fn keyed(ctx: &TokenContext, key: &str, args: &[TokenNumber]) -> Option<String> {
    Some((ctx.printf)(&(ctx.global)(key)?, args))
}

/// `"%.1f"` (`0x84f398`).
fn tenths(ctx: &TokenContext, v: f32) -> String {
    (ctx.printf)("%.1f", &[TokenNumber::Float(f64::from(v))])
}

/// `0x6e3130` for the active player (`0x6e318d`: `CGPlayer`'s `[vtbl+0xa8]`, `0x5ea690`).
fn skill_level(ctx: &TokenContext, d: &SpellDisplay) -> u32 {
    d.skill_level((ctx.skill)(d.id))
}

/// `0x6e3b80`'s verdict on an effect: whether its points round to whole numbers (its out-byte)
/// and whether it takes a damage op (its return).
fn classify(effect: u32, aura: u32) -> (bool, bool) {
    match effect {
        2 | 9 | 17 | 31 | 58 | 121 => (true, true),
        6 | 27 | 35 | 119 | 128 | 129 => match aura {
            3 | 15 | 43 | 53 | 89 => (true, true),
            8 | 20 | 21 | 24 | 62 | 63 | 64 | 162 => (true, false),
            _ => (false, false),
        },
        8 | 10 | 30 | 62 | 67 | 75 => (true, false),
        _ => (false, false),
    }
}

/// `GetEffectPoints 0x6e3800`: the effect's `(min, max)` as the soft floats it returns. A `level`
/// of 0 takes the spell's own ([`skill_level`], `6e3841`-`6e3852`); `Δ` is it less `baseLevel`
/// when that is positive, floored at 0 (`6e3854`-`6e3861`). With `n` the dice, the integer
/// `BaseDice + DicePerLevel·Δ`, the bounds start as `BasePoints + n` and `BasePoints +
/// DieSides·n` (`6e3863`, `6e38bb`), each made a soft float plus `RealPointsPerLevel × Δ`
/// through `0x760be0` and `0x760e20`. Unless the context is the aura tooltip's, op 8, then a
/// damage effect's op (22 for aura 3, else 0), then the aura's ([`aura_op`]) apply to each bound
/// through `0x6e6c30`. The tail quantizes both to 1/128, and a rounding effect floors the minimum
/// and ceils the maximum (`6e3a67`).
fn effect_points(d: &SpellDisplay, slot: usize, ctx: &TokenContext, level: u32) -> (f32, f32) {
    let level = if level == 0 {
        skill_level(ctx, d)
    } else {
        level
    } as i32;
    let base_level = d.base_level as i32;
    let delta = if base_level > 0 {
        level.wrapping_sub(base_level)
    } else {
        level
    }
    .max(0);
    let base = d.effect_base_points[slot];
    let dice =
        d.effect_base_dice[slot].wrapping_add(d.effect_dice_per_level[slot].wrapping_mul(delta));
    let sides = d.effect_die_sides[slot];
    let real = soft_float::mul_f32(
        d.effect_real_points_per_level[slot],
        soft_float::int_to_float(delta),
    );
    let mut min = soft_float::add_f32(soft_float::int_to_float(base.wrapping_add(dice)), real);
    let mut max = soft_float::add_f32(
        soft_float::int_to_float(base.wrapping_add(sides.wrapping_mul(dice))),
        real,
    );
    let aura = d.effect_apply_aura[slot];
    let (rounds, damage) = classify(d.effects[slot], aura);
    if let Some(mods) = ctx.mods.filter(|_| !ctx.unmodified_points) {
        let damage_op = damage.then_some(if aura == 3 { 22 } else { 0 });
        for op in [Some(8), damage_op, aura_op(d, slot)].into_iter().flatten() {
            min = mods.apply_soft(d, op, min);
            max = mods.apply_soft(d, op, max);
        }
    }
    let (min, max) = (soft_float::quantize(min), soft_float::quantize(max));
    if rounds {
        soft_float::floor_ceil(min, max)
    } else {
        (min, max)
    }
}

/// The aura's own op (`0x6e397e`, jump table `0x6e3ab8` over the byte table `0x6e3ad0`, indexed
/// by `aura − 10`): 10/103/183 op 2 (`6e3996`), the speed auras op 12 (`6e39a6`), 138 op 23
/// (`6e39b6`), 65 op 24 (`6e39c6`), 99 op 3 (`6e39d6`).
fn aura_op(d: &SpellDisplay, slot: usize) -> Option<u8> {
    match d.effect_apply_aura[slot] {
        10 | 103 | 183 => Some(2),
        31 | 32 | 33 | 58 | 129 | 130 | 171 | 172 => Some(12),
        138 => Some(23),
        65 => Some(24),
        99 => Some(3),
        _ => None,
    }
}

fn modify_int(ctx: &TokenContext, d: &SpellDisplay, op: u8, value: i32) -> i32 {
    ctx.mods.map_or(value, |m| m.apply_int(d, op, value))
}

fn modify_float(ctx: &TokenContext, d: &SpellDisplay, op: u8, value: f32) -> f32 {
    ctx.mods.map_or(value, |m| m.apply_float(d, op, value))
}

/// A spell's duration in ms, `GetSpellDuration 0x6ea000`, which `$d` and `$o` both call: 0 with
/// no row (`6ea032`), else the row's base plus its per-level term times the spell's own level ([`skill_level`], `6ea041`)
/// less `baseLevel`, subtracted and multiplied unfloored (`6ea046`-`6ea053`), capped at the row's
/// maximum (`6ea055`), then op 1 unless its flag says not to (`6ea064`).
fn duration_ms(d: &SpellDisplay, ctx: &TokenContext) -> i64 {
    let Some(row) = ctx.durations.get(d.duration_index) else {
        return 0;
    };
    let levels = (skill_level(ctx, d) as i32).wrapping_sub(d.base_level as i32);
    let resolved = row
        .base_ms
        .wrapping_add(levels.wrapping_mul(row.per_level_ms))
        .min(row.max_ms);
    i64::from(if ctx.unmodified_points {
        resolved
    } else {
        modify_int(ctx, d, 1, resolved)
    })
}

/// The `$d` text: no positive duration is `SPELL_DURATION_UNTIL_CANCELLED` (`507cda`); else the
/// formatter at `0x52f980` selects the integer ladder (`0x52fa50`) for whole units and the
/// floating ladder (`0x52fbd0`) for fractional units. Both use the largest unit.
fn duration_text(ms: i64, ctx: &TokenContext) -> Option<String> {
    if ms <= 0 {
        return keyed(ctx, "SPELL_DURATION_UNTIL_CANCELLED", &[]);
    }
    let (unit, unit_ms) = if ms < 60_000 {
        ("SEC", 1_000)
    } else if ms < 3_600_000 {
        ("MIN", 60_000)
    } else if ms < 86_400_000 {
        ("HOURS", 3_600_000)
    } else {
        ("DAYS", 86_400_000)
    };
    let amount = ms as f64 / unit_ms as f64;
    if (amount - amount.trunc()).abs() >= 0.01 {
        return keyed(
            ctx,
            &format!("SPELL_DURATION_{unit}"),
            &[TokenNumber::Float(amount)],
        );
    }
    let n = ms / unit_ms;
    let key = format!("INT_SPELL_DURATION_{unit}");
    (n != 1)
        .then(|| keyed(ctx, &format!("{key}_P1"), &[TokenNumber::Int(n)]))
        .flatten()
        .or_else(|| keyed(ctx, &key, &[TokenNumber::Int(n)]))
}

/// The expander's integer read of a scaled bound (`5079cc`-`507b04`): the bound truncated, and
/// whether it counts as whole, within 0.001 (`[0x801360]`) of its floor or its ceiling. A bound
/// just under its ceiling reads as the ceiling.
fn whole(v: f32) -> (i32, bool) {
    let x = f64::from(v);
    let eps = f64::from(0.001f32);
    let truncated = x.floor() as i32;
    let near_ceil = x.ceil() - x < eps;
    if x - x.floor() < eps || near_ceil {
        let up = near_ceil && truncated != x.ceil() as i32;
        (truncated + i32::from(up), true)
    } else {
        (truncated, false)
    }
}

/// The `$s $S $m $M $o $O` arm (`0x5078b4`): the effect points, spread over the duration for `$o`
/// (`507926`: duration × points / period, 0 without a positive period or duration, a period of
/// 0 read as 5000), then made positive and scaled (`507952`) and printed by the letter's rule
/// (`507b1a`-`507c4a`). The text and the integer the `$l` plural picker keys on (`[0xbe0b84]`).
fn points_text(
    letter: char,
    slot: usize,
    d: &SpellDisplay,
    ctx: &TokenContext,
    scale: f32,
    level: u32,
) -> Option<(String, f64)> {
    // The expander multiplies both by its integer argument here (`5078d1`), which is 1 but in
    // the aura tooltip's stack count; no caller passes a count.
    let (mut min, mut max) = effect_points(d, slot, ctx, level);
    if letter.eq_ignore_ascii_case(&'o') {
        let amplitude = d.effect_amplitude[slot] as i32;
        let period = if amplitude == 0 { 5000 } else { amplitude };
        let duration = if period > 0 { duration_ms(d, ctx) } else { 0 };
        (min, max) = if duration > 0 {
            let over = |v: f32| (duration as f64 * f64::from(v) / f64::from(period)) as f32;
            (over(min), over(max))
        } else {
            (0.0, 0.0)
        };
    }
    let scaled = |v: f32| (f64::from(v.abs()) * f64::from(scale)) as f32;
    let (lo, hi) = (scaled(min), scaled(max));
    let ((lo_int, lo_whole), (hi_int, hi_whole)) = (whole(lo), whole(hi));
    let plural = match letter {
        'm' if lo_whole => lo_int,
        'm' => 2,
        _ if hi_whole => hi_int,
        _ => 2,
    };
    let int = |n: i32| TokenNumber::Int(i64::from(n));
    let text = match letter {
        'm' if lo_whole => lo_int.to_string(),
        'm' => tenths(ctx, lo),
        'M' if hi_whole => hi_int.to_string(),
        'M' => tenths(ctx, hi),
        // Equal bounds print one number, `%d` unless `$S` holds a fraction (`507ba3`).
        _ if min == max && (lo_whole || letter != 'S') => lo_int.to_string(),
        _ if min == max => tenths(ctx, lo),
        _ if lo_whole && hi_whole => keyed(
            ctx,
            "INT_SPELL_POINTS_SPREAD_TEMPLATE",
            &[int(lo_int), int(hi_int)],
        )?,
        'S' => keyed(
            ctx,
            "SPELL_POINTS_SPREAD_TEMPLATE",
            &[
                TokenNumber::Float(f64::from(lo)),
                TokenNumber::Float(f64::from(hi)),
            ],
        )?,
        // A fractional spread under `s`/`o`/`O`: the minimum truncated, a fractional maximum
        // truncated plus one (`507c2a`-`507c4a`).
        _ => {
            let top = if hi_whole { hi_int } else { hi as i32 + 1 };
            keyed(
                ctx,
                "INT_SPELL_POINTS_SPREAD_TEMPLATE",
                &[int(lo as i32), int(top)],
            )?
        }
    };
    Some((text, f64::from(plural)))
}

/// One token's text and the numeric value the `$l` plural picker keys on.
fn token_value(
    letter: char,
    slot: usize,
    d: &SpellDisplay,
    ctx: &TokenContext,
    scale: f32,
    level: u32,
) -> Option<(String, f64)> {
    match letter.to_ascii_lowercase() {
        's' | 'm' | 'o' => points_text(letter, slot, d, ctx, scale, level),
        'd' => {
            let ms = duration_ms(d, ctx);
            let v = if ms < 0 { 0.0 } else { ms as f64 / 1000.0 };
            Some((duration_text(ms, ctx)?, v))
        }
        't' => {
            // `507e3c`: ProcFlags bit 0 is five seconds unmodified, else the amplitude through
            // op 19, zero included (`507e5c`); whole seconds, truncated (`507e61`).
            let ms = if d.proc_flags & 1 != 0 {
                5000
            } else {
                modify_int(ctx, d, 19, d.effect_amplitude[slot] as i32)
            };
            let secs = ms / 1000;
            Some((secs.to_string(), f64::from(secs)))
        }
        'a' => {
            // `507c74`: the SpellRadius row's radius truncated, then op 6; no row is 0.
            let r = ctx.radii.get(d.effect_radius_index[slot]);
            let v = r.map_or(0, |r| modify_int(ctx, d, 6, r.radius as i32));
            Some((v.to_string(), f64::from(v)))
        }
        'h' => {
            let v = modify_int(ctx, d, 18, d.proc_chance as i32);
            Some((v.to_string(), f64::from(v)))
        }
        'x' => {
            let v = *d.effect_chain_targets.get(slot).unwrap_or(&0);
            let v = modify_int(ctx, d, 17, v as i32);
            Some((v.to_string(), f64::from(v)))
        }
        'e' => {
            // `507f83`: EffectMultipleValue through op 27, always one decimal.
            let v = modify_float(ctx, d, 27, d.effect_multiple_value[slot]);
            Some((tenths(ctx, v), f64::from(v)))
        }
        'n' => {
            let v = modify_int(ctx, d, 4, d.proc_charges as i32);
            Some((v.to_string(), f64::from(v)))
        }
        'z' => {
            // Player state, not spell data: the home-bind area name.
            let name = ctx.home_area?;
            Some((name.to_string(), 0.0))
        }
        'r' => {
            // `507d5b`: RangeIndex at or below 1 reads row 1, its maximum through op 5; no row is
            // 0.0. One decimal, and the plural keys on the ceiling (`507dbd`).
            let row = ctx.ranges?.get(d.range_index.max(1));
            let v = row.map_or(0.0, |r| modify_float(ctx, d, 5, r.max));
            Some((tenths(ctx, v), f64::from(v).ceil()))
        }
        _ => None,
    }
}

/// `$/N;` or `$*N;` just after the `$` (`507738`-`507790`): the text up to the `;` (at most seven
/// characters) read by `atoi` (`0x64ac60`), the scale its value or, for `/`, its reciprocal in
/// single precision (`507784`); a zero leaves 1.0 (`507774`). Returns the scale and the index past
/// the `;`, or `None` without a `;`, where the reference consumes nothing.
fn scale_prefix(text: &str, at: usize) -> Option<(f32, usize)> {
    let op = *text.as_bytes().get(at)?;
    if op != b'/' && op != b'*' {
        return None;
    }
    let semi = at + text[at..].find(';')?;
    let digits = &text.as_bytes()[at + 1..semi.min(at + 8)];
    let n = atoi(digits);
    let scale = match (n, op) {
        (0, _) => 1.0,
        (n, b'/') => (1.0 / f64::from(n)) as f32,
        (n, _) => n as f32,
    };
    Some((scale, semi + 1))
}

/// The CRT's `atoi`: leading whitespace, an optional sign, then decimal digits up to the first
/// other character; nothing parsed is 0.
fn atoi(bytes: &[u8]) -> i32 {
    let mut rest = bytes
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .peekable();
    let negative = match rest.peek() {
        Some(b'-') => {
            rest.next();
            true
        }
        Some(b'+') => {
            rest.next();
            false
        }
        _ => false,
    };
    let n = rest.take_while(u8::is_ascii_digit).fold(0i32, |n, b| {
        n.wrapping_mul(10).wrapping_add(i32::from(b - b'0'))
    });
    if negative {
        n.wrapping_neg()
    } else {
        n
    }
}

/// Substitute every `$`-token in `text` against `spell`.
pub fn substitute(text: &str, spell: &SpellDisplay, ctx: &TokenContext) -> String {
    // `0x5075f0`'s level: its caller's, which only the player-buff tooltip passes (the aura's
    // `AURALEVELS` byte, `0x532bc3`, not built), else the expanded spell's own (`50764a`).
    let level = skill_level(ctx, spell);
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
        let mut scale = 1.0f32;
        if let Some((s, next)) = scale_prefix(text, i) {
            scale = s;
            i = next;
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
        // A cross-spell token caps the level at its row's positive `maxLevel` (`507805`-`507817`).
        let (target, level): (&SpellDisplay, u32) = match ref_spell {
            Some(id) => match (ctx.lookup)(id) {
                Some(s) if s.max_level > 0 => (s, level.min(s.max_level)),
                Some(s) => (s, level),
                None => {
                    out.push_str(&text[start..i]);
                    continue;
                }
            },
            None => (spell, level),
        };
        match token_value(letter, slot, target, ctx, scale, level) {
            Some((sub, val)) => {
                // Deviation: every token keys the `$l` plural. The reference's `$a`, `$d`, `$t`,
                // `$e`, `$c`, `$p`, `$f`, `$F` and `$z` arms never write `[0xbe0b84]`, so its `$l`
                // keys on the number before them, and Blizzard's "$s1 … every $t1
                // $lsecond:seconds;" reads "every 1 seconds" in 1.12.1. We print the grammar the
                // text means.
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
    fn global(key: &str) -> Option<String> {
        Some(
            match key {
                "SPELL_DURATION_UNTIL_CANCELLED" => "<forever>",
                "INT_SPELL_DURATION_SEC" => "<%dsec>",
                "INT_SPELL_DURATION_MIN" => "<%dmin>",
                "INT_SPELL_DURATION_HOURS" => "<%dhour>",
                "INT_SPELL_DURATION_HOURS_P1" => "<%dhrs>",
                "INT_SPELL_DURATION_DAYS" => "<%ddays>",
                "SPELL_DURATION_MIN" => "<%.2fmin>",
                "INT_SPELL_POINTS_SPREAD_TEMPLATE" => "<%d..%d>",
                "SPELL_POINTS_SPREAD_TEMPLATE" => "<%.1f to %.1f>",
                _ => return None,
            }
            .into(),
        )
    }

    /// `%d`, `%.Nf` and `%%` in order; the tests keep clear of the CRT's rounding ties.
    fn printf(template: &str, args: &[TokenNumber]) -> String {
        let mut out = String::new();
        let mut args = args.iter();
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            let mut spec = String::new();
            while let Some(&n) = chars.peek() {
                chars.next();
                spec.push(n);
                if n.is_ascii_alphabetic() || n == '%' {
                    break;
                }
            }
            match (spec.as_str(), args.next()) {
                ("d", Some(TokenNumber::Int(n))) => out.push_str(&n.to_string()),
                (f, Some(TokenNumber::Float(x))) if f.starts_with('.') => {
                    let p: usize = f[1..f.len() - 1].parse().unwrap();
                    out.push_str(&format!("{x:.p$}"));
                }
                (s, a) => panic!("printf: %{s} with {a:?}"),
            }
        }
        out
    }

    /// A caster's table: `(op, flat, pct)` rows through the three appliers' arithmetic.
    struct Mods(Vec<(u8, i32, i32)>);

    impl Mods {
        fn find(&self, op: u8) -> Option<(i32, i32)> {
            self.0
                .iter()
                .find(|m| m.0 == op)
                .map(|&(_, flat, pct)| (flat, pct))
        }
    }

    impl SpellMods for Mods {
        fn apply_int(&self, _: &SpellDisplay, op: u8, value: i32) -> i32 {
            self.find(op)
                .map_or(value, |(flat, pct)| (value + flat) * pct / 100)
        }
        fn apply_float(&self, _: &SpellDisplay, op: u8, value: f32) -> f32 {
            self.find(op).map_or(value, |(flat, pct)| {
                ((f64::from(value) + f64::from(flat)) * f64::from(pct) * f64::from(0.01f32)) as f32
            })
        }
        fn apply_soft(&self, _: &SpellDisplay, op: u8, value: f32) -> f32 {
            self.find(op)
                .map_or(value, |(flat, pct)| crate::soft_modify(value, flat, pct))
        }
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
            ranges: None,
            skill: &|_| 0,
            lookup,
            mods: None,
            unmodified_points: false,
            global: &global,
            printf: &printf,
        }
    }

    fn none_lookup<'a>(_: u32) -> Option<&'a SpellDisplay> {
        None
    }

    /// A flat effect of `points` in slot 0 (base `points − 1`, one one-sided die).
    fn points(points: i32) -> SpellDisplay {
        SpellDisplay {
            effect_base_points: [points - 1, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [1, 0, 0],
            ..Default::default()
        }
    }

    /// An effect rolling `min..=max` in slot 0.
    fn spread(min: i32, max: i32) -> SpellDisplay {
        SpellDisplay {
            effect_base_points: [min - 1, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [max - min + 1, 0, 0],
            ..Default::default()
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
            (7, 0),
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
        // No positive duration, and no row, which `0x6ea000` reads as 0 (`507cda`).
        assert_eq!(d(7), "<forever>");
        assert_eq!(d(8), "<forever>");
    }

    #[test]
    fn duration_modifier_updates_description_and_overtime_total() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 120_000);
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(1, 0, 150)]);
        let c = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let d = SpellDisplay {
            duration_index: 1,
            effect_amplitude: [3_000, 0, 0],
            ..points(3)
        };
        assert_eq!(
            substitute("Lasts $d; deals $o1 total.", &d, &c),
            "Lasts <3min>; deals 180 total."
        );
    }

    #[test]
    fn battle_shout_description_uses_modified_duration_from_real_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let durations = crate::load_spell_durations(&mut chain).expect("SpellDuration.dbc");
        let radii = SpellRadiusCatalog::default();
        let d = spells.get(6673).expect("Battle Shout rank 1");
        let description = d.description.as_deref().expect("description");
        let lookup = |id| spells.get(id);
        let base = ctx(&durations, &radii, &lookup);
        assert!(substitute(description, d, &base).contains("<2min>"));
        let mods = Mods(vec![(1, 0, 150)]);
        let modified = TokenContext {
            mods: Some(&mods),
            ..base
        };
        assert!(substitute(description, d, &modified).contains("<3min>"));
    }

    #[test]
    fn booming_voice_ranks_preserve_fractional_minutes() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 120_000);
        let radii = SpellRadiusCatalog::default();
        let spell = SpellDisplay {
            duration_index: 1,
            ..Default::default()
        };
        for (rank, expected) in [
            (0, "<2min>"),
            (1, "<2.20min>"),
            (2, "<2.40min>"),
            (3, "<2.60min>"),
            (4, "<2.80min>"),
            (5, "<3min>"),
        ] {
            let mods = Mods(vec![(1, 0, 100 + rank * 10)]);
            let ctx = TokenContext {
                mods: Some(&mods),
                ..ctx(&durations, &radii, &none_lookup)
            };
            assert_eq!(substitute("$d", &spell, &ctx), expected, "rank {rank}");
        }
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
    fn scaled_effect_points_keep_fractional_seconds() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        for (n, expected) in [(100, "0.1"), (200, "0.2"), (500, "0.5")] {
            let d = points(n);
            assert_eq!(
                substitute("$/1000;S1 sec.", &d, &c),
                format!("{expected} sec.")
            );
            assert_eq!(
                substitute("$/1000;m1 sec.", &d, &c),
                format!("{expected} sec.")
            );
        }
        // A whole scaled value prints `%d` under any letter.
        assert_eq!(substitute("$/1000;S1", &points(1000), &c), "1");
    }

    /// Flametongue Weapon's shape (`$/77;8026m1 to $/25;8026M1` over 326): `m`/`M` print a
    /// fraction at one decimal. The scale is `atoi`'s, so a decimal `$*` reads its integer part
    /// and `$*0.04;` is no scale at all.
    #[test]
    fn scaled_m_tokens_print_tenths_and_the_scale_is_an_integer() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let d = points(326);
        assert_eq!(
            substitute("$/77;m1 to $/25;M1 additional Fire damage", &d, &c),
            "4.2 to 13.0 additional Fire damage"
        );
        assert_eq!(substitute("$*0.04;M1", &d, &c), "326");
        assert_eq!(substitute("$*2;M1", &d, &c), "652");
        assert_eq!(substitute("$/0;M1", &d, &c), "326", "atoi 0 leaves 1.0");
        assert_eq!(
            substitute("$/2 x;M1", &d, &c),
            "163",
            "atoi stops at the space"
        );
        assert_eq!(substitute("$/2 M1", &d, &c), "$/2 M1", "no `;`, no token");
    }

    /// `$/2;s1` of 3 truncates to 1 (`507b28`), where `$/2;S1` keeps its fraction.
    #[test]
    fn a_fractional_single_value_truncates_under_s_and_keeps_tenths_under_capital_s() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let d = points(3);
        assert_eq!(substitute("$/2;s1", &d, &c), "1");
        assert_eq!(
            substitute("$/2;o1", &d, &c),
            "0",
            "no duration spreads nothing"
        );
        assert_eq!(substitute("$/2;S1", &d, &c), "1.5");
        assert_eq!(substitute("$/2;m1 $/2;M1", &d, &c), "1.5 1.5");
    }

    /// The spread arms (`507bc2`-`507c4a`): both whole takes the integer template; else `$S`
    /// takes the float one, and `s` the integer one over the truncated minimum and a fractional
    /// maximum truncated plus one.
    #[test]
    fn a_spread_picks_its_template_by_letter_and_wholeness() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        assert_eq!(substitute("$/2;s1", &spread(4, 6), &c), "<2..3>");
        assert_eq!(substitute("$/2;s1", &spread(3, 5), &c), "<1..3>");
        assert_eq!(substitute("$/2;S1", &spread(3, 5), &c), "<1.5 to 2.5>");
        assert_eq!(substitute("$/2;s1", &spread(4, 5), &c), "<2..3>");
        assert_eq!(substitute("$/2;S1", &spread(4, 5), &c), "<2.0 to 2.5>");
        assert_eq!(substitute("$/2;O1", &spread(4, 5), &c), "0");
    }

    /// The plural value `[0xbe0b84]`: `m` keys on its whole minimum, the others on the whole
    /// maximum, and a fraction keys as 2 (`507aa2`-`507b10`).
    #[test]
    fn the_plural_keys_on_the_whole_bound_or_two() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let plural = |text: &str, d: &SpellDisplay| substitute(text, d, &c);
        assert_eq!(plural("$m1 $lpoint:points;", &spread(1, 3)), "1 point");
        assert_eq!(
            plural("$s1 $lpoint:points;", &spread(1, 3)),
            "<1..3> points"
        );
        assert_eq!(plural("$M1 $lpoint:points;", &points(1)), "1 point");
        assert_eq!(plural("$/2;m1 $lpoint:points;", &points(2)), "1 point");
        assert_eq!(plural("$/4;S1 $lpoint:points;", &points(2)), "0.5 points");
    }

    /// `0x6e3b80`'s rounding set: School Damage floors the minimum and ceils the maximum after
    /// its modifiers, where an unrounded effect keeps the quantized fraction; a zero maximum
    /// ceils to 1 (`0x761040`).
    #[test]
    fn a_rounding_effect_floors_and_ceils_its_modified_bounds() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(0, 0, 110)]);
        let c = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let school = SpellDisplay {
            effects: [2, 0, 0],
            ..spread(14, 22)
        };
        assert_eq!(substitute("$s1", &school, &c), "<15..25>");
        assert_eq!(substitute("$m1 $M1", &school, &c), "15 25");
        let zero = SpellDisplay {
            effects: [2, 0, 0],
            ..points(0)
        };
        assert_eq!(
            substitute("$s1", &zero, &ctx(&durations, &radii, &none_lookup)),
            "<0..1>"
        );
        // A dummy effect is not in the set: op 0 does not reach it, and nothing rounds.
        let dummy = SpellDisplay {
            effects: [3, 0, 0],
            ..spread(14, 22)
        };
        assert_eq!(substitute("$s1", &dummy, &c), "<14..22>");
    }

    /// The damage op is 22 for aura 3 and 0 for the other damage auras (`6e394a`).
    #[test]
    fn a_periodic_damage_aura_takes_op_22() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(22, 0, 200)]);
        let c = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let aura = |aura| SpellDisplay {
            effects: [6, 0, 0],
            effect_apply_aura: [aura, 0, 0],
            ..points(10)
        };
        assert_eq!(substitute("$s1", &aura(3), &c), "20");
        assert_eq!(substitute("$s1", &aura(15), &c), "10");
    }

    /// `GetEffectPoints` modifies through the software-float applier `0x6e6c30`, whose truncating
    /// steps land 269 × 1.01 × 1.33 on 361.34375 after the 1/128 quantizer, where the FPU applier
    /// would reach 361.3515625 and print "361.4".
    #[test]
    fn effect_points_take_the_software_float_applier() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(8, 0, 101), (2, 0, 133)]);
        let c = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let d = SpellDisplay {
            effects: [6, 0, 0],
            effect_apply_aura: [10, 0, 0],
            ..points(269)
        };
        assert_eq!(substitute("$M1", &d, &c), "361.3");
    }

    /// The aura tooltip's context skips the effect points' modifiers and `$d`'s op 1, and keeps
    /// the other tokens' ops.
    #[test]
    fn the_aura_tooltip_context_leaves_points_and_duration_unmodified() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 60_000);
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(8, 0, 125), (1, 0, 200), (18, 10, 100)]);
        let modified = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let aura = TokenContext {
            unmodified_points: true,
            ..modified
        };
        let d = SpellDisplay {
            duration_index: 1,
            proc_chance: 5,
            ..points(40)
        };
        assert_eq!(substitute("$s1 $d $h", &d, &modified), "50 <2min> 15");
        assert_eq!(substitute("$s1 $d $h", &d, &aura), "40 <1min> 15");
    }

    #[test]
    fn scaled_damage_tokens_from_real_spell_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let durations = crate::load_spell_durations(&mut chain).expect("SpellDuration.dbc");
        let radii = SpellRadiusCatalog::default();
        let lookup = |id| spells.get(id);
        let c = ctx(&durations, &radii, &lookup);
        for (id, expected) in [
            (8024, "4.2 to 13.0 additional Fire damage"),
            (20154, "an additional 1 to 4 Holy damage"),
            // `$/1000;s1` of 1500 and of 600: lowercase `s` truncates a fraction (`507b28`).
            (21854, "Enslave Demon spell by 1 sec."),
            (24271, "Raptor Strike by 0 sec."),
        ] {
            let d = spells.get(id).expect("spell");
            let description = substitute(d.description.as_deref().unwrap(), d, &c);
            assert!(description.contains(expected), "{id}: {description}");
        }
    }

    #[test]
    fn improved_frostbolt_ranks_display_tenths_from_real_spell_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let catalog = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let lookup = |id| catalog.get(id);
        let c = ctx(&durations, &radii, &lookup);
        for (id, expected) in [
            (11070, "0.1"),
            (12473, "0.2"),
            (16763, "0.3"),
            (16765, "0.4"),
            (16766, "0.5"),
        ] {
            let d = catalog.get(id).expect("Improved Frostbolt rank");
            assert!(d.name == "Improved Frostbolt");
            let description = substitute(d.description.as_deref().unwrap(), d, &c);
            assert!(
                description.contains(&format!("by {expected} sec.")),
                "{description}"
            );
        }
    }

    /// The per-level terms scale by the skill level (`0x6e3130`): the skill capped at
    /// `maxLevel × 5`, over 5 in integers, less `baseLevel` floored at 0. Battle Shout rank 1
    /// (14 + 1d1, 0.5 a level from 1, cap 11), Power Word: Shield rank 1 (43 + 1d1, 0.8 from 6,
    /// cap 11), Fireball rank 1 (13 + 1d9, 0.6 from 1, cap 5, a rounding effect).
    #[test]
    fn per_level_terms_scale_by_the_skill_level_from_real_spells() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let lookup = |id| spells.get(id);
        let skill = std::cell::Cell::new(0);
        let skill_of = |_| skill.get();
        let c = TokenContext {
            skill: &skill_of,
            ..ctx(&durations, &radii, &lookup)
        };
        let at = |value: u32, id: u32| {
            skill.set(value);
            substitute("$s1", spells.get(id).unwrap(), &c)
        };
        // Levels 1, 11 and 60 at a class line's level × 5, and the cap at 11.
        for (skill, expected) in [(0, "15"), (5, "15"), (55, "20"), (300, "20")] {
            assert_eq!(at(skill, 6673), expected, "Battle Shout at skill {skill}");
        }
        assert_eq!(at(300, 17), "48");
        // Level 3 is under baseLevel 6: no negative term.
        assert_eq!(at(15, 17), "44");
        assert_eq!(at(0, 133), "<14..22>");
        // Skill 24 is level 4, not 4.8: 14 + 1.8 floored, 22 + 1.8 ceiled.
        assert_eq!(at(24, 133), "<15..24>");
        // 14 + 2.4 floored, 22 + 2.4 ceiled (`6e3a67`).
        assert_eq!(at(300, 133), "<16..25>");
    }

    /// A `baseLevel` at or below 0 is not subtracted (`6e3859`): the level itself is Δ.
    #[test]
    fn a_base_level_at_or_below_zero_is_not_subtracted() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let d = SpellDisplay {
            base_level: -1i32 as u32,
            effect_real_points_per_level: [1.0, 0.0, 0.0],
            ..points(10)
        };
        let c = TokenContext {
            skill: &|_| 50,
            ..ctx(&durations, &radii, &none_lookup)
        };
        assert_eq!(substitute("$s1", &d, &c), "20");
    }

    /// A cross-spell token takes the expanded spell's level capped at its own row's `maxLevel`
    /// (`507805`); only a level of 0 falls through to the referenced spell's own (`6e3841`).
    #[test]
    fn a_cross_spell_token_caps_the_outer_level() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let per_level = |id: u32, max_level: u32| SpellDisplay {
            id,
            base_level: 1,
            max_level,
            effect_real_points_per_level: [1.0, 0.0, 0.0],
            ..points(10)
        };
        let (capped, uncapped) = (per_level(2, 10), per_level(3, 0));
        let lookup = |id: u32| match id {
            2 => Some(&capped),
            3 => Some(&uncapped),
            _ => None,
        };
        let outer = SpellDisplay {
            id: 1,
            ..Default::default()
        };
        // The outer spell at level 30; the referenced ones in no line of the player's.
        let c = TokenContext {
            skill: &|id| if id == 1 { 150 } else { 0 },
            ..ctx(&durations, &radii, &lookup)
        };
        assert_eq!(substitute("$2s1 $3s1", &outer, &c), "19 39");
        // The outer spell at level 0: each takes its own, 100 capped at 10 × 5.
        let c = TokenContext {
            skill: &|id| if id == 1 { 0 } else { 100 },
            ..ctx(&durations, &radii, &lookup)
        };
        assert_eq!(substitute("$2s1 $3s1", &outer, &c), "19 29");
    }

    /// Resurrection Sickness (15007) is the one shipped spell on a per-level duration row: 427,
    /// -600000 ms plus 60000 a level, capped at 600000. In no line of the player's its level is 0,
    /// and a duration at or below 0 reads as until cancelled.
    #[test]
    fn the_duration_takes_its_per_level_term_from_real_data() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let durations = crate::load_spell_durations(&mut chain).expect("SpellDuration.dbc");
        let radii = SpellRadiusCatalog::default();
        let lookup = |id| spells.get(id);
        let sickness = spells.get(15007).expect("Resurrection Sickness");
        assert_eq!(sickness.duration_index, 427);
        let skill = std::cell::Cell::new(0);
        let skill_of = |_| skill.get();
        let c = TokenContext {
            skill: &skill_of,
            ..ctx(&durations, &radii, &lookup)
        };
        for (value, expected) in [
            (0, "<forever>"),
            (50, "<forever>"),
            (55, "<1min>"),
            (150, "<10min>"),
        ] {
            skill.set(value);
            assert_eq!(substitute("$d", sickness, &c), expected, "skill {value}");
        }
    }

    /// Below `baseLevel` the duration's per-level term goes negative: no floor (`6ea046`).
    #[test]
    fn the_duration_level_term_is_not_floored() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_row_for_tests(
            1,
            crate::SpellDuration {
                base_ms: 20_000,
                per_level_ms: 1_000,
                max_ms: 30_000,
            },
        );
        let radii = SpellRadiusCatalog::default();
        let d = SpellDisplay {
            base_level: 10,
            duration_index: 1,
            ..Default::default()
        };
        let skill = std::cell::Cell::new(0);
        let skill_of = |_| skill.get();
        let c = TokenContext {
            skill: &skill_of,
            ..ctx(&durations, &radii, &none_lookup)
        };
        // Level 5, five under: 20 s less 5 s. Level 20: 30 s, the cap.
        skill.set(25);
        assert_eq!(substitute("$d", &d, &c), "<15sec>");
        skill.set(100);
        assert_eq!(substitute("$d", &d, &c), "<30sec>");
    }

    #[test]
    fn dice_per_level_reaches_spread_and_overtime_tokens() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 9_000);
        let radii = SpellRadiusCatalog::default();
        let spell = SpellDisplay {
            base_level: 1,
            max_level: 3,
            duration_index: 1,
            effect_base_points: [10, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [3, 0, 0],
            effect_dice_per_level: [1, 0, 0],
            effect_amplitude: [3_000, 0, 0],
            ..Default::default()
        };
        // Level 20, capped at 3: two more dice.
        let c = TokenContext {
            skill: &|_| 100,
            ..ctx(&durations, &radii, &none_lookup)
        };
        assert_eq!(substitute("$s1; $o1", &spell, &c), "<13..19>; <39..57>");
    }

    #[test]
    fn overtime_duration_period_scale_plural() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 18_000);
        let radii = SpellRadiusCatalog::default();
        let d = SpellDisplay {
            duration_index: 1,
            effect_amplitude: [3000, 0, 0],
            ..points(3)
        };
        let c = ctx(&durations, &radii, &none_lookup);
        assert_eq!(
            substitute("Deals $o1 damage over $d, every $t1 sec.", &d, &c),
            "Deals 18 damage over <18sec>, every 3 sec."
        );
        assert_eq!(
            substitute("Restores $/2;S1 health: $l point:points;.", &d, &c),
            // Uppercase S keeps 3/2 fractional; the plural picks "points".
            "Restores 1.5 health: points.".to_string()
        );
    }

    #[test]
    fn cross_spell_and_unknown_tokens() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let other = points(100);
        let lookup = |id: u32| -> Option<&SpellDisplay> { (id == 1234).then_some(&other) };
        let d = SpellDisplay::default();
        let c = TokenContext {
            home_area: Some("Goldshire"),
            ..ctx(&durations, &radii, &lookup)
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
        let unbound = ctx(&durations, &radii, &lookup);
        assert_eq!(
            substitute("Returns you to $z.", &d, &unbound),
            "Returns you to $z."
        );
    }

    /// Devotion Aura rank 1's shape: 55 armor, and Improved Devotion Aura's +25% on op 8 through
    /// the soft applier and the 1/128 quantizer is 68.75, which `$s1` truncates and `$M1` prints
    /// at one decimal.
    #[test]
    fn effect_value_tokens_take_the_all_effects_modifier() {
        let d = SpellDisplay {
            effects: [35, 0, 0],
            effect_apply_aura: [22, 0, 0],
            ..points(55)
        };
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(8, 0, 125)]);
        let ctx = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        assert_eq!(substitute("Gives $s1 armor.", &d, &ctx), "Gives 68 armor.");
        assert_eq!(
            substitute("Increases armor by $M1.", &d, &ctx),
            "Increases armor by 68.8."
        );
    }

    /// Every aura `0x6e3ab8`'s table covers (10..=183), and none outside it.
    #[test]
    fn each_aura_takes_the_op_of_its_jump_table_arm() {
        for aura in 0..=255u32 {
            let expected = match aura {
                10 | 103 | 183 => Some(2),
                31 | 32 | 33 | 58 | 129 | 130 | 171 | 172 => Some(12),
                138 => Some(23),
                65 => Some(24),
                99 => Some(3),
                _ => None,
            };
            let d = SpellDisplay {
                effect_apply_aura: [aura, 0, 0],
                ..Default::default()
            };
            assert_eq!(aura_op(&d, 0), expected, "aura {aura}");
        }
    }

    /// `$t` (`507e3c`): ProcFlags bit 0 is five seconds whatever the amplitude; else the
    /// amplitude through op 19, zero included, in whole seconds.
    #[test]
    fn t_is_whole_seconds_of_the_modified_amplitude() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let mods = Mods(vec![(19, -1000, 100)]);
        let c = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let tick = |amplitude, proc_flags| SpellDisplay {
            effect_amplitude: [amplitude, 0, 0],
            proc_flags,
            ..Default::default()
        };
        let bare = ctx(&durations, &radii, &none_lookup);
        assert_eq!(substitute("$t1", &tick(1500, 0), &bare), "1");
        assert_eq!(substitute("$t1", &tick(0, 0), &bare), "0");
        assert_eq!(substitute("$t1", &tick(3000, 0), &c), "2");
        assert_eq!(substitute("$t1", &tick(3000, 1), &c), "5");
    }

    /// The `$l` deviation in [`substitute`]: Blizzard's wording keys its plural on the `$t` just
    /// before it, "every 1 second", where 1.12.1 keys on the damage and reads "every 1 seconds".
    #[test]
    fn the_plural_keys_on_the_nearest_token_a_deviation() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let blizzard = SpellDisplay {
            effect_base_points: [24, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_die_sides: [1, 0, 0],
            effect_amplitude: [1000, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            substitute(
                "$s1 Frost damage every $t1 $lsecond:seconds;",
                &blizzard,
                &c
            ),
            "25 Frost damage every 1 second"
        );
    }

    /// `$r` (`507d5b`) reads row 1 for an index at or below 1 and prints one decimal through
    /// op 5; `$e` (`507f83`) prints one decimal through op 27; `$a` (`507c74`) truncates the
    /// radius, applies op 6 and prints an integer, 0 without a row.
    #[test]
    fn r_e_and_a_follow_their_arms() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::from_rows(
            [(
                7,
                crate::SpellRadius {
                    radius: 8.5,
                    per_level: 0.0,
                    max: 8.5,
                },
            )]
            .into(),
        );
        let range = |max| crate::SpellRange {
            min: 0.0,
            max,
            flags: 0,
        };
        let ranges = SpellRangeCatalog::from_rows([(1, range(5.0)), (4, range(30.0))].into());
        let mods = Mods(vec![(5, 0, 110), (27, 0, 150), (6, 2, 100)]);
        let c = TokenContext {
            ranges: Some(&ranges),
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        let d = |range_index, radius| SpellDisplay {
            range_index,
            effect_radius_index: [radius, 0, 0],
            effect_multiple_value: [2.0, 0.0, 0.0],
            ..Default::default()
        };
        assert_eq!(substitute("$r", &d(4, 0), &c), "33.0");
        assert_eq!(substitute("$r", &d(0, 0), &c), "5.5", "index 0 reads row 1");
        assert_eq!(substitute("$r", &d(9, 0), &c), "0.0", "no row");
        assert_eq!(substitute("$e1", &d(4, 0), &c), "3.0");
        assert_eq!(substitute("$a1", &d(4, 7), &c), "10");
        assert_eq!(substitute("$a1", &d(4, 3), &c), "0", "no row");
    }

    /// Whole within 0.001 of either neighbour, a near-ceiling value reading as the ceiling.
    #[test]
    fn a_bound_is_whole_within_a_thousandth() {
        assert_eq!(whole(3.0), (3, true));
        assert_eq!(whole(2.9995), (3, true));
        assert_eq!(whole(3.0005), (3, true));
        assert_eq!(whole(2.5), (2, false));
        assert_eq!(whole(0.0), (0, true));
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
