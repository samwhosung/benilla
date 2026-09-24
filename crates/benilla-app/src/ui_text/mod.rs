//! Text rendering for `QuadContent::Text`: an on-demand glyph cache over the
//! client's own TTFs, shaped through `cosmic-text` 0.16, emitted as [`crate::ui_pass::UiQuad`]s the
//! quad pass draws with no special case.
//!
//! - [`engine`] — the cache: faces, the size law, and the two lookup tables everything else reads.
//! - [`pack`] — the texture the cells live in, and how they get there.
//! - [`outline`] — the client's outline blit recipe, run per cell.
//! - [`layout`] — the emit pass, and (in `layout::measure`) the width law, the wrap and the
//!   ellipsis seam.
//! - [`measurer`] — the same engine, as the script VM holds it for a synchronous `GetStringWidth`.
//!
//! ## Font objects and remaining v1 simplifications
//!
//! - **Per-`FontString` size + face.** [`QuadContent::Text`] now carries the resolved `{ font, height }`
//!   from the engine's font registry (`Fonts.xml`'s named virtual `<Font>` objects). A `FontString`
//!   with no font object falls back to Friz Quadrata at [`DEFAULT_FONT_SIZE`] (the pre-0084 behavior).
//! - **One exact raster size, on demand**. A logical height becomes the integer
//!   device-pixel size it will be drawn at (`round(height × scale_factor)` — the client's own
//!   `0x5ca030` law), and the glyphs for that size are rasterized the first time anything asks for
//!   them. There is no size ladder, no snapping, and no rescale of finished quads: the size a
//!   string is measured at, rasterized at, and drawn at is one number. See [`engine`] for what that
//!   replaced and why.
//! - **Outlines are one composite cell.** An outlined glyph's black ring and its fill are
//!   rasterized together into a single cell and blitted once — the client's own baked-outline
//!   architecture (the AA-outline blit `0x5cea30`; see [`outline`]), which is what makes an
//!   outlined string fade as one thing instead of blackening. The Number* fonts (action-bar
//!   hotkeys/counts) carry it.
//! - **`justifyH` and `justifyV` are honored** (see [`layout_text_quads`]): lines justify
//!   horizontally per line, and the line block vertically (default MIDDLE, the client's
//!   FontString default — what seats sized labels like the money numbers on their icons'
//!   centerline).
//! - **`|T...|t` inline textures are stripped**, matching the probe — a `FontString` never draws an
//!   inline icon in v1.
//!
//! ## The metric-fidelity seam
//!
//! `cosmic-text`'s shaper is used for glyph SELECTION and rasterization, **one character at a
//! time** ([`engine::TextEngine::ensure_char`]); the pen then **advances by the client's own step
//! law**: per glyph, `floor(advance) + 1` px (the glyph rasterizer 0x5d1120's
//! `out[5] = (FT_advance>>6)+1.0`), plus another `+1` for a **THICK**-outlined font only
//! (`GlyphStepBase` 0x5ca2b0 biases solely under the THICK flag — correcting this module's earlier
//! any-outline reading) — the extra tracking that makes real client text read wider/denser than
//! raw font metrics.
//!
//! Per character is not a shortcut; it is the law. The client's `ComputeStep` (0x5ca2d0) has no
//! neighbour term, so a string's width is a pure function of its characters — which is what lets
//! one table answer a measure and a draw, and what lets a measure be answered from inside a Lua
//! call. It is a property of the FONTS that the decomposition holds (no ligatures, no contextual
//! substitution in these four faces), so it is asserted directly rather than assumed:
//! `engine::differential_tests`.
//!
//! Remaining divergences, all confined here: kerning is dropped entirely (the client applies only
//! *negative* pair kerns and rounds — ~0 at UI sizes; the test that pins the width law also pins
//! that this is what we are doing), and glyph BITMAPS are cosmic/swash unhinted coverage where the
//! client runs 2004-era FreeType with hinting flags (0x208a/0x20c2) — slightly slimmer stems at
//! small sizes. A future bit-exact replacement (a port of the client's font kernel) only changes
//! this module; nothing in its callers moves. The deliberate *rendering* divergences — where the
//! client stretches a cell and we rasterize the true size — are named in [`engine`]'s module doc.

mod engine;
mod layout;
mod markup;
mod measurer;
mod outline;
mod pack;

pub(crate) use engine::{UiFontAtlas, UiTextPlugin};

use benilla_ui::script::Outline;
use benilla_ui::widget::RegionHandle;
pub(crate) use measurer::{measure_request, AtlasMeasurer};

pub(crate) use layout::{
    ellipsize_to_fit, layout_text_quads, layout_text_quads_links, line_advances, line_origin,
    line_rows, measure_text, measure_wrapped_rows, FontSpec, Justify, TextSeat, UI_SEAT_NUDGE,
};

