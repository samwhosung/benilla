//! **The realm list's two computed columns** — the load band and the realm type — and the row
//! colours that go with them.
//!
//! The load word a player reads (`Low` / `Medium` / `High` / `Full` / `New` / `Recommended`) is
//! *not* the population the auth server sent. It is a **band**: the client takes the populations of
//! **every realm it knows** — all categories, not the selected tab — computes their mean and a
//! scaled standard deviation, and places each realm's population in that distribution. So the same realm reads `High` on a quiet
//! list and `Low` on a busy one. Anything that printed the raw float would be showing a different
//! word from the real client on every list.
//!
//! **A one-realm list is not a special case that reads `Medium`** — a tempting thing to assume, and
//! wrong. The reference short-circuits `n <= 1` to `(mean = 1.0, stddev = 0.0)`, which is a real
//! distribution of zero width, so the single realm is banded against the literal 1.0: below it
//! reads `Low`, above it reads `High`, and only an exact 1.0 reads `Medium`. Our own vmangos realm
//! advertises 0.06 and therefore reads **Low**, which is what the real client would show too.
//!
//! [`realm_load_stats`] and [`realm_load_classify`] are ports of `CGlue::RealmLoadStats`
//! `0x46e510` and `CGlue::RealmLoadClassify` `0x46ec60`, including their x87 rounding
//! behaviour — the accumulators round to `f32` every
//! iteration because the binary stores them to `f32` slots inside the loop, while the mean, the
//! sqrt and the band comparisons stay 53-bit. Kept faithful rather than tidied because the band
//! edges are exactly where a rounding difference would flip a visible word.
//!
//! The band → string → colour mapping is the reference `GlueXML/RealmList.lua`'s
//! `RealmListUpdate`, read literally.

use bevy::prelude::*;

// ── The six glue font colours (`GlueXML/GlueFonts.xml` l.4-9) ──────────────────────────────────
//
// Glue has its own copies of the FrameXML globals; `BLUE_FONT_COLOR` exists ONLY here (FrameXML's
// `Fonts.xml` ships five, not six) and the realm list is its one consumer in the whole glue tree.

/// `NORMAL_FONT_COLOR` — `{1.0, 0.82, 0}`.
pub(super) const NORMAL: Color = Color::srgb(1.0, 0.82, 0.0);
/// `HIGHLIGHT_FONT_COLOR` — `{1.0, 1.0, 1.0}`.
pub(super) const HIGHLIGHT: Color = Color::srgb(1.0, 1.0, 1.0);
/// `GRAY_FONT_COLOR` — `{0.5, 0.5, 0.5}`.
pub(super) const GRAY: Color = Color::srgb(0.5, 0.5, 0.5);
/// `GREEN_FONT_COLOR` — `{0.1, 1.0, 0.1}`.
pub(super) const GREEN: Color = Color::srgb(0.1, 1.0, 0.1);
/// `RED_FONT_COLOR` — `{1.0, 0.1, 0.1}`.
pub(super) const RED: Color = Color::srgb(1.0, 0.1, 0.1);
/// `BLUE_FONT_COLOR` — `{0, 0.749, 0.953}`.
pub(super) const BLUE: Color = Color::srgb(0.0, 0.749, 0.953);

/// `[0x8038d8]` = 0.6, the std-dev scale (`realm_load_stats`).
const STDDEV_MULT: f32 = f32::from_bits(0x3f19_999a);

/// `realm_load_stats` (`CGlue::RealmLoadStats`, `0x46e510`): over the `populations` of **every**
/// realm, compute `(mean, scaled_stddev)` where `mean = Σpop / n` and `scaled_stddev =
/// sqrt(Σ(pop − mean)² / (n − 1)) · 0.6`.
///
/// The caller passes the flat list deliberately — see [`super::Realms::stats`] for why this is not
/// the per-category statistic it looks like.
///
/// The sum and the variance accumulator are each rounded to `f32` every iteration (the binary
/// stores them to `f32` slots in the loop); the mean and std-dev are 53-bit through the
/// divide/sqrt/scale and stored as `f32`. With `n ≤ 1` the binary short-circuits to
/// `(mean = 1.0, stddev = 0.0)`: a zero-width distribution centred on 1.0, not a "no opinion".
pub(super) fn realm_load_stats(populations: &[f32]) -> (f32, f32) {
    let n = populations.len();
    if n <= 1 {
        return (1.0, 0.0);
    }
    // Σ population — accumulated and stored as f32 each step (`fld sum; fadd pop; fstp sum`).
    let mut sum: f32 = 0.0;
    for &pop in populations {
        sum = (f64::from(sum) + f64::from(pop)) as f32;
    }
    // mean = sum / n (`fild n; fdivr sum; fstp mean`).
    let mean = (f64::from(sum) / n as f64) as f32;
    // Σ(pop − mean)² — accumulated and stored as f32 each step.
    let mut acc: f32 = 0.0;
    for &pop in populations {
        let d = f64::from(pop) - f64::from(mean);
        acc = (d * d + f64::from(acc)) as f32;
    }
    // stddev = sqrt(acc / (n − 1)) · 0.6 (`fild n-1; fdivr acc; fsqrt; fmul 0.6; fstp stddev`).
    let stddev = ((f64::from(acc) / (n - 1) as f64).sqrt() * f64::from(STDDEV_MULT)) as f32;
    (mean, stddev)
}

