//! Button / CheckButton: method sets, state textures, click registration, additive highlight.

use super::common::script;
use crate::script::*;

#[test]
fn button_methods_exist_only_on_buttons() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "PlainF")
        b = CreateFrame("Button", "Btn")
        cb = CreateFrame("CheckButton", "CBtn")
    "#,
    )
    .unwrap();
    assert!(s.eval::<bool>("return f.SetText == nil").unwrap());
    assert!(s.eval::<bool>("return f.SetChecked == nil").unwrap());
    assert!(s.eval::<bool>("return b.SetText ~= nil").unwrap());
    assert!(s.eval::<bool>("return b.SetChecked == nil").unwrap());
    assert!(s.eval::<bool>("return b.SetValue == nil").unwrap());
    assert!(s.eval::<bool>("return cb.SetChecked ~= nil").unwrap());
    assert!(s.eval::<bool>("return cb.SetNormalTexture ~= nil").unwrap());
    // The 1.12 client's Button constructor enables mouse input; a Frame's does not.
    assert!(s.eval::<bool>("return b:IsMouseEnabled()").unwrap());
    assert!(!s.eval::<bool>("return f:IsMouseEnabled()").unwrap());
}

#[test]
fn button_state_textures_switch_with_interaction() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local b = CreateFrame("Button", "StateBtn")
        b:SetPoint("BOTTOMLEFT", 0, 0); b:SetWidth(100); b:SetHeight(100)
        b:SetNormalTexture("Interface\\N.blp")
        b:SetPushedTexture("Interface\\P.blp")
        b:SetDisabledTexture("Interface\\D.blp")
        b:SetHighlightTexture("Interface\\H.blp")
        b:SetText("Go")
    "#,
    )
    .unwrap();
    s.resolve();

    let visible = |s: &UiScript| -> Vec<String> {
        s.extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
            .collect()
    };

    assert_eq!(visible(&s), vec!["Interface\\N.blp".to_string()]);
    assert!(s
        .extract()
        .iter()
        .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Go")));

    s.mouse_move(50.0, 50.0);
    let v = visible(&s);
    assert!(
        v.contains(&"Interface\\N.blp".to_string()) && v.contains(&"Interface\\H.blp".to_string())
    );

    s.mouse_button(50.0, 50.0, "LeftButton", true);
    let v = visible(&s);
    assert!(
        v.contains(&"Interface\\P.blp".to_string()) && !v.contains(&"Interface\\N.blp".to_string())
    );
    s.mouse_button(50.0, 50.0, "LeftButton", false);

    s.run("StateBtn:Disable()").unwrap();
    assert_eq!(visible(&s), vec!["Interface\\D.blp".to_string()]);
    // The 1.12 client answers the number 0, not false or nil.
    assert_eq!(s.eval::<i64>("return StateBtn:IsEnabled()").unwrap(), 0);
    s.run("StateBtn:Enable()").unwrap();
    assert_eq!(s.eval::<String>("return StateBtn:GetText()").unwrap(), "Go");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetState 0x779790` moves the shown texture (`+0x4c4`) only onto a non-null slot, so a press or
/// a disable with no art of its own keeps what was shown; art written into another state's slot
/// is stored, not shown (`0x778fd0`'s `idx == [this+0x328]` gate).
#[test]
fn a_state_with_no_texture_leaves_the_shown_one_standing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local b = CreateFrame("Button", "StickyBtn")
        b:SetPoint("BOTTOMLEFT", 0, 0); b:SetWidth(100); b:SetHeight(100)
        b:SetNormalTexture("Interface\\N.blp")
    "#,
    )
    .unwrap();
    s.resolve();

    let visible = |s: &UiScript, owner: &str| -> Vec<String> {
        s.extract()
            .iter()
            .filter(|q| s.quad_owner_name(q.target).as_deref() == Some(owner))
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
            .collect()
    };
    let n = vec!["Interface\\N.blp".to_string()];

    assert_eq!(visible(&s, "StickyBtn"), n, "resting on its normal art");

    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert_eq!(visible(&s, "StickyBtn"), n, "a press with no pushed art");
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    s.mouse_move(400.0, 400.0);

    s.run("StickyBtn:Disable()").unwrap();
    assert_eq!(
        visible(&s, "StickyBtn"),
        n,
        "a disable with no disabled art leaves the box on screen"
    );
    s.run("StickyBtn:SetDisabledTexture(\"Interface\\\\D.blp\")")
        .unwrap();
    assert_eq!(
        visible(&s, "StickyBtn"),
        vec!["Interface\\D.blp".to_string()],
        "and real disabled art displaces it on the spot"
    );
    s.run("StickyBtn:Enable()").unwrap();
    assert_eq!(visible(&s, "StickyBtn"), n, "back to normal on Enable");

    s.run(
        r#"
        local b = CreateFrame("Button", "BornDeadBtn")
        b:SetPoint("BOTTOMLEFT", 200, 0); b:SetWidth(100); b:SetHeight(100)
        b:Disable()
        b:SetNormalTexture("Interface\\N.blp")
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(
        visible(&s, "BornDeadBtn").is_empty(),
        "art set while in another state is stored, not shown"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The enable helper `0x779160` (reached by `Enable`, `Disable` and the constructor) also writes
/// layer 4, HIGHLIGHT, into the array `Enable/DisableDrawLayer` write (`[frame+0x198]`, `0x7791bb`
/// → `0x76a730`), so a disable darkens every HIGHLIGHT region and `EnableDrawLayer` relights them.
#[test]
fn disabling_a_button_takes_its_whole_highlight_layer() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local b = CreateFrame("Button", "LayerBtn")
        b:SetPoint("BOTTOMLEFT", 0, 0); b:SetWidth(100); b:SetHeight(100)
        b:SetNormalTexture("Interface\\N.blp")
        b:SetHighlightTexture("Interface\\H.blp")
        local own = b:CreateTexture(nil, "HIGHLIGHT")
        own:SetAllPoints(b)
        own:SetTexture("Interface\\Own.blp")
    "#,
    )
    .unwrap();
    s.resolve();

    let art = |s: &UiScript| -> Vec<String> {
        s.extract()
            .iter()
            .filter(|q| s.quad_owner_name(q.target).as_deref() == Some("LayerBtn"))
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
            .collect()
    };

    s.mouse_move(50.0, 50.0);
    let v = art(&s);
    for want in ["Interface\\N.blp", "Interface\\Own.blp", "Interface\\H.blp"] {
        assert!(
            v.contains(&want.to_string()),
            "{want} draws while enabled: {v:?}"
        );
    }

    s.run("LayerBtn:Disable()").unwrap();
    assert_eq!(
        art(&s),
        vec!["Interface\\N.blp".to_string()],
        "the whole HIGHLIGHT layer goes with the disable"
    );

    s.run(r#"LayerBtn:EnableDrawLayer("HIGHLIGHT")"#).unwrap();
    let v = art(&s);
    assert!(
        v.contains(&"Interface\\Own.blp".to_string())
            && v.contains(&"Interface\\H.blp".to_string()),
        "EnableDrawLayer restores the layer on a still-disabled button: {v:?}"
    );
    assert_eq!(
        s.eval::<i64>("return LayerBtn:IsEnabled()").unwrap(),
        0,
        "and it really is still disabled"
    );

    s.run(r#"LayerBtn:DisableDrawLayer("HIGHLIGHT") LayerBtn:Enable()"#)
        .unwrap();
    let v = art(&s);
    assert!(
        v.contains(&"Interface\\Own.blp".to_string()),
        "one array, one writer: Enable() clears the addon's own disable too: {v:?}"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `CButton::OnMouseDown` shows the pushed texture when the button is registered for that mouse
/// button, up or down (`0x77924b`: `[this+0x330] & (m | m << 8)`), whether or not a click fires.
#[test]
fn any_registered_mouse_button_shows_the_pushed_texture() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local function slot(name, x)
            local b = CreateFrame("Button", name)
            b:SetPoint("BOTTOMLEFT", x, 0); b:SetWidth(100); b:SetHeight(100)
            b:SetNormalTexture("Interface\\" .. name .. "N.blp")
            b:SetPushedTexture("Interface\\" .. name .. "P.blp")
            return b
        end
        -- A bar slot: both buttons registered, exactly as ActionButton/PetActionButton do.
        slot("Bar", 0):RegisterForClicks("LeftButtonUp", "RightButtonUp")
        -- A plain button: the default set, {LeftButtonUp}.
        slot("Plain", 200)
    "#,
    )
    .unwrap();
    s.resolve();
    let shows = |s: &UiScript, path: &str| {
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
    };

    for button in ["LeftButton", "RightButton"] {
        s.mouse_move(50.0, 50.0);
        s.mouse_button(50.0, 50.0, button, true);
        assert!(
            shows(&s, "Interface\\BarP.blp") && !shows(&s, "Interface\\BarN.blp"),
            "{button} down must show the pushed art"
        );
        assert_eq!(
            s.eval::<String>("return Bar:GetButtonState()").unwrap(),
            "PUSHED",
            "and the state variable the engine writes is one variable ({button})"
        );
        s.mouse_button(50.0, 50.0, button, false);
        assert!(shows(&s, "Interface\\BarN.blp"), "the release restores it");
    }

    s.mouse_move(250.0, 50.0);
    s.mouse_button(250.0, 50.0, "RightButton", true);
    assert!(
        shows(&s, "Interface\\PlainN.blp") && !shows(&s, "Interface\\PlainP.blp"),
        "an unregistered button must not light"
    );
    s.mouse_button(250.0, 50.0, "RightButton", false);
    s.mouse_button(250.0, 50.0, "LeftButton", true);
    assert!(shows(&s, "Interface\\PlainP.blp"));
    s.mouse_button(250.0, 50.0, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetButtonState`/`GetButtonState` (`0x780270`/`0x780180`), the press state stock
/// `ActionButtonDown`/`Up` drive; an unknown state raises, a disabled button reads DISABLED.
#[test]
fn set_button_state_drives_the_pushed_visual() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local b = CreateFrame("Button", "PushBtn")
        b:SetPoint("BOTTOMLEFT", 0, 0); b:SetWidth(100); b:SetHeight(100)
        b:SetNormalTexture("Interface\\N.blp")
        b:SetPushedTexture("Interface\\P.blp")
    "#,
    )
    .unwrap();
    s.resolve();
    let shows = |s: &UiScript, path: &str| {
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
    };

    assert_eq!(
        s.eval::<String>("return PushBtn:GetButtonState()").unwrap(),
        "NORMAL"
    );
    s.run(r#"PushBtn:SetButtonState("PUSHED")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return PushBtn:GetButtonState()").unwrap(),
        "PUSHED"
    );
    assert!(shows(&s, "Interface\\P.blp") && !shows(&s, "Interface\\N.blp"));
    s.run(r#"PushBtn:SetButtonState("NORMAL")"#).unwrap();
    assert!(shows(&s, "Interface\\N.blp") && !shows(&s, "Interface\\P.blp"));
    assert!(s.run(r#"PushBtn:SetButtonState("SIDEWAYS")"#).is_err());
    s.run("PushBtn:Disable()").unwrap();
    assert_eq!(
        s.eval::<String>("return PushBtn:GetButtonState()").unwrap(),
        "DISABLED"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn disabled_button_swallows_clicks_checkbutton_toggles_before_onclick() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        clicks, seen_checked = 0, nil
        local cb = CreateFrame("CheckButton", "Toggler")
        cb:SetPoint("BOTTOMLEFT", 0, 0); cb:SetWidth(100); cb:SetHeight(100)
        cb:SetScript("OnClick", function(self, button, down)
            clicks = clicks + 1
            seen_checked = self:GetChecked()
        end)
    "#,
    )
    .unwrap();
    s.resolve();

    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 1);
    // `GetChecked()` answers the number 1, not true.
    assert!(s.eval::<bool>("return seen_checked == 1").unwrap());
    assert!(s.eval::<bool>("return Toggler:GetChecked()").unwrap());

    s.run("Toggler:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 2);
    assert!(!s.eval::<bool>("return Toggler:GetChecked()").unwrap());

    s.run("Toggler:Disable()").unwrap();
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    s.run("Toggler:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 2);
    assert!(!s.eval::<bool>("return Toggler:GetChecked()").unwrap());

    s.run("Toggler:SetChecked(1)").unwrap();
    assert!(s.eval::<bool>("return Toggler:GetChecked()").unwrap());
    s.run("Toggler:SetChecked(nil)").unwrap();
    assert!(!s.eval::<bool>("return Toggler:GetChecked()").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn default_registration_is_left_click_only_right_click_reaches_nothing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        clicks = 0
        local btn = CreateFrame("Button", "Vendor")
        btn:SetPoint("BOTTOMLEFT", 0, 0); btn:SetWidth(100); btn:SetHeight(100)
        btn:SetScript("OnClick", function(self, button, down) clicks = clicks + 1 end)
    "#,
    )
    .unwrap();
    s.resolve();

    // The 1.12 client's default registered-click set is {"LeftButtonUp"} alone.
    s.mouse_button(50.0, 50.0, "RightButton", true);
    s.mouse_button(50.0, 50.0, "RightButton", false);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn register_for_clicks_grows_right_click_and_carries_the_button_name() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        clicks, click_btn, arg1_btn = 0, nil, nil
        local btn = CreateFrame("Button", "Vendor")
        btn:SetPoint("BOTTOMLEFT", 0, 0); btn:SetWidth(100); btn:SetHeight(100)
        btn:RegisterForClicks("LeftButtonUp", "RightButtonUp")
        btn:SetScript("OnClick", function(self, button, down)
            clicks = clicks + 1
            click_btn = button
            arg1_btn = arg1   -- the 1.12 legacy-global convention, same value
        end)
    "#,
    )
    .unwrap();
    s.resolve();

    s.mouse_button(50.0, 50.0, "RightButton", true);
    s.mouse_button(50.0, 50.0, "RightButton", false);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 1);
    assert_eq!(s.eval::<String>("return click_btn").unwrap(), "RightButton");
    assert_eq!(s.eval::<String>("return arg1_btn").unwrap(), "RightButton");

    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 2);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn down_registration_fires_on_press_and_toggles_checked_once() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        clicks, seen_down = 0, nil
        local cb = CreateFrame("CheckButton", "QuickSell")
        cb:SetPoint("BOTTOMLEFT", 0, 0); cb:SetWidth(100); cb:SetHeight(100)
        cb:RegisterForClicks("LeftButtonDown")
        cb:SetScript("OnClick", function(self, button, down)
            clicks = clicks + 1
            seen_down = down
        end)
    "#,
    )
    .unwrap();
    s.resolve();

    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 1);
    assert!(s.eval::<bool>("return seen_down == true").unwrap());
    assert!(s.eval::<bool>("return QuickSell:GetChecked()").unwrap());

    // `RegisterForClicks` replaced the set, so "LeftButtonUp" no longer fires.
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 1);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn highlight_is_additive_and_state_textures_fill_then_anchor() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local b = CreateFrame("Button", "AddBtn")
        b:SetPoint("BOTTOMLEFT", 100, 100); b:SetWidth(36); b:SetHeight(36)
        b:SetNormalTexture("Interface\\Ring.blp")
        local nt = b:GetNormalTexture(); nt:SetWidth(64); nt:SetHeight(64)
        b:SetHighlightTexture("Interface\\Hi.blp")
    "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(118.0, 118.0); // hover so the highlight draws

    let quads = s.extract();
    let find = |path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .expect(path)
    };
    // The 1.12 client's `SetHighlightTexture` defaults the texture to ADD.
    assert!(matches!(
        &find("Interface\\Hi.blp").content,
        QuadContent::Texture { additive: true, .. }
    ));
    // The reference's string setter anchors a texture it builds with `SetAllPoints`, so the later
    // 64px size is unread and the ring fills the 36px button.
    let r = find("Interface\\Ring.blp").rect.unwrap();
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 136.0, 100.0, 136.0)
    );
    // The quickslot overhang is an anchor (`ActionButtonTemplate.xml`'s 66x66 UI-Quickslot2 is
    // `<Anchor point="CENTER">`): cleared, then one CENTER point and the 64px size give 86..150.
    s.run(
        r#"
        local n = AddBtn:GetNormalTexture()
        n:ClearAllPoints()
        n:SetPoint("CENTER", AddBtn, "CENTER", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let ring = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == "Interface\\Ring.blp"))
        .unwrap();
    let r = ring.rect.unwrap();
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (86.0, 150.0, 86.0, 150.0)
    );
    s.run(r#"AddBtn:GetHighlightTexture():SetBlendMode("BLEND")"#)
        .unwrap();
    let quads = s.extract();
    let hi = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == "Interface\\Hi.blp"))
        .unwrap();
    assert!(matches!(
        &hi.content,
        QuadContent::Texture {
            additive: false,
            ..
        }
    ));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The label wears the current state's font object (`SetTextFontObject`, `SetDisabledFontObject`)
