//! The stock `Blizzard_MacroUI` addon, the macro editor and its name/icon popup, driven against the
//! engine's macro table.

use benilla_ui::script::{CursorPayload, UiScript};

/// Click the first macro: `MacroFrame_OnShow` only highlights a selection and never makes one
/// (`Blizzard_MacroUI.lua:19-22`, `:75`).
fn select_first(s: &UiScript) {
    s.run("MacroButton1:Click()").unwrap();
}

/// [`harness`] with a chosen player name, which tab 2's label carries.
fn harness_named(player: &str) -> UiScript {
    harness_with(player)
}

fn harness() -> UiScript {
    harness_with("Probefour")
}

fn harness_with(player: &str) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The global strings this window formats.
    s.run(
        r#"
        CREATE_MACROS = "Create Macros"
        GENERAL_MACROS = "General Macros"
        CHARACTER_SPECIFIC_MACROS = "%s Specific Macros"
        ENTER_MACRO_LABEL = "Enter Macro Commands:"
        MACROFRAME_CHAR_LIMIT = "%d/255 Characters Used"
        MACRO_POPUP_TEXT = "Enter Macro Name (Max 16 Characters):"
        MACRO_POPUP_CHOOSE_ICON = "Choose an Icon:"
        CHANGE_MACRO_NAME_ICON = "Change Name/Icon"
        DELETE = "Delete"
        NEW = "New"
        EXIT = "Exit"
        CANCEL = "Cancel"
        OKAY = "Okay"
        MACROS = "Macros"
        -- The tooltip plate colours the body's backdrop reads (the app gets these from
        -- UIParent.lua's own globals; the window only needs them to exist).
        TOOLTIP_DEFAULT_COLOR = { r = 1.0, g = 1.0, b = 1.0 }
        TOOLTIP_DEFAULT_BACKGROUND_COLOR = { r = 0.09, g = 0.09, b = 0.19 }
        "#,
    )
    .unwrap();
    // Synchronous, like the app's `AtlasMeasurer`: `PanelTemplates_TabResize` measures the label
    // once, in the tab's OnLoad, and a pending measure would size the tab off a width of 0.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    // Before the load: tab 2's OnLoad formats `UnitName("player")` into its label
    // (`Blizzard_MacroUI.xml:509`).
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some(player.into()),
            level: 60,
            ..Default::default()
        }),
    );
    for file in [
        r"Interface\FrameXML\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        // `PanelTemplates_SelectTab` reads `GameTooltip` unguarded (`UIPanelTemplates.lua:130`).
        "Interface\\FrameXML\\GameTooltip.xml",
        // ScrollTemplates before UIPanelTemplates, the manifest's order.
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // `ClassTrainerListScrollFrameTemplate`, which the icon chooser's scroll frame inherits.
        r"Interface\FrameXML\ClassTrainerFrameTemplates.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml", // the window's OnShow/OnHide call it
    ] {
        super::test_ui::load_ui(&s, file);
    }
    // A LoadOnDemand addon, seated off the chain and loaded by stock `MacroFrame_LoadUI`
    // (`UIParent.lua:178`), as the app reaches it.
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_MacroUI");
    s.run("MacroFrame_LoadUI()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The icon chooser's list is the app's push.
    s.set_macro_icons(vec![
        "Interface\\Icons\\Ability_Ambush".into(),
        "Interface\\Icons\\Ability_BackStab".into(),
        "Interface\\Icons\\Spell_Fire_FlameBolt".into(),
    ]);
    s
}

fn no_errors(s: &UiScript, step: &str) {
    assert!(s.errors().is_empty(), "{step}: {:?}", s.errors());
}

