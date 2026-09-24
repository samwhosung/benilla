//! The reference's `UIMenu` kit (`Interface\FrameXML\UIMenu.xml` + `UIMenu.lua`) driven as the
//! chat bubble drives it — a row is a `UIMenuButtonTemplate` whose label is the Button's OWN text
//! (`UIMenu_AddButton` → `button:SetText(text)`, no `<ButtonText>` in the template) and whose
//! shortcut is a sibling FontString anchored `RIGHT`. `UIMenu_Initialize` fixes every row at
//! 104×16 and the menu at 128 wide; nothing auto-sizes in 1.12.
//!
//! What places the label is the template's `<NormalFont inherits="GameFontNormal"
//! justifyH="LEFT"/>`: `SetText`'s lazy label creation funnels through the adopter
//! `CSimpleButton::SetFontString 0x778d20`, which anchors an unanchored label to the button by the
//! NORMAL embedded font's justify word (`[button+0x390]`: LEFT→LEFT, RIGHT→RIGHT, else CENTER).
//! Decision 1996.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The chat stack the menu lives in — the same chain `chat_tests` loads for the window, with the
/// fixed-advance font so the rows have real widths in the tick that asks.
fn chat_menu() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
        r"Interface\FrameXML\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\GlobalStrings.lua",
        r"Interface\FrameXML\BasicControls.xml",
        r"Interface\FrameXML\LocaleProperties.lua",
        r"Interface\FrameXML\StaticPopup.xml",
        r"Interface\FrameXML\GameTooltip.xml",
        r"Interface\FrameXML\UIMenu.xml",
        r"Interface\FrameXML\ChatFrame.xml",
        r"Interface\FrameXML\UIDropDownMenu.xml",
        r"Interface\FrameXML\FloatingChatFrame.xml",
    ] {
        load_xml(&s, f);
    }
    s.set_text_measurer(Box::new(super::FixedWidthFont(8.0)));
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    let _ = s.errors();
    s
}

fn edges(s: &mut UiScript, name: &str) -> (f32, f32) {
    s.eval::<(f32, f32)>(&format!("return {name}:GetLeft(), {name}:GetRight()"))
        .unwrap_or_else(|e| panic!("{name}'s rect: {e}"))
}

/// **The label hugs the row's LEFT edge and the shortcut its RIGHT** — the reference's own
/// anchoring, so "Macro" and "/macro" never meet (B-report: the labels drew centred and the
/// Macro row read "Macro/macro").
#[test]
fn chat_menu_rows_left_align_their_label_and_right_align_their_shortcut() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_menu();
    s.run("ChatMenu:Show()").unwrap();
    assert!(s.errors().is_empty(), "opening it raises: {:?}", s.errors());
    s.resolve();

    // The Macro row is the tenth `UIMenu_AddButton` in `ChatMenu_OnLoad` (ChatFrame.lua l.2301).
    assert_eq!(
        s.eval::<String>("return ChatMenuButton10:GetText()")
            .unwrap(),
        "Macro"
    );
    assert_eq!(
        s.eval::<String>("return ChatMenuButton10ShortcutText:GetText()")
            .unwrap(),
        "/macro"
    );
    // The control: `UIMenu_Initialize` fixed the row at 104 wide; nothing here may resize it.
    let (row_l, row_r) = edges(&mut s, "ChatMenuButton10");
    assert_eq!(row_r - row_l, 104.0, "UIMENU_BUTTON_WIDTH");

    // The label's implicit anchor is LEFT→LEFT (0,0) — the adopter reading the template's
    // `<NormalFont justifyH="LEFT"/>` — so its left edge IS the row's.
    let (p, rp, x, y): (String, String, f32, f32) = s
        .eval(
            "local p, _, rp, x, y = ChatMenuButton10:GetFontString():GetPoint(1) \
             return p, rp, x, y",
        )
        .unwrap();
    assert_eq!(
        (p.as_str(), rp.as_str(), x, y),
        ("LEFT", "LEFT", 0.0, 0.0),
        "the label is anchored by the normal font's justify"
    );
    let (label_l, label_r) = edges(&mut s, "ChatMenuButton10:GetFontString()");
    assert_eq!(label_l, row_l, "the label starts at the row's left edge");
    // The shortcut is the template's own `<Anchor point="RIGHT"/>` string.
    let (short_l, short_r) = edges(&mut s, "ChatMenuButton10ShortcutText");
    assert_eq!(short_r, row_r, "the shortcut ends at the row's right edge");
    // And the two never overlap — with 8-px glyphs "Macro" spans 40 and "/macro" 48 of the 104,
    // which a centred label (32..72 against 56..104) fails.
    assert!(
        label_r <= short_l,
        "label {label_l}..{label_r} collides with shortcut {short_l}..{short_r}"
    );

    // Every row, not only the one the report named.
    for i in 1..=s.eval::<i64>("return ChatMenu.numButtons").unwrap() {
        let row = format!("ChatMenuButton{i}");
        let (rl, _) = edges(&mut s, &row);
        let (ll, _) = edges(&mut s, &format!("{row}:GetFontString()"));
        assert_eq!(ll, rl, "row {i}'s label starts at its left edge");
    }
}

/// The label's *query* surface agrees with its paint: a lazily created label answers the normal
/// font's justify and object (the adopter links it on the spot — `0x779810`), not a fresh
/// FontString's defaults.
#[test]
fn a_menu_rows_label_reports_the_normal_fonts_justify_and_object() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_menu();
    s.run("ChatMenu:Show()").unwrap();
    assert_eq!(
        s.eval::<String>("return ChatMenuButton1:GetFontString():GetJustifyH()")
            .unwrap(),
        "LEFT"
    );
    assert!(
        s.eval::<bool>("return ChatMenuButton1:GetFontString():GetFontObject() == GameFontNormal")
            .unwrap(),
        "the label inherits the button's normal font object"
    );
}
