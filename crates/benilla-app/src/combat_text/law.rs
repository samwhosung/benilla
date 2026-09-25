//! The transcribed WORLDTEXTSTRING law — tables, fade, scale, colors, emitter splits; the
//! knowledge index is the parent module doc (`combat_text`).

use bevy::prelude::*;

/// One config-table row (`0xce8828`, stride 0x1c, filled by `0x6c79a0`): rise span (world units
/// over the full duration), fade-in end / fade-out start / duration (ms), the scale value pair
/// (`valueLo`/`valueHi` — equal except the crit row, whose keyframes ramp `valueHi`), and the
/// default color (ARGB, packed by `0x4a2c10`).
pub(super) struct Category {
    pub(super) rise: f32,
    fade_in_ms: f32,
    fade_out_ms: f32,
    pub(super) dur_ms: f32,
    value_lo: f32,
    value_hi: f32,
    pub(super) color: u32,
}

/// The category scale values, bit-exact (`0.018333` = `0x3c962fc9`, `0.0275` = `0x3ce147ad`).
const VALUE_NORMAL: f32 = f32::from_bits(0x3c96_2fc9);
const VALUE_CRIT: f32 = f32::from_bits(0x3ce1_47ad);

/// The 6 byte-verified rows: 0 normal number · 1 ABSORB word · 2 crit number · 3 miss/dodge/parry
/// word · 4 XP · 5 honor. Row 1's `fade_out(90) < fade_in(150)` is real (a quick flicker, not a
/// decode error); rows 4/5 are the slow colored 4.5 s texts.
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

/// The crit "pop" keyframes (`0x8112dc`, gated on category 2): 3 segments `{t0, t1, s0, s1}` whose
/// interpolated factor multiplies `valueHi` — pop to 2× in the first 10% of life, settle to 1×
/// by 20%.
const CRIT_KEYFRAMES: [(f32, f32, f32, f32); 3] = [
    (0.0, 0.1, 0.1, 2.0),
    (0.1, 0.2, 2.0, 1.0),
    (0.2, 1.0, 1.0, 1.0),
];

/// The localized outcome WORDS, indexed by the client's outcome code 1–11 (`0x86582c` key table →
/// `FrameScript_GetText`) — which is bit-for-bit vmangos's `SpellMissInfo` (`SpellDefines.h:160`).
/// Strings are the shipped enUS `GlobalStrings.lua` values (patch-2.MPQ), hardcoded like the rest
/// of our enUS-only data.
const WORDS: [&str; 11] = [
    "Miss", "Resist", "Dodge", "Parry", "Block", "Evade", "Immune", "Immune", "Deflect", "Absorb",
    "Reflect",
];

/// Outcome code (1–11) → `(word, category)`: category 3 for every word except ABSORB → 1 (the
/// parallel category table `0x80c48c`).
pub(crate) fn miss_word(code: u8) -> Option<(&'static str, u8)> {
    let word = *WORDS.get((code as usize).checked_sub(1)?)?;
    Some((word, if code == 10 { 1 } else { 3 }))
}

/// The emitter override colors, hard-init at `0x5fa0b0`/`0x5fa0f0`: player/pet SPELL damage gold
/// `[0xc4d8a0]`, pet MELEE damage orange `[0xc4d8cc]`. A NULL override falls to the category
/// row's default (rows 0–3: white).
pub(crate) const COLOR_SPELL_GOLD: u32 = 0xFFFF_DE00;
pub(crate) const COLOR_PET_MELEE_ORANGE: u32 = 0xFFFF_8400;