#[test]
fn new_then_pick_an_icon_then_okay_creates_the_macro() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);
    no_errors(&s, "show");

    // OKAY starts disabled: no name, no icon.
    s.run("MacroNewButton:Click()").unwrap();
    assert!(s
        .eval::<bool>("return MacroPopupFrame:IsVisible()")
        .unwrap());
    assert!(
        !s.eval::<bool>("return MacroPopupOkayButton:IsEnabled() ~= 0")
            .unwrap(),
        "a nameless, iconless macro cannot be created"
    );

    s.run(r#"MacroPopupEditBox:SetText("Ambush")"#).unwrap();
    s.run("MacroPopupButton1:Click()").unwrap();
    assert!(
        s.eval::<bool>("return MacroPopupOkayButton:IsEnabled() ~= 0")
            .unwrap(),
        "a name and an icon enable OKAY"
    );

    s.run("MacroPopupOkayButton:Click()").unwrap();
    no_errors(&s, "okay");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (1, 0),
        "created on the ACCOUNT tab (macroBase 0)"
    );
    let (name, tex) = s
        .eval::<(String, String)>("local n, t = GetMacroInfo(1) return n, t")
        .unwrap();
    assert_eq!(name, "Ambush");
    assert_eq!(tex, "Interface\\Icons\\Ability_Ambush");
    assert!(
        !s.eval::<bool>("return MacroPopupFrame:IsVisible()")
            .unwrap(),
        "OKAY closes the popup"
    );
    assert_eq!(
        s.eval::<String>("return MacroFrameSelectedMacroName:GetText()")
            .unwrap(),
        "Ambush"
    );
}

/// `MacroFrame_SaveMacro` commits the body from the tab switch, the list click and the OnHide.
#[test]
fn typing_a_body_and_closing_the_window_commits_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"CreateMacro("Ambush", 1, "")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);
    s.run(r#"MacroFrameText:SetText("/cast Ambush\n/say pew")"#)
        .unwrap();
    // The dirty flag comes from the box's `OnTextChanged`, which the drain delivers next frame.
    s.tick(0.0);
    no_errors(&s, "type");
    assert!(
        s.eval::<bool>("return MacroFrame.textChanged == 1")
            .unwrap(),
        "OnTextChanged marks the window dirty"
    );
    assert_eq!(
        s.eval::<String>("return MacroFrameCharLimitText:GetText()")
            .unwrap(),
        "21/255 Characters Used"
    );

    s.run(r#"HideUIPanel(MacroFrame)"#).unwrap();
    no_errors(&s, "hide");
    assert_eq!(
        s.eval::<String>("local _, _, b = GetMacroInfo(1) return b")
            .unwrap(),
        "/cast Ambush\n/say pew",
        "closing the window commits the body"
    );
}

/// `MacroFrame.macroBase` is 0 or `MAX_MACROS` (18) and every binding takes `macroBase + i`, so the
/// character tab starts at 19 (`Blizzard_MacroUI.lua:1`, `:42`).
#[test]
fn the_character_tab_creates_in_the_second_index_range_and_switching_saves() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"CreateMacro("Acct", 1, "")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    // Type into the account macro, then switch tabs with no explicit save.
    s.run(r#"MacroFrameText:SetText("/say account")"#).unwrap();
    s.tick(0.0); // the edit marks, the drain notifies
    s.run("MacroFrameTab2:Click()").unwrap();
    no_errors(&s, "tab 2");
    assert_eq!(
        s.eval::<String>("local _, _, b = GetMacroInfo(1) return b")
            .unwrap(),
        "/say account",
        "the tab switch saved the body first"
    );
    assert_eq!(s.eval::<i64>("return MacroFrame.macroBase").unwrap(), 18);

    s.run("MacroNewButton:Click()").unwrap();
    s.run(r#"MacroPopupEditBox:SetText("Char")"#).unwrap();
    s.run("MacroPopupButton2:Click()").unwrap();
    s.run("MacroPopupOkayButton:Click()").unwrap();
    no_errors(&s, "create on tab 2");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (1, 1)
    );
    assert_eq!(
        s.eval::<String>("return GetMacroInfo(19)").unwrap(),
        "Char",
        "the character tab's first slot is index 19"
    );

    s.run("MacroFrameTab1:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return MacroFrame.macroBase").unwrap(), 0);
    assert_eq!(
        s.eval::<String>("return MacroFrameSelectedMacroName:GetText()")
            .unwrap(),
        "Acct"
    );
}

