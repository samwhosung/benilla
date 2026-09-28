//! The stock ESC menu's rungs under the addons that reshape it (ShaguTweaks, MCP, pfUI), with the
//! layer on and without it: every stock rung stays in the chain, anchored `TOP`, so an addon's
//! insert lays out as on 1.12.1. Without the layer, the menu is the stock one.

use std::path::Path;

use benilla_ui::script::UiScript;

use crate::local_state::test_env::ENV_LOCK;

/// The production in-game load with the ESC menu open: the layer on unless `stock_ui`, and the
/// AddOns root filled by `addons`. Fails on any load error or raise.
fn open_menu_with(tag: &str, stock_ui: bool, addons: impl FnOnce(&Path)) -> UiScript {
    let (mut s, failures) = super::layer_tests::production_load_with(tag, stock_ui, "", addons);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    s.run("ShowUIPanel(GameMenuFrame)").unwrap();
    s.resolve();
    assert!(s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// An addon of `files`, each a name and its text, in the AddOns root.
fn write_addon(root: &Path, name: &str, files: &[(&str, &str)]) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
}

/// The corpus addon `name`, copied whole into the AddOns root.
fn copy_addon(corpus: &Path, root: &Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(corpus.join(name)).unwrap().flatten() {
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
        }
    }
}

/// A shown rung's `(top, bottom)` down from the frame's top, and whether it is centred in it.
fn rung(s: &UiScript, name: &str) -> (f64, f64, bool) {
    let (top, bottom, left, right, ftop, fleft, fright) = s
        .eval::<(f64, f64, f64, f64, f64, f64, f64)>(&format!(
            "local f = GameMenuFrame return {name}:GetTop(), {name}:GetBottom(), \
             {name}:GetLeft(), {name}:GetRight(), f:GetTop(), f:GetLeft(), f:GetRight()"
        ))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let centred = ((left - fleft) - (fright - right)).abs() < 1e-3;
    (ftop - top, ftop - bottom, centred)
}

/// The shown rungs of the open menu, top to bottom: the children of `GameMenuFrame` that draw.
fn shown_rungs(s: &UiScript) -> Vec<String> {
    let mut names: Vec<String> = s
        .eval(
            "local out = {} for _, c in ipairs({ GameMenuFrame:GetChildren() }) do \
             if c:IsVisible() then table.insert(out, c:GetName()) end end return out",
        )
        .unwrap();
    names.sort_by(|a, b| rung(s, a).0.total_cmp(&rung(s, b).0));
    names
}

/// No two shown rungs overlap, each is centred, and each is 21 tall.
fn assert_a_clean_ladder(s: &UiScript) -> Vec<String> {
    let names = shown_rungs(s);
    for pair in names.windows(2) {
        let (above, below) = (rung(s, &pair[0]), rung(s, &pair[1]));
        assert!(
            below.0 >= above.1 - 1e-3,
            "{} (down {}..{}) overlaps {} (down {}..{})",
            pair[1],
            below.0,
            below.1,
            pair[0],
            above.0,
            above.1
        );
    }
    for name in &names {
        let (top, bottom, centred) = rung(s, name);
        assert!(
            (bottom - top - 21.0).abs() < 1e-3,
            "{name} is {} tall",
            bottom - top
        );
        assert!(centred, "{name} is off centre in the menu");
    }
    names
}

/// ShaguTweaks' insert (`config.lua:313-324`), restated in its own calls: narrow and deepen the
/// menu, hang a new rung under Interface Options, then hang Key Bindings under the new rung.
const SHAGU_ADVANCED_OPTIONS: &str = r#"
local panel = CreateFrame("Frame", nil, UIParent)
panel:Hide()
GameMenuFrame:SetWidth(GameMenuFrame:GetWidth() - 10)
GameMenuFrame:SetHeight(GameMenuFrame:GetHeight() + 10)
local rung = CreateFrame("Button", "GameMenuButtonAdvancedOptions", GameMenuFrame, "GameMenuButtonTemplate")
rung:SetPoint("TOP", GameMenuButtonUIOptions, "BOTTOM", 0, -1)
rung:SetText("Advanced Options")
rung:SetScript("OnClick", function() HideUIPanel(GameMenuFrame); panel:Show() end)
GameMenuButtonKeybindings:ClearAllPoints()
GameMenuButtonKeybindings:SetPoint("TOP", rung, "BOTTOM", 0, -1)
"#;

