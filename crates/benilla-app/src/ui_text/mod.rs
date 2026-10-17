//! Text rendering for `QuadContent::Text`: an on-demand glyph cache over the client's own TTFs,
//! shaped through `cosmic-text` 0.16 and emitted as [`crate::ui_pass::UiQuad`]s.
//!
//! Glyphs are shaped one character at a time ([`engine::TextEngine::ensure_char`]) and the pen
//! steps by the reference's law: the advance in whole px plus 1 (`(FT_advance>>6)+1.0` at
//! `0x5d1120`), plus 1 more under a THICK outline (`0x5ca2b0`), and the pair's negative kern, the
//! sum rounded up (`ComputeStep`, `0x5ca2d0`). Advances and kerns come off the face's own tables
//! with FreeType's fixed point (`engine::sfnt`), not the shaper's floats. The four client faces
//! have no ligature or contextual substitution, which `engine::differential_tests` asserts. A width
//! is then a pure function of its characters, so one table answers a measure and a draw, even
//! inside a Lua call.
//!
//! The reference loads every glyph unhinted: both its load-flag words (`0x208a`/`0x20c2`, at
//! `0x5d1324`) carry FreeType's `FT_LOAD_NO_HINTING` (`0x2`), as `FT_Load_Glyph` (`0x7cdb40`) reads
//! it. The measure and the cells both read the unhinted outline: the face's own tables as FreeType
//! scales them (`engine::sfnt`), rasterized 1-bit or smooth as the font's flags say.

mod engine;
mod layout;
mod markup;
mod measurer;
mod outline;
mod pack;

pub(crate) use engine::{FontAtlasInit, UiFontAtlas, UiTextPlugin};

use benilla_ui::script::Outline;
use benilla_ui::widget::RegionHandle;
pub(crate) use measurer::{editbox_advances, measure_request, AtlasMeasurer};

pub(crate) use layout::{
    ellipsize_text, ellipsize_to_fit, layout_text_quads, layout_text_quads_links, line_advances,
    line_origin, line_rows, measure_text, measure_wrapped_rows, string_width, unwrapped_height,
    FontSpec, Justify, TextSeat, UI_SEAT_NUDGE,
};

/// A glyph quad's cell in the glyph sheet's texels: the `ImageNode::rect` that draws it from a
/// `bevy_ui` node one to one.
pub(crate) fn glyph_texels(uv: &crate::ui_pass::UvRect) -> bevy::math::Rect {
    let side = pack::SHEET_SIZE as f32;
    let [tl, _, br, _] = uv.corners;
    bevy::math::Rect::new(tl[0] * side, tl[1] * side, br[0] * side, br[1] * side)
}

/// The text size in logical px of a FontString with no font object: `GameFontNormal`'s 12.
const DEFAULT_FONT_SIZE: f32 = 12.0;

/// The one-to-one raster cap. Every `CSimpleFontString` sets the one-to-one bit (`+0x120` bit
/// `0x200`, ctor `0x770d30` default `0x212`), which only `SetTextHeight` clears (`0x771600`), so it
/// draws at its raster size (getter `0x7727b0(1)`), which is
/// `min(32, max(2, round((height/768)·deviceH)))` (`0x5ca030` → `[CGxFont+0x24c]`): a FontHeight
/// of 32 or more draws 32 px, and nothing in the text pipeline scales to fit.
///
/// Deviation: the cap is 32 logical units, not 32 device px, because the reference's raster-memory
/// ceiling draws all big text at the same 32 device px on a modern display; at the 768-tall design
/// height the two agree. UI FontStrings only: world text ([`crate::combat_text`], the nameplates)
/// renders crisp above it, where the reference stretched a 32 px raster.
pub(crate) const FONTSTRING_EM_CAP: f32 = 32.0;

/// Apply [`FONTSTRING_EM_CAP`] to a UI FontString's requested height. Render and measure both cap,
/// so `GetStringWidth` reports the width at the capped size, as the reference does (`0x772890`).
pub(crate) fn fontstring_em(height: Option<f32>) -> Option<f32> {
    height.map(|h| h.min(FONTSTRING_EM_CAP))
}