/// DELETE re-runs `MacroFrame_OnLoad`, which selects the first macro or, with none left, nothing
/// (`Blizzard_MacroUI.xml:551-553`).
#[test]
fn delete_removes_the_macro_and_re_selects() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run(r#"CreateMacro("One", 1, "/say one")"#).unwrap();
    s.run(r#"CreateMacro("Two", 2, "/say two")"#).unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    s.run("MacroDeleteButton:Click()").unwrap();
    no_errors(&s, "delete");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (1, 0)
    );
    assert_eq!(s.eval::<String>("return GetMacroInfo(1)").unwrap(), "Two");
    assert_eq!(
        s.eval::<String>("return MacroFrameSelectedMacroName:GetText()")
            .unwrap(),
        "Two"
    );

    s.run("MacroDeleteButton:Click()").unwrap();
    no_errors(&s, "delete last");
    assert_eq!(
        s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
            .unwrap(),
        (0, 0)
    );
    assert!(
        !s.eval::<bool>("return MacroFrameSelectedMacroButton:IsVisible()")
            .unwrap(),
        "no selection, no detail pane"
    );
}

#[test]
fn dragging_a_macro_button_loads_the_cursor_with_the_macro_payload() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
        .unwrap();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    // A real press-and-move: calling `GetScript("OnDragStart")` directly leaves `this` nil, and
    // the stock handler reads it.
    s.resolve();
    let (bx, by): (f32, f32) = s.eval("return MacroButton1:GetCenter()").unwrap();
    s.mouse_button(bx, by, "LeftButton", true);
    s.mouse_move(bx + 60.0, by + 60.0);
    no_errors(&s, "drag");
    let payload = s.cursor_payload();
    assert!(
        matches!(&payload, Some(CursorPayload::Macro(m)) if m.index == 1),
        "the macro payload, carrying its index: {payload:?}"
    );
    assert_eq!(
        s.eval::<(String, i64)>("local k, i = GetCursorInfo() return k, i")
            .unwrap(),
        ("macro".to_string(), 1)
    );

    // A bar slot packs the MACRO tag (0x40 << 24) with the macro index.
    let mut s = s;
    s.run("PlaceAction(1)").unwrap();
    assert_eq!(s.take_action_sets(), vec![(1, 0x4000_0000 | 1)]);
}

#[test]
fn the_icon_chooser_shows_the_pushed_list_and_hides_its_tail() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);
    s.run("MacroNewButton:Click()").unwrap();
    no_errors(&s, "popup");

    assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 3);
    for i in 1..=3 {
        assert!(
            s.eval::<bool>(&format!("return MacroPopupButton{i}:IsVisible()"))
                .unwrap(),
            "button {i} shows an icon"
        );
    }
    assert!(
        !s.eval::<bool>("return MacroPopupButton4:IsVisible()")
            .unwrap(),
        "past the end of the list the button hides — not a blank square"
    );

    s.run("MacroPopupButton3:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return MacroPopupFrame.selectedIcon")
            .unwrap(),
        3
    );
    assert!(
        s.eval::<bool>("return MacroPopupButton3:GetChecked()")
            .unwrap(),
        "the picked icon is checked"
    );
}