/// `realm_load_classify` (`CGlue::RealmLoadClassify`, `0x46ec60`): map one realm's `population` to
/// the load indicator `GetRealmInfo` returns, given the `mean`/`stddev` from [`realm_load_stats`]
/// and the realm's flags byte.
///
/// Note the three sentinel bits are **not** wire flags: `benilla_protocol`'s parser synthesizes
/// them from the magic populations the server sends instead, exactly as the reference's parser
/// does, so by the time a realm reaches here the bit is set and the population is the rewritten
/// one. See `auth::MAGIC_POPULATIONS`.
///
/// The three flag sentinels take priority, tested in this order, then the mean±stddev band. The
/// band comparisons are 53-bit and **strict** — a population exactly on a band edge reads as
/// normal, matching the x87 `fcomp` flag tests.
pub(super) fn realm_load_classify(flags: u8, population: f32, mean: f32, stddev: f32) -> f32 {
    if flags & 0x20 != 0 {
        return -3.0;
    }
    if flags & 0x40 != 0 {
        return -2.0;
    }
    if flags & 0x80 != 0 {
        return 2.0;
    }
    if (f64::from(mean) - f64::from(stddev)) > f64::from(population) {
        return -1.0;
    }
    if (f64::from(mean) + f64::from(stddev)) < f64::from(population) {
        return 1.0;
    }
    0.0
}

/// The **Population** column: the authored key and colour for one realm's load indicator.
///
/// `RealmListUpdate` reads the classifier's float back as a ladder of exact comparisons against the
/// sentinels, then signs — so the arms are the reference's, in the reference's order. `down`
/// overrides every band: an offline realm reads `Offline` in grey whatever its population says.
pub(super) fn load_column(down: bool, load: f32) -> (&'static str, Color) {
    if down {
        return ("REALM_DOWN", GRAY);
    }
    // The three sentinels are compared for exact equality in the Lua (`load == -3.0`), which is
    // sound because they are the classifier's own literals travelling unmodified.
    if load == -3.0 {
        ("LOAD_RECOMMENDED", BLUE)
    } else if load == -2.0 {
        ("LOAD_NEW", GREEN)
    } else if load == 2.0 {
        ("LOAD_FULL", RED)
    } else if load > 0.0 {
        ("LOAD_HIGH", RED)
    } else if load < 0.0 {
        ("LOAD_LOW", GREEN)
    } else {
        ("LOAD_MEDIUM", NORMAL)
    }
}

/// The **Type** column: the authored key and colour for one realm's game type.
///
/// `pvp` and `rp` are two independent booleans in the reference's `GetRealmInfo`, and the Lua tests
/// them in the order `pvp and rp` → `rp` → `pvp` → neither, so `RPPVP` is not "the RP colour" — it
/// takes `NORMAL`, the same as a normal realm. Read literally rather than collapsed.
pub(super) fn type_column(realm_type: u32) -> (&'static str, Color) {
    let (pvp, rp) = pvp_rp(realm_type);
    match (pvp, rp) {
        (true, true) => ("RPPVP_PARENTHESES", NORMAL),
        (false, true) => ("RP_PARENTHESES", GREEN),
        (true, false) => ("PVP_PARENTHESES", RED),
        (false, false) => ("GAMETYPE_NORMAL", NORMAL),
    }
}