/// **The three floating-combat-text CVar gates, live** — `CombatDamage` (`[0xc4d944]`),
/// `PetMeleeDamage` (`[0xc4d9cc]`) and `PetSpellDamage` (`[0xc4d99c]`), each read as the record's
/// int `+0x28`.
///
/// Each has a **closed reader census** in the reference: one store and exactly two reads
/// image-wide, one in the localized-WORD emitter `0x607140` and one in the `"%d"` NUMBER emitter
/// `0x6128b0`. Both branch targets are function epilogues, so a gate that fails
/// **suppresses the emit entirely** — it never falls through to a default colour, which is why
/// [`damage_color`] returns `Option` rather than a colour.
///
/// `CombatDamage` is the master: off, nothing floats over any unit from any source, and — despite
/// the CVar's own help text saying "damage numbers" — that includes the miss/dodge/parry/block/
/// absorb/resist WORDS, because the word emitter carries the same gate. The `Pet*` pair are
/// downstream sub-gates that only ever see the owned-by-you branch; the self sub-case is
/// unconditional.
///
/// **`PetMeleeDamage` and `PetSpellDamage` do not split on "melee vs spell" the way a player
/// would mean it.** The selector is `B` — no spell record (a real melee swing) *or* the spell's
/// `AttributesEx` bit 15 — so a pet spell carrying that bit is gated by `PetMeleeDamage` and
/// coloured orange. Which spell ids carry it is an open question.
///
/// These were `const bool`s with the comment "consts until a cvar system exists". The CVar system
/// existed; this is the row 2077's census was holding the place for.
#[derive(Resource, Clone, Copy)]
pub(crate) struct DamageTextGates {
    pub(crate) combat_damage: bool,
    pub(crate) pet_melee: bool,
    pub(crate) pet_spell: bool,
}

impl Default for DamageTextGates {
    /// The reference's registered defaults — all three `"1"`.
    fn default() -> Self {
        Self {
            combat_damage: true,
            pet_melee: true,
            pet_spell: true,
        }
    }
}

/// The `0x5efea0` source-ownership classes that may draw (`K`): the active player itself, or a
/// unit it owns (pet/guardian/totem — Summoned/CreatedBy = me). Every other source class (other
/// players, their pets, wild units) is suppressed at the emitter — the caller drops the emit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DamageSource {
    Player,
    Pet,
}

/// The color law's `B` bit — "this damage is melee-STYLED" — from the spell record the emitting
/// call site pushes: `(recordPtr == 0) || sign(byte[SpellRec+0x25])`, i.e. no record at all, or
/// `AttributesEx3` bit 15 ([`benilla_formats::SpellDisplay::melee_white_damage`]). `display` is
/// the row the site resolved; `None` stands for BOTH "the site pushed NULL" (a melee swing, a
/// damage shield) and "we have no catalog" — the client degrades a NULL record to melee-styled,
/// so a missing catalog degrades the same way.
///
/// **It is the WORD emitter's bit as much as the number's**. `0x607140` and
/// `0x6128b0` are separate functions — different arg counts, different register allocation, not
/// the "byte-identical twins" an earlier reading called them — but they compute `B` and `K`
/// identically, and seven of the eight `0x607140` call sites push a resolved SpellRec. Only the
/// melee swing's word site (`0x624511`, whose seven predecessors are all `6a 00 push 0x0`) pushes
/// NULL. So a spell's "Miss"/"Resist" is spell-GOLD exactly like its number, and only a white
/// hit's miss is white.
pub(crate) fn melee_styled(display: Option<&benilla_formats::SpellDisplay>) -> bool {
    display.is_none_or(benilla_formats::SpellDisplay::melee_white_damage)
}

/// The color branch (`0x6128b0` `6128f6`–`612964`): the effective override for a qualifying
/// source, by `B` (`melee` = record NULL; the AttributesEx bit-15 leg is a named divergence) and
/// `K`. `None` = the whole emit is gated off (CombatDamage master, or the pet path's Pet* cvar);
/// `Some(None)` = draw with the category row's default (white); `Some(Some(argb))` = draw with
/// the override. Crit never enters this pick.
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

/// The melee emitter split — `0x6243e0`'s branch order, byte-verified (closing
/// the phase-2 INFERRED flag): a **word state** (victim states 2 dodge · 3 parry · 5 block ·
/// 6 evade · 7 immune · 8 deflect) floats its word UNCONDITIONALLY, Damage ignored; otherwise
/// (states 0/1/4) landed damage floats the bare post-mitigation number (category 0, or 2 on
/// `HITINFO_CRITICALHIT 0x80`) — a partial block or absorb is never annotated — and zero damage
/// falls to `hit_info & 0x20` → "Absorb", `& 0x40` → "Resist", else the "Miss" word (the fn
/// never tests `HITINFO_MISS`; a vs-0 miss reaches the word through this default).
pub(crate) fn melee_text(hit_info: u32, victim_state: u32, damage: u32) -> Option<(u8, String)> {
    let code = match victim_state {
        2 => 3, // DODGE
        3 => 4, // PARRY
        5 => 5, // BLOCK
        6 => 6, // EVADE
        7 => 7, // IMMUNE
        8 => 9, // DEFLECT
        _ => {
            // 0 UNAFFECTED / 1 NORMAL / 4 INTERRUPT (the silent NORMAL alias): Damage-keyed.
            if damage > 0 {
                let category = if hit_info & 0x80 != 0 { 2 } else { 0 };
                return Some((category, damage.to_string()));
            }
            if hit_info & 0x20 != 0 {
                10 // ABSORB (full — a partial rides the number above)
            } else if hit_info & 0x40 != 0 {
                2 // RESIST (full)
            } else {
                1 // MISS — the zero-damage default word
            }
        }
    };
    miss_word(code).map(|(w, c)| (c, w.to_string()))
}

