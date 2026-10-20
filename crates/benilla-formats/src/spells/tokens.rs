//! The spell-description `$`-token engine the 1.12 client runs over `Spell.dbc` description and
//! aura text (`0x5075f0` → `0x507710`), effect values from `GetEffectPoints 0x6e3800`. Effect
//! point values print unsigned, as the client's do. `$g` and `$G` branch on the active player's
//! gender ([`TokenContext::gender`]). `$c` and `$p` (`507dde`, `507ded`), which no shipped text
//! uses, and unknown or unresolved tokens stay raw.

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
    /// The active player's gender, `UNIT_FIELD_BYTES_0` byte 2 (`0x508214`), which `$g`/`$G`
    /// branch on. Asked only where a branch expands.
    pub gender: &'a dyn Fn() -> u8,
    /// The `$z` token: the home-bind area's name, from `SMSG_BINDPOINTUPDATE`'s area id through
    /// `AreaTable.dbc`; `None` leaves the token raw. Asked only where a `$z` expands, so a caller
    /// can tell which texts read the bind point.
    pub home_area: &'a dyn Fn() -> Option<&'a str>,
    /// `GetText 0x703bf0`: a `GlobalStrings` template by key. `None` leaves the token raw.
    pub global: &'a dyn Fn(&str) -> Option<String>,
    /// `SStrPrintf 0x64a7f0`: a template's `%` holes filled, as the CRT formats them.
    pub printf: &'a dyn Fn(&str, &[TokenNumber]) -> String,
}

/// One piece of an expanded text: literal text, or a `GlobalStrings` template a token names,
/// read and filled when the text is read (`GetText 0x703bf0`, then `SStrPrintf 0x64a7f0`), as the
/// tooltip builders read `_G` at each call.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenPart {
    Text(String),
    Keyed {
        key: String,
        /// `GetText`'s plural ordinal: other than 1, the `_P1` twin when the strings hold one.
        ordinal: Option<i64>,
        args: Vec<TokenNumber>,
        /// The token as written, which stands in for a missing template.
        raw: String,
    },
}

/// What `0x5075f0`'s caller passes beside the text: the level, 0 for the expanded spell's own
/// (`507632`-`50764a`), and the count the effect points multiply by (`5078d1`-`5078e4`). Only the
/// aura tooltip builder `0x52f880` passes other than 0 and 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpandAt {
    pub level: u32,
    pub stack: u32,
}

impl Default for ExpandAt {
    fn default() -> Self {
        Self { level: 0, stack: 1 }
    }
}

/// A token's text before the template read: literal, or a key with its arguments.
enum Piece {
    Text(String),
    Keyed {
        key: String,
        ordinal: Option<i64>,
        args: Vec<TokenNumber>,
    },
}

/// A `GlobalStrings` template by key, filled at the read.
fn keyed(key: impl Into<String>, args: Vec<TokenNumber>) -> Piece {
    Piece::Keyed {
        key: key.into(),
        ordinal: None,
        args,
    }
}

/// The text of `parts` with each template read through `global` and filled through `printf`; a
/// missing template leaves its token as written.
pub fn resolve_parts(
    parts: &[TokenPart],
    global: &dyn Fn(&str) -> Option<String>,
    printf: &dyn Fn(&str, &[TokenNumber]) -> String,
) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            TokenPart::Text(t) => out.push_str(t),
            TokenPart::Keyed {
                key,
                ordinal,
                args,
                raw,
            } => {
                let template = ordinal
                    .filter(|&n| n != 1)
                    .and_then(|_| global(&format!("{key}_P1")))
                    .or_else(|| global(key));
                match template {
                    Some(t) => out.push_str(&printf(&t, args)),
                    None => out.push_str(raw),
                }
            }
        }
    }
    out
}

