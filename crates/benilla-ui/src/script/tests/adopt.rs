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

/// The third argument of `0x77fd10` is the region's own shown bit (`+0xc4`): a slot that takes a
/// hidden texture shows it (`0x77fd85`; the label `0x778d49`, the highlight `0x779139`, the bar
/// `0x78321b`, the thumb `0x789885`, the ColorSelect slots, a current state `0x779037`), and it
/// draws.
#[test]
fn a_slot_shows_the_hidden_texture_it_takes() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        a = CreateFrame("Frame")
        a:SetPoint("CENTER", 0, 0) a:SetWidth(50) a:SetHeight(50)
        local function hidden(path)
            local t = a:CreateTexture(nil, "ARTWORK")
            t:SetTexture(path) t:SetAllPoints(a) t:Hide()
            assert(not t:IsShown())
            return t
        end
        sb = CreateFrame("StatusBar", "ShowSB")
        sb:SetPoint("CENTER", 0, 0) sb:SetWidth(50) sb:SetHeight(50)
        bar = hidden("bar") sb:SetStatusBarTexture(bar)
        assert(bar:IsShown(), "bar")

        b = CreateFrame("Button", "ShowBtn")
        b:SetPoint("CENTER", 0, 0) b:SetWidth(50) b:SetHeight(50)
        n = hidden("normal") b:SetNormalTexture(n)
        assert(n:IsShown(), "the current state's texture")
        h = hidden("hl") b:SetHighlightTexture(h)
        assert(h:IsShown(), "highlight")
        fs = a:CreateFontString(nil, "OVERLAY") fs:Hide()
        b:SetFontString(fs)
        assert(fs:IsShown(), "label")

        sl = CreateFrame("Slider", "ShowSl")
        th = hidden("thumb") sl:SetThumbTexture(th)
        assert(th:IsShown(), "thumb")
        cs = CreateFrame("ColorSelect", "ShowCS")
        w, wt = hidden("w"), hidden("wt")
        cs:SetColorWheelTexture(w) cs:SetColorWheelThumbTexture(wt)
        assert(w:IsShown() and wt:IsShown(), "ColorSelect slots")
    "#,
    )
    .unwrap();
    s.resolve();
    let d = drawn(&s);
    for p in ["bar", "normal"] {
        assert!(d.contains(&p.to_string()), "{p} draws: {d:?}");
    }
}

/// With flag 0 (`0x77fdc8`..`0x77fddf`, and the state switch behind it) a Button's state texture
/// that is not the current one ends hidden, and shows when its state comes: a texture already on
/// the button at the same layer included, and a hidden one adopted into a later state too. The
/// checked pair follows `SetChecked` (`0x7854c0`).
#[test]
fn a_non_current_state_texture_is_hidden_until_its_state_comes() {
    let s = script();
    s.run(
        r#"
        b = CreateFrame("CheckButton", "StateShownBtn")
        b:SetNormalTexture("n")
        own = b:CreateTexture(nil, "ARTWORK")
        assert(own:IsShown(), "a fresh texture is shown")
        b:SetPushedTexture(own)
        assert(not own:IsShown(), "a same-parent same-layer re-slot with flag 0 hides it")
        gone = b:CreateTexture(nil, "ARTWORK") gone:Hide()
        b:SetDisabledTexture(gone)
        assert(not gone:IsShown())
        b:SetButtonState("PUSHED")
        assert(own:IsShown() and not b:GetNormalTexture():IsShown(), "the pushed state shows it")
        b:SetButtonState("NORMAL")
        assert(not own:IsShown() and b:GetNormalTexture():IsShown())
        b:Disable()
        assert(gone:IsShown(), "a hidden texture adopted earlier shows in its state")

        c = b:CreateTexture(nil, "OVERLAY")
        b:SetCheckedTexture(c)
        assert(not c:IsShown(), "unchecked")
        b:SetChecked(1)
        assert(c:IsShown(), "checked")
    "#,
    )
    .unwrap();
}