/// The drawn pixel height of a UI FontString at `s`, the seam scale `windowH/768`
/// (`crate::ui_script::seam_scale`) times the owner's effective scale, the UI scale included,
/// under the reference's two size regimes (getter `0x7727b0`): one-to-one, the default, draws the
/// raster size ([`FONTSTRING_EM_CAP`] applied in units), and a `SetTextHeight` size
/// (`text_height`) draws uncapped, the reference magnifying the raster to it (`0x771600` clears bit
/// `0x200`; the drawn em is `round(size/768·deviceH)`).
pub(crate) fn drawn_px(font_height: Option<f32>, text_height: Option<f32>, s: f32) -> Option<f32> {
    match text_height {
        Some(t) => Some(t * s),
        None => Some(fontstring_em(font_height).unwrap_or(DEFAULT_FONT_SIZE) * s),
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn one_to_one_cap_matches_the_byte_law() {
        // `ZoneTextFont` and `WorldMapTextFont` are 102 in the stock `Fonts.xml`.
        assert_eq!(fontstring_em(Some(102.0)), Some(32.0));
        assert_eq!(fontstring_em(Some(33.0)), Some(32.0));
        // At-cap and under-cap sizes pass through untouched (SubZoneTextFont 26, chat 14).
        assert_eq!(fontstring_em(Some(32.0)), Some(32.0));
        assert_eq!(fontstring_em(Some(26.0)), Some(26.0));
        assert_eq!(fontstring_em(Some(14.0)), Some(14.0));
        assert_eq!(fontstring_em(None), None);
    }
}
// Line pitch has no line-height factor: the reference's `LayoutLines` (`0x5cdc20`) steps
// `px(size) + px(spacing)`, the spacing defaulting to 0. Cosmic buffers take `size` as their line
// height too, unused: every run shapes single-line (`Wrap::None`).

/// The FontString display string, the ellipsis seam's answer, remembered per region. The reference
/// builds it once (`0x771ec0`, into `CGxString+0xf8`) and rebuilds it only when the guard
/// `[fontstring+0x60] & 1` is cleared: by `SetFont` (`0x7715e0`), `SetTextHeight` (`0x771666`),
/// `SetText` (`0x771ea4`) or a resolved-rect size change (`0x768d20`), and a spacing change
/// rebuilds at once (`0x77228f` → `0x7724a0`); nothing per frame reaches `0x771ec0` or `0x7724a0`.
/// This compares those inputs instead of keeping a dirty flag, sizes at the reference's `1e-5`, and
/// not position, as a move rebuilds nothing.
#[derive(Default)]
pub(crate) struct EllipsisMemo {
    entries: std::collections::HashMap<RegionHandle, Remembered>,
}

/// Every input [`layout::ellipsize_to_fit`] reads, and its answer.
struct Remembered {
    text: String,
    box_w: f32,
    box_h: f32,
    font: Option<String>,
    height: Option<f32>,
    outline: Outline,
    /// The line gap moves the fit, so it moves the answer.
    spacing: f32,
    /// So do the flag that stops the fit walk and the line limit.
    non_space_wrap: bool,
    max_lines: u32,
    /// The answer; `None` means the text fits and draws raw, also worth remembering.
    display: Option<String>,
}

/// The reference's rect-change tolerance (`0x768d20`): edges moving less than this rebuild nothing.
const RECT_EPS: f32 = 1e-5;

/// Entries past which the map is dropped whole, for handle churn: a `/reload` builds a new frame
/// tree, and the old regions' handles never come back.
const MEMO_CAP: usize = 4096;

impl EllipsisMemo {
    /// The remembered display string for `region`, if none of these inputs moved.
    fn get(
        &self,
        region: RegionHandle,
        text: &str,
        box_w: f32,
        box_h: f32,
        font: &FontSpec,
    ) -> Option<&Option<String>> {
        let e = self.entries.get(&region)?;
        (e.text == text
            && (e.box_w - box_w).abs() < RECT_EPS
            && (e.box_h - box_h).abs() < RECT_EPS
            && e.font.as_deref() == font.path
            && e.height == font.height
            && e.outline == font.outline
            && e.spacing.to_bits() == font.spacing.to_bits()
            && e.non_space_wrap == font.non_space_wrap
            && e.max_lines == font.max_lines)
            .then_some(&e.display)
    }

    fn put(
        &mut self,
        region: RegionHandle,
        text: &str,
        box_w: f32,
        box_h: f32,
        font: &FontSpec,
        display: Option<String>,
    ) {
        if self.entries.len() >= MEMO_CAP {
            self.entries.clear();
        }
        self.entries.insert(
            region,
            Remembered {
                text: text.to_string(),
                box_w,
                box_h,
                font: font.path.map(str::to_string),
                height: font.height,
                outline: font.outline,
                spacing: font.spacing,
                non_space_wrap: font.non_space_wrap,
                max_lines: font.max_lines,
                display,
            },
        );
    }
}

/// [`EllipsisMemo`]'s invalidation set, the reference's: a hit after an input moved would draw a
/// stale string, so each input gets its own test.
#[cfg(test)]
mod ellipsis_memo_tests {
    use super::*;
    use benilla_ui::widget::RegionHandle;

    fn spec(
        path: Option<&'static str>,
        height: Option<f32>,
        outline: Outline,
    ) -> FontSpec<'static> {
        FontSpec {
            path,
            height,
            outline,
            monochrome: false,
            spacing: 0.0,
            shadow_x: 0.0,
            alpha_gradient: None,
            non_space_wrap: false,
            max_lines: 0,
        }
    }

    fn base() -> FontSpec<'static> {
        spec(Some("Fonts\\MORPHEUS.TTF"), Some(15.0), Outline::None)
    }

    /// Real `RegionHandle`s, built as regions and read off `extract()` in draw order.
    pub(super) fn regions(n: usize) -> Vec<RegionHandle> {
        use benilla_ui::order::ZTarget;
        let mut script = benilla_ui::script::UiScript::new().expect("a VM");
        script.set_screen_size(1024.0, 768.0);
        script
            .run(
                r#"
                local f = CreateFrame("Frame", "MemoHost")
                f:SetPoint("TOPLEFT", 0, 0)
                f:SetWidth(100); f:SetHeight(100)
                for i = 1, 8 do
                    local t = f:CreateTexture(nil, "ARTWORK")
                    t:SetTexture(1, 0, 0)
                    t:SetAllPoints()
                end
            "#,
            )
            .expect("the fixture loads");
        script.resolve();
        let out: Vec<RegionHandle> = script
            .extract()
            .into_iter()
            .filter_map(|q| match q.target {
                ZTarget::Region(r) => Some(r),
                ZTarget::Frame(_) => None,
            })
            .collect();
        assert!(out.len() >= n, "the fixture makes enough regions");
        out.into_iter().take(n).collect()
    }

    /// A stored answer comes back under identical inputs, `None` included.
    #[test]
    fn identical_inputs_hit() {
        let mut m = EllipsisMemo::default();
        let r = regions(1)[0];
        m.put(
            r,
            "a long page body",
            270.0,
            304.0,
            &base(),
            Some("a lo...".into()),
        );
        assert_eq!(
            m.get(r, "a long page body", 270.0, 304.0, &base()),
            Some(&Some("a lo...".into()))
        );
        m.put(r, "short", 270.0, 304.0, &base(), None);
        assert_eq!(m.get(r, "short", 270.0, 304.0, &base()), Some(&None));
    }

    /// `SetText`, `SetFont`, `SetTextHeight` and a resize each miss, as each rebuilds in the
    /// reference; so does an outline change.
    #[test]
    fn a_changed_input_misses() {
        let mut m = EllipsisMemo::default();
        let r = regions(1)[0];
        m.put(r, "body", 270.0, 304.0, &base(), Some("bo...".into()));
        assert!(
            m.get(r, "other", 270.0, 304.0, &base()).is_none(),
            "SetText"
        );
        assert!(
            m.get(r, "body", 271.0, 304.0, &base()).is_none(),
            "wider box"
        );
        assert!(
            m.get(r, "body", 270.0, 305.0, &base()).is_none(),
            "taller box"
        );
        assert!(
            m.get(
                r,
                "body",
                270.0,
                304.0,
                &spec(Some("Fonts\\FRIZQT__.TTF"), Some(15.0), Outline::None)
            )
            .is_none(),
            "SetFont"
        );
        assert!(
            m.get(
                r,
                "body",
                270.0,
                304.0,
                &spec(Some("Fonts\\MORPHEUS.TTF"), Some(16.0), Outline::None)
            )
            .is_none(),
            "SetTextHeight"
        );
        assert!(
            m.get(
                r,
                "body",
                270.0,
                304.0,
                &spec(Some("Fonts\\MORPHEUS.TTF"), Some(15.0), Outline::Thick)
            )
            .is_none(),
            "the outline biases the step law, so it changes where the wrap breaks"
        );
        let flagged = FontSpec {
            non_space_wrap: true,
            ..base()
        };
        assert!(
            m.get(r, "body", 270.0, 304.0, &flagged).is_none(),
            "nonspacewrap moves where the fit walk stops"
        );
        let capped = FontSpec {
            max_lines: 2,
            ..base()
        };
        assert!(
            m.get(r, "body", 270.0, 304.0, &capped).is_none(),
            "maxLines sets the box's height"
        );
    }

    /// A size change under the reference's `1e-5` still hits; a real resize misses.
    #[test]
    fn sub_epsilon_rect_dust_still_hits() {
        let mut m = EllipsisMemo::default();
        let r = regions(1)[0];
        m.put(r, "body", 270.0, 304.0, &base(), Some("bo...".into()));
        assert!(m
            .get(r, "body", 270.0 + 1e-7, 304.0 - 1e-7, &base())
            .is_some());
        assert!(m.get(r, "body", 270.001, 304.0, &base()).is_none());
    }

    #[test]
    fn entries_are_per_region() {
        let mut m = EllipsisMemo::default();
        let r = regions(3);
        m.put(r[0], "body", 270.0, 304.0, &base(), Some("one...".into()));
        m.put(r[1], "body", 100.0, 40.0, &base(), Some("two...".into()));
        assert_eq!(
            m.get(r[0], "body", 270.0, 304.0, &base()),
            Some(&Some("one...".into()))
        );
        assert_eq!(
            m.get(r[1], "body", 100.0, 40.0, &base()),
            Some(&Some("two...".into()))
        );
        assert!(m.get(r[2], "body", 270.0, 304.0, &base()).is_none());
    }

    #[test]
    fn the_map_is_capped() {
        let mut m = EllipsisMemo::default();
        // The fixture yields few handles, so they are re-put under distinct texts.
        let r = regions(8);
        for i in 0..=MEMO_CAP {
            m.put(
                r[i % r.len()],
                &format!("body {i}"),
                270.0,
                304.0,
                &base(),
                None,
            );
        }
        assert!(m.entries.len() <= MEMO_CAP);
    }
}

