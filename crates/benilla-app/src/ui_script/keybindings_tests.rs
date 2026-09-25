//! The options window's Keybindings page (`KeyBindingsPage.xml`, its body in `OptionsFrame.xml`)
//! over the engine's binding table. Deviation: it is the era Settings panel's page, because 1.12's
//! settings screens are much worse to use; capture and set follow 1.12's `Blizzard_BindingUI`.

use benilla_ui::script::keybind::{KeybindCommand, KeybindRequest};
use benilla_ui::script::{QuadContent, UiScript};

use crate::bindings::commands::SPECS;

/// The page's files in manifest order, the registry seeded first, as `seed_bindings_for_vm` does.
pub(crate) fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    let cmds: Vec<KeybindCommand> = SPECS
        .iter()
        .map(|spec| KeybindCommand {
            name: spec.name,
            category: spec.category,
            run_on_up: spec.run_on_up(),
            default1: spec.d1,
            default2: spec.d2,
        })
        .collect();
    s.register_bindings(&cmds);
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "GameMenuFrame.xml",
    ] {
        // Strict for ours: an unknown template only warns, so a skinless window would pass.
        if file == "KeyBindingsPage.xml" || file == "OptionsFrame.xml" {
            super::test_ui::load_ui_strict(&s, file);
        } else {
            super::test_ui::load_ui(&s, file);
        }
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// A label as the page resolves it: the GlobalStrings global named `token`, else `raw`.
pub(crate) fn label(s: &UiScript, token: &str, raw: &str) -> String {
    s.eval::<String>(&format!(r#"return KeyBindings_String("{token}", "{raw}")"#))
        .unwrap()
}

/// Open the options window on the Keybindings page.
pub(crate) fn on_page(s: &mut UiScript) {
    s.run(r#"ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrame_SelectCategory("Keybindings")"#)
        .unwrap();
    assert!(s.errors().is_empty(), "on page: {:?}", s.errors());
}

const ROW: &str = "BenillaOptionsFrameContainerBodyKeybindingsRow";

#[test]
fn the_page_is_an_options_category_with_the_collapsed_honest_tree() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyKeybindings:IsVisible()")
        .unwrap());
    assert!(
        s.eval::<bool>("return BenillaOptionsFrameContainerUnbind:IsVisible()")
            .unwrap(),
        "Unbind Key exists on this page"
    );
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameContainerUnbind:IsEnabled() ~= 0")
            .unwrap(),
        "…disabled until a capsule is selected"
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());
    // The categories in `Bindings.xml` order, each a header collapsed by default, as in the era.
    let mut expected: Vec<&str> = Vec::new();
    for spec in SPECS {
        if !expected.contains(&spec.category) {
            expected.push(spec.category);
        }
    }
    for (i, token) in expected.iter().enumerate() {
        let text = s
            .eval::<String>(&format!("return {ROW}{}HeaderText:GetText()", i + 1))
            .unwrap();
        assert_eq!(text, label(&s, token, token), "section {}", i + 1);
    }
    assert!(
        !s.eval::<bool>(&format!("return {ROW}{}:IsVisible()", expected.len() + 1))
            .unwrap(),
        "all sections collapsed: nothing past the headers"
    );
    assert!(expected.contains(&"BINDING_HEADER_MULTIACTIONBAR"));
    s.run(&format!("{ROW}1Header:Click()")).unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}2Description:GetText()"))
            .unwrap(),
        label(&s, "BINDING_NAME_MOVEANDSTEER", "MOVEANDSTEER")
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}2Key1ButtonText:GetText()"))
            .unwrap(),
        label(&s, "KEY_BUTTON3", "BUTTON3")
    );
    // The strings pinned by hand, so the expectation is literal 1.12 text.
    s.run(
        r#"BINDING_HEADER_MOVEMENT = "Movement Keys"
             BINDING_NAME_MOVEANDSTEER = "Move and Steer"
             KEY_BUTTON3 = "Middle Mouse"
             KeyBindingsPage_Update()"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}1HeaderText:GetText()"))
            .unwrap(),
        "Movement Keys"
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}2Description:GetText()"))
            .unwrap(),
        "Move and Steer"
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}2Key1ButtonText:GetText()"))
            .unwrap(),
        "Middle Mouse"
    );
    // Collapsing slides the next header back under the first.
    s.run(&format!("{ROW}1Header:Click()")).unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}2HeaderText:GetText()"))
            .unwrap(),
        label(&s, "BINDING_HEADER_CHAT", "BINDING_HEADER_CHAT")
    );
    s.run(r#"BenillaOptionsFrame_SelectCategory("Controls")"#)
        .unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyKeybindings:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerUnbind:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn the_capture_flow_binds_steals_and_refuses_like_112() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    s.run(&format!("{ROW}1Header:Click()")).unwrap(); // expand Movement
    s.take_keybind_requests();
    // Row 3 is MOVEFORWARD (W, UP); selecting its Key 1 capsule arms the capture.
    assert!(!s.bind_capture_armed());
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    assert!(
        s.bind_capture_armed(),
        "a selected capsule arms the capture"
    );
    assert!(
        s.eval::<bool>("return BenillaOptionsFrameContainerUnbind:IsEnabled() ~= 0")
            .unwrap(),
        "Unbind arms with the selection"
    );
    // J, a key no command holds, takes slot 1 from W; UP stays in slot 2 and the bind saves.
    s.run(r#"KeyBindings_OnHostKey("J")"#).unwrap();
    assert!(!s.bind_capture_armed(), "a completed bind disarms");
    assert!(s
        .eval::<bool>(
            r#"local k1, k2 = GetBindingKey("MOVEFORWARD"); return k1 == "J" and k2 == "UP""#
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("W")"#).unwrap(),
        "",
        "the old key is free"
    );
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyKeybindingsOutput:GetText()")
            .unwrap(),
        "Key Bound Successfully"
    );
    // T is ATTACKTARGET's only key, so taking it names the victim in red (1.12's
    // `KEY_UNBOUND_ERROR`, `Blizzard_BindingUI.lua:185-190`).
    s.run(&format!("{ROW}3Key2Button:Click()")).unwrap();
    s.run(r#"KeyBindings_OnHostKey("T")"#).unwrap();
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("T")"#).unwrap(),
        "MOVEFORWARD"
    );
    let victim = label(&s, "BINDING_NAME_ATTACKTARGET", "ATTACKTARGET");
    assert!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyKeybindingsOutput:GetText()")
            .unwrap()
            .contains(&victim),
        "the newly-bare victim is named"
    );
    // The wheel binds onto a press+release command too: `0x4b7490` never reads the command.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    s.run(r#"KeyBindings_OnHostKey("MOUSEWHEELUP")"#).unwrap();
    assert!(
        s.eval::<bool>(r#"local k1 = GetBindingKey("MOVEFORWARD"); return k1 == "MOUSEWHEELUP""#)
            .unwrap(),
        "the notch takes the slot"
    );
    // It was the camera zoom's last key, so the page names that in red.
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyKeybindingsOutput:GetText()")
            .unwrap(),
        format!(
            "|cffff0000{} Function is Now Unbound!|r",
            label(&s, "BINDING_NAME_CAMERAZOOMIN", "CAMERAZOOMIN")
        )
    );
    // A key string the engine rejects restores the slot's old key and shows the only refusal text
    // 1.12 has, the wheel's (`KeyBindingFrame_SetBinding`, `Blizzard_BindingUI.lua:260-270`).
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    s.run(r#"KeyBindings_OnHostKey("SCROLLLOCK")"#).unwrap();
    assert!(
        s.eval::<bool>(r#"local k1 = GetBindingKey("MOVEFORWARD"); return k1 == "MOUSEWHEELUP""#)
            .unwrap(),
        "the refused slot restored its key"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyKeybindingsOutput:GetText()")
            .unwrap(),
        "Can't bind mousewheel to actions with up and down states"
    );
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    assert!(s.bind_capture_armed());
    s.run(&format!(r#"{ROW}3Key1Button:Click("RightButton")"#))
        .unwrap();
    assert!(!s.bind_capture_armed(), "right-click deselects");
    // Hiding the window disarms: an armed capture with no window would swallow all input.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    assert!(s.bind_capture_armed());
    s.run("HideUIPanel(BenillaOptionsFrame)").unwrap();
    assert!(!s.bind_capture_armed(), "OnHide disarms");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn unbind_reset_and_the_live_commit_replace_okay_cancel() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    s.run(&format!("{ROW}1Header:Click()")).unwrap(); // expand Movement
    s.take_keybind_requests();
    // JUMP is Movement's 8th command, row 9. Unbinding its Key 1 (SPACE) slides NUMPAD0 into
    // slot 1, as 1.12's Unbind does (`Blizzard_BindingUI.xml:507-527`).
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}9Description:GetText()"))
            .unwrap(),
        label(&s, "BINDING_NAME_JUMP", "JUMP")
    );
    s.run(&format!("{ROW}9Key1Button:Click()")).unwrap();
    s.run("BenillaOptionsFrameContainerUnbind:Click()").unwrap();
    assert!(s
        .eval::<bool>(
            r#"local k1, k2 = GetBindingKey("JUMP"); return k1 == "NUMPAD0" and k2 == nil"#
        )
        .unwrap());
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    // Deviation: a bind saves at once, as the era panel commits, where 1.12 had Okay and Cancel;
    // closing the window keeps it.
    s.run(&format!("{ROW}9Key1Button:Click()")).unwrap();
    s.run(r#"KeyBindings_OnHostKey("G")"#).unwrap();
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    s.run("BenillaOptionsFrameCloseButton:Click()").unwrap();
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("G")"#).unwrap(),
        "JUMP",
        "closing keeps the live-committed bind"
    );
    // Defaults, behind a confirm, restores JUMP's default keys and saves.
    on_page(&mut s);
    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert!(s
        .eval::<bool>(
            r#"local k1, k2 = GetBindingKey("JUMP"); return k1 == "SPACE" and k2 == "NUMPAD0""#
        )
        .unwrap());
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn the_esc_ladder_closes_the_window_and_the_checkbox_switches_sets() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    // ESC's options rung hides the window; with binds already saved there is nothing to revert.
    s.run("ToggleGameMenu()").unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrame:IsVisible()")
        .unwrap());

    on_page(&mut s);
    s.take_keybind_requests();
    s.run("BenillaOptionsFrameContainerBodyKeybindingsCharacterRowCheck:Click()")
        .unwrap();
    assert_eq!(s.current_binding_set(), 2);
    assert!(s.character_bindings_exist());
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(2)]);
    // Unchecking deletes the set, so the box springs back until 1.12's confirm decides; Cancel
    // keeps it.
    s.run("BenillaOptionsFrameContainerBodyKeybindingsCharacterRowCheck:Click()")
        .unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "leaving the character set confirms the permanent delete"
    );
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyKeybindingsCharacterRowCheck:GetChecked() ~= nil"
        )
        .unwrap(),
        "the box springs back until the popup decides"
    );
    s.run("StaticPopup1Button2:Click()").unwrap();
    assert_eq!(s.current_binding_set(), 2);
    assert!(s.character_bindings_exist());
    // Accept: the account set loads before the save, so it does not inherit the character binds.
    s.run("BenillaOptionsFrameContainerBodyKeybindingsCharacterRowCheck:Click()")
        .unwrap();
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(s.current_binding_set(), 1);
    assert!(
        !s.character_bindings_exist(),
        "the confirmed delete dropped set 2"
    );
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn search_surfaces_bindings_as_live_rows_under_the_redirect_head() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(r#"BINDING_NAME_JUMP = "Jump""#).unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.take_keybind_requests();
    // "jump" matches one binding and no CVar row.
    s.run(r#"BenillaOptionsFrameSearchBox:SetText("jump")"#)
        .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodySearchHeadKeybindings:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyKeybindSearch1:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyKeybindSearch1Description:GetText()"
        )
        .unwrap(),
        "Jump"
    );
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyKeybindSearch2:IsVisible()")
        .unwrap());
    s.run("BenillaOptionsFrameContainerBodyKeybindSearch1Key1Button:Click()")
        .unwrap();
    assert!(s.bind_capture_armed());
    s.run(r#"KeyBindings_OnHostKey("H")"#).unwrap();
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("H")"#).unwrap(),
        "JUMP"
    );
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyKeybindSearch1Key1ButtonText:GetText()"
        )
        .unwrap(),
        "H"
    );
    s.run("BenillaOptionsFrameContainerBodySearchHeadKeybindings:Click()")
        .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrame.selectedCategory")
            .unwrap(),
        "Keybindings"
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyKeybindings:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyKeybindSearch1:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn the_action_bar_abbreviation_is_the_refs_own_getbindingtext() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    // Stock `GetBindingText` (`UIParent.lua:1819`): one modifier abbreviates…
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("SHIFT-2", "KEY_", 1)"#)
            .unwrap(),
        "s-2",
        "the director's SHIF… truncation reads s-2 now"
    );
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("ALT-Z", "KEY_", 1)"#)
            .unwrap(),
        "a-Z"
    );
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("W", "KEY_", 1)"#)
            .unwrap(),
        "W"
    );
    // …two or more collapse to a dot, `CTRL--` included: the dash-counting loop counts its second
    // dash as a modifier.
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("CTRL-SHIFT-2", "KEY_", 1)"#)
            .unwrap(),
        "·"
    );
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("CTRL--", "KEY_", 1)"#)
            .unwrap(),
        "·"
    );
    // The full form keeps the prefixes and reads the KEY_* global when it exists.
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("SHIFT-2", "KEY_")"#)
            .unwrap(),
        "SHIFT-2"
    );
    s.run(r#"KEY_SPACE = "Spacebar""#).unwrap();
    assert_eq!(
        s.eval::<String>(r#"return GetBindingText("SPACE", "KEY_")"#)
            .unwrap(),
        "Spacebar"
    );
    assert!(s
        .eval::<bool>(r#"return GetBindingText(nil) == """#)
        .unwrap());
}

/// A spin bubbles up the parent chain, so the wheel handler sits on the mouse-enabled page body,
/// which also catches a spin over a row's name. The bar sits in the gutter inside the body.
#[test]
fn the_wheel_bubbles_from_the_rows_and_the_bar_rides_the_gutter() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    // Every section open: over 100 rows overflow the list's 19 slots.
    s.run(
        r#"for i = 1, table.getn(KeyBindingsPage.sections) do KeyBindings_ExpandSection(i, true) end
           KeyBindingsPage_Update()"#,
    )
    .unwrap();
    const SF: &str = "BenillaOptionsFrameContainerBodyKeybindingsScrollFrame";
    assert!(s
        .eval::<bool>(&format!("return {SF}ScrollBar:IsVisible()"))
        .unwrap());
    s.resolve();
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    // Only the list's bar shows in the shared gutter. The outer page's range is not 0, as the
    // reference would measure it: the stock faux list sizes its scroll child to
    // `numItems * valueStep`, and the range unions the child's whole subtree (`0x786f80`).
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameContainerScrollBar:IsVisible()")
            .unwrap(),
        "only one bar is on screen: the list's own owns the gutter"
    );
    let body_right = s
        .eval::<f64>("return BenillaOptionsFrameContainerBodyKeybindings:GetRight()")
        .unwrap();
    let rows_right = s.eval::<f64>(&format!("return {ROW}1:GetRight()")).unwrap();
    let bar_left = s
        .eval::<f64>(&format!("return {SF}ScrollBar:GetLeft()"))
        .unwrap();
    let bar_right = s
        .eval::<f64>(&format!("return {SF}ScrollBar:GetRight()"))
        .unwrap();
    assert!(
        bar_left >= rows_right,
        "bar starts right of the rows ({bar_left} vs {rows_right})"
    );
    assert!(
        bar_right <= body_right - 8.0,
        "bar inset from the body edge ({bar_right} vs body {body_right})"
    );
    // The trough seats itself on the bar (`BenillaScrollTrough_Seat`): 31 wide, 8 left of it,
    // 21 above and 20 below, which leaves 5 units over the up arrow and 4 under the down arrow.
    const TROUGH: &str = "BenillaOptionsFrameContainerBodyKeybindingsScrollFrameScrollBarTrough";
    let bar = format!("{SF}ScrollBar");
    let g = |f: &str, m: &str| s.eval::<f64>(&format!("return {f}:{m}()")).unwrap();
    assert!(
        s.eval::<bool>(&format!("return {TROUGH}:IsVisible()"))
            .unwrap(),
        "the trough shows with the bar"
    );
    assert!((g(TROUGH, "GetWidth") - 31.0).abs() < 0.01);
    assert!((bar_left - g(TROUGH, "GetLeft") - 8.0).abs() < 0.01);
    assert!((g(TROUGH, "GetTop") - g(&bar, "GetTop") - 21.0).abs() < 0.01);
    assert!((g(&bar, "GetBottom") - g(TROUGH, "GetBottom") - 20.0).abs() < 0.01);
    assert!(
        (g(TROUGH, "GetTop") - g(&format!("{bar}ScrollUpButton"), "GetTop") - 5.0).abs() < 0.01
    );
    assert!(
        (g(&format!("{bar}ScrollDownButton"), "GetBottom") - g(TROUGH, "GetBottom") - 4.0).abs()
            < 0.01
    );
    let set_all = |s: &UiScript, open: bool| {
        s.run(&format!(
            "for i = 1, table.getn(KeyBindingsPage.sections) do KeyBindings_ExpandSection(i, {}) end
             KeyBindingsPage_Update()",
            if open { "true" } else { "nil" }
        ))
        .unwrap();
    };
    set_all(&s, false);
    for f in [format!("{SF}ScrollBar"), TROUGH.to_string()] {
        assert!(
            !s.eval::<bool>(&format!("return {f}:IsVisible()")).unwrap(),
            "{f} goes away when the list fits"
        );
    }
    set_all(&s, true);
    s.resolve();

    // A spin over a row's name, which no child frame claims.
    let steer = label(&s, "BINDING_NAME_MOVEANDSTEER", "MOVEANDSTEER");
    let quads = s.extract();
    let (wx, wy) = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if *t == steer => q
                .rect
                .map(|r| ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)),
            _ => None,
        })
        .expect("a MOVEANDSTEER description quad");
    s.mouse_wheel(wx, wy, -1.0);
    assert!(s.errors().is_empty(), "wheel over a name: {:?}", s.errors());
    // A spin moves half the bar's height (`ScrollFrameTemplate_OnMouseWheel`,
    // `UIPanelTemplates.lua:150-157`), more than a row.
    let after_name = s
        .eval::<f64>(&format!("return FauxScrollFrame_GetOffset({SF})"))
        .unwrap();
    assert!(
        after_name > 1.0,
        "a spin over a row NAME scrolled the list by a page, got {after_name}"
    );
    // Over a capsule the spin bubbles through the row to the body. `GetLeft` answers in the
    // frame's own scale and the pointer is in screen pixels, hence `GetEffectiveScale`.
    let (cx, cy) = {
        let k = s
            .eval::<f64>(&format!("return {ROW}2Key1Button:GetEffectiveScale()"))
            .unwrap();
        let l = s
            .eval::<f64>(&format!("return {ROW}2Key1Button:GetLeft()"))
            .unwrap();
        let r = s
            .eval::<f64>(&format!("return {ROW}2Key1Button:GetRight()"))
            .unwrap();
        let t = s
            .eval::<f64>(&format!("return {ROW}2Key1Button:GetTop()"))
            .unwrap();
        let b = s
            .eval::<f64>(&format!("return {ROW}2Key1Button:GetBottom()"))
            .unwrap();
        ((((l + r) * 0.5 * k) as f32), (((t + b) * 0.5 * k) as f32))
    };
    s.mouse_wheel(cx, cy, -1.0);
    let after_capsule = s
        .eval::<f64>(&format!("return FauxScrollFrame_GetOffset({SF})"))
        .unwrap();
    assert!(
        after_capsule > after_name,
        "a spin over a CAPSULE bubbles to the page too: {after_name} -> {after_capsule}"
    );
    // While a capsule is armed a spin binds, never scrolls; the page's guard catches a spin the
    // host lets through.
    s.run(&format!("{ROW}2Key1Button:Click()")).unwrap();
    s.mouse_wheel(wx, wy, -1.0);
    assert_eq!(
        s.eval::<f64>(&format!("return FauxScrollFrame_GetOffset({SF})"))
            .unwrap(),
        after_capsule,
        "armed: the wheel must not scroll"
    );
    s.run("KeyBindings_SetSelected(nil)").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The pet bar's bindings, 1.12's `BONUSACTIONBUTTON1-10` (`BonusActionBarFrame.lua:106-112`),
