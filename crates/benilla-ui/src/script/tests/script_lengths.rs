//! The script lengths besides the frame size, edges and points: each setter stores `S(v)` and its
//! getter answers `G` of the stored value (benilla-re `systems/ui/methods.md`, "Other methods that
//! convert"). At 4:3, `G(S(100))` is 99.99999850988391 and `G(S(50))` 49.999999254941955.

use super::common::script;
use crate::script::UiScript;

/// `G(S(100))` and friends at a 1024×768 window, from the law's own steps.
const G_100: f64 = 99.99999850988391;
const G_50: f64 = 49.999999254941955;
const G_11: f64 = 11.000000074505804;
const G_12: f64 = 12.00000029802322;
const G_3: f64 = 3.000000074505805;
const G_5: f64 = 4.999999925494195;

fn vm() -> UiScript {
    let mut s = script();
    s.set_window_size(1024.0, 768.0);
    s
}

/// `SetMinResize`/`SetMaxResize` store `S` of both (`0x77610f`, `0x776138`, `0x77638f`,
/// `0x7763b8`) and the getters answer `G` (`0x775fda`, `0x775ffe`, `0x77625a`, `0x77627e`);
/// `SetHitRectInsets` and `GetHitRectInsets` the same on four (`0x775e4d`, `0x775cbc`).
#[test]
fn the_resize_bounds_and_hit_insets_answer_through_the_getter_steps() {
    let s = vm();
    s.run(
        r#"F = CreateFrame("Frame", "Bounds")
           F:SetMinResize(100, 50) F:SetMaxResize(11, 12)
           F:SetHitRectInsets(3, 5, 100, 0)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return F:GetMinResize()").unwrap(),
        (G_100, G_50)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return F:GetMaxResize()").unwrap(),
        (G_11, G_12)
    );
    assert_eq!(
        s.eval::<(f64, f64, f64, f64)>("return F:GetHitRectInsets()")
            .unwrap(),
        (G_3, G_5, G_100, 0.0)
    );
}