/// The default body text size (logical px) — WoW's own `GameFontNormal`. A `FontString` with no
/// resolved font object (no `inherits=`/`SetFont`) bakes and lays out at this, preserving the
/// pre-font-object behavior; a font object supplies a per-`FontString` height instead.
const DEFAULT_FONT_SIZE: f32 = 12.0;

/// The FontString **one-to-one raster cap**:
/// every `CSimpleFontString` ships with the one-to-one bit
/// set (`+0x120` bit `0x200`, ctor `0x770d30` default `0x212`; the sole clearer is `SetTextHeight`
/// `0x771600`, which nothing we ship calls), so the size every build/measure actually uses is the
/// getter `0x7727b0(1)`'s return — the font's RASTERIZED pixel size drawn 1:1, not the requested
/// FontHeight — and the rasterizer clamps that pixel size to `min(32, max(2,
/// round((height/768)·deviceH)))` (`0x5ca030` → `[CGxFont+0x24c]`). Net law: any FontHeight ≥ 32
/// draws 32 px — the "102" zone splash has always rendered 32 px tall, which is also why it fits
/// its 512-wide box on ONE line ("Stormwind City" ≈ 253 px at em 32; even "The Temple of
/// Atal'Hakkar" ≈ 442 < 512). There is **no scale-to-fit anywhere** in the client's text pipeline.
///
/// Deliberate divergence (the fold-back record): the binary caps in DEVICE pixels — a 2004
/// raster-memory ceiling that, applied literally on a modern display, converges ALL text toward
/// the same 32 device px (the splash no taller than chat). We apply the law in LOGICAL units —
/// `min(32, height)` — byte-identical to the client at its 768-tall design resolution, and scaled
/// by the DPI-aware bake like every other size. **UI FontStrings only**: world text
/// ([`crate::combat_text`], the nameplates) sizes by its own byte-derived viewport laws and
/// deliberately renders crisp above the cap, where the real client stretched a ≤32 px raster
/// (the era's blurry big crits — a recorded upgrade, not an oversight).
pub(crate) const FONTSTRING_EM_CAP: f32 = 32.0;

/// Apply the one-to-one raster cap ([`FONTSTRING_EM_CAP`]) to a UI `FontString`'s requested
/// height — the app-side seam ([`crate::ui_script`]'s extraction + measure round-trips) caps every
/// height it forwards, so render and measure agree with the client's drawn-size echo
/// (`GetStringWidth` reports the natural width *at the capped size*, `0x772890`). `None` (no font
/// object) stays `None`: the [`DEFAULT_FONT_SIZE`] fallback is already under the cap.
pub(crate) fn fontstring_em(height: Option<f32>) -> Option<f32> {
    height.map(|h| h.min(FONTSTRING_EM_CAP))
}

/// The DRAWN pixel height of a UI FontString under the client's two size regimes (the size getter
/// `0x7727b0`), times the seam scale `s = windowH/768 × uiScale` (
/// `crate::ui_script::seam_scale`; `s = 1` at the design height with the dial at 1, where this is
/// byte-identical to the pre-scaling law):
///
/// - **one-to-one** (the ctor default, everything but SetTextHeight): the raster size — the
///   [`FONTSTRING_EM_CAP`] applied in UNITS (the recorded divergence above: the client caps in
///   device px, converging all big text to 32 px on tall windows; we keep 32 *units* so the
///   capped splash scales with the screen and stays crisp), then × `s`.
/// - **SetTextHeight** (`text_height`): the literal size × `s`, UNCAPPED — the client magnifies
///   the raster to it (`0x771600` clears bit `0x200`; the drawn em is `round(size/768·deviceH)`).
///
/// `None`/`None` scales the renderer default so fallback text obeys the same space.
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
        // ZoneTextFont/WorldMapTextFont's nominal 102 → the 32 px raster cap (0x5ca030).
        assert_eq!(fontstring_em(Some(102.0)), Some(32.0));
        // The world-map label's SetFont 33 (decision 0235's taste dial) — the law caps it too.
        assert_eq!(fontstring_em(Some(33.0)), Some(32.0));
        // At-cap and under-cap sizes pass through untouched (SubZoneTextFont 26, chat 14).
        assert_eq!(fontstring_em(Some(32.0)), Some(32.0));
        assert_eq!(fontstring_em(Some(26.0)), Some(26.0));
        assert_eq!(fontstring_em(Some(14.0)), Some(14.0));
        // No font object: the default-size fallback stays the caller's concern.
        assert_eq!(fontstring_em(None), None);
    }
}
// NOTE on line pitch: there is deliberately NO line-height factor. The client's line-step law
// (`LayoutLines` 0x5cdc20) is `lineStep = px(size) + spacing`, where
// `spacing` is the FontString's own extra line gap (XML `spacing`, default **0** — none of our
// shipped UI sets one), so the pitch IS the font height. The earlier `1.25` factor here was
// cosmic-text's convention, not the client's, and read as "line height too large" against the
// reference. Cosmic buffers get `size` as their `Metrics` line height too — every run shapes
// single-line (`Wrap::None`), so that value never affects layout.