fn shagu(root: &Path) {
    write_addon(
        root,
        "ZZShagu",
        &[
            ("ZZShagu.toc", "## Interface: 11200\nconfig.lua\n"),
            ("config.lua", SHAGU_ADVANCED_OPTIONS),
        ],
    );
}

/// ShaguTweaks anchors its Advanced Options under `GameMenuButtonUIOptions` and re-anchors
/// `GameMenuButtonKeybindings` under it (`GameMenuFrame.xml:78`, `:93`): it loads without a raise,
/// the button lands 1 under the Options rung, and every rung below moves down by its height and
/// the gap, 22, as the stock rungs do on the stock menu.
#[test]
fn shagutweaks_advanced_options_lands_under_the_options_rung_and_pushes_the_rest_down() {
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for stock_ui in [false, true] {
        let tag = if stock_ui { "shagu-stock" } else { "shagu" };
        let base = open_menu_with(&format!("{tag}-base"), stock_ui, |_| {});
        let s = open_menu_with(tag, stock_ui, shagu);
        let before = shown_rungs(&base);
        let after = assert_a_clean_ladder(&s);

        let options = rung(&s, "GameMenuButtonOptions");
        let advanced = rung(&s, "GameMenuButtonAdvancedOptions");
        // Under the Options rung on ours, as the era menu has no Sound or UI Options rung.
        let above = if stock_ui {
            "GameMenuButtonUIOptions"
        } else {
            "GameMenuButtonOptions"
        };
        assert!(
            (advanced.0 - (rung(&s, above).1 + 1.0)).abs() < 1e-3,
            "stock_ui {stock_ui}: Advanced Options at {advanced:?}, 1 under {above}"
        );
        let at = after
            .iter()
            .position(|n| n == "GameMenuButtonAdvancedOptions")
            .expect("the button is shown");
        assert_eq!(after[at - 1], above, "stock_ui {stock_ui}: {after:?}");
        let mut expected = before.clone();
        expected.insert(at, "GameMenuButtonAdvancedOptions".into());
        assert_eq!(after, expected, "stock_ui {stock_ui}: the ladder");
        for name in &after[at + 1..] {
            let moved = (rung(&s, name).0 - options.0)
                - (rung(&base, name).0 - rung(&base, "GameMenuButtonOptions").0);
            assert!(
                (moved - 22.0).abs() < 1e-3,
                "stock_ui {stock_ui}: {name} moved {moved} down, not 22"
            );
        }
    }
}
/// MCP's AddOns button (`MCP.xml:108-127`) anchors `TOP` under Macros, sets Logout's `TOP` under
/// itself without `ClearAllPoints` and grows the menu by 25: Logout's one `TOP` anchor is replaced,
/// so Logout, Exit Game and Return to Game move down under it, as on the stock menu.
#[test]
fn mcps_addons_button_lands_under_macros_and_pushes_logout_down() {
    let _data = benilla_formats::wow_data_or_skip!();
    let corpus = benilla_formats::addon_corpus_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for stock_ui in [false, true] {
        let tag = if stock_ui { "mcp-stock" } else { "mcp" };
        let base = open_menu_with(&format!("{tag}-base"), stock_ui, |_| {});
        let s = open_menu_with(tag, stock_ui, |root| copy_addon(&corpus, root, "MCP"));
        let after = assert_a_clean_ladder(&s);

        let macros = rung(&s, "GameMenuButtonMacros");
        let addons = rung(&s, "GameMenuButtonAddOns");
        let logout = rung(&s, "GameMenuButtonLogout");
        assert!(
            (addons.0 - (macros.1 + 1.0)).abs() < 1e-3,
            "stock_ui {stock_ui}: AddOns {addons:?} 1 under Macros {macros:?}"
        );
        assert!(
            (logout.0 - (addons.1 + 1.0)).abs() < 1e-3,
            "stock_ui {stock_ui}: Logout {logout:?} 1 under AddOns {addons:?}"
        );
        let at = after
            .iter()
            .position(|n| n == "GameMenuButtonAddOns")
            .unwrap();
        assert_eq!(after[at - 1], "GameMenuButtonMacros");
        assert_eq!(
            after[at + 1..],
            [
                "GameMenuButtonLogout",
                "GameMenuButtonQuit",
                "GameMenuButtonContinue"
            ]
        );
        let height = |s: &UiScript| s.eval::<f64>("return GameMenuFrame:GetHeight()").unwrap();
        assert!(
            (height(&s) - height(&base) - 25.0).abs() < 1e-3,
            "stock_ui {stock_ui}: the menu grows by MCP's 25"
        );
        let span = |s: &UiScript, a: &str, b: &str| rung(s, b).0 - rung(s, a).0;
        for (a, b) in [
            ("GameMenuButtonLogout", "GameMenuButtonQuit"),
            ("GameMenuButtonQuit", "GameMenuButtonContinue"),
        ] {
            assert!(
                (span(&s, a, b) - span(&base, a, b)).abs() < 1e-3,
                "stock_ui {stock_ui}: {b} still follows {a}"
            );
        }
    }
}

