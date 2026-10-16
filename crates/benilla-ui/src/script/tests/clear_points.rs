//! `ClearAllPoints` (`0x7a2b70` → `0x767620`) only unhooks each anchor from its target
//! (`0x767fa0`) and frees it: no resolve is queued (`0x7680e0`) and no flag is written, so the
//! cached rect and its valid bit stand until the object's own next invalidate. That resolve
//! (`0x768d20`) then fails at `0x768d55`: the valid bit clears and the rect-changed slot is
//! skipped, so nothing anchored to it re-resolves. A Texture keeps the quad its last good resolve
//! wrote (`0x770670` → `0x7705b0`), as its emit checks only its texture (`0x7706e0`); a FontString's
//! emit needs the valid bit (`0x7730b7`), and so do the getters (`GetRect 0x768320`).

use super::common::script;
use crate::layout::Rect;
use crate::script::{UiScript, ZTarget};

/// `Host` at `(100, 100)`–`(300, 200)` with `Child`, a frame pinned to its top right, and `Tex`, a
/// solid texture on its bottom left, 20 square.
fn host() -> UiScript {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        Host = CreateFrame("Frame", "Host")
        Host:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        Host:SetWidth(200); Host:SetHeight(100)
        Child = CreateFrame("Frame", "Child")
        Child:SetPoint("BOTTOMLEFT", Host, "TOPRIGHT", 0, 0)
        Child:SetWidth(10); Child:SetHeight(10)
        Tex = Host:CreateTexture("Tex", "ARTWORK")
        Tex:SetTexture(1, 1, 1)
        Tex:SetPoint("BOTTOMLEFT", Host, "BOTTOMLEFT", 10, 10)
        Tex:SetWidth(20); Tex:SetHeight(20)
    "#,
    )
    .unwrap();
    s.resolve();
    s
}

fn left(s: &mut UiScript, name: &str) -> Option<f64> {
    s.eval::<Option<f64>>(&format!("return {name}:GetLeft()"))
        .unwrap()
}

/// The rect the render list draws region `name` at, if it draws.
fn drawn(s: &UiScript, name: &str) -> Option<Rect> {
    let rh = {
        let m = s.model_ref();
        let id = *m.region_by_name.get(name).expect("a named region");
        *m.id_to_region.get(&id).expect("a live region")
    };
    s.extract()
        .into_iter()
        .find(|q| q.target == ZTarget::Region(rh))
        .and_then(|q| q.rect)
}