/// file under the action bar's header (`Bindings.xml:121`, `:321-390`) with `CTRL-1..CTRL-0`
/// defaults, and the page's search finds them.
#[test]
fn the_pet_lane_is_registered_under_the_action_bar_header() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);

    let category = |s: &mut UiScript, name: &str| {
        s.eval::<String>(&format!(
            r#"for i = 1, GetNumBindings() do
                   local n, c = GetBinding(i)
                   if n == "{name}" then return c end
               end"#
        ))
        .unwrap()
    };
    for i in 1..=10 {
        let name = format!("BONUSACTIONBUTTON{i}");
        assert_eq!(
            category(&mut s, &name),
            "BINDING_HEADER_ACTIONBAR",
            "{name} files under the action bar, as 1.12 does"
        );
        let key = s
            .eval::<String>(&format!(r#"return GetBindingKey("{name}")"#))
            .unwrap();
        assert_eq!(key, format!("CTRL-{}", i % 10), "{name}'s 1.12 default");
    }
    // `runOnUp` decides that a press also delivers the release, not whether the wheel may bind.
    assert_eq!(
        s.eval::<Option<u32>>(r#"return SetBinding("MOUSEWHEELUP", "BONUSACTIONBUTTON1")"#)
            .unwrap(),
        Some(1),
        "a press+release command takes the wheel — the refusal was ours, B265"
    );

    // Search matches display names (the era's `AddSearchTags`), so the query is the row's label.
    let query = label(&s, "BINDING_NAME_BONUSACTIONBUTTON1", "BONUSACTIONBUTTON1").to_lowercase();
    s.run(&format!(
        r#"BenillaOptionsFrameSearchBox:SetText("{query}")"#
    ))
    .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodySearchHeadKeybindings:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyKeybindSearch1Description:GetText()"
        )
        .unwrap(),
        label(&s, "BINDING_NAME_BONUSACTIONBUTTON1", "BONUSACTIONBUTTON1"),
        "the row wears the app's own string"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyKeybindSearch1Key1ButtonText:GetText()"
        )
        .unwrap(),
        "CTRL-1"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `loadstring` on each command body: running one would need FrameXML this test does not load.
#[test]
fn every_command_body_compiles() {
    use crate::bindings::commands::Kind;

    let s = UiScript::new().unwrap();
    for spec in SPECS {
        let bodies: Vec<&str> = match &spec.kind {
            Kind::Held | Kind::Host => Vec::new(),
            Kind::Edge(body) => vec![*body],
            Kind::EdgeUpDown(down, up) => vec![*down, *up],
        };
        for body in bodies {
            s.run(&format!(
                "local f, err = loadstring({body:?});                  if not f then BenillaBodyError = {name:?} .. \": \" .. err end",
                body = body,
                name = spec.name
            ))
            .unwrap_or_else(|e| panic!("{}: {e}", spec.name));
        }
    }
    let bad: Vec<String> = s.errors();
    assert!(bad.is_empty(), "script errors: {bad:?}");
    s.run("if BenillaBodyError then error(BenillaBodyError) end")
        .unwrap_or_else(|e| panic!("a command body is not valid Lua: {e}"));
}
