//! The stock `Interface\FrameXML\ColorPickerFrame.xml` + the dropdown's colour-swatch row, driven
//! the way the corpus drives them.
//!
//! Nothing benilla ships opens this window. Its consumers are third-party addons — 86 of the 218
//! reach `ColorPickerFrame`, and 64 of those files are copies of `Dewdrop-2.0.lua`, THE Ace2 menu
//! library, so its block below is not one addon's idiom but the path every colour option in every
//! Ace2 config menu takes. Every test here therefore enters from Lua exactly as they do; the
//! Dewdrop and AceConsole sequences are transcribed from corpus copies, cited per test.

use benilla_ui::script::{QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The manifest prefix the picker needs: fonts, the panel manager (`ShowUIPanel`/`HideUIPanel`,
/// `UISpecialFrames`, the ESC chain), the shared widget kit (`GameMenuButtonTemplate`), then the
/// window. `UIDropDownMenu.xml` rides along because `CloseMenus` and the swatch row live there.
fn picker() -> UiScript {
    let mut s = UiScript::new().unwrap();
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\ColorPickerFrame.xml",
    ] {
        load_xml(&s, file);
    }
    s.set_screen_size(1024.0, 768.0);
    s.resolve();
    s
}

/// What the widget hands back after being *set* to `(r, g, b)` — the client's whole colour law, run
/// through the engine's own transcription of it rather than restated here.
///
/// It is emphatically **not** the identity, and not a simple quantize either: `SetColorRGB` rounds
/// half-up into bytes and stores HSV floats, while every read path floors, so 9.75 % of colours come
/// back a step low on one channel (`SetColorRGB 0x78eae0`, the read-back floor `0x7bbec0`;
/// measured exhaustively over all 256³). `benilla_ui::script::colorselect`'s own tests pin that law
/// against the reference's measured witnesses; these tests use it as a given so they stay about the
/// Dewdrop plumbing.
fn after_round_trip(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let mut cs = benilla_ui::widget::ColorSelectState::default();
    cs.set_rgb(r, g, b);
    cs.rgb_f64()
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The window's own API surface
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The four names addons address by string all exist, are the right kinds, and the two buttons'
/// `OnClick` is fetchable — which is not decoration: `AceConsole-2.0.lua` l.1402 does
/// `ColorPickerOkayButton:GetScript("OnClick")` and refuses to install its colour option at all if
/// `ColorPickerOkayButton` is nil.
#[test]
fn the_named_pieces_exist_and_their_scripts_are_fetchable() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    assert!(s
        .eval::<bool>("return ColorPickerFrame ~= nil and ColorPickerFrame.SetColorRGB ~= nil")
        .unwrap());
    assert!(s
        .eval::<bool>("return OpacitySliderFrame ~= nil and OpacitySliderFrame.GetValue ~= nil")
        .unwrap());
    for button in ["ColorPickerOkayButton", "ColorPickerCancelButton"] {
        assert!(
            s.eval::<bool>(&format!("return {button} ~= nil")).unwrap(),
            "{button} is missing"
        );
        assert!(
            s.eval::<bool>(&format!(
                r#"return type({button}:GetScript("OnClick")) == "function""#
            ))
            .unwrap(),
            "{button}'s OnClick is not fetchable"
        );
    }
    // Born hidden, like the reference's `hidden="true"`.
    assert!(!s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetColorRGB` paints the preview swatch through `OnColorSelect`, and `GetColorRGB` answers the
/// client's quantized colour. The swatch is the ONLY thing in the window that shows the colour, and
/// nothing but this handler ever writes it.
///
/// The assertion goes through `extract()` on purpose. `ColorSwatch:SetTexture(r, g, b)` is the
/// *solid-colour* form of SetTexture — it replaces the texture, it does not tint one — so
/// `GetVertexColor` still reads the `<Color>` the XML declared (1,1,1,1) and would have "passed"
/// this test while the swatch drew nothing. What the renderer receives is the only honest witness.
#[test]
fn set_color_rgb_paints_the_swatch_and_reads_back_the_widgets_colour() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();
    s.run("ShowUIPanel(ColorPickerFrame)").unwrap();
    s.run("ColorPickerFrame:SetColorRGB(0.2, 0.4, 0.8)")
        .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    let (r, g, b): (f64, f64, f64) = s.eval("return ColorPickerFrame:GetColorRGB()").unwrap();
    assert_eq!((r, g, b), after_round_trip(0.2, 0.4, 0.8));

    s.resolve();
    let want = [r as f32, g as f32, b as f32];
    let painted = s.extract().into_iter().any(|q| match &q.content {
        QuadContent::Texture {
            path: None,
            color: Some(c),
            ..
        } => [c[0], c[1], c[2]] == want && c[3] == 1.0,
        _ => false,
    });
    assert!(
        painted,
        "no solid quad carrying the widget's colour {want:?} — the swatch is not painted"
    );
}

/// `hasOpacity` is what decides whether the window has an opacity slider at all — and the window's
/// WIDTH with it. Both arms of the reference's `OnShow` (l.161-170).
#[test]
fn has_opacity_shows_the_slider_and_widens_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();

    // Without it: no slider, the narrow window. (`GetWidth` reports the *resolved* rect, so the
    // OnShow's SetWidth needs a resolve pass before it is readable — the same round-trip every
    // geometry assertion in this file set makes.)
    s.run("ColorPickerFrame.hasOpacity = nil ShowUIPanel(ColorPickerFrame)")
        .unwrap();
    s.resolve();
    assert!(!s
        .eval::<bool>("return OpacitySliderFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<f64>("return ColorPickerFrame:GetWidth()").unwrap(),
        305.0
    );
    s.run("HideUIPanel(ColorPickerFrame)").unwrap();

    // With it: the slider shows, seeded from `.opacity`, and the window is back to 365.
    s.run("ColorPickerFrame.hasOpacity = 1 ColorPickerFrame.opacity = 0.25 ShowUIPanel(ColorPickerFrame)")
        .unwrap();
    s.resolve();
    assert!(s
        .eval::<bool>("return OpacitySliderFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<f64>("return OpacitySliderFrame:GetValue()")
            .unwrap(),
        0.25
    );
    assert_eq!(
        s.eval::<f64>("return ColorPickerFrame:GetWidth()").unwrap(),
        365.0
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Moving the slider runs `opacityFunc` on every change — the live-preview half of the contract,
/// and the reference's own `<OnValueChanged>` (l.146-152).
#[test]
fn the_opacity_slider_drives_opacity_func_on_every_change() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    s.run(
        r#"
        seen = {}
        ColorPickerFrame.opacityFunc = function()
            table.insert(seen, OpacitySliderFrame:GetValue())
        end
    "#,
    )
    .unwrap();
    s.run("OpacitySliderFrame:SetValue(0.5)").unwrap();
    s.run("OpacitySliderFrame:SetValue(0.75)").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(s.eval::<usize>("return table.getn(seen)").unwrap(), 2);
    assert_eq!(s.eval::<f64>("return seen[1]").unwrap(), 0.5);
    assert_eq!(s.eval::<f64>("return seen[2]").unwrap(), 0.75);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Dewdrop-2.0 — the sequence 64 corpus files share
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The `Dewdrop-2.0.lua` block verbatim in shape (a corpus copy, l.425-465): set `func` to a closure
/// that reads `GetColorRGB()` + `1 - OpacitySliderFrame:GetValue()` and calls the addon's setter
/// with the closed-over arg prefix, mirror it into `opacityFunc`, set `opacity = 1 - this.opacity`,
/// `SetColorRGB`, capture the old values into `cancelFunc`, `ShowUIPanel`.
///
/// Two things this proves beyond "it runs": the setter fires **at `SetColorRGB` time**, before the
/// window is even shown (the reference's own behaviour — `func` is a live preview, and it is why
/// `hasOpacity`/`opacity` are assigned before the colour), and clicking **Okay** commits with the
/// slider's final value.
fn dewdrop_open(s: &UiScript, r: f64, g: f64, b: f64, opacity: f64) {
    s.run(&format!(
        r#"
        applied = {{}}
        -- the addon's own setter, the `func` Dewdrop closes over
        local func = function(a1, r, g, b, a)
            table.insert(applied, {{ key = a1, r = r, g = g, b = b, a = a }})
        end
        local a1 = "bordercolor"          -- Dewdrop's colorArg1
        local hasOpacity = 1
        local this = {{ r = {r}, g = {g}, b = {b}, opacity = {opacity}, hasOpacity = 1 }}

        ColorPickerFrame.func = function()
            if func then
                local r, g, b = ColorPickerFrame:GetColorRGB()
                local a = hasOpacity and 1 - OpacitySliderFrame:GetValue() or nil
                func(a1, r, g, b, a)
            end
        end
        ColorPickerFrame.hasOpacity = this.hasOpacity
        ColorPickerFrame.opacityFunc = ColorPickerFrame.func
        ColorPickerFrame.opacity = 1 - this.opacity
        ColorPickerFrame:SetColorRGB(this.r, this.g, this.b)
        local pr, pg, pb, pa = this.r, this.g, this.b, this.opacity
        ColorPickerFrame.cancelFunc = function()
            func(a1, pr, pg, pb, pa)
        end
        ShowUIPanel(ColorPickerFrame)
    "#
    ))
    .unwrap();
}

/// **Okay commits.** The Dewdrop sequence, then the OK button: the addon's setter receives the
/// quantized colour and the slider-derived alpha, and the window closes.
#[test]
fn the_dewdrop_sequence_commits_on_okay() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    dewdrop_open(&s, 0.1, 0.5, 0.9, 0.25);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The live preview has already run TWICE before the window is even on screen, and both are the
    // reference's own: `SetColorRGB` fires `OnColorSelect` → `func()`, and then `OnShow`'s
    // `OpacitySliderFrame:SetValue(this.opacity)` moves the slider off its `defaultValue="1"` and
    // fires `OnValueChanged` → `opacityFunc()`, which Dewdrop has aliased to the same closure.
    // Pinned rather than glossed: an addon whose `func` writes through to the server sees both.
    assert_eq!(s.eval::<usize>("return table.getn(applied)").unwrap(), 2);
    assert!(s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    // The window came up with the slider at `1 - opacity`, which is what OnShow seeded.
    assert_eq!(
        s.eval::<f64>("return OpacitySliderFrame:GetValue()")
            .unwrap(),
        0.75
    );

    // The player drags the opacity down, then accepts.
    s.run("OpacitySliderFrame:SetValue(0.4)").unwrap();
    s.run("ColorPickerOkayButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(!s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());

    // Last call wins: the addon holds the quantized colour and `1 - slider`.
    let (key, r, g, b, a): (String, f64, f64, f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.key, t.r, t.g, t.b, t.a")
        .unwrap();
    assert_eq!(
        key, "bordercolor",
        "the closed-over arg prefix is preserved"
    );
    assert_eq!((r, g, b), after_round_trip(0.1, 0.5, 0.9));
    // 1e-6, not exact: a Slider holds `f32` (the client's `CSimpleSlider` does too), so
    // `1 - GetValue()` is `1 - f32(0.4)` = 0.59999999…, not the literal 0.6.
    assert!(
        (a - 0.6).abs() < 1e-6,
        "alpha is 1 - the slider's 0.4, got {a}"
    );
}

/// **Cancel restores.** The half people get wrong: Cancel must put back the colour the addon came
/// in with, and it does it by *calling `cancelFunc`* — the window has no memory of its own.
#[test]
fn cancel_restores_the_previous_colour_through_cancel_func() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    dewdrop_open(&s, 0.1, 0.5, 0.9, 0.25);

    // The player fiddles: a new colour previewed live, and the opacity moved.
    s.run("ColorPickerFrame:SetColorRGB(1, 0, 0)").unwrap();
    s.run("OpacitySliderFrame:SetValue(0.05)").unwrap();
    let (mid_r, mid_a): (f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.r, t.a")
        .unwrap();
    assert_eq!(mid_r, 1.0, "the live preview really did go red");
    // 1e-6, like the accept test's alpha above and for one more reason on top of the f32 store:
    // `OpacitySliderFrame` carries `valueStep="0.01"`, so `SetValue` rebuilds the value as
    // `n·step + min` (2133) and the reconstruction costs a few more ulps than the literal.
    assert!(
        (mid_a - 0.95).abs() < 1e-6,
        "alpha is 1 - the slider's 0.05, got {mid_a}"
    );

    // …then backs out.
    s.run("ColorPickerCancelButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(!s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());

    let (r, g, b, a): (f64, f64, f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.r, t.g, t.b, t.a")
        .unwrap();
    assert_eq!(
        (r, g, b, a),
        (0.1, 0.5, 0.9, 0.25),
        "cancelFunc replays the addon's OWN pre-open values — raw, never through the widget"
    );
}

/// ESC cancels too, and that is a deviation working as designed: the reference hooks `<OnKeyDown>`
/// on the frame, this engine routes the press through `ToggleGameMenu`'s chain (UiPanels.xml), and
/// the rung cancel-CLICKS rather than hiding — so the colour comes back and the game menu does not
/// open behind it.
#[test]
fn escape_hides_the_picker_and_cancel_is_what_reverts() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    dewdrop_open(&s, 0.1, 0.5, 0.9, 0.25);
    s.run("ColorPickerFrame:SetColorRGB(1, 0, 0)").unwrap();

    s.run("ToggleGameMenu()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(!s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    // **The reference's ESC is a bare hide, and the previewed colour stands.** `ColorPickerFrame`
    // is a `UISpecialFrames` row (`UIParent.lua:52-55`), so the ladder's `CloseAllWindows` hides
    // it — the stock file runs `cancelFunc` from the Cancel button's own OnClick and from nowhere
    // else, and its OnHide has no such arm. Our retired ladder clicked Cancel for the player;
    // that arm went with the file (1988).
    let (r, g, b): (f64, f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.r, t.g, t.b")
        .unwrap();
    assert_eq!(
        (r, g, b),
        (1.0, 0.0, 0.0),
        "ESC hid the picker and left the previewed colour applied"
    );
    // The Cancel button is still what reverts it, which is the reference's whole cancel path.
    s.run("ColorPickerFrame:Show() ColorPickerCancelButton:Click()")
        .unwrap();
    let (r, g, b): (f64, f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.r, t.g, t.b")
        .unwrap();
    assert_eq!((r, g, b), (0.1, 0.5, 0.9), "Cancel runs cancelFunc");
}

/// `AceConsole-2.0.lua` l.1402-1406, verbatim in shape: fetch the Okay button's own handler, replace
/// it with one that calls the original and then does the addon's own commit. If `GetScript` returned
/// nil, or `SetScript` did not take, the addon's arm would never run — and 20+ corpus addons ship
/// this library.
#[test]
fn ace_console_can_chain_the_okay_buttons_onclick() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    s.run(
        r#"
        chained = {}
        ColorPickerFrame.func = function() table.insert(chained, "original") end
        if ColorPickerOkayButton then
            local ColorPickerOkayButton_OnClick = ColorPickerOkayButton:GetScript("OnClick")
            ColorPickerOkayButton:SetScript("OnClick", function()
                if ColorPickerOkayButton_OnClick then
                    ColorPickerOkayButton_OnClick()
                end
                table.insert(chained, "ace")
            end)
        end
        ShowUIPanel(ColorPickerFrame)
    "#,
    )
    .unwrap();
    s.run("ColorPickerOkayButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(s.eval::<usize>("return table.getn(chained)").unwrap(), 2);
    assert_eq!(s.eval::<String>("return chained[1]").unwrap(), "original");
    assert_eq!(s.eval::<String>("return chained[2]").unwrap(), "ace");
    assert!(
        !s.eval::<bool>("return ColorPickerFrame:IsVisible()")
            .unwrap(),
        "the original handler still hid the window"
    );
}

/// ESC's other path: `CloseWindows` walks `UISpecialFrames`, and `ColorPickerFrame` is in it now
/// (decision 1206 declined to seed it only because we shipped no such frame). This is the plain
/// `Hide` the reference does on that path — no cancel — reached when something else closes the
/// world's windows.
#[test]
fn the_picker_is_a_uispecialframe_and_close_windows_puts_it_away() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    let listed: bool = s
        .eval(
            r#"
        for _, name in ipairs(UISpecialFrames) do
            if name == "ColorPickerFrame" then return true end
        end
        return false
    "#,
        )
        .unwrap();
    assert!(listed, "the reference's own entry is seeded");

    s.run("ShowUIPanel(ColorPickerFrame)").unwrap();
    let found: bool = s.eval("return CloseWindows() ~= nil").unwrap();
    assert!(found, "CloseWindows reports it closed something");
    assert!(!s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The dropdown's colour-swatch row — the other half of the same feature
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A menu row with `hasColorSwatch` shows its square tinted to `r/g/b`, and clicking the square is
/// what opens the picker — seeding `func`/`hasOpacity`/`opacity`/`cancelFunc`, the colour, AND
/// `previousValues`, which is captured here and nowhere else. Then Cancel gets it back.
///
/// The info table is the reference's own documented shape (`UIDropDownMenu.lua` l.114-125) and the
/// exact one FloatingChatFrame's colour rows use.
#[test]
fn a_dropdown_row_with_has_color_swatch_opens_the_picker_and_cancel_restores() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    s.run(
        r#"
        restored = {}
        picked = {}
        local dd = CreateFrame("Frame", "TestColorDropDown", nil, "UIDropDownMenuTemplate")
        -- The anchor needs a POSITION. `ToggleDropDownMenu` anchors the list to this frame and then
        -- guards on `listFrame:GetCenter()`, hiding the list again and returning when it is nil
        -- (ref UIDropDownMenu.lua:624-631) — so an unplaced dropdown opens no menu in the real
        -- client either. Our deleted transcription carried no such guard and showed it regardless,
        -- which is the only reason this fixture ever worked unanchored.
        dd:SetPoint("CENTER", 0, 0)
        UIDropDownMenu_Initialize(dd, function()
            local info = {}
            info.text = "Border Color"
            info.hasColorSwatch = 1
            info.r = 0.2
            info.g = 0.4
            info.b = 0.6
            info.hasOpacity = 1
            info.opacity = 0.3
            info.notCheckable = 1
            info.swatchFunc = function()
                local r, g, b = ColorPickerFrame:GetColorRGB()
                table.insert(picked, r .. "," .. g .. "," .. b)
            end
            info.opacityFunc = function() end
            info.cancelFunc = function(previous)
                table.insert(restored, previous.r .. "," .. previous.g .. "," .. previous.b .. "," .. previous.opacity)
            end
            UIDropDownMenu_AddButton(info)
        end, "MENU")
        ToggleDropDownMenu(1, nil, dd, "TestColorDropDown", 0, 0)
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The square is up and wears the row's colour.
    assert!(s
        .eval::<bool>("return DropDownList1Button1ColorSwatch:IsVisible()")
        .unwrap());
    let (r, g, b): (f64, f64, f64) = s
        .eval("return DropDownList1Button1ColorSwatchNormalTexture:GetVertexColor()")
        .unwrap();
    assert!(
        (r - 0.2).abs() < 1e-6 && (g - 0.4).abs() < 1e-6 && (b - 0.6).abs() < 1e-6,
        "the swatch is tinted to info.r/g/b, got {r},{g},{b}"
    );

    // Clicking it opens the picker, seeded from the row.
    s.run("DropDownList1Button1ColorSwatch:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    assert!(
        !s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "CloseMenus() shut the menu first, the ref's own first line"
    );
    assert_eq!(
        s.eval::<f64>("return ColorPickerFrame.opacity").unwrap(),
        0.3
    );
    assert!(s
        .eval::<bool>("return OpacitySliderFrame:IsVisible()")
        .unwrap());
    let (pr, pg, pb): (f64, f64, f64) = s.eval("return ColorPickerFrame:GetColorRGB()").unwrap();
    assert_eq!((pr, pg, pb), after_round_trip(0.2, 0.4, 0.6));
    // …and swatchFunc became the picker's live-preview `func`, which already ran once.
    assert_eq!(s.eval::<usize>("return table.getn(picked)").unwrap(), 1);

    // Cancel hands `previousValues` — the table OpenColorPicker captured — to the addon.
    s.run("ColorPickerCancelButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return restored[1]").unwrap(),
        "0.2,0.4,0.6,0.3",
        "cancelFunc(previousValues) carries r/g/b/opacity straight off the row"
    );
}

/// A row WITHOUT `hasColorSwatch` keeps its square hidden — the branch that stops every ordinary
/// menu row from sprouting a colour button.
#[test]
fn a_row_without_the_flag_has_no_swatch() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    s.run(
        r#"
        local dd = CreateFrame("Frame", "TestPlainDropDown", nil, "UIDropDownMenuTemplate")
        UIDropDownMenu_Initialize(dd, function()
            local info = {}
            info.text = "Just A Row"
            info.notCheckable = 1
            UIDropDownMenu_AddButton(info)
        end, "MENU")
        ToggleDropDownMenu(1, nil, dd, "TestPlainDropDown", 0, 0)
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(!s
        .eval::<bool>("return DropDownList1Button1ColorSwatch:IsVisible()")
        .unwrap());
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The wheel (decision 1592) — the shipped window's generated art, end to end
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The four elements the reference declares are installed on the SHIPPED window, at the
/// reference's own geometry, and `ColorPickerWheel` publishes under the reference's name — which is
/// load-bearing twice over: `<ColorValueTexture>` anchors to it by name, and an addon re-skinning
/// the picker addresses it by name.
#[test]
fn the_shipped_window_carries_the_wheel_at_the_references_geometry() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();
    s.run("ShowUIPanel(ColorPickerFrame)").unwrap();
    s.resolve();

    let (wl, wb, ww, wh): (f32, f32, f32, f32) = s
        .eval(
            "local r = ColorPickerWheel; \
             return r:GetLeft(), r:GetBottom(), r:GetWidth(), r:GetHeight()",
        )
        .unwrap();
    assert_eq!((ww, wh), (128.0, 128.0), "ref l.186-195: a 128 square");
    // The window is 365x200 centred on a 1024x768 screen; the wheel is TOPLEFT (16, -32) in it.
    let (fl, ft): (f32, f32) = s
        .eval("return ColorPickerFrame:GetLeft(), ColorPickerFrame:GetTop()")
        .unwrap();
    assert_eq!(wl, fl + 16.0);
    assert_eq!(wb, ft - 32.0 - 128.0);

    let (sl, sw, sh): (f32, f32, f32) = s
        .eval(
            "local r = ColorPickerFrame:GetColorValueTexture(); \
             return r:GetLeft(), r:GetWidth(), r:GetHeight()",
        )
        .unwrap();
    assert_eq!((sw, sh), (32.0, 128.0), "ref l.203-215");
    assert_eq!(
        sl,
        wl + ww + 24.0,
        "the strip anchors LEFT to the wheel's RIGHT at +24 — BY NAME"
    );

    // The markers: the one BLP in this window, at the reference's two crops.
    for (getter, want_w, want_h) in [
        ("GetColorWheelThumbTexture", 10.0, 10.0),
        ("GetColorValueThumbTexture", 48.0, 14.0),
    ] {
        let (w, h): (f32, f32) = s
            .eval(&format!(
                "local r = ColorPickerFrame:{getter}(); return r:GetWidth(), r:GetHeight()"
            ))
            .unwrap();
        assert_eq!((w, h), (want_w, want_h), "{getter}");
        let file: String = s
            .eval(&format!("return ColorPickerFrame:{getter}():GetTexture()"))
            .unwrap();
        assert!(file.contains("UI-ColorPicker-Buttons"), "{getter}: {file}");
    }
}

/// **The window can now be used for what it is for**: Dewdrop opens it, the user clicks a hue, and
/// the caller's `func` sees the colour under the cursor. This is the whole of B246's picker path
/// and the thing that was impossible before 1592 — every other test in this file could pass with no
/// wheel at all.
///
/// The click is at the wheel's right rim, which the pick law makes 180° — cyan.
#[test]
fn a_click_on_the_wheel_reaches_the_callers_func() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();
    // Dewdrop-2.0.lua l.425-465's block, as transcribed in this file's other tests.
    s.run(
        r#"
        picked = nil
        ColorPickerFrame.func = function()
            local r, g, b = ColorPickerFrame:GetColorRGB()
            picked = { r = r, g = g, b = b }
        end
        ColorPickerFrame.hasOpacity = nil
        ColorPickerFrame:SetColorRGB(1, 1, 1)
        ShowUIPanel(ColorPickerFrame)
    "#,
    )
    .unwrap();
    s.resolve();
    let (l, b, w, h): (f32, f32, f32, f32) = s
        .eval(
            "local r = ColorPickerWheel; \
             return r:GetLeft(), r:GetBottom(), r:GetWidth(), r:GetHeight()",
        )
        .unwrap();
    s.mouse_button(l + w - 1.0, b + h * 0.5, "LeftButton", true);
    s.mouse_button(l + w - 1.0, b + h * 0.5, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    let (r, g, bl): (f64, f64, f64) = s
        .eval("return picked.r, picked.g, picked.b")
        .expect("func ran with a colour");
    assert!(
        r < 0.05 && g > 0.95 && bl > 0.95,
        "the right rim is 180° — cyan, got ({r:.3}, {g:.3}, {bl:.3})"
    );
    // And the window's own preview swatch followed, through OnColorSelect.
    let swatch = s.extract().into_iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { color: Some(c), .. }
            if c[0] < 0.05 && c[1] > 0.95)
    });
    assert!(swatch, "ColorSwatch repainted to the picked colour");
}

/// The two file-less regions reach the renderer as their own content kinds, carrying exactly what
/// changes their pixels and nothing more: the disc carries **no colour at all** (it is drawn at a
/// literal `V = 1`), and the strip carries hue and saturation but **not value**. Anything more
/// would churn these quads on every step of a drag for no pixel — the extract gate is only honest
/// if the content is (decision 1592 §4).
#[test]
fn the_generated_art_reaches_the_renderer_carrying_only_what_moves_a_pixel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();
    s.run("ColorPickerFrame:SetColorRGB(0.15, 0.55, 0.75); ShowUIPanel(ColorPickerFrame)")
        .unwrap();
    s.resolve();
    let quads = s.extract();
    assert!(
        quads
            .iter()
            .any(|q| matches!(q.content, QuadContent::ColorWheel)),
        "the disc draws"
    );
    let strip = quads
        .iter()
        .find_map(|q| match q.content {
            QuadContent::ColorValue { hue, sat } => Some((hue, sat)),
            _ => None,
        })
        .expect("the strip draws");
    // (0.15, 0.55, 0.75) is a cyan-blue: hue 200°, saturation 0.80 (through the client's own
    // quantize on the way in — `set_rgb` stores what the 8-bit lattice can hold).
    assert!(
        (strip.0 - 200.0).abs() < 0.5 && (strip.1 - 0.801).abs() < 0.01,
        "the strip carries the live hue/sat, got {strip:?}"
    );

    // Dropping the brightness moves NO strip pixel — the same quad content comes back.
    s.run("ColorPickerFrame:SetColorRGB(0.05, 0.18, 0.25)")
        .unwrap();
    s.resolve();
    let after = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::ColorValue { hue, sat } => Some((hue, sat)),
            _ => None,
        })
        .expect("the strip still draws");
    assert!(
        (after.0 - strip.0).abs() < 2.0 && (after.1 - strip.1).abs() < 0.02,
        "a third of the brightness, the same strip: {strip:?} -> {after:?}"
    );
}
