//! Benilla's options window (`OptionsFrame.xml`): the shell, the search and scroll, and every
//! page's rows against the real CVar set, the saved option globals and the API rows.

use benilla_ui::script::{QuadContent, SoundRequest, UiScript, WornDisplay};

/// The options window and the game menu over the manifest slice they need.
fn harness() -> UiScript {
    harness_on(UiScript::new().unwrap())
}

/// Load the manifest slice onto `s`, so a page test can seed CVars before the XML loads, as the
/// app does. GameTooltip.xml precedes UIDropDownMenu.xml for the kit's `TOOLTIP_DEFAULT_COLOR`.
fn harness_on(mut s: UiScript) -> UiScript {
    s.set_screen_size(1024.0, 768.0);
    // Each file loads once: a page harness may have loaded it already, and a second load of the
    // money kit's frames breaks `MoneyFrame_Update`, which writes through `getglobal(name)`.
    let loaded = |s: &UiScript, file: &str| -> bool {
        s.eval::<bool>(&format!(
            "return (BENILLA_TEST_LOADED or {{}})[{file:?}] == true"
        ))
        .unwrap_or(false)
    };
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        // Before every window that names `parent="UIParent"`, which resolves at load.
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        // The stock options kit, hidden: `UIOptionsFrame_Init` assigns the option globals our
        // rows capture at OnLoad, and these declare the tables the Graphics rows and census read.
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\OptionsFrame.lua",
        r"Interface\FrameXML\UIOptionsFrame.xml",
        "ScrollTemplates.xml", // the page scroll and the Keybindings list
        "KeyBindingsPage.xml", // the Keybindings page's templates and script
        "OptionsFrame.xml",
        "GameMenuFrame.xml",
    ] {
        // Our window loads strict: a missing template there fails instead of only warning.
        if loaded(&s, file) {
            continue;
        }
        if file == "OptionsFrame.xml" {
            super::test_ui::load_ui_strict(&s, file);
        } else {
            super::test_ui::load_ui(&s, file);
        }
    }
    // The app's post-load pass: `BuffFrame_OnLoad` never pitches the buff rows, and the stock arm
    // that does waits for `VARIABLES_LOADED` (`UIOptionsFrame.lua:206`).
    super::manifest::apply_buff_durations(&s).unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// On the reference's igMainMenuOption kit; the window takes the center slot the menu leaves.
#[test]
fn the_menu_options_button_swaps_the_menu_for_the_options_window() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ShowUIPanel(GameMenuFrame)").unwrap();
    let _ = s.take_sounds();

    s.run("GameMenuButtonOptions:Click()").unwrap();
    assert!(
        s.eval::<bool>("return BenillaOptionsFrame:IsVisible()")
            .unwrap(),
        "the options window opened"
    );
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "…and the menu went away first"
    );
    assert!(
        s.eval::<bool>("return GetCenterFrame():GetName() == \"BenillaOptionsFrame\"")
            .unwrap(),
        "the window holds the native-center slot the menu vacated"
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOption".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The title is the selected row's label, which differs from its key only for ActionBars.
#[test]
fn controls_is_the_default_category_and_the_title_reads_it() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrame.selectedCategory")
            .unwrap(),
        "Controls"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Controls"
    );
    // The row labels are the era category tree's.
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameCategoryListRowActionBars:GetText()")
            .unwrap(),
        "Action Bars"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The selection survives a close and reopen: the OnShow re-applies the last category.
#[test]
fn clicking_a_row_moves_the_selection_and_the_page_title() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrame.selectedCategory")
            .unwrap(),
        "Graphics"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Graphics"
    );

    s.run("HideUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Graphics"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_close_button_hides_the_window() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    let _ = s.take_sounds();
    s.run("BenillaOptionsFrameCloseButton:Click()").unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrame:IsVisible()")
        .unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuClose".into())));

    // No corner X, unlike the era window: the red button and ESC close it.
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameClosePanelButton == nil")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.14's OptionsListButtonTemplate wash: one UI-QuestLogTitleHighlight quad in ADD blend at the
/// era's 187x21, gold (1,1,0) locked on the selected row, steel blue (.196,.388,.8) on hover.
#[test]
fn the_selected_row_wears_the_gold_wash_and_hover_runs_blue() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // Scale 1, so rects and the pointer share coordinates; at 1024x768 the fit clamp stays above 1.
    s.run("ERA_WINDOW_SCALE = 1").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    assert_eq!(
        s.eval::<f64>("return BenillaOptionsFrameCategoryListRowControlsBg:GetWidth()")
            .unwrap(),
        187.0
    );
    assert_eq!(
        s.eval::<f64>("return BenillaOptionsFrameCategoryListRowControlsBg:GetHeight()")
            .unwrap(),
        21.0
    );

    let washes = |s: &mut UiScript| -> Vec<(benilla_ui::layout::Rect, [f32; 4], bool)> {
        s.resolve();
        s.extract()
            .into_iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color,
                    additive,
                    ..
                } if p.contains("UI-QuestLogTitleHighlight") => {
                    q.rect.map(|r| (r, color.unwrap_or([1.0; 4]), *additive))
                }
                _ => None,
            })
            .collect()
    };
    let gold =
        |c: &[f32; 4]| (c[0] - 1.0).abs() < 1e-3 && (c[1] - 1.0).abs() < 1e-3 && c[2].abs() < 1e-3;
    let blue = |c: &[f32; 4]| {
        (c[0] - 0.196).abs() < 1e-3 && (c[1] - 0.388).abs() < 1e-3 && (c[2] - 0.8).abs() < 1e-3
    };

    let before = washes(&mut s);
    assert_eq!(
        before.len(),
        1,
        "exactly one wash shows: the locked selection"
    );
    assert!(before[0].2, "the wash draws ADD — 1.14's alphaMode");
    assert!(gold(&before[0].1), "…in the locked gold: {:?}", before[0].1);

    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    let after = washes(&mut s);
    assert_eq!(after.len(), 1, "the wash moved, not multiplied");
    assert!(gold(&after[0].1), "still the gold tint: {:?}", after[0].1);
    assert!(
        after[0].0.top < before[0].0.top,
        "Audio sits below Controls, so the wash reseats lower (top {} -> {})",
        before[0].0.top,
        after[0].0.top
    );

    let (cx, cy) = {
        let l: f32 = s
            .eval("return BenillaOptionsFrameCategoryListRowControls:GetLeft()")
            .unwrap();
        let r: f32 = s
            .eval("return BenillaOptionsFrameCategoryListRowControls:GetRight()")
            .unwrap();
        let t: f32 = s
            .eval("return BenillaOptionsFrameCategoryListRowControls:GetTop()")
            .unwrap();
        let b: f32 = s
            .eval("return BenillaOptionsFrameCategoryListRowControls:GetBottom()")
            .unwrap();
        ((l + r) * 0.5, (t + b) * 0.5)
    };
    s.mouse_move(cx, cy);
    let hovered = washes(&mut s);
    assert_eq!(
        hovered.len(),
        2,
        "hover adds its wash beside the locked gold"
    );
    assert_eq!(hovered.iter().filter(|w| gold(&w.1)).count(), 1);
    assert_eq!(
        hovered.iter().filter(|w| blue(&w.1)).count(),
        1,
        "the hover tint is 1.14's steel-blue: {:?}",
        hovered.iter().map(|w| w.1).collect::<Vec<_>>()
    );

    s.mouse_move(5.0, 5.0);
    let left = washes(&mut s);
    assert_eq!(left.len(), 1);
    assert!(gold(&left[0].1));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The black fill draws over the tiled ground and clear of the rope border, whose edge slices in
/// `UI-DialogBox-Border` are dead from texel 16 of 32 and whose corners' ink ends by 14: an inset
/// of 14 clears it, where the 11/12 bg insets would not.
#[test]
fn the_ground_dim_draws_over_the_tile_and_clear_of_the_rope() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ERA_WINDOW_SCALE = 1").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.resolve();

    let edge = |frame: &str, side: &str| -> f32 {
        s.eval(&format!("return {frame}:Get{side}()")).unwrap()
    };
    let (fl, fr, ft, fb) = (
        edge("BenillaOptionsFrame", "Left"),
        edge("BenillaOptionsFrame", "Right"),
        edge("BenillaOptionsFrame", "Top"),
        edge("BenillaOptionsFrame", "Bottom"),
    );
    let (dl, dr, dt, db) = (
        edge("BenillaOptionsFrameGroundDim", "Left"),
        edge("BenillaOptionsFrameGroundDim", "Right"),
        edge("BenillaOptionsFrameGroundDim", "Top"),
        edge("BenillaOptionsFrameGroundDim", "Bottom"),
    );
    for (name, inset) in [
        ("left", dl - fl),
        ("right", fr - dr),
        ("top", ft - dt),
        ("bottom", db - fb),
    ] {
        assert!(
            inset >= 14.0,
            "the dim must clear the rope's ink on the {name} — inset {inset}"
        );
    }

    // `extract` is in ascending z: a later index draws later.
    let quads = s.extract();
    let ground = quads
        .iter()
        .position(|q| match &q.content {
            QuadContent::Backdrop { path, .. } => path.contains("UI-DialogBox-Background"),
            _ => false,
        })
        .expect("the window still wears the 1.14 dialog ground");
    let dim = quads
        .iter()
        .position(|q| {
            let Some(r) = q.rect else { return false };
            matches!(
                &q.content,
                QuadContent::Texture { path: None, color: Some(c), .. }
                    if c[0] == 0.0 && c[1] == 0.0 && c[2] == 0.0 && c[3] > 0.0 && c[3] < 1.0
            ) && (r.left - dl).abs() < 0.5
                && (r.top - dt).abs() < 0.5
        })
        .expect("the black ground fill is drawn");
    assert!(
        dim > ground,
        "the dim draws over the tile (ground #{ground}, dim #{dim})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The era SettingsPanel search: the live rows under a category head, a matched child pulling its
/// parent in above it; clearing the box lays every row back on its authored XML chain.
#[test]
fn search_reflows_live_rows_and_restores_the_page() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    s.resolve();
    let authored_top: f32 = s
        .eval("return BenillaOptionsFrameContainerBodyAudioRowMaster:GetTop()")
        .unwrap();

    s.run("BenillaOptionsFrameSearchBox:SetText(\"music\")")
        .unwrap();
    // The reflow runs from the box's `OnTextChanged`, which the engine defers to the drain.
    s.tick(0.0);
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Search Results"
    );
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsVisible()")
            .unwrap(),
        "Defaults hides while the search holds the page (the era SetShown(not hasText))"
    );
    for (frame, shown) in [
        ("BenillaOptionsFrameContainerBodySearchHeadAudio", true),
        ("BenillaOptionsFrameContainerBodySearchHeadControls", false),
        ("BenillaOptionsFrameContainerBodySearchHeadGraphics", false),
        ("BenillaOptionsFrameContainerBodyAudioRowMusic", true),
        ("BenillaOptionsFrameContainerBodyAudioRowEnableMusic", true),
        ("BenillaOptionsFrameContainerBodyAudioRowMaster", true), // the parent, pulled in unmatched
        ("BenillaOptionsFrameContainerBodyAudioRowSound", false),
        ("BenillaOptionsFrameContainerBodyAudioRowEnableAll", false),
        ("BenillaOptionsFrameContainerBodyControlsRowAutoLoot", false),
        ("BenillaOptionsFrameContainerBodyGraphicsRowUiScale", false),
        ("BenillaOptionsFrameContainerBodyNoResults", false),
    ] {
        assert_eq!(
            s.eval::<bool>(&format!("return {frame}:IsVisible()"))
                .unwrap(),
            shown,
            "{frame} shown={shown}"
        );
    }
    s.resolve();
    let tops: Vec<f32> = [
        "BenillaOptionsFrameContainerBodySearchHeadAudio",
        "BenillaOptionsFrameContainerBodyAudioRowMaster",
        "BenillaOptionsFrameContainerBodyAudioRowMusic",
        "BenillaOptionsFrameContainerBodyAudioRowEnableMusic",
    ]
    .iter()
    .map(|f| s.eval::<f32>(&format!("return {f}:GetTop()")).unwrap())
    .collect();
    assert!(
        tops[0] > tops[1] && tops[1] > tops[2] && tops[2] > tops[3],
        "head, parent, then the matches chain downward: {tops:?}"
    );
    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyAudioRowMusicControlSlider:SetValue(0.25)")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MusicVolume".to_string(), "0.25".to_string())]
    );

    s.run("BenillaOptionsFrameSearchBox:SetText(\"\")").unwrap();
    s.tick(0.0); // the clear reflows on the drain too
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Audio"
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsVisible()")
        .unwrap());
    s.resolve();
    let restored_top: f32 = s
        .eval("return BenillaOptionsFrameContainerBodyAudioRowMaster:GetTop()")
        .unwrap();
    assert!(
        (restored_top - authored_top).abs() < 0.01,
        "the restore law equals the authored XML chain ({restored_top} vs {authored_top})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The era scoring (`MatchesSearchTags`, Blizzard_Settings.lua): the whole query is the first
/// word tried, so a phrase hit outscores its own words and leads its group.
#[test]
fn the_phrase_match_outranks_its_words() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameSearchBox:SetText(\"master volume\")")
        .unwrap();
    s.resolve();
    let tops: Vec<f32> = [
        "BenillaOptionsFrameContainerBodyAudioRowMaster", // phrase hit, score 12
        "BenillaOptionsFrameContainerBodyAudioRowSound",  // "VOLUME" hits, score 5, page order
        "BenillaOptionsFrameContainerBodyAudioRowMusic",
        "BenillaOptionsFrameContainerBodyAudioRowAmbience",
    ]
    .iter()
    .map(|f| s.eval::<f32>(&format!("return {f}:GetTop()")).unwrap())
    .collect();
    assert!(
        tops[0] > tops[1] && tops[1] > tops[2] && tops[2] > tops[3],
        "phrase first, then the word matches in page order: {tops:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A head click selects its category and clears the box, as the era's `SelectCategory` does.
#[test]
fn a_head_click_ends_the_search_and_a_miss_shows_no_results() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    s.run("BenillaOptionsFrameSearchBox:SetText(\"volume\")")
        .unwrap();
    s.tick(0.0); // the reflow builds the head clicked below
    s.run("BenillaOptionsFrameContainerBodySearchHeadAudio:Click()")
        .unwrap();
    // The click clears the box at once; the restore rides that clear's deferred `OnTextChanged`.
    s.tick(0.0);
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameSearchBox:GetText()")
            .unwrap(),
        "",
        "the head click cleared the search (the era SelectCategory law)"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrame.selectedCategory")
            .unwrap(),
        "Audio"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Audio"
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyAudio:IsVisible()")
        .unwrap());

    s.run("BenillaOptionsFrameSearchBox:SetText(\"flibbertigibbet\")")
        .unwrap();
    s.tick(0.0); // the miss renders on the drain too
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyNoResults:IsVisible()")
        .unwrap());
    for head in ["Controls", "Audio", "Graphics"] {
        assert!(
            !s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodySearchHead{head}:IsVisible()"
            ))
            .unwrap(),
            "no head on a miss"
        );
    }
    s.run("BenillaOptionsFrameSearchBox:SetText(\"\")").unwrap();
    s.tick(0.0); // and so does clearing it
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyNoResults:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A left press on the track, off the thumb, seats the thumb under the cursor and drags on.
#[test]
fn a_track_press_seats_the_thumb_and_keeps_dragging() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.set_cvar_host("MasterVolume", "0.1");
    s.run("ERA_WINDOW_SCALE = 1").unwrap(); // pointer and rects share coordinates
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();

    for btn in ["Back", "Forward"] {
        assert!(s
            .eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyAudioRowMasterControl{btn} == nil"
            ))
            .unwrap());
    }

    let slider = "BenillaOptionsFrameContainerBodyAudioRowMasterControlSlider";
    s.resolve(); // seat the rects before reading them
    let (l, r, t, b) = (
        s.eval::<f32>(&format!("return {slider}:GetLeft()"))
            .unwrap(),
        s.eval::<f32>(&format!("return {slider}:GetRight()"))
            .unwrap(),
        s.eval::<f32>(&format!("return {slider}:GetTop()")).unwrap(),
        s.eval::<f32>(&format!("return {slider}:GetBottom()"))
            .unwrap(),
    );
    let cy = (t + b) * 0.5;
    // 1 unit inside the right end, within the thumb's half-width end zone, where the centered
    // seat clamps the fraction to 1.0.
    s.mouse_button(r - 1.0, cy, "LeftButton", true);
    assert!(
        s.eval::<bool>(&format!(
            "return math.abs({slider}:GetValue() - 1.0) < 0.0001"
        ))
        .unwrap(),
        "the press itself seats the thumb"
    );
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MasterVolume".to_string(), "1".to_string())]
    );
    // Still held. The midpoint is exactly fraction 0.5, on the 0.05 grid, so nothing snaps.
    s.mouse_move((l + r) * 0.5, cy);
    assert!(
        s.eval::<bool>(&format!(
            "return math.abs({slider}:GetValue() - 0.5) < 0.0001"
        ))
        .unwrap(),
        "the capture began at the press — the drag follows without re-grabbing"
    );
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MasterVolume".to_string(), "0.5".to_string())]
    );
    s.mouse_button((l + r) * 0.5, cy, "LeftButton", false);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The readout shares its label's line: the +3 that seats the groove and thumb in the row is on
