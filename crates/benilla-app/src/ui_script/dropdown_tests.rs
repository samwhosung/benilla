//! The stock dropdown lists' own scripts (`UIDropDownMenu.xml`): OnHide closes the next level
//! and clears its `OPEN_DROPDOWNMENUS` entry, and list 1's OnLoad sets the default text height.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// Fonts for Button1's height, and GameTooltip.xml's `TOOLTIP_DEFAULT_COLOR` for the backdrop.
fn load_dropdown_kit(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
    ] {
        load_xml(s, file);
    }
}

/// The close lives on OnHide (`UIDropDownMenu.xml:15`), so every path that hides level 1 counts.
#[test]
fn hiding_the_parent_list_closes_an_open_submenu() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_dropdown_kit(&s);

    s.run("DropDownList1:Show(); DropDownList2:Show();")
        .unwrap();
    assert!(
        s.eval::<bool>("return DropDownList2:IsVisible()").unwrap(),
        "fixture: the submenu must be open before level 1 hides"
    );

    s.run("DropDownList1:Hide();").unwrap();

    assert!(
        !s.eval::<bool>("return DropDownList2:IsVisible()").unwrap(),
        "level 2 was left on screen after its parent list hid — the orphaned submenu"
    );
}

/// A unit menu writes its entry (`UnitPopup.lua:149`), and `UnitPopup_OnUpdate` walks the table.
#[test]
fn hiding_a_list_clears_its_open_menu_registry_entry() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_dropdown_kit(&s);

    s.run(
        r#"
        DropDownList1:Show();
        DropDownList2:Show();
        OPEN_DROPDOWNMENUS[1] = { which = "SELF", unit = "player" };
        OPEN_DROPDOWNMENUS[2] = { which = "SELF", unit = "player" };
    "#,
    )
    .unwrap();

    s.run("DropDownList2:Hide();").unwrap();
    assert!(
        s.eval::<bool>("return OPEN_DROPDOWNMENUS[2] == nil")
            .unwrap(),
        "level 2's registry entry survived its list hiding"
    );

    s.run("DropDownList1:Hide();").unwrap();
    assert!(
        s.eval::<bool>("return OPEN_DROPDOWNMENUS[1] == nil")
            .unwrap(),
        "level 1's registry entry survived its list hiding"
    );
}

/// `UIDropDownMenu.lua:16` declares the height nil; list 1's OnLoad sets it from Button1's font
/// (`UIDropDownMenu.xml:11`), so the test compares against that font, not a number.
#[test]
fn the_default_text_height_is_derived_from_button1_not_a_literal() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_dropdown_kit(&s);

    let (height, from_button): (Option<f64>, Option<f64>) = s
        .eval(
            r#"
            local _, buttonHeight = DropDownList1Button1NormalText:GetFont();
            return UIDROPDOWNMENU_DEFAULT_TEXT_HEIGHT, buttonHeight
        "#,
        )
        .unwrap();

    let from_button = from_button.expect("fixture: Button1's NormalText must report a font height");
    assert_eq!(
        height,
        Some(from_button),
        "the constant must be the height Button1's own NormalText reports, not a literal"
    );
}

/// `UIDropDownMenuButtonTemplate` gives its label `GameFontHighlightSmall` through `<NormalFont>`.
#[test]
fn a_button_label_reports_the_font_object_its_button_set() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_dropdown_kit(&s);

    let (object_height, label_height, label_face): (Option<f64>, Option<f64>, Option<String>) = s
        .eval(
            r#"
            local _, objectHeight = GameFontHighlightSmall:GetFont();
            local face, height = DropDownList1Button1NormalText:GetFont();
            return objectHeight, height, face
        "#,
        )
        .unwrap();

    assert_eq!(
        label_height, object_height,
        "the row label must report GameFontHighlightSmall's height, not nil"
    );
    assert!(
        label_face.is_some_and(|f| f.ends_with(".TTF")),
        "the row label must report a real font face through its button's font object"
    );
    assert!(
        s.eval::<bool>(
            "return DropDownList1Button1NormalText:GetFontObject() == GameFontHighlightSmall"
        )
        .unwrap(),
        "the label must hand back the font OBJECT, which is what the corpus indexes immediately"
    );
}
