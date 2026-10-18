//! The script-unit law at the Lua boundary: what the geometry, screen and cursor getters answer
//! for a window, against values derived from the reference's steps (benilla-re `screencoord`,
//! `ui/methods`), each a double the client pushes after its float stores.

use crate::script::UiScript;

/// An in-game VM laid out for a `w` by `h` window.
fn game(w: f32, h: f32) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_window_size(w, h);
    s
}

/// `GetScreenWidth`/`GetScreenHeight` (`0x46dd30`, `0x46dd80`, `0x48b480`, `0x48b4d0`): at 4:3
/// the height is 767.9999427795418, so a `floor` of it is 767; at 1366×768 the width floors to
/// 1365. In game, `UIParent`'s scale divides the extent first.
#[test]
fn the_screen_getters_round_as_the_reference() {
    let read = |s: &UiScript| {
        s.eval::<(f64, f64, i64, i64)>(
            "local w, h = GetScreenWidth(), GetScreenHeight() \
             return w, h, math.floor(w), math.floor(h)",
        )
        .unwrap()
    };
    let mut glue = UiScript::new_glue().unwrap();
    glue.set_window_size(1024.0, 768.0);
    assert_eq!(read(&glue), (1024.0, 767.9999427795418, 1024, 767));
    assert_eq!(
        read(&game(1024.0, 768.0)),
        (1024.0, 767.9999427795418, 1024, 767)
    );
    assert_eq!(
        read(&game(1366.0, 768.0)),
        (1365.9999734688627, 767.9999649898396, 1365, 767)
    );
    assert_eq!(
        read(&game(1680.0, 1050.0)),
        (1228.8000575804501, 768.000053981672, 1228, 768)
    );
    let s = game(1920.0, 1080.0);
    s.run(r#"CreateFrame("Frame", "UIParent"):SetScale(0.5)"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return GetScreenWidth(), GetScreenHeight()")
            .unwrap(),
        (2730.666853409422, 1535.9999649857334)
    );
}

/// `GetCursorPosition` (`0x46dad0`, `0x48b820`): the cursor's fractions of the screen scaled to
/// layout units as floats, then the getter steps, both axes by `K1`, so the 4:3 centre's y is
/// under 384, in glue and in game alike.
#[test]
fn the_cursor_rounds_as_the_reference() {
    let mut glue = UiScript::new_glue().unwrap();
    glue.set_window_size(1024.0, 768.0);
    let mut s = game(1024.0, 768.0);
    for s in [&mut glue, &mut s] {
        s.mouse_move(512.0, 384.0);
        assert_eq!(
            s.eval::<(f64, f64)>("return GetCursorPosition()").unwrap(),
            (512.0, 383.9999713897709)
        );
    }
}