/// the art frames, not on the control the readout hangs off.
#[test]
fn a_slider_rows_readout_sits_on_its_labels_line() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    for (page, rows) in [
        ("Controls", &["RowMouseSpeed", "RowMaxCameraDistance"][..]),
        (
            "Audio",
            &["RowMaster", "RowSound", "RowMusic", "RowAmbience"][..],
        ),
        ("Graphics", &["RowUiScale", "RowFarclip"][..]),
    ] {
        s.run(&format!("BenillaOptionsFrameCategoryListRow{page}:Click()"))
            .unwrap();
        s.resolve(); // seat the rects before reading them
        let mid = |frame: &str| -> f32 {
            let top: f32 = s.eval(&format!("return {frame}:GetTop()")).unwrap();
            let bottom: f32 = s.eval(&format!("return {frame}:GetBottom()")).unwrap();
            (top + bottom) * 0.5
        };
        for row in rows {
            let base = format!("BenillaOptionsFrameContainerBody{page}{row}");
            // Both are regions, so their rects share a space and need no scale.
            let (label, value) = (
                mid(&format!("{base}Label")),
                mid(&format!("{base}ControlValue")),
            );
            assert!(
                (label - value).abs() < 0.01,
                "{base}: the readout sits {:.2} off its label's line",
                value - label
            );
            // The groove still rides 3 units high of the row.
            assert!(
                (mid(&format!("{base}ControlGroove")) - mid(&base) - 3.0).abs() < 0.01,
                "{base}: the groove left its seat"
            );
        }
    }
}

/// A VM with the real registered CVar set, before any XML loads, as the app boots.
fn audio_harness() -> UiScript {
    let s = UiScript::new().unwrap();
    s.register_cvars(crate::cvars::registered_pairs());
    s
}

/// The Combat page's harness, with `Blizzard_CombatText` seated off the chain as LoadOnDemand, for
/// the master row's apply to load as the client does.
fn combat_harness() -> UiScript {
    let mut s = audio_harness();
    s.set_screen_size(1024.0, 768.0);
    load_definers(&s, &[r"Interface\FrameXML\UIParent.xml"]);
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatText");
    harness_on(s)
}

/// Load `files` ahead of the window, in order, each recorded so `harness_on` loads it only once.
fn load_definers(s: &UiScript, files: &[&str]) {
    for file in files {
        super::test_ui::load_ui(s, file);
        s.run(&format!(
            "BENILLA_TEST_LOADED = BENILLA_TEST_LOADED or {{}} BENILLA_TEST_LOADED[{file:?}] = true"
        ))
        .unwrap();
    }
}

/// The Interface page's harness, with its rows' consumers loaded ahead of the window: the unit
/// frames, the buff bar, the XP bar, the quest windows and the tutorial frame.
fn interface_harness() -> UiScript {
    let mut s = audio_harness();
    s.set_screen_size(1024.0, 768.0);
    load_definers(
        &s,
        &[
            "Interface\\FrameXML\\Fonts.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            "Interface\\FrameXML\\GlobalStrings.lua",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            "Interface\\FrameXML\\StaticPopup.xml",
            // The unit frames and the kits ahead of them: their dropdowns initialize at load, and
            // the dropdown kit reads `GameTooltip`'s `TOOLTIP_DEFAULT_COLOR`.
            "Interface\\FrameXML\\GameTooltip.xml",
            "Interface\\FrameXML\\UIDropDownMenu.xml",
            "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua calls at load
            "Interface\\FrameXML\\UnitPopup.xml",
            "Interface\\FrameXML\\TextStatusBar.lua",
            "Interface\\FrameXML\\TextStatusBar.xml",
            "Interface\\FrameXML\\BuffFrame.xml",
            "Interface\\FrameXML\\UnitFrame.xml",
            "Interface\\FrameXML\\CombatFeedback.xml",
            "Interface\\FrameXML\\PlayerFrame.xml",
            "Interface\\FrameXML\\PartyFrame.xml",
            "Interface\\FrameXML\\TargetFrame.xml",
            "Interface\\FrameXML\\PetFrame.xml",
            // For `OpacityFrameSlider`, which `PartyMemberBackground` sets on `VARIABLES_LOADED`.
            "Interface\\FrameXML\\ColorPickerFrame.xml",
            "Interface\\FrameXML\\Cooldown.xml",
            "Interface\\FrameXML\\ActionButtonTemplate.xml",
            "Interface\\FrameXML\\MainMenuBar.xml",
            "Interface\\FrameXML\\ActionBarFrame.xml",
            "Interface\\FrameXML\\BonusActionBarFrame.xml",
            // `ExhaustionTick_Update` reads `ReputationWatchBar`, which `ReputationFrame.xml`
            // declares, after the templates its check boxes inherit.
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            r"Interface\FrameXML\OptionsFrameTemplates.xml",
            r"Interface\FrameXML\ReputationFrame.xml",
            "ScrollTemplates.xml",
            "Interface\\FrameXML\\CharacterFrameTemplates.xml",
            "Interface\\FrameXML\\MerchantFrame.xml",
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\ItemButtonTemplate.xml",
            "Interface\\FrameXML\\QuestFrame.xml",
            r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
            "Interface\\FrameXML\\QuestLogFrame.xml",
            // The Show Tutorials setter, like the reference's Save arm, reaches
            // `TutorialFrameCheckButton` and `TutorialFrame_HideAllAlerts` unguarded.
            "Interface\\FrameXML\\TutorialFrame.xml",
        ],
    );
    harness_on(s)
}

/// The Chat page's harness: the chat frames ahead of the window, for `SetChatMouseOverDelay` and
/// the fade constants it moves (`FloatingChatFrame.lua:8-9`, `:1456`).
fn chat_harness() -> UiScript {
    let mut s = audio_harness();
    s.set_screen_size(1024.0, 768.0);
    load_definers(
        &s,
        &[
            "Interface\\FrameXML\\Fonts.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\StaticPopup.xml",
            "Interface\\FrameXML\\GameTooltip.xml",
            "Interface\\FrameXML\\UIDropDownMenu.xml",
            "Interface\\FrameXML\\UIMenu.xml", // the kit the chat and emote menus build on
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\ChatFrame.xml",
            "Interface\\FrameXML\\UIPanelTemplates.lua",
            "Interface\\FrameXML\\UIPanelTemplates.xml",
            "Interface\\FrameXML\\FloatingChatFrame.xml",
        ],
    );
    harness_on(s)
}

/// The Action Bars page's harness: the stock bars its rows move, and the options windows ahead of
/// `MultiActionBars.xml`, as in the manifest.
fn actionbars_harness() -> UiScript {
    let mut s = audio_harness();
    s.set_screen_size(1024.0, 768.0);
    load_definers(
        &s,
        &[
            "Interface\\FrameXML\\Fonts.xml",
            r"Interface\FrameXML\UIParent.xml",
            "Interface\\FrameXML\\Cooldown.xml",
            "Interface\\FrameXML\\ActionButtonTemplate.xml",
            "Interface\\FrameXML\\TextStatusBar.lua",
            "Interface\\FrameXML\\TextStatusBar.xml",
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\MainMenuBar.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            "Interface\\FrameXML\\GameTooltip.xml",
            "Interface\\FrameXML\\ActionBarFrame.xml",
            "Interface\\FrameXML\\BonusActionBarFrame.xml",
            // `ExhaustionTick_Update` reads `ReputationWatchBar`, which `ReputationFrame.xml`
            // declares, after the templates its check boxes inherit.
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            r"Interface\FrameXML\OptionsFrameTemplates.xml",
            r"Interface\FrameXML\ReputationFrame.xml",
            "Interface\\FrameXML\\ActionBarFrame.xml",
            "Interface\\FrameXML\\UIDropDownMenu.xml",
            "ScrollTemplates.xml",
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            "Interface\\FrameXML\\StaticPopup.xml",
            "KeyBindingsPage.xml",
            // `UIOptionsFrame_Init` assigns the globals these rows capture at OnLoad as their
            // default, and `MultiActionBars.xml` writes into `UIOptionsFrameCheckButtons` at load.
            r"Interface\FrameXML\OptionsFrame.lua",
            r"Interface\FrameXML\UIOptionsFrame.xml",
            "OptionsFrame.xml",
            "Interface\\FrameXML\\MultiActionBars.xml",
        ],
    );
    harness_on(s)
}

/// Sliders read the stored value with the era's rounded-percent readout; checkboxes read the flag.
#[test]
fn the_audio_page_reads_the_cvar_table_on_select() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("MusicVolume", "0.7");
    s.set_cvar_host("EnableMusic", "0");
    let s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();

    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyAudio:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return math.abs(BenillaOptionsFrameContainerBodyAudioRowMusicControlSlider:GetValue() - 0.7) < 0.0001"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyAudioRowMusicControlValue:GetText()"
        )
        .unwrap(),
        "70%"
    );
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowEnableMusicCheck:GetChecked()"
        )
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyAudioRowEnableAllCheck:GetChecked()")
        .unwrap());

    s.run("BenillaOptionsFrameCategoryListRowChat:Click()")
        .unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyAudio:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyChat:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A move snaps to the era's 5% grid (`obeyStepOnDrag`) and writes a short string.
#[test]
fn a_slider_move_snaps_and_writes_the_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );

    s.run("BenillaOptionsFrameContainerBodyAudioRowMasterControlSlider:SetValue(0.43)")
        .unwrap();
    assert!(s
        .eval::<bool>(
            "return math.abs(BenillaOptionsFrameContainerBodyAudioRowMasterControlSlider:GetValue() - 0.45) < 0.0001"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyAudioRowMasterControlValue:GetText()"
        )
        .unwrap(),
        "45%"
    );
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MasterVolume".to_string(), "0.45".to_string())]
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The sound panel's inverted click kit (`SoundOptionsFrame.lua:86-90`) and its dependency:
/// Enable All Sound off greys Enable Ambience but not Enable Music.
#[test]
fn the_checkbox_rows_write_flags_and_the_master_greys_ambience() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();
    let _ = s.take_sounds();

    s.run("BenillaOptionsFrameContainerBodyAudioRowEnableAllCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MasterSoundEffects".to_string(), "0".to_string())]
    );
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowEnableAmbienceCheck:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowEnableMusicCheck:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOn".into())));

    s.run("BenillaOptionsFrameContainerBodyAudioRowEnableAllCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MasterSoundEffects".to_string(), "1".to_string())]
    );
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowEnableAmbienceCheck:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12 has no such checkbox and always goes quiet in the background, so the row ships off; the
/// master's rule (`SoundOptionsFrame_UpdateDependencies`) names two other rows, not this one.
#[test]
fn the_background_sound_row_boots_off_and_writes_the_era_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();

    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowBackgroundSoundCheck:GetChecked()"
        )
        .unwrap(),
        "the reference goes quiet in the background, so the box ships unticked"
    );
    s.run("BenillaOptionsFrameContainerBodyAudioRowBackgroundSoundCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![(
            "Sound_EnableSoundWhenGameIsInBG".to_string(),
            "1".to_string()
        )]
    );

    s.run("BenillaOptionsFrameContainerBodyAudioRowEnableAllCheck:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowBackgroundSoundCheck:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Defaults is the era's per-page reset, to the registered defaults.
#[test]
fn defaults_resets_the_audio_page_to_registered_defaults() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("MusicVolume", "0.9");
    s.set_cvar_host("EnableMusic", "0");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    let changes = s.take_cvar_changes();
    assert!(
        changes.contains(&("MusicVolume".to_string(), "0.4".to_string())),
        "music back to its 1.12 registration default: {changes:?}"
    );
    assert!(
        changes.contains(&("EnableMusic".to_string(), "1".to_string())),
        "the flag back on: {changes:?}"
    );
    // Rows already at their default write nothing.
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowEnableMusicCheck:GetChecked()"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyAudioRowMusicControlValue:GetText()"
        )
        .unwrap(),
        "40%"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The two 1.12 sliders read the table: uiScale over 0.64..1.0 in percent, farclip over 177..777
/// in yards (`OptionsFrame.lua:25-26`).
#[test]
fn the_graphics_page_reads_the_cvar_table_on_select() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("uiScale", "0.8");
    s.set_cvar_host("farclip", "297");
    let s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();

    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyGraphics:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return math.abs(BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:GetValue() - 0.8) < 0.0001"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlValue:GetText()"
        )
        .unwrap(),
        "80%"
    );
    assert!(s
        .eval::<bool>(
            "return math.abs(BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlSlider:GetValue() - 297) < 0.001"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlValue:GetText()"
        )
        .unwrap(),
        "297"
    );
    // The labels are the 1.12 GlobalStrings' own.
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleLabel:GetText()"
        )
        .unwrap(),
        "UI Scale"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowFarclipLabel:GetText()"
        )
        .unwrap(),
        "Terrain Distance"
    );

    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyGraphics:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyAudio:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Modern Classic's two-entry Display Mode dropdown over 1.12's `gxWindow`, whose "1" is
