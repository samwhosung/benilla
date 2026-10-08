//! The realm list's two computed columns, the load band and the realm type, and the row colours.
//!
//! The load word is a band, not the population the auth server sent: the reference takes the
//! mean and a scaled standard deviation over every realm it knows (all categories) and places each
//! realm in that distribution. Ports of `CGlue::RealmLoadStats` `0x46e510` and
//! `CGlue::RealmLoadClassify` `0x46ec60`, with their x87 rounding, since the band edges are where a
//! rounding difference flips the word. The band to string to colour mapping is `RealmListUpdate`
//! in `GlueXML/RealmList.lua`.

use bevy::prelude::*;

use benilla_formats::RealmConfigs;

// ── The six glue font colours (`GlueFonts.xml:4-9`) ─────────────────────────────────────────────
// `BLUE_FONT_COLOR` exists only in glue; FrameXML's `Fonts.xml` has the other five.

/// `NORMAL_FONT_COLOR`.
pub(super) const NORMAL: Color = Color::srgb(1.0, 0.82, 0.0);
/// `HIGHLIGHT_FONT_COLOR`.
pub(super) const HIGHLIGHT: Color = Color::srgb(1.0, 1.0, 1.0);
/// `GRAY_FONT_COLOR`.
pub(super) const GRAY: Color = Color::srgb(0.5, 0.5, 0.5);
/// `GREEN_FONT_COLOR`.
pub(super) const GREEN: Color = Color::srgb(0.1, 1.0, 0.1);
/// `RED_FONT_COLOR`.
pub(super) const RED: Color = Color::srgb(1.0, 0.1, 0.1);
/// `BLUE_FONT_COLOR`.
pub(super) const BLUE: Color = Color::srgb(0.0, 0.749, 0.953);

/// The std-dev scale, 0.6 (`[0x8038d8]`).
const STDDEV_MULT: f32 = f32::from_bits(0x3f19_999a);

/// `CGlue::RealmLoadStats` (`0x46e510`): `(mean, sqrt(Σ(pop − mean)² / (n − 1)) · 0.6)` over every
/// realm's population. Both accumulators round to `f32` each iteration; the divide, sqrt and scale
/// run at 53 bits. `n <= 1` gives `(1.0, 0.0)`, so a lone realm is banded against the literal 1.0.
pub(super) fn realm_load_stats(populations: &[f32]) -> (f32, f32) {
    let n = populations.len();
    if n <= 1 {
        return (1.0, 0.0);
    }
    // `fld sum; fadd pop; fstp sum`: stored as f32 each step.
    let mut sum: f32 = 0.0;
    for &pop in populations {
        sum = (f64::from(sum) + f64::from(pop)) as f32;
    }
    // `fild n; fdivr sum; fstp mean`.
    let mean = (f64::from(sum) / n as f64) as f32;
    // Stored as f32 each step.
    let mut acc: f32 = 0.0;
    for &pop in populations {
        let d = f64::from(pop) - f64::from(mean);
        acc = (d * d + f64::from(acc)) as f32;
    }
    // `fild n-1; fdivr acc; fsqrt; fmul 0.6; fstp stddev`.
    let stddev = ((f64::from(acc) / (n - 1) as f64).sqrt() * f64::from(STDDEV_MULT)) as f32;
    (mean, stddev)
}

/// `CGlue::RealmLoadClassify` (`0x46ec60`): the load indicator `GetRealmInfo` returns. The three
/// sentinel bits, synthesized by the parser from magic populations as the reference's does, win in
/// this order; then the band, whose 53-bit comparisons are strict, so an edge reads normal.
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