/// with no Lua repaint; its own `SetTextColor` clears the colour's inherit bit and survives that.
#[test]
fn button_label_repaints_by_state_font_object() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "GoldFont",
        FontObject {
            color: Some([1.0, 0.82, 0.0, 1.0]),
            height: Some(12.0),
            ..Default::default()
        },
    );
    s.register_font_object(
        "GrayFont",
        FontObject {
            color: Some([0.5, 0.5, 0.5, 1.0]),
            height: Some(12.0),
            ..Default::default()
        },
    );
    s.run(
        r#"
        b = CreateFrame("Button", "FontBtn")
        b:SetPoint("CENTER", 0, 0); b:SetWidth(100); b:SetHeight(20)
        b:SetText("Label")
        b:SetTextFontObject("GoldFont")
        b:SetDisabledFontObject("GrayFont")
    "#,
    )
    .unwrap();

    let label_color = |s: &mut crate::script::UiScript| {
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == "Label" => Some(color),
                _ => None,
            })
            .expect("label text quad")
    };

    assert_eq!(
        label_color(&mut s),
        Some([1.0, 0.82, 0.0, 1.0]),
        "enabled: gold"
    );
    s.run("b:Disable()").unwrap();
    assert_eq!(
        label_color(&mut s),
        Some([0.5, 0.5, 0.5, 1.0]),
        "disabled: gray"
    );
    s.run("b:Enable()").unwrap();
    assert_eq!(
        label_color(&mut s),
        Some([1.0, 0.82, 0.0, 1.0]),
        "re-enabled: gold again"
    );

    s.run("b:GetFontString():SetTextColor(0.1, 0.2, 0.3)")
        .unwrap();
    let c = label_color(&mut s).expect("colored");
    assert!((c[0] - 0.1).abs() < 1e-6 && (c[1] - 0.2).abs() < 1e-6);
}

