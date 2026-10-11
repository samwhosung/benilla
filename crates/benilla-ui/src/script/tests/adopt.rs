//! A texture object handed to a widget's texture setter moves into that widget: unlinked from its
//! old parent (running the parent's region-removed hook), linked into the new one on the setter's
//! own draw layer (`0x77fd10`).

use super::common::script;
use crate::script::*;

fn drawn(s: &UiScript) -> Vec<String> {
    s.extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// `0x778fd0` → `0x77fd10(button, 2, 0)`: the adopted texture answers the button for its parent,
/// sits on ARTWORK whatever layer it was made on, and draws and hides with the button.
#[test]
fn a_state_slot_adopts_the_texture_into_the_button_on_artwork() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        a = CreateFrame("Frame", "AdoptA")
        a:SetPoint("CENTER", 0, 0) a:SetWidth(50) a:SetHeight(50)
        a:SetFrameStrata("LOW")
        t = a:CreateTexture(nil, "OVERLAY")
        t:SetTexture("adopted") t:SetAllPoints(a)
        b = CreateFrame("Button", "AdoptB")
        b:SetPoint("CENTER", 0, 0) b:SetWidth(50) b:SetHeight(50)
        b:SetFrameStrata("DIALOG")
        c = CreateFrame("Frame", "AdoptC")
        c:SetPoint("CENTER", 0, 0) c:SetWidth(50) c:SetHeight(50)
        c:SetFrameStrata("HIGH")
        ct = c:CreateTexture(nil, "BACKGROUND")
        ct:SetTexture("mid") ct:SetAllPoints(c)
        assert(t:GetParent() == a)
        b:SetNormalTexture(t)
    "#,
    )
    .unwrap();
    s.run(
        r#"
        assert(t:GetParent() == b, "the button is the parent")
        assert(t:GetDrawLayer() == "ARTWORK", "layer 2: " .. t:GetDrawLayer())
        assert(b:GetNormalTexture() == t)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        drawn(&s),
        vec!["mid".to_string(), "adopted".to_string()],
        "it draws at the button's strata, over the HIGH frame's texture, not at LOW"
    );

    s.run("b:Hide()").unwrap();
    s.resolve();
    assert_eq!(
        drawn(&s),
        vec!["mid".to_string()],
        "it hides with the button, not with its old frame"
    );
}

/// The pushed and disabled slots link the same way (`0x778fd0` is one function over the slot index).
#[test]
fn the_pushed_and_disabled_slots_adopt_on_artwork_too() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        b = CreateFrame("Button")
        p = a:CreateTexture(nil, "BORDER")
        d = a:CreateTexture(nil, "OVERLAY")
        b:SetPushedTexture(p) b:SetDisabledTexture(d)
        assert(p:GetParent() == b and d:GetParent() == b)
        assert(p:GetDrawLayer() == "ARTWORK" and d:GetDrawLayer() == "ARTWORK")
    "#,
    )
    .unwrap();
}

/// `0x779110` → `0x77fd10(button, 4, 1)`: the highlight links on HIGHLIGHT.
#[test]
fn the_highlight_slot_adopts_on_highlight() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        b = CreateFrame("Button")
        t = a:CreateTexture(nil, "ARTWORK")
        b:SetHighlightTexture(t)
        assert(t:GetParent() == b, "reparented")
        assert(t:GetDrawLayer() == "HIGHLIGHT", t:GetDrawLayer())
    "#,
    )
    .unwrap();
}

/// `0x785330` / `0x785440` → `0x77fd10(button, 3, 0)`: the checked pair links on OVERLAY.
#[test]
fn the_checked_pair_adopts_on_overlay() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        b = CreateFrame("CheckButton")
        c = a:CreateTexture(nil, "BACKGROUND")
        dc = a:CreateTexture(nil, "ARTWORK")
        b:SetCheckedTexture(c) b:SetDisabledCheckedTexture(dc)
        assert(c:GetParent() == b and dc:GetParent() == b)
        assert(c:GetDrawLayer() == "OVERLAY" and dc:GetDrawLayer() == "OVERLAY")
    "#,
    )
    .unwrap();
}