/// Both tabs pass `PanelTemplates_TabResize` a -15 padding and tab 2, whose label carries the
/// player name, a 150 cap; a short and a long name run both arms of the cap.
#[test]
fn the_two_tabs_fit_inside_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `sideWidths`: twice `TabButtonTemplate`'s 16-unit end slice (`UIPanelTemplates.lua:41`).
    const SIDES: f64 = 32.0;
    /// The stock padding on both tabs (`Blizzard_MacroUI.xml:488`, `:511`).
    const PAD: f64 = -15.0;
    /// The stock cap on tab 2 (`Blizzard_MacroUI.xml:511`).
    const CAP: f64 = 150.0;

    for (name, cap_should_bind) in [("Probe", false), ("Bartholomewthelongnamed", true)] {
        let mut s = harness_named(name);
        s.run("ShowMacroFrame()").unwrap();
        select_first(&s);
        s.resolve();

        let (w1, w2, left1, label1, label2): (f64, f64, f64, f64, f64) = s
            .eval(
                "return MacroFrameTab1:GetWidth(), MacroFrameTab2:GetWidth(), \
                 MacroFrameTab1:GetLeft() - MacroFrame:GetLeft(), \
                 MacroFrameTab1Text:GetStringWidth(), MacroFrameTab2Text:GetStringWidth()",
            )
            .unwrap();

        assert!(label1 > 0.0 && label2 > 0.0, "both labels measured");
        assert_eq!(
            w1,
            label1 + PAD + SIDES,
            "tab 1 is text − 15 + the end slices"
        );
        let uncapped = label2 + PAD + SIDES;
        if cap_should_bind {
            assert_eq!(
                w2,
                CAP + PAD + SIDES,
                "the reference's 150 cap binds on a long name"
            );
            assert!(
                uncapped > CAP + PAD + SIDES,
                "…and the name really was over it"
            );
        } else {
            assert_eq!(w2, uncapped, "a short name is under the cap");
        }
        assert!(
            w2 <= CAP + PAD + SIDES,
            "the clamp may tighten the reference's cap, never loosen it"
        );

        // The tab ends inside the plate the player sees, not merely inside the 384-unit frame.
        let tab2_right: f64 = s
            .eval("return MacroFrameTab2:GetLeft() + MacroFrameTab2:GetWidth()")
            .unwrap();
        assert!(
            tab2_right <= 349.0,
            "tab 2 ends at {tab2_right}, past the drawn plate's last opaque column (349)"
        );
        assert!(
            left1 + w1 + w2 <= 384.0,
            "the tab row ({left1} + {w1} + {w2}) runs off the 384-wide window"
        );
        no_errors(&s, "tab fit");
    }
}

#[test]
fn the_icon_choosers_scroll_bar_sits_on_the_popup_plate() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ShowMacroFrame() MacroNewButton:Click()").unwrap();
    s.resolve();
    let (bar_left, bar_right, plate_left, plate_right): (f64, f64, f64, f64) = s
        .eval(
            "local b = MacroPopupScrollFrameScrollBar local p = MacroPopupFrame \
             return b:GetLeft(), b:GetRight(), p:GetLeft(), p:GetRight()",
        )
        .unwrap();
    assert!(
        bar_left >= plate_left && bar_right <= plate_right,
        "scroll bar {bar_left}..{bar_right} is outside the popup plate {plate_left}..{plate_right}"
    );
    // The stock seat: 6 right of a scroll frame whose right edge is 39 in from the 297-wide popup's
    // (`UIPanelTemplates.xml:172`, `Blizzard_MacroUI.xml:766`), so 264..280.
    assert_eq!(
        (bar_left - plate_left, bar_right - plate_left),
        (264.0, 280.0)
    );
    no_errors(&s, "popup scroll bar");
}

#[test]
fn the_tab_row_settles_and_stays_inside_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The widths must hold across frames, not just on the first.
    let mut s = harness_named("Onehunter");
    s.run("ShowMacroFrame()").unwrap();
    select_first(&s);

    let mut widths = Vec::new();
    for _ in 0..12 {
        s.tick(0.016);
        s.resolve();
        widths.push(
            s.eval::<(f64, f64, f64)>(
                "return MacroFrameTab1:GetWidth(), MacroFrameTab2:GetWidth(), \
                 MacroFrameTab2:GetRight() - MacroFrame:GetLeft()",
            )
            .unwrap(),
        );
    }

    let last = *widths.last().unwrap();
    let settled = &widths[widths.len() - 4..];
    assert!(
        settled.iter().all(|w| *w == last),
        "the tab fit never settles — it changes every frame: {widths:?}"
    );
    // 344, not 384: this window's art stops 40 units short of its frame rect. The stock padding
    // and cap alone keep the row inside it.
    assert!(
        last.2 <= 344.0,
        "tab 2 ends at {} of a window whose plate stops at 344: {widths:?}",
        last.2
    );
    no_errors(&s, "tab settle");
}