/// `0x77fd9a`: a texture the button already owns still takes the slot's layer.
#[test]
fn a_texture_already_on_the_button_takes_the_slots_layer() {
    let s = script();
    s.run(
        r#"
        b = CreateFrame("Button")
        t = b:CreateTexture(nil, "BORDER")
        b:SetNormalTexture(t)
        assert(t:GetParent() == b and t:GetDrawLayer() == "ARTWORK", t:GetDrawLayer())
        h = b:CreateTexture(nil, "BACKGROUND")
        b:SetHighlightTexture(h)
        assert(h:GetDrawLayer() == "HIGHLIGHT")
    "#,
    )
    .unwrap();
}

/// `0x78b580` replaces the wheel's image with the generated one (`0x770300`) and `0x78b8a0` the
/// strip's, so the adopted art loses its file; the same texture again returns first
/// (`0x78b598`) and keeps what it was set to since. The markers keep their file.
#[test]
fn the_wheel_and_strip_lose_their_file_the_markers_keep_theirs() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        cs = CreateFrame("ColorSelect", "FileCS")
        local function tex(path) local t = a:CreateTexture() t:SetTexture(path) return t end
        w, v, wt = tex("wheel"), tex("strip"), tex("thumb")
        cs:SetColorWheelTexture(w) cs:SetColorValueTexture(v) cs:SetColorWheelThumbTexture(wt)
        assert(w:GetTexture() == nil and v:GetTexture() == nil, "art is generated")
        assert(wt:GetTexture() == "thumb", "the marker keeps its file")
        w:SetTexture("since")
        cs:SetColorWheelTexture(w)
        assert(w:GetTexture() == "since", "the same texture is a no-op")
    "#,
    )
    .unwrap();
}

/// `0x779110` and `0x779050` set the highlight's blend through `0x7703f0` on every call: ADD, or the
/// third argument's name (a name that does not parse leaves ADD, `0x6f1920`). The same texture
/// (`0x779122`) and nil (`0x779130`) skip it, and no other state has one.
#[test]
fn the_highlight_sets_its_blend_on_the_object_leg_and_the_path_leg() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame")
        b = CreateFrame("Button")
        t = a:CreateTexture() t:SetBlendMode("BLEND")
        b:SetHighlightTexture(t)
        assert(t:GetBlendMode() == "ADD", "object leg: " .. t:GetBlendMode())
        u = a:CreateTexture()
        b:SetHighlightTexture(u, "MOD")
        assert(u:GetBlendMode() == "MOD", "the third argument")
        u:SetBlendMode("BLEND")
        b:SetHighlightTexture(u, "MOD")
        assert(u:GetBlendMode() == "BLEND", "the same texture skips it")
        v = a:CreateTexture() v:SetBlendMode("BLEND")
        b:SetHighlightTexture(v, "bogus")
        assert(v:GetBlendMode() == "ADD", "an unparsed name leaves ADD")
        b:SetHighlightTexture(nil)
        assert(b:GetHighlightTexture() == nil)

        b:SetHighlightTexture("p")
        hl = b:GetHighlightTexture()
        assert(hl:GetBlendMode() == "ADD", "a new region")
        hl:SetBlendMode("BLEND")
        b:SetHighlightTexture("p2")
        assert(hl:GetBlendMode() == "ADD", "path leg, every call")
        b:SetHighlightTexture("p3", "MOD")
        assert(hl:GetBlendMode() == "MOD" and hl:GetTexture() == "p3")

        n = a:CreateTexture() n:SetBlendMode("MOD")
        b:SetNormalTexture(n)
        assert(n:GetBlendMode() == "MOD", "a state texture's blend is its own")
    "#,
    )
    .unwrap();
}