/// windowed: the first entry, the borderless window, is "0".
#[test]
fn the_display_mode_dropdown_maps_its_entries_to_the_gx_window_polarity() {
    benilla_formats::wow_data_or_skip!();
    const ROW: &str = "BenillaOptionsFrameContainerBodyGraphicsRowDisplayMode";
    let mut s = audio_harness();
    s.set_cvar_host("gxWindow", "0");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}Label:GetText()"))
            .unwrap(),
        "Display Mode"
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}DropdownText:GetText()"))
            .unwrap(),
        "Windowed (Fullscreen)"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );

    // Two entries: there is no exclusive fullscreen, as in modern Classic.
    s.run(&format!("{ROW}DropdownButton:Click()")).unwrap();
    assert!(s.eval::<bool>("return DropDownList1:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<f64>("return DropDownList1.numButtons").unwrap(),
        2.0
    );
    assert!(s
        .eval::<bool>("return DropDownList1Button1Check:IsVisible()")
        .unwrap());

    s.run("DropDownList1Button2:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("gxWindow".to_string(), "1".to_string())]
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}DropdownText:GetText()"))
            .unwrap(),
        "Windowed"
    );
    assert!(!s.eval::<bool>("return DropDownList1:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12's Video Options checkbox 5 (`OptionsFrame.lua:9`). Not deferred despite its
/// `gxRestart = 1`: the present mode changes live.
#[test]
fn the_vertical_sync_row_reads_and_writes_the_present_mode_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("gxVSync", "0");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();

    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowVerticalSyncCheck:GetChecked()"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowVerticalSyncLabel:GetText()"
        )
        .unwrap(),
        "Vertical Sync"
    );
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameContainerBodyGraphicsRowVerticalSyncCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("gxVSync".to_string(), "1".to_string())],
        "the checkbox writes the cvar on click, not on Apply"
    );

    // Defaults walks it back to the registered "1".
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowVerticalSyncCheck:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowVerticalSyncCheck:GetChecked()"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Brightness is an API row over `GetGamma`/`SetGamma`, the reference's slider 6
/// (`OptionsFrame.lua:30`): a drag writes `gamma`, and a revisit reads it back.
#[test]
fn the_brightness_slider_writes_through_its_engine_pair_and_survives_a_reopen() {
    benilla_formats::wow_data_or_skip!();
    const ROW: &str = "BenillaOptionsFrameContainerBodyGraphicsRowBrightness";
    let s = audio_harness();
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}Label:GetText()"))
            .unwrap(),
        "Brightness"
    );
    // The registered `gamma` "1.0" is `GetGamma() == 0`, the centre of the slider's travel.
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "50%"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the pair on select must not write it back"
    );

    // `SetGamma` writes the reference's `1 - slider`, with six decimals.
    for (slider, cvar, readout) in [
        (0.5, "0.500000", "100%"),
        (-0.5, "1.500000", "0%"),
        (0.2, "0.800000", "70%"),
    ] {
        s.run(&format!("{ROW}ControlSlider:SetValue({slider})"))
            .unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![("gamma".to_string(), cvar.to_string())],
            "dragging to {slider} must reach the store"
        );
        assert_eq!(
            s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
                .unwrap(),
            readout
        );
    }

    // A revisit re-reads the pair, not the control; the row was left at 0.2.
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "70%",
        "the reopened page reads the store, not the registered default"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "…and the refresh itself writes nothing"
    );
}

/// Environment Detail over `WorldDetail`: the readout names the stop, and an out-of-range value
/// shows the nearest one without being written back.
#[test]
fn the_world_detail_slider_writes_the_cvar_and_the_readout_names_its_stop() {
    benilla_formats::wow_data_or_skip!();
    const ROW: &str = "BenillaOptionsFrameContainerBodyGraphicsRowWorldDetail";
    let mut s = audio_harness();
    s.set_cvar_host("WorldDetail", "0");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}Label:GetText()"))
            .unwrap(),
        "Environment Detail"
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "Low"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );

    // The reference's grid: three stops, one apart (`OptionsFrame.lua:27`).
    assert!(s
        .eval::<bool>(&format!(
            "local lo, hi = {ROW}ControlSlider:GetMinMaxValues() \
             return lo == 0 and hi == 2 and {ROW}ControlSlider:GetValueStep() == 1"
        ))
        .unwrap());

    s.run(&format!("{ROW}ControlSlider:SetValue(2)")).unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("WorldDetail".to_string(), "2".to_string())]
    );
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "High"
    );

    s.run(&format!("{ROW}ControlSlider:SetValue(0.6)")).unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "Medium"
    );

    // "-1" is what the clutter-density override seeds for clutter off.
    s.set_cvar_host("WorldDetail", "-1");
    s.take_cvar_changes();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click(); BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "Low"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "the nearest-stop display must not write back"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// On the real manifest and CVar table: `UIParent_OnEvent` calls `UpdateNameplates()` on
/// `VARIABLES_LOADED` and `PLAYER_ENTERING_WORLD` (`UIParent.lua:234`, `:367`), and here its verbs
/// write the CVar pair that is the store. The setting survives only with both the host seeding
/// `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` first and our `UpdateNameplates` without the stock
/// friendly show-then-hide (`UIOptionsFrame.lua:775-776`).
#[test]
fn a_world_entry_leaves_the_saved_nameplate_setting_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The client never loads the manifest without a player.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "loader errors: {failures:?}");
    let _ = s.errors();

    // Both halves on, restored from `config.toml`: a host write, which queues nothing.
    let plates = crate::vplates::VPlateMode {
        enemies: true,
        friends: true,
    };
    let saved = |s: &mut UiScript| {
        s.set_cvar_host(crate::vplates::CVAR_ENEMIES, "1");
        s.set_cvar_host(crate::vplates::CVAR_FRIENDS, "1");
        assert!(s.take_cvar_changes().is_empty(), "the host write is silent");
    };

    // The control: with the globals nil, the entry writes both halves off.
    saved(&mut s);
    s.fire_event("VARIABLES_LOADED", vec![]);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert_eq!(
        (
            s.cvar(crate::vplates::CVAR_ENEMIES),
            s.cvar(crate::vplates::CVAR_FRIENDS)
        ),
        (Some("0".into()), Some("0".into())),
        "nil globals ⇒ the replay writes the store off — this is what the report saw"
    );
    let _ = s.take_cvar_changes();

    // With the globals seeded by the host, the entry changes nothing.
    saved(&mut s);
    crate::vplates::push_plate_globals(&s, plates);
    assert_eq!(
        s.eval::<i64>("return NAMEPLATES_ON").unwrap(),
        1,
        "the reference's own truthiness — the NUMBER 1, never the truthy `0`"
    );
    s.fire_event("VARIABLES_LOADED", vec![]);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert_eq!(
        (
            s.cvar(crate::vplates::CVAR_ENEMIES),
            s.cvar(crate::vplates::CVAR_FRIENDS)
        ),
        (Some("1".into()), Some("1".into())),
        "the player's setting survives the entry"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "and queues nothing, so `config.toml` is never dirtied by a world entry"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // An addon that moves a global and calls the verb still writes the CVar.
    s.run("FRIENDNAMEPLATES_ON = nil UpdateNameplates()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![(crate::vplates::CVAR_FRIENDS.to_string(), "0".to_string())],
        "UpdateNameplates keeps its contract — it is adapted, not stubbed"
    );
}

#[test]
fn the_nameplates_page_toggles_the_unit_name_cvars() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowNameplates:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyNameplates:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());
    // Both plates boot off, the reference's state: its plate bits start clear, and
    // `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` start nil (`UIOptionsFrame.lua:181-183`).
    for row in ["RowEnemyPlates", "RowFriendlyPlates"] {
        assert!(
            !s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyNameplates{row}Check:GetChecked()"
            ))
            .unwrap(),
            "{row} defaults unchecked"
        );
    }
    // The name rows at the reference's registered defaults: player "1", NPC and own "0".
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyNameplatesRowPlayerNamesCheck:GetChecked()"
        )
        .unwrap(),
        "Player Names defaults checked"
    );
    for row in ["RowNpcNames", "RowOwnName"] {
        assert!(
            !s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyNameplates{row}Check:GetChecked()"
            ))
            .unwrap(),
            "{row} defaults unchecked"
        );
    }
    let _ = s.take_sounds();

    // The CVars the V and Shift-V bindings write, so the window and the keys agree.
    s.run("BenillaOptionsFrameContainerBodyNameplatesRowFriendlyPlatesCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![(crate::vplates::CVAR_FRIENDS.to_string(), "1".to_string())]
    );
    s.run("BenillaOptionsFrameContainerBodyNameplatesRowEnemyPlatesCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![(crate::vplates::CVAR_ENEMIES.to_string(), "1".to_string())]
    );
    let _ = s.take_sounds();

    // The interface panel's click kit (`PlayClickSound`, `OptionsFrame.lua:509-515`), not the
    // sound panel's inverted one.
    s.run("BenillaOptionsFrameContainerBodyNameplatesRowNpcNamesCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("UnitNameNPC".to_string(), "1".to_string())]
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOn".into())));
    s.run("BenillaOptionsFrameContainerBodyNameplatesRowNpcNamesCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("UnitNameNPC".to_string(), "0".to_string())]
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOff".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The era's `CommitFlag.Apply`: a move snaps to 1.12's 0.01 grid and shows, Apply commits it,
/// and dragging back onto the committed value disarms Apply (`IsModified`).
#[test]
fn the_ui_scale_slider_defers_to_the_apply_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
            .unwrap(),
        "no pending edit, no Apply button"
    );

    // The era shows and enables Apply together.
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.787)")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlValue:GetText()"
        )
        .unwrap(),
        "79%"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "a deferred row must not write the CVar on the move"
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsEnabled() ~= 0")
        .unwrap());

    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.8)")
        .unwrap();
    assert!(s.take_cvar_changes().is_empty());

    s.run("BenillaOptionsFrameApplyButton:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("uiScale".to_string(), "0.8".to_string())]
    );
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());

    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.79)")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.8)")
        .unwrap();
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    assert!(
        s.take_cvar_changes().is_empty(),
        "arming and disarming never touched the CVar"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Render Scale, benilla's own row: a percentage readout, deferred to Apply because a live write
/// would rebuild the world render target on every drag tick.
#[test]
fn the_render_scale_row_shows_a_percentage_and_defers_to_apply() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("renderScale", "1");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();

    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowRenderScaleLabel:GetText()"
        )
        .unwrap(),
        "Render Scale"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowRenderScaleControlValue:GetText()"
        )
        .unwrap(),
        "100%",
        "off has to read as 100%, not as 1"
    );
    // The row offers 50% to 200%, inside the CVar's 0.25 to 4.0 clamp.
    assert!(s
        .eval::<bool>(
            "local lo, hi = BenillaOptionsFrameContainerBodyGraphicsRowRenderScaleControlSlider:GetMinMaxValues()              return math.abs(lo - 0.5) < 0.0001 and math.abs(hi - 2.0) < 0.0001"
        )
        .unwrap());
    // A `BENILLA_` description, as 1.12 has no string for the row.
    assert!(s
        .eval::<bool>("return BENILLA_TOOLTIP_RENDER_SCALE ~= nil")
        .unwrap());
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameContainerBodyGraphicsRowRenderScaleControlSlider:SetValue(0.75)")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowRenderScaleControlValue:GetText()"
        )
        .unwrap(),
        "75%"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "a deferred row must not write the CVar on the move"
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());

    s.run("BenillaOptionsFrameApplyButton:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("renderScale".to_string(), "0.75".to_string())]
    );
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Pending edits are panel-wide, as the era's modified table: a category switch keeps them, and
/// hiding the window discards them.
#[test]
fn a_pending_ui_scale_survives_the_page_switch_and_dies_on_hide() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.7)")
        .unwrap();
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlValue:GetText()"
        )
        .unwrap(),
        "70%"
    );

    // Hiding discards, with no confirm dialog (cut from the era window).
    s.run("HideUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlValue:GetText()"
        )
        .unwrap(),
        "90%"
    );
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    assert!(
        s.take_cvar_changes().is_empty(),
        "the whole pending lifecycle never wrote the CVar"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A live row on 1.12's grid, 177 + n * 60 (`OptionsFrame.lua:26`), so 300 lands on 297.
#[test]
fn the_terrain_distance_slider_snaps_to_the_1_12_grid_and_writes_live() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );

    s.run("BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlSlider:SetValue(300)")
        .unwrap();
    assert!(s
        .eval::<bool>(
            "return math.abs(BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlSlider:GetValue() - 297) < 0.001"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlValue:GetText()"
        )
        .unwrap(),
        "297"
    );
    assert_eq!(
        s.take_cvar_changes(),
        vec![("farclip".to_string(), "297".to_string())]
    );
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
            .unwrap(),
        "a live row stages nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// uiScale back to its registered 0.9, farclip to 350, and a pending uiScale edit dropped.
#[test]
fn defaults_resets_the_graphics_page_to_registered_defaults() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("uiScale", "0.8");
    s.set_cvar_host("farclip", "297");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.7)")
        .unwrap();
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    let changes = s.take_cvar_changes();
    assert!(
        changes.contains(&("uiScale".to_string(), "0.9".to_string())),
        "{changes:?}"
    );
    assert!(
        changes.contains(&("farclip".to_string(), "350".to_string())),
        "{changes:?}"
    );
    assert_eq!(
        changes.len(),
        2,
        "only the default writes queue — never the dead pending: {changes:?}"
    );
    // 357, not 350: the registered 350 is off the reference's 177 + n * 60 ladder
    // (`OptionsFrame.lua:1-2`, `:26`), and `SetValue` snaps it on as 1.12's does; only our
    // readout shows it.
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowFarclipControlValue:GetText()"
        )
        .unwrap(),
        "357"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlValue:GetText()"
        )
        .unwrap(),
        "90%"
    );
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Sticky Targeting reads inverted, as 1.12 checks it on `deselectOnClick` "0"
/// (`UIOptionsFrame.lua:249-252`); the other flags read directly.
#[test]
fn the_controls_page_reads_flags_with_the_sticky_inversion() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("deselectOnClick", "0");
    s.set_cvar_host("autoLootDefault", "1");
    let s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyControls:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowStickyTargetCheck:GetChecked()"
        )
        .unwrap(),
        "deselectOnClick '0' reads as Sticky Targeting CHECKED"
    );
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowAutoLootCheck:GetChecked()"
        )
        .unwrap());
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowInvertMouseCheck:GetChecked()"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowStickyTargetLabel:GetText()"
        )
        .unwrap(),
        "Sticky Targeting"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Checked plays CheckBoxOn (`PlayClickSound`, `OptionsFrame.lua:509-515`), and Sticky