/// The ellipsis truncate's gate (`0x771ec0`): a width, and a height or a `maxLines`.
#[cfg(test)]
mod ellipsis_gate_tests {
    use super::*;
    use bevy::math::Rect;

    fn spec(max_lines: u32) -> FontSpec<'static> {
        FontSpec {
            path: Some("Fonts\\FRIZQT__.TTF"),
            height: Some(12.0),
            outline: Outline::None,
            monochrome: false,
            spacing: 0.0,
            shadow_x: 0.0,
            alpha_gradient: None,
            non_space_wrap: false,
            max_lines,
        }
    }

    /// A string with a width and no height is never cut (`0x771f19`), unless it has `maxLines`
    /// (`0x771f0f`): `KLHThreatMeter`'s master-target text, one line wide as its bar.
    #[test]
    fn maxlines_arms_the_truncate_for_a_string_with_no_height() {
        benilla_formats::wow_data_or_skip!();
        let mut atlas = UiFontAtlas::for_test(1.0).expect("the client faces open");
        let region = super::ellipsis_memo_tests::regions(1)[0];
        let text = "Magister Kalendris the Unyielding of Dire Maul";
        let no_height = Rect::new(0.0, 0.0, 103.0, 0.0);
        assert_eq!(
            ellipsize_to_fit(&mut atlas, region, text, no_height, spec(0)),
            None,
            "no height and no line limit: drawn whole"
        );
        let one = ellipsize_to_fit(&mut atlas, region, text, no_height, spec(1))
            .expect("maxLines 1 cuts the long name");
        assert!(one.ends_with("..."), "{one:?}");
        let two = ellipsize_to_fit(&mut atlas, region, text, no_height, spec(2))
            .expect("maxLines 2 cuts it too");
        assert!(
            two.len() > one.len(),
            "two lines hold more than one: {one:?} {two:?}"
        );
        // A height of its own does not matter: the limit sets it.
        let tall = Rect::new(0.0, 0.0, 103.0, 200.0);
        assert_eq!(
            ellipsize_to_fit(&mut atlas, region, text, tall, spec(1)),
            Some(one)
        );
        // No width, no truncate.
        let narrow = Rect::new(0.0, 0.0, 0.0, 12.0);
        assert_eq!(
            ellipsize_to_fit(&mut atlas, region, text, narrow, spec(1)),
            None
        );
    }
}
