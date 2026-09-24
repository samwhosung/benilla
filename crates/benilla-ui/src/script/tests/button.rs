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
    // Duck-typing honesty across the class chain (the reference's per-type method sets).
    assert!(s.eval::<bool>("return f.SetText == nil").unwrap());
    assert!(s.eval::<bool>("return f.SetChecked == nil").unwrap());
    assert!(s.eval::<bool>("return b.SetText ~= nil").unwrap());
    assert!(s.eval::<bool>("return b.SetChecked == nil").unwrap());
    assert!(s.eval::<bool>("return b.SetValue == nil").unwrap());
    assert!(s.eval::<bool>("return cb.SetChecked ~= nil").unwrap());
    assert!(s.eval::<bool>("return cb.SetNormalTexture ~= nil").unwrap());
    // Buttons are mouse-enabled by construction (the client ctor enables input).
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

    // Idle: normal only (pushed/disabled suppressed; not hovered ⇒ no highlight).
    assert_eq!(visible(&s), vec!["Interface\\N.blp".to_string()]);
    // The ButtonText always draws.
    assert!(s
        .extract()
        .iter()
        .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Go")));

    // Hover: normal + additive highlight.
    s.mouse_move(50.0, 50.0);
    let v = visible(&s);
    assert!(
        v.contains(&"Interface\\N.blp".to_string()) && v.contains(&"Interface\\H.blp".to_string())
    );

    // Held down over it: pushed replaces normal (highlight still hovering).
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    let v = visible(&s);
    assert!(
        v.contains(&"Interface\\P.blp".to_string()) && !v.contains(&"Interface\\N.blp".to_string())
    );
    s.mouse_button(50.0, 50.0, "LeftButton", false);

    // Disabled: disabled texture only, and no highlight even under the cursor.
    s.run("StateBtn:Disable()").unwrap();
    assert_eq!(visible(&s), vec!["Interface\\D.blp".to_string()]);
    // 0, not false, and not nil — the number the real client answers.
    assert_eq!(s.eval::<i64>("return StateBtn:IsEnabled()").unwrap(), 0);
    s.run("StateBtn:Enable()").unwrap();
    assert_eq!(s.eval::<String>("return StateBtn:GetText()").unwrap(), "Go");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The shown texture is STICKY: a state with no art of its own changes nothing.**
///
/// `SetState 0x779790` gates the hide-old step *and* the show-new step on the NEW state's slot
/// being non-null, so the shown pointer `+0x4c4`
/// only ever moves onto a real texture — it is never cleared, and there is no fallback path in the
/// function at all. Three consequences, and they are one mechanism seen from three sides:
///
/// - a press with no `<PushedTexture>` keeps the normal art (which our old model hard-coded as a
///   `pushed.or(normal)` fallback — the special case *was* this rule, seen from one side);
/// - **`Disable()` with no `<DisabledTexture>` keeps the normal art** — B369, where the reference's
///   `ReputationDetailAtWarCheckBox` stays a visible box for a faction whose war flag is locked
///   while ours went to a bare grey label;
/// - a state texture assigned while the button is in a *different* state is not displayed
///   (`0x778fd0`'s `idx == [this+0x328]` gate), so a button disabled *before* it is given normal
///   art draws nothing — the two disabled cases differ only in whether anything was ever shown.
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

    // Pressed with no PushedTexture: SetState(2) finds a null slot, so nothing moves.
    s.mouse_move(50.0, 50.0);
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert_eq!(visible(&s, "StickyBtn"), n, "a press with no pushed art");
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    s.mouse_move(400.0, 400.0);

    // Disabled with no DisabledTexture: the same null slot, the same nothing. **B369.**
    s.run("StickyBtn:Disable()").unwrap();
    assert_eq!(
        visible(&s, "StickyBtn"),
        n,
        "a disable with no disabled art leaves the box on screen"
    );
    // Giving it disabled art NOW does show it — `0x778fd0` pushes to `+0x4c4` because the slot
    // being written IS the current state's.
    s.run("StickyBtn:SetDisabledTexture(\"Interface\\\\D.blp\")")
        .unwrap();
    assert_eq!(
        visible(&s, "StickyBtn"),
        vec!["Interface\\D.blp".to_string()],
        "and real disabled art displaces it on the spot"
    );
    s.run("StickyBtn:Enable()").unwrap();
    assert_eq!(visible(&s, "StickyBtn"), n, "back to normal on Enable");

    // The other side of the same gate: a button disabled BEFORE it has normal art shows nothing,
    // because the write lands in a slot that is not the current state's.
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

/// **`Disable()` switches off the HIGHLIGHT draw LAYER, not just the HighlightTexture.**
///
/// The helper both `Enable`/`Disable` and the constructor reach (`0x779160`) does two things: the
/// `SetState` above, and `0x7791bb push 4; call 0x76a730` — the per-layer enable for layer 4,
/// written into the same `[frame+0x198]` array `Enable/DisableDrawLayer` writes. So a region the
/// button owns in HIGHLIGHT that is not its HighlightTexture goes dark with it, and an explicit
/// `EnableDrawLayer` brings the whole layer back on a still-disabled button. Our old rule gated
/// the HighlightTexture alone on `enabled`, which agreed on the common case and not on this one.
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

    // Hovered and enabled: normal art, the button's own HIGHLIGHT region, and the HighlightTexture.
    s.mouse_move(50.0, 50.0);
    let v = art(&s);
    for want in ["Interface\\N.blp", "Interface\\Own.blp", "Interface\\H.blp"] {
        assert!(
            v.contains(&want.to_string()),
            "{want} draws while enabled: {v:?}"
        );
    }

    // Disabled under the same cursor: the layer is off, so BOTH highlight-layer regions go — and
    // the normal art stays, because no state texture moved (the sticky rule).
    s.run("LayerBtn:Disable()").unwrap();
    assert_eq!(
        art(&s),
        vec!["Interface\\N.blp".to_string()],
        "the whole HIGHLIGHT layer goes with the disable"
    );

    // The layer is a layer: turning it back on lights both again, disabled or not.
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

    // …and Enable() writes the same one array, so it undoes an addon's DisableDrawLayer.
    s.run(r#"LayerBtn:DisableDrawLayer("HIGHLIGHT") LayerBtn:Enable()"#)
        .unwrap();
    let v = art(&s);
    assert!(
        v.contains(&"Interface\\Own.blp".to_string()),
        "one array, one writer: Enable() clears the addon's own disable too: {v:?}"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **A right-click lights a button up too** — the director's report, and `0x77924b`'s law.
///
/// `CButton::OnMouseDown` gates the PushedTexture on `[this+0x330] & (m | m << 8)`: the button
/// registered for that mouse button in **either** variant. Not on the click firing, not on the
/// handler doing anything — which is why right-clicking an action, spellbook or pet slot flashes,
/// while right-clicking a default `LeftButtonUp` button does not.
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

    // The bar slot lights under EITHER button.
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

    // The plain button lights under the left only — `0x77924b` is a real gate, not a formality.
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

/// `SetButtonState`/`GetButtonState` (`0x780270`/`0x780180`) — the scripted press state the ref's
/// `ActionButtonDown/Up` keybind pair drives: PUSHED shows the pushed texture with no mouse
/// involved, NORMAL restores, unknown states error, and a disabled button answers DISABLED.
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
    // Unknown state: a runtime error, not a silent no-op.
    assert!(s.run(r#"PushBtn:SetButtonState("SIDEWAYS")"#).is_err());
    // Disabled wins the read (Enable/Disable owns that state, not SetButtonState).
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

    // Physical click: the checked state flips BEFORE OnClick (the documented contract).
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 1);
    // `seen_checked` is whatever `GetChecked()` handed the handler — the NUMBER 1 (1830).
    assert!(s.eval::<bool>("return seen_checked == 1").unwrap());
    assert!(s.eval::<bool>("return Toggler:GetChecked()").unwrap());

    // Programmatic Click() rides the same path: toggles back off.
    s.run("Toggler:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 2);
    assert!(!s.eval::<bool>("return Toggler:GetChecked()").unwrap());

    // Disabled: the click is swallowed — no OnClick, no toggle.
    s.run("Toggler:Disable()").unwrap();
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    s.mouse_button(50.0, 50.0, "LeftButton", false);
    s.run("Toggler:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 2);
    assert!(!s.eval::<bool>("return Toggler:GetChecked()").unwrap());

    // SetChecked takes 1/true/nil (1.12 and Era forms).
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

    // A right-click (press+release, same frame) reaches OnMouseDown/OnMouseUp but never OnClick —
    // the client's own default registered-click set is {"LeftButtonUp"} only.
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

    // RegisterForClicks replaces the set with its whole vararg list — LeftButtonUp still fires.
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

    // The press alone fires OnClick (down=true) and toggles Checked exactly once.
    s.mouse_button(50.0, 50.0, "LeftButton", true);
    assert_eq!(s.eval::<i64>("return clicks").unwrap(), 1);
    assert!(s.eval::<bool>("return seen_down == true").unwrap());
    assert!(s.eval::<bool>("return QuickSell:GetChecked()").unwrap());

    // The release does not fire again — "LeftButtonUp" is no longer in the registered set
    // (RegisterForClicks replaced it, it did not add to it).
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
    // The highlight carries the ADD contract (the client's SetHighlightTexture default).
    assert!(matches!(
        &find("Interface\\Hi.blp").content,
        QuadContent::Texture { additive: true, .. }
    ));
    // A fresh state texture gets the creation-path implicit SetAllPoints (the
    // reference's string setters anchor a freshly built texture to the button outright), whose
    // two corners pin all four edges: the later 64px size is structurally unread and the ring
    // FILLS the 36px button.
    let r = find("Interface\\Ring.blp").rect.unwrap();
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 136.0, 100.0, 136.0)
    );
    // The real quickslot-overhang idiom is an ANCHOR, not an anchorless size (ActionButton's
    // 66×66 UI-Quickslot2 authors `<Anchor point="CENTER">`): one CENTER point replaces only its
    // own slot, but clearing first leaves the single anchor + the 64px size → the centered
    // overhang, 100..136 → 86..150.
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
    // SetBlendMode overrides back to straight alpha.
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

/// The per-state label fonts (`SetTextFontObject`/`SetDisabledFontObject`, XML `<NormalFont>` etc.):
/// the ButtonText re-points to the CURRENT state's font object at extract — gold while enabled,
/// gray after Disable() — with no Lua repaint (the client's UIPanelButtonTemplate label behavior).
/// An explicit SetTextColor survives the re-point (the client's explicitly-set mask).
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

    // An explicit SetTextColor wins over the state font's color (explicitly-set mask).
    s.run("b:GetFontString():SetTextColor(0.1, 0.2, 0.3)")
        .unwrap();
    let c = label_color(&mut s).expect("colored");
    assert!((c[0] - 0.1).abs() < 1e-6 && (c[1] - 0.2).abs() < 1e-6);
}

/// **The highlighted label is its own font instance — and `LockHighlight` enters it.** The two
/// halves of the recipe-list look the director asked for: a row turns white under the cursor, and
/// the SELECTED row stays white with the cursor elsewhere.
///
/// Both are one mechanism in the reference. `ClassTrainerSkillButtonTemplate` (the base under
/// TradeSkill's, Craft's and the class trainer's rows) declares
/// `<HighlightFont inherits="GameFontHighlight">`, and each window paints its rows by difficulty
/// with `skillButton:SetTextColor(...)` — orange, yellow, green, gray. If a `SetTextColor` reached
/// the highlighted state, hovering a row would leave it orange; it does not, because the highlight
/// instance is a different font instance and the normal one's colour cannot reach it. That is the
/// first assertion below.
///
/// The second is `LockHighlight`. `TradeSkillFrame_Update` blanks a recipe row's highlight texture
/// (`getglobal("TradeSkillSkill"..i.."Highlight"):SetTexture("")`, Blizzard_TradeSkillUI.lua l.131)
/// and *then* locks the row it selected (l.144) — with no texture left to pin, the lock can only be
/// reaching the label. Craft (Blizzard_CraftUI.lua l.234) and the trainer
/// (Blizzard_TrainerUI.lua l.183) do the same.
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

    // Cursor away again, but the row is the selected one.
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

    // A button with no highlight instance keeps the normal one whole, hover or not — the shape
    // every plain labelled Button in the corpus has.
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

/// **`Button:GetTextWidth` / `GetTextHeight`** — the reference's own Button text-extent readers
/// (`0x782290` / `0x782390`; present on Button, and `GetStringWidth` **absent**). Both forward to
/// the label FontString's own extent slots, which is what this asserts: answer a host measure for
/// the label, and the BUTTON must report it.
///
/// `Bagnon_Forever/database/ui.lua:61` sizes its character-switch dropdown from
/// `button:GetTextWidth() + 40`, so a nil method took the whole dropdown down — the director could
/// not switch characters in Bagnon at all.
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

    // Unmeasured is 0 — the same "converges next frame" contract every other metric read has.
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

    // A Button with no label at all answers 0 rather than raising: the reference dereferences a
    // FontString pointer (`+0x338`) here that a bare CreateFrame("Button") leaves null, and what it
    // does then is not byte-read — so this takes the harmless number instead of guessing a crash.
    assert_eq!(
        s.eval::<f64>("return LabellessBtn:GetTextWidth()").unwrap(),
        0.0
    );

    // …and it is a BUTTON method: a plain Frame must not have grown one.
    assert!(
        s.eval::<bool>(r#"return CreateFrame("Frame").GetTextWidth == nil"#)
            .unwrap(),
        "GetTextWidth is Button's, not Region's (the reference's own split)"
    );
}

/// **`Button:SetFontString` adopts aggressively, and it RAISES** — `0x780a60`.
///
/// The first cut of this binding was a lenient no-op on a bad argument and a bare pointer swap on
/// a good one. Every line below is a clause of that binding that the plausible reading got wrong.
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

    // Adoption binds the string as the label the whole rest of the surface reads.
    s.run("b:SetFontString(fs)").unwrap();
    assert!(s.eval::<bool>("return b:GetFontString() == fs").unwrap());
    s.run("b:SetText(\"Okay\")").unwrap();
    assert_eq!(s.eval::<String>("return fs:GetText()").unwrap(), "Okay");
    assert_eq!(s.eval::<String>("return b:GetText()").unwrap(), "Okay");

    // Re-parented — always, whatever it was parented to before.
    assert!(s.eval::<bool>("return fs:GetParent() == b").unwrap());
    // The draw layer is forced to ARTWORK on the same call (`0x77fd10(parent, 2, 1)`), and that
    // half is NOT asserted here for a stated reason: `GetDrawLayer` is one of the client's nine
    // per-leaf verbs we deliberately do not install (`script::REGION_LEAF_SHARED`'s own note —
    // "absent is absent"), so there is no Lua way to read it back yet. The binding does it; when
    // that verb lands, this is where it gets pinned.

    // Adopting the SAME string again is a total no-op — the first compare in the client, and the
    // one path that must not run the destroy below.
    s.run("b:SetFontString(fs)").unwrap();
    assert!(s.eval::<bool>("return b:GetFontString() == fs").unwrap());
    assert_eq!(s.eval::<String>("return fs:GetText()").unwrap(), "Okay");

    // A SECOND adoption DESTROYS the first label rather than orphaning it.
    s.run("fs2 = b:CreateFontString(\"MyLabel2\", \"OVERLAY\") b:SetFontString(fs2)")
        .unwrap();
    assert!(s.eval::<bool>("return b:GetFontString() == fs2").unwrap());
    let err = s.run("fs:GetText()").unwrap_err().to_string();
    assert!(
        err.contains("stale") || err.contains("invalid"),
        "the replaced label must be destroyed, not left live: {err}"
    );

    // Three distinct raises, all naming the BUTTON — never a silent no-op, and `nil` cannot clear
    // the label from Lua at all.
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
    // A TEXTURE is the near-miss the type gate exists for: it is a Region, and it still fails.
    s.run("tex = b:CreateTexture(nil, \"ARTWORK\")").unwrap();
    let err = s.run("b:SetFontString(tex)").unwrap_err().to_string();
    assert!(
        err.contains("Wrong object type, expected fontstring"),
        "{err}"
    );
    // …and none of that changed the label.
    assert!(s.eval::<bool>("return b:GetFontString() == fs2").unwrap());
}

/// Adoption anchors the string **only when it has none of its own** — and leaves an already
/// anchored one exactly where the caller put it.
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

    // The unanchored one gets the justify-derived point, to the matching point on the button.
    s.run("b:SetFontString(bare)").unwrap();
    assert_eq!(s.eval::<i64>("return bare:GetNumPoints()").unwrap(), 1);
    let (p, _rel, rp, x, y): (String, mlua::Value, String, f64, f64) =
        s.eval("return bare:GetPoint(1)").unwrap();
    assert_eq!(
        (p.as_str(), rp.as_str(), x, y),
        ("CENTER", "CENTER", 0.0, 0.0)
    );
}

/// A label the button makes for itself (`SetText`'s lazy creator `0x778dc0`) or adopts
/// (`SetFontString`) is anchored by the BUTTON's normal font justify, not by the fresh string's
/// own ctor-default word: both funnel through `CSimpleButton::SetFontString 0x778d20`, whose
/// unanchored-label leg reads `[button+0x390]` — the NORMAL embedded `CSimpleFont`'s justify —
/// LEFT→LEFT / RIGHT→RIGHT / else CENTER, then links the label to that font (`0x779810`).
/// Decision 1996.
///
/// `<NormalFont justifyH=>` is what writes that word in FrameXML (`UIMenuButtonTemplate`,
/// `MailFrame.xml`'s RIGHT money button), so this is the chat menu's "Macro/macro" in miniature.
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
    // `<NormalFont>` alone creates no label — the reference's LoadXML never touches `+0x338` on
    // that leg — so `GetFontString()` is nil until something sets text.
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
    // The link is applied on the spot: the label's own word and object answer the normal font's.
    assert_eq!(
        s.eval::<String>("return left:GetFontString():GetJustifyH()")
            .unwrap(),
        "LEFT"
    );
    assert!(s
        .eval::<bool>("return left:GetFontString():GetFontObject() == ProbeFont")
        .unwrap());

    // Adoption reads the same word: an unanchored string handed to the LEFT button seats LEFT.
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

    // A later `SetTextFontObject` re-links the normal font but does NOT re-anchor — the anchor
    // was decided at adoption and is an ordinary anchor from then on.
    s.run("left:SetTextFontObject(ProbeFontRight)").unwrap();
    assert_eq!(point(&s, "left"), ("LEFT".into(), "LEFT".into(), 0.0, 0.0));
}

