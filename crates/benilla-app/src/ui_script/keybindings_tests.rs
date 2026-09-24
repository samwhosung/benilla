//! The shipped `assets/ui/KeyBindingsPage.xml` + OptionsFrame.xml's Keybindings body — the
//! Options window's Keybindings category over the engine's binding table (
//! superseding 0997's standalone window; the provenance block in the XML).
//!
//! What these guard: the module + the options window load clean together; the page is an
//! ordinary category (body-swapped, Defaults live, the Unbind button only here); the section
//! tree is the honest tree's non-empty categories in 1.12 `Bindings.xml` order, COLLAPSED by
//! default (the era's own state) and toggling like the era's expandable sections; the capture
//! flow — select a capsule → the host arm arms → the canonical chord lands through
//! `KeyBindings_OnHostKey` — binds LIVE-COMMIT (every mutation queues the host persist,
//! 1008's law; closing the window keeps everything), steals with the red 1.12 message only
//! when the victim goes bare, takes the wheel onto ANY command (1295 — the engine refuses
//! nothing but a key string that is not a key, and that refusal restores the old key); the
//! character-specific checkbox runs 1.12's set model with the era's
//! confirm-on-uncheck-only; search surfaces binding matches as LIVE rows under the
//! Keybindings redirect head; and the action bar's abbreviation law (`GetBindingText`
//! transcribed in UIParent.xml) reads `s-2`, the ref's own Lua.
//!
//! Labels are read the way the page reads them — `KeyBindings_String(token, raw)`, the
//! GlobalStrings global with the raw token as fallback ([`label`]). The harness carries the
//! real 1.12 GlobalStrings.lua since the dialog engine became the reference's file (1960; the
//! stock StaticPopup.lua reads the strings at load), so a label here is the app's own
//! "Movement Keys" / "Jump", never the raw token the strings-less harness used to show.

use benilla_ui::script::keybind::{KeybindCommand, KeybindRequest};
use benilla_ui::script::{QuadContent, UiScript};

use crate::bindings::commands::SPECS;