/// Targeting writes inverted, as 1.12 flips it (`UIOptionsFrame.lua:337-343`).
#[test]
fn the_controls_checkboxes_write_flags_with_the_interface_panel_kit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on open must not write it back"
    );
    let _ = s.take_sounds();

    s.run("BenillaOptionsFrameContainerBodyControlsRowInvertMouseCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("mouseInvertPitch".to_string(), "1".to_string())]
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOn".into())));

    s.run("BenillaOptionsFrameContainerBodyControlsRowStickyTargetCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("deselectOnClick".to_string(), "0".to_string())]
    );

    s.run("BenillaOptionsFrameContainerBodyControlsRowStickyTargetCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("deselectOnClick".to_string(), "1".to_string())]
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOff".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn defaults_resets_the_controls_page_to_registered_defaults() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("deselectOnClick", "0");
    s.set_cvar_host("autoLootDefault", "1");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    let changes = s.take_cvar_changes();
    assert!(
        changes.contains(&("deselectOnClick".to_string(), "1".to_string())),
        "{changes:?}"
    );
    assert!(
        changes.contains(&("autoLootDefault".to_string(), "0".to_string())),
        "{changes:?}"
    );
    assert_eq!(changes.len(), 2, "only the moved values queue: {changes:?}");
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowStickyTargetCheck:GetChecked()"
        )
        .unwrap(),
        "deselectOnClick back at '1' reads as Sticky Targeting unchecked"
    );
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowAutoLootCheck:GetChecked()"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Run a few frames so the page body's fit converges: `OptionsScroll_Fit` reads the previous
/// frame's rects.
fn settle(s: &mut UiScript) {
    for _ in 0..4 {
        s.resolve();
        s.tick(0.016);
    }
    s.resolve();
}

/// Results taller than the page grow the scroll body: the bar and its trough appear, and
/// everything past the fold is clipped to the page rect.
#[test]
fn a_broad_search_scrolls_the_page_instead_of_overflowing_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    // Nameplates' six rows fit its area; Controls' sixteen overflow on their own.
    s.run("BenillaOptionsFrameCategoryListRowNameplates:Click()")
        .unwrap();
    settle(&mut s);

    // Control: a page that fits has no bar, and the body is exactly the page rect.
    assert_eq!(
        s.eval::<f32>("return BenillaOptionsFrameContainerScroll:GetVerticalScrollRange()")
            .unwrap(),
        0.0,
        "the Nameplates page fits its area"
    );
    for f in [
        "BenillaOptionsFrameContainerScrollBar",
        "BenillaOptionsFrameContainerScrollBarTrough",
    ] {
        assert!(
            !s.eval::<bool>(&format!("return {f}:IsVisible()")).unwrap(),
            "{f} stays away while the page fits"
        );
    }
    let page_h: f32 = s
        .eval("return BenillaOptionsFrameContainerScroll:GetHeight()")
        .unwrap();
    assert!(
        (s.eval::<f32>("return BenillaOptionsFrameContainerBody:GetHeight()")
            .unwrap()
            - page_h)
            .abs()
            < 0.5,
        "the body is the page rect when nothing overflows"
    );

    s.run("BenillaOptionsFrameSearchBox:SetText(\"e\")")
        .unwrap();
    settle(&mut s);
    let range: f32 = s
        .eval("return BenillaOptionsFrameContainerScroll:GetVerticalScrollRange()")
        .unwrap();
    assert!(
        range > 100.0,
        "the results outrun the page, so there is something to scroll (range {range})"
    );
    for f in [
        "BenillaOptionsFrameContainerScrollBar",
        "BenillaOptionsFrameContainerScrollBarTrough",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {f}:IsVisible()")).unwrap(),
            "{f} appears with the overflow"
        );
    }

    // Every content quad carries the page rect as its clip, so nothing draws past the page's
    // bottom while the rects themselves reach far below it.
    let quads = s.extract();
    let clip = quads
        .iter()
        .find_map(|q| q.clip)
        .expect("the page clips its content");
    assert!(
        quads.iter().all(|q| q.clip.is_none_or(|c| c == clip)),
        "one clip in play: the page area"
    );
    let deepest_rect = quads
        .iter()
        .filter(|q| q.clip.is_some())
        .filter_map(|q| q.rect)
        .map(|r| r.bottom)
        .fold(f32::INFINITY, f32::min);
    assert!(
        deepest_rect < clip.bottom - 100.0,
        "there is a real fold to make: the results reach {deepest_rect} against a page bottom of {}",
        clip.bottom
    );
    let deepest_drawn = quads
        .iter()
        .filter_map(|q| Some((q.rect?, q.clip?)))
        .map(|(r, c)| r.bottom.max(c.bottom))
        .fold(f32::INFINITY, f32::min);
    assert!(
        deepest_drawn >= clip.bottom - 0.01,
        "…and it is folded AT the page bottom, not spilling ({deepest_drawn} vs {})",
        clip.bottom
    );

    // Widget coordinates here, not the extract's pixels: the window carries `ERA_WINDOW_SCALE`.
    let sf_bottom: f32 = s
        .eval("return BenillaOptionsFrameContainerScroll:GetBottom()")
        .unwrap();
    let tail = |s: &UiScript| -> f32 { s.eval("return OptionsScroll_ContentBottom()").unwrap() };
    assert!(
        tail(&s) < sf_bottom,
        "the tail starts below the fold ({} vs {sf_bottom})",
        tail(&s)
    );
    s.run(&format!(
        "BenillaOptionsFrameContainerScrollBar:SetValue({range})"
    ))
    .unwrap();
    settle(&mut s);
    assert!(
        tail(&s) >= sf_bottom - 0.5,
        "scrolling to the end brings the tail into the page ({} vs {sf_bottom})",
        tail(&s)
    );

    s.run("BenillaOptionsFrameSearchBox:SetText(\"\")").unwrap();
    settle(&mut s);
    assert_eq!(
        s.eval::<f32>("return BenillaOptionsFrameContainerScroll:GetVerticalScrollRange()")
            .unwrap(),
        0.0
    );
    for f in [
        "BenillaOptionsFrameContainerScrollBar",
        "BenillaOptionsFrameContainerScrollBarTrough",
    ] {
        assert!(
            !s.eval::<bool>(&format!("return {f}:IsVisible()")).unwrap(),
            "{f} goes away with the results"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The trough, 1.12's `UI-Character-ScrollBar` channel, seats itself on the bar at the reference's
/// hang: 31 wide, 8 left of the 16-wide bar, overhanging it 21 above and 20 below, so each arrow
/// sits in the 16-tall socket the art carries for it.
#[test]
fn the_page_scroll_bar_wears_the_trough_with_its_arrows_in_the_sockets() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameSearchBox:SetText(\"e\")")
        .unwrap();
    settle(&mut s);

    let bar = "BenillaOptionsFrameContainerScrollBar";
    let trough = "BenillaOptionsFrameContainerScrollBarTrough";
    let g = |f: &str, m: &str| s.eval::<f32>(&format!("return {f}:{m}()")).unwrap();
    assert!(
        (g(trough, "GetWidth") - 31.0).abs() < 0.01,
        "the trough is the 31-wide channel"
    );
    assert!(
        (g(bar, "GetWidth") - 16.0).abs() < 0.01,
        "against a 16-wide bar"
    );
    assert!(
        (g(bar, "GetLeft") - g(trough, "GetLeft") - 8.0).abs() < 0.01,
        "the trough hangs the ref's 8 units left of the bar, so the bar rides it centred"
    );
    // The reference's hang: ReputationFrame.xml's trough at its scroll frame's +5/-4, against the
    // template bar's -16/+16.
    assert!(
        (g(trough, "GetTop") - g(bar, "GetTop") - 21.0).abs() < 0.01,
        "trough top 21 above the bar"
    );
    assert!(
        (g(bar, "GetBottom") - g(trough, "GetBottom") - 20.0).abs() < 0.01,
        "trough bottom 20 below the bar"
    );
    // Off the arrows: 5 units of cap above the up arrow, 4 below the down arrow.
    let up = format!("{bar}ScrollUpButton");
    let down = format!("{bar}ScrollDownButton");
    assert!((g(&up, "GetHeight") - 16.0).abs() < 0.01, "16-tall arrows");
    assert!((g(&down, "GetHeight") - 16.0).abs() < 0.01);
    assert!(
        (g(trough, "GetTop") - g(&up, "GetTop") - 5.0).abs() < 0.01,
        "the cap shows 5 above the up arrow"
    );
    assert!(
        (g(&down, "GetBottom") - g(trough, "GetBottom") - 4.0).abs() < 0.01,
        "and 4 below the down arrow"
    );
    // On this page the channel spans exactly the scroll frame, the bar taking the 21/20 inset.
    let sf = "BenillaOptionsFrameContainerScroll";
    assert!((g(trough, "GetTop") - g(sf, "GetTop")).abs() < 0.01);
    assert!((g(trough, "GetBottom") - g(sf, "GetBottom")).abs() < 0.01);

    // Three slices of the one 1.12 file, flush and at the channel's full width.
    let mut slices: Vec<_> = s
        .extract()
        .into_iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } if p.contains("UI-Character-ScrollBar") => {
                q.rect
            }
            _ => None,
        })
        .collect();
    assert_eq!(slices.len(), 3, "top cap, stretched run, bottom cap");
    slices.sort_by(|a, b| b.top.total_cmp(&a.top));
    assert!(
        slices
            .windows(2)
            .all(|w| (w[0].bottom - w[1].top).abs() < 0.01),
        "the three stack flush into one channel: {slices:?}"
    );
    let width = slices[0].right - slices[0].left;
    assert!(
        slices
            .iter()
            .all(|r| (r.right - r.left - width).abs() < 0.01),
        "…all at the channel's own width: {slices:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The row tooltips ─────────────────────────────────────────────────────────────

/// Hover a row's label half, 60 units in at mid-height. Callers pin `ERA_WINDOW_SCALE = 1` first,
/// so the row's rect and the pointer share coordinates.
fn hover_label(s: &mut UiScript, frame: &str) {
    s.resolve();
    let g = |s: &mut UiScript, verb: &str| -> f32 {
        s.eval(&format!("return {frame}:{verb}()")).unwrap()
    };
    let (l, t, b) = (g(s, "GetLeft"), g(s, "GetTop"), g(s, "GetBottom"));
    s.mouse_move(l + 60.0, (t + b) * 0.5);
}

/// Scroll `frame` into the page's rect: past the fold a row is clipped and cannot be hovered.
fn scroll_into_view(s: &mut UiScript, frame: &str) {
    s.resolve();
    s.run(&format!(
        "local sf = BenillaOptionsFrameContainerScroll \
         local range = sf:GetVerticalScrollRange() \
         if range and range > 0 then \
           local row = getglobal(\"{frame}\") \
           local want = sf:GetVerticalScroll() \
           if row:GetBottom() < sf:GetBottom() then \
             want = want + (sf:GetBottom() - row:GetBottom()) + 2 \
           elseif row:GetTop() > sf:GetTop() then \
             want = want - (row:GetTop() - sf:GetTop()) - 2 \
           end \
           if want < 0 then want = 0 end \
           if want > range then want = range end \
           sf:SetVerticalScroll(want) \
         end"
    ))
    .unwrap();
    s.resolve();
}

/// The row's key resolves at hover, so a string set after the window loaded still paints.
#[test]
fn a_hovered_row_raises_its_1_12_description_on_the_era_seat() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    // The stock tooltip sizes from its lines through the font engine, so reading its rect needs
    // a measurer.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    s.run("OPTION_TOOLTIP_GAMEFIELD_DESELECT = \"Checking this will prevent the deselection.\"")
        .unwrap();
    s.run("ERA_WINDOW_SCALE = 1").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    let row = "BenillaOptionsFrameContainerBodyControlsRowStickyTarget";
    hover_label(&mut s, row);
    s.resolve();
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the label half raises the plate — the reported gap"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Checking this will prevent the deselection."
    );
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        1,
        "the description ALONE — the era's white name line is cut"
    );
    // The era seat, `DefaultTooltipMixin`'s `ANCHOR_RIGHT` at x -10: BOTTOMLEFT on the label
    // region's TOPRIGHT, 10 back.
    let owned: bool = s
        .eval(&format!("return GameTooltip:IsOwned({row}Tip)"))
        .unwrap();
    assert!(owned, "owned by the row's $parentTip region");
    let (tip_right, tip_top): (f32, f32) = (
        s.eval(&format!("return {row}Tip:GetRight()")).unwrap(),
        s.eval(&format!("return {row}Tip:GetTop()")).unwrap(),
    );
    let (left, bottom): (f32, f32) = (
        s.eval("return GameTooltip:GetLeft()").unwrap(),
        s.eval("return GameTooltip:GetBottom()").unwrap(),
    );
    assert!(
        (left - (tip_right - 10.0)).abs() < 0.01 && (bottom - tip_top).abs() < 0.01,
        "plate at ({left}, {bottom}); the era seat is ({}, {tip_top})",
        tip_right - 10.0
    );

    // Onto the checkbox: the row's OnLeave and the box's OnEnter land in one move.
    let (bl, br, bt, bb): (f32, f32, f32, f32) = (
        s.eval(&format!("return {row}Check:GetLeft()")).unwrap(),
        s.eval(&format!("return {row}Check:GetRight()")).unwrap(),
        s.eval(&format!("return {row}Check:GetTop()")).unwrap(),
        s.eval(&format!("return {row}Check:GetBottom()")).unwrap(),
    );
    s.mouse_move((bl + br) * 0.5, (bt + bb) * 0.5);
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the plate survives the crossing onto the control"
    );

    s.mouse_move(5.0, 5.0);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "OnLeave drops the plate"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Auto Loot has no 1.12 setting and so no string: its hover raises nothing, and puts away a
