//! `SetTexture` (`0x770200`, the colour form `0x770360`) writes the texture slot and marks the
//! draw batch dirty (`0x77fdf0`); it never invalidates the layout (`0x7680e0`). A texture's axis
//! sized 0 takes its texel extent only when the texture resolves: the resolver reads the size
//! through the geometry vtable (`0x767579`, `0x7675e9`), which a Texture answers from its art
//! (`0x770720`, loading it first if it must, `0x44b3b0`). New art moves nothing until the
//! texture's own next invalidate, or a target's new rect.

use super::common::script;
use crate::layout::Rect;
use crate::script::{UiScript, ZTarget};

/// Resolve, then require the cached graph and its rects to equal a from-scratch solve.
fn resolve_matches_scratch(s: &mut UiScript, what: &str) {
    s.resolve();
    let mut model = s.model_mut();
    if let Some(why) = UiScript::scratch_divergence(&mut model) {
        panic!("{what}: {why}");
    }
}

fn num(s: &mut UiScript, expr: &str) -> Option<f64> {
    s.eval::<Option<f64>>(&format!("return {expr}")).unwrap()
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

/// `Owner` at `(100, 100)`–`(300, 200)`, with art `Art` 64×32 and `Big` 128×64.
fn owner() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_texture_size_probe(Box::new(|p| match p {
        "Art" => Some((64, 32)),
        "Big" => Some((128, 64)),
        _ => None,
    }));
    s.run(
        r#"
        f = CreateFrame("Frame", "Owner")
        f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        f:SetWidth(200); f:SetHeight(100)
    "#,
    )
    .unwrap();
    s
}

/// One anchor and no size: with no art the resolve fails for want of a span. Art set after
/// leaves it failed, the path form and the colour form alike, until its own invalidate.
#[test]
fn a_failed_texture_stays_failed_when_its_art_arrives() {
    let mut s = owner();
    s.run(
        r#"
        T = f:CreateTexture("T", "ARTWORK"); T:SetPoint("TOPLEFT", f, "TOPLEFT", 0, 0)
        C = f:CreateTexture("C", "ARTWORK"); C:SetPoint("TOPLEFT", f, "TOPLEFT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(num(&mut s, "T:GetLeft()"), None, "no art, no span");
    assert_eq!(num(&mut s, "C:GetLeft()"), None);

    s.run(r#"T:SetTexture("Art"); C:SetTexture(1, 1, 1)"#)
        .unwrap();
    resolve_matches_scratch(&mut s, "art on failed textures");
    assert_eq!(
        num(&mut s, "T:GetLeft()"),
        None,
        "SetTexture queued no resolve"
    );
    assert_eq!(num(&mut s, "C:GetLeft()"), None);
    assert_eq!(drawn(&s, "T"), None, "and it draws nothing");
    assert_eq!(drawn(&s, "C"), None);
    // The size getter reads the art live (`0x770720`), resolved or not.
    assert_eq!(num(&mut s, "T:GetWidth()"), Some(64.0));

    s.run(
        r#"T:SetPoint("TOPLEFT", f, "TOPLEFT", 1, 0); C:SetPoint("TOPLEFT", f, "TOPLEFT", 2, 0)"#,
    )
    .unwrap();
    resolve_matches_scratch(&mut s, "their own invalidates");
    assert_eq!(num(&mut s, "T:GetRight()"), Some(165.0), "the art's 64");
    assert_eq!(num(&mut s, "T:GetBottom()"), Some(168.0), "and its 32");
    assert_eq!(num(&mut s, "C:GetRight()"), Some(110.0), "the solid's 8");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A texture resolved at its art's size keeps that rect when the art changes, and takes the new
/// art's size when a target stores a new rect (`0x768cf0`).
#[test]
fn a_resolved_texture_keeps_its_rect_when_its_art_changes() {
    let mut s = owner();
    s.run(
        r#"T = f:CreateTexture("T", "ARTWORK"); T:SetTexture("Art")
        T:SetPoint("TOPLEFT", f, "TOPLEFT", 0, 0)"#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(num(&mut s, "T:GetRight()"), Some(164.0));
    let before = drawn(&s, "T").expect("drawn");

    s.run(r#"T:SetTexture("Big")"#).unwrap();
    resolve_matches_scratch(&mut s, "new art");
    assert_eq!(num(&mut s, "T:GetRight()"), Some(164.0), "the rect stands");
    assert_eq!(drawn(&s, "T"), Some(before), "and draws there");
    assert_eq!(
        num(&mut s, "T:GetWidth()"),
        Some(128.0),
        "the getter reads the art"
    );

    // The owner's new rect reaches the texture, whose resolve reads the art it holds now.
    s.run("f:SetHeight(120)").unwrap();
    resolve_matches_scratch(&mut s, "the target moved");
    assert_eq!(
        num(&mut s, "T:GetRight()"),
        Some(228.0),
        "the new art's 128"
    );
    assert_eq!(num(&mut s, "T:GetTop()"), Some(220.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A write the reference invalidates for, though it moves no input, re-resolves a texture whose
/// art changed since: `SetWidth` always invalidates (`0x7683d0`), so the new art's span lands.
#[test]
fn a_same_value_invalidate_after_new_art_takes_the_art() {
    let mut s = owner();
    s.run(
        r#"T = f:CreateTexture("T", "ARTWORK"); T:SetTexture("Art")
        T:SetPoint("TOPLEFT", f, "TOPLEFT", 0, 0); T:SetWidth(0)"#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(num(&mut s, "T:GetRight()"), Some(164.0));
    s.run(r#"T:SetTexture("Big")"#).unwrap();
    resolve_matches_scratch(&mut s, "new art");
    assert_eq!(
        num(&mut s, "T:GetRight()"),
        Some(164.0),
        "nothing reached it"
    );
    s.run("T:SetWidth(0)").unwrap();
    resolve_matches_scratch(&mut s, "the same width");
    assert_eq!(
        num(&mut s, "T:GetRight()"),
        Some(228.0),
        "the invalidate read the new art"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
