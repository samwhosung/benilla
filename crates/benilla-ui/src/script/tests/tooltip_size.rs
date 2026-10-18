//! The tooltip sizer (`0x530640`) as the show, `AppendText` and the clear reach it, measured in the
//! call through a stand-in font engine.

use std::cell::Cell;
use std::rc::Rc;

use super::common::script;
use crate::script::{MeasureRequest, TextMeasure, UiScript};

/// Each byte's advance, whatever the font.
const ADVANCE: f32 = 5.0;

/// A stand-in font engine: each byte [`ADVANCE`] wide, each row as tall as the font, wrapping at
/// spaces as the host's does; it counts the measures it answers.
pub(in crate::script::tests) struct Mono(pub(in crate::script::tests) Rc<Cell<usize>>);

impl TextMeasure for Mono {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        self.0.set(self.0.get() + 1);
        let row = req.text_height.or(req.height).unwrap_or(12.0);
        let width = |s: &str| s.len() as f32 * ADVANCE;
        let natural = width(&req.text);
        let Some(wrap) = req.wrap_width else {
            return (natural, row, natural);
        };
        let mut rows: Vec<String> = Vec::new();
        for word in req.text.split(' ') {
            match rows.last_mut() {
                Some(last) if width(last) + width(" ") + width(word) <= wrap => {
                    last.push(' ');
                    last.push_str(word);
                }
                _ => rows.push(word.to_string()),
            }
        }
        let widest = rows.iter().map(|r| width(r)).fold(0.0, f32::max);
        (widest, row * rows.len() as f32, natural)
    }
}

/// A screen, the template, a counting engine, and an owner `O` with tooltip `TS` on it.
fn tooltip() -> (UiScript, Rc<Cell<usize>>) {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let calls = Rc::new(Cell::new(0));
    s.set_text_measurer(Box::new(Mono(calls.clone())));
    s.run(
        r#"
        O = CreateFrame("Button", "O")
        O:SetPoint("TOPLEFT", 100, -100); O:SetWidth(40); O:SetHeight(40)
        TS = CreateFrame("GameTooltip", "TS", nil, "TestTooltipTemplate")
        TS:SetOwner(O, "ANCHOR_RIGHT")
        "#,
    )
    .unwrap();
    (s, calls)
}

/// The plate's width for a widest line of `widest`: `2·10.24 + padding + widest` (`0x5309f4`).
fn plate(widest: f32) -> f64 {
    f64::from(2.0 * 10.24f32 + 0.0 + widest)
}

fn close(got: f64, want: f64) -> bool {
    (got - want).abs() < 1e-4
}

fn size(s: &UiScript) -> (f64, f64) {
    s.eval("return TS:GetWidth(), TS:GetHeight()").unwrap()
}

/// The constants are the reference's floats (`0x808e6c`-`0x808e80`).
#[test]
fn the_sizer_constants_are_the_reference_floats() {
    use crate::widget::*;
    assert_eq!(TOOLTIP_PLATE_PAD.to_bits(), 0x4123_d70a);
    assert_eq!(TOOLTIP_COLUMN_GAP.to_bits(), 0x4219_999a);
    assert_eq!(TOOLTIP_WRAP_WIDTH.to_bits(), 0x4366_6666);
    assert_eq!(TOOLTIP_LINE_GAP.to_bits(), 0x4000_0000);
}