/// neighbour's plate left standing without an OnLeave.
#[test]
fn a_row_with_no_1_12_string_raises_no_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("OPTION_TOOLTIP_GAMEFIELD_DESELECT = \"Sticky's own description.\"")
        .unwrap();
    s.run("ERA_WINDOW_SCALE = 1").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    hover_label(
        &mut s,
        "BenillaOptionsFrameContainerBodyControlsRowAutoLoot",
    );
    s.resolve();
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "no 1.12 description, no plate"
    );
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowAutoLootHover:IsVisible()"
        )
        .unwrap(),
        "…but the row still lights: the wash is not gated on the string"
    );

    // Sticky's plate up, then straight into the mute row with no OnLeave in between.
    s.run("OptionsRow_Hover(BenillaOptionsFrameContainerBodyControlsRowStickyTarget, 1)")
        .unwrap();
    assert!(s.eval::<bool>("return GameTooltip:IsVisible()").unwrap());
    s.run("OptionsRow_Hover(BenillaOptionsFrameContainerBodyControlsRowAutoLoot, 1)")
        .unwrap();
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the mute row puts the neighbour's description away — it never describes THIS row"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Every row's key resolves in the player's 1.12 `GlobalStrings.lua`, so a mistyped key cannot
/// silence a description.
#[test]
fn every_row_tooltip_key_resolves_in_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let strings = UiScript::new().expect("VM");
    strings
        .run(&String::from_utf8_lossy(&src))
        .expect("GlobalStrings runs clean");

    // Read off the live rows, so a new page is checked too.
    let s = harness();
    let listing: String = s
        .eval(
            "local out = {} \
             for page, rows in pairs(OPTIONS_PAGE_ROWS) do \
               for _, rkey in ipairs(rows) do \
                 local row = getglobal(\"BenillaOptionsFrameContainerBody\" .. page .. rkey) \
                 table.insert(out, page .. rkey .. \"=\" .. (row.tip or \"\")) \
               end \
             end \
             return table.concat(out, \",\")",
        )
        .unwrap();

    let mut untipped = vec![];
    let mut checked = 0;
    for entry in listing.split(',') {
        let (row, key) = entry.split_once('=').expect("row=key");
        if key.is_empty() {
            untipped.push(row.to_string());
            continue;
        }
        // Four rows with no fitting 1.12 string carry a `BENILLA_` one, each held to its row:
        // Render Scale and Enable Sound in Background have no 1.12 setting, Display Mode's
        // string describes a checkbox, and `OPTION_TOOLTIP_GAMMA` cites art this page lacks.
        const BENILLA_OWNED: &[(&str, &str)] = &[
            ("BENILLA_TOOLTIP_RENDER_SCALE", "GraphicsRowRenderScale"),
            ("BENILLA_TOOLTIP_DISPLAY_MODE", "GraphicsRowDisplayMode"),
            (
                "BENILLA_TOOLTIP_BACKGROUND_SOUND",
                "AudioRowBackgroundSound",
            ),
            ("BENILLA_TOOLTIP_BRIGHTNESS", "GraphicsRowBrightness"),
        ];
        if let Some((_, want_row)) = BENILLA_OWNED.iter().find(|(k, _)| *k == key) {
            assert_eq!(row, *want_row, "{row}: not this row's string");
            let text: String = s.eval(&format!("return {key}")).unwrap();
            assert!(!text.is_empty(), "{row}: {key} resolves to nothing");
            checked += 1;
            continue;
        }
        assert!(
            key.starts_with("OPTION_TOOLTIP_"),
            "{row}: {key} is not a 1.12 option-tooltip key"
        );
        let text: String = strings
            .lua()
            .globals()
            .get::<String>(key)
            .unwrap_or_default();
        assert!(
            !text.is_empty() && text != "PLACE_HOLDER",
            "{row}: {key} resolves to nothing in the real GlobalStrings"
        );
        checked += 1;
    }
    // 81 rows less the three untipped below; four of the 78 carry a `BENILLA_` key, and a dropdown
    // row is checked on the key it wears at rest.
    assert_eq!(checked, 78, "every tipped row carries a live key");
    assert_eq!(
        untipped,
        vec![
            "ControlsRowAutoLoot".to_string(),
            "NameplatesRowEnemyPlates".to_string(),
            "NameplatesRowFriendlyPlates".to_string(),
        ],
        "the rows with no 1.12 option-tooltip string: Auto Loot (no 1.12 setting at all) and \
         the two V-plate toggles (1.12 HAS the setting, as the V/Shift-V keybinding over a \
         RegisterForSave'd global, but its options UI never carried a row for it — its own \
         UIOptionsFrame comment says so — so there is no OPTION_TOOLTIP_ key to resolve)"
    );

    // Sticky Targeting's string, byte for byte off the chain.
    assert_eq!(
        strings
            .lua()
            .globals()
            .get::<String>("OPTION_TOOLTIP_GAMEFIELD_DESELECT")
            .unwrap(),
        "Checking this will prevent the deselection of targets by clicking on the gamefield.  \
         Targets can only be cleared by pressing escape or clicking another target."
    );
}

/// Each row template needs its `$parentTip` seat: without it `SetOwner(nil, ...)` errors instead
/// of raising a plate.
#[test]
fn every_flavor_of_row_raises_its_plate_from_the_page_it_lives_on() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    // Stand-in strings for every key the rows name.
    s.run(
        "for page, rows in pairs(OPTIONS_PAGE_ROWS) do \
           for _, rkey in ipairs(rows) do \
             local row = getglobal(\"BenillaOptionsFrameContainerBody\" .. page .. rkey) \
             if row.tip then setglobal(row.tip, \"described: \" .. rkey) end \
           end \
         end",
    )
    .unwrap();
    s.run("ERA_WINDOW_SCALE = 1").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    let mut raised = 0;
    for page in [
        "Controls",
        "Audio",
        "Graphics",
        "Nameplates",
        "Combat",
        "Interface",
        "ActionBars",
        "Chat",
    ] {
        s.run(&format!("BenillaOptionsFrameCategoryListRow{page}:Click()"))
            .unwrap();
        let rows: String = s
            .eval(&format!(
                "return table.concat(OPTIONS_PAGE_ROWS.{page}, \",\")"
            ))
            .unwrap();
        for rkey in rows.split(',') {
            let row = format!("BenillaOptionsFrameContainerBody{page}{rkey}");
            scroll_into_view(&mut s, &row);
            hover_label(&mut s, &row);
            s.resolve();
            let tipped: bool = s.eval(&format!("return {row}.tip ~= nil")).unwrap();
            let shown: bool = s.eval("return GameTooltip:IsVisible()").unwrap();
            assert_eq!(
                shown, tipped,
                "{row}: plate shown {shown}, has key {tipped}"
            );
            if tipped {
                assert_eq!(
                    s.eval::<String>("return GameTooltipTextLeft1:GetText()")
                        .unwrap(),
                    format!("described: {rkey}"),
                    "{row}: the plate reads ITS OWN row's description"
                );
                assert!(
                    s.eval::<bool>(&format!("return GameTooltip:IsOwned({row}Tip)"))
                        .unwrap(),
                    "{row}: seated on its own $parentTip region"
                );
                raised += 1;
            }
            s.mouse_move(5.0, 5.0);
        }
        assert!(
            s.errors().is_empty(),
            "{page}: script errors: {:?}",
            s.errors()
        );
    }
    // Every tipped row: the same 78 the key census counts.
    assert_eq!(raised, 78, "every row but Auto Loot raises a description");
}

/// 1.12's AdvancedOptionsCombatText box as saved-global rows: a click writes the global, never a
/// CVar, and runs the family's apply, `CombatText_UpdateOrLoad`.
#[test]
fn the_combat_page_writes_saved_variable_globals_and_applies_them() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyCombat:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());

    // Each box reads its global at `UIOptionsFrame_Init`'s value, the reference's default.
    for (row, checked) in [
        ("RowCombatText", false),
        ("RowAuras", true),
        ("RowAuraFade", false),
        ("RowDodgeParryMiss", false),
    ] {
        assert_eq!(
            s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyCombat{row}Check:GetChecked() and true or false"
            ))
            .unwrap(),
            checked,
            "{row} reads its global"
        );
    }
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyCombatRowFloatModeDropdownText:GetText()"
        )
        .unwrap(),
        "Scroll Up"
    );

    // The master first: it ships "0", which greys the other rows, and a greyed box eats its click.
    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyCombatRowCombatTextCheck:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(),
        "1",
        "the master writes its own global"
    );
    s.run("BenillaOptionsFrameContainerBodyCombatRowDodgeParryMissCheck:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_SHOW_DODGE_PARRY_MISS")
            .unwrap(),
        "1"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "a uvar row must not touch the CVar table"
    );
    assert_eq!(
        s.eval::<i64>("return COMBAT_TEXT_TYPE_INFO[\"MISS\"].show or 0")
            .unwrap(),
        1,
        "applyFunc ran: the message type is live now"
    );

    // The dropdown applies too: the scroll function follows the mode.
    s.run(
        "BenillaOptionsFrameContainerBodyCombatRowFloatModeDropdownButton:Click() \
         DropDownList1Button2:Click()",
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_FLOAT_MODE").unwrap(),
        "2"
    );
    assert!(
        s.eval::<bool>("return COMBAT_TEXT_SCROLL_FUNCTION == CombatText_StandardScroll")
            .unwrap(),
        "mode 2 keeps the standard scroll with a downward step (CombatText.xml's own arm)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12's rule (`UIOptionsFrame.lua:728-763`): the master off greys the floating-text rows and the
/// dropdown, and Combo Points stays greyed for all but rogues and druids.
#[test]
fn the_combat_master_greys_the_family_and_combo_points_is_class_gated() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();

    let enabled = |s: &mut UiScript, row: &str, control: &str| -> bool {
        s.eval::<bool>(&format!(
            "return BenillaOptionsFrameContainerBodyCombat{row}{control}:IsEnabled() ~= 0"
        ))
        .unwrap()
    };
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "0");
    assert!(!enabled(&mut s, "RowAuras", "Check"));
    assert!(!enabled(&mut s, "RowFloatMode", "DropdownButton"));
    assert!(enabled(&mut s, "RowCombatText", "Check"));

    // No player class in this VM, so Combo Points stays greyed while the rest wake.
    s.run("BenillaOptionsFrameContainerBodyCombatRowCombatTextCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "1");
    assert!(enabled(&mut s, "RowAuras", "Check"));
    assert!(!enabled(&mut s, "RowComboPoints", "Check"));
    assert!(enabled(&mut s, "RowFloatMode", "DropdownButton"));

    s.run("BenillaOptionsFrameContainerBodyCombatRowCombatTextCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "0");
    assert!(
        !enabled(&mut s, "RowAuras", "Check"),
        "the master off greys every sub-toggle"
    );
    assert!(
        !enabled(&mut s, "RowFloatMode", "DropdownButton"),
        "…and the scroll dropdown, so its list cannot even open"
    );
    assert!(
        enabled(&mut s, "RowCombatText", "Check"),
        "the master itself stays live — it is the way back"
    );
    assert!(
        !enabled(&mut s, "RowComboPoints", "Check"),
        "the class gate outlives the master's departure too"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Defaults restores each global to the value captured at its row's OnLoad, before the saved
/// chunk runs: `UIOptionsFrame_Init`'s, with no second copy in the window.
#[test]
fn defaults_resets_the_combat_page_to_the_shipped_assignments() {
    benilla_formats::wow_data_or_skip!();
    let s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    // Four moved, both ways and both row kinds; the master first, as a greyed box eats its click.
    s.run(
        "BenillaOptionsFrameContainerBodyCombatRowCombatTextCheck:Click() \
         BenillaOptionsFrameContainerBodyCombatRowAurasCheck:Click() \
         BenillaOptionsFrameContainerBodyCombatRowReputationCheck:Click() \
         BenillaOptionsFrameContainerBodyCombatRowFloatModeDropdownButton:Click() \
         DropDownList1Button3:Click()",
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "1");
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_SHOW_AURAS").unwrap(),
        "0"
    );
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_SHOW_REPUTATION")
            .unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_FLOAT_MODE").unwrap(),
        "3"
    );

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(),
        "0",
        "the master walks back too — the reference's own value"
    );
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_SHOW_AURAS").unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_SHOW_REPUTATION")
            .unwrap(),
        "0"
    );
    assert_eq!(
        s.eval::<String>("return COMBAT_TEXT_FLOAT_MODE").unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyCombatRowFloatModeDropdownText:GetText()"
        )
        .unwrap(),
        "Scroll Up",
        "the capsule follows the reset"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A toggle is saved under its own name, and a fresh VM running the saved text comes up on it.
#[test]
fn what_the_combat_page_writes_survives_a_restart() {
    benilla_formats::wow_data_or_skip!();
    let s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    // The master first: it ships off, and a greyed box eats its click.
    s.run("BenillaOptionsFrameContainerBodyCombatRowCombatTextCheck:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameContainerBodyCombatRowHonorGainedCheck:Click()")
        .unwrap();

    let saved = String::from_utf8(s.saved_variables_bytes()).unwrap();
    assert!(
        saved.contains("COMBAT_TEXT_SHOW_HONOR_GAINED = \"0\""),
        "the toggle is in the saved text:\n{saved}"
    );
    assert!(
        saved.contains("SHOW_COMBAT_TEXT = \"1\""),
        "and so is the master:\n{saved}"
    );

    // The restart: a fresh tree at its defaults, then the saved chunk over it.
    let fresh = combat_harness();
    assert_eq!(
        fresh
            .eval::<String>("return COMBAT_TEXT_SHOW_HONOR_GAINED")
            .unwrap(),
        "1"
    );
    assert_eq!(
        fresh.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(),
        "0"
    );
    fresh.run(&saved).unwrap();
    assert_eq!(
        fresh
            .eval::<String>("return COMBAT_TEXT_SHOW_HONOR_GAINED")
            .unwrap(),
        "0",
        "the saved value wins over the file-scope default"
    );
    assert_eq!(
        fresh.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(),
        "1",
        "…and so does the master the player turned on"
    );
    fresh.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    fresh
        .run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    assert!(!fresh
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyCombatRowHonorGainedCheck:GetChecked() and true or false"
        )
        .unwrap());
    assert!(
        fresh.errors().is_empty(),
        "script errors: {:?}",
        fresh.errors()
    );
}

/// The lock row's `LOCK_ACTIONBAR` write reaches the stock bar's drag guard in the same VM.
#[test]
fn the_action_bars_page_locks_the_real_bar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = actionbars_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowActionBars:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyActionBars:IsVisible()")
        .unwrap());
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyActionBarsRowLockActionBarCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "unchecked: ActionBar.xml ships the bar unlocked"
    );

    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyActionBarsRowLockActionBarCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return LOCK_ACTIONBAR").unwrap(), "1");
    assert!(
        s.take_cvar_changes().is_empty(),
        "a uvar row must not touch the CVar table"
    );

    s.set_action(
        1,
        Some(benilla_ui::script::ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnDragStart\")()")
        .unwrap();
    assert!(
        s.cursor_payload().is_none(),
        "the page's write reached the bar's guard"
    );

    // Defaults walks it back to `UIOptionsFrame_Init`'s "0", and the bar drags again.
    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return LOCK_ACTIONBAR").unwrap(), "0");
    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnDragStart\")()")
        .unwrap();
    assert!(s.cursor_payload().is_some(), "unlocked again");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Three saved-global rows with no apply: each consumer reads its global as it acts, so the write
/// alone changes behaviour.
#[test]
fn the_interface_page_writes_the_three_stock_globals() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyInterface:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());

    // `UIOptionsFrame_Init`'s values, but for our window's string "1" in `AUTO_QUEST_WATCH`.
    for (row, checked) in [
        ("RowInstantQuestText", false),
        ("RowAutoQuestWatch", true),
        ("RowNewbieTips", true),
    ] {
        assert_eq!(
            s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyInterface{row}Check:GetChecked() and true or false"
            ))
            .unwrap(),
            checked,
            "{row} reads its global"
        );
    }

    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowNewbieTipsCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return SHOW_NEWBIE_TIPS").unwrap(), "0");
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowAutoQuestWatchCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return AUTO_QUEST_WATCH").unwrap(), "0");
    assert!(
        s.take_cvar_changes().is_empty(),
        "a uvar row must not touch the CVar table"
    );

    // The questgiver's fade arm reads the global on each show (`QuestFrame.lua:85`).
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowInstantQuestTextCheck:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return QUEST_FADING_DISABLE").unwrap(),
        "1"
    );
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowInstantQuestText.applyFunc == nil"
        )
        .unwrap(),
        "these three need no apply hook"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The picker is dead while the switch is off (`UIOptionsFrame.lua:697-701`), both rows run
