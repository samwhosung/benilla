//! The font engine as the script VM holds it: the app's half of
//! [`benilla_ui::script::TextMeasure`], so `GetStringWidth` answers inline, as in the reference.
//! It holds the very [`TextEngine`] the render path holds, behind the same lock, and fills only
//! the metrics half ([`TextEngine::ensure_metrics`]). [`measure_request`] is the one measuring
//! body: the batch pass (`ui_script::extract::measure_fontstrings`) and [`AtlasMeasurer`] call it.

use std::sync::{Arc, Mutex};

use benilla_ui::script::{EditBoxAdvanceRequest, MeasureRequest, TextMeasure};

use super::engine::TextEngine;

/// Answer one [`MeasureRequest`] as `(laid_out_w, laid_out_h, natural_w)` in frame-local units. It
/// measures at the drawn raster size, the host's `seam` ([`crate::ui_script::seam_scale`]) times
/// the owner frame's `effective_scale`, and divides that back out: whole-pixel glyph steps do not
/// commute with scaling.
pub(crate) fn measure_request(
    e: &mut TextEngine,
    seam: f32,
    r: &MeasureRequest,
) -> (f32, f32, f32) {
    let rs = seam * r.scale;
    let spec = request_spec(r, rs);
    let wrap = r.wrap_width.map(|w| w * rs);
    // `GetStringWidth` answers the unwrapped width (the extent measure `0x5c6940`), its line seeded
    // with the shadow's x offset at the seam alone: `0x772890` leaves `[fs+0x108]` unscaled.
    let natural = super::string_width(e, &r.text, spec, r.shadow_x * seam);
    // Unwrapped, the laid-out width is that same measure (`[fs+0xfc]`).
    let (w, h) = match wrap {
        Some(w) => {
            // `0x7729b0` measures the string `0x771ec0` returns: with `maxLines` the text is cut
            // to that many lines first, and a FontString with no height takes its own from it.
            let cut = (r.max_lines > 0)
                .then(|| super::ellipsize_text(e, &r.text, w, 0.0, spec))
                .flatten();
            super::measure_text(e, cut.as_deref().unwrap_or(&r.text), wrap, spec)
        }
        None => (natural, super::unwrapped_height(e, &r.text, spec)),
    };
    (w / rs, h / rs, natural / rs)
}

/// `r`'s font at the drawn raster size `rs`, the seam times the owner's scale.
fn request_spec(r: &MeasureRequest, rs: f32) -> super::FontSpec<'_> {
    super::FontSpec {
        path: r.font.as_deref(),
        // The render pass's drawn px: `GetStringWidth` and `GetStringHeight` report the drawn
        // size (`0x772890`).
        height: super::drawn_px(r.height, r.text_height, rs),
        // THICK adds 1 px per glyph to the step law (`0x5ca2b0`), so the measure needs it.
        outline: r.outline,
        monochrome: false,
        // The gap between wrapped lines, scaled as the height is (`0x5c2070`).
        spacing: r.spacing * rs,
        // The drawn rows' seed rides the frame scale, as the draw's does (`0x7726a4`).
        shadow_x: r.shadow_x * rs,
        alpha_gradient: None, // alpha never changes metrics
        non_space_wrap: r.non_space_wrap,
        max_lines: r.max_lines,
    }
}

/// The widest of the first `max_pieces` pieces of `r`'s text split at its wrap width, laid out at
/// the size [`measure_request`] measures at. A piece runs from its row's start (`0x5c2430`) to the
/// next one's, the last to the text's end (`0x530884`-`0x5308a8`), so it keeps the whitespace the
/// wrap skipped (`0x5c24fe`); it is measured with `GetStringWidth`'s kernel and seed (`0x772ae0`).
pub(crate) fn piece_width(
    e: &mut TextEngine,
    seam: f32,
    r: &MeasureRequest,
    max_pieces: usize,
) -> f32 {
    let rs = seam * r.scale;
    let spec = request_spec(r, rs);
    let wrap = r.wrap_width.map_or(0.0, |w| w * rs);
    let mut starts = super::line_rows(e, &r.text, wrap, spec).0;
    starts.truncate(max_pieces);
    // Kept only when wider, from 0 (`0x53086d`, `0x5308ad`).
    let mut widest = 0.0f32;
    for (i, &at) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(r.text.len());
        let piece = r.text.get(at..end).unwrap_or("");
        let w = super::string_width(e, piece, spec, r.shadow_x * seam);
        if w > widest {
            widest = w;
        }
    }
    widest / rs
}