/// The wire's realm **type** split into the reference's two booleans — **the one place** the
/// mapping lives. `GetRealmInfo` returns `pvp`/`rp` for the realm list and `GetServerName` returns
/// the same pair for the character screen's banner, so a second copy of this table is a second
/// thing to get wrong.
///
/// **This is a `Cfg_Configs.dbc` join, not an enumeration** — `0x46efda` takes the realm's type
/// dword and linearly scans the DBC for the row whose `RealmType` column matches, reading
/// `PlayerKillingAllowed` → `pvp` and `RoleplayingRealm` → `rp`. (Note the key is the type at
/// `[realm+0x04]`, the *first* dword on the wire; `[+0x130]` is `numCharacters`, not the type.)
///
/// The table below is the shipped `DBFilesClient\Cfg_Configs.dbc` transcribed — a frozen table in
/// the same spirit as the `*_ICON_TCOORDS` in `crate::glue::art`, and the honest way to read it is
/// as *data*, not as a rule. It is also why the natural guess is wrong: types **3 and 5 are PvP**
/// and **7 is RP**, which an "0 normal / 1 pvp / 6 rp / 8 rppvp" reading silently gets wrong for
/// any server that uses them.
///
/// A type with no row falls to `(false, false)` — the scan finds nothing and neither column is
/// read. That is also what the two rows this table does not carry (the shipped DBC has 11; nine
/// map a distinct `RealmType`) degrade to, and it is the safe answer for a server inventing one.
pub(crate) fn pvp_rp(realm_type: u32) -> (bool, bool) {
    match realm_type {
        // RealmType → (PlayerKillingAllowed, RoleplayingRealm)
        0 | 2 | 4 => (false, false),
        1 | 3 | 5 => (true, false),
        6 | 7 => (false, true),
        8 => (true, true),
        _ => (false, false),
    }
}

/// The realm **name**'s colour, and the colour it takes under the cursor
/// (`SetTextColor`/`SetHighlightTextColor` in `RealmListUpdate`).
///
/// The ladder is the reference's: offline first, then invalid, then "you have characters here",
/// then the default gold. Note the reference's own gold here is `1.0, 0.78, 0` — a *different*
/// value from `NORMAL_FONT_COLOR`'s `1.0, 0.82, 0`, hardcoded at the four call sites rather than
/// taken from the global. Kept as it is written.
/// The row gold `RealmListUpdate` writes — **a literal in the Lua** (`SetTextColor(1.0, 0.78,
/// 0.0)`), not `NORMAL_FONT_COLOR`, which is 0.82. Four hundredths, and the reference is explicit
/// about it in both places it appears (the name and the selection band's vertex colour), so it is
/// pinned rather than folded into [`NORMAL`].
pub(super) const ROW_GOLD: Color = Color::srgb(1.0, 0.78, 0.0);

pub(super) fn name_colors(down: bool, invalid: bool, characters: u8) -> (Color, Color) {
    if down {
        (GRAY, Color::srgb(0.8, 0.8, 0.8))
    } else if invalid {
        (RED, Color::srgb(1.0, 0.5, 0.5))
    } else if characters > 0 {
        (GREEN, HIGHLIGHT)
    } else {
        (ROW_GOLD, HIGHLIGHT)
    }
}

/// The colour the selection band behind the chosen row takes
/// (`RealmListHighlightTexture:SetVertexColor`) — the name colour's ladder minus the offline arm,
/// because an offline row cannot be the chosen one (`RealmListHighlight:Hide()`).
pub(super) fn highlight_color(invalid: bool, characters: u8) -> Color {
    if invalid {
        RED
    } else if characters > 0 {
        GREEN
    } else {
        ROW_GOLD
    }
}