/// `TargetofTarget_Update` on a write, and the five entries are the reference's, in its order.
#[test]
fn the_target_of_target_rows_gate_each_other_and_write_their_globals() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();

    // Off, at Always: `UIOptionsFrame_Init`'s two values (`UIOptionsFrame.lua:116-117`).
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "the switch ships off"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetModeDropdownText:GetText()"
        )
        .unwrap(),
        "Always"
    );
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetModeDropdownButton \
             :IsEnabled() ~= 0"
        )
        .unwrap(),
        "the picker is dead while the switch is off"
    );

    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetCheck:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return SHOW_TARGET_OF_TARGET").unwrap(),
        "1"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "a uvar row must not touch the CVar table"
    );
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetModeDropdownButton \
             :IsEnabled() ~= 0"
        )
        .unwrap(),
        "and the picker wakes with it"
    );
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTarget.applyFunc \
                 == \"TargetofTarget_Update\" \
             and BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetMode.applyFunc \
                 == \"TargetofTarget_Update\""
        )
        .unwrap(),
        "both rows re-decide the frame when they are written"
    );

    s.run("BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetModeDropdownButton:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return DropDownList1.numButtons").unwrap(),
        5.0
    );
    assert!(
        s.eval::<bool>("return DropDownList1Button5Check:IsVisible()")
            .unwrap(),
        "Always is the one checked"
    );
    s.run("DropDownList1Button3:Click()").unwrap();
    assert_eq!(
        s.eval::<String>("return SHOW_TARGET_OF_TARGET_STATE")
            .unwrap(),
        "3",
        "Solo is the reference's third value"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowTargetOfTargetModeDropdownText:GetText()"
        )
        .unwrap(),
        "Solo"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "still nothing in the CVar table"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Show Cloak and Show Helm are bits of the character's `PLAYER_FLAGS`, read and written through
/// 1.12's `func`/`setFunc` pair (`UIOptionsFrame.lua:18-19`). The getter follows the click before
/// the server answers: the wire verb is a blind flip, so a second click must see the new state.
#[test]
fn the_equipment_display_rows_read_and_write_through_the_api_not_a_store() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();

    // Both ship shown, the reference's hand-written default (`UIOptionsFrame.lua:631-634`).
    for row in ["RowShowHelm", "RowShowCloak"] {
        assert!(
            s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyInterface{row}Check:GetChecked() and true or false"
            ))
            .unwrap(),
            "{row} reads the API"
        );
    }

    let _ = s.take_cvar_changes();
    let _ = s.take_worn_display_toggles();
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowShowHelmCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_worn_display_toggles(),
        vec![WornDisplay::Helm],
        "the click is a CMSG_TOGGLE_HELM intent, not a stored value"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "an API row must not touch the CVar table"
    );
    assert!(
        !s.eval::<bool>("return ShowingHelm() and true or false")
            .unwrap(),
        "the getter follows the click at once — the server has not answered yet"
    );
    assert!(
        s.eval::<bool>("return ShowingCloak() and true or false")
            .unwrap(),
        "and the other slot is a different bit"
    );

    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowShowHelmCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "the revisit re-asks the getter"
    );

    // A descriptor edge that disagrees wins over the optimistic flip.
    s.set_worn_display(true, true);
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowShowHelmCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "the server said the helm is shown, so the row says so"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Show Tutorials' entry is a bare `{ index = 28 }` (`UIOptionsFrame.lua:37`): its store is the
/// special arms, `TutorialsEnabled()` to read and `ClearTutorials()`/`ResetTutorials()` to write.
#[test]
fn show_tutorials_reads_the_bank_and_writes_through_clear_and_reset() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    // A bank with unacknowledged bits: tutorials are enabled.
    s.set_tutorial_bank(Some(vec![0x00; 32]));
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowShowTutorialsCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "the row reads TutorialsEnabled(), not a stored value"
    );

    let _ = s.take_cvar_changes();
    let _ = s.take_tutorial_clears();
    let _ = s.take_tutorial_resets();

    s.run("BenillaOptionsFrameContainerBodyInterfaceRowShowTutorialsCheck:Click()")
        .unwrap();
    assert_eq!(s.take_tutorial_clears(), 1, "off clears the bank");
    assert_eq!(s.take_tutorial_resets(), 0);
    assert!(
        s.take_cvar_changes().is_empty(),
        "there is no tutorial CVar in 1.12 — the row must not invent one"
    );

    // The app pushes the cleared bank back; the row now reads off.
    s.set_tutorial_bank(Some(vec![0xFF; 32]));
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowShowTutorialsCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "the revisit re-asks the getter"
    );

    s.run("BenillaOptionsFrameContainerBodyInterfaceRowShowTutorialsCheck:Click()")
        .unwrap();
    assert_eq!(s.take_tutorial_resets(), 1, "on resets the bank");
    assert_eq!(s.take_tutorial_clears(), 0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference's `~=` guard (`UIOptionsFrame.lua:319`): writing the value the row already holds
/// sends nothing, so Defaults cannot re-arm dismissed tutorials.
#[test]
fn a_no_op_write_does_not_re_arm_the_tutorials() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.set_tutorial_bank(Some(vec![0x00; 32])); // enabled
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    let _ = s.take_tutorial_clears();
    let _ = s.take_tutorial_resets();

    // Defaults writes "1" over a row that already reads "1".
    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        (s.take_tutorial_resets(), s.take_tutorial_clears()),
        (0, 0),
        "the guard held: no bank write for a value that had not moved"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The setter is a set over a wire verb that toggles, so Defaults flips only the row that moved.
#[test]
fn defaults_sends_a_flip_only_for_the_row_that_moved() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowShowCloakCheck:Click()")
        .unwrap();
    let _ = s.take_worn_display_toggles();
    assert!(!s
        .eval::<bool>("return ShowingCloak() and true or false")
        .unwrap());

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.take_worn_display_toggles(),
        vec![WornDisplay::Cloak],
        "only the cloak had moved"
    );
    assert!(s
        .eval::<bool>("return ShowingCloak() and true or false")
        .unwrap());
    assert!(s
        .eval::<bool>("return ShowingHelm() and true or false")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Defaults restores what each global's definer assigned, captured at the row's OnLoad, so the
/// window keeps no second copy of a default.
#[test]
fn defaults_on_the_interface_page_restores_the_definers_own_assignment() {
    benilla_formats::wow_data_or_skip!();
    let s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    s.run(
        "BenillaOptionsFrameContainerBodyInterfaceRowInstantQuestTextCheck:Click() \
         BenillaOptionsFrameContainerBodyInterfaceRowNewbieTipsCheck:Click()",
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return QUEST_FADING_DISABLE").unwrap(),
        "1"
    );
    assert_eq!(s.eval::<String>("return SHOW_NEWBIE_TIPS").unwrap(), "0");

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return QUEST_FADING_DISABLE").unwrap(),
        "0",
        "back to QuestFrame.xml's own assignment, which is the reference's \"0\""
    );
    assert_eq!(s.eval::<String>("return SHOW_NEWBIE_TIPS").unwrap(), "1");
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowInstantQuestTextCheck:GetChecked() \
             and true or false"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The toggle is saved under the reference's global name, `AUTO_QUEST_WATCH`, and a fresh tree
/// replaying the text comes up on it.
#[test]
fn what_the_interface_page_writes_survives_a_restart() {
    benilla_formats::wow_data_or_skip!();
    let s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowAutoQuestWatchCheck:Click()")
        .unwrap();

    let saved = String::from_utf8(s.saved_variables_bytes()).unwrap();
    assert!(
        saved.contains("AUTO_QUEST_WATCH = \"0\""),
        "the toggle is in the saved text under the reference's name:\n{saved}"
    );

    let fresh = interface_harness();
    assert_eq!(
        fresh.eval::<String>("return AUTO_QUEST_WATCH").unwrap(),
        "1"
    );
    fresh.run(&saved).unwrap();
    assert_eq!(
        fresh.eval::<String>("return AUTO_QUEST_WATCH").unwrap(),
        "0",
        "the saved value wins over the file-scope default"
    );
    fresh.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    fresh
        .run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(!fresh
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowAutoQuestWatchCheck:GetChecked() \
             and true or false"
        )
        .unwrap());
    assert!(
        fresh.errors().is_empty(),
        "script errors: {:?}",
        fresh.errors()
    );
}

/// The saved chunk runs after the whole UI has loaded, so a saved-on `SHOW_COMBAT_TEXT` takes
/// effect through the stock `VARIABLES_LOADED` ladder, which loads `Blizzard_CombatText` for it
/// (`UIOptionsFrame.lua:204-226`).
#[test]
fn a_saved_switch_with_a_side_effect_is_applied_when_the_variables_land() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    // What the saved chunk does: assign over the default, then the event.
    s.run("SHOW_COMBAT_TEXT = \"1\"").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    // A frame passes before any message can arrive, and the stock placement reads the screen
    // positions that frame lays out.
    s.resolve();

    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            benilla_ui::script::ScriptValue::Str("DAMAGE".into()),
            benilla_ui::script::ScriptValue::Str("17".into()),
        ],
    );
    assert!(
        s.eval::<bool>("return getn(COMBAT_TEXT_TO_ANIMATE) == 1")
            .unwrap(),
        "the saved-on master is applied at load: the walk armed the registrations the XML did not"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The row pitch is set once, by `BuffButtons_UpdatePositions` (`BuffFrame.lua:152-160`): the
/// second row hangs 15 below the first with timers and 5 without. The click re-runs it, and a
/// saved value needs the stock `VARIABLES_LOADED` arm (`UIOptionsFrame.lua:206`).
#[test]
fn the_buff_durations_row_repitches_the_bar_and_the_pitch_survives_a_restart() {
    benilla_formats::wow_data_or_skip!();
    let gap = |s: &mut UiScript| -> f64 {
        s.resolve();
        s.eval::<f64>("return BuffButton0:GetBottom() - BuffButton8:GetTop()")
            .unwrap()
    };

    let mut s = interface_harness();
    assert_eq!(s.eval::<String>("return SHOW_BUFF_DURATIONS").unwrap(), "0");
    assert!(
        (gap(&mut s) - 5.0).abs() < 1e-3,
        "the shipped default is the durations-HIDDEN geometry: {}",
        gap(&mut s)
    );

    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowBuffDurationsCheck:GetChecked() \
             and true or false"
        )
        .unwrap());

    s.run("BenillaOptionsFrameContainerBodyInterfaceRowBuffDurationsCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return SHOW_BUFF_DURATIONS").unwrap(), "1");
    assert!(
        (gap(&mut s) - 15.0).abs() < 1e-3,
        "the click opened the timer gutter: {}",
        gap(&mut s)
    );

    // Restart: the chunk replaces the value, and `VARIABLES_LOADED` moves the bar to match.
    let saved = String::from_utf8(s.saved_variables_bytes()).unwrap();
    assert!(
        saved.contains("SHOW_BUFF_DURATIONS = \"1\""),
        "the switch is in the saved text:\n{saved}"
    );
    let mut fresh = interface_harness();
    fresh.run(&saved).unwrap();
    assert!(
        (gap(&mut fresh) - 5.0).abs() < 1e-3,
        "the chunk moved the variable, not the bar"
    );
    fresh.fire_event("VARIABLES_LOADED", vec![]);
    assert!(
        (gap(&mut fresh) - 15.0).abs() < 1e-3,
        "the apply walk re-derives the geometry from the saved value: {}",
        gap(&mut fresh)
    );
    assert!(
        fresh.errors().is_empty(),
        "script errors: {:?}",
        fresh.errors()
    );
}

/// Every category has rows but Keybindings, which runs its own page, and each arms Defaults; a key
/// with no rows behind it leaves Defaults dead.
#[test]
fn the_defaults_button_is_armed_by_rows_not_by_a_category() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    let keys: Vec<String> = s
        .eval::<String>("return table.concat(OPTIONS_CATEGORY_KEYS, \",\")")
        .unwrap()
        .split(',')
        .map(str::to_string)
        .collect();
    assert_eq!(keys.len(), 9, "the nine 1.15.9 categories: {keys:?}");
    for key in &keys {
        let has_rows = s
            .eval::<bool>(&format!(
                "return OPTIONS_PAGE_ROWS[\"{key}\"] ~= nil and \
                 getn(OPTIONS_PAGE_ROWS[\"{key}\"]) > 0"
            ))
            .unwrap();
        assert!(
            has_rows || key == "Keybindings",
            "{key} opens onto nothing — every category leads somewhere"
        );
        s.run(&format!("BenillaOptionsFrameCategoryListRow{key}:Click()"))
            .unwrap();
        assert!(
            s.eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
                .unwrap(),
            "{key}: Defaults is live on a page that has something to reset"
        );
    }

    s.run("BenillaOptionsFrame_SelectCategory(\"NotACategory\")")
        .unwrap();
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
            .unwrap(),
        "Defaults is dead when the selected page has no rows"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `spamFilter` registers "1" and the label says Disable: 1.12 checks the box on "0" and flips the
/// write (`UIOptionsFrame.lua:253-256`, `:337-343`), so it boots unchecked.
#[test]
fn the_disable_spam_filter_row_is_inverted() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowChat:Click()")
        .unwrap();

    assert_eq!(
        s.cvar("spamFilter").as_deref(),
        Some("1"),
        "the registered default is the filter ON"
    );
    assert!(
        !s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyChatRowSpamFilterCheck:GetChecked() and true or false"
        )
        .unwrap(),
        "a filter that is ON shows *Disable Spam Filter* unchecked"
    );

    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyChatRowSpamFilterCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("spamFilter".to_string(), "0".to_string())],
        "ticking *Disable* writes the CVar OFF"
    );
    assert_eq!(s.cvar("spamFilter").as_deref(), Some("0"));

    s.run("BenillaOptionsFrameContainerBodyChatRowSpamFilterCheck:Click()")
        .unwrap();
    assert_eq!(s.cvar("spamFilter").as_deref(), Some("1"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12 files Profanity Filter in the Display box, CheckButton5 under 66
/// (`UIOptionsFrame.xml:407-409`), not with chat, so the row is on the Interface page.
#[test]
fn the_profanity_filter_row_sits_on_the_interface_page_and_writes_its_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();

    assert!(
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowProfanityFilterCheck:GetChecked() \
             and true or false"
        )
        .unwrap(),
        "registered \"1\" — a stock client boots with profanity masking on"
    );
    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowProfanityFilterCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("profanityFilter".to_string(), "0".to_string())],
        "not inverted — the label and the CVar agree"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_chat_page_toggles_the_chat_bubble_cvars() {
    benilla_formats::wow_data_or_skip!();
    // No host override: the page reads the registered pair, bubbles on and party bubbles off.
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowChat:Click()")
        .unwrap();

    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyChat:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyChatRowChatBubblesCheck:GetChecked()")
        .unwrap());
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyChatRowPartyChatBubblesCheck:GetChecked()"
        )
        .unwrap());
    let _ = s.take_sounds();

    s.run("BenillaOptionsFrameContainerBodyChatRowPartyChatBubblesCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("ChatBubblesParty".to_string(), "1".to_string())]
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOn".into())));

    // Say and yell bubbles off leaves party bubbles, which the client gates on their own CVar.
    s.run("BenillaOptionsFrameContainerBodyChatRowChatBubblesCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("ChatBubbles".to_string(), "0".to_string())]
    );
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyChatRowPartyChatBubblesCheck:GetChecked()"
        )
        .unwrap());

    // Defaults: the reference's registered pair, `ChatBubbles` "1" and `ChatBubblesParty` "0".
    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyChatRowChatBubblesCheck:GetChecked()")
        .unwrap());
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyChatRowPartyChatBubblesCheck:GetChecked()"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Remove Chat Hover Delay is a saved global whose apply moves two fade constants nothing re-reads;
/// Detailed Loot Information is a CVar the loot-roll composer reads per line, so it needs none.
#[test]
fn the_chat_page_writes_the_hover_delay_global_and_the_loot_spam_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowChat:Click()")
        .unwrap();
    let _ = s.take_sounds();
    let _ = s.take_cvar_changes();

    assert_eq!(
        s.eval::<String>("return REMOVE_CHAT_DELAY").unwrap(),
        "0",
        "ChatFrame.xml's own file-scope value, and the reference's declared default"
    );
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyChatRowRemoveChatDelayCheck:GetChecked()"
        )
        .unwrap());
    assert!(
        s.eval::<bool>("return BenillaOptionsFrameContainerBodyChatRowLootSpamCheck:GetChecked()")
            .unwrap(),
        "showLootSpam is registered \"1\" — the binary's own default"
    );

    // Its apply is the reference's `SetChatMouseOverDelay`, which zeroes both fade constants.
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.2, 0.15)
    );
    s.run("BenillaOptionsFrameContainerBodyChatRowRemoveChatDelayCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return REMOVE_CHAT_DELAY").unwrap(), "1");
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.0, 0.0),
        "the box appears the instant the cursor crosses it"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "a saved-variable row reaches the CVar table not at all"
    );

    // And back, which a one-way apply would break.
    s.run("BenillaOptionsFrameContainerBodyChatRowRemoveChatDelayCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return REMOVE_CHAT_DELAY").unwrap(), "0");
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.2, 0.15)
    );

    s.run("BenillaOptionsFrameContainerBodyChatRowLootSpamCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("showLootSpam".to_string(), "0".to_string())]
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyChatRowChatBubblesCheck:GetChecked()")
        .unwrap());

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("showLootSpam".to_string(), "1".to_string())],
        "only the row that had moved is written back"
    );
    assert_eq!(s.eval::<String>("return REMOVE_CHAT_DELAY").unwrap(), "0");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A saved "1" takes effect through the stock `VARIABLES_LOADED` arm (`UIOptionsFrame.lua:216`).