/// The highlight label is its own font instance, put in force by a hover or `LockHighlight`, so the
/// normal instance's `SetTextColor` cannot reach it: `ClassTrainerSkillButtonTemplate`'s recipe
/// rows (`<HighlightFont inherits="GameFontHighlight">`) turn white, and
/// `Blizzard_TradeSkillUI.lua:144` locks the selected row after blanking its highlight texture.
#[test]
fn a_locked_or_hovered_button_wears_its_highlight_font_over_its_normal_color() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "RowNormal",
        FontObject {
            color: Some([1.0, 0.82, 0.0, 1.0]),
            height: Some(12.0),
            ..Default::default()
        },
    );
    s.register_font_object(
        "RowHighlight",
        FontObject {
            color: Some([1.0, 1.0, 1.0, 1.0]),
            height: Some(12.0),
            ..Default::default()
        },
    );
    s.run(
        r#"
        b = CreateFrame("Button", "RowBtn")
        b:SetPoint("BOTTOMLEFT", 100, 100); b:SetWidth(100); b:SetHeight(20)
        b:SetText("Rough Copper Vest")
        b:SetTextFontObject("RowNormal")
        b:SetHighlightFontObject("RowHighlight")
        -- The difficulty paint every one of the three list windows applies to every row.
        b:SetTextColor(1.0, 0.5, 0.25)
    "#,
    )
    .unwrap();

    let label_color = |s: &mut crate::script::UiScript| {
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == "Rough Copper Vest" => Some(color),
                _ => None,
            })
            .expect("label text quad")
    };

    s.resolve();
    s.mouse_move(400.0, 300.0); // nowhere near the row
    assert_eq!(
        label_color(&mut s),
        Some([1.0, 0.5, 0.25, 1.0]),
        "at rest the row wears the difficulty colour SetTextColor gave it"
    );

    s.resolve();
    s.mouse_move(150.0, 110.0); // over the row
    assert_eq!(
        label_color(&mut s),
        Some([1.0, 1.0, 1.0, 1.0]),
        "hovered: the HIGHLIGHT instance is in force, and the normal colour cannot reach it"
    );

    s.resolve();
    s.mouse_move(400.0, 300.0);
    s.run("b:LockHighlight()").unwrap();
    assert_eq!(
        label_color(&mut s),
        Some([1.0, 1.0, 1.0, 1.0]),
        "locked: white with the cursor elsewhere — the selected recipe row"
    );
    s.run("b:UnlockHighlight()").unwrap();
    assert_eq!(
        label_color(&mut s),
        Some([1.0, 0.5, 0.25, 1.0]),
        "unlocked: back to the difficulty colour"
    );

    s.run(r#"b:SetHighlightFontObject(nil); b:SetText("Rough Copper Vest")"#)
        .unwrap();
    s.resolve();
    s.mouse_move(150.0, 110.0);
    assert_eq!(
        label_color(&mut s),
        Some([1.0, 0.5, 0.25, 1.0]),
        "no <HighlightFont> → hover changes nothing"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `Button:GetTextWidth`/`GetTextHeight` (`0x782290`/`0x782390`; Button has no `GetStringWidth`)
/// forward to the label FontString's extent.
#[test]
fn a_button_reports_its_labels_extent() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        b = CreateFrame("Button", "WidthBtn", UIParent)
        b:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        b:SetWidth(120) b:SetHeight(24)
        b:SetText("A Label")
        plain = CreateFrame("Button", "LabellessBtn", UIParent)
    "#,
    )
    .unwrap();
    s.resolve();

    // 0 until the host's measure lands, like every metric read here.
    assert_eq!(
        s.eval::<f64>("return WidthBtn:GetTextWidth()").unwrap(),
        0.0
    );

    let req = s
        .fontstrings_needing_measure()
        .into_iter()
        .find(|r| r.text == "A Label")
        .expect("the label asks the host for its extent");
    s.set_measured_text_unwrapped(&[(req.id, 47.0, 14.0, req.key)]);

    assert_eq!(
        s.eval::<f64>("return WidthBtn:GetTextWidth()").unwrap(),
        47.0,
        "the Button forwards to its own label"
    );
    assert_eq!(
        s.eval::<f64>("return WidthBtn:GetTextHeight()").unwrap(),
        14.0
    );

    // A label-less Button answers 0; the reference reads the label pointer (`+0x338`), null here,
    // and what it does then is untraced.
    assert_eq!(
        s.eval::<f64>("return LabellessBtn:GetTextWidth()").unwrap(),
        0.0
    );

    assert!(
        s.eval::<bool>(r#"return CreateFrame("Frame").GetTextWidth == nil"#)
            .unwrap(),
        "GetTextWidth is Button's, not Region's (the reference's own split)"
    );
}

/// `Button:SetFontString` (`0x780a60`) re-parents the string, adopts it as the label and destroys
/// the old one, and raises on anything but a FontString.
#[test]
fn set_font_string_adopts_the_label_and_raises_on_anything_else() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        b = CreateFrame("Button", "Btn")
        b:SetWidth(120) b:SetHeight(24)
        b:SetPoint("CENTER", nil, "CENTER", 0, 0)
        fs = b:CreateFontString("MyLabel", "BACKGROUND")
        "#,
    )
    .unwrap();

    s.run("b:SetFontString(fs)").unwrap();
    assert!(s.eval::<bool>("return b:GetFontString() == fs").unwrap());
    s.run("b:SetText(\"Okay\")").unwrap();
    assert_eq!(s.eval::<String>("return fs:GetText()").unwrap(), "Okay");
    assert_eq!(s.eval::<String>("return b:GetText()").unwrap(), "Okay");

    assert!(s.eval::<bool>("return fs:GetParent() == b").unwrap());
    // It also forces the draw layer to ARTWORK (`0x77fd10(parent, 2, 1)`); not asserted here.

    // The same string again is a no-op, the client's first compare, so it is not destroyed.
    s.run("b:SetFontString(fs)").unwrap();
    assert!(s.eval::<bool>("return b:GetFontString() == fs").unwrap());
    assert_eq!(s.eval::<String>("return fs:GetText()").unwrap(), "Okay");

    s.run("fs2 = b:CreateFontString(\"MyLabel2\", \"OVERLAY\") b:SetFontString(fs2)")
        .unwrap();
    assert!(s.eval::<bool>("return b:GetFontString() == fs2").unwrap());
    let err = s.run("fs:GetText()").unwrap_err().to_string();
    assert!(
        err.contains("stale") || err.contains("invalid"),
        "the replaced label must be destroyed, not left live: {err}"
    );

    // Three distinct raises, each naming the button; `nil` cannot clear the label from Lua.
    for (call, want) in [
        ("b:SetFontString()", "Usage: Btn:SetFontString(fontstring)"),
        (
            "b:SetFontString(nil)",
            "Usage: Btn:SetFontString(fontstring)",
        ),
        ("b:SetFontString(7)", "Usage: Btn:SetFontString(fontstring)"),
        (
            "b:SetFontString(\"x\")",
            "Usage: Btn:SetFontString(fontstring)",
        ),
        (
            "b:SetFontString({})",
            "Btn:SetFontString(): Couldn't find 'this' in fontstring",
        ),
        (
            "b:SetFontString(b)",
            "Btn:SetFontString(): Wrong object type, expected fontstring",
        ),
    ] {
        let err = s.run(call).unwrap_err().to_string();
        assert!(err.contains(want), "`{call}` → {err}");
    }
    // A Texture is a Region and still fails the FontString type gate.
    s.run("tex = b:CreateTexture(nil, \"ARTWORK\")").unwrap();
    let err = s.run("b:SetFontString(tex)").unwrap_err().to_string();
    assert!(
        err.contains("Wrong object type, expected fontstring"),
        "{err}"
    );
    assert!(s.eval::<bool>("return b:GetFontString() == fs2").unwrap());
}