/// `SetWidth` stores `S(v)` and `GetWidth` answers `G` of it (`0x7a1fcb`..`0x7a1feb`,
/// `0x7a1eef`..`0x7a1f0e`): at 4:3 a width of 100 is 0.078125 layout units and reads back as
/// 99.99999850988391; at 16:9 as 100.00000136774479. `GetPoint` converts the same way.
#[test]
fn a_set_size_and_offset_read_back_through_the_cells() {
    let mut s = game(1024.0, 768.0);
    s.run(
        r#"local f = CreateFrame("Frame", "F") f:SetWidth(100) f:SetHeight(100)
           f:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, 0)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return F:GetWidth(), F:GetHeight()")
            .unwrap(),
        (99.99999850988391, 99.99999850988391),
        "SetHeight converts by K1 as SetWidth does"
    );
    s.resolve();
    let stored = {
        let m = s.model_ref();
        let h = m.arena.lookup("F").unwrap();
        m.frame_rects[&h].width()
    };
    assert_eq!(stored, 0.078125, "the rect spans S(100) layout units");
    let s = game(1920.0, 1080.0);
    s.run(r#"CreateFrame("Frame", "F"):SetWidth(100)"#).unwrap();
    assert_eq!(
        s.eval::<f64>("return F:GetWidth()").unwrap(),
        100.00000136774479
    );
    let s = game(1366.0, 768.0);
    s.run(r#"CreateFrame("Frame", "F"):SetPoint("CENTER", nil, "CENTER", 0.5, 0.5)"#)
        .unwrap();
    let (.., x, y) = s
        .eval::<(String, mlua::Value, String, f64, f64)>("return F:GetPoint()")
        .unwrap();
    assert_eq!((x, y), (0.5000000341896098, 0.5000000341896098));
}

/// A frame filling the screen answers what the screen getters answer: the span, the right edge and
/// the centre's x through `K1`, the top edge 767.9999427795418 at 4:3. Under a 0.8 `UIParent` at
/// 16:9 the right edge is 1706.6666433238222 while `GetScreenWidth` is 1706.6667833808888, as in
/// 1.12.1, where the screen getter stores `K1/u` as a float and the edge getter does not; the
/// top agrees with `GetScreenHeight` and the span is not divided by the scale.
#[test]
fn a_full_screen_frame_agrees_with_the_screen_getters() {
    let s = game(1024.0, 768.0);
    s.run(r#"CreateFrame("Frame", "F"):SetAllPoints()"#)
        .unwrap();
    let got = s
        .eval::<(f64, f64, f64, f64, f64, f64, f64)>(
            "local cx, cy = F:GetCenter() \
             return F:GetWidth(), F:GetRight(), F:GetTop(), cx, cy, \
             GetScreenWidth(), GetScreenHeight()",
        )
        .unwrap();
    assert_eq!(
        got,
        (
            1024.0,
            1024.0,
            767.9999427795418,
            512.0,
            383.9999713897709,
            1024.0,
            767.9999427795418
        )
    );
    let s = game(1920.0, 1080.0);
    s.run(
        r#"local p = CreateFrame("Frame", "UIParent") p:SetAllPoints() p:SetScale(0.8)
           CreateFrame("Frame", "C", p):SetAllPoints(p)"#,
    )
    .unwrap();
    let got = s
        .eval::<(f64, f64, f64, f64, f64)>(
            "return C:GetRight(), GetScreenWidth(), C:GetTop(), GetScreenHeight(), C:GetWidth()",
        )
        .unwrap();
    assert_eq!(
        got,
        (
            1706.6666433238222,
            1706.6667833808888,
            960.00001313035,
            960.00001313035,
            1365.333426704711
        )
    );
}

/// An anchored chain at a scale, against the reference's resolver steps (each size times the
/// scale, each neighbouring and final edge stored as a float, `scale·offset + edge` at 53 bits)
/// and its getters (`E`, `C`, the span's `G`): at 16:9, `A` at scale 0.9 hangs off the screen's
/// top left, `B` spans from `A`'s bottom left to its bottom right.
#[test]
fn an_anchored_frames_edges_answer_the_law() {
    let s = game(1920.0, 1080.0);
    s.run(
        r#"local a = CreateFrame("Frame", "A") a:SetScale(0.9) a:SetWidth(300) a:SetHeight(200)
           a:SetPoint("TOPLEFT", nil, "TOPLEFT", 100.5, -37.25)
           local b = CreateFrame("Frame", "B", a)
           b:SetPoint("TOPRIGHT", a, "BOTTOMRIGHT", -12.3, 4.7)
           b:SetPoint("BOTTOMLEFT", a, "BOTTOMLEFT", 7, -40)"#,
    )
    .unwrap();
    let a = s
        .eval::<(f64, f64, f64)>("return A:GetWidth(), A:GetTop(), A:GetLeft()")
        .unwrap();
    assert_eq!(
        a,
        (299.9999953496677, 816.0833941352881, 100.50000509484934)
    );
    let b = s
        .eval::<(f64, f64, f64, f64)>("return B:GetLeft(), B:GetRight(), B:GetTop(), B:GetBottom()")
        .unwrap();
    assert_eq!(
        b,
        (
            107.50000475291314,
            388.20002255411157,
            620.7833388989148,
            576.0833908527006
        )
    );
    let b = s
        .eval::<(f64, f64, f64, f64)>(
            "local x, y = B:GetCenter() return B:GetWidth(), B:GetHeight(), x, y",
        )
        .unwrap();
    assert_eq!(
        b,
        (
            252.62999938930196,
            40.22999175728603,
            247.85000927672903,
            598.4333823829411
        )
    );
}

/// The edge getters divide the stored edge by the scale before the getter steps (`0x7a1a63`): a
/// frame at scale 0.9 offset 100 from the left reads 99.99999850988391 at 4:3.
#[test]
fn a_scaled_frames_edge_divides_out_its_scale() {
    let s = game(1024.0, 768.0);
    s.run(
        r#"local f = CreateFrame("Frame", "F") f:SetScale(0.9) f:SetWidth(10) f:SetHeight(10)
           f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 0)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<f64>("return F:GetLeft()").unwrap(),
        99.99999850988391
    );
}

/// A live resize sets the cells as a rebuild would and lays out afresh under them, so every getter
/// answers what a VM built at the new size answers, bit for bit; the reference reaches the new
/// cells only through that rebuild (`0x41ad10` from `0x46a828` and `0x48fe2b`).
#[test]
fn a_resize_lays_out_as_a_build_at_the_new_size() {
    let chain = r#"local a = CreateFrame("Frame", "A") a:SetScale(0.9) a:SetWidth(300) a:SetHeight(200)
        a:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 2.5, 3.25)
        local b = CreateFrame("Frame", "B", a)
        b:SetPoint("TOPRIGHT", a, "BOTTOMRIGHT", -12.3, 4.7)
        b:SetPoint("BOTTOMLEFT", a, "BOTTOMLEFT", 7, -40)
        local c = CreateFrame("Frame", "C") c:SetWidth(2) c:SetHeight(2)
        c:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 1, 1)"#;
    let read = |s: &UiScript| {
        s.eval::<(f64, f64, f64, f64, f64, f64, f64, f64)>(
            "local x, y = B:GetCenter() \
             return A:GetWidth(), A:GetLeft(), B:GetLeft(), B:GetBottom(), B:GetWidth(), x, y, \
             C:GetRight()",
        )
        .unwrap()
    };
    // A small resize moves `C`, by the origin, by less than the drain's keep threshold (`0x768d20`,
    // 1e-5 layout units), which a rebuild has no old rect to hold against.
    for (w, h) in [(1920.0, 1080.0), (1025.0, 768.0)] {
        let mut resized = game(1024.0, 768.0);
        resized.run(chain).unwrap();
        resized.resolve();
        let before = read(&resized);
        resized.set_window_size(w, h);
        resized.resolve();
        let built = game(w, h);
        built.run(chain).unwrap();
        assert_eq!(read(&resized), read(&built), "{w}x{h}");
        assert_ne!(read(&resized), before, "the cells moved");
    }
}