#[test]
fn a_saved_hover_delay_is_applied_when_the_variables_land() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_harness();
    // What the saved chunk does: assign the global, then the event.
    s.run("REMOVE_CHAT_DELAY = \"1\"").unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.2, 0.15),
        "the bare assignment changes nothing on its own — that is why the row has an applyFunc"
    );
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.0, 0.0)
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The row's `cvarEvent` is 1.12's third `SetCVar` argument (`UIOptionsFrame.lua:345`), handed
/// back as `CVAR_UPDATE`'s arg1, so `TextStatusBar.lua:15` shows the numerals on the click.
#[test]
fn the_status_bar_text_row_pins_the_numerals_the_moment_it_is_clicked() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    // A real span first: the bar's update hides the numerals while `valueMax` is 0.
    s.set_player_xp(1000, 10000);
    s.run("this = MainMenuExpBar; MainMenuExpBar_Update()")
        .unwrap();
    // Registered "0": the numerals show only on hover.
    assert_eq!(
        s.eval::<String>("return GetCVar(\"statusBarText\")")
            .unwrap(),
        "0"
    );
    assert!(!s
        .eval::<bool>("return MainMenuBarExpText:IsShown()")
        .unwrap());

    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyInterfaceRowStatusTextCheck:GetChecked()"
        )
        .unwrap());

    s.run("BenillaOptionsFrameContainerBodyInterfaceRowStatusTextCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("statusBarText".to_string(), "1".to_string())]
    );
    // No repaint and no XP tick: only the queued `CVAR_UPDATE`.
    s.tick(0.0);
    assert!(
        s.eval::<bool>("return MainMenuBarExpText:IsShown()")
            .unwrap(),
        "the watcher woke on the click, not on the next value change"
    );

    s.run("BenillaOptionsFrameContainerBodyInterfaceRowStatusTextCheck:Click()")
        .unwrap();
    s.tick(0.0);
    assert!(!s
        .eval::<bool>("return MainMenuBarExpText:IsShown()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12's row (`UIOptionsFrame.lua:87`): 0.5 to 1.5 by 0.05, a multiplier; 1 reads 100%.
#[test]
fn the_mouse_sensitivity_slider_snaps_to_the_reference_step() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("mousespeed", "1.25");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();

    assert!(s
        .eval::<bool>(
            "return math.abs(BenillaOptionsFrameContainerBodyControlsRowMouseSpeedControlSlider:GetValue() \
             - 1.25) < 0.0001"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMouseSpeedControlValue:GetText()"
        )
        .unwrap(),
        "125%"
    );

    s.run("BenillaOptionsFrameContainerBodyControlsRowMouseSpeedControlSlider:SetValue(1.42)")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("mousespeed".to_string(), "1.4".to_string())]
    );

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GetCVar(\"mousespeed\")").unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMouseSpeedControlValue:GetText()"
        )
        .unwrap(),
        "100%"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12 stores a factor, 1.0 to 2.0 (`UIOptionsFrame.lua:90`), over `cameraDistanceMax`'s 15 yd
/// base: the CVar carries the factor and the readout the distance.
#[test]
fn the_max_camera_distance_slider_stores_a_factor_and_reads_out_yards() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();

    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlValue:GetText()"
        )
        .unwrap(),
        "15 yd",
        "vanilla's own out-of-box ceiling"
    );

    s.run(
        "BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlSlider:SetValue(1.4)",
    )
    .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("cameraDistanceMaxFactor".to_string(), "1.4".to_string())]
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlValue:GetText()"
        )
        .unwrap(),
        "21 yd"
    );

    s.run(
        "BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlSlider:SetValue(2.0)",
    )
    .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("cameraDistanceMaxFactor".to_string(), "2".to_string())]
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlValue:GetText()"
        )
        .unwrap(),
        "30 yd"
    );

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GetCVar(\"cameraDistanceMaxFactor\")")
            .unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlValue:GetText()"
        )
        .unwrap(),
        "15 yd",
        "the readout follows the reset"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// 1.12's dropdown writes Smart 1, Always 2 and Never 0 for `cameraSmoothStyle`
/// (`UIOptionsFrame.lua:525,536,547`); the validator (`0x50c060`) also accepts 3, which the
/// terrain-tilt consumer (`0x50dbc0`) reads past its table. The entries carry those values in the
/// reference's order, a stray 3 reads Never, and the plate follows the selection.
#[test]
fn the_camera_following_style_dropdown_carries_the_engine_enum_and_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();

    // The registered default, Smart, is the reference's.
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownText:GetText()"
        )
        .unwrap(),
        "Smart"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleLabel:GetText()"
        )
        .unwrap(),
        "Camera Following Style"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyle.tip")
            .unwrap(),
        "OPTION_TOOLTIP_CAMERA1"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );

    s.run("BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownButton:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return DropDownList1.numButtons").unwrap(),
        3.0
    );
    assert_eq!(
        s.eval::<String>(
            "return DropDownList1Button1:GetText() .. \",\" .. DropDownList1Button2:GetText() \
             .. \",\" .. DropDownList1Button3:GetText()"
        )
        .unwrap(),
        "Smart,Always,Never"
    );
    assert!(s
        .eval::<bool>("return DropDownList1Button1Check:IsVisible()")
        .unwrap());

    s.run("DropDownList1Button3:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("cameraSmoothStyle".to_string(), "0".to_string())]
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownText:GetText()"
        )
        .unwrap(),
        "Never"
    );
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyle.tip")
            .unwrap(),
        "OPTION_TOOLTIP_CAMERA3",
        "the plate follows the selection, like the reference dropdown's own"
    );

    s.run("BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownButton:Click()")
        .unwrap();
    s.run("DropDownList1Button2:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("cameraSmoothStyle".to_string(), "2".to_string())]
    );

    // A stored "3" is in range but no entry's value: it shows as Never, not the nearest, Always.
    s.set_cvar_host("cameraSmoothStyle", "3");
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click(); BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownText:GetText()"
        )
        .unwrap(),
        "Never",
        "the reference dropdown's own stray value still means Never"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "displaying a stray value must not write it back"
    );

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GetCVar(\"cameraSmoothStyle\")")
            .unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownText:GetText()"
        )
        .unwrap(),
        "Smart"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The bar rows are API rows over the character's `PLAYER_FIELD_BYTES` byte 2, re-sent whole with
/// four arguments; Always Show ActionBars is a saved global, as `SetActionBarToggles` drops a
/// fifth (`0x4e770e`). The writes move the real bars in the same VM.
#[test]
fn the_action_bars_page_toggles_the_real_bars() {
    benilla_formats::wow_data_or_skip!();
    let mut s = actionbars_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowActionBars:Click()")
        .unwrap();

    let box_of = |row: &str| format!("BenillaOptionsFrameContainerBodyActionBars{row}Check");
    let checked = |s: &UiScript, row: &str| {
        s.eval::<bool>(&format!(
            "return {}:GetChecked() and true or false",
            box_of(row)
        ))
        .unwrap()
    };
    let shown =
        |s: &UiScript, bar: &str| s.eval::<bool>(&format!("return {bar}:IsShown()")).unwrap();

    for row in [
        "RowMultiBar1",
        "RowMultiBar2",
        "RowMultiBar3",
        "RowMultiBar4",
        "RowAlwaysShowMultibars",
    ] {
        assert!(!checked(&s, row), "{row} ships unticked");
    }
    for bar in [
        "MultiBarBottomLeft",
        "MultiBarBottomRight",
        "MultiBarRight",
        "MultiBarLeft",
    ] {
        assert!(!shown(&s, bar), "{bar} ships down");
    }

    // Bar 4's row is dead while bar 3's is off (`UIOptionsFrame.lua:722-726`).
    assert!(
        !s.eval::<bool>(&format!(
            "return {}:IsEnabled() ~= 0",
            box_of("RowMultiBar4")
        ))
        .unwrap(),
        "Show Right ActionBar 2 is disabled until Show Right ActionBar is on"
    );

    let _ = s.take_cvar_changes();
    s.run(&format!("{}:Click()", box_of("RowMultiBar1")))
        .unwrap();
    assert!(shown(&s, "MultiBarBottomLeft"), "the row reached the bar");
    assert_eq!(s.eval::<i64>("return SHOW_MULTI_ACTIONBAR_1").unwrap(), 1);
    assert_eq!(
        s.take_action_bar_toggle_sends(),
        vec![0x01],
        "one CMSG_SET_ACTIONBAR_TOGGLES carrying the whole byte"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "an API row must not touch the CVar table"
    );

    // The managed stack rises too: `CONTAINER_OFFSET_Y`'s base 70 plus `bottomEither` 27
    // (`UIParent.lua:1587`).
    assert_eq!(s.eval::<f64>("return CONTAINER_OFFSET_Y").unwrap(), 97.0);

    s.run(&format!("{}:Click()", box_of("RowMultiBar3")))
        .unwrap();
    assert!(
        s.eval::<bool>(&format!(
            "return {}:IsEnabled() ~= 0",
            box_of("RowMultiBar4")
        ))
        .unwrap(),
        "bar 3 on wakes bar 4's row"
    );
    assert!(shown(&s, "MultiBarRight"));
    assert!(!shown(&s, "MultiBarLeft"), "bar 4 is still off");
    s.run(&format!("{}:Click()", box_of("RowMultiBar4")))
        .unwrap();
    assert!(shown(&s, "MultiBarLeft"));
    assert_eq!(
        s.take_action_bar_toggle_sends(),
        vec![0x05, 0x0d],
        "one packet per click — bars 1+3, then 1+3+4"
    );

    // MultiBarLeft needs bar 3 as well as its own flag (`MultiActionBars.lua:73`).
    s.run(&format!("{}:Click()", box_of("RowMultiBar3")))
        .unwrap();
    assert!(!shown(&s, "MultiBarRight"));
    assert!(!shown(&s, "MultiBarLeft"), "MultiBarLeft rides on bar 3");
    assert_eq!(s.eval::<i64>("return SHOW_MULTI_ACTIONBAR_4").unwrap(), 1);
    assert!(
        !s.eval::<bool>(&format!(
            "return {}:IsEnabled() ~= 0",
            box_of("RowMultiBar4")
        ))
        .unwrap(),
        "…and its row goes back to sleep"
    );

    // The grid switch is a saved global: no packet, and its apply shows every bar's empty slots.
    let _ = s.take_action_bar_toggle_sends();
    s.run(&format!("{}:Click()", box_of("RowAlwaysShowMultibars")))
        .unwrap();
    assert_eq!(
        s.eval::<String>("return ALWAYS_SHOW_MULTIBARS").unwrap(),
        "1",
        "a uvar row stores the panel's string"
    );
    assert!(
        s.take_action_bar_toggle_sends().is_empty(),
        "and sends NOTHING — the binding has no room for a fifth argument"
    );
    assert!(
        s.eval::<bool>("return MultiBarBottomLeftButton5:IsShown()")
            .unwrap(),
        "the applyFunc opened the empty wells"
    );

    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    for bar in [
        "MultiBarBottomLeft",
        "MultiBarBottomRight",
        "MultiBarRight",
        "MultiBarLeft",
    ] {
        assert!(!shown(&s, bar), "{bar} back down");
    }
    assert_eq!(
        *s.take_action_bar_toggle_sends()
            .last()
            .expect("Defaults writes every bar row"),
        0,
        "the last packet Defaults sends is the empty byte"
    );
    assert_eq!(
        s.eval::<String>("return ALWAYS_SHOW_MULTIBARS").unwrap(),
        "0",
        "MultiBars.xml's own file-scope assignment IS the registered default"
    );
    assert_eq!(s.eval::<f64>("return CONTAINER_OFFSET_Y").unwrap(), 70.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The window tabs ─────────────────────────────────────────────────────────────

/// A fixed-width measurer whose glyph advances round to whole physical pixels, as the app's do:
/// `round(6 * scale) / scale` units a character.
struct SteppedFont(f32);

impl benilla_ui::script::TextMeasure for SteppedFont {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let per_glyph = (self.0 * req.scale).round();
        let natural = req.text.chars().count() as f32 * per_glyph / req.scale;
        (natural, 12.0, natural)
    }
}

/// The era's tab width, label + 40 (its `MinimalTab.lua:7`), fit on each show as 1.12's
/// `PanelTemplates_TabResize` is: `GetStringWidth` answers in the region's own units, and the
/// window's OnShow sets its 0.78 scale after every OnLoad has run.
#[test]
fn the_two_option_tabs_fit_their_labels_at_the_drawn_scale() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_text_measurer(Box::new(SteppedFont(6.0)));
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.resolve();

    let num = |s: &mut UiScript, expr: &str| -> f32 { s.eval(&format!("return {expr}")).unwrap() };
    assert_eq!(
        num(&mut s, "BenillaOptionsFrame:GetScale()"),
        0.78,
        "ERA_WINDOW_SCALE"
    );

    for tab in ["BenillaOptionsFrameGameTab", "BenillaOptionsFrameAddOnsTab"] {
        let w = num(&mut s, &format!("{tab}:GetWidth()"));
        let l = num(&mut s, &format!("{tab}Text:GetStringWidth()"));
        assert!(
            (w - (l + 40.0)).abs() < 0.01,
            "{tab} is {w} wide; the era's law is its label ({l}) + 40"
        );
    }
    // At 0.78 a 6-unit glyph rasters at 5 px and reads back 5/0.78 units: "Game" is 25.64 and
    // "AddOns" 38.46, where a fit at scale 1 would give tabs of 64 and 76.
    let close = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(close(
        num(&mut s, "BenillaOptionsFrameGameTab:GetWidth()"),
        65.641_03
    ));
    assert!(close(
        num(&mut s, "BenillaOptionsFrameAddOnsTab:GetWidth()"),
        78.461_54
    ));

    // Frames run afterwards change nothing.
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(close(
        num(&mut s, "BenillaOptionsFrameGameTab:GetWidth()"),
        65.641_03
    ));
    assert!(close(
        num(&mut s, "BenillaOptionsFrameAddOnsTab:GetWidth()"),
        78.461_54
    ));

    // A re-show re-fits, as the stock template does.
    s.run("HideUIPanel(BenillaOptionsFrame) ShowUIPanel(BenillaOptionsFrame)")
        .unwrap();
    s.resolve();
    assert!(close(
        num(&mut s, "BenillaOptionsFrameGameTab:GetWidth()"),
        65.641_03
    ));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// With no measurer a label measures 0 and each tab is the bare 40; the app has seated
/// `AtlasMeasurer` before any window shows.
#[test]
fn without_a_seated_measurer_the_same_fit_reads_zero() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(UiScript::new().unwrap());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.resolve();
    for tab in ["BenillaOptionsFrameGameTab", "BenillaOptionsFrameAddOnsTab"] {
        let w: f32 = s.eval(&format!("return {tab}:GetWidth()")).unwrap();
        assert!(
            (w - 40.0).abs() < 0.01,
            "{tab} is {w} wide, not the bare 40"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The reference tables' CVar census ────────────────────────────────────────────
//
// Every CVar the stock option tables name (`UIOptionsFrame.lua:4-91`) is registered, or listed
// here with the feature it waits on: `SetCVar` on a name this client never registered stores
// nothing and reads back nil, so its box would offer a setting benilla does not have. A CVar is
// registered only once something reads it.
const UNBACKED_REFERENCE_CVARS: &[(&str, &str)] = &[
    (
        "autointeract",
        "click-to-move — the one row here that is a whole movement mode rather than a knob. \
         `CanAutoInteract 0x60f900` gates the world-click pick mask's bit 0 and inverts \
         `0x5ec110`'s interaction-distance refusal, so an out-of-range corpse/GO/NPC/cast click \
         queues an approach (`0x60fed0`, move kinds 6/7/9/0xa) instead of refusing. The commit \
         itself puts NOTHING on the wire — it stamps a local goal at `0x611130` — and the packets \
         are its consequences (a stand, then the original verb on arrival). benilla has only the \
         keyboard controller and /follow (mode 3). Registered default is \"0\" on every locale but \
         koKR (`0x603374` selects on the locale index), so stock West ships it OFF",
    ),
    (
        "UnitNamePlayerPVPTitle",
        "the PvP rank prefix on the overhead name line — slot a4 of `0x608f50`, bit `0x20` of the \
         same mask, resolved by `0x609370` through the `PVP_RANK_%d_%d` GlobalStrings key. Blocked \
         one step further back than its guild twin: the rank byte streams, but the key's second \
         index is a FACTION SIDE that `ui_unit` does not resolve for an arbitrary player yet",
    ),
    // No slider CVar may land here: `UIOptionsFrame_Load` hands `GetCVar` to `Slider:SetValue`
    // (`UIOptionsFrame.lua:271-275`), which raises on nil (`0x790980`) and stops the stock
    // window's load. A check button's `GetCVar(x) == "1"` is nil-safe.
];

/// Checked both ways: an unregistered name missing from the list fails, and so does a listed name
/// that is registered now.
#[test]
fn every_cvar_the_reference_table_names_is_registered_or_listed_with_its_blocker() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    let named: Vec<String> = s
        .eval::<Vec<String>>(
            "local out = {} \
             for key, v in pairs(UIOptionsFrameCheckButtons) do \
                 if v.cvar then table.insert(out, v.cvar) end \
             end \
             for _, v in ipairs(UIOptionsFrameSliders) do \
                 if v.cvar then table.insert(out, v.cvar) end \
             end \
             table.sort(out) \
             return out",
        )
        .expect("read UIOptionsFrameCheckButtons and UIOptionsFrameSliders");
    assert!(
        named.len() >= 28,
        "the transcription lost rows: only {} cvar entries",
        named.len()
    );

    let registered: std::collections::HashSet<String> = crate::cvars::registered_pairs()
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();
    let listed: std::collections::HashSet<String> = UNBACKED_REFERENCE_CVARS
        .iter()
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();

    let mut unlisted: Vec<&str> = Vec::new();
    for cvar in &named {
        let key = cvar.to_ascii_lowercase();
        if !registered.contains(&key) && !listed.contains(&key) {
            unlisted.push(cvar);
        }
    }
    assert!(
        unlisted.is_empty(),
        "these rows name a CVar nothing registers, and nothing says why: {unlisted:?} — either \
         build the backing or add the row to UNBACKED_REFERENCE_CVARS with its blocker"
    );

    let stale: Vec<&str> = UNBACKED_REFERENCE_CVARS
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| registered.contains(&n.to_ascii_lowercase()))
        .collect();
    assert!(
        stale.is_empty(),
        "these are registered now — drop them from UNBACKED_REFERENCE_CVARS and give them a row: \
         {stale:?}"
    );

    // And a listed name the table never mentions is a row describing nothing.
    let phantom: Vec<&str> = UNBACKED_REFERENCE_CVARS
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !named.iter().any(|c| c.eq_ignore_ascii_case(n)))
        .collect();
    assert!(
        phantom.is_empty(),
        "UNBACKED_REFERENCE_CVARS names CVars the reference's table does not: {phantom:?}"
    );
}