/// Tab 2's `150` cap (`Blizzard_MacroUI.xml:511`) holds the row inside the drawn edge for any name.
#[test]
fn no_character_name_can_push_the_tab_row_off_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    for name in ["Ai", "Onehunter", "Bartholomewthethird", &"W".repeat(64)] {
        let mut s = harness_named(name);
        s.run("ShowMacroFrame()").unwrap();
        select_first(&s);
        s.tick(0.016);
        s.resolve();
        let (right, w2): (f64, f64) = s
            .eval(
                "return MacroFrameTab2:GetRight() - MacroFrame:GetLeft(), \
                 MacroFrameTab2:GetWidth()",
            )
            .unwrap();
        assert!(
            right <= 344.0,
            "{name:?}: the tab row ends at {right}, past the drawn plate's edge at 344 \
             (tab 2 is {w2} wide)"
        );
        no_errors(&s, name);
    }
}

#[test]
fn the_tab_highlight_is_exactly_its_tab() {
    let _data = benilla_formats::wow_data_or_skip!();
    // A name under the stock cap, one over it, and a 40-character one, checked on every frame.
    for name in ["Ai", "Onehunter", &"W".repeat(40)] {
        let mut s = harness_named(name);
        s.run("ShowMacroFrame()").unwrap();
        select_first(&s);

        for frame in 0..12 {
            s.tick(0.016);
            s.resolve();
            for tab in ["MacroFrameTab1", "MacroFrameTab2"] {
                let (tl, tr, hl, hr): (f64, f64, f64, f64) = s
                    .eval(&format!(
                        "return {tab}:GetLeft(), {tab}:GetRight(), \
                         {tab}HighlightTexture:GetLeft(), {tab}HighlightTexture:GetRight()"
                    ))
                    .unwrap();
                // The widths differ, as in the reference: only the most-derived `<OnLoad>` runs,
                // since `SetScript` releases the slot's one ref before reading the new body
                // (`0x7025ec`). Tab 2's ends in `PanelTemplates_TabResize`, which sets the
                // highlight to the tab width; tab 1's then sets it to `GetTextWidth() + 31`, 14
                // wider than the tab (`Blizzard_MacroUI.xml:488-489`, `:510-511`).
                let label: f64 = s
                    .eval(&format!("return {tab}Text:GetStringWidth()"))
                    .unwrap();
                let want = if tab == "MacroFrameTab1" {
                    label + 31.0
                } else {
                    tr - tl
                };
                assert_eq!(
                    hr - hl,
                    want,
                    "{tab} f{frame} ({name}): highlight {} wide, label {label}, tab {}",
                    hr - hl,
                    tr - tl
                );
                // `TabButtonTemplate` anchors the highlight `BOTTOM` at `(2, -8)`: centred on the
                // tab, 2 right (`UIPanelTemplates.xml:395-397`).
                assert_eq!(
                    (hl + hr) / 2.0,
                    (tl + tr) / 2.0 + 2.0,
                    "{tab} f{frame} ({name}): highlight not centred on the tab"
                );
            }
        }
        no_errors(&s, "tab highlight");
    }
}

#[test]
fn the_icon_picker_opens_without_raising() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    select_first(&s);
    let _ = s.errors();
    s.run("MacroEditButton:Click()").unwrap();
    assert!(
        s.errors().is_empty(),
        "opening the icon picker must not raise: {:?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return MacroPopupFrame:IsShown() and true or false")
            .unwrap(),
        "the picker is up"
    );
}