/// Answer one EditBox advance table as `(cumulative widths, row starts, row pitch)` in screen UI
/// units: measured at the drawn size, the seam times the box's scale, and divided by the seam
/// alone, like the mouse feed. A multi-line box also gets the draw's row starts and pitch. The one
/// body for the extract's round trip and [`AtlasMeasurer`]'s inline answer.
pub(crate) fn editbox_advances(
    e: &mut TextEngine,
    seam: f32,
    req: &EditBoxAdvanceRequest,
) -> (Vec<f32>, Vec<usize>, f32) {
    let spec = super::FontSpec {
        path: req.font.as_deref(),
        height: super::drawn_px(req.height, None, seam * req.scale),
        outline: req.outline,
        monochrome: false,
        // A multiline box's row pitch adds the gap (`0x77d161`).
        spacing: req.spacing * seam * req.scale,
        // The draw's row seed, at the box's drawn scale.
        shadow_x: req.shadow_x * seam * req.scale,
        alpha_gradient: None, // alpha never changes metrics
        // A multi-line box's text is nonspacewrap (`0x77a58f`); a single line never wraps.
        non_space_wrap: true,
        max_lines: 0,
    };
    let cum = super::line_advances(e, &req.text, spec)
        .iter()
        .map(|a| a / seam)
        .collect();
    let (rows, cell_h) = match req.wrap_width {
        Some(w) => super::line_rows(e, &req.text, w * seam, spec),
        None => (vec![0], 0.0),
    };
    (cum, rows, cell_h / seam)
}

/// The measurer installed into the VM: the shared engine and the host's screen seam. It answers
/// only for the seam it was built under, so the host installs a fresh one when the seam moves
/// (`ui_script::extract::seat_text_measurer`).
pub(crate) struct AtlasMeasurer {
    engine: Arc<Mutex<TextEngine>>,
    seam: f32,
}

impl AtlasMeasurer {
    pub(crate) fn new(engine: Arc<Mutex<TextEngine>>, seam: f32) -> Self {
        Self { engine, seam }
    }
}