/// The page's real neighbourhood, in the manifest's own order, with the registry seeded the
/// way the app seeds it (`crate::bindings::seed_bindings` — registration before any show).
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
        // `test_ui::load_ui`, not a local read: a manifest entry carrying a path separator is the
        // REFERENCE's own file and must come off the player's chain, which
        // `std::fs::read_to_string` under `assets/ui` cannot do — it goes looking for
        // `assets/ui/Interface/FrameXML/...` and fails. The shared loader resolves both shapes, and
        // its own doc already records this consolidation happening once before. Hand-rolling it
        // here is what made this kit break the moment a file it loads migrated (1751).
        // `load_ui_strict` for the files whose art actually matters here: an unknown template is
        // a loader WARNING, not an error, so a silently skinless window would otherwise pass.
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
    // The section tree: the registry's category tokens, first-appearance order — exactly
    // 1.12's file order — every section a COLLAPSED header row (the era default).
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
    // The multibar section is real (1008): its header token sits in the tree.
    assert!(expected.contains(&"BINDING_HEADER_MULTIACTIONBAR"));
    // Expanding Movement puts its rows under the header, byte-real defaults on the capsules.
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
    // The same rows with the strings pinned by hand, so the expectation is literal 1.12 text.
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
    // Collapse again: the next header slides back up under the first.
    s.run(&format!("{ROW}1Header:Click()")).unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}2HeaderText:GetText()"))
            .unwrap(),
        label(&s, "BINDING_HEADER_CHAT", "BINDING_HEADER_CHAT")
    );
    // Leaving the page hides its body and the Unbind button.
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
    s.take_keybind_requests(); // drop any noise before the flow under test
                               // Row 3 is MOVEFORWARD (W, UP). Selecting its Key 1 capsule arms the host seam.
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
    // The host hands back a canonical chord: J binds into slot 1, W's old seat; UP survives
    // in slot 2; the capture disarms; the table is LIVE and the bind COMMITTED (Save queued —
    // 1008's live-commit law, where 0997 waited for Okay).
    //
    // **`J`, not `F`** — this leg is about the plain bind, so its key has to be one nothing else
    // holds, and `F` stopped being one when 1745 registered `ASSISTTARGET` on its byte-real
    // default. The steal path has its own leg below (`T`), which is where a taken key belongs.
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
    // Stealing the LAST key of another command names the victim in red (1.12's
    // KEY_UNBOUND_ERROR). T is ATTACKTARGET's only key.
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
    // **The wheel binds like any other key** — including onto MOVEFORWARD,
    // which has press+release state. This block used to assert the opposite; `0x4b7490` never
    // reads a command node, so the refusal it asserted is not in the client.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    s.run(r#"KeyBindings_OnHostKey("MOUSEWHEELUP")"#).unwrap();
    assert!(
        s.eval::<bool>(r#"local k1 = GetBindingKey("MOVEFORWARD"); return k1 == "MOUSEWHEELUP""#)
            .unwrap(),
        "the notch takes the slot"
    );
    // …and the notch STEALS the wheel from the camera, whose last key it was — so the page says
    // so in red, exactly as it would for any other stolen key. That the wheel's own default
    // victim is a real command is half of why the old refusal read as plausible.
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyKeybindingsOutput:GetText()")
            .unwrap(),
        format!(
            "|cffff0000{} Function is Now Unbound!|r",
            label(&s, "BINDING_NAME_CAMERAZOOMIN", "CAMERAZOOMIN")
        )
    );
    // The refusal that IS there is the key-string validator, and this is the page arm that
    // reports it: a chord the engine will not take restores the slot's old key and says why
    // (1.12's KeyBindingFrame_SetBinding, its wheel-shaped message and all).
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
    // A right-click on the armed capsule deselects without binding.
    s.run(&format!("{ROW}3Key1Button:Click()")).unwrap();
    assert!(s.bind_capture_armed());
    s.run(&format!(r#"{ROW}3Key1Button:Click("RightButton")"#))
        .unwrap();
    assert!(!s.bind_capture_armed(), "right-click deselects");
    // Hiding the window disarms a straggling capture (the OnHide hook — a locked-out client
    // otherwise: the armed seam swallows all input with no window on screen).
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
    // JUMP is Movement's 8th command → row 9 under the header. Unbind its Key 1: SPACE goes,
    // NUMPAD0 slides into slot 1 (the 1.12 slot dance), and the change commits at once.
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
    // LIVE-COMMIT is the law now (1008): bind G, close the window — the bind KEEPS (0997's
    // Cancel/ESC revert died with the standalone window).
    s.run(&format!("{ROW}9Key1Button:Click()")).unwrap();
    s.run(r#"KeyBindings_OnHostKey("G")"#).unwrap();
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    s.run("BenillaOptionsFrameCloseButton:Click()").unwrap();
    assert_eq!(
        s.eval::<String>(r#"return GetBindingAction("G")"#).unwrap(),
        "JUMP",
        "closing keeps the live-committed bind"
    );
    // Reset To Default: the page's Defaults button behind the era confirm — JUMP's real
    // defaults return and the reset itself commits.
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
    // The ladder (the ESC binding's own body): the options rung hides the window — since 1008
    // that IS the whole gesture for keybinds too (live-commit; nothing to revert).
    s.run("ToggleGameMenu()").unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrame:IsVisible()")
        .unwrap());

    // The character-specific checkbox (1.12's set model, era confirm-on-uncheck-only law):
    // CHECK switches to the character set and saves it into existence at once.
    on_page(&mut s);
    s.take_keybind_requests();
    s.run("BenillaOptionsFrameContainerBodyKeybindingsCharacterRowCheck:Click()")
        .unwrap();
    assert_eq!(s.current_binding_set(), 2);
    assert!(s.character_bindings_exist());
    assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(2)]);
    // UNCHECK is destructive: the box springs back and the 1.12 confirm decides. Cancel
    // first — still on the character set.
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
    // Accept: back to the account set, the character set dropped (load-then-save order — the
    // account file must not inherit the character binds).
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
    // A query that matches one binding and no CVar row: the Keybindings group head shows,
    // with the match painted LIVE on the search pool (the era reflows its real rows the
    // same way).
    s.run(r#"BenillaOptionsFrameSearchBox:SetText("jump")"#)
        .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here (decision 1831)
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
    // The result row is LIVE: its capsule arms, the bind lands and commits, the row relabels.
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
    // The head is the era redirect: clicking it ends the search on the Keybindings page.
    s.run("BenillaOptionsFrameContainerBodySearchHeadKeybindings:Click()")
        .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here (decision 1831)
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
    // ref UIParent.lua:1819 transcribed (UIParent.xml): one modifier abbreviates…
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
    // …two or more collapse to the ref's dot — including the CTRL-- oddity, whose second
    // dash the ref's dash-counting loop counts as a second modifier (their quirk, pinned).
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

/// The wheel and the bar's seat (the director's scuff report, same day 1008 landed). The wheel
/// is the QuestLog/Trainer lesson relearned: a spin bubbles up the PARENT chain (pointer.rs),
/// so a handler on the sibling faux frame never sees it — it lives on the page body, which is
/// mouse-enabled so a spin over a row's NAME (a plain-Frame area no child claims) is caught
/// too, not just the bubbles from capsule/header Buttons. The bar rides the gutter INSIDE the
/// body: the kit hangs it 6px right of the faux frame's edge, so the frame ends at the rows'
/// own -32 — anchoring it to body-right hung the bar on the window border.
#[test]
fn the_wheel_bubbles_from_the_rows_and_the_bar_rides_the_gutter() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);
    // Every section open: 100+ flat rows — the list overflows its 19 slots and the bar shows.
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
    // The page area's OWN scroll stays out of it: this page fills the body exactly, so the
    // outer range is 0 and its bar never joins this one in the gutter the two share.
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    // **B217's property is the BAR, not the range, and since 1860 those differ.** The page scroll
    // now reports a nonzero range here, and that is not a defect in either half: the chain's
    // `FauxScrollFrameTemplate` carries a real `<ScrollChild>` which `FauxScrollFrame_Update` sizes
    // to `numItems * valueStep` (~5275px with every section open), our old template had none, and
    // the range is measured by unioning the scroll child's whole SUBTREE — the reference's own
    // `0x786f80` recursion, which re-enters itself for each shown child frame with no clip or
    // ScrollFrame exception. The reference would measure
    // the same; it simply never nests a faux list inside a real-scroll page, which our options
    // window is alone in doing.
    //
    // What B217 actually cleared — a second bar in the shared gutter — still holds, and that is
    // what is asserted.
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
    // …wearing its TROUGH: the recessed channel (the shared kit's, shown and hidden with the bar),
    // which seats ITSELF on the bar — 31 wide against the bar's 16, hung 8 units left, and
    // overhanging it 21/20 so each arrow drops into the 16-tall socket the art carries (the
    // law and its ref citation are in BenillaScrollTrough_Seat). Asserted off the ARROWS here,
    // which is the half the eye judges: 5 units of cap above the up arrow, 4 below the down arrow.
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
    // A list that FITS shows neither.
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

    // A spin over a row NAME — the dead zone when the handler lived on the sibling.
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
    // The spin REACHED the list, which is what this dead zone was about. The distance is the
    // reference's half-bar page (`ScrollFrameTemplate_OnMouseWheel` moves `GetHeight()/2` pixels),
    // not the single row our deleted kit stepped — 1860 re-pointed that everywhere it is asserted.
    let after_name = s
        .eval::<f64>(&format!("return FauxScrollFrame_GetOffset({SF})"))
        .unwrap();
    assert!(
        after_name > 1.0,
        "a spin over a row NAME scrolled the list by a page, got {after_name}"
    );
    // …and over a CAPSULE (a Button: the bubble path, capsule → row → body). GetLeft-family
    // reads are scale-LOCAL (the 1.12 contract; this window wears ERA_WINDOW_SCALE) — the
    // pointer lives in screen px, so the aim converts through GetEffectiveScale.
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
    // While a capsule is armed the wheel is a BIND, never a scroll (1.12's law) — in the app
    // the host seam swallows the spin before the UI sees one; the Lua guard is the belt for
    // a spin that reaches the page anyway.
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

/// **The pet lane reaches the page**. 1008 registered the two multibars
/// and stopped; 1.12's `BONUSACTIONBUTTON1-10` — "Secondary Action Button" in its own window —
/// never joined, so the pet bar was the one visible bar with no way to bind it.
///
/// Pinned on both sides of the seam, because the report is about the *page*: the registry carries
/// the ten rows under the ACTION BAR header with their byte-real `CTRL-1..CTRL-0` defaults (1.12
/// files them under l.121's header, having none of their own), and the page's own search — the
/// path a player actually finds a binding by — paints the row live with that key on its capsule.
#[test]
fn the_pet_lane_is_registered_under_the_action_bar_header() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    on_page(&mut s);

    // Registry side: the whole lane, in Bindings.xml order right after the shapeshift block,
    // sharing the action bar's category token.
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
    // …and it is a runOnUp pair like the action bar's own rows — which decides that a press
    // delivers a release half, and NOT whether the row may wear a wheel chord (1295). It takes
    // the wheel like any other command; the notch then runs both halves of the pair.
    assert_eq!(
        s.eval::<Option<u32>>(r#"return SetBinding("MOUSEWHEELUP", "BONUSACTIONBUTTON1")"#)
            .unwrap(),
        Some(1),
        "a press+release command takes the wheel — the refusal was ours, B265"
    );

    // Page side: search finds it and paints a live row with the capsule filled.
    // The search tags are the DISPLAY names uppercased (era AddSearchTags), so the query is the
    // label the app's strings give the row, not its token.
    let query = label(&s, "BINDING_NAME_BONUSACTIONBUTTON1", "BONUSACTIONBUTTON1").to_lowercase();
    s.run(&format!(
        r#"BenillaOptionsFrameSearchBox:SetText("{query}")"#
    ))
    .unwrap();
    s.tick(0.0); // the deferred OnTextChanged drains here (decision 1831)
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

/// **Every registry body is real Lua.** 167 rows carry a Lua chunk as a Rust string literal, and
/// nothing compiled them: a typo in one shipped as a command that errors on its first press, in a
/// file whose tests all live one layer up at the page. `loadstring` is the whole check — running
/// them needs the FrameXML the harness deliberately does not load — and it is exactly the check
/// that a hand-typed string wants.
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