/// The getter answers the old edge, before and after the next resolve: the clear queued nothing.
#[test]
fn a_cleared_frame_answers_its_old_left() {
    let mut s = host();
    s.run("Host:ClearAllPoints()").unwrap();
    assert_eq!(left(&mut s, "Host"), Some(100.0), "the cached rect stands");
    s.resolve();
    assert_eq!(left(&mut s, "Host"), Some(100.0), "and outlasts a resolve");
    assert_eq!(s.eval::<i64>("return Host:GetNumPoints()").unwrap(), 0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A cleared texture not re-anchored keeps drawing in place and keeps its edges.
#[test]
fn a_cleared_texture_still_draws_and_keeps_its_left() {
    let mut s = host();
    let before = drawn(&s, "Tex").expect("drawn before the clear");
    s.run("Tex:ClearAllPoints()").unwrap();
    s.resolve();
    assert_eq!(drawn(&s, "Tex"), Some(before), "the same quad");
    assert_eq!(left(&mut s, "Tex"), Some(110.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// What anchors to a cleared frame keeps its rect, and the cleared frame no longer follows the
/// frame it was anchored to: `0x767fa0` cut that edge.
#[test]
fn a_child_of_a_cleared_frame_keeps_its_rect() {
    let mut s = host();
    s.run(
        r#"
        Outer = CreateFrame("Frame", "Outer")
        Outer:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        Outer:SetWidth(50); Outer:SetHeight(50)
        Host:SetPoint("BOTTOMLEFT", Outer, "TOPRIGHT", 50, 50)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Host"), Some(100.0));
    s.run("Host:ClearAllPoints()").unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Child"), Some(300.0), "the child stands");
    assert_eq!(left(&mut s, "Tex"), Some(110.0), "and so does the region");
    s.run(r#"Outer:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 40, 0)"#)
        .unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Outer"), Some(40.0));
    assert_eq!(left(&mut s, "Host"), Some(100.0), "the old target's move");
    assert_eq!(left(&mut s, "Child"), Some(300.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The cleared frame's own next invalidate resolves it, and the resolve fails: its getters
/// answer nil, while everything anchored to it keeps its rect, since a failed resolve skips the
/// rect-changed slot (`0x768e0e`).
#[test]
fn a_cleared_frame_fails_on_its_own_invalidate_and_its_dependents_stand() {
    let mut s = host();
    let tex = drawn(&s, "Tex").expect("drawn");
    s.run("Host:ClearAllPoints(); Host:SetWidth(100)").unwrap();
    assert_eq!(left(&mut s, "Host"), None, "the failed resolve");
    s.resolve();
    assert_eq!(left(&mut s, "Host"), None);
    assert_eq!(left(&mut s, "Child"), Some(300.0), "the child stands");
    assert_eq!(left(&mut s, "Tex"), Some(110.0), "the region stands");
    assert_eq!(drawn(&s, "Tex"), Some(tex), "and draws");
    // A re-anchor resolves it again, and its rect change reaches what anchors to it.
    s.run(r#"Host:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)"#)
        .unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Host"), Some(0.0));
    assert_eq!(left(&mut s, "Child"), Some(100.0), "the child follows");
    assert_eq!(left(&mut s, "Tex"), Some(10.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetWidth` invalidates whether or not the width moved (`0x7a1feb` → `0x7683d0` → `0x7680e0`).
#[test]
fn a_same_width_still_invalidates_a_cleared_frame() {
    let mut s = host();
    s.run("Host:ClearAllPoints()").unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Host"), Some(100.0));
    s.run("Host:SetWidth(200)").unwrap();
    assert_eq!(left(&mut s, "Host"), None);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A clear behind an invalidate the resolve has not taken yet leaves that resolve to fail: the
/// frame is still queued (`0x7680e0`), and the clear does not dequeue it.
#[test]
fn a_clear_behind_a_queued_resolve_fails_it() {
    let mut s = host();
    s.run(
        r#"
        Host:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 120, 100)
        Host:ClearAllPoints()
    "#,
    )
    .unwrap();
    assert_eq!(left(&mut s, "Host"), None);
    assert_eq!(left(&mut s, "Child"), Some(300.0), "nothing moved under it");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A region's own invalidate after a clear fails it at once: its getter resolves first, as a
/// frame's does (`0x7a1980` serves both).
#[test]
fn a_cleared_texture_fails_on_its_own_invalidate() {
    let mut s = host();
    let tex = drawn(&s, "Tex").expect("drawn");
    s.run("Tex:ClearAllPoints(); Tex:SetWidth(30)").unwrap();
    assert_eq!(left(&mut s, "Tex"), None, "the getter resolves");
    s.resolve();
    assert_eq!(left(&mut s, "Tex"), None);
    assert_eq!(drawn(&s, "Tex"), Some(tex), "the last good quad");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A texture re-anchored onto a frame that never resolved fails, and keeps drawing the quad its
/// last good resolve wrote; a FontString in the same spot stops drawing. Both getters answer nil.
#[test]
fn a_failed_texture_keeps_its_quad_and_a_failed_fontstring_does_not() {
    let mut s = host();
    s.run(
        r#"
        Label = Host:CreateFontString("Label", "ARTWORK"); Label:SetFont("Fonts\\FRIZQT__.TTF", 12)
        Label:SetText("x")
        Label:SetPoint("BOTTOMLEFT", Host, "BOTTOMLEFT", 10, 10)
        Label:SetWidth(20); Label:SetHeight(20)
    "#,
    )
    .unwrap();
    s.resolve();
    let tex = drawn(&s, "Tex").expect("the texture draws");
    assert!(drawn(&s, "Label").is_some(), "the label draws");
    s.run(
        r#"
        Ghost = CreateFrame("Frame", "Ghost")
        Tex:SetPoint("BOTTOMLEFT", Ghost, "BOTTOMLEFT", 0, 0)
        Label:SetPoint("BOTTOMLEFT", Ghost, "BOTTOMLEFT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Tex"), None);
    assert_eq!(left(&mut s, "Label"), None);
    assert_eq!(drawn(&s, "Tex"), Some(tex), "the texture's last good quad");
    assert_eq!(drawn(&s, "Label"), None, "the label stops");
    // Anchored again where it resolves, it draws there.
    s.run(r#"Tex:SetPoint("BOTTOMLEFT", Host, "BOTTOMLEFT", 30, 30)"#)
        .unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "Tex"), Some(130.0));
    assert_eq!(drawn(&s, "Tex").map(|r| r.left), Some(130.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `T` spans from `FS`'s top left to the bottom right of `X`; `X` is then cleared and invalidated,
/// so it fails, and `T`, not reached, keeps its rect. `FS` is sized, so its text never moves it,
/// and `T`'s art has no extent here, so it needs both anchors.
fn stale_reader_of_a_label() -> UiScript {
    let mut s = host();
    s.run(
        r#"
        FS = Host:CreateFontString("FS", "ARTWORK"); FS:SetFont("Fonts\\FRIZQT__.TTF", 12)
        FS:SetText("AAAA")
        FS:SetPoint("BOTTOMLEFT", Host, "BOTTOMLEFT", 10, 10)
        FS:SetWidth(40); FS:SetHeight(20)
        X = CreateFrame("Frame", "X")
        X:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 400, 300)
        X:SetWidth(20); X:SetHeight(20)
        T = Host:CreateTexture("T", "ARTWORK")
        T:SetTexture("Interface\Buttons\WHITE8X8")
        T:SetPoint("TOPLEFT", FS, "TOPLEFT", 0, 0)
        T:SetPoint("BOTTOMRIGHT", X, "BOTTOMRIGHT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "T"), Some(110.0));
    s.run("X:ClearAllPoints(); X:SetWidth(10)").unwrap();
    s.resolve();
    assert_eq!(left(&mut s, "X"), None);
    assert_eq!(left(&mut s, "T"), Some(110.0), "not reached");
    s
}

/// A FontString's new string, font or spacing clears its valid bit before it invalidates
/// (`0x771ea4`, `0x7715e0`, `0x771666`), so its next resolve stores whole and its rect-changed
/// slot reaches `T`, which fails on `X` and keeps only its quad. The same value reaches nothing.
#[test]
fn a_labels_new_text_font_or_spacing_reaches_what_anchors_to_it() {
    let mut wrong = Vec::new();
    for (write, reaches) in [
        (r#"FS:SetText("BBBB")"#, true),
        (r#"FS:SetText("AAAA")"#, false),
        (r#"FS:SetFont("Fonts\\ARIALN.TTF", 14)"#, true),
        ("FS:SetSpacing(2)", true),
        ("FS:SetSpacing(0)", false),
    ] {
        let mut s = stale_reader_of_a_label();
        let quad = drawn(&s, "T").expect("drawn");
        s.run(write).unwrap();
        s.resolve();
        let want = if reaches { None } else { Some(110.0) };
        let got = left(&mut s, "T");
        if got != want {
            wrong.push(format!("{write}: T:GetLeft() {got:?}, want {want:?}"));
        }
        assert_eq!(
            left(&mut s, "FS"),
            Some(110.0),
            "{write}: the label resolves"
        );
        assert_eq!(drawn(&s, "T"), Some(quad), "{write}: the quad stays");
        assert!(s.errors().is_empty(), "{write}: {:?}", s.errors());
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