/// `SetBackdrop` stores `S` of `tileSize`, `edgeSize` and each inset (`0x777934`, `0x77798d`,
/// `0x777a2d`..`0x777b38`) and `GetBackdrop` answers `G` of each (`0x7774ec`, `0x77752e`,
/// `0x7775ec`..`0x7776a0`). A missing `edgeSize` keeps the constructor's 0.025 layout units
/// (`0x77e65d`), 32 at 4:3 and 39.16 at 16:9.
#[test]
fn the_backdrop_lengths_answer_through_the_getter_steps() {
    let mut s = vm();
    s.run(
        r#"F = CreateFrame("Frame", "Plate")
           F:SetBackdrop({ edgeFile = "e", tileSize = 100, edgeSize = 11,
                           insets = { left = 3, right = 5, top = 12, bottom = 50 } })
           G = CreateFrame("Frame", "Bare") G:SetBackdrop({ edgeFile = "e" })"#,
    )
    .unwrap();
    let read = |s: &UiScript| {
        s.eval::<(f64, f64, f64, f64, f64, f64, f64)>(
            "local b, d = F:GetBackdrop(), G:GetBackdrop() local i = b.insets
             return b.tileSize, b.edgeSize, i.left, i.right, i.top, i.bottom, d.edgeSize",
        )
        .unwrap()
    };
    assert_eq!(read(&s), (G_100, G_11, G_3, G_5, G_12, G_50, 32.0));
    s.set_window_size(1920.0, 1080.0);
    assert_eq!(
        read(&s).6,
        39.16279630224725,
        "the default is a layout length"
    );
    // And it draws at that size: the default plate's corners are 39.16 units square.
    s.run(r#"G:SetPoint("CENTER", 0, 0) G:SetWidth(200) G:SetHeight(200)"#)
        .unwrap();
    s.resolve();
    let corner = s
        .extract()
        .into_iter()
        .filter(|q| matches!(&q.content, crate::script::QuadContent::Texture { path: Some(p), .. } if p == "e"))
        .filter_map(|q| q.rect)
        .map(|r| r.right - r.left)
        .fold(f32::INFINITY, f32::min);
    crate::assert_near!(corner, 39.162796);
}

/// `GetTextWidth`/`GetTextHeight` answer `G` of the label's size slot (`0x782345`, `0x782445`):
/// its `GetWidth`/`GetHeight`, an authored size included, at full precision. `SetPushedTextOffset`
/// stores `S` (`0x7820fd`, `0x782126`) and `GetPushedTextOffset` answers `G` (`0x782249`,
/// `0x78226d`); a script number is stored again by new cells, as their rebuild would store it.
#[test]
fn the_button_lengths_answer_through_the_getter_steps() {
    let mut s = vm();
    s.run(
        r#"B = CreateFrame("Button", "Btn") B:SetText("x")
           local t = B:GetFontString() t:SetWidth(100) t:SetHeight(50)
           B:SetPushedTextOffset(100, 50)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return B:GetTextWidth(), B:GetTextHeight()")
            .unwrap(),
        (G_100, G_50)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return B:GetPushedTextOffset()")
            .unwrap(),
        (G_100, G_50)
    );
    s.set_window_size(1920.0, 1080.0);
    assert_eq!(
        s.eval::<(f64, f64)>("return B:GetPushedTextOffset()")
            .unwrap(),
        (100.00000136774479, 50.00000068387239),
        "G(S(v)) under the new cells, not G of the old store"
    );
}

/// `SetTextInsets` stores `S` of each (`0x798b5b`..`0x798bd9`) and `GetTextInsets` answers `G`
/// of each (`0x798cfc`..`0x798d68`).
#[test]
fn the_edit_box_text_insets_answer_through_the_getter_steps() {
    let s = vm();
    s.run(r#"E = CreateFrame("EditBox", "Insets") E:SetTextInsets(3, 5, 100, 50)"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f64, f64, f64, f64)>("return E:GetTextInsets()")
            .unwrap(),
        (G_3, G_5, G_100, G_50)
    );
}

/// `SetFont`'s height (`0x79d5d3`; the shared `0x79f276`), `SetShadowOffset` (`0x79e150`,
/// `0x79fa28`) and `SetSpacing` (`0x79e36b`, `0x79fb7e`) store `S`; `GetFont` (`0x79d4b7`,
/// `0x79f3e3`), `GetShadowOffset` (`0x79e029`, `0x79faf5`) and `GetSpacing` (`0x79e28e`,
/// `0x79fbfc`) answer `G`, on a FontString and on a font instance alike.
#[test]
fn the_font_lengths_answer_through_the_getter_steps() {
    let s = vm();
    s.run(
        r#"local f = CreateFrame("Frame", "Host")
           T = f:CreateFontString(nil, "ARTWORK")
           E = CreateFrame("EditBox", "Box")
           for _, o in ipairs({ T, E }) do
               o:SetFont("Fonts\\FRIZQT__.TTF", 12)
               o:SetShadowOffset(7, -3)
               o:SetSpacing(11)
           end"#,
    )
    .unwrap();
    for who in ["T", "E"] {
        let (h, x, y, sp): (f64, f64, f64, f64) = s
            .eval(&format!(
                "local _, h = {who}:GetFont() local x, y = {who}:GetShadowOffset() \
                 return h, x, y, {who}:GetSpacing()"
            ))
            .unwrap();
        assert_eq!(
            (h, x, y, sp),
            (
                12.00000029802322,
                6.999999776482586,
                -3.000000074505805,
                G_11
            ),
            "{who}"
        );
    }
}

/// The layout cache's writer takes the getter steps on the layout rect (`0x490fb3`..`0x491130`)
/// before it rounds, so a half the store moved under or over rounds as the reference rounds it.
/// At 4:3 the left edge reads 20.4999992 and the width 12.4999998, which round down; the top,
/// `RN24(K2 + S(−30.5)) − K2`, reads −30.50003 and the height 11.500015, which round away.
#[test]
fn the_layout_cache_rounds_the_getter_steps() {
    let mut s = vm();
    s.run(
        r#"local f = CreateFrame("Frame", "Saved", UIParent)
           f:SetMovable(true) f:SetResizable(true)
           f:SetPoint("TOPLEFT", UIParent, "TOPLEFT", 20.5, -30.5)
           f:SetWidth(12.5) f:SetHeight(11.5)
           f:SetUserPlaced(true)"#,
    )
    .unwrap();
    s.resolve();
    let row = s
        .user_placed_layouts()
        .into_iter()
        .find(|r| r.name == "Saved")
        .expect("a row");
    assert_eq!(
        (row.x, row.y, row.width, row.height),
        (Some(20), Some(-31), Some(12), Some(12))
    );
}

/// `PingLocation` stores both offsets by `S` (`0x4eede6`, `0x4eee11`) before it divides them by
/// the frame's stored height and width (slots 0x20 and 0x1c) and stores each fraction times the
/// diameter as a float (`0x4eee2e`, `0x4eee3d`).
#[test]
fn ping_location_divides_the_stored_offsets() {
    let mut s = vm();
    s.run(r#"m = CreateFrame("Minimap", "Pinged") m:SetWidth(140) m:SetHeight(100)"#)
        .unwrap();
    s.set_minimap_player(Some((100.0, 200.0)));
    let u = crate::layout::LayoutUnits::from_aspect(1024.0 / 768.0);
    let d = 2.0 * f64::from(crate::widget::minimap_radius_yd(3, 3, false));
    let st = |v: f32| f64::from(u.store(v));
    for (x, y) in [(10.0f32, 20.0f32), (37.3, -51.7), (-3.1, 66.6), (12.9, 5.5)] {
        s.run(&format!("m:PingLocation({x}, {y})")).unwrap();
        let sent = s.take_minimap_pings_sent();
        let second = f64::from((st(y) / st(140.0) * d) as f32);
        let first = f64::from((-st(x) / st(100.0) * d) as f32);
        let want = ((100.0 + second) as f32, (200.0 + first) as f32);
        assert_eq!(sent, vec![want], "{x} {y}");
    }
}