#[test]
fn set_font_string_anchors_only_an_unanchored_label() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        b = CreateFrame("Button", "Btn")
        b:SetWidth(120) b:SetHeight(24)
        b:SetPoint("CENTER", nil, "CENTER", 0, 0)
        bare = b:CreateFontString(nil, "OVERLAY")
        bare:ClearAllPoints()
        placed = b:CreateFontString(nil, "OVERLAY")
        placed:ClearAllPoints()
        placed:SetPoint("TOPLEFT", b, "TOPLEFT", 7, -3)
        "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return bare:GetNumPoints()").unwrap(), 0);

    s.run("b:SetFontString(placed)").unwrap();
    let (p, _rel, rp, x, y): (String, mlua::Value, String, f64, f64) =
        s.eval("return placed:GetPoint(1)").unwrap();
    assert_eq!(
        (p.as_str(), rp.as_str(), x, y),
        ("TOPLEFT", "TOPLEFT", 7.0, -3.0),
        "an already-anchored label keeps its own anchors"
    );
    assert_eq!(s.eval::<i64>("return placed:GetNumPoints()").unwrap(), 1);

    // With no normal-font justify, the unanchored one is anchored CENTER to CENTER.
    s.run("b:SetFontString(bare)").unwrap();
    assert_eq!(s.eval::<i64>("return bare:GetNumPoints()").unwrap(), 1);
    let (p, _rel, rp, x, y): (String, mlua::Value, String, f64, f64) =
        s.eval("return bare:GetPoint(1)").unwrap();
    assert_eq!(
        (p.as_str(), rp.as_str(), x, y),
        ("CENTER", "CENTER", 0.0, 0.0)
    );
}