/// The show sizes the plate before it returns (`0x530aad`), and so do `SetText` (its show,
/// `0x531e08`) and `AppendText` (`0x530615`): the same chunk reads the new size.
#[test]
fn show_set_text_and_append_text_size_the_plate_in_the_call() {
    let (s, _) = tooltip();
    let (w, h): (f64, f64) = s
        .eval(r#"TS:AddLine("abcdefghij") TS:Show() return TS:GetWidth(), TS:GetHeight()"#)
        .unwrap();
    // The header font is 14 tall.
    assert!(close(w, plate(50.0)) && close(h, plate(14.0)), "{w}x{h}");
    let w: f64 = s
        .eval(r#"TS:SetText("abcde") return TS:GetWidth()"#)
        .unwrap();
    assert!(close(w, plate(25.0)), "SetText: {w}");
    let w: f64 = s
        .eval(r#"TS:AppendText("fghijklmno") return TS:GetWidth()"#)
        .unwrap();
    assert!(close(w, plate(75.0)), "AppendText: {w}");
}

/// `AddLine` sizes nothing (`0x531630` → `0x5303b0`): the plate keeps the last sizing until the next
/// show, through a resolve, and a script's size written after the line stands.
#[test]
fn add_line_keeps_the_last_size_until_the_next_show() {
    let (mut s, _) = tooltip();
    s.run(r#"TS:AddLine("abcde") TS:Show()"#).unwrap();
    let before = size(&s);
    s.run(r#"TS:AddLine("a much longer second line")"#).unwrap();
    assert_eq!(size(&s), before, "AddLine sized the plate");
    s.resolve();
    assert_eq!(size(&s), before, "the resolve sized the plate");
    // The addons' pattern: AddLine, then room for it by hand (`CT_MailMod.lua:494`).
    s.run("TS:SetHeight(TS:GetHeight() + 14)").unwrap();
    s.resolve();
    s.tick(0.016);
    s.resolve();
    assert!(close(size(&s).1, before.1 + 14.0), "{:?}", size(&s));
    s.run("TS:Show()").unwrap();
    let (w, h) = size(&s);
    // 25 bytes; the second line is 12 tall, 2 under the first.
    assert!(
        close(w, plate(125.0)) && close(h, plate(14.0 + 2.0 + 12.0)),
        "{w}x{h}"
    );
}

/// A double line is its two texts and the 38.4 gap (`0x5306cf`); its right text sits at the
/// widest line's width from its left text's LEFT (`0x53094f`).
#[test]
fn a_double_line_counts_the_column_gap() {
    let (s, _) = tooltip();
    s.run(r#"TS:AddDoubleLine("aaaaaaaaaa", "bbbbbb") TS:Show()"#)
        .unwrap();
    let (w, _) = size(&s);
    assert!(close(w, plate(50.0 + 38.4 + 30.0)), "{w}");
    let x: f64 = s
        .eval("local _, _, _, x = TSTextRight1:GetPoint(1) return x")
        .unwrap();
    assert!(close(x, f64::from(50.0f32 + 38.4 + 30.0)), "{x}");
}

/// The height sums the shown left texts (`0x530983`, `0x5309c4`): a row with only a right text, a
/// taller right text (#1182) and a hidden left text add nothing, but a right text's width counts.
#[test]
fn the_height_sums_the_shown_left_texts_alone() {
    let (s, _) = tooltip();
    s.run(r#"TS:AddLine("abcdefghij") TS:AddDoubleLine("", "bbbbbbbbbbbbbbbb") TS:Show()"#)
        .unwrap();
    let (w, h) = size(&s);
    assert!(
        close(w, plate(80.0)) && close(h, plate(14.0)),
        "right only: {w}x{h}"
    );

    s.run(
        r#"TS:ClearLines()
        TSTextRight1:SetFont("Fonts\\FRIZQT__.TTF", 30)
        TS:AddDoubleLine("aaaa", "bbbb") TS:Show()"#,
    )
    .unwrap();
    assert!(
        close(size(&s).1, plate(14.0)),
        "taller right: {:?}",
        size(&s)
    );

    s.run(
        r#"TS:ClearLines()
        TS:AddLine("a") TS:AddLine("b") TS:AddLine("c")
        TSTextLeft2:Hide() TS:Show()"#,
    )
    .unwrap();
    assert!(
        close(size(&s).1, plate(14.0 + 2.0 + 12.0)),
        "hidden left: {:?}",
        size(&s)
    );
}

/// A left text's authored height is its `GetHeight` (`0x772a60`), which the sum reads; the clear
/// keeps it, as it resets the width alone (`0x5301f0`).
#[test]
fn an_authored_line_height_counts_and_the_clear_keeps_it() {
    let (s, _) = tooltip();
    s.run(
        r#"TS:AddLine("a") TS:AddLine("b")
        TSTextLeft2:SetWidth(100); TSTextLeft2:SetHeight(24)
        TS:ClearLines()"#,
    )
    .unwrap();
    let (w, h): (f64, f64) = s
        .eval("return TSTextLeft2:GetWidth(), TSTextLeft2:GetHeight()")
        .unwrap();
    assert_eq!((w, h), (1.0, 24.0), "the clear zeroes the width alone");
    s.run(r#"TS:AddLine("a") TS:AddLine("b") TS:Show()"#)
        .unwrap();
    assert!(
        close(size(&s).1, plate(14.0 + 2.0 + 24.0)),
        "{:?}",
        size(&s)
    );
}

/// A wrapped line (`0x530790`): its text's width capped at 230.4; wider than the widest line so
/// far, the widest piece of the text split there; then the line is as wide as the widest so far.
#[test]
fn a_wrapped_line_takes_the_widest_so_far() {
    let (s, _) = tooltip();
    // Short and alone: as wide as its text.
    s.run(r#"TS:AddLine("abcd", 1, 1, 1, 1) TS:Show()"#)
        .unwrap();
    let lw: f64 = s.eval("return TSTextLeft1:GetWidth()").unwrap();
    assert!(
        close(lw, 20.0) && close(size(&s).0, plate(20.0)),
        "short: {lw}"
    );

    // Long and alone: twelve 4-byte words, 295 wide, split at 230.4 into rows of 220 and 70.
    let long = ["aaaa"; 12].join(" ");
    s.run(&format!(
        r#"TS:ClearLines() TS:AddLine("{long}", 1, 1, 1, 1) TS:Show()"#
    ))
    .unwrap();
    let lw: f64 = s.eval("return TSTextLeft1:GetWidth()").unwrap();
    let (w, h) = size(&s);
    assert!(
        close(lw, 220.0) && close(w, plate(220.0)) && close(h, plate(28.0)),
        "long: {lw}, {w}x{h}"
    );

    // Under a 300-wide line: no split, the line takes 300 and fits on one row.
    s.run(&format!(
        r#"TS:ClearLines() TS:AddLine("{}") TS:AddLine("{long}", 1, 1, 1, 1) TS:Show()"#,
        "x".repeat(60)
    ))
    .unwrap();
    let lw: f64 = s.eval("return TSTextLeft2:GetWidth()").unwrap();
    let (w, h) = size(&s);
    assert!(
        close(lw, 300.0) && close(w, plate(300.0)) && close(h, plate(14.0 + 2.0 + 12.0)),
        "under a wider line: {lw}, {w}x{h}"
    );
}

/// With no owner the show hides and neither it nor `AppendText` sizes (`0x530a83`, `0x530649`).
#[test]
fn an_ownerless_show_hides_and_sizes_nothing() {
    let (mut s, _) = tooltip();
    s.run(
        r#"Bare = CreateFrame("GameTooltip", "Bare", nil, "TestTooltipTemplate")
        Bare:SetWidth(77); Bare:SetHeight(33)
        Bare:AddLine("abcdefghij") Bare:Show() Bare:AppendText("klmno")"#,
    )
    .unwrap();
    s.resolve();
    let (shown, w, h): (bool, f64, f64) = s
        .eval("return Bare:IsShown() == 1, Bare:GetWidth(), Bare:GetHeight()")
        .unwrap();
    assert_eq!((shown, w, h), (false, 77.0, 33.0));
}

/// The sizer re-applies the anchor without its clear (`0x530a49` → `0x52fe90(0)`): an owner mode
/// sets its point again at the stored offsets, `ANCHOR_NONE` keeps the script's points and
/// `ANCHOR_PRESERVE` keeps the plate as it was.
#[test]
fn the_show_re_anchors_the_owner_modes_only() {
    let (s, _) = tooltip();
    s.run(
        r#"TS:SetOwner(O, "ANCHOR_RIGHT", 3, 4) TS:AddLine("x") TS:Show()
        TS:ClearAllPoints() TS:Show()"#,
    )
    .unwrap();
    let (n, p, rel, rp, x, y): (i64, String, bool, String, f64, f64) = s
        .eval(
            "local p, rel, rp, x, y = TS:GetPoint(1)
            return TS:GetNumPoints(), p, rel == O, rp, x, y",
        )
        .unwrap();
    assert_eq!(
        (n, p.as_str(), rel, rp.as_str(), x, y),
        (1, "BOTTOMLEFT", true, "TOPRIGHT", 3.0, 4.0)
    );

    for mode in ["ANCHOR_NONE", "ANCHOR_PRESERVE"] {
        s.run(&format!(
            r#"TS:SetOwner(O, "{mode}") TS:ClearAllPoints()
            TS:SetPoint("TOPLEFT", 5, -5) TS:AddLine("x") TS:Show()"#
        ))
        .unwrap();
        let (n, p): (i64, String) = s
            .eval("local p = TS:GetPoint(1) return TS:GetNumPoints(), p")
            .unwrap();
        assert_eq!((n, p.as_str()), (1, "TOPLEFT"), "{mode}");
    }
}

/// `SetMinimumWidth` stores any number (`0x530f05`) and raises its usage on anything else
/// (`0x530edc`); `SetPadding` reads `lua_tonumber` untested (`0x530fa0`). Neither sizes: the next
/// show reads them.
#[test]
fn minimum_width_and_padding_read_their_arguments_as_the_reference() {
    let (s, _) = tooltip();
    s.run(r#"TS:AddLine("abcd") TS:Show()"#).unwrap();
    let err = s
        .run(r#"TS:SetMinimumWidth("wide")"#)
        .expect_err("a non-number width raises");
    assert!(
        err.to_string().contains("Usage: SetMinimumWidth(width)"),
        "{err}"
    );
    s.run(r#"TS:SetMinimumWidth("90") TS:SetPadding(nil)"#)
        .unwrap();
    assert!(close(size(&s).0, plate(20.0)), "no sizing yet");
    s.run("TS:Show()").unwrap();
    assert!(close(size(&s).0, plate(90.0)), "{:?}", size(&s));
    s.run("TS:SetMinimumWidth(-5) TS:SetPadding(16) TS:Show()")
        .unwrap();
    let want = f64::from(2.0 * 10.24f32 + 16.0 + 20.0);
    assert!(close(size(&s).0, want), "{:?}", size(&s));
}

/// The hover loops clear, rebuild and show every frame (`ContainerFrameItemButton_OnUpdate`): past
/// the first frame the sizer measures nothing, its wrapped line's three widths answered from the
/// memo, and solves nothing, as the clear's width reset and the sizer's restore leave the line's
/// node where it was.
#[test]
fn a_steady_hover_measures_nothing_after_its_first_frame() {
    let (mut s, calls) = tooltip();
    let long = ["aaaa"; 12].join(" ");
    let frame = format!(
        r#"TS:SetOwner(O, "ANCHOR_RIGHT")
        TS:AddLine("Title") TS:AddLine("{long}", 1, 1, 1, 1) TS:AddDoubleLine("left", "right")
        TS:Show()"#
    );
    s.run(&frame).unwrap();
    s.tick(0.016);
    s.resolve();
    s.run(&frame).unwrap();
    s.tick(0.016);
    s.resolve();
    let first = size(&s);
    let (after_first, solves) = (calls.get(), s.layout_solves());
    assert!(after_first > 0);
    for _ in 0..4 {
        s.run(&frame).unwrap();
        s.tick(0.016);
        s.resolve();
        assert_eq!(size(&s), first);
    }
    assert_eq!(calls.get(), after_first, "a steady hover re-measured");
    assert_eq!(s.layout_solves(), solves, "a steady hover re-solved");
}
