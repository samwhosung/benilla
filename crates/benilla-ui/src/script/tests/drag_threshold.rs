//! The drag-start distance: 0.01 of the screen diagonal (`0x76b320`, `0x76b300`, `0x81c468`).

use super::common::script;
use crate::script::UiScript;

/// A drag-registered frame covering `width`×`height` root units from `(-100, 0)`, under a
/// `UIParent` when `ui_scale` is given, with `OnDragStart` counting into `starts`.
fn drag_script(width: f32, height: f32, ui_scale: Option<f32>) -> UiScript {
    let mut s = script();
    s.set_screen_size(width, height);
    s.run(
        r#"
        starts = 0
        local up = CreateFrame("Frame", "UIParent")
        up:SetPoint("BOTTOMLEFT", 0, 0); up:SetPoint("TOPRIGHT", 0, 0)
        local a = CreateFrame("Frame", "A", up)
        a:SetPoint("BOTTOMLEFT", -100, 0); a:SetWidth(5000); a:SetHeight(5000); a:EnableMouse(true)
        a:RegisterForDrag("LeftButton")
        a:SetScript("OnDragStart", function() starts = starts + 1 end)
        "#,
    )
    .unwrap();
    if let Some(v) = ui_scale {
        s.set_ui_scale(v);
        s.apply_ui_scale((1024, 768), &mut None);
    }
    s.resolve();
    s
}

fn starts(s: &UiScript) -> i64 {
    s.eval::<i64>("return starts").unwrap()
}

/// The layout unit, the screen diagonal in root units.
fn unit(s: &UiScript) -> f32 {
    s.model_ref().layout_unit()
}

/// Press at the origin of a drag frame, move `frac` of the diagonal right, and say whether the
/// drag started.
fn starts_at(s: &mut UiScript, frac: f32) -> bool {
    let d = frac * unit(s);
    s.run("starts = 0").unwrap();
    s.mouse_button(0.0, 300.0, "LeftButton", true);
    s.mouse_move(d, 300.0);
    let started = starts(s) == 1;
    s.mouse_button(d, 300.0, "LeftButton", false);
    started
}

/// 0.0099 of the diagonal is short of the start; 0.0101 is past it.
#[test]
fn a_move_under_one_percent_of_the_diagonal_starts_no_drag() {
    let mut s = drag_script(1024.0, 768.0, None);
    assert!(!starts_at(&mut s, 0.0099), "0.0099 of the diagonal");
    assert!(starts_at(&mut s, 0.0101), "0.0101 of the diagonal");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// At 1024×768 the threshold is 12.8 px, not the old 4.
#[test]
fn at_1024_by_768_a_drag_needs_12_8_px() {
    let mut s = drag_script(1024.0, 768.0, None);
    let u = unit(&s);
    assert!(!starts_at(&mut s, 12.0 / u), "12 px");
    assert!(starts_at(&mut s, 13.0 / u), "13 px");
}

/// The boundary of `0x76b368`: the squared distance in x87 53-bit precision (`WinMain`'s
/// `_controlfp(0x9001f)`, `D3DCREATE_FPU_PRESERVE`) against the f32 `0.01f × 0.01f`, which rounds up
/// past the exact square of `0.01f`. A move of exactly `0.01f` in layout units is short of it and
/// the next f32 above starts the drag (`0x76b372`: only a smaller distance skips).
#[test]
fn the_boundary_is_the_f32_square_of_0_01_compared_as_a_double() {
    let mut s = drag_script(1024.0, 768.0, None);
    let u = unit(&s);
    let at = 0.01_f32;
    let above = f32::from_bits(at.to_bits() + 1);
    // A root-unit x whose layout coordinate (`x / u`) is exactly `want`.
    let root_x = |want: f32| -> f32 {
        let guess = want * u;
        (-16..=16_i32)
            .map(|k| f32::from_bits((guess.to_bits() as i32 + k) as u32))
            .find(|x| x / u == want)
            .expect("a root x that lands on the layout coordinate")
    };
    for (want, expect) in [(at, false), (above, true)] {
        let x = root_x(want);
        s.mouse_button(0.0, 300.0, "LeftButton", true);
        s.mouse_move(x, 300.0);
        assert_eq!(starts(&s) == 1, expect, "a layout move of {want:e}");
        s.mouse_button(x, 300.0, "LeftButton", false);
        s.run("starts = 0").unwrap();
    }
}

/// The threshold follows the window diagonal: 22 px at 1920×1080 (1% of 2203 px), not 12.8.
#[test]
fn the_threshold_scales_with_the_screen_diagonal() {
    let root_w = 1920.0 * 768.0 / 1080.0;
    let mut s = drag_script(root_w, 768.0, None);
    let px = |root: f32| root * 1080.0 / 768.0;
    let thr = 0.01 * unit(&s);
    assert!((px(thr) - 22.03).abs() < 0.01, "{} px", px(thr));
    assert!(!starts_at(&mut s, 0.0099));
    assert!(starts_at(&mut s, 0.0101));
    // 14 px passes at 1024×768 and not here.
    let u = unit(&s);
    assert!(!starts_at(&mut s, 14.0 * 768.0 / 1080.0 / u));
}

/// The UI scale is `UIParent`'s own, never the pointer's: the same fraction of the diagonal starts
/// a drag under any scale, on either screen.
#[test]
fn the_threshold_does_not_follow_the_ui_scale() {
    for (w, h) in [(1024.0, 768.0), (1920.0 * 768.0 / 1080.0, 768.0)] {
        for scale in [0.64, 1.0, 1.15] {
            let mut s = drag_script(w, h, Some(scale));
            assert!(!starts_at(&mut s, 0.0099), "{w}x{h} scale {scale}");
            assert!(starts_at(&mut s, 0.0101), "{w}x{h} scale {scale}");
        }
    }
}
