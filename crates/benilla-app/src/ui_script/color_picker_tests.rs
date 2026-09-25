//! The stock `ColorPickerFrame.xml` and the dropdown's colour-swatch row, driven from Lua the way
//! addons drive them; the Dewdrop-2.0 and AceConsole-2.0 sequences are shaped on corpus copies.

use benilla_ui::script::{QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// The files the picker needs, in manifest order, ending with the window.
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

/// What the widget reads back after being set to `(r, g, b)`: `SetColorRGB` (`0x78eae0`) rounds
/// into bytes and stores HSV, the read-back (`0x7bbec0`) floors, so 9.75% of colours come back a
/// step low on one channel.
fn after_round_trip(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let mut cs = benilla_ui::widget::ColorSelectState::default();
    cs.set_rgb(r, g, b);
    cs.rgb_f64()
}

// ── The window's own API surface ─────────────────────────────────────────────────────────────────

/// The four names addons address exist, and both buttons' `OnClick` is fetchable:
/// `AceConsole-2.0.lua:1402` wraps the Okay button's handler, and skips its option without it.
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
    assert!(!s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetColorRGB` paints the swatch through `OnColorSelect` (`ColorPickerFrame.xml:171-176`), and
/// `GetColorRGB` reads back the quantized colour. Checked on the draw list: `SetTexture(r, g, b)`
/// replaces the texture with a solid colour, so `GetVertexColor` still reads the XML's white.
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

/// `hasOpacity` shows the slider and widens the window to 365 (`ColorPickerFrame.xml:161-170`).
#[test]
fn has_opacity_shows_the_slider_and_widens_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();

    // Without it: no slider, the narrow window. `GetWidth` reads the resolved rect.
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

/// Every slider change runs `opacityFunc` (`ColorPickerFrame.xml:147-151`).
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

// ── Dewdrop-2.0's colour sequence ────────────────────────────────────────────────────────────────

/// `Dewdrop-2.0.lua`'s colour-swatch `OnClick` in shape: a `func` closure over the addon's setter,
/// mirrored into `opacityFunc`, then `SetColorRGB`, a `cancelFunc` over the old values, the show.
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

/// Okay: the setter gets the quantized colour and the slider's alpha, and the window closes.
#[test]
fn the_dewdrop_sequence_commits_on_okay() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = picker();
    dewdrop_open(&s, 0.1, 0.5, 0.9, 0.25);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The preview ran twice before the window shows: `SetColorRGB` fires `OnColorSelect` into
    // `func`, then `OnShow`'s `SetValue` fires `OnValueChanged` into `opacityFunc`, the same one.
    assert_eq!(s.eval::<usize>("return table.getn(applied)").unwrap(), 2);
    assert!(s
        .eval::<bool>("return ColorPickerFrame:IsVisible()")
        .unwrap());
    // OnShow seeded the slider at `1 - opacity`.
    assert_eq!(
        s.eval::<f64>("return OpacitySliderFrame:GetValue()")
            .unwrap(),
        0.75
    );

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
    // A Slider holds `f32`, as the client's `CSimpleSlider` does, so this is not exactly 0.6.
    assert!(
        (a - 0.6).abs() < 1e-6,
        "alpha is 1 - the slider's 0.4, got {a}"
    );
}

/// Cancel restores the addon's pre-open colour through `cancelFunc` (`ColorPickerFrame.xml:71-73`).
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
    // Not exact: the slider's `valueStep="0.01"` rebuilds the value as `n * step + min`.
    assert!(
        (mid_a - 0.95).abs() < 1e-6,
        "alpha is 1 - the slider's 0.05, got {mid_a}"
    );

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

/// The ESC ladder (`ToggleGameMenu`) hides the picker as a `UISpecialFrames` row
/// (`UIParent.lua:52-55`), a bare hide that leaves the previewed colour. `cancelFunc` runs from
/// the Cancel button and the frame's own ESC `OnKeyDown` (`ColorPickerFrame.xml:177-184`), never
/// from the ladder.
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
    let (r, g, b): (f64, f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.r, t.g, t.b")
        .unwrap();
    assert_eq!(
        (r, g, b),
        (1.0, 0.0, 0.0),
        "ESC hid the picker and left the previewed colour applied"
    );
    s.run("ColorPickerFrame:Show() ColorPickerCancelButton:Click()")
        .unwrap();
    let (r, g, b): (f64, f64, f64) = s
        .eval("local t = applied[table.getn(applied)] return t.r, t.g, t.b")
        .unwrap();
    assert_eq!((r, g, b), (0.1, 0.5, 0.9), "Cancel runs cancelFunc");
}

/// `AceConsole-2.0.lua:1402-1406` in shape: the Okay button's handler, fetched with `GetScript`
/// and replaced through `SetScript` by one that calls it first.
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

/// `CloseWindows` walks `UISpecialFrames`, which lists `ColorPickerFrame` (`UIParent.lua:52-55`),
/// and hides it plainly, with no cancel.
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

// ── The dropdown's colour-swatch row ─────────────────────────────────────────────────────────────

/// A `hasColorSwatch` row (fields per `UIDropDownMenu.lua:114-125`) shows its square in `r/g/b`;
/// clicking it opens the picker through `UIDropDownMenuButton_OpenColorPicker`
/// (`UIDropDownMenu.lua:801-815`), which captures `previousValues` for Cancel to hand back.
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
    // `swatchFunc` became the picker's live-preview `func`, which has already run once.
    assert_eq!(s.eval::<usize>("return table.getn(picked)").unwrap(), 1);

    // Cancel hands the addon `previousValues`, the table the opener captured.
    s.run("ColorPickerCancelButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return restored[1]").unwrap(),
        "0.2,0.4,0.6,0.3",
        "cancelFunc(previousValues) carries r/g/b/opacity straight off the row"
    );
}

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

// ── The wheel: the window's generated art ────────────────────────────────────────────────────────

/// The wheel, the value strip and both thumbs sit at the stock geometry
/// (`ColorPickerFrame.xml:186-221`), and `ColorPickerWheel` publishes its name, which the strip's
/// anchor and reskinning addons use.
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

    // The two thumbs: one BLP, two crops.
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

/// A click on the wheel reaches the caller's `func` with the colour under the cursor; the right
/// rim is hue 180, cyan.
#[test]
fn a_click_on_the_wheel_reaches_the_callers_func() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = picker();
    // The Dewdrop block above, reduced to its `func`.
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
    // The window's own preview swatch followed, through OnColorSelect.
    let swatch = s.extract().into_iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { color: Some(c), .. }
            if c[0] < 0.05 && c[1] > 0.95)
    });
    assert!(swatch, "ColorSwatch repainted to the picked colour");
}

/// The file-less regions carry only what moves a pixel: the disc no colour at all (it draws at
/// V = 1), the strip hue and saturation but not value.
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
    // (0.15, 0.55, 0.75) is hue 200, saturation 0.80 after the 8-bit quantize on the way in.
    assert!(
        (strip.0 - 200.0).abs() < 0.5 && (strip.1 - 0.801).abs() < 0.01,
        "the strip carries the live hue/sat, got {strip:?}"
    );

    // A lower brightness leaves the strip's content unchanged.
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