/// A label made lazily (`SetText`, `0x778dc0`) or adopted goes through
/// `CSimpleButton::SetFontString 0x778d20`, which anchors it by the button's normal font justify
/// (`[button+0x390]`: LEFT, RIGHT, else CENTER), not the string's own, then links it to that font
/// (`0x779810`). FrameXML writes that justify with `<NormalFont justifyH=>`.
#[test]
fn a_lazily_made_label_is_anchored_by_the_normal_fonts_justify() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Font name="ProbeFont" font="Fonts\FRIZQT__.TTF" virtual="true">
               <FontHeight><AbsValue val="12"/></FontHeight>
             </Font>
             <Font name="ProbeFontRight" inherits="ProbeFont" justifyH="RIGHT" virtual="true"/>
             <Button name="LeftRowTemplate" virtual="true">
               <Size><AbsDimension x="104" y="16"/></Size>
               <NormalFont inherits="ProbeFont" justifyH="LEFT"/>
               <HighlightFont inherits="ProbeFont" justifyH="LEFT"/>
             </Button>
             <Button name="RightRowTemplate" virtual="true">
               <Size><AbsDimension x="104" y="16"/></Size>
               <NormalFont inherits="ProbeFontRight"/>
             </Button>
             <Button name="PlainRowTemplate" virtual="true">
               <Size><AbsDimension x="104" y="16"/></Size>
               <NormalFont inherits="ProbeFont"/>
             </Button>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    s.run(
        r#"
        left = CreateFrame("Button", "LeftRow", nil, "LeftRowTemplate")
        left:SetPoint("CENTER", nil, "CENTER", 0, 0)
        right = CreateFrame("Button", "RightRow", nil, "RightRowTemplate")
        right:SetPoint("CENTER", nil, "CENTER", 0, 0)
        plain = CreateFrame("Button", "PlainRow", nil, "PlainRowTemplate")
        plain:SetPoint("CENTER", nil, "CENTER", 0, 0)
        "#,
    )
    .unwrap();
    // `<NormalFont>` alone creates no label (the reference's LoadXML leaves `+0x338` null), so
    // `GetFontString()` is nil until text is set.
    assert!(s
        .eval::<bool>("return left:GetFontString() == nil")
        .unwrap());

    s.run(r#"left:SetText("Say") right:SetText("12") plain:SetText("Okay")"#)
        .unwrap();
    let point = |s: &UiScript, who: &str| -> (String, String, f64, f64) {
        let (p, _rel, rp, x, y): (String, mlua::Value, String, f64, f64) = s
            .eval(&format!("return {who}:GetFontString():GetPoint(1)"))
            .unwrap();
        (p, rp, x, y)
    };
    assert_eq!(
        point(&s, "left"),
        ("LEFT".into(), "LEFT".into(), 0.0, 0.0),
        "the element-level justifyH on <NormalFont> anchors the lazy label LEFT"
    );
    assert_eq!(
        point(&s, "right"),
        ("RIGHT".into(), "RIGHT".into(), 0.0, 0.0),
        "a justify the normal font INHERITS from its object counts the same"
    );
    assert_eq!(
        point(&s, "plain"),
        ("CENTER".into(), "CENTER".into(), 0.0, 0.0),
        "no justify anywhere → the else leg, CENTER"
    );
    for who in ["left", "right", "plain"] {
        assert_eq!(
            s.eval::<i64>(&format!("return {who}:GetFontString():GetNumPoints()"))
                .unwrap(),
            1
        );
    }
    assert_eq!(
        s.eval::<String>("return left:GetFontString():GetJustifyH()")
            .unwrap(),
        "LEFT"
    );
    assert!(s
        .eval::<bool>("return left:GetFontString():GetFontObject() == ProbeFont")
        .unwrap());

    s.run(
        r#"
        bare = left:CreateFontString(nil, "OVERLAY")
        bare:ClearAllPoints()
        left:SetFontString(bare)
        "#,
    )
    .unwrap();
    assert_eq!(
        point(&s, "left"),
        ("LEFT".into(), "LEFT".into(), 0.0, 0.0),
        "SetFontString anchors by the normal font's justify too"
    );

    // A later `SetTextFontObject` re-links the normal font but does not re-anchor.
    s.run("left:SetTextFontObject(ProbeFontRight)").unwrap();
    assert_eq!(point(&s, "left"), ("LEFT".into(), "LEFT".into(), 0.0, 0.0));
}