/// `"%.1f"` (`0x84f398`).
fn tenths(ctx: &TokenContext, v: impl Into<f64>) -> String {
    (ctx.printf)("%.1f", &[TokenNumber::Float(v.into())])
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
fn duration_text(ms: i64) -> Piece {
    if ms <= 0 {
        return keyed("SPELL_DURATION_UNTIL_CANCELLED", Vec::new());
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
            format!("SPELL_DURATION_{unit}"),
            vec![TokenNumber::Float(amount)],
        );
    }
    let n = ms / unit_ms;
    Piece::Keyed {
        key: format!("INT_SPELL_DURATION_{unit}"),
        ordinal: Some(n),
        args: vec![TokenNumber::Int(n)],
    }
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
    at: ExpandAt,
) -> (Piece, f64) {
    // The expander multiplies both by its count here (`5078d1`-`5078e4`), the x87 product of the
    // single and the integer stored back as a single.
    let (min, max) = effect_points(d, slot, ctx, at.level);
    let stacked = |v: f32| (f64::from(v) * f64::from(at.stack)) as f32;
    let (mut min, mut max) = (stacked(min), stacked(max));
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
    let text = |t: String| Piece::Text(t);
    let piece = match letter {
        'm' if lo_whole => text(lo_int.to_string()),
        'm' => text(tenths(ctx, lo)),
        'M' if hi_whole => text(hi_int.to_string()),
        'M' => text(tenths(ctx, hi)),
        // Equal bounds print one number, `%d` unless `$S` holds a fraction (`507ba3`).
        _ if min == max && (lo_whole || letter != 'S') => text(lo_int.to_string()),
        _ if min == max => text(tenths(ctx, lo)),
        _ if lo_whole && hi_whole => keyed(
            "INT_SPELL_POINTS_SPREAD_TEMPLATE",
            vec![int(lo_int), int(hi_int)],
        ),
        'S' => keyed(
            "SPELL_POINTS_SPREAD_TEMPLATE",
            vec![
                TokenNumber::Float(f64::from(lo)),
                TokenNumber::Float(f64::from(hi)),
            ],
        ),
        // A fractional spread under `s`/`o`/`O`: the minimum truncated, a fractional maximum
        // truncated plus one (`507c2a`-`507c4a`).
        _ => {
            let top = if hi_whole { hi_int } else { hi as i32 + 1 };
            keyed(
                "INT_SPELL_POINTS_SPREAD_TEMPLATE",
                vec![int(lo as i32), int(top)],
            )
        }
    };
    (piece, f64::from(plural))
}