/// pfUI's game-menu skin (`skins/blizzard/game_menu.lua`) in its shape: it narrows the menu by 30,
/// finds the unnamed MAIN_MENU title among the regions, adds a button at the top, shifts
/// `GameMenuButtonOptions`' own point down 22 through `GetPoint`, and skins every stock rung that
/// exists. The rungs stay centred in the narrower frame, as the stock ones do, with no raise.
const PFUI_SHAPED_SKIN: &str = r#"
GameMenuFrame:SetWidth(GameMenuFrame:GetWidth() - 30)
GameMenuFrame:SetHeight(GameMenuFrame:GetHeight() + 6)

local title
for _, region in ipairs({ GameMenuFrame:GetRegions() }) do
  if region:GetObjectType() == "FontString" and region:GetDrawLayer() == "ARTWORK"
      and region:GetText() and string.find(region:GetText(), MAIN_MENU, 1) then
    title = region
  end
end
title:SetTextColor(1, 1, 1, 1)
title:ClearAllPoints()
title:SetPoint("TOP", GameMenuFrame, "TOP", 0, 16)

local button = CreateFrame("Button", "GameMenuButtonPFUI", GameMenuFrame, "GameMenuButtonTemplate")
button:SetPoint("TOP", 0, -10)
button:SetText("pfUI Config")

local point, relativeTo, relativePoint, xOffset, yOffset = GameMenuButtonOptions:GetPoint()
GameMenuButtonOptions:SetPoint(point, relativeTo, relativePoint, xOffset, yOffset - 22)

ZZPFUI_SKINNED = 0
for _, rung in pairs({ GameMenuButtonOptions, GameMenuButtonSoundOptions, GameMenuButtonUIOptions,
    GameMenuButtonKeybindings, GameMenuButtonMacros, GameMenuButtonRatings, GameMenuButtonLogout,
    GameMenuButtonQuit, GameMenuButtonContinue }) do
  if rung then ZZPFUI_SKINNED = ZZPFUI_SKINNED + 1 end
end
"#;

#[test]
fn a_pfui_shaped_skin_keeps_every_rung_centred_in_the_narrowed_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for stock_ui in [false, true] {
        let tag = if stock_ui { "pfui-stock" } else { "pfui" };
        let s = open_menu_with(tag, stock_ui, |root| {
            write_addon(
                root,
                "ZZpfUI",
                &[
                    ("ZZpfUI.toc", "## Interface: 11200\nskin.lua\n"),
                    ("skin.lua", PFUI_SHAPED_SKIN),
                ],
            );
        });
        let after = assert_a_clean_ladder(&s);
        assert_eq!(
            after[0], "GameMenuButtonPFUI",
            "stock_ui {stock_ui}: {after:?}"
        );
        assert_eq!(
            after[1], "GameMenuButtonOptions",
            "stock_ui {stock_ui}: {after:?}"
        );
        assert_eq!(
            s.eval::<i64>("return ZZPFUI_SKINNED").unwrap(),
            8,
            "stock_ui {stock_ui}: every stock rung is there to skin"
        );
    }
}