/// A label's own `SetFont` clears its font inherit bit (the reference's `FONTINSTANCE+0x2c`
/// inherit mask, a FontString's `+0xd4`), and nothing sets it back, so its face, height and
/// outline outlast every state font re-point while its other axes still inherit.
#[test]
fn a_button_labels_own_setfont_survives_the_state_font_repoint() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "TemplateFont",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(12.0),
            color: Some([1.0, 0.82, 0.0, 1.0]),
            outline: Outline::None,
            ..Default::default()
        },
    );
    s.run(
        r#"
        b = CreateFrame("Button", "SkinnedBtn")
        b:SetPoint("CENTER", 0, 0); b:SetWidth(100); b:SetHeight(20)
        b:SetText("Label")
        b:SetTextFontObject("TemplateFont")
        b:GetFontString():SetFont("Interface\\Addons\\Skin\\Fonts\\porky.ttf", 18, "OUTLINE")
    "#,
    )
    .unwrap();
    let painted = |s: &mut crate::script::UiScript| {
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text {
                    text: Some(t),
                    ref font,
                    font_height,
                    outline,
                    ..
                } if t == "Label" => Some((font.clone(), font_height, outline)),
                _ => None,
            })
            .expect("label text quad")
    };
    assert_eq!(
        painted(&mut s),
        (
            Some("Interface\\Addons\\Skin\\Fonts\\porky.ttf".to_string()),
            Some(18.0),
            Outline::Normal
        ),
        "the label's own SetFont severs face, height AND outline from the state font object"
    );
    s.run("b:Disable()").unwrap();
    assert_eq!(
        painted(&mut s),
        (
            Some("Interface\\Addons\\Skin\\Fonts\\porky.ttf".to_string()),
            Some(18.0),
            Outline::Normal
        ),
        "…and a disable re-points the instance without restoring what the label severed"
    );
    s.resolve();
    let color = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(t),
                color,
                ..
            } if t == "Label" => Some(color),
            _ => None,
        })
        .expect("label text quad");
    assert_eq!(
        color,
        Some([1.0, 0.82, 0.0, 1.0]),
        "an axis the label never set still inherits — severance is per-axis"
    );
}