/// Leaving a parent runs its region-removed hook (`0x77fd55`): the button that gave the texture up
/// no longer holds it.
#[test]
fn the_old_parent_lets_go_of_the_texture_it_gave_up() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Button", "GiverBtn")
        a:SetNormalTexture("own")
        t = a:GetNormalTexture()
        b = CreateFrame("Button", "TakerBtn")
        b:SetNormalTexture(t)
        assert(a:GetNormalTexture() == nil, "the giver's slot let go")
        assert(b:GetNormalTexture() == t and t:GetParent() == b)
        a:SetNormalTexture("again")
        assert(a:GetNormalTexture() ~= t and b:GetNormalTexture() == t, "and rebuilds its own")
    "#,
    )
    .unwrap();
}

/// A backdrop deletes its nine pieces by the pointers it keeps (`0x77f4d0`), wherever they have
/// moved to; the piece dies with its current parent's hook (`0x77fd10` links it there), which
/// empties the button's slot, so the slot never dangles and takes a fresh texture.
#[test]
fn an_old_frames_backdrop_teardown_leaves_the_new_parents_slot_empty_not_dangling() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame")
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        piece = ({ f:GetRegions() })[1]
        b = CreateFrame("Button")
        b:SetPushedTexture(piece)
        assert(piece:GetParent() == b and b:GetPushedTexture() == piece)
        f:SetBackdrop(nil)
        assert(b:GetPushedTexture() == nil, "the slot let go of the deleted piece")
        assert(not pcall(piece.GetTexture, piece), "the piece is gone")
        b:SetPushedTexture("fresh")
        assert(b:GetPushedTexture():GetTexture() == "fresh")
    "#,
    )
    .unwrap();
}

/// The slot compare comes first (`0x778fe5`): the same texture again changes nothing, not its
/// layer and not its life.
#[test]
fn adopting_the_same_texture_twice_is_a_no_op() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        b = CreateFrame("Button")
        t = a:CreateTexture(nil, "OVERLAY")
        t:SetTexture("keep")
        b:SetNormalTexture(t)
        t:SetDrawLayer("BORDER")
        b:SetNormalTexture(t)
        assert(b:GetNormalTexture() == t and t:GetTexture() == "keep", "still held, still alive")
        assert(t:GetDrawLayer() == "BORDER", "the second call did not re-layer it")
    "#,
    )
    .unwrap();
}

/// `0x7831f0`: the bar texture is the object, linked on the layer argument (ARTWORK without one),
/// and the old bar is deleted; nil clears the bar (`0x78fa6d`).
#[test]
fn a_status_bar_takes_a_texture_object_and_a_nil() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        sb = CreateFrame("StatusBar", "AdoptSB")
        sb:SetStatusBarTexture("old")
        old = sb:GetStatusBarTexture()
        t = a:CreateTexture(nil, "OVERLAY")
        sb:SetStatusBarTexture(t)
        assert(sb:GetStatusBarTexture() == t, "installed")
        assert(t:GetParent() == sb, "reparented")
        assert(t:GetDrawLayer() == "ARTWORK", "default layer 2")
        assert(not pcall(old.GetTexture, old), "the old bar texture was deleted")
        u = a:CreateTexture(nil, "BACKGROUND")
        sb:SetStatusBarTexture(u, "BORDER")
        assert(u:GetParent() == sb and u:GetDrawLayer() == "BORDER", "the layer argument")
        assert(not pcall(t.GetTexture, t), "and t, displaced, was deleted")
        sb:SetStatusBarTexture(nil)
        assert(sb:GetStatusBarTexture() == nil, "nil clears")
        assert(not pcall(u.GetTexture, u))
    "#,
    )
    .unwrap();
}