/// A name no candidate resolves fails `0x770200`, which keeps what the region held (`0x7702ac`). A
/// region made for the call is destroyed with no slot and no blend (`0x7790ce`, `0x7790fa`); an
/// existing highlight keeps its texture and still takes the blend (`0x77907c`-`0x779082`).
#[test]
fn a_path_that_does_not_load_makes_no_slot_region_and_keeps_the_old_texture() {
    let mut s = script();
    s.set_texture_probe(Box::new(|p| p != "missing"));
    s.run(
        r#"
        b = CreateFrame("Button")
        b:SetHighlightTexture("missing", "MOD")
        b:SetNormalTexture("missing")
        assert(b:GetHighlightTexture() == nil, "no highlight region for a failed load")
        assert(b:GetNormalTexture() == nil, "nor a normal one")

        b:SetHighlightTexture("present")
        hl = b:GetHighlightTexture()
        hl:SetBlendMode("BLEND")
        b:SetHighlightTexture("missing", "MOD")
        assert(b:GetHighlightTexture() == hl and hl:GetTexture() == "present", "the old texture stands")
        assert(hl:GetBlendMode() == "MOD", "the blend still applies to the existing region")
    "#,
    )
    .unwrap();
}

/// The bar, thumb and marker path legs (`0x783140`, `0x7897c0`, `0x78b7b0`) load into a texture
/// they hold, which keeps its image when the load fails (`0x770200` → `0x7702ac`); with none, a
/// texture whose load fails is deleted and nothing is installed (`0x7831d9`, `0x789849`, `0x78b837`),
/// so a later good path builds one on the layer that call names.
#[test]
fn a_widget_path_that_does_not_load_installs_nothing_and_keeps_the_old_texture() {
    let mut s = script();
    s.set_texture_probe(Box::new(|p| p != "missing"));
    s.run(
        r#"
        sb = CreateFrame("StatusBar")
        sl = CreateFrame("Slider")
        cs = CreateFrame("ColorSelect")
        sb:SetStatusBarTexture("missing")
        sl:SetThumbTexture("missing")
        cs:SetColorWheelThumbTexture("missing")
        assert(sb:GetStatusBarTexture() == nil, "no bar for a failed load")
        assert(sl:GetThumbTexture() == nil, "no thumb")
        assert(cs:GetColorWheelThumbTexture() == nil, "no marker")

        sb:SetStatusBarTexture("present", "BORDER")
        assert(sb:GetStatusBarTexture():GetDrawLayer() == "BORDER", "built on the good call's layer")
        sl:SetThumbTexture("present")
        cs:SetColorWheelThumbTexture("present")
        for _, t in ipairs({ sb:GetStatusBarTexture(), sl:GetThumbTexture(),
                             cs:GetColorWheelThumbTexture() }) do
            t:SetDesaturated(1)
        end
        sb:SetStatusBarTexture("missing")
        sl:SetThumbTexture("missing")
        cs:SetColorWheelThumbTexture("missing")
        for _, t in ipairs({ sb:GetStatusBarTexture(), sl:GetThumbTexture(),
                             cs:GetColorWheelThumbTexture() }) do
            assert(t:GetTexture() == "present", "the old texture stands")
            assert(t:IsDesaturated() == 1, "and keeps its grey")
        end
        sb:SetStatusBarTexture("other")
        sl:SetThumbTexture("other")
        cs:SetColorWheelThumbTexture("other")
        for _, t in ipairs({ sb:GetStatusBarTexture(), sl:GetThumbTexture(),
                             cs:GetColorWheelThumbTexture() }) do
            assert(t:GetTexture() == "other" and t:IsDesaturated() == nil, "a new file clears it")
        end
    "#,
    )
    .unwrap();
}

/// A path leg that builds its texture asks the probe once, as `0x770200` loads once.
#[test]
fn a_path_leg_that_builds_a_texture_probes_once() {
    let mut s = script();
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = calls.clone();
    s.set_texture_probe(Box::new(move |_| {
        seen.set(seen.get() + 1);
        true
    }));
    for (make, call) in [
        ("StatusBar", "f:SetStatusBarTexture('present')"),
        ("Slider", "f:SetThumbTexture('present')"),
        ("ColorSelect", "f:SetColorWheelThumbTexture('present')"),
        ("Button", "f:SetNormalTexture('present')"),
        (
            "Button",
            "f:SetNormalTexture('present') f:SetNormalTexture('other')",
        ),
    ] {
        calls.set(0);
        s.run(&format!("f = CreateFrame('{make}') {call}")).unwrap();
        let loads = call.matches("Texture(").count();
        assert_eq!(calls.get(), loads, "{call}: one probe per load");
    }
}