/// A state-texture setter forks on `lua_type(L, 2)` (`0x781970`): a Texture object is installed
/// into the slot (`0x781b0b` → `0x778fd0`), and nil clears the slot and destroys its texture
/// (`0x781b5a`), which merely unhooked would draw in every state.
#[test]
fn a_state_texture_slot_takes_an_object_and_a_nil() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        b = CreateFrame("Button", "SlotBtn")
        b:SetPoint("BOTTOMLEFT", 0, 0); b:SetWidth(100); b:SetHeight(100)
        b:SetNormalTexture("Interface\\N.blp")
        -- Bongos' own idiom: build the highlight yourself and hand the object over.
        hl = b:CreateTexture()
        hl:SetTexture("Interface\\OWN.blp")
        hl:SetAllPoints(b)
        b:SetHighlightTexture(hl)
    "#,
    )
    .unwrap();
    s.resolve();

    let drawn = |s: &UiScript| -> Vec<String> {
        s.extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
            .collect()
    };

    assert_eq!(drawn(&s), vec!["Interface\\N.blp".to_string()]);
    assert!(
        s.eval::<bool>("return b:GetHighlightTexture() == hl")
            .unwrap(),
        "the getter must hand back the object that was installed, not a slot region of our own"
    );
    s.mouse_move(50.0, 50.0); // hover, so the highlight slot draws
    s.resolve();
    assert_eq!(
        drawn(&s),
        vec![
            "Interface\\N.blp".to_string(),
            "Interface\\OWN.blp".to_string()
        ]
    );

    s.run("b:SetHighlightTexture(nil) b:SetNormalTexture(nil)")
        .unwrap();
    s.resolve();
    assert!(
        drawn(&s).is_empty(),
        "a cleared slot still draws: {:?}",
        drawn(&s)
    );
    assert!(
        s.eval::<bool>("return b:GetHighlightTexture() == nil")
            .unwrap(),
        "the slot must read empty after nil"
    );
}