/// **A label that set its own face keeps it — the severance mask covers every axis, not three of
/// six**.
///
/// `font_explicit` is the client's explicitly-set mask (`FONTINSTANCE+0x38`): an axis a widget
/// writes for *itself* severs inheritance from the font instance it reads, and is never restored
/// (see the `button_font` block in `script::extract`). `font::repaint` honours it on all seven
/// axes. The extract's per-state re-point honoured it on shadow, colour and both justifies — and
/// not on **face, height or outline**: the face and height read `fo.x.or(data.x)`, which makes the
/// object outrank an explicit `SetFont`, and the outline was written unconditionally. A
/// `<ButtonText>` that called `SetFont(path, h, "OUTLINE")` for itself therefore had all three
/// silently put back from the button's font object on the very next extract — every frame, so no
/// Lua could win the race.
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
    // …and it survives a state change, which is what re-runs the re-point.
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
    // The axes the label did NOT set still follow the object: the colour is the template's gold.
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

/// **A state-texture setter takes an OBJECT and takes nil**, not only a path — the reference's
/// `0x781970` forks on `lua_type(L, 2)` into four legs and benilla honoured one of them.
///
/// Both missing legs are silent no-ops rather than errors, which is why nothing caught them:
///
/// * **the object leg** (`0x781b0b` → `0x778fd0`) installs the handed Texture *into the slot*.
///   Three corpus addons build a highlight this way and every one of them drew nothing —
///   `Bongos/bar.lua:64-71`, `_Nameplates/_Nameplates.lua:217-222`,
///   `Quiver/Quiver.bundle.lua:8949-8953`. The idiom below is Bongos', verbatim in shape.
/// * **the nil leg** (`0x781b5a` → `0x778fd0` with 0) clears the slot and dtors what was in it.
///   `TheoryCraft/TheoryCraftUI.lua:215-217` strips a talent-rank button with three of these.
///   Clearing the *pointer* alone would be worse than the no-op: `region_visible` draws any region
///   that is not one of the button's slots unconditionally, so a merely unhooked state texture
///   would appear in every state instead of none — which is what this test's last assertion pins.
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

    // The handed object IS the slot now: it draws on hover and only on hover, exactly as a
    // path-loaded highlight does.
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

    // nil clears — and the cleared art is gone from every state, not merely unhooked.
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