/// Every registered CVar the reference's tables name has a row on our window, or a reason on
/// `UNREACHABLE_REFERENCE_CVARS`: the stock windows load hidden, so ours is the only way in.
#[test]
fn every_registered_reference_cvar_has_a_row_on_our_own_window() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();

    let named: Vec<String> = s
        .eval::<Vec<String>>(
            "local out = {} \
             for key, v in pairs(UIOptionsFrameCheckButtons) do \
                 if v.cvar then table.insert(out, v.cvar) end \
             end \
             for _, v in ipairs(UIOptionsFrameSliders) do \
                 if v.cvar then table.insert(out, v.cvar) end \
             end \
             table.sort(out) \
             return out",
        )
        .expect("read the reference's two option tables");

    // What our rows move: each row's `cvar`, and the `partner` a player's control writes with it.
    let ours: std::collections::HashSet<String> = s
        .eval::<Vec<String>>(
            "local out = {} \
             for page, rows in pairs(OPTIONS_PAGE_ROWS) do \
                 for _, rkey in ipairs(rows) do \
                     local row = getglobal(\"BenillaOptionsFrameContainerBody\" .. page .. rkey) \
                     if row.cvar then table.insert(out, row.cvar) end \
                     if row.partner then table.insert(out, row.partner.cvar) end \
                 end \
             end \
             return out",
        )
        .expect("read our own window's rows")
        .into_iter()
        .map(|c| c.to_ascii_lowercase())
        .collect();

    let registered: std::collections::HashSet<String> = crate::cvars::registered_pairs()
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();
    let excused: std::collections::HashSet<String> = UNREACHABLE_REFERENCE_CVARS
        .iter()
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();

    // Floors: unloaded tables or an unbuilt window would read nothing and pass.
    let backed = named
        .iter()
        .filter(|c| registered.contains(&c.to_ascii_lowercase()))
        .count();
    assert!(
        backed >= 28,
        "the reference's tables did not load: only {backed} of their names are registered here"
    );
    assert!(
        ours.len() >= 40,
        "our own window's rows did not build: only {} carry a cvar",
        ours.len()
    );

    let missing: Vec<&str> = named
        .iter()
        .filter(|c| {
            let k = c.to_ascii_lowercase();
            registered.contains(&k) && !ours.contains(&k) && !excused.contains(&k)
        })
        .map(String::as_str)
        .collect();
    assert!(
        missing.is_empty(),
        "these are backed and the reference lets a player change them, and our own options \
         window has no row for them: {missing:?} — seat a row, or add them to \
         UNREACHABLE_REFERENCE_CVARS with the reason"
    );

    let stale: Vec<&str> = UNREACHABLE_REFERENCE_CVARS
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| ours.contains(&n.to_ascii_lowercase()))
        .collect();
    assert!(
        stale.is_empty(),
        "these have a row now — drop them from UNREACHABLE_REFERENCE_CVARS: {stale:?}"
    );
}

/// Registered reference-table CVars with no row on our window, each with what the player cannot
/// do and why; empty, as every one has a row.
const UNREACHABLE_REFERENCE_CVARS: &[(&str, &str)] = &[];

#[test]
fn the_four_camera_toggles_read_their_shipped_defaults_and_write_on_the_click() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the four on open must not write them back"
    );

    let checked = |s: &mut UiScript, row: &str| -> bool {
        s.eval::<bool>(&format!(
            "return BenillaOptionsFrameContainerBodyControls{row}Check:GetChecked()"
        ))
        .unwrap()
    };
    // The reference's registered values: pivot and water collision on, tilt and head bob off.
    assert!(checked(&mut s, "RowSmartPivot"));
    assert!(checked(&mut s, "RowWaterCollision"));
    assert!(!checked(&mut s, "RowFollowTerrain"));
    assert!(!checked(&mut s, "RowHeadBob"));

    for (row, cvar, want) in [
        ("RowFollowTerrain", "cameraTerrainTilt", "1"),
        ("RowHeadBob", "cameraBobbing", "1"),
        ("RowWaterCollision", "cameraWaterCollision", "0"),
        ("RowSmartPivot", "cameraPivot", "0"),
    ] {
        s.run(&format!(
            "BenillaOptionsFrameContainerBodyControls{row}Check:Click()"
        ))
        .unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![(cvar.to_string(), want.to_string())],
            "{row} writes {cvar}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Two rows also write a CVar neither table names, as `UIOptionsFrame_Save`'s special arms do:
/// `PetSpellDamage` (`UIOptionsFrame.lua:334-336`) and `cameraPitchMoveSpeed` (`:355-356`).
#[test]
fn the_pet_damage_box_and_the_look_slider_each_write_their_unnamed_twin() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();

    s.run("BenillaOptionsFrameContainerBodyCombatRowPetDamageCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![
            ("PetMeleeDamage".to_string(), "0".to_string()),
            ("PetSpellDamage".to_string(), "0".to_string()),
        ],
        "one box, both pet knobs"
    );

    // The pitch twin at half the yaw, the ratio of their registered 180 and 90.
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyControlsRowMouseLookSpeedControlSlider:SetValue(240)")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![
            ("cameraYawMoveSpeed".to_string(), "240".to_string()),
            ("cameraPitchMoveSpeed".to_string(), "120".to_string()),
        ]
    );
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMouseLookSpeedControlValue:GetText()"
        )
        .unwrap(),
        "240",
        "a deg/s slider reads out the raw number, not a percent of nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Two separate masters, as the reference keeps them: CheckButton19 greys 9 and 11, and 52 greys
/// twelve boxes from 53 to 69 and the dropdown (`UIOptionsFrame.lua:710-716`, `:728-756`).
#[test]
fn show_target_damage_greys_its_own_pair_and_the_floating_text_master_leaves_it_alone() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    let enabled = |s: &mut UiScript, row: &str| -> bool {
        s.eval::<bool>(&format!(
            "return BenillaOptionsFrameContainerBodyCombat{row}Check:IsEnabled() ~= 0"
        ))
        .unwrap()
    };

    // `SHOW_COMBAT_TEXT` ships "0": the floating-text rows arrive greyed, the damage trio live.
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "0");
    assert!(!enabled(&mut s, "RowAuras"));
    assert!(enabled(&mut s, "RowShowDamage"));
    assert!(enabled(&mut s, "RowPeriodicDamage"));
    assert!(enabled(&mut s, "RowPetDamage"));

    s.run("BenillaOptionsFrameContainerBodyCombatRowShowDamageCheck:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GetCVar(\"CombatDamage\")")
            .unwrap(),
        "0"
    );
    assert!(!enabled(&mut s, "RowPeriodicDamage"));
    assert!(!enabled(&mut s, "RowPetDamage"));
    assert!(
        enabled(&mut s, "RowShowDamage"),
        "the master is the way back in"
    );

    // Waking the floating-text family does not wake them: the two gates are independent.
    s.run("BenillaOptionsFrameContainerBodyCombatRowCombatTextCheck:Click()")
        .unwrap();
    assert!(enabled(&mut s, "RowAuras"));
    assert!(
        !enabled(&mut s, "RowPetDamage"),
        "scrolling combat text does not own the damage numbers"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference's rules: Player Guild Names is dead while player names are off
/// (`UIOptionsFrame.lua:703-709`), Auto-Follow Speed while the style is Never (`:717-721`).
#[test]
fn the_guild_line_greys_with_player_names_and_the_follow_speed_with_the_style() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    // Nameplates: `UnitNamePlayer` ships "1", so the child arrives live.
    s.run("BenillaOptionsFrameCategoryListRowNameplates:Click()")
        .unwrap();
    let guild_on = |s: &mut UiScript| -> bool {
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyNameplatesRowGuildNamesCheck:IsEnabled() ~= 0",
        )
        .unwrap()
    };
    assert!(guild_on(&mut s));
    s.run("BenillaOptionsFrameContainerBodyNameplatesRowPlayerNamesCheck:Click()")
        .unwrap();
    assert!(!guild_on(&mut s), "no names, no guild line to gate");
    s.run("BenillaOptionsFrameContainerBodyNameplatesRowPlayerNamesCheck:Click()")
        .unwrap();
    assert!(guild_on(&mut s));

    // Controls: the style ships "1" (Smart), so the slider arrives live; "0" is Never.
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();
    let speed_thumb = |s: &mut UiScript| -> bool {
        s.eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowAutoFollowSpeedControlSliderThumb\
             :IsShown()",
        )
        .unwrap()
    };
    assert!(speed_thumb(&mut s));
    s.run("SetCVar(\"cameraSmoothStyle\", \"0\") OptionsPage_Refresh(\"Controls\")")
        .unwrap();
    assert!(
        !speed_thumb(&mut s),
        "a Never follow leaves the speed groove thumbless, 1.12's own way of saying dead"
    );
    // The groove also takes no press: ours turns its mouse off, where the stock disable only
    // hides the thumb (`OptionsFrame.lua:481-487`).
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyControlsRowAutoFollowSpeedControlSlider\
             :IsMouseEnabled()"
        )
        .unwrap());
    s.run("SetCVar(\"cameraSmoothStyle\", \"1\") OptionsPage_Refresh(\"Controls\")")
        .unwrap();
    assert!(speed_thumb(&mut s));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