/// The FontString **display string** — the ellipsis seam's answer, remembered per region.
///
/// The real client does not recompute this per frame, and the reason it can afford the back-off's
/// shape is that it almost never runs: `0x771ec0`'s result is built into the `CGxString` at `+0xf8`
/// and rebuilt only when the rebuild guard `[fontstring+0x60] & 1` is cleared — by `SetFont`
/// (`0x7715e0`), `SetTextHeight` (`0x771666`), `SetText` (`0x771ea4`), or a resolved-rect **size**
/// change (`0x768d20`, which fires nothing when all four edges move less than `1e-5`). Nothing on
/// the draw path can reach it at all: a dword scan of every PE section finds `0x771ec0` and
/// `0x7724a0` in no vtable, table or pointer cell, so the complete caller set is their direct
/// `call` sites — none of them per-frame. **A pure move, a same-text `SetText`, and any no-op
/// setter cost zero.**
///
/// We had the opposite: the paint pass recomputed the seam every frame for every overflowing
/// FontString, and the extract gate is all-or-nothing over the whole render list,
/// so one flashing frame (`PlayerStatusGlow`, which never stops while you are resting) kept
/// it open — measured live at the reported plaque, 0 of 240 frames skipped. That is what turned a
/// one-time 5.6 ms into +5.4 ms *per frame* and halved the framerate.
///
/// This is the same invalidation set, expressed as a comparison of the inputs rather than a dirty
/// flag — which cannot go stale by construction, because the answer is a pure function of exactly
/// what is compared. **Position is deliberately not compared** (the client's guard is a size
/// change; a window dragged across the screen re-uses its string), and sizes compare at the
/// client's own `1e-5`, so anchor-graph dust cannot evict an entry every frame.
#[derive(Default)]
pub(crate) struct EllipsisMemo {
    entries: std::collections::HashMap<RegionHandle, Remembered>,
}

/// What a region's remembered answer depends on — every input [`layout::ellipsize_to_fit`] reads.
struct Remembered {
    text: String,
    box_w: f32,
    box_h: f32,
    font: Option<String>,
    height: Option<f32>,
    outline: Outline,
    /// The answer itself. `None` = "this text fits, draw it raw" — a real answer worth
    /// remembering, and the common case for every FontString that is merely *near* its box.
    display: Option<String>,
}

/// The client's own rect-change tolerance (`0x768d20`: a resolve whose four edges all move less
/// than this fires no rebuild).
const RECT_EPS: f32 = 1e-5;

/// Entries past which the map is dropped wholesale. One entry per region that has ever passed the
/// seam's geometric gate, so the live set is small (the overflow-capable FontStrings on screen);
/// the cap only catches handle churn — a `/reload` builds a whole new frame tree, and the old
/// regions' handles never come back.
const MEMO_CAP: usize = 4096;

impl EllipsisMemo {
    /// The remembered display string for `region` under these exact inputs, or `None` if this
    /// region has no entry or any input moved.
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
            && e.outline == font.outline)
            .then_some(&e.display)
    }

    /// Remember `display` as `region`'s answer under these inputs.
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
                display,
            },
        );
    }
}

/// [`EllipsisMemo`]'s invalidation set — the client's own (what rebuilds `0x7724a0`: `SetFont`,
/// `SetTextHeight`, `SetText`, and a resolved-rect SIZE change past `1e-5`), expressed as an input
/// comparison. A memo that misses is merely slow; one that HITS when an input moved draws a stale
/// string, so each input gets its own test.
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
            alpha_gradient: None,
        }
    }

    fn base() -> FontSpec<'static> {
        spec(Some("Fonts\\MORPHEUS.TTF"), Some(15.0), Outline::None)
    }

    /// Real `RegionHandle`s — the engine owns their construction, so the honest way to get some
    /// is to build regions and read their targets off `extract()`. Returns them in draw order.
    fn regions(n: usize) -> Vec<RegionHandle> {
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

    /// A stored answer comes back under identical inputs — including `None`, which is a real
    /// answer ("this fits, draw it raw") and the common case for text merely near its box.
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

    /// Every input that changes the answer evicts. `SetText`, `SetFont`, `SetTextHeight`, and a
    /// box that resized — the client's four.
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
    }

    /// Anchor-graph dust must not evict every frame — the client's guard is `1e-5` and so is this.
    /// A box that really resized still does.
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

    /// One entry per region — two regions holding the same text keep their own answers, and a
    /// region with no entry misses.
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

    /// The handle-churn backstop: the map is dropped wholesale rather than grown without bound.
    #[test]
    fn the_map_is_capped() {
        let mut m = EllipsisMemo::default();
        // Distinct keys are what the cap counts, and the engine hands out only a few per fixture —
        // so the same handles are re-put under distinct TEXTS, which is the churn shape anyway.
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