/// A Button state texture's path leg loads through `0x770200` with shader index 0 (`0x778f3a`): a
/// new file that loads clears the desaturation, one that fails keeps it.
#[test]
fn a_button_path_that_loads_clears_the_desaturation() {
    let mut s = script();
    s.set_texture_probe(Box::new(|p| p != "missing"));
    s.run(
        r#"
        b = CreateFrame("Button")
        b:SetNormalTexture("present")
        n = b:GetNormalTexture()
        n:SetDesaturated(1)
        b:SetNormalTexture("missing")
        assert(n:GetTexture() == "present" and n:IsDesaturated() == 1, "a failed load keeps both")
        b:SetNormalTexture("present")
        assert(n:IsDesaturated() == 1, "the same file returns first")
        b:SetNormalTexture("other")
        assert(n:GetTexture() == "other" and n:IsDesaturated() == nil, "a new file clears it")
    "#,
    )
    .unwrap();
}

/// The Button setters' raises (`0x87a210`/`0x87a1e0`, the highlight's `0x87a2cc`/`0x87a28c`, the
/// checked pair's `0x87b224`/`0x87b1e8` and `0x87c0e8`/`0x87c0a0`) name the receiver and the
/// method; a call with no argument raises Usage and clears nothing (`0x781b4f`).
#[test]
fn the_button_setters_raise_with_their_own_lines_and_no_argument_clears_nothing() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame")
        b = CreateFrame("CheckButton", "RaiseBtn")
        b:SetNormalTexture("n") b:SetHighlightTexture("h") b:SetCheckedTexture("c")
        keep = { b:GetNormalTexture(), b:GetHighlightTexture(), b:GetCheckedTexture() }
    "#,
    )
    .unwrap();
    for (call, want) in [
        (
            "b:SetNormalTexture(f)",
            "RaiseBtn:SetNormalTexture(): Wrong object type, expected texture",
        ),
        (
            "b:SetPushedTexture({})",
            "RaiseBtn:SetPushedTexture(): Couldn't find 'this' in texture",
        ),
        (
            "b:SetHighlightTexture(f)",
            "RaiseBtn:SetHighlightTexture(): Wrong object type, expected texture",
        ),
        (
            "b:SetHighlightTexture({})",
            "RaiseBtn:SetHighlightTexture(): Couldn't find 'this' in texture",
        ),
        (
            "b:SetCheckedTexture(f)",
            "RaiseBtn:SetCheckedTexture(): Wrong object type, expected texture",
        ),
        (
            "b:SetDisabledCheckedTexture({})",
            "RaiseBtn:SetDisabledCheckedTexture(): Couldn't find 'this' in texture",
        ),
        (
            "b:SetNormalTexture()",
            "Usage: RaiseBtn:SetNormalTexture(texture or \"texture\" or nil)",
        ),
        (
            "b:SetHighlightTexture()",
            "Usage: RaiseBtn:SetHighlightTexture(texture or \"texture\" or nil [, \"blendmode\")",
        ),
        (
            "b:SetCheckedTexture()",
            "Usage: RaiseBtn:SetCheckedTexture(texture or \"texture\" or nil)",
        ),
    ] {
        let err = s.run(call).unwrap_err().to_string();
        assert!(err.contains(want), "`{call}` -> {err}");
    }
    s.run(
        r#"
        assert(b:GetNormalTexture() == keep[1] and keep[1]:GetTexture() == "n")
        assert(b:GetHighlightTexture() == keep[2] and keep[2]:GetTexture() == "h")
        assert(b:GetCheckedTexture() == keep[3] and keep[3]:GetTexture() == "c")
    "#,
    )
    .unwrap();
}