/// One token's text and the numeric value the `$l` plural picker keys on.
fn token_value(
    letter: char,
    slot: usize,
    d: &SpellDisplay,
    ctx: &TokenContext,
    scale: f32,
    at: ExpandAt,
) -> Option<(Piece, f64)> {
    let text = |t: String, v: f64| Some((Piece::Text(t), v));
    match letter.to_ascii_lowercase() {
        's' | 'm' | 'o' => Some(points_text(letter, slot, d, ctx, scale, at)),
        'b' => {
            // `507ed4`: `EffectPointsPerComboPoint`, truncated toward zero by `0x40a2b0`.
            let v = d.effect_points_per_combo_point[slot].trunc() as i32;
            text(v.to_string(), f64::from(v))
        }
        'f' => {
            // `507ff7` / `508025`: the scale times `DmgMultiplier`, exact at the x87's PC_53.
            let v = f64::from(scale) * f64::from(d.damage_multiplier[slot]);
            if letter == 'F' {
                // ±0.5 by the sign, then chopped by `0x40a2b0` (`50802f`-`50804a`).
                let n = v.round() as i32;
                text(n.to_string(), f64::from(n))
            } else {
                text(tenths(ctx, v), v)
            }
        }
        'i' => {
            // `507fc3`: `MaxAffectedTargets` through `"%d"`.
            let v = d.max_affected_targets as i32;
            text(v.to_string(), f64::from(v))
        }
        'q' => {
            // `50806e`: the slot's `EffectMiscValue` through `"%d"`.
            let v = d.effect_misc_value[slot];
            text(v.to_string(), f64::from(v))
        }
        'u' => {
            // `507ef1`: `StackAmount` through `"%d"`.
            let v = d.stack_amount as i32;
            text(v.to_string(), f64::from(v))
        }
        'v' => {
            // `507f4f`: `MaxTargetLevel` through `"%d"`.
            let v = d.max_target_level as i32;
            text(v.to_string(), f64::from(v))
        }
        'd' => {
            let ms = duration_ms(d, ctx);
            let v = if ms < 0 { 0.0 } else { ms as f64 / 1000.0 };
            Some((duration_text(ms), v))
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
            text(secs.to_string(), f64::from(secs))
        }
        'a' => {
            // `507c74`: the SpellRadius row's radius truncated, then op 6; no row is 0.
            let r = ctx.radii.get(d.effect_radius_index[slot]);
            let v = r.map_or(0, |r| modify_int(ctx, d, 6, r.radius as i32));
            text(v.to_string(), f64::from(v))
        }
        'h' => {
            let v = modify_int(ctx, d, 18, d.proc_chance as i32);
            text(v.to_string(), f64::from(v))
        }
        'x' => {
            let v = *d.effect_chain_targets.get(slot).unwrap_or(&0);
            let v = modify_int(ctx, d, 17, v as i32);
            text(v.to_string(), f64::from(v))
        }
        'e' => {
            // `507f83`: EffectMultipleValue through op 27, always one decimal.
            let v = modify_float(ctx, d, 27, d.effect_multiple_value[slot]);
            text(tenths(ctx, v), f64::from(v))
        }
        'n' => {
            let v = modify_int(ctx, d, 4, d.proc_charges as i32);
            text(v.to_string(), f64::from(v))
        }
        'z' => {
            // Player state, not spell data: the home-bind area name.
            let name = (ctx.home_area)()?;
            text(name.to_string(), 0.0)
        }
        'r' => {
            // `507d5b`: RangeIndex at or below 1 reads row 1, its maximum through op 5; no row is
            // 0.0. One decimal, and the plural keys on the ceiling (`507dbd`).
            let row = ctx.ranges?.get(d.range_index.max(1));
            let v = row.map_or(0.0, |r| modify_float(ctx, d, 5, r.max));
            text(tenths(ctx, v), f64::from(v).ceil())
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

/// `0x5081c8`-`0x5081d7` and `0x50822a`-`0x50823b`: a `$g`/`$G` branch arm's leading spaces are
/// skipped, and `0x508267`-`0x508280` trims its trailing spaces — both forms alike.
fn trim_spaces(arm: &str) -> &str {
    arm.trim_matches(' ')
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

/// Substitute every `$`-token in `text` against `spell`, each template read through the context.
pub fn substitute(text: &str, spell: &SpellDisplay, ctx: &TokenContext) -> String {
    resolve_parts(
        &substitute_parts(text, spell, ctx, ExpandAt::default()),
        ctx.global,
        ctx.printf,
    )
}

/// Substitute every `$`-token in `text` against `spell` as `0x5075f0` does for a caller passing
/// `at`, leaving each template a token names to be read where the text is shown.
pub fn substitute_parts(
    text: &str,
    spell: &SpellDisplay,
    ctx: &TokenContext,
    at: ExpandAt,
) -> Vec<TokenPart> {
    // `0x5075f0`'s level: its caller's when nonzero (`507635`-`50763e`), else the expanded spell's
    // own (`50764a`).
    let level = if at.level != 0 {
        at.level
    } else {
        skill_level(ctx, spell)
    };
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
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
                        // `$g`/`$G` (`0x508180`): the first form on gender 0, else the second.
                        _ => trim_spaces(if (ctx.gender)() == 0 { a } else { b }),
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
        let at = ExpandAt {
            level,
            stack: at.stack,
        };
        match token_value(letter, slot, target, ctx, scale, at) {
            Some((sub, val)) => {
                // Deviation: every token keys the `$l` plural. The reference's `$a`, `$d`, `$t`,
                // `$e`, `$c`, `$p`, `$f`, `$F` and `$z` arms never write `[0xbe0b84]`, so its `$l`
                // keys on the number before them, and Blizzard's "$s1 … every $t1
                // $lsecond:seconds;" reads "every 1 seconds" in 1.12.1. We print the grammar the
                // text means.
                last_value = val;
                match sub {
                    Piece::Text(t) => out.push_str(&t),
                    Piece::Keyed { key, ordinal, args } => {
                        if !out.is_empty() {
                            parts.push(TokenPart::Text(std::mem::take(&mut out)));
                        }
                        parts.push(TokenPart::Keyed {
                            key,
                            ordinal,
                            args,
                            raw: text[start..i].to_string(),
                        });
                    }
                }
            }
            None => out.push_str(&text[start..i]), // unknown token: keep raw
        }
    }
    if !out.is_empty() {
        parts.push(TokenPart::Text(out));
    }
    parts
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

    /// Every spell-data formatter key needed by a corpus walk; individual formatter tests above
    /// pin the reference's unit and spread choices separately.
    fn corpus_global(key: &str) -> Option<String> {
        global(key).or_else(|| {
            if key.starts_with("INT_SPELL_DURATION_") {
                Some("%d".into())
            } else if key.starts_with("SPELL_DURATION_") {
                Some("%.1f".into())
            } else {
                None
            }
        })
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
            home_area: &|| None,
            durations,
            radii,
            ranges: None,
            skill: &|_| 0,
            lookup,
            mods: None,
            unmodified_points: false,
            gender: &|| 0,
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

    /// `0x508180`'s branch: the first form on gender 0, the second on anything else, the chosen
    /// form trimmed of spaces at both ends (`0x5081c8`-`0x5081d7`, `0x50822a`-`0x50823b`,
    /// `0x508267`-`0x508280`).
    #[test]
    fn gender_takes_the_players_side_of_the_branch() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let d = SpellDisplay::default();
        let gender = std::cell::Cell::new(0u8);
        let c = TokenContext {
            gender: &|| gender.get(),
            ..ctx(&durations, &radii, &none_lookup)
        };

        assert_eq!(substitute("$ghis:her;", &d, &c), "his");
        gender.set(1);
        assert_eq!(substitute("$ghis:her;", &d, &c), "her");
        gender.set(2);
        assert_eq!(substitute("$ghis:her;", &d, &c), "her");
        // `G` is the same branch (`0x508138`'s table maps both spellings to `0x50786d`).
        assert_eq!(substitute("$GLad:LAss;", &d, &c), "LAss");
        // The chosen form's spaces go from both ends.
        gender.set(0);
        assert_eq!(substitute("$g his : her ;", &d, &c), "his");
        gender.set(1);
        assert_eq!(substitute("$g his : her ;", &d, &c), "her");
        // An empty chosen arm consumes the token.
        gender.set(0);
        assert_eq!(substitute("$g:male;", &d, &c), "");
        gender.set(1);
        assert_eq!(substitute("$g:male;", &d, &c), "male");
        assert_eq!(substitute("$g female:;", &d, &c), "");
        // The shipped wording: Conjure Food 587 and Hellfire 1949.
        assert_eq!(
            substitute("providing the mage and $ghis:her; allies", &d, &c),
            "providing the mage and her allies"
        );
        gender.set(0);
        assert_eq!(
            substitute("damage to $ghimself:herself;", &d, &c),
            "damage to himself"
        );
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
            home_area: &|| Some("Goldshire"),
            ..ctx(&durations, &radii, &lookup)
        };
        assert_eq!(
            substitute("as strong as $1234s1 hits", &d, &c),
            "as strong as 100 hits"
        );
        assert_eq!(substitute("stacks $u times", &d, &c), "stacks 0 times");
        assert_eq!(substitute("unknown $j", &d, &c), "unknown $j");
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
        // `$f` and `$F` key it too, though their arms (`507ff7`, `508025`) leave it alone.
        let multiplier = SpellDisplay {
            effect_base_points: [24, 0, 0],
            effect_base_dice: [1, 1, 0],
            effect_die_sides: [1, 1, 0],
            damage_multiplier: [1.0, 1.5, 0.0],
            ..Default::default()
        };
        assert_eq!(
            substitute("$s1 then $F1 $lpoint:points;", &multiplier, &c),
            "25 then 1 point"
        );
        assert_eq!(
            substitute("$s2 then $f2 $lpoint:points;", &multiplier, &c),
            "1 then 1.5 points"
        );
    }

    /// The field arms read the referenced row, `$b`, `$q`, `$f` and `$F` at the slot digit; `$b`,
    /// `$u`, `$v`, `$i` and `$q` print `"%d"` and key the `$l` plural (`507ee0`, `507ef7`,
    /// `507f55`, `507fc9`, `508075`).
    #[test]
    fn field_tokens_read_their_columns_and_plural_counts() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let rows = [
            SpellDisplay {
                id: 42,
                effect_points_per_combo_point: [0.0, -2.9, 0.0],
                damage_multiplier: [0.5, 1.3, -0.25],
                effect_misc_value: [135, -1, 0],
                max_affected_targets: 4,
                stack_amount: 5,
                max_target_level: 40,
                ..Default::default()
            },
            SpellDisplay {
                id: 43,
                effect_points_per_combo_point: [1.9, 0.0, 0.0],
                effect_misc_value: [0, 0, 1],
                max_affected_targets: 1,
                stack_amount: 1,
                max_target_level: 1,
                ..Default::default()
            },
            SpellDisplay {
                id: 44,
                max_affected_targets: 0x8000_0000,
                stack_amount: u32::MAX,
                max_target_level: u32::MAX - 1,
                ..Default::default()
            },
        ];
        let lookup = |id| rows.iter().find(|r| r.id == id);
        let c = ctx(&durations, &radii, &lookup);
        let empty = SpellDisplay::default();
        assert_eq!(substitute("$42b2", &empty, &c), "-2");
        assert_eq!(substitute("$42q1 $42Q2", &empty, &c), "135 -1");
        assert_eq!(substitute("$42i $42u $42v", &empty, &c), "4 5 40");
        assert_eq!(substitute("$42I $42U $42V", &empty, &c), "4 5 40");
        assert_eq!(substitute("$*100;42F1", &empty, &c), "50");
        assert_eq!(substitute("$42f2", &empty, &c), "1.3");
        assert_eq!(substitute("$*10;42F3", &empty, &c), "-3");
        assert_eq!(
            substitute("$44i $44u $44v", &empty, &c),
            "-2147483648 -1 -2"
        );
        for token in ["$43b1", "$43q3", "$43i", "$43u", "$43v"] {
            assert_eq!(
                substitute(&format!("{token} $lone:many;"), &empty, &c),
                "1 one",
                "{token}"
            );
        }
    }

    /// `$f` and `$F` (`507ff7`, `508025`) multiply on the x87 at PC_53, so the product of the two
    /// floats is exact: rounded to a float first, these would read 1.1 and 3.
    #[test]
    fn the_multiplier_tokens_multiply_in_double() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let d = SpellDisplay {
            damage_multiplier: [0.23, 0.833_333_3, 0.0],
            ..Default::default()
        };
        assert_eq!(substitute("$*5;f1", &d, &c), "1.2");
        assert_eq!(substitute("$*3;F2", &d, &c), "2");
    }

    #[test]
    fn shipped_spell_data_tokens_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let durations = crate::load_spell_durations(&mut chain).expect("SpellDuration.dbc");
        let radii = crate::load_spell_radii(&mut chain).expect("SpellRadius.dbc");
        let ranges = crate::load_spell_ranges(&mut chain).expect("SpellRange.dbc");
        let lookup = |id| spells.get(id);
        let c = TokenContext {
            ranges: Some(&ranges),
            global: &corpus_global,
            ..ctx(&durations, &radii, &lookup)
        };
        for (id, text, want) in [
            (1064, "$*100;F1", "50"),
            (10622, "$*100;F1", "50"),
            (10623, "$*100;F1", "50"),
            (22568, "$f1", "1.0"),
            (2006, "$q1", "135"),
            (1680, "$i", "4"),
            (22959, "$u", "5"),
            (453, "$v", "40"),
            (14179, "$b1", "20"),
        ] {
            assert_eq!(
                substitute(text, spells.get(id).unwrap(), &c),
                want,
                "spell {id}"
            );
        }
        for id in [1064, 10622, 10623] {
            let spell = spells.get(id).unwrap();
            assert_eq!(spell.damage_multiplier[0], 0.5, "Chain Heal {id}");
            let description = spell.description.as_deref().unwrap();
            assert!(
                description.contains("$*100;F1"),
                "Chain Heal {id}: {description}"
            );
            assert!(
                substitute(description, spell, &c).contains("50%"),
                "Chain Heal {id} must show 50%"
            );
        }

        // Walk every shipped spell-data token, including other ranks, effect slots, scale
        // prefixes and cross-spell references. `$z` needs the player's home area, not Spell.dbc.
        let mut seen = [0usize; 26];
        for (id, spell) in spells.iter() {
            for description in [
                spell.description.as_deref(),
                spell.aura_description.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                for (arm, token) in spell_data_tokens_in(description) {
                    let expanded = substitute(token, spell, &c);
                    assert_ne!(expanded, token, "spell {id} leaves {token} raw");
                    seen[arm] += 1;
                }
            }
        }
        for &arm in b"bfiquv" {
            assert!(
                seen[usize::from(arm - b'a')] > 0,
                "missing ${} in {seen:?}",
                arm as char
            );
        }
        assert!(
            seen.iter().sum::<usize>() >= 50,
            "too few shipped spell-data tokens: {seen:?}"
        );
    }

    /// Isolate spell-data tokens, including a scale prefix, cross-spell id and one slot digit,
    /// bounded as `substitute` bounds them. `$l`, `$g` and `$z` need other context.
    fn spell_data_tokens_in(text: &str) -> Vec<(usize, &str)> {
        let bytes = text.as_bytes();
        let mut found = Vec::new();
        for start in 0..bytes.len() {
            if bytes[start] != b'$' {
                continue;
            }
            let mut end = start + 1;
            if matches!(bytes.get(end), Some(b'*' | b'/')) {
                let Some(semi) = bytes[end..].iter().position(|&b| b == b';') else {
                    continue;
                };
                end += semi + 1;
            }
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            let Some(letter) = bytes.get(end).copied().map(|b| b.to_ascii_lowercase()) else {
                continue;
            };
            if !b"abdefhimnoqrstuvx".contains(&letter) {
                continue;
            }
            end += 1;
            if bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            found.push((usize::from(letter - b'a'), &text[start..end]));
        }
        found
    }

    #[test]
    fn spell_data_token_scan_keeps_scale_reference_and_slot() {
        assert_eq!(
            spell_data_tokens_in("Each jump is $*100;F1%, stacks $22959u times; $s1 is separate."),
            vec![(5, "$*100;F1"), (20, "$22959u"), (18, "$s1")]
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

    /// A template a token names is left to the read (`0x703bf0` at the call): the parts hold the
    /// key, so a rewrite after the expansion reaches the text, and a key missing at the read
    /// leaves the token as written.
    #[test]
    fn a_tokens_template_is_read_where_the_text_is_shown() {
        let mut durations = SpellDurationCatalog::default();
        durations.insert_for_tests(1, 30_000);
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let d = SpellDisplay {
            duration_index: 1,
            ..Default::default()
        };
        let parts = substitute_parts("for $d.", &d, &c, ExpandAt::default());
        assert_eq!(
            parts,
            [
                TokenPart::Text("for ".into()),
                TokenPart::Keyed {
                    key: "INT_SPELL_DURATION_SEC".into(),
                    ordinal: Some(30),
                    args: vec![TokenNumber::Int(30)],
                    raw: "$d".into(),
                },
                TokenPart::Text(".".into()),
            ]
        );
        let rewritten =
            |key: &str| (key == "INT_SPELL_DURATION_SEC_P1").then(|| "%d seconds".to_string());
        assert_eq!(
            resolve_parts(&parts, &rewritten, &printf),
            "for 30 seconds."
        );
        assert_eq!(resolve_parts(&parts, &|_| None, &printf), "for $d.");
    }

    /// The aura tooltip's caller arguments (`0x52f880` into `0x5075f0`): a nonzero level replaces
    /// the spell's own for the per-level terms (`507635`-`50763e`), and the points multiply by the
    /// count (`5078d1`-`5078e4`) before the spread and the scale.
    #[test]
    fn the_callers_level_and_count_scale_the_points() {
        let durations = SpellDurationCatalog::default();
        let radii = SpellRadiusCatalog::default();
        let c = ctx(&durations, &radii, &none_lookup);
        let text = |d: &SpellDisplay, t: &str, at: ExpandAt| {
            resolve_parts(&substitute_parts(t, d, &c, at), &global, &printf)
        };
        // Rend Flesh (3147): a periodic-damage aura, 1 point plus 1.3 a level over level 1.
        let rend = SpellDisplay {
            effects: [6, 0, 0],
            effect_apply_aura: [3, 0, 0],
            effect_real_points_per_level: [1.3, 0.0, 0.0],
            base_level: 1,
            ..points(1)
        };
        assert_eq!(text(&rend, "$s1", ExpandAt::default()), "1");
        let caster_25 = ExpandAt {
            level: 25,
            stack: 1,
        };
        assert_eq!(text(&rend, "$s1", caster_25), "<32..33>");
        let three = ExpandAt { level: 0, stack: 3 };
        assert_eq!(text(&points(10), "$s1%", three), "30%");
        assert_eq!(text(&spread(14, 22), "$s1", three), "<42..66>");
    }
}