/// `0x789860`: the thumb links on OVERLAY (the layer argument is constant 3), its anchors are
/// cleared (`0x767ed0`) since the slider places it, and the old thumb is deleted.
#[test]
fn a_slider_thumb_object_links_on_overlay_with_its_anchors_cleared() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        sl = CreateFrame("Slider", "AdoptSl")
        sl:SetThumbTexture("old")
        old = sl:GetThumbTexture()
        t = a:CreateTexture(nil, "BACKGROUND")
        t:SetAllPoints(a)
        assert(t:GetNumPoints() > 0)
        sl:SetThumbTexture(t)
        assert(sl:GetThumbTexture() == t and t:GetParent() == sl)
        assert(t:GetDrawLayer() == "OVERLAY", t:GetDrawLayer())
        assert(t:GetNumPoints() == 0, "anchors cleared")
        assert(not pcall(old.GetTexture, old), "old thumb deleted")
        sl:SetThumbTexture(nil)
        assert(sl:GetThumbTexture() == nil and not pcall(t.GetTexture, t))
    "#,
    )
    .unwrap();
}

/// `0x78b580` and `0x78b8a0` link the wheel and the strip on ARTWORK (layer 2); `0x78b850` and
/// `0x78ba90` link the two markers on OVERLAY and clear their anchors.
#[test]
fn a_color_select_adopts_its_four_textures_on_the_reference_layers() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        cs = CreateFrame("ColorSelect", "AdoptCS")
        local function mk(layer)
            local t = a:CreateTexture(nil, layer)
            t:SetAllPoints(a)
            return t
        end
        w, wt, v, vt = mk("OVERLAY"), mk("BACKGROUND"), mk("OVERLAY"), mk("BACKGROUND")
        cs:SetColorWheelTexture(w)
        cs:SetColorWheelThumbTexture(wt)
        cs:SetColorValueTexture(v)
        cs:SetColorValueThumbTexture(vt)
        for _, t in ipairs({ w, wt, v, vt }) do assert(t:GetParent() == cs) end
        assert(cs:GetColorWheelTexture() == w and cs:GetColorValueTexture() == v)
        assert(cs:GetColorWheelThumbTexture() == wt and cs:GetColorValueThumbTexture() == vt)
        assert(w:GetDrawLayer() == "ARTWORK" and v:GetDrawLayer() == "ARTWORK")
        assert(wt:GetDrawLayer() == "OVERLAY" and vt:GetDrawLayer() == "OVERLAY")
        assert(w:GetNumPoints() > 0 and v:GetNumPoints() > 0, "art keeps its anchors")
        assert(wt:GetNumPoints() == 0 and vt:GetNumPoints() == 0, "markers lose theirs")
        cs:SetColorWheelThumbTexture(nil)
        assert(cs:GetColorWheelThumbTexture() == nil and not pcall(wt.GetTexture, wt))
    "#,
    )
    .unwrap();
}

/// A table that is no texture raises with the setter's own line (`0x781ab1`, `0x78f9d4`).
#[test]
fn a_non_texture_object_raises_in_the_widget_setters() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame")
        sb = CreateFrame("StatusBar", "RaiseSB")
        sl = CreateFrame("Slider", "RaiseSl")
        cs = CreateFrame("ColorSelect", "RaiseCS")
    "#,
    )
    .unwrap();
    for (call, want) in [
        (
            "sb:SetStatusBarTexture(f)",
            "RaiseSB:SetCheckedTexture(): Wrong object type, expected texture",
        ),
        (
            "sb:SetStatusBarTexture({})",
            "RaiseSB:SetCheckedTexture(): Couldn't find 'this' in texture",
        ),
        (
            "sl:SetThumbTexture(f)",
            "RaiseSl:SetThumbTexture(): Wrong object type, expected texture",
        ),
        (
            "cs:SetColorWheelTexture(f)",
            "RaiseCS:SetColorWheelTexture(): Wrong object type, expected texture",
        ),
        (
            "cs:SetColorValueThumbTexture({})",
            "RaiseCS:SetColorValueThumbTexture(): Couldn't find 'this' in texture",
        ),
    ] {
        let err = s.run(call).unwrap_err().to_string();
        assert!(err.contains(want), "`{call}` -> {err}");
    }
}