/// The spell/periodic damage emitter split (`0x5e85e0`/`0x626dd0`): landed damage floats as a
/// number (category 0, or 2 on `SPELL_HIT_TYPE_CRIT 0x2` — periodic ticks never crit in 1.12, the
/// caller passes `crit: false`); zero damage floats ABSORB or RESIST (the word choice at
/// `0x5e88d1` is INFERRED from the packet's fields — flagged in the decision record).
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

/// The gx px round — `ScreenToPixelWidth 0x5c7010` / `ScreenToPixelHeight 0x5c6fa0` verbatim
/// (`fild; fmul; +0.5; __ftol`): `+0.5` then truncate (half away from zero).
fn round_px(t: f64) -> f32 {
    let r = if t > 0.0 { t + 0.5 } else { t - 0.5 };
    r.trunc() as f32
}

/// The composed size law (module doc): category value `v` → on-screen pixel height. One gx unit
/// is the **screen diagonal** `√(W²+H²)`: the screencoord device space spans `[0,G44]×[0,G48]`
/// with `G44 = s/√(s²+1)`, `G48 = 1/√(s²+1)` (`s = W/H` — the live globals `0x832a44/48`), so
/// `v/G48 × H = v·√(W²+H²)`. The first reading hardcoded G48's **4:3 value** (0.6), which
/// under-sizes ~22% at 16:9 — the director's "damage numbers should be 1–2 sizes bigger", one
/// root cause with the small nameplates. The round is the gx px law ([`round_px`]); constant
/// with unit distance.
pub(super) fn text_px(v: f32, viewport: Vec2) -> f32 {
    round_px(f64::from(v) * f64::from(viewport.x).hypot(f64::from(viewport.y)))
}

/// The shadow's offset — the static at `0xce8804` (`{0.002, 0.002}`, init `0x6c7c20`):
/// a **viewport fraction**, resolved per-axis and integer-rounded at draw (module doc; `0x5c8710`,
/// not our first `× diagonal` reading: the
/// `√(W²+H²)` lives only in the unrelated Lua `GetScreenWidth/Height` path).
const SHADOW_OFFSET_FRAC: f32 = 0.002;

/// The rendered shadow offset: `{round(0.002·W), round(0.002·H)}` px, down-right — anisotropic
/// (at 1920×1080: `{4, 2}` — the isotropic diagonal read overstated the vertical ~2.2×, the
/// director's "offset reads too large").
pub(super) fn shadow_offset_px(viewport: Vec2) -> Vec2 {
    Vec2::new(
        round_px(f64::from(SHADOW_OFFSET_FRAC) * f64::from(viewport.x)),
        round_px(f64::from(SHADOW_OFFSET_FRAC) * f64::from(viewport.y)),
    )
}

/// The anti-overlap CLAIM box (full width × height, px) — the ref's measured block under its
/// own units quirk: `0x6c81a0`
/// stores the halves as SCREEN FRACTIONS — height = the raw size value ÷ G48 verbatim (single
/// line: no line gap, and the shadow-Y term is a dead store), width = the advance
/// sum ÷ screen width plus a `round(0.002·diag)` pen seed — and `0x6c7cc0` then spends those
/// fractions as DDC lengths in the solver rect. A fraction in a DDC slot inflates by diag/dim
/// (1/G48 ≈ 1.667× tall, 1/G44 ≈ 1.25× wide at 4:3): the reference's generous,
/// size-proportional between-number padding, ported as the same multiplicative factors. `ink_w`
/// (our laid-out ink) stands in for the ref's advance sum (side bearings + its 0/2/4 raster
/// pad stay INFERRED there, ≤ ~3 px).
pub(super) fn claimed_box_px(ink_w: f32, size_value: f32, viewport: Vec2) -> Vec2 {
    let diag = viewport.length();
    Vec2::new(
        (ink_w + (SHADOW_OFFSET_FRAC * diag).round()) * diag / viewport.x,
        size_value * diag * diag / viewport.y,
    )
}