/// The Population column's key and colour: `RealmListUpdate`'s ladder, offline first, then the
/// sentinels by exact comparison, then the sign.
pub(super) fn load_column(down: bool, load: f32) -> (&'static str, Color) {
    if down {
        return ("REALM_DOWN", GRAY);
    }
    // Exact equality is sound: the sentinels are the classifier's own literals.
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

/// The Type column's key and colour for a realm's `(pvp, rp)`; the Lua tests `pvp and rp`, `rp`,
/// `pvp`, then neither, so RP-PvP takes `NORMAL`, not the RP green.
pub(super) fn type_column((pvp, rp): (bool, bool)) -> (&'static str, Color) {
    match (pvp, rp) {
        (true, true) => ("RPPVP_PARENTHESES", NORMAL),
        (false, true) => ("RP_PARENTHESES", GREEN),
        (true, false) => ("PVP_PARENTHESES", RED),
        (false, false) => ("GAMETYPE_NORMAL", NORMAL),
    }
}

/// The realm type as `(pvp, rp)`, shared by `GetRealmInfo`, `GetServerName` and the world-entry
/// latch: each scans `Cfg_Configs.dbc` for the first row whose `RealmType` is the realm's wire
/// type and reads `PlayerKillingAllowed` and `RoleplayingRealm` (`0x46efda`); a type with no row
/// reads neither.
pub(super) fn pvp_rp(types: &RealmConfigs, realm_type: u32) -> (bool, bool) {
    types
        .get(realm_type)
        .map_or((false, false), |c| (c.pvp, c.rp))
}

/// Read `Cfg_Configs.dbc` once, after the archive chain opens.
pub(super) fn load_realm_types(
    mut realms: ResMut<super::Realms>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else {
        return;
    };
    let mut chain = assets.chain.lock_recover();
    match benilla_formats::load_realm_configs(&mut chain) {
        Ok(types) => {
            info!(
                "realm list: {} Cfg_Configs.dbc realm types",
                types.rows().len()
            );
            realms.types = types;
        }
        Err(e) => warn!(
            "realm list: Cfg_Configs.dbc failed to load, so every realm reads Normal and no realm \
             is PvP: {e:#}"
        ),
    }
}

/// The row gold, a literal in `RealmListUpdate` (`1.0, 0.78, 0`), not `NORMAL_FONT_COLOR`'s 0.82.
pub(super) const ROW_GOLD: Color = Color::srgb(1.0, 0.78, 0.0);

/// The realm name's colour and its highlight colour (`RealmListUpdate`): offline, then invalid,
/// then has characters, then the row gold.
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

/// The selection band's vertex colour: the name ladder without the offline arm, since an offline
/// row hides the highlight.
pub(super) fn highlight_color(invalid: bool, characters: u8) -> Color {
    if invalid {
        RED
    } else if characters > 0 {
        GREEN
    } else {
        ROW_GOLD
    }
}

/// The character count beside the realm name, `"(3)"`, or nothing when there are none.
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

    /// 0.06 is the population a stock vmangos realm advertises.
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

    /// The same population reads `High` against quiet neighbours and `Low` against busy ones.
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

    /// A hand-computed case of the scaled sample std-dev.
    #[test]
    fn the_stats_are_the_sample_stddev_scaled_by_six_tenths() {
        let (mean, stddev) = realm_load_stats(&[2.0, 4.0, 4.0, 6.0]);
        assert_eq!(mean, 4.0);
        // Σ(pop−mean)² = 4+0+0+4 = 8; 8/3 = 2.666…; sqrt = 1.63299…; ×0.6 = 0.97979…
        assert!((stddev - 0.979_795_9).abs() < 1e-6, "stddev was {stddev}");
    }

    /// Recommended (0x20) outranks full (0x80).
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

    /// The reference's band comparisons are strict.
    #[test]
    fn a_population_exactly_on_a_band_edge_is_normal() {
        assert_eq!(realm_load_classify(0, 0.5, 1.0, 0.5), 0.0);
        assert_eq!(realm_load_classify(0, 1.5, 1.0, 0.5), 0.0);
        assert_eq!(realm_load_classify(0, 0.4, 1.0, 0.5), -1.0);
        assert_eq!(realm_load_classify(0, 1.6, 1.0, 0.5), 1.0);
    }

    /// The Lua tests `realmDown` first.
    #[test]
    fn an_offline_realm_reads_offline_whatever_its_population_says() {
        assert_eq!(load_column(true, -3.0), ("REALM_DOWN", GRAY));
        assert_eq!(load_column(true, 2.0), ("REALM_DOWN", GRAY));
    }

    /// The Type column through the install's `Cfg_Configs.dbc`: type 10 is PvP, and RP-PvP takes
    /// `NORMAL`, as the Lua's `pvp and rp` arm sets `NORMAL_FONT_COLOR`.
    #[test]
    fn the_type_column_reads_the_shipped_cfg_configs() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let types = benilla_formats::load_realm_configs(&mut chain).expect("Cfg_Configs.dbc");
        let column = |t| type_column(pvp_rp(&types, t));
        for t in [0, 2, 4, 9] {
            assert_eq!(column(t), ("GAMETYPE_NORMAL", NORMAL), "type {t}");
        }
        for t in [1, 3, 5, 10] {
            assert_eq!(column(t), ("PVP_PARENTHESES", RED), "type {t}");
        }
        for t in [6, 7] {
            assert_eq!(column(t), ("RP_PARENTHESES", GREEN), "type {t}");
        }
        assert_eq!(column(8), ("RPPVP_PARENTHESES", NORMAL));
    }

    /// No row: the scan reads neither column, so the type is Normal, as it is with no table.
    #[test]
    fn a_type_with_no_row_is_normal() {
        let types = RealmConfigs::from_rows(vec![benilla_formats::RealmConfig {
            realm_type: 1,
            pvp: true,
            rp: false,
        }]);
        assert_eq!(pvp_rp(&types, 1), (true, false));
        assert_eq!(pvp_rp(&types, 77), (false, false));
        assert_eq!(pvp_rp(&RealmConfigs::default(), 1), (false, false));
    }

    /// The default gold is not `NORMAL_FONT_COLOR`.
    #[test]
    fn the_name_ladder_is_offline_then_invalid_then_has_characters() {
        assert_eq!(name_colors(true, true, 5).0, GRAY);
        assert_eq!(name_colors(false, true, 5).0, RED);
        assert_eq!(name_colors(false, false, 5).0, GREEN);
        assert_eq!(name_colors(false, false, 0).0, crate::glue::art::GOLD);
        assert_ne!(crate::glue::art::GOLD, NORMAL);
    }

    /// Not `"(0)"`.
    #[test]
    fn a_realm_with_no_characters_shows_no_count() {
        assert_eq!(players_text(0), "");
        assert_eq!(players_text(3), "(3)");
    }
}