/// **The unlocked scripted push is released by the next mouse release — Tablet-2.0's rows**.
///
/// `SetButtonState(state[, lock])` writes `[+0x32c]` unconditionally, and the mouse-up edge
/// `0x7793de` un-presses whenever `locked == 0`. Tablet-2.0 (`Tablet-2.0.lua:1645`) pushes a row it
/// finds `clicked` and calls `SetButtonState("NORMAL")` *nowhere in the library* — it relies on
/// exactly this. Ours kept a scripted push until Lua cleared it, so a Questie/FuBar/oRA2 row stayed
/// depressed for the rest of the session.
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

    // A press and release over the row: the release edge finds it PUSHED and unlocked.
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

/// **`lock` pins the state against the mouse — the micro buttons** (`0x780270`'s third index,
/// `GetBoolOrDefault` with default 0).
///
/// `MainMenuBarMicroButtons.lua` calls `SetButtonState("PUSHED", 1)` when its panel opens, and the
/// button must stay depressed through every press and release until the panel closes. The same
/// flag pins a NORMAL button *against* being pushed, which is the half a "sticky push" model
/// cannot express at all.
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

    // `0x779160` passes `locked = 0` on both arms, so Enable/Disable UNLOCK. The panel closing
    // with a plain `SetButtonState("NORMAL")` (flag defaulting to 0) does the same.
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