/// The player count beside the realm name — `"(3)"`, or nothing at all when they have none there.
pub(super) fn players_text(characters: u8) -> String {
    if characters > 0 {
        format!("({characters})")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's `n <= 1` short circuit bands the single realm against the literal 1.0
    /// rather than against itself, so `Medium` is the knife edge and not the default — the
    /// assumption this pins against. The 0.06 case is our own vmangos realm, read off the live
    /// realm list: it reads **Low**.
    #[test]
    fn a_single_realm_is_banded_against_the_literal_one_not_against_itself() {
        assert_eq!(realm_load_stats(&[]), (1.0, 0.0));
        assert_eq!(realm_load_stats(&[400.0]), (1.0, 0.0));
        let word = |pop: f32| {
            let (mean, stddev) = realm_load_stats(&[pop]);
            load_column(false, realm_load_classify(0, pop, mean, stddev)).0
        };
        assert_eq!(word(0.06), "LOAD_LOW", "the live vmangos realm");
        assert_eq!(word(1.0), "LOAD_MEDIUM", "the knife edge");
        assert_eq!(word(400.0), "LOAD_HIGH");
    }

    /// The band is relative, which is the whole point of computing it: the SAME population reads
    /// `High` against quiet neighbours and `Low` against busy ones.
    #[test]
    fn the_same_population_reads_differently_against_different_neighbours() {
        let quiet = [1.0f32, 1.0, 1.0, 9.0];
        let (mean, stddev) = realm_load_stats(&quiet);
        assert_eq!(
            load_column(false, realm_load_classify(0, 9.0, mean, stddev)).0,
            "LOAD_HIGH"
        );

        let busy = [90.0f32, 90.0, 90.0, 9.0];
        let (mean, stddev) = realm_load_stats(&busy);
        assert_eq!(
            load_column(false, realm_load_classify(0, 9.0, mean, stddev)).0,
            "LOAD_LOW"
        );
    }

    /// `mean = Σpop/n`, and `stddev = sqrt(Σ(pop−mean)²/(n−1)) · 0.6` — the sample std-dev, scaled.
    /// Pinned against a hand-computed case so a "simplification" of the accumulator loop shows up.
    #[test]
    fn the_stats_are_the_sample_stddev_scaled_by_six_tenths() {
        let (mean, stddev) = realm_load_stats(&[2.0, 4.0, 4.0, 6.0]);
        assert_eq!(mean, 4.0);
        // Σ(pop−mean)² = 4+0+0+4 = 8; 8/3 = 2.666…; sqrt = 1.63299…; ×0.6 = 0.97979…
        assert!((stddev - 0.979_795_9).abs() < 1e-6, "stddev was {stddev}");
    }

    /// The three flag sentinels beat the band, and they are tested in the reference's order — a
    /// realm flagged both recommended (0x20) and full (0x80) reads `Recommended`.
    #[test]
    fn the_flag_sentinels_outrank_the_band_and_each_other_in_order() {
        assert_eq!(realm_load_classify(0x20, 999.0, 1.0, 0.5), -3.0);
        assert_eq!(realm_load_classify(0x40, 999.0, 1.0, 0.5), -2.0);
        assert_eq!(realm_load_classify(0x80, 0.0, 1.0, 0.5), 2.0);
        assert_eq!(realm_load_classify(0x20 | 0x80, 0.0, 1.0, 0.5), -3.0);
        assert_eq!(load_column(false, -3.0).0, "LOAD_RECOMMENDED");
        assert_eq!(load_column(false, -2.0).0, "LOAD_NEW");
        assert_eq!(load_column(false, 2.0).0, "LOAD_FULL");
    }

    /// An exactly-on-edge population reads as normal: the binary's comparisons are strict.
    #[test]
    fn a_population_exactly_on_a_band_edge_is_normal() {
        assert_eq!(realm_load_classify(0, 0.5, 1.0, 0.5), 0.0);
        assert_eq!(realm_load_classify(0, 1.5, 1.0, 0.5), 0.0);
        assert_eq!(realm_load_classify(0, 0.4, 1.0, 0.5), -1.0);
        assert_eq!(realm_load_classify(0, 1.6, 1.0, 0.5), 1.0);
    }

    /// Offline beats every band, including the sentinels — the Lua tests `realmDown` first.
    #[test]
    fn an_offline_realm_reads_offline_whatever_its_population_says() {
        assert_eq!(load_column(true, -3.0), ("REALM_DOWN", GRAY));
        assert_eq!(load_column(true, 2.0), ("REALM_DOWN", GRAY));
    }

    /// The four wire types, and the colour each takes. `RPPVP` deliberately takes `NORMAL` — the
    /// Lua's `pvp and rp` arm sets `NORMAL_FONT_COLOR`, not the RP green.
    #[test]
    fn the_type_column_is_the_shipped_cfg_configs_rows() {
        for t in [0, 2, 4] {
            assert_eq!(type_column(t), ("GAMETYPE_NORMAL", NORMAL), "type {t}");
        }
        // 3 and 5 are the rows a "0/1/6/8" reading gets wrong.
        for t in [1, 3, 5] {
            assert_eq!(type_column(t), ("PVP_PARENTHESES", RED), "type {t}");
        }
        for t in [6, 7] {
            assert_eq!(type_column(t), ("RP_PARENTHESES", GREEN), "type {t}");
        }
        assert_eq!(type_column(8), ("RPPVP_PARENTHESES", NORMAL));
        // No row: the scan reads neither column.
        assert_eq!(type_column(77), ("GAMETYPE_NORMAL", NORMAL));
    }

    /// The name ladder, and the gold that is NOT `NORMAL_FONT_COLOR`.
    #[test]
    fn the_name_ladder_is_offline_then_invalid_then_has_characters() {
        assert_eq!(name_colors(true, true, 5).0, GRAY);
        assert_eq!(name_colors(false, true, 5).0, RED);
        assert_eq!(name_colors(false, false, 5).0, GREEN);
        assert_eq!(name_colors(false, false, 0).0, crate::glue::art::GOLD);
        assert_ne!(crate::glue::art::GOLD, NORMAL);
    }

    /// No characters on a realm means no count beside its name — not `"(0)"`.
    #[test]
    fn a_realm_with_no_characters_shows_no_count() {
        assert_eq!(players_text(0), "");
        assert_eq!(players_text(3), "(3)");
    }
}