/// Booted without the layer, as `WOW_STOCK_UI=1` does, the ESC menu is the stock one: its eight
/// rungs at the stock file's geometry (`GameMenuFrame.xml:5-185`), each opening its stock window.
#[test]
fn the_stock_ui_menu_is_the_stock_menu_and_every_rung_opens_its_stock_window() {
    use benilla_ui::script::SessionRequest;
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = open_menu_with("stock-menu", true, |_| {});
    let rungs = assert_a_clean_ladder(&s);
    assert_eq!(
        rungs,
        [
            "GameMenuButtonOptions",
            "GameMenuButtonSoundOptions",
            "GameMenuButtonUIOptions",
            "GameMenuButtonKeybindings",
            "GameMenuButtonMacros",
            "GameMenuButtonLogout",
            "GameMenuButtonQuit",
            "GameMenuButtonContinue",
        ]
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return GameMenuFrame:GetWidth(), GameMenuFrame:GetHeight()")
            .unwrap(),
        (195.0, 246.0)
    );
    // Options' centre 37 under the top, then 1-unit gaps, and 16 above Return to Game.
    let tops: Vec<f64> = rungs.iter().map(|n| rung(&s, n).0).collect();
    let want = [26.5, 48.5, 70.5, 92.5, 114.5, 136.5, 158.5, 195.5];
    for ((name, got), want) in rungs.iter().zip(&tops).zip(want) {
        assert!((got - want).abs() < 1e-3, "{name} at {got}, not {want}");
    }
    let labels: Vec<String> = s
        .eval(
            "local out = {} for _, n in ipairs({ 'Options', 'SoundOptions', 'UIOptions', \
             'Keybindings', 'Macros', 'Logout', 'Quit', 'Continue' }) do \
             table.insert(out, getglobal('GameMenuButton' .. n):GetText()) end return out",
        )
        .unwrap();
    let globals: Vec<String> = s
        .eval(
            "return { VIDEOOPTIONS_MENU, SOUNDOPTIONS_MENU, UIOPTIONS_MENU, KEY_BINDINGS, MACROS, \
             LOGOUT, EXIT_GAME, RETURN_TO_GAME }",
        )
        .unwrap();
    assert_eq!(labels, globals);
    assert!(!s
        .eval::<bool>("return BenillaGameMenuButtonEditMode ~= nil")
        .unwrap());

    for (rung, window) in [
        ("Options", "OptionsFrame"),
        ("SoundOptions", "SoundOptionsFrame"),
        ("UIOptions", "UIOptionsFrame"),
        ("Keybindings", "KeyBindingFrame"),
        ("Macros", "MacroFrame"),
    ] {
        s.run("HideUIPanel(GameMenuFrame) ShowUIPanel(GameMenuFrame)")
            .unwrap();
        s.run(&format!("GameMenuButton{rung}:Click()")).unwrap();
        assert!(
            s.eval::<bool>(&format!(
                "return {window} ~= nil and {window}:IsVisible() and true or false"
            ))
            .unwrap(),
            "{rung} opens {window}"
        );
        s.run(&format!("HideUIPanel({window})")).unwrap();
        // The stock video window's own show and hide read video CVars and verbs benilla does not
        // register yet (`OptionsFrame.lua:146`, `:260`); what the menu does is the click above.
        let errors = s.take_errors();
        assert!(
            errors
                .iter()
                .all(|e| rung == "Options" && e.contains("OptionsFrame.lua")),
            "{rung}: {errors:?}"
        );
    }
    for (rung, request) in [
        ("Logout", SessionRequest::Logout),
        ("Quit", SessionRequest::Quit),
    ] {
        s.run("ShowUIPanel(GameMenuFrame)").unwrap();
        let _ = s.take_session_requests();
        s.run(&format!("GameMenuButton{rung}:Click()")).unwrap();
        assert_eq!(s.take_session_requests(), vec![request], "{rung}");
    }
    s.run("ShowUIPanel(GameMenuFrame) GameMenuButtonContinue:Click()")
        .unwrap();
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