/// **Walking off a held button does NOT un-press it, and walking back on does not re-press it.**
///
/// The correction decision 2134 is built on. Our old model derived the press as
/// `(held && hovered) || pushed_state` and re-evaluated it whenever the hover moved; the
/// reference's enter and leave notifies (`0x779490`/`0x7794e0`) read `[+0x328]` only as a DISABLED
/// guard and **never write it** — proved by a census of every store to the field image-wide.
/// The only thing that un-presses a held button before its release is the drag-threshold crossing,
/// and only for a frame that registered for drag.
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

    // Walk the cursor right off it, still holding.
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

    // The release, off the button, still un-presses it — `0x7792d0` runs no hit test of its own.
    s.mouse_button(500.0, 400.0, "LeftButton", false);
    assert!(
        shows(&s, "Interface\\HeldN.blp"),
        "the release is the edge, wherever the cursor is"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **A button hidden while held comes back NORMAL** — `CSimpleButton`'s hide override
/// (`+0x34` = `0x7791e0`), which un-presses and then tail-jumps the base notify so `<OnHide>`
/// still fires. It hangs off the visibility transition, not off the hover, so it reaches a button
/// hidden nowhere near the cursor too.
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

/// `Button:GetTextColor()` — **FOUR** values, r/g/b/a (`0x781100`, table `0x879d00`, argc 1,
/// arity 4, kinds `(number,number,number,number)`). Three is the plausible wrong answer, and the
/// shapes table flags the name `name-not-unique` because seven tables register it — this asserts
/// the BUTTON one, on a Button, and its two inheritance legs.
///
/// Completeness rather than a live break: no corpus site has a Button receiver today (every
/// measured `GetTextColor` is on a FontString or a font object). It is here because the reference
/// registers it, so the widget shape gate can cover it from now on.
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

    // A button with nothing set anywhere is the untinted white every other colour getter here
    // (`GetVertexColor`, `FontString:GetTextColor`) answers.
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

    // With no local colour it reads THROUGH what the normal state inherits — the same leg
    // `Button:GetFont` takes, and the one a stock `GameMenuButtonTemplate` button relies on.
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

    // A local SetTextColor wins, alpha included, and round-trips.
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

    // A CheckButton reaches it through Button's table, as it does the rest of the trio.
    s.run(r#"CreateFrame("CheckButton", "ChkColorBtn")"#)
        .unwrap();
    assert_eq!(s.arity("ChkColorBtn:GetTextColor()").unwrap(), 4);
}