impl TextMeasure for AtlasMeasurer {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        // Taken and released inside this call: a VM tick can land here, so nothing on the app side
        // may hold the engine lock across one.
        let mut e = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        measure_request(&mut e, self.seam, req)
    }

    fn editbox_advances(
        &mut self,
        req: &EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let mut e = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(editbox_advances(&mut e, self.seam, req))
    }

    fn piece_width(&mut self, req: &MeasureRequest, max_pieces: usize) -> Option<f32> {
        let mut e = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(piece_width(&mut e, self.seam, req, max_pieces))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use benilla_ui::script::UiScript;

    use super::AtlasMeasurer;

    /// A multi-line EditBox is its text's measured height tall (`0x77d4d0` @`0x77d8ad`), one
    /// line's when empty, and the line law the host measures by (`0x5c2070`) folds a trailing
    /// break into the line it ends, so the break alone adds no line.
    #[test]
    fn a_multi_line_boxs_trailing_newline_adds_no_line() {
        benilla_formats::wow_data_or_skip!();
        let engine = super::super::engine::test_engine(1.0).expect("the client faces open");
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.set_text_measurer(Box::new(AtlasMeasurer::new(
            Arc::new(Mutex::new(engine)),
            1.0,
        )));
        s.run(
            r#"
            E = CreateFrame("EditBox", "E")
            E:SetAutoFocus(false); E:SetMultiLine(true)
            E:SetPoint("TOPLEFT", 100, -100); E:SetWidth(270); E:SetHeight(200)
            E:SetFont("Fonts\\MORPHEUS.TTF", 15)
        "#,
        )
        .unwrap();
        s.resolve();
        let mut height = |text: &str| {
            s.run(&format!("E:SetText({text:?})")).unwrap();
            s.tick(0.016);
            s.resolve();
            s.eval::<f32>("return E:GetHeight()").unwrap()
        };
        let empty = height("");
        let one = height("abc");
        assert_eq!(one, empty, "an empty box is one line tall");
        assert_eq!(height("abc\n"), one, "the break alone");
        let two = height("abc\nd");
        assert!(
            (two - 2.0 * one).abs() < 1e-4,
            "the first character on the new line: {two} against {one}"
        );
    }

    /// A wrapped FontString is `N·S + (N−1)·gap` tall (`0x5c2070`), the gap being its spacing
    /// rounded up to a whole device pixel (`0x5c2146`-`0x5c215c`), and a negative spacing 0, as the
    /// FontString's setter clamps it (`0x772240`).
    #[test]
    fn a_wrapped_fontstring_is_its_lines_plus_the_gaps_between_them() {
        benilla_formats::wow_data_or_skip!();
        let engine = super::super::engine::test_engine(1.0).expect("the client faces open");
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.set_text_measurer(Box::new(AtlasMeasurer::new(
            Arc::new(Mutex::new(engine)),
            1.0,
        )));
        s.run(
            r#"
            FS = CreateFrame("Frame", "GapHost"):CreateFontString("GapText")
            FS:SetPoint("TOPLEFT", 10, -10)
            FS:SetWidth(60)
            FS:SetFont("Fonts\\FRIZQT__.TTF", 12)
            FS:SetText("one two three four five six")
        "#,
        )
        .unwrap();
        let mut height = |spacing: f32| {
            s.run(&format!("FS:SetSpacing({spacing})")).unwrap();
            s.tick(0.016);
            s.resolve();
            s.eval::<f32>("return FS:GetHeight()").unwrap()
        };
        let flush = height(0.0);
        let lines = (flush / 12.0).round();
        assert!(
            lines >= 3.0 && (flush - lines * 12.0).abs() < 1e-4,
            "the text wraps to whole 12 px lines: {flush}"
        );
        let gaps = lines - 1.0;
        assert!((height(4.0) - (flush + gaps * 4.0)).abs() < 1e-4);
        assert!(
            (height(2.5) - (flush + gaps * 3.0)).abs() < 1e-4,
            "the gap rounds up to the device pixel"
        );
        assert!((height(-5.0) - flush).abs() < 1e-4, "a negative gap is 0");
    }

    /// A FontString with a width and no height takes its height from the text `0x771ec0` returns
    /// (`0x7729b0` measures it): with `maxLines` that is the text cut to that many lines and
    /// "...", however many lines it wraps to whole. `$parentSpellName` of the stock spellbook
    /// (103 wide, `maxLines="3"`) is the one stock user.
    #[test]
    fn an_auto_height_fontstring_with_maxlines_is_as_tall_as_the_cut_text() {
        benilla_formats::wow_data_or_skip!();
        let engine = super::super::engine::test_engine(1.0).expect("the client faces open");
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.set_text_measurer(Box::new(AtlasMeasurer::new(
            Arc::new(Mutex::new(engine)),
            1.0,
        )));
        let region = |name: &str, extra: &str, y: i32| {
            format!(
                r#"<FontString name="{name}" font="Fonts\FRIZQT__.TTF" justifyH="LEFT" {extra}>
                     <Size><AbsDimension x="103"/></Size>
                     <FontHeight><AbsValue val="12"/></FontHeight>
                     <Anchors><Anchor point="TOPLEFT"><Offset><AbsDimension x="0" y="{y}"/></Offset></Anchor></Anchors>
                   </FontString>"#
            )
        };
        let xml = format!(
            r#"<Ui><Frame name="CapHost"><Size><AbsDimension x="300" y="300"/></Size>
                 <Anchors><Anchor point="TOPLEFT"/></Anchors>
                 <Layers><Layer level="ARTWORK">{}{}{}</Layer></Layers></Frame></Ui>"#,
            region("Capped", r#"maxLines="2""#, 0),
            region("Whole", "", -100),
            region("Spare", r#"maxLines="9""#, -200),
        );
        let doc = benilla_ui::framexml::parse(&xml).expect("valid FrameXML");
        let report = benilla_ui::loader::load(&s, &doc, &|_| None);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let long = "Two-Handed Weapon Specialization of the Unyielding Mountain King";
        s.run(&format!(
            "for _, n in ipairs({{'Capped', 'Whole', 'Spare'}}) do getfenv(0)[n]:SetText({long:?}) end"
        ))
        .unwrap();
        s.tick(0.016);
        s.resolve();
        let height = |n: &str| s.eval::<f32>(&format!("return {n}:GetHeight()")).unwrap();
        let whole = height("Whole");
        assert!(whole > 36.0, "the text wraps to over three lines: {whole}");
        assert_eq!(height("Capped"), 24.0, "two lines of 12");
        assert_eq!(
            height("Spare"),
            whole,
            "text within its lines is measured as it is"
        );
    }

    /// A tooltip split's piece runs from its row's start to the next (`0x530884`-`0x5308a8`): it
    /// keeps the space its wrap skipped, a space being a glyph to the extent kernel (`0x5c6940`),
    /// and a piece crossing a LF is as wide as its widest line, the LF no glyph.
    #[test]
    fn a_split_piece_keeps_its_space_and_ends_its_line_at_a_lf() {
        benilla_formats::wow_data_or_skip!();
        let mut e = super::super::engine::test_engine(1.0).expect("the client faces open");
        let req = |text: &str, wrap: Option<f32>| benilla_ui::script::MeasureRequest {
            id: 0,
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(12.0),
            text_height: None,
            wrap_width: wrap,
            outline: benilla_ui::script::Outline::None,
            spacing: 0.0,
            shadow_x: 0.0,
            non_space_wrap: false,
            max_lines: 0,
            scale: 1.0,
            text: text.into(),
            key: 0,
        };
        let natural = |e: &mut super::TextEngine, text: &str| {
            super::measure_request(e, 1.0, &req(text, None)).2
        };
        let (word, spaced) = (natural(&mut e, "abcd"), natural(&mut e, "abcd "));
        assert!(spaced > word, "a space is a glyph: {spaced} against {word}");
        // Wide enough for "abcd" and not for "abcd ef": rows "abcd", "ef" and, past the LF, "gh".
        let wrap = Some(word + 1.0);
        let text = "abcd ef\ngh";
        let r = req(text, wrap);
        let rows = super::super::line_rows(&mut e, text, word + 1.0, super::request_spec(&r, 1.0));
        assert_eq!(rows.0, [0, 5, 8]);
        let got = super::piece_width(&mut e, 1.0, &req(text, wrap), 20);
        assert_eq!(got, spaced, "the first piece, its space and all");
        // One piece: the whole text, as wide as its widest line.
        let got = super::piece_width(&mut e, 1.0, &req(text, wrap), 1);
        assert_eq!(got, natural(&mut e, "abcd ef"));
        // The first piece crosses the LF: "abcd\n" is as wide as "abcd".
        let got = super::piece_width(&mut e, 1.0, &req("abcd\nef", wrap), 20);
        assert_eq!(got, word);
    }

    /// The stock GameTooltip's second line (`GameTooltipTemplate.xml:35`, `GameTooltipText`)
    /// wrapped past 20 rows at 230.4: the split stops at 20 pieces (`0x53083f`), the 20th running
    /// to the text's end unwrapped (`0x53088e`), so the plate is that tail's `GetStringWidth` and
    /// the pads.
    #[test]
    fn a_stock_tooltip_line_past_20_rows_sizes_to_its_20th_piece() {
        benilla_formats::wow_data_or_skip!();
        let engine = super::super::engine::test_engine(1.0).expect("the client faces open");
        let mut s = UiScript::new().unwrap();
        for file in [
            r"Interface\FrameXML\GlobalStrings.lua",
            r"Interface\FrameXML\Fonts.xml",
            r"Interface\FrameXML\BasicControls.xml",
            r"Interface\FrameXML\LocaleProperties.lua",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            r"Interface\FrameXML\GameTooltip.xml",
        ] {
            crate::ui_script::load_ui_for_test(&s, file);
        }
        s.set_screen_size(1024.0, 768.0);
        s.set_text_measurer(Box::new(AtlasMeasurer::new(
            Arc::new(Mutex::new(engine)),
            1.0,
        )));
        s.resolve();
        // How many words of "word" a 230.4-wide row of the line's font holds.
        let per_row: usize = s
            .eval(
                r#"
                P = CreateFrame("Frame", nil, UIParent):CreateFontString(nil, "ARTWORK", "GameTooltipText")
                P:SetWidth(230.4)
                P:SetText("word")
                local one = P:GetHeight()
                local n = 1
                while P:GetHeight() == one do
                    n = n + 1
                    P:SetText(string.rep("word ", n - 1) .. "word")
                end
                return n - 1
            "#,
            )
            .unwrap();
        assert!(per_row > 2, "{per_row} words a row");
        let words = |n: usize| vec!["word"; n].join(" ");
        let plate = |rows: usize| -> f32 {
            s.eval(&format!(
                r#"GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
                GameTooltip:AddLine("x") GameTooltip:AddLine("{}", 1, 1, 1, 1) GameTooltip:Show()
                return GameTooltip:GetWidth()"#,
                words(per_row * rows)
            ))
            .unwrap()
        };
        let natural = |n: usize| -> f32 {
            s.eval(&format!(
                r#"P:SetText("{}") return P:GetStringWidth()"#,
                words(n)
            ))
            .unwrap()
        };
        use benilla_ui::widget::TOOLTIP_PLATE_PAD;
        let pads = 2.0 * TOOLTIP_PLATE_PAD;
        let (nineteen, twenty) = (plate(19), plate(20));
        assert_eq!(twenty, nineteen, "20 rows: the 20th piece is the last row");
        // A piece before the last keeps the space its row's wrap skipped (`0x530884`).
        let row = pads + natural(per_row);
        assert!(twenty > row, "{twenty} against the row alone, {row}");
        let tail = pads + natural(2 * per_row);
        let got = plate(21);
        assert!(
            (got - tail).abs() < 1e-3,
            "21 rows: the plate is {got}, the last two rows unwrapped {tail}"
        );
    }

    /// `GetStringWidth` (`0x79e510` → `0x772890`) answers the measure kernel's width divided by the
    /// drawn scale, as it stands, fractions and all: no `ceil` and no added pixel. A frame sized
    /// from it, as `MoneyFrame.lua:202-208` sizes each coin (`SetWidth(GetTextWidth() + 13)`), gets
    /// exactly that plus its icon. The shadow's x offset seeds the line (`0x5c69a4`), rounded to
    /// device px and not scaled with the frame: `0x772890` scales the height by `[fs+0x7c]` but
    /// hands the offset `[fs+0x108]` over as it is.
    #[test]
    fn get_string_width_answers_the_reference_measure() {
        benilla_formats::wow_data_or_skip!();
        // The reference's widths in device px for FRIZQT__.TTF, computed outside this code base off
        // the font's tables (as `engine::line_law_tests`), at 12 px and 17 px, and "1234" at
        // 24 px and 34 px for scale 2.
        let golden = |ppem: f32, text: &str| -> f32 {
            match (ppem.round() as u16, text) {
                (12, "1") => 7.0,
                (12, "27") => 16.0,
                (12, "1234") => 32.0,
                (12, "Gold") => 31.0,
                (12, "Ironforge") => 59.0,
                (17, "1") => 9.0,
                (17, "27") => 21.0,
                (17, "1234") => 44.0,
                (17, "Gold") => 41.0,
                (17, "Ironforge") => 79.0,
                (24, "1234") => 62.0,
                (34, "1234") => 86.0,
                other => panic!("no golden for {other:?}"),
            }
        };
        let law = golden;
        // A 768-tall window, then a 1080-tall one, where a unit is 1.40625 px.
        for seam in [1.0f32, 1080.0 / 768.0] {
            let engine = super::super::engine::test_engine(1.0).expect("the client faces open");
            let mut s = UiScript::new().unwrap();
            s.set_screen_size(1024.0, 768.0);
            s.set_text_measurer(Box::new(AtlasMeasurer::new(
                Arc::new(Mutex::new(engine)),
                seam,
            )));
            s.run(
                r#"
                F = CreateFrame("Frame", "F")
                F:SetPoint("TOPLEFT", 100, -100); F:SetHeight(20)
                FS = F:CreateFontString("FS")
                FS:SetFont("Fonts\\FRIZQT__.TTF", 12)
                FS:SetPoint("LEFT", F, "LEFT")
            "#,
            )
            .unwrap();
            s.resolve();
            for text in ["1", "27", "1234", "Gold", "Ironforge"] {
                s.run(&format!(
                    "FS:SetText({text:?}); F:SetWidth(FS:GetStringWidth() + 13)"
                ))
                .unwrap();
                s.resolve();
                let w = s.eval::<f32>("return FS:GetStringWidth()").unwrap();
                let want = law(12.0 * seam, text) / seam;
                benilla_ui::assert_near!(w, want, "seam {seam}, {text:?}: GetStringWidth");
                let framed = s.eval::<f32>("return F:GetWidth()").unwrap();
                assert!(
                    (framed - (want + 13.0)).abs() < 1e-4,
                    "seam {seam}, {text:?}: the coin frame is {framed}, not {want} + 13"
                );
            }

            // A whole-pixel shadow to the right adds its pixels; one to the left, or under half a
            // pixel, adds none.
            for (dx, pad) in [(1.0f32, 1.0f32), (2.0, 2.0), (0.3, 0.0), (-1.0, 0.0)] {
                s.run(&format!(
                    "FS:SetShadowOffset({dx}, -1); FS:SetText(\"1234\")"
                ))
                .unwrap();
                let w = s.eval::<f32>("return FS:GetStringWidth()").unwrap();
                let pad = if pad > 0.0 { (dx * seam).round() } else { 0.0 };
                let want = (law(12.0 * seam, "1234") + pad) / seam;
                benilla_ui::assert_near!(w, want, "seam {seam}, shadow x {dx}");
            }

            // At scale 2 the glyphs measure twice the size, but the shadow keeps its pixel.
            s.run("FS:SetShadowOffset(1, -1); F:SetScale(2); FS:SetText(\"1234\")")
                .unwrap();
            s.resolve();
            let w = s.eval::<f32>("return FS:GetStringWidth()").unwrap();
            let want = (law(24.0 * seam, "1234") + seam.round()) / (2.0 * seam);
            benilla_ui::assert_near!(w, want, "seam {seam}, scale 2");
        }
    }
}