/// The text and RENDERED shadow alpha bytes at `elapsed_ms` — the alpha fade `0x6c82e0`'s two
/// lanes (constants 255.0 / 127.0) composed through the store seam (module doc, the
/// STORE law): the shadow's stored alpha is **`min(shadow lane, text alpha)`** (`SetShadowColor
/// 0x5cd650`, font node — SetColor runs first each tick, so `mainA` is the fresh text byte).
/// Branch order is the client's: fade-in first (below fade-in-end the ramp arm wins even when
/// fade-out-start is earlier — row 1), then the fade-out arm, else the unconditional
/// `(0xFF, 0x7F)` plateau. The ramps divide by the row's DURATION (the byte-verified quirk: the
/// fade-in boundary is a step, not a ramp arrival). In fade-out the raw shadow lane inverts into
/// `[128, 255]` — but the min-cap pins the rendered value to the text alpha there, so the shadow
/// steps up to ~0xFF as the fade begins and then tracks the text down to 0.
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
    // The store seam: `0x5cd650` writes shadow alpha = min(shadowA, mainA) — every tick, after
    // SetColor. The raw lane above is what the fade computes; this is what renders.
    (text, shadow.min(text))
}

/// The category's scale value at normalized life `t` (`0x6c80b0`): category 2 runs
/// the crit-pop keyframes × `valueHi`; every other row is the affine `lo + (hi − lo)·t` (constant,
/// since `lo == hi` outside the crit row). Floor 0.001, as the client clamps.
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

