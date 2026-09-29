//! The options window's Keybindings page (`KeyBindingsPage.xml`, its body in `OptionsFrame.xml`)
//! over the engine's binding table. Deviation: it is the era Settings panel's page, because 1.12's
//! settings screens are much worse to use; capture and set follow 1.12's `Blizzard_BindingUI`.

use benilla_ui::script::keybind::KeybindRequest;
use benilla_ui::script::{QuadContent, UiScript};

/// The page's files in the production order over the stock commands and the install's defaults.
pub(crate) fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    crate::ui_script::load_stock_bindings(&mut s);
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // Before our files, as the core loads before the layer: it sources UIParent.lua again.
        r"Interface\FrameXML\GameMenuFrame.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "ScrollTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "GameMenuAdapters.xml",
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

/// The install's defaults live, as a first login's account set is, on a VM whose load registered
/// the stock commands.
pub(crate) fn seed_defaults(s: &mut UiScript) {
    s.set_default_bindings(crate::ui_script::default_bindings());
    s.load_binding_set(1);
}

/// A label as the page resolves it: the GlobalStrings global named `token`, else `raw`.
pub(crate) fn label(s: &UiScript, token: &str, raw: &str) -> String {
    s.eval::<String>(&format!(
        r#"return BenillaKeyBindings_String("{token}", "{raw}")"#
    ))
    .unwrap()
}

/// Open the options window on the Keybindings page.
pub(crate) fn on_page(s: &mut UiScript) {
    s.run(r#"ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrame_SelectCategory("Keybindings")"#)
        .unwrap();
    assert!(s.errors().is_empty(), "on page: {:?}", s.errors());
}

const ROW: &str = "BenillaOptionsFrameContainerBodyKeybindingsRow";

/// Whether the window takes keys and the wheel: a selected capsule arms it, as 1.12's
/// keyboard-enabled `KeyBindingFrame` (`Blizzard_BindingUI.xml:73`).
pub(crate) fn armed(s: &UiScript) -> bool {
    s.eval::<bool>(
        "return BenillaOptionsFrame:IsKeyboardEnabled() == 1 \
            and BenillaOptionsFrame:IsMouseWheelEnabled() == 1 \
            and BenillaKeyBindingsPage.selected ~= nil",
    )
    .unwrap()
}

/// A key press as the host feeds it (`ui_script::input`): ENTER, ESCAPE and TAB through the named
/// walk, every other key through the frame walk by its 1.12 name. Answers whether a frame took it.
pub(crate) fn press(s: &mut UiScript, key: &str) -> bool {
    match key {
        "ENTER" | "ESCAPE" | "TAB" => s.key_input(key),
        _ => s.frame_key_input(key),
    }
}

/// A frame's centre in screen pixels, where the pointer feed hits it.
pub(crate) fn centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (l, r, t, b, k): (f64, f64, f64, f64, f64) = s
        .eval(&format!(
            "local f = {frame} \
             local k = (f.GetEffectiveScale and f or f:GetParent()):GetEffectiveScale() \
             return f:GetLeft(), f:GetRight(), f:GetTop(), f:GetBottom(), k"
        ))
        .unwrap();
    (((l + r) * 0.5 * k) as f32, ((t + b) * 0.5 * k) as f32)
}