/// `SetButtonState(state[, lock])` always writes the lock (`[+0x32c]`), and the mouse-up edge
/// (`0x7793de`) un-presses an unlocked button; `Tablet-2.0.lua:1645` pushes a row and never
/// releases it itself.
#[test]
fn an_unlocked_scripted_push_is_released_by_the_next_mouse_release() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        row = CreateFrame("Button", "TabletRow")
        row:SetPoint("BOTTOMLEFT", 0, 0); row:SetWidth(100); row:SetHeight(100)
        row:SetNormalTexture("Interface\\RowN.blp")
        row:SetPushedTexture("Interface\\RowP.blp")
        row:SetButtonState("PUSHED")            -- Tablet-2.0's call, verbatim: no lock argument
    "#,
    )
    .unwrap();
    s.resolve(); // a press only reaches a frame with a resolved rect
    assert_eq!(
        s.eval::<String>("return TabletRow:GetButtonState()")
            .unwrap(),
        "PUSHED"
    );

    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return TabletRow:GetButtonState()")
            .unwrap(),
        "NORMAL",
        "the release un-pushes an unlocked scripted push — the row does not stay depressed"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetButtonState`'s third argument (`0x780270`, default 0) locks the state against the mouse,
/// PUSHED or NORMAL; `MainMenuBarMicroButtons.lua:22` holds a micro button PUSHED this way.
#[test]
fn a_locked_state_ignores_the_mouse_and_enable_disable_clears_the_lock() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        micro = CreateFrame("Button", "MicroButton")
        micro:SetPoint("BOTTOMLEFT", 0, 0); micro:SetWidth(100); micro:SetHeight(100)
        micro:SetButtonState("PUSHED", 1)       -- MainMenuBarMicroButtons.lua, verbatim
        pin = CreateFrame("Button", "PinnedNormal")
        pin:SetPoint("BOTTOMLEFT", 200, 0); pin:SetWidth(100); pin:SetHeight(100)
        pin:SetButtonState("NORMAL", 1)
    "#,
    )
    .unwrap();
    s.resolve();

    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return MicroButton:GetButtonState()")
            .unwrap(),
        "PUSHED",
        "a locked push survives a whole click"
    );

    s.mouse_move(250.0, 50.0);
    s.mouse_button(250.0, 50.0, "LeftButton", true);
    assert_eq!(
        s.eval::<String>("return PinnedNormal:GetButtonState()")
            .unwrap(),
        "NORMAL",
        "…and a locked NORMAL cannot be pushed by the mouse at all"
    );
    s.mouse_button(250.0, 50.0, "LeftButton", false);

    // `0x779160` passes `locked = 0` on both arms, so Enable and Disable unlock.
    s.run("MicroButton:Disable() MicroButton:Enable()").unwrap();
    assert_eq!(
        s.eval::<String>("return MicroButton:GetButtonState()")
            .unwrap(),
        "NORMAL",
        "Disable() wrote DISABLED, Enable() wrote NORMAL — and both cleared the lock"
    );
    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert_eq!(
        s.eval::<String>("return MicroButton:GetButtonState()")
            .unwrap(),
        "PUSHED",
        "so the mouse reaches it again"
    );
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The enter and leave notifies (`0x779490`/`0x7794e0`) read the button state (`[+0x328]`) only as
/// a DISABLED guard and never write it; before the release, only a drag-threshold crossing on a
/// drag-registered frame un-presses a held button.
#[test]
fn the_hover_is_not_an_input_to_the_press_state() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        b = CreateFrame("Button", "HeldButton")
        b:SetPoint("BOTTOMLEFT", 0, 0); b:SetWidth(100); b:SetHeight(100)
        b:SetNormalTexture("Interface\\HeldN.blp")
        b:SetPushedTexture("Interface\\HeldP.blp")
    "#,
    )
    .unwrap();
    s.resolve();
    let shows = |s: &UiScript, path: &str| {
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
    };

    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert!(shows(&s, "Interface\\HeldP.blp"), "the press pushed it");

    s.mouse_move(500.0, 400.0);
    assert!(
        shows(&s, "Interface\\HeldP.blp"),
        "…and it stays pushed off the rect: the leave notify does not write the state"
    );
    assert_eq!(
        s.eval::<String>("return HeldButton:GetButtonState()")
            .unwrap(),
        "PUSHED"
    );

    // The release un-presses it off the button too: `0x7792d0` runs no hit test.
    s.mouse_button(500.0, 400.0, "LeftButton", false);
    assert!(
        shows(&s, "Interface\\HeldN.blp"),
        "the release is the edge, wherever the cursor is"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `CSimpleButton`'s hide override (`+0x34` = `0x7791e0`) un-presses the button, then runs the
/// base notify so `<OnHide>` still fires.
#[test]
fn hiding_a_held_button_un_presses_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        h = CreateFrame("Button", "HidButton")
        h:SetPoint("BOTTOMLEFT", 0, 0); h:SetWidth(100); h:SetHeight(100)
    "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert_eq!(
        s.eval::<String>("return HidButton:GetButtonState()")
            .unwrap(),
        "PUSHED"
    );
    s.run("HidButton:Hide()").unwrap();
    assert_eq!(
        s.eval::<String>("return HidButton:GetButtonState()")
            .unwrap(),
        "NORMAL",
        "the hide edge un-pressed it"
    );
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `Button:GetTextColor()` returns four values, r, g, b, a (`0x781100`, table `0x879d00`).
#[test]
fn button_get_text_color_answers_four_values_through_the_state_font() {
    let s = script();
    s.register_font_object(
        "BtnGold",
        FontObject {
            color: Some([1.0, 0.82, 0.0, 1.0]),
            height: Some(12.0),
            ..Default::default()
        },
    );
    s.run(
        r#"
        Plain = CreateFrame("Button", "PlainBtn")
        Themed = CreateFrame("Button", "ThemedBtn")
        Themed:SetTextFontObject("BtnGold")
        "#,
    )
    .unwrap();

    assert_eq!(
        s.arity("PlainBtn:GetTextColor()").unwrap(),
        4,
        "arity 4 — not 3, the plausible wrong answer"
    );
    let kinds: String = s
        .eval(
            r#"local r,g,b,a = PlainBtn:GetTextColor()
               return type(r)..","..type(g)..","..type(b)..","..type(a)"#,
        )
        .unwrap();
    assert_eq!(kinds, "number,number,number,number");

    // Nothing set: the untinted white the other colour getters here answer.
    let plain: Vec<f32> = (1..=4)
        .map(|i| {
            let discards = "_, ".repeat(i - 1);
            s.eval::<f32>(&format!(
                "local {discards}v = PlainBtn:GetTextColor() return v"
            ))
            .unwrap()
        })
        .collect();
    assert_eq!(plain, vec![1.0, 1.0, 1.0, 1.0]);

    // Unset locally, it reads through the normal state's font object, as `Button:GetFont` does.
    let themed: Vec<f32> = (1..=4)
        .map(|i| {
            let discards = "_, ".repeat(i - 1);
            s.eval::<f32>(&format!(
                "local {discards}v = ThemedBtn:GetTextColor() return v"
            ))
            .unwrap()
        })
        .collect();
    assert_eq!(themed, vec![1.0, 0.82, 0.0, 1.0]);

    s.run("ThemedBtn:SetTextColor(0.1, 0.2, 0.3, 0.4)").unwrap();
    let set: Vec<f32> = (1..=4)
        .map(|i| {
            let discards = "_, ".repeat(i - 1);
            s.eval::<f32>(&format!(
                "local {discards}v = ThemedBtn:GetTextColor() return v"
            ))
            .unwrap()
        })
        .collect();
    assert_eq!(set, vec![0.1, 0.2, 0.3, 0.4]);

    // A CheckButton reaches it through Button's table.
    s.run(r#"CreateFrame("CheckButton", "ChkColorBtn")"#)
        .unwrap();
    assert_eq!(s.arity("ChkColorBtn:GetTextColor()").unwrap(), 4);
}