/// ARGB (client-packed, `0x4a2c10`) → straight-alpha client-space sRGB RGBA, the
/// [`UiQuads`](crate::ui_pass::UiQuads) color convention. The packed ALPHA byte is discarded (forced 1.0): the per-tick fade REPLACES
/// it before anything renders (module doc, the ALPHA + SHADOW law) — keeping row 4's `0x80` here
/// was exactly the half-visible XP text the director reported.
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

    /// The composed size law (`v · diagonal` → `ScreenToPixelHeight`): the exact pixel heights
    /// at the 1024×768 reference window (diag 1280 — where the old `/0.6 × H` law coincides),
    /// the round-half-away behavior, and the ASPECT correction (the whole point: a 16:9 window
    /// sizes by ITS diagonal through the live G48 `0x832a48`, not the 4:3 constant).
    /// The claimed-box anchors (`0x6c81a0`) at
    /// 1024×768: the box the solver sees runs 1/G48 taller than the glyph render — 39.1 px for
    /// the 23 px steady number, 58.7 px for the 35 px crit settle — and 1/G44 (1.25×) wider
    /// than ink + the `round(0.002·diag)` pen seed.
    #[test]
    fn claimed_box_matches_the_measured_block_anchors() {
        let ref43 = Vec2::new(1024.0, 768.0); // diag = 1280, G44 = 0.8, G48 = 0.6
        let steady = claimed_box_px(10.0, VALUE_NORMAL, ref43);
        assert!((steady.y - 39.1).abs() < 0.05, "steady box {}", steady.y);
        // ink 10 + seed round(2.56)=3 → 13 · 1.25.
        assert!((steady.x - 16.25).abs() < 1e-3, "steady box {}", steady.x);
        let crit = claimed_box_px(10.0, VALUE_CRIT, ref43);
        assert!((crit.y - 58.7).abs() < 0.05, "crit box {}", crit.y);
    }

    #[test]
    fn text_px_matches_the_screen_to_pixel_law() {
        let ref43 = Vec2::new(1024.0, 768.0); // diag = 1280
                                              // Normal number: 0.018333 × 1280 = 23.47 → +0.5 trunc → 23.
        assert_eq!(text_px(VALUE_NORMAL, ref43), 23.0);
        // Crit settled (1.0 × valueHi): 0.0275 × 1280 = 35.2 → 35.
        assert_eq!(text_px(VALUE_CRIT, ref43), 35.0);
        // Crit pop peak (2.0 × valueHi): 70.4 → 70.
        assert_eq!(text_px(2.0 * VALUE_CRIT, ref43), 70.0);
        // The +0.5-then-truncate round: t = 23.5 exactly → 24 (half away from zero, not floor).
        assert_eq!(text_px(23.5, Vec2::new(0.6, 0.8)), 24.0);
        // The aspect correction: at 1920×1080 (diag ≈ 2202.9) a normal number is 40 px — the
        // 4:3-hardcoded law gave 33, the ~22% the director read as "1–2 sizes smaller".
        assert_eq!(text_px(VALUE_NORMAL, Vec2::new(1920.0, 1080.0)), 40.0);
        // The size is exactly the scale_value composition the render loop feeds it.
        assert_eq!(text_px(scale_value(0, 0.5), ref43), 23.0);
    }

    /// The shadow offset law (`0x5c8710`): a per-axis viewport
    /// fraction, gx-rounded — NOT the isotropic diagonal (which overstated the vertical ~2.2×
    /// at 16:9 and grew with resolution).
    #[test]
    fn shadow_offset_is_a_per_axis_viewport_fraction() {
        // 1920×1080 → {round(3.84), round(2.16)} = {4, 2}.
        assert_eq!(
            shadow_offset_px(Vec2::new(1920.0, 1080.0)),
            Vec2::new(4.0, 2.0)
        );
        // The 4:3 reference window: {round(2.048), round(1.536)} = {2, 2} — the "~2.6 px"
        // diagonal read never rendered.
        assert_eq!(
            shadow_offset_px(Vec2::new(1024.0, 768.0)),
            Vec2::new(2.0, 2.0)
        );
    }

    /// The config rows against the byte-verified table (bit patterns for the two scale values).
    #[test]
    fn config_rows_match_the_byte_table() {
        assert_eq!(VALUE_NORMAL.to_bits(), 0x3c96_2fc9);
        assert_eq!(VALUE_CRIT.to_bits(), 0x3ce1_47ad);
        assert_eq!(CATEGORIES[0].dur_ms, 1500.0);
        assert_eq!(CATEGORIES[1].fade_out_ms, 90.0); // the ABSORB flicker row, as shipped
        assert_eq!(CATEGORIES[4].color, 0x8094_008B);
        assert_eq!(CATEGORIES[5].color, 0xFFE0_CA0A);
    }

    /// The alpha fade `0x6c82e0` at the pinned points of row 0 (in 150 / out 760 / dur 1500),
    /// composed through the store seam (rendered values): fade-in ramps 255·t / 127·t over the
    /// DURATION (so the fade-in boundary is a step to 255, the pop-in), the
    /// plateau is the unconditional `(0xFF, 0x7F)`, the fade-out drops the text
    /// `255 − clamp(255·u)` — and the SHADOW's raw `[128, 255]` inversion is min-capped to the
    /// text alpha by the `0x5cd650` store, so it steps to ~0xFF at fade-out start and then
    /// fades in sync to 0 (never the 128-floor black ghost).
    #[test]
    fn fade_law_ramps_holds_and_mirrors() {
        let cat = &CATEGORIES[0];
        assert_eq!(fade_alpha(cat, 0.0), (0, 0));
        // Fade-in at 75 ms: 255 × 0.05 = 12.75 → trunc 12; shadow 127 × 0.05 = 6.35 → 6.
        assert_eq!(fade_alpha(cat, 75.0), (12, 6));
        // 149 ms is still the ramp; 150 ms steps onto the plateau — the pop-in.
        assert_eq!(fade_alpha(cat, 149.0), (25, 12));
        assert_eq!(fade_alpha(cat, 150.0), (0xff, 0x7f));
        assert_eq!(fade_alpha(cat, 759.0), (0xff, 0x7f));
        // Fade-out start (u = 0): both lanes 255 — the shadow's byte-real step up from 0x7f.
        assert_eq!(fade_alpha(cat, 760.0), (0xff, 0xff));
        // Fade-out midpoint u = 0.5: text 255 − 127.5 → 127; raw shadow lane 191, min-capped
        // to the text's 127 — the shadow fades WITH the text, not above it.
        assert_eq!(fade_alpha(cat, 1130.0), (127, 127));
        assert_eq!(fade_alpha(cat, 1500.0), (0, 0));
        // Row 1's fade_out(90) < fade_in(150): the fade-in branch is tested FIRST — this row
        // has NO plateau; at 150 ms the fade-out arm is already 60/1410 deep.
        let absorb = &CATEGORIES[1];
        assert_eq!(fade_alpha(absorb, 100.0), (17, 8)); // 255·t / 127·t at t = 1/15
        assert_eq!(fade_alpha(absorb, 150.0), (244, 244)); // text 255−10.85; shadow min(249, text)
                                                           // Row 4 (XP): the plateau is FULLY OPAQUE — the packed 0x80 never renders (the
                                                           // director's "less visible than ref", root-caused).
        assert_eq!(fade_alpha(&CATEGORIES[4], 1000.0), (0xff, 0x7f));
        // Row 4's fade tail (u = 0.8, elapsed 4000): text 51, shadow min-capped to 51 — the
        // 4.5 s XP text's shadow dies with the text (the reported lingering ghost).
        assert_eq!(fade_alpha(&CATEGORIES[4], 4000.0), (51, 51));
    }

    /// The crit pop (`0x8112dc` × valueHi): 0.1→2.0 over the first 10% of life, settle 1.0 by 20%.
    #[test]
    fn crit_keyframes_pop_then_settle() {
        assert!((scale_value(2, 0.0) - 0.1 * VALUE_CRIT).abs() < 1e-7);
        assert!((scale_value(2, 0.05) - 1.05 * VALUE_CRIT).abs() < 1e-6);
        assert!((scale_value(2, 0.1) - 2.0 * VALUE_CRIT).abs() < 1e-6);
        assert!((scale_value(2, 0.15) - 1.5 * VALUE_CRIT).abs() < 1e-6);
        assert!((scale_value(2, 0.5) - VALUE_CRIT).abs() < 1e-7);
        // Non-crit rows are constant (lo == hi).
        assert_eq!(scale_value(0, 0.0), scale_value(0, 0.9));
        // Settled crit = 1.5× a normal number — the verified ratio.
        assert!((scale_value(2, 0.5) / scale_value(0, 0.5) - 1.5).abs() < 1e-3);
    }

    /// The emitter splits: melee number/word (`0x6243e0`'s byte-verified branch order, decision
    /// 0279) and the spell damage/absorb/resist fallbacks; ABSORB is the one category-1 word.
    #[test]
    fn emitters_split_numbers_and_words() {
        assert_eq!(melee_text(0x2, 1, 37), Some((0, "37".into())));
        assert_eq!(melee_text(0x82, 1, 99), Some((2, "99".into()))); // crit bit 0x80
        assert_eq!(melee_text(0x10, 0, 0), Some((3, "Miss".into())));
        assert_eq!(melee_text(0x0, 2, 0), Some((3, "Dodge".into())));
        assert_eq!(melee_text(0x0, 3, 0), Some((3, "Parry".into())));
        assert_eq!(melee_text(0x0, 5, 0), Some((3, "Block".into())));
        assert_eq!(melee_text(0x0, 6, 0), Some((3, "Evade".into())));
        // A word state ignores Damage entirely (the client's unconditional word arm).
        assert_eq!(melee_text(0x2, 3, 25), Some((3, "Parry".into())));
        assert_eq!(melee_text(0x22, 1, 0), Some((1, "Absorb".into()))); // full absorb: bit 0x20
        assert_eq!(melee_text(0x42, 1, 0), Some((3, "Resist".into()))); // full resist: bit 0x40
                                                                        // Zero damage, no absorb/resist bit: the default word is Miss (0x10 is never tested).
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

    /// The `0x6128b0` B/K color branch (the byte table): self melee → NULL override (row-default
    /// white), self spell → gold, pet melee → orange (PetMeleeDamage), pet spell → gold
    /// (PetSpellDamage). Crit never enters the pick (it only selects the pop row).
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
        // The byte values themselves (`0x5fa0b0`/`0x5fa0f0` hard-inits).
        assert_eq!(COLOR_SPELL_GOLD, 0xFFFF_DE00);
        assert_eq!(COLOR_PET_MELEE_ORANGE, 0xFFFF_8400);
    }

    /// **The three gates SUPPRESS; they never recolour** — both of the reference's read sites
    /// branch to a function epilogue, so a failed gate means no emit at all. And the master's
    /// reach is total: `CombatDamage = 0` takes the self sub-case with it, which is the one case
    /// no `Pet*` row can reach.
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

        // A pet sub-gate takes ONLY its own branch — and leaves the player's alone, because the
        // self sub-case is unconditional in the reference.
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