/// A click of `button` on `frame` through the pointer feed.
pub(crate) fn click(s: &mut UiScript, frame: &str, button: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_move(x, y);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// One wheel notch over the page, through the pointer feed.
pub(crate) fn wheel(s: &mut UiScript, delta: f32) {
    let (x, y) = centre(s, "BenillaOptionsFrameContainerBodyKeybindings");
    s.mouse_wheel(x, y, delta);
}

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
    // The categories in `Bindings.xml` order, each a header collapsed by default, as in the era;
    // the spacer headers over multibars 2-4 continue the section above (the page's deviation).
    let stock = benilla_ui::bindings_xml::parse(
        &crate::ui_script::stock_bindings_file().expect("the install's Bindings.xml"),
    )
    .unwrap();
    let mut expected: Vec<String> = Vec::new();
    for b in &stock {
        let Some(h) = &b.header else { continue };
        let token = format!("BINDING_HEADER_{h}");
        if b.platform.is_none() && !token.starts_with("BINDING_HEADER_BLANK") {
            expected.push(token);
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
    assert!(expected
        .iter()
        .any(|h| h == "BINDING_HEADER_MULTIACTIONBAR"));
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
             BenillaKeyBindingsPage_Update()"#,
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
    // Unarmed, the window takes no key: it reaches the game's bindings.
    assert!(!armed(&s));
    assert!(!press(&mut s, "J"), "an unarmed page lets a key through");
    // Row 3 is MOVEFORWARD (W, UP); selecting its Key 1 capsule arms the window.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    assert!(armed(&s), "a selected capsule arms the window");
    assert!(
        s.eval::<bool>("return BenillaOptionsFrameContainerUnbind:IsEnabled() ~= 0")
            .unwrap(),
        "Unbind arms with the selection"
    );
    // A lone modifier is no key (`Blizzard_BindingUI.lua:172-174`): still armed.
    assert!(press(&mut s, "SHIFT"), "the armed window takes it");
    assert!(armed(&s), "…and waits for a real key");
    // J, a key no command holds, takes slot 1 from W; UP stays in slot 2 and the bind saves.
    assert!(press(&mut s, "J"), "the armed window takes the key");
    assert!(!armed(&s), "a completed bind disarms");
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
    press(&mut s, "T");
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
    // A chord: the modifiers held at the press prefix the key, ALT-CTRL-SHIFT order
    // (`Blizzard_BindingUI.lua:175-183`).
    s.run(&format!("{ROW}3Key2Button:Click()")).unwrap();
    s.set_modifiers(true, true, false);
    press(&mut s, "K");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("CTRL-SHIFT-K")"#)
            .unwrap(),
        "MOVEFORWARD"
    );
    // ESC while armed binds Escape: 1.12's armed branch has no ESCAPE filter.
    s.run(&format!("{ROW}3Key2Button:Click()")).unwrap();
    assert!(press(&mut s, "ESCAPE"));
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("ESCAPE")"#)
            .unwrap(),
        "MOVEFORWARD"
    );
    // The wheel binds onto a press+release command too: `0x4b7490` never reads the command.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    wheel(&mut s, 1.0);
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
    press(&mut s, "SCROLLLOCK");
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
    assert!(armed(&s));
    s.run(&format!(r#"{ROW}3Key1Button:Click("RightButton")"#))
        .unwrap();
    assert!(!armed(&s), "right-click deselects");
    // Hiding the window disarms.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    assert!(armed(&s));
    s.run("HideUIPanel(BenillaOptionsFrame)").unwrap();
    assert!(!armed(&s), "OnHide disarms");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Mouse buttons 3-5 bind through the clicks 1.12's window takes: a capsule's own `OnClick`
/// (`Blizzard_BindingUI.lua:246`) and the window's (`Blizzard_BindingUI.xml:538-540`), here the
/// page's `OnMouseUp`; left and right clicks only select.
#[test]
fn mouse_buttons_bind_through_the_capsules_and_the_page() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    s.run(&format!("{ROW}1Header:Click()")).unwrap(); // expand Movement
    let capsule = format!("{ROW}3Key1Button");
    click(&mut s, &capsule, "LeftButton");
    assert!(armed(&s), "a left click selects");
    click(&mut s, &capsule, "MiddleButton");
    assert!(!armed(&s));
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("BUTTON3")"#)
            .unwrap(),
        "MOVEFORWARD",
        "a middle click on the armed capsule binds BUTTON3"
    );
    // Over the page, away from any capsule: mouse 4 with SHIFT held.
    click(&mut s, &capsule, "LeftButton");
    s.set_modifiers(true, false, false);
    click(&mut s, &format!("{ROW}3Description"), "Button4");
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("SHIFT-BUTTON4")"#)
            .unwrap(),
        "MOVEFORWARD"
    );
    // Unarmed, a middle click on a capsule selects it, as 1.12's does.
    click(&mut s, &format!("{ROW}4Key2Button"), "MiddleButton");
    assert!(armed(&s));
    s.run("BenillaKeyBindings_SetSelected(nil)").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Loading the stock `Blizzard_BindingUI` (an addon manager's Load button does) defines its own
/// `KeyBindingButton_OnClick` and popup (`Blizzard_BindingUI.lua:228`, `:10`) and changes no other
/// window: the page keeps binding and leaving the character set. The stock window reads the 1.12
/// list: a `HEADER_*` row draws a section header, a command's keys fill Key 1 and Key 2.
#[test]
fn the_page_works_after_the_stock_binding_ui_loads_and_the_stock_window_reads_the_list() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) =
        super::layer_tests::production_load_with("bindingui", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    seed_defaults(&mut s);
    s.run(r#"assert(LoadAddOn("Blizzard_BindingUI") == 1)"#)
        .unwrap();
    assert!(s
        .eval::<bool>("return KeyBindingFrame ~= nil and KeyBindingButton_OnClick ~= nil")
        .unwrap());

    on_page(&mut s);
    s.run(&format!("{ROW}1Header:Click()")).unwrap(); // expand Movement
    s.take_keybind_requests();
    click(&mut s, &format!("{ROW}3Key1Button"), "LeftButton");
    assert!(armed(&s), "the page's capsule still arms its own window");
    assert!(
        s.eval::<bool>("return KeyBindingFrame.selected == nil")
            .unwrap(),
        "the stock window is untouched"
    );
    press(&mut s, "J");
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("J")"#).unwrap(),
        "MOVEFORWARD"
    );
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    // The character set, both ways, through the page's own popup.
    let check = "BenillaOptionsFrameContainerBodyKeybindingsCharacterRowCheck";
    s.run(&format!("{check}:Click()")).unwrap();
    assert_eq!(s.current_binding_set(), 2);
    s.run(&format!("{check}:Click()")).unwrap();
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(
        s.current_binding_set(),
        1,
        "leaving the character set works"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    s.run("HideUIPanel(BenillaOptionsFrame)").unwrap();

    // The stock window over the same list.
    s.run("ShowUIPanel(KeyBindingFrame)").unwrap();
    assert!(s
        .eval::<bool>("return KeyBindingFrame:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return KeyBindingFrameBinding1Header:IsVisible() \
                and KeyBindingFrameBinding1Header:GetText() == BINDING_HEADER_MOVEMENT"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>("return KeyBindingFrameBinding2Key1Button:GetText()")
            .unwrap(),
        s.eval::<String>(r#"return GetBindingText("BUTTON3", "KEY_")"#)
            .unwrap(),
        "Move and Steer's first key in the Key 1 column"
    );
    assert_eq!(
        s.eval::<String>("return KeyBindingFrameBinding3Key2Button:GetText()")
            .unwrap(),
        s.eval::<String>(r#"return GetBindingText("UP", "KEY_")"#)
            .unwrap(),
        "Move Forward's second key in the Key 2 column"
    );
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
    press(&mut s, "G");
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
    assert!(armed(&s));
    press(&mut s, "H");
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
        "SHIFT-2 reads s-2, not a SHIF… truncation"
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
        r#"for i = 1, table.getn(BenillaKeyBindingsPage.sections) do BenillaKeyBindings_ExpandSection(i, true) end
           BenillaKeyBindingsPage_Update()"#,
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
            "for i = 1, table.getn(BenillaKeyBindingsPage.sections) do BenillaKeyBindings_ExpandSection(i, {}) end
             BenillaKeyBindingsPage_Update()",
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
    // While a capsule is armed a spin binds, never scrolls.
    s.run(&format!("{ROW}2Key1Button:Click()")).unwrap();
    let armed_command = s
        .eval::<String>(&format!("return {ROW}2Key1Button.commandName"))
        .unwrap();
    s.mouse_wheel(wx, wy, -1.0);
    assert_eq!(
        s.eval::<f64>(&format!("return FauxScrollFrame_GetOffset({SF})"))
            .unwrap(),
        after_capsule,
        "armed: the wheel must not scroll"
    );
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("MOUSEWHEELDOWN")"#)
            .unwrap(),
        armed_command,
        "armed: the spin binds"
    );
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

    // The header row a command sits under: the last `HEADER_*` row above it.
    let category = |s: &mut UiScript, name: &str| {
        s.eval::<String>(&format!(
            r#"local header
               for i = 1, GetNumBindings() do
                   local n = GetBinding(i)
                   if strsub(n, 1, 6) == "HEADER" then header = n end
                   if n == "{name}" then return header end
               end"#
        ))
        .unwrap()
    };
    for i in 1..=10 {
        let name = format!("BONUSACTIONBUTTON{i}");
        assert_eq!(
            category(&mut s, &name),
            "HEADER_ACTIONBAR",
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
        "a press+release command takes the wheel — the reference does not refuse it"
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

/// `loadstring` on each stock command body: the reference compiles each at load (`0x704c70`),
/// and running one would need FrameXML this test does not load.
#[test]
fn every_command_body_compiles() {
    benilla_formats::wow_data_or_skip!();
    let stock = benilla_ui::bindings_xml::parse(
        &crate::ui_script::stock_bindings_file().expect("the install's Bindings.xml"),
    )
    .unwrap();
    let s = UiScript::new().unwrap();
    for b in &stock {
        s.run(&format!(
            "local f, err = loadstring({body:?}); \
             if not f then BenillaBodyError = {name:?} .. \": \" .. err end",
            body = b.body,
            name = b.name
        ))
        .unwrap_or_else(|e| panic!("{}: {e}", b.name));
    }
    let bad: Vec<String> = s.errors();
    assert!(bad.is_empty(), "script errors: {bad:?}");
    s.run("if BenillaBodyError then error(BenillaBodyError) end")
        .unwrap_or_else(|e| panic!("a command body is not valid Lua: {e}"));
}

/// Every function and object a stock command body names exists once the in-game UI has loaded:
/// the engine's behaviour sits behind the Lua globals the bodies call. Read off each body: a
/// `Name(` call is a function, an `Object:` or `Object.` receiver a frame or table.
#[test]
fn every_stock_body_calls_what_the_load_defines() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = super::layer_tests::production_load_with("bindcalls", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let stock = benilla_ui::bindings_xml::parse(
        &crate::ui_script::stock_bindings_file().expect("the install's Bindings.xml"),
    )
    .unwrap();
    const KEYWORDS: [&str; 14] = [
        "if", "then", "else", "elseif", "end", "not", "and", "or", "local", "function", "return",
        "do", "while", "for",
    ];
    let mut missing = Vec::new();
    for b in stock.iter().filter(|b| b.platform.is_none()) {
        let body = b.body.as_bytes();
        let mut i = 0;
        while i < body.len() {
            let c = body[i];
            if !(c.is_ascii_alphabetic() || c == b'_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < body.len() && (body[i].is_ascii_alphanumeric() || body[i] == b'_') {
                i += 1;
            }
            // A field or method name is its receiver's, not a global.
            if start > 0 && matches!(body[start - 1], b'.' | b':') {
                continue;
            }
            let name = &b.body[start..i];
            let next = b.body[i..].trim_start().chars().next();
            let want = match next {
                Some('(') if !KEYWORDS.contains(&name) => "function",
                Some(':') | Some('.') => "table",
                _ => continue,
            };
            let kind: String = s.eval(&format!("return type({name})")).unwrap();
            if kind != want {
                missing.push(format!("{}: {name} is {kind}", b.name));
            }
        }
    }
    assert!(missing.is_empty(), "unresolved: {missing:#?}");
}

/// ALT-Z runs the stock TOGGLEUI body (`Bindings.xml:655-661`): `CloseAllWindows()` and
/// `UIParent:Hide()`, then `UIParent:Show()`, so the interface under `UIParent` stops drawing and
/// an open window is closed, as in the reference.
#[test]
fn toggleui_hides_uiparent_and_closes_the_windows() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = super::layer_tests::production_load_with("toggleui", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    s.run("ShowUIPanel(GameMenuFrame)").unwrap();
    assert!(s
        .eval::<bool>("return GameMenuFrame:IsVisible() == 1")
        .unwrap());
    s.resolve();
    let lit = s.extract().len();
    assert!(s.execute_binding("TOGGLEUI", true).unwrap());
    s.resolve();
    assert!(s
        .eval::<bool>("return UIParent:IsVisible() == nil")
        .unwrap());
    assert!(s
        .eval::<bool>("return GameMenuFrame:IsShown() == nil")
        .unwrap());
    assert!(
        s.extract().len() < lit,
        "the frames under UIParent stop drawing"
    );
    assert!(s.execute_binding("TOGGLEUI", true).unwrap());
    assert!(s.eval::<bool>("return UIParent:IsVisible() == 1").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
