//! The shipped `assets/ui/OptionsFrame.xml` — the era-shaped, 1.12-skinned options window
//! (0950: the shell; 0957: the Audio page; 0959: the Graphics page; 0978: the 1.12-native
//! skin — no era extraction, every texture from the MPQ chain; 0981: the 1.14 System-window
//! dialog chrome — translucent dark ground, outline boxes, hairline dividers; 0984: the 1.14
//! select/hover wash mechanism, the working era search; 0985: the provenance split those
//! cite; 0989: the directed cuts — steppers and the corner X gone, the whole bar live via
//! the engine's track-press law, the search box at the era's verbatim seat; 0992: the
//! dropdown row shape on the 1.12 kit — Camera Following Style — and the Nameplates page's
//! three UnitName* rows; 1476: the ground dim — a black fill over the 0.6 tile, seated clear
//! of the rope's ink, because a 60% veil is not a page you can read over bright terrain).
//!
//! What these guard: the file loads clean inside the real neighbourhood (Fonts + UiPanels +
//! GameMenuFrame); the menu's Options button is the door in (menu down, options up, on the ref's
//! own kit); Controls is the default category and the page title follows the selection; both
//! close spellings put the window away; the selected row wears the LOCKED GOLD additive wash
//! and hover runs the steel-blue one (the 1.14 pair); and the search reflows the live rows
//! under category heads and restores the authored page exactly.
//!
//! The page tests (0957 Audio, 0959 Graphics) run against the REAL registered CVar set
//! (`crate::cvars`): rows read the table on select, writes land on the change queue the host
//! drains, the snap grids and readouts hold (the era 5% volumes; the 1.12 uiscale 0.01 and
//! farclip min-anchored 60), the 1.12 master→ambience dependency greys, and Defaults walks the
//! visible page back to the registered defaults.

use benilla_ui::script::{QuadContent, SoundRequest, UiScript, WornDisplay};

/// The window's real neighbourhood, in the manifest's own order (options before the menu — the
/// game_menu_tests::harness_with idiom, minus the extras this file never needs).
fn harness() -> UiScript {
    harness_on(UiScript::new().unwrap())
}

/// Load the manifest slice onto a prepared script — split out so page tests can seed CVars
/// BEFORE the XML loads, the way the app does (ui_script::setup_script). The dropdown kit
/// rides along since 0992 (the dropdown rows inherit its capsule template), and GameTooltip
/// before it for the kit's TOOLTIP_DEFAULT_COLOR — the app's own order.
fn harness_on(mut s: UiScript) -> UiScript {
    s.set_screen_size(1024.0, 768.0);
    // A file loads ONCE, as in the client: the definer harnesses below load some of these ahead
    // of this list to capture file-scope values, and a second load of the stock money kit's
    // frames trips its own global-named update (`MoneyFrame_Update` writes the amount onto
    // `getglobal(name)` — the first instance — while the second instance reads its own).
    let loaded = |s: &UiScript, file: &str| -> bool {
        s.eval::<bool>(&format!(
            "return (BENILLA_TEST_LOADED or {{}})[{file:?}] == true"
        ))
        .unwrap_or(false)
    };
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        // Every panel window declares `parent="UIParent"`, resolved at LOAD — so UIParent has to
        // exist by the time they are read, exactly as it does in the manifest (decision 1734).
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
        // The reference's own Interface Options window, hidden — this kit's source of truth for
        // `UIOptionsFrameCheckButtons`/`UIOptionsFrameSliders`, which our own file used to
        // transcribe and 2115 deleted. Its widget kit and the video window's shared helpers come
        // with it, in the manifest's own order.
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\OptionsFrame.lua",
        r"Interface\FrameXML\UIOptionsFrame.xml",
        "ScrollTemplates.xml", // the Keybindings page's faux-scroll kit
        "KeyBindingsPage.xml", // the Keybindings body's templates + script (1008)
        "OptionsFrame.xml",
        "GameMenuFrame.xml",
    ] {
        // `test_ui::load_ui`, not a local read: a manifest entry carrying a path separator is the
        // REFERENCE's own file and must come off the player's chain, which
        // `std::fs::read_to_string` under `assets/ui` cannot do — it goes looking for
        // `assets/ui/Interface/FrameXML/...` and fails. The shared loader resolves both shapes, and
        // its own doc already records this consolidation happening once before. Hand-rolling it
        // here is what made this kit break the moment a file it loads migrated (1751).
        // The dialect announces DROPPED subtrees as warnings, not errors — for the new file,
        // a warning is a silently-missing piece of chrome, so it fails there.
        if loaded(&s, file) {
            continue;
        }
        if file == "OptionsFrame.xml" {
            super::test_ui::load_ui_strict(&s, file);
        } else {
            super::test_ui::load_ui(&s, file);
        }
    }
    // The app's own post-load pass, at the app's own moment: everything is loaded, so
    // `SHOW_BUFF_DURATIONS` (declared by OptionsFrame.xml, just above) is real by the time the
    // buff bar's row pitch is settled from it. `BuffFrame_OnLoad` does not do this and 1.12 leaves
    // it to UIOptionsFrame.lua, which we have no counterpart to (`manifest::apply_buff_durations`).
    super::manifest::apply_buff_durations(&s).unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The door in: the game menu's Options button (its ref OnClick + the explicit hide) swaps the
/// two native-center frames — menu down, options up holding the center slot — on the ref's own
/// igMainMenuOption kit.
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

/// Controls is the default page (the OnShow seat), and the page title is the selected row's own
/// label — including the one key whose label differs from it (ActionBars → "Action Bars").
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

/// Clicking a category row moves the selection and the page title with it — and the selection
/// survives a close/reopen (the OnShow re-applies the last seat, not the default).
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

    // Close and reopen: still Graphics, not Controls.
    s.run("HideUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    assert_eq!(
        s.eval::<String>("return BenillaOptionsFrameContainerTitle:GetText()")
            .unwrap(),
        "Graphics"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The red Close hides the window on the igMainMenuClose kit, and the corner X does not EXIST
/// (0989's directed cut).
///
/// This test used to ride a rowless page here to watch Defaults stay disabled, and the stand-in
/// kept moving as the arc filled pages in — Controls until 0961, Interface until 1136, Social
/// (now Chat) until 1139. **There is no rowless category left**, which is the milestone rather than a gap;
/// the guard itself is pinned by `the_defaults_button_is_armed_by_rows_not_by_a_category` below.
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

    // No corner X (0989's directed cut — the era HAS one; the red button and ESC are the
    // window's exits).
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameClosePanelButton == nil")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The row wash (0984: the 1.14 OptionsListButtonTemplate mechanism, verbatim — ONE
/// UI-QuestLogTitleHighlight quad in ADD blend, two tints): the selected row's wash draws
/// additive in the LOCKED GOLD (1,1,0) at the era plate seat (187x21 — sized once in
/// OptionsCategoryRow_OnLoad); a moved selection reseats the single gold quad lower; and
/// hovering an UNSELECTED row lights the same texture in the steel-blue hover tint
/// (.196,.388,.8) while the locked gold stands — 1.14's LockHighlight guard.
#[test]
fn the_selected_row_wears_the_gold_wash_and_hover_runs_blue() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // Pin the window at scale 1 so row rects and the pointer share coordinates (the fit
    // clamp stays out of the way at 1024x768: both ratios sit above 1).
    s.run("ERA_WINDOW_SCALE = 1").unwrap();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    // The plate seat: the era's 187x21, authored in the row OnLoad now that no
    // SetAtlas(name, true) sizes it.
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

    // Every visible category wash: rect + tint + blend.
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

    // Hover the now-unselected Controls row: its wash lights BLUE beside Audio's gold.
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

    // Leaving puts the hover wash away; the lock stays.
    s.mouse_move(5.0, 5.0);
    let left = washes(&mut s);
    assert_eq!(left.len(), 1);
    assert!(gold(&left[0].1));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The GROUND DIM (1476) — the plain black fill that turns 0.6 of veil into a page you can
/// read. Two laws here, both silently breakable by hand, and neither about the taste number:
/// the fill must draw **above** the backdrop's tiled ground (a region sorts after its frame's
/// own draw slot, which is the whole reason a region *can* dim it), and it must sit clear of
/// the rope's **ink** — the border's four edge slices are dead from texel 16 of 32 and the
/// corners' ink ends by 14 (decoded from `UI-DialogBox-Border`), so anything inset 14 or more
/// darkens the page without touching the frame. At the backdrop's own 11/12/12/11 bg insets it
/// would smear across the rope's inner half, which is exactly the mistake this pins.
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

    // Draw order: `extract` returns ascending z, so a later index draws later. The dim has to
    // land after the tiled ground or it dims nothing.
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

/// Search (0984: the era SettingsPanel mechanism transcribed): typing reflows the LIVE
/// matching rows under a clickable category head — a matched CHILD pulls its parent volume
/// row in above it (the era parentInitializer rule) — the title reads "Search Results" and
/// Defaults hides; clearing the box lands every row back on its authored XML chain exactly
/// (the one-layout-law pin) with the page view restored.
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
    // The search reflows off the box's `OnTextChanged`, which is deferred to the drain (1831).
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
    // One head — Audio's; the matches plus their pulled-in parent; nothing else.
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
    // The chain reads head → parent → the two matches, downward.
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
    // The reflowed row is LIVE: moving the music slider writes its CVar mid-search.
    let _ = s.take_cvar_changes();
    s.run("BenillaOptionsFrameContainerBodyAudioRowMusicControlSlider:SetValue(0.25)")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("MusicVolume".to_string(), "0.25".to_string())]
    );

    // Clearing restores the page view and the authored chain EXACTLY.
    s.run("BenillaOptionsFrameSearchBox:SetText(\"\")").unwrap();
    s.tick(0.0); // the clear reflows on the drain, like the search itself (1831)
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

/// The era's word scoring (Blizzard_Settings.lua MatchesSearchTags + the words list): the
/// WHOLE query is the first word, so a phrase match outscores its own tokens and chains
/// first within the group — "master volume" seats Master above the token-matched children.
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

/// The era's search exits: clicking a results head (the era's jump — SelectCategory clears
/// the box) and a no-match query (the section-header "no results" line, nothing chained).
#[test]
fn a_head_click_ends_the_search_and_a_miss_shows_no_results() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();

    s.run("BenillaOptionsFrameSearchBox:SetText(\"volume\")")
        .unwrap();
    s.tick(0.0); // the head being clicked below is built by the search reflow, so drain first (1831)
    s.run("BenillaOptionsFrameContainerBodySearchHeadAudio:Click()")
        .unwrap();
    // The click clears the box synchronously; restoring the page rides that clear's
    // `OnTextChanged`, which the drain owes (1831).
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
    s.tick(0.0); // the miss renders on the drain too (1831)
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
    s.tick(0.0); // and so does clearing it (1831)
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyNoResults:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The bar IS the control (0989: steppers cut by the director's call; the engine's
/// track-press law): a LeftButton press on the slider's track — off the thumb — seats the
/// thumb under the cursor (the value jumps, the CVar write queues), the SAME press keeps
/// dragging (0250 §5's capture began), and the stepper buttons no longer exist.
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

    // The steppers are gone.
    for btn in ["Back", "Forward"] {
        assert!(s
            .eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyAudioRowMasterControl{btn} == nil"
            ))
            .unwrap());
    }

    let slider = "BenillaOptionsFrameContainerBodyAudioRowMasterControlSlider";
    s.resolve(); // seat the rects before reading them (the wash test's idiom)
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
    // Press 1 unit inside the RIGHT end of the track — far from the 0.1-seated thumb. The
    // center-grab seat clamps the fraction to 1.0 there (the cursor sits inside the thumb's
    // half-width end zone), so the value lands exactly at the max.
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
    // Still held: the same gesture drags on. The slider's midpoint is exactly fraction 0.5
    // ((mid − thumb/2 − left) / (width − thumb)), on the 0.05 grid, so no snap correction.
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

/// **A slider row's readout sits on its label's line.** The row shows one setting as two
/// `GameFontNormal` strings — the name at the left, the value at the right — and a reader takes
/// them as a single line; ours were **3 units apart** at every UI scale (≈4 px on a maximised
/// 1440p window, which is where B232 photographed them).
///
/// The cause is a nudge that leaked from the art onto the text: the readout hangs off
/// `$parentControl`, and the control was seated at the row's `CENTER(-80, +3)` — the +3 lifting
/// the 17-tall groove and its 32 px thumb inside the 26-tall row. Anything anchored to the
/// control inherited a seat that exists for a texture. The nudge now lives on the two art frames
/// it was for (`$parentGroove`, `$parentSlider`), so the bar does not move by a pixel and the
/// number falls onto the label's line — asserted here on both halves, over every slider row the
/// window has: a label/value line, and the groove still riding 3 units high of the row.
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
            // The two strings are the assertion: same font, same row, one line. (Both are
            // regions, so both rects arrive in the same space — the comparison needs no scale.)
            let (label, value) = (
                mid(&format!("{base}Label")),
                mid(&format!("{base}ControlValue")),
            );
            assert!(
                (label - value).abs() < 0.01,
                "{base}: the readout sits {:.2} off its label's line",
                value - label
            );
            // …and the art it used to ride is exactly where it was: 3 units high of the row.
            assert!(
                (mid(&format!("{base}ControlGroove")) - mid(&base) - 3.0).abs() < 0.01,
                "{base}: the groove left its seat"
            );
        }
    }
}

/// The Audio harness: the real registered CVar set on the table before the XML loads, exactly
/// the app's boot order (register → seed → load → select).
fn audio_harness() -> UiScript {
    let s = UiScript::new().unwrap();
    s.register_cvars(crate::cvars::registered_pairs());
    s
}

/// The Combat page's harness (decision 1134, 1964): the family's defaults are the window's own
/// file-scope block (the reference's UIOptionsFrame.lua l.135-152), and the addon they drive is
/// `Blizzard_CombatText` off the chain, loaded on demand by the master row's apply.
fn combat_harness() -> UiScript {
    let mut s = audio_harness();
    s.set_screen_size(1024.0, 768.0);
    load_definers(&s, &[r"Interface\FrameXML\UIParent.xml"]);
    // The family's definers are the options window's own since 1964 (the reference's
    // UIOptionsFrame.lua block); the addon itself is LoadOnDemand off the chain, seated so the
    // master row's apply can load it the way the client does.
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatText");
    harness_on(s)
}

/// The files a saved-variable page's DEFINERS live in, loaded ahead of the window (see
/// `combat_harness`). Every page over the second store needs this: a row captures its default from
/// the global's own file-scope assignment at OnLoad, so that file has to have run first — which is
/// exactly the ordering the real manifest guarantees.
fn load_definers(s: &UiScript, files: &[&str]) {
    for file in files {
        super::test_ui::load_ui(s, file);
        // Remembered so `harness_on` loads each file once (see there).
        s.run(&format!(
            "BENILLA_TEST_LOADED = BENILLA_TEST_LOADED or {{}} BENILLA_TEST_LOADED[{file:?}] = true"
        ))
        .unwrap();
    }
}

/// The Interface page's harness (decision 1136), the same posture as `combat_harness` above: the
/// **real** definers ahead of the window, in the manifest's own order, so each row captures the
/// same file-scope value it captures in the client. `SHOW_NEWBIE_TIPS` is set by our own
/// `assets/ui/OptionsFrame.xml` (it lived in our `GameTooltip.xml` until the tooltip went stock,
/// 1968; the reference keeps it in `UIOptionsFrame.lua`), which `harness_on` already loads; the two
/// quest globals need their own windows — `MerchantFrame.xml` for the coin helpers both quest files
/// reuse and `ScrollTemplates.xml` for the kit `QuestFrame.xml` inherits from (the same chain
/// `quest_tests`/`questlog_tests` load). `SHOW_BUFF_DURATIONS` arrives with the bar it re-anchors
/// (1139), behind the two files `buff_tests` loads ahead of it — `Cooldown.xml` (every button's
/// child) and `ActionBar.xml` (`BENILLA_FALLBACK_ICON`), themselves behind `UIParent.xml`.
/// `TextStatusBar.xml` rides in ahead of them for the Status Bar Text row's consumer — the XP bar's numerals (1140).
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
            // The target-of-target pair's definer (1576) and the three files ahead of it, all in
            // their manifest seats. The chain is a real load-ORDER requirement rather than
            // tidiness: `UnitFrames`' three menu hosts initialize into the dropdown kit at load
            // (`UIDropDownMenu_Initialize` is nil without it and the OnLoad raises), and the kit's
            // own backdrop reads `GameTooltip`'s TOOLTIP_DEFAULT_COLOR. `harness_on` loads two of
            // these again after these — re-running a UI file is what `/reload` does, and the
            // loader takes it.
            "Interface\\FrameXML\\GameTooltip.xml",
            "Interface\\FrameXML\\UIDropDownMenu.xml",
            "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
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
            // `PartyMemberBackground`'s OnEvent sets `OpacityFrameSlider` on VARIABLES_LOADED,
            // and that slider is declared in ColorPickerFrame.xml. The reference loads them in
            // this same order (its toc: PartyFrame 45, ColorPickerFrame 84) — it works there
            // because the reader is an EVENT handler, not a load-time one, so by the time
            // VARIABLES_LOADED fires the slider exists. A test that fires the event has to have
            // loaded it too.
            "Interface\\FrameXML\\ColorPickerFrame.xml",
            "Interface\\FrameXML\\Cooldown.xml",
            "Interface\\FrameXML\\ActionButtonTemplate.xml",
            "Interface\\FrameXML\\MainMenuBar.xml",
            "Interface\\FrameXML\\ActionBarFrame.xml",
            "Interface\\FrameXML\\BonusActionBarFrame.xml",
            // The reference declares the reputation WATCH BAR in `ReputationFrame.xml`, and
            // `ExhaustionTick_Update` reads `ReputationWatchBar:IsShown()` twice — the reference's own
            // coupling of MainMenuBar to that pane. So an action-bar harness loads it, and with it the
            // two template files its check boxes inherit through (1875).
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
            // The Show Tutorials row's setter is the reference's own Save arm, which reaches
            // `TutorialFrameCheckButton` and `TutorialFrame_HideAllAlerts` UNGUARDED (2077) — so
            // the page's harness loads the window that declares them, as the client does.
            "Interface\\FrameXML\\TutorialFrame.xml",
        ],
    );
    harness_on(s)
}

/// The Chat page's harness (decision 1589), the same posture as `combat_harness`: the **real**
/// `ChatFrame.xml` ahead of the window, so the *Remove Chat Hover Delay* row captures the same
/// file-scope `REMOVE_CHAT_DELAY = "0"` and the same `ChatFrame_ApplyMouseOverDelay` it captures in
/// the client. Its own chain is the manifest's: `UIParent.xml` for the managed bottom stack the
/// dock sits in, and `GameTooltip.xml` + `UIDropDownMenu.xml` because the tabs' options menus are
/// dropdown capsules and a tab click reaches `CloseDropDownMenus`.
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
            "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
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

/// The Action Bars page's harness (1136's lock row, 1500's five switches): `ActionBar.xml` declares
/// `LOCK_ACTIONBAR` and `MultiBars.xml` the four `SHOW_MULTI_ACTIONBAR_*` globals plus
/// `ALWAYS_SHOW_MULTIBARS` (whose file-scope "0" IS the row's registered default), and both need
/// `UIParent.xml` (the managed bottom stack the bars move) and `Cooldown.xml`
/// (`CooldownFrame_SetTimer`, every button's child) ahead of them — the manifest's own order.
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
            // The reference declares the reputation WATCH BAR in `ReputationFrame.xml`, and
            // `ExhaustionTick_Update` reads `ReputationWatchBar:IsShown()` twice — the reference's own
            // coupling of MainMenuBar to that pane. So an action-bar harness loads it, and with it the
            // two template files its check boxes inherit through (1875).
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
            // The reference's own Interface Options window BEFORE ours, as the manifest has it
            // (2115) — `UIOptionsFrame_Init` is what assigns `LOCK_ACTIONBAR` and
            // `ALWAYS_SHOW_MULTIBARS`, and this page's rows capture those at their own OnLoad as
            // the Defaults value. It is also the home of `UIOptionsFrameCheckButtons`, which
            // `MultiActionBars.xml` below writes into at its load.
            r"Interface\FrameXML\OptionsFrame.lua",
            r"Interface\FrameXML\UIOptionsFrame.xml",
            "OptionsFrame.xml",
            "Interface\\FrameXML\\MultiActionBars.xml",
        ],
    );
    harness_on(s)
}

/// Selecting Audio shows the page body, arms Defaults, and every row reads the CVar table:
/// sliders take the stored value with the era rounded-percent readout, checkboxes take the flag.
/// Leaving the page hides it and puts Defaults back to sleep.
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
    // The music slider holds the stored 0.7 (f32 wobble tolerated), readout "70%".
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
    // Checkboxes: EnableMusic off, the master (default "1") on.
    assert!(!s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowEnableMusicCheck:GetChecked()"
        )
        .unwrap());
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyAudioRowEnableAllCheck:GetChecked()")
        .unwrap());

    // Off to another page: the Audio body goes away with the selection (the swap is what the
    // page loop does, and it is the reason a stale row can never be read from the wrong page).
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

/// A user move snaps to the era 5% grid (obeyStepOnDrag transcribed) and writes the CVar as a
/// clean short string — the change queue carries what config.toml will store. A refresh write
/// (the page reading the table) queues nothing.
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

    // An off-grid move (what a drag delivers) snaps to 0.45 and queues exactly that.
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

/// The checkbox rows write their flag CVar on the 1.12 panel's own (quirky) click kits, and the
/// 1.12 dependency holds: Enable All Sound off greys exactly the Enable Ambience row.
#[test]
fn the_checkbox_rows_write_flags_and_the_master_greys_ambience() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();
    let _ = s.take_sounds();

    // Uncheck the master: flag "0" queued, ambience greyed, music left alive (the 1.12 quirk),
    // and the just-UNchecked box plays the CheckBoxOn kit (SoundOptionsFrame.lua verbatim).
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

    // Re-check: flag "1", ambience live again.
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

/// The background-sound row (decision 1847): the one Audio row 1.12 has no checkbox for. It
/// boots UNCHECKED — the reference's own behaviour, which is to go quiet in the background — and
/// its click writes the era CVar. It rides OUTSIDE the master's dependency rule, which names
/// exactly two rows in `SoundOptionsFrame_UpdateDependencies` and never grew a third.
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

    // The master's rule greys Ambience and Error Speech and nothing else — this row stays live,
    // because "should the game be audible at all" is not the question it answers.
    s.run("BenillaOptionsFrameContainerBodyAudioRowEnableAllCheck:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>(
            "return BenillaOptionsFrameContainerBodyAudioRowBackgroundSoundCheck:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Defaults walks every Audio row's CVar back to its registered default and the rows follow —
/// the era per-page reset, on the one page with rows.
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
    // Only the MOVED values queue — the rows already at default write nothing.
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

/// Selecting Graphics shows ITS page body (0959) with both 1.12 sliders reading the table:
/// uiScale on the 0.64..1.0 panel range with the percent readout, farclip (Terrain Distance —
/// retired 0961, back 1513) on 177..777 with the raw-yards readout. The swap works both ways —
/// Audio's body takes over when clicked.
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

    // The swap, the other way: Audio in, Graphics out.
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

/// The **Display Mode** row (1650) — modern Classic's own control, which 1627 had already given us
/// the two states for. Blizzard's `Blizzard_SettingsDefinitions_Shared/Graphics.lua` (identical on
/// live, classic and classic_era) builds a boolean proxy as a dropdown with exactly two entries,
/// `VIDEO_OPTIONS_WINDOWED_FULLSCREEN` and `VIDEO_OPTIONS_WINDOWED`, and no Fullscreen entry at all.
///
/// **The polarity is the thing worth pinning.** `gxWindow` is 1.12's CVar and keeps 1.12's sense —
/// `"1"` is WINDOWED — so the FIRST entry, the borderless one this client defaults to, is the CVar's
/// `"0"`. A row whose label and value run opposite ways is exactly the kind that gets silently
/// inverted by a later edit, and until this test there was no coverage of the row at all.
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
    // "0" is NOT windowed, which 1627 redefined as the borderless fullscreen window.
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}DropdownText:GetText()"))
            .unwrap(),
        "Windowed (Fullscreen)"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the table on select must not write it back"
    );

    // Two entries, and only two — the modern list has no Fullscreen row, because 8.0.1 removed
    // the exclusive mode those clients had and 1627 ships none for its own platform reasons.
    s.run(&format!("{ROW}DropdownButton:Click()")).unwrap();
    assert!(s.eval::<bool>("return DropDownList1:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<f64>("return DropDownList1.numButtons").unwrap(),
        2.0
    );
    assert!(s
        .eval::<bool>("return DropDownList1Button1Check:IsVisible()")
        .unwrap());

    // Picking Windowed writes the CVar's "1", closes the list, and repaints the capsule.
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

/// **Vertical Sync** (1394) — the Graphics page's third row, and the first option in this window
/// that reaches the *window* rather than a gameplay or UI knob. It is 1.12's own Video Options
/// checkbox 5 (`gxVSync`), which lived on the perf HUD as a dev checkbox until it turned out to be
/// unreachable in a player build (the HUD is `#[cfg(feature = "dev")]`) and to be a setting rather
/// than an instrument in the first place.
///
/// The row is deliberately **not** deferred, unlike the UI Scale slider above it: the reference's
/// row carries `gxRestart = 1` because its device could not swap the presentation interval live,
/// and wgpu can — so the click commits, like every other checkbox here.
#[test]
fn the_vertical_sync_row_reads_and_writes_the_present_mode_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("gxVSync", "0");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();

    // Read from the table, not from a restated default: the harness seeded it off.
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

    // A click commits immediately — nothing stages it behind Apply.
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowVerticalSyncCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("gxVSync".to_string(), "1".to_string())],
        "the checkbox writes the cvar on click, not on Apply"
    );

    // Defaults walks it back to the registered "1" — which `cvars::tests` welds to
    // `VideoConfig::default()`, and `video::tests` welds in turn to the window's boot mode.
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

/// Environment Detail is the reference's own slider (1649): 0..2 step 1 over `WorldDetail`, with
/// 0992's Low/Medium/High names kept in the readout seat — a groove whose readout says "1" tells a
/// player nothing. Dragging writes the CVar; a value from outside the range shows the nearest stop
/// **The Brightness slider actually writes** (decision 2182, and the bug that shipped with it).
///
/// This row is the kit's first slider whose store is an engine PAIR rather than a CVar or a
/// saved-variable global — 1.12's own arrangement for its slider 6 — and
/// `OptionsSlider_OnValueChanged` guarded its write on `row.cvar or row.uvar`, two of the kit's
/// three stores. So dragging it moved the READOUT and wrote nothing: the control looked alive, the
/// picture never changed, and reopening the window re-read an untouched CVar and snapped back to
/// the centre. No error anywhere, on any path.
///
/// The shipped test missed it by calling `SetGamma` directly and checking the row's *wiring* — the
/// bounds, the numeric flag, the readout at the default. Everything it asserted was true. What it
/// never did was move the slider, which is the only thing a player does. So this one drags.
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
    // The registered `gamma = "1.0"` is `GetGamma() == 0`, the centre of the stock slider's
    // travel — and selecting the page must not write it back.
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "50%"
    );
    assert!(
        s.take_cvar_changes().is_empty(),
        "reading the pair on select must not write it back"
    );

    // **A drag to each end WRITES.** The value is the reference's own `1 - slider`, six decimals,
    // because `SetGamma` owns the formatting — this is the assertion the shipped test lacked.
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

    // **And it comes back.** Leaving the page and returning re-reads the pair rather than the
    // control, which is the half the player sees as "it didn't save": the row was left at 0.2.
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

/// and writes nothing back (0959's out-of-range law).
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

    // The reference's grid, verbatim: three stops, one apart (OptionsFrame.lua l.27).
    assert!(s
        .eval::<bool>(&format!(
            "local lo, hi = {ROW}ControlSlider:GetMinMaxValues() \
             return lo == 0 and hi == 2 and {ROW}ControlSlider:GetValueStep() == 1"
        ))
        .unwrap());

    // A drag to the top stop: the write queues and the readout names it.
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

    // Off-grid input snaps to the stop grid (era obeyStepOnDrag), naming the stop it landed on.
    s.run(&format!("{ROW}ControlSlider:SetValue(0.6)")).unwrap();
    assert_eq!(
        s.eval::<String>(&format!("return {ROW}ControlValue:GetText()"))
            .unwrap(),
        "Medium"
    );

    // An out-of-range value (an env A/B: the hermetic capture's clutter-off session seeds "-1")
    // displays the NEAREST stop — 0959's out-of-range law — and writes nothing back.
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

/// The Nameplates page (0992 — live at last): the three 1.12 UnitName* checkbox rows read the
/// table on select and write their flags on the interface panel's own click kit (checked →
/// CheckBoxOn; these rows carry no soundQuirk).
/// **A world entry must leave the saved nameplate setting alone** (decision 2132) — the report,
/// on the real manifest with a real CVar table, which is the one combination no test had.
///
/// `UIParent_OnEvent` calls `UpdateNameplates()` twice per entry — `UIParent.lua` l.234
/// (VARIABLES_LOADED) and l.367 (PLAYER_ENTERING_WORLD) — and it acts on
/// `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON`. On benilla the four verbs it calls write the CVar pair,
/// and the CVar pair IS the store, so with the globals left nil every entry wrote `"0"` over the
/// player's `"1"` and `config.toml` then dropped the line as "at default".
///
/// Two mechanisms are needed and this asserts both: the host seeding the globals ahead of
/// `VARIABLES_LOADED` (the enemy half), and our re-declared `UpdateNameplates` without the stock
/// `ShowFriendNameplates(); HideFriendNameplates();` typo (the friendly half). Drop either and the
/// matching row here goes back to `"0"`.
#[test]
fn a_world_entry_leaves_the_saved_nameplate_setting_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // A manifest load with no player is a state the client never reaches (1848).
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

    // The player has both halves on, restored from `config.toml` — a host write, so nothing is
    // queued and the file is not re-dirtied by having been read.
    let plates = crate::vplates::VPlateMode {
        enemies: true,
        friends: true,
    };
    let saved = |s: &mut UiScript| {
        s.set_cvar_host(crate::vplates::CVAR_ENEMIES, "1");
        s.set_cvar_host(crate::vplates::CVAR_FRIENDS, "1");
        assert!(s.take_cvar_changes().is_empty(), "the host write is silent");
    };

    // ── The bug, kept as the control: with the globals nil the entry erases both halves ────────
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

    // ── The fix: the host seeds the globals, and the entry is a no-op end to end ───────────────
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

    // The other direction still works: an addon that moves a global and calls the verb.
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
    // The plate pair leads the page and rides `VPlateMode::default()`: since 1804 both are OFF,
    // which is the reference's own boot state (its bitmask starts clear and FrameXML's
    // `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` start nil). Enemy was 0167's pin until then.
    for row in ["RowEnemyPlates", "RowFriendlyPlates"] {
        assert!(
            !s.eval::<bool>(&format!(
                "return BenillaOptionsFrameContainerBodyNameplates{row}Check:GetChecked()"
            ))
            .unwrap(),
            "{row} defaults unchecked"
        );
    }
    // The name rows ride their registered defaults, which are the binary's: player "1", NPC and
    // own "0" (1804 — the last two were director pins from 2026-07-12 until then).
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

    // The friendly plates row writes the bit the V/Shift-V pair writes — the same CVar, so the
    // window and the keys can never disagree about what is on.
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

    // Checking NPC Names queues the flag on and plays the ON kit; unchecking plays OFF (no quirk
    // here — the 1.12 interface panel's PlayClickSound mapping, not the sound panel's inverted
    // one).
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

/// The uiScale row DEFERS (0961, era CommitFlag.Apply transcribed): moves snap to the 1.12
/// 0.01 grid and update the readout, but the CVar does not move — the Apply button appears
/// instead, commits the pending value on click, and disappears. Dragging back onto the
/// committed value clears the pending without a commit (era's IsModified).
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

    // Off-grid 0.787 snaps to 0.79 and the readout follows — but NOTHING queues; the Apply
    // button exists now (era shows and enables it together).
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

    // A second move re-stages the pending value — still nothing queues (0989: the steppers
    // are gone; a drag's SetValue is the remaining move).
    s.run("BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlSlider:SetValue(0.8)")
        .unwrap();
    assert!(s.take_cvar_changes().is_empty());

    // Apply commits the LAST pending value, once, and the button goes away.
    s.run("BenillaOptionsFrameApplyButton:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("uiScale".to_string(), "0.8".to_string())]
    );
    assert!(!s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());

    // A move away arms Apply; dragging back onto the committed value disarms it.
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

/// **Render Scale** (decision 1639) — benilla's own Graphics row, and since 1648 the page's ONLY
/// antialiasing control: the reference's own Multisampling row was pulled on the director's call,
/// leaving `gxMultisample` reachable as a CVar and `$WOW_MSAA` but off the page.
///
/// Three things this pins, each of which was a real choice:
///
/// - **It reads the CVar and shows a percentage.** 1.0 must render as "100%", because the whole
///   point of the row is that the number in front of the player means something without a manual.
/// - **It is DEFERRED.** Committing live would rebuild the world's render target on every drag
///   tick — tens of megabytes, thirty times across one sweep of the handle. Staging to Apply makes
///   that exactly one rebuild. (`uiScale` above it defers for a different reason; the flag is the
///   same.)
/// - **It is not mute.** Every other tooltip on this window resolves an `OPTION_TOOLTIP_*` out of
///   1.12's GlobalStrings, and a benilla row with no counterpart there has so far gone silent (the
///   nameplate pair). This one carries a description under a `BENILLA_` prefix — the reference's
///   namespace stays the reference's — because the dial's first reviewer said outright that they
///   could not tell what it did.
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
    // The player's range is 50–200 %, narrower than the CVar's own clamp (0.25–4.0, which leaves
    // room for the supersampling instrument). A row that offered the whole clamp would put 400 %
    // in front of someone who only wanted their frame rate back.
    assert!(s
        .eval::<bool>(
            "local lo, hi = BenillaOptionsFrameContainerBodyGraphicsRowRenderScaleControlSlider:GetMinMaxValues()              return math.abs(lo - 0.5) < 0.0001 and math.abs(hi - 2.0) < 0.0001"
        )
        .unwrap());
    // Not mute: the row resolves a description, unlike the other benilla-own rows.
    assert!(s
        .eval::<bool>("return BENILLA_TOOLTIP_RENDER_SCALE ~= nil")
        .unwrap());
    let _ = s.take_cvar_changes();

    // The move stages and shows Apply; it does NOT rebuild the render target.
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

    // Apply commits once.
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

/// Pending edits are PANEL-wide like era's modified table: they survive a category switch
/// (the row redisplays the pending value on return, era's GetValue-returns-pending) and die
/// only when the window hides — the reopened window reads the committed truth.
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

    // Off to Audio: the Apply button stays (the pending edit is not page-scoped)…
    s.run("BenillaOptionsFrameCategoryListRowAudio:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameApplyButton:IsVisible()")
        .unwrap());
    // …and back: the slider shows the PENDING value, not the committed one.
    s.run("BenillaOptionsFrameCategoryListRowGraphics:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyGraphicsRowUiScaleControlValue:GetText()"
        )
        .unwrap(),
        "70%"
    );

    // Hide discards (the era confirm dialog is cut): the reopened window reads the truth.
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

/// The Terrain Distance slider (1513) is a LIVE row, unlike uiScale: a move snaps to the 1.12
/// panel grid — 177+n·60, ANCHORED AT THE MINIMUM, so 300 lands on 297, not the multiple-of-60
/// 300 — writes the CVar on the move as a clean short string, and raises no Apply button. The
/// write is what moves the far-clip wall and the residency window together.
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

/// Defaults on the Graphics page: uiScale back to its registered default (0.9) and farclip to
/// its (350), the rows following, ONLY the moved values queuing — and a pending uiScale edit
/// dies with it (the default write supersedes what Apply would have committed).
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
    // Stage a pending edit too — Defaults must kill it, not commit it.
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
    // **357, not 350, and that is the reference's ladder rather than a rounding slip.** The row
    // is the reference's own farclip slider — `minValue = OPTIONS_FARCLIP_MIN (177)`,
    // `maxValue = OPTIONS_FARCLIP_MAX (777)`, `valueStep = (max − min)/10` — so its ten stops are
    // 177, 237, 297, **357**, …, 777, and `SetValue` snaps onto them (2133). The registered
    // default is 350 (1804: the reference's own `CVar::Register` value), which is not a stop, so
    // seeding the slider from it lands on 357. 1.12 does exactly this and nobody can see it,
    // because its option sliders print no number; ours prints one, so the ladder shows.
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

/// Controls is the DEFAULT page and has rows since 0961: opening the window lands on it with
/// Defaults armed, and the rows read the table — Sticky Targeting INVERTED (checked when
/// `deselectOnClick` is "0", the 1.12 interface panel's own arm), the plain flags direct.
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

/// The Controls checkboxes write their flags on the INTERFACE panel's kit mapping (checked →
/// CheckBoxOn — OptionsFrame.lua's PlayClickSound, NOT the Audio page's inverted quirk), and
/// Sticky Targeting writes the CVar inverted both ways.
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

    // Invert Mouse on: flag "1", and the just-CHECKED box plays CheckBoxOn (normal mapping).
    s.run("BenillaOptionsFrameContainerBodyControlsRowInvertMouseCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("mouseInvertPitch".to_string(), "1".to_string())]
    );
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOptionCheckBoxOn".into())));

    // Sticky Targeting on: the write INVERTS — checking it writes deselectOnClick "0".
    s.run("BenillaOptionsFrameContainerBodyControlsRowStickyTargetCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("deselectOnClick".to_string(), "0".to_string())]
    );

    // …and off again: back to "1", the just-UNchecked box on the CheckBoxOff kit.
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

/// Defaults on the Controls page: the moved flags come back (deselectOnClick "1",
/// autoLootDefault "0"), the rows follow, and only the moved values queue.
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

/// Drive the window a few frames so the scroll body's frame-late fit converges: rects resolve
/// after Lua runs, so `OptionsScroll_Fit` answers one frame behind. (The tab widths are not in
/// that set — 2028 put them on a one-shot `<OnShow>OptionsTab_FitWidth`.)
fn settle(s: &mut UiScript) {
    for _ in 0..4 {
        s.resolve();
        s.tick(0.016);
    }
    s.resolve();
}

/// B217: a search that matches across categories chains five group heads and every matched row
/// into one column — ~150 units past the page area, which before the page scrolled drew straight
/// out through the dialog border onto the world. Now the body is the scroll child: it grows to the
/// content, the bar and its trough appear, and everything past the fold is CLIPPED to the page
/// rect until scrolled to.
#[test]
fn a_broad_search_scrolls_the_page_instead_of_overflowing_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    // On NAMEPLATES, not the default Controls page: since 2180 seated the four camera toggles and
    // the two camera-speed sliders, Controls is sixteen rows and overflows its area on its own —
    // which is the scroll body doing its job, and no longer a control for "nothing overflows".
    // Nameplates is six rows and fits.
    s.run("BenillaOptionsFrameCategoryListRowNameplates:Click()")
        .unwrap();
    settle(&mut s);

    // Control first: a settled page fits, so there is no scroll and no bar at all — and the body
    // sits exactly on the page rect, the seat every page's XML anchors were authored against.
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

    // Now the broad search: every category surfaces, and the column outruns the page.
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

    // THE SYMPTOM: nothing under the page draws past its bottom edge any more. Every quad the
    // page's own content emits carries the page rect as its clip (the engine's ScrollFrame
    // mechanism), so the deepest DRAWN pixel is the page's own bottom — while the content's own
    // rects still reach far below it, which is exactly the spill that used to be on screen.
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

    // The last result is reachable: scrolling to the end brings it inside the page rect. (Widget
    // coordinates here, not the extract's screen px — the window carries ERA_WINDOW_SCALE.)
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

    // Clearing the search puts the page — and the bar with its trough — back.
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

/// The bar's TROUGH — the recessed channel it rides in, the "background and border" a bare
/// arrows-and-knob bar was missing. It is the shared kit's template on 1.12's own
/// UI-Character-ScrollBar channel art over the ref's black backing, and it seats ITSELF on its bar
/// at the ref's hang: 31 wide against the bar's 16, 8 units left, and — the part B224 caught —
/// overhanging the bar by 21 above and 20 below, which is what drops each arrow button into the
/// 16-tall SOCKET the art carries for it. (The Keybindings page's own bar wears the same one —
/// keybindings_tests, where the harness has a real binding registry to overflow the list with.)
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
    // THE SOCKET LAW (BenillaScrollTrough_Seat): the trough overhangs the bar by 21/20 — not the
    // arrow-to-arrow 16/16 that B224 reported, which sat both arrows 4 units OUT of their sockets,
    // riding the caps with bare socket showing beside them. Ref: ReputationFrame's trough at the
    // scroll frame's +5/-4 against a UIPanelScrollBarTemplate bar at -16/+16.
    assert!(
        (g(trough, "GetTop") - g(bar, "GetTop") - 21.0).abs() < 0.01,
        "trough top 21 above the bar"
    );
    assert!(
        (g(bar, "GetBottom") - g(trough, "GetBottom") - 20.0).abs() < 0.01,
        "trough bottom 20 below the bar"
    );
    // Read the same law off the ARROWS, which is what the eye actually judges: 5 units of top cap
    // above the up arrow, 4 of bottom cap below the down arrow, and the 16-tall sockets between.
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
    // …and on THIS page the whole channel lands on the page rect: the scroll frame is the page
    // area, so the bar takes the 21/20 inset rather than the trough poking out through the header
    // divider above and the container's bottom below.
    let sf = "BenillaOptionsFrameContainerScroll";
    assert!((g(trough, "GetTop") - g(sf, "GetTop")).abs() < 0.01);
    assert!((g(trough, "GetBottom") - g(sf, "GetBottom")).abs() < 0.01);

    // The art: three channel slices from the one 1.12 file — top cap, stretched run, bottom cap —
    // each spanning the channel's full width, stacked with no gap and no overlap.
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

// ── B223 · the row tooltips (decision 1054) ─────────────────────────────────────────────────────

/// Hover the middle of a named row's LABEL half (left of the control column) — the surface the
/// reporter's cursor was on when the row lit and said nothing. Callers pin `ERA_WINDOW_SCALE = 1`
/// first (the wash test's own idiom): frame rects are in the frame's scale space, the pointer is
/// in the screen's, and at scale 1 the two are the same numbers.
fn hover_label(s: &mut UiScript, frame: &str) {
    s.resolve();
    let g = |s: &mut UiScript, verb: &str| -> f32 {
        s.eval(&format!("return {frame}:{verb}()")).unwrap()
    };
    let (l, t, b) = (g(s, "GetLeft"), g(s, "GetTop"), g(s, "GetBottom"));
    s.mouse_move(l + 60.0, (t + b) * 0.5);
}

/// Bring `frame` inside the page's scroll rect, if the page scrolls at all. A page longer than
/// its area clips everything past the fold (B217), so a row down there cannot be hovered where its
/// rect says it is — the cursor lands outside the scroll frame and hits nothing. Nine of the
/// Controls page's sixteen rows and the last of Combat's seventeen live there since 2180.
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

/// A hovered row raises its 1.12 description, on the era's seat, and drops it on leave — the row
/// itself, its checkbox, and (B223's report) the label the cursor actually crosses. The string
/// resolves by KEY at hover, so seeding it AFTER the window loaded still paints: that is the
/// property the 1.12 panel's own `getglobal("OPTION_TOOLTIP_"..key)` lookup buys.
#[test]
fn a_hovered_row_raises_its_1_12_description_on_the_era_seat() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    // The stock tooltip declares no size: it sizes from its lines through the font engine, as
    // the client's does (1968) — a harness that reads its rect needs one; the fixed-width
    // font is that engine here.
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
        "the description ALONE — the era's white name line is cut (1054)"
    );
    // The era seat: BOTTOMLEFT on the label region's TOPRIGHT, hung 10 back over the control
    // column (DefaultTooltipMixin's ANCHOR_RIGHT / x -10).
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

    // Crossing onto the checkbox keeps it up (one seam lights the wash and raises the plate, so
    // the row's OnLeave and the box's OnEnter cancel out inside the one move).
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

    // Leaving the row puts it away.
    s.mouse_move(5.0, 5.0);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "OnLeave drops the plate"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The row with no 1.12 string raises NOTHING — not a plate echoing the label under the cursor,
/// and not the LAST row's description left standing over it. Auto Loot is that row: 1.12 has no
/// Auto Loot setting, so no `OPTION_TOOLTIP_*` for it. The second leg is the live probe's find: a
/// hover driven without the outgoing row's OnLeave (1054) must still put the neighbour away.
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

/// The RUNTIME leg on the real client data (`ui_quest`'s pattern): every key the rows carry
/// resolves to a non-empty string in the shipped 1.12 `GlobalStrings.lua` — the guard against a
/// typo'd key degrading a description to silence — and the ONE row without a key is Auto Loot.
/// Pins the reporter's own row end to end (B223's screenshot text). Skips without client data.
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

    // The mapping, read off the LIVE rows so a page added later can't slip the check.
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
        // The deliberate exceptions (1639 Render Scale, 1650 Display Mode, 1847 Enable Sound in
        // Background, 2182 Brightness). None has a 1.12 counterpart whose `OPTION_TOOLTIP_*` could
        // be resolved — Render Scale has no era row at all, Display Mode's era row was a CHECKBOX
        // whose string says "Check to…", 1.12 has no background-sound setting at all (it mutes on
        // its window-activation event and offers no way out), and `OPTION_TOOLTIP_GAMMA` spends
        // its second sentence on "all 21 levels of gray bars to the right", which is the stock
        // video window's own calibration art and not something this page has — and each is a row a
        // player needs a description for. Each carries one under a `BENILLA_` prefix so the reference's
        // namespace stays the reference's, which is exactly what this guard is here to protect.
        // Everything the guard was built to catch — an invented or typo'd `OPTION_TOOLTIP_` key
        // that silently resolves to nothing — is untouched: the pairing below is exact, so a
        // `BENILLA_` key on the wrong row still fails.
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
    // 27 CVar rows (the Chat page's two bubble switches are 1139's and its Detailed Loot
    // Information + Guild Member Alert are 1589's; Status Bar Text, Mouse Sensitivity
    // and Max Camera Distance 1140's; Graphics' Vertical Sync is 1394's, its Display Mode
    // 1627's (a dropdown since 1650) and its Multisampling 1632's; Camera Following Style
    // 1493's; Terrain Distance 1513's) + the Combat page's 14 saved-variable rows (1134) + the Interface page's 6 (3 from
    // 1136, Buff Durations 1139, the target-of-target pair 1576), the Action Bars page's 2 (the
    // lock 1136, Always Show
    // ActionBars 1500) and the Chat page's 1 (Remove Chat Hover Delay, 1589) + 7 API rows (the Interface page's Show Cloak / Show Helm, 1472 and
    // Show Tutorials, 2077; the Action Bars page's four multibar switches, 1500) — which is the point of counting here rather than
    // per page: the third store's rows are held to the same "the key is 1.12's own and it resolves"
    // bar as the other two. Camera Following Style is counted on the key it wears at rest (Smart's
    // OPTION_TOOLTIP_CAMERA1) and Show When on its own (Always's OPTION_TOOLTIP_TARGETOFTARGET5);
    // their other entries ride the same census as the selection moves.
    // 59 of the 62 are 1.12's own; the other three are Render Scale (1639), Display Mode
    // (1650) and Enable Sound in Background (1847), whose descriptions are benilla's and whose
    // carve-out is above.
    // The 28th CVar row is Block Trades (1764), on the Controls page — its key
    // OPTION_TOOLTIP_BLOCK_TRADES is the reference's, so it is counted here like the rest; the
    // 29th is Enable Error Speech (1815), 1.12's own fourth Sound checkbox, ditto; the 30th is
    // Enable Sound in Background (1847), the Audio page's fifth checkbox and the one row on that
    // page the reference never made settable.
    // The 7th API row is Show Tutorials (2077) — the Interface page's Help box, whose store is
    // `TutorialsEnabled()` / `ResetTutorials()` / `ClearTutorials()` rather than a CVar, and whose
    // key OPTION_TOOLTIP_SHOW_TUTORIALS is 1.12's own.
    // The 30th and 31st CVar rows are the two text filters (2077): Profanity Filter on the
    // Interface page and Disable Spam Filter on the Chat page, both keys 1.12's own.
    // 2180 added fourteen rows, every one of them tipped with the reference's own key: the
    // four camera toggles and two camera-speed sliders plus Attack on assist, Auto Clear AFK and
    // Auto Self Cast on Controls, Enhanced Tooltips on Interface, Player Guild Names on
    // Nameplates, and the damage-number trio on Combat. 62 -> 76.
    // …and Weather Intensity (2181), on the Graphics page, whose OPTION_TOOLTIP_WEATHER_DETAIL is
    // 1.12's own and rides its own slider-9 row verbatim. 76 -> 77.
    // …and Brightness (2182), the FOURTH row whose description is benilla's own rather than 1.12
    // GlobalStrings — OPTION_TOOLTIP_GAMMA spends its second sentence on the stock window's
    // 21-step grey ramp, which this page does not have (see the guard above). 77 -> 78.
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

    // The reporter's row, byte for byte off the MPQ chain.
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

/// EVERY row on every page, all three control flavors: hovering it raises a plate exactly when
/// the row carries a 1.12 key, and no row's hover errors. The teeth are the per-template
/// `$parentTip` seat — a flavor missing that region would take `SetOwner(nil, …)` and print a red
/// line instead of a plate, which no key-mapping check would ever see.
#[test]
fn every_flavor_of_row_raises_its_plate_from_the_page_it_lives_on() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    // Stand-in strings for every key the rows name (the real texts are the data test's job).
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
    // 27 of the 28 CVar rows (the Chat page's two bubble switches are 1139's and its Detailed
    // Loot Information + Guild Member Alert 1589's; Status Bar Text, Mouse Sensitivity and
    // Max Camera Distance 1140's; Vertical Sync 1394's, Display Mode 1627's and Multisampling
    // 1632's; Camera Following
    // Style 1493's; Terrain Distance 1513's), plus the Combat page's 14 saved-variable rows (1134), the Interface
    // page's 6 (1136, + Buff Durations 1139, + the target-of-target pair 1576), Action Bars' 2
    // (the lock 1136, Always Show
    // ActionBars 1500), the Chat page's 1 (Remove Chat Hover Delay, 1589) and 6 API rows (Show
    // Cloak / Show Helm, 1472; the four multibar switches, 1500).
    // …plus the Graphics page's Render Scale (1639) and Display Mode (1650) and the Audio page's
    // Enable Sound in Background (1847), the three rows whose descriptions are benilla's own
    // rather than 1.12 GlobalStrings — see the guard above.
    // …and Block Trades (1764), the Controls page's 28th CVar row, and Enable Error Speech
    // (1815), the Audio page's fourth checkbox and 1.12's own.
    // …and Show Tutorials (2077), the Interface page's seventh API row.
    // …and the two text-filter rows (2077).
    // …and 2180's fourteen: Attack on assist, Auto Clear AFK, Auto Self Cast, Follow Terrain,
    // Head Bob, Auto-Follow Speed, Water Collision, Smart Pivot and Mouse Look Speed on Controls;
    // Enhanced Tooltips on Interface; Player Guild Names on Nameplates; Show Target Damage,
    // Periodic Damage and Pet Damage on Combat. Every one of them tipped with the reference's
    // own key, which is what makes the count move by exactly the number of rows added.
    // …and Weather Intensity (2181) and Brightness (2182), the Graphics page's fourth and fifth
    // sliders.
    assert_eq!(raised, 78, "every row but Auto Loot raises a description");
}

/// The **Combat page** (decision 1134) — the first rows in this window whose store is a
/// saved-variable GLOBAL rather than a CVar, and so the first thing that can change any of the
/// stock globals 1128 ported. The page is 1.12's AdvancedOptionsCombatText box: it reads the
/// globals on select, a click writes the global (and *nothing* reaches the CVar table), and each
/// write runs the family's `applyFunc` — `CombatText_UpdateDisplayedMessages`, whose visible
/// effect is the per-type `show` flag and the scroll function.
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

    // Read: each box shows its global, at CombatText.xml's own file-scope value — the whole
    // family at the reference's own defaults since 1804, master included (it was "1" from 0578).
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
    // The dropdown capsule reads COMBAT_TEXT_FLOAT_MODE = "1" as its named stop.
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyCombatRowFloatModeDropdownText:GetText()"
        )
        .unwrap(),
        "Scroll Up"
    );

    // Write: the global moves, the CVar table is untouched, and the applyFunc lands.
    //
    // The master goes first, and it is a write under test in its own right — but it is also the
    // gate: every other row on this page is greyed while `SHOW_COMBAT_TEXT` is "0" (the shipped
    // state since 1804; the dependency rule is pinned next door in
    // `the_combat_master_greys_the_family_and_combo_points_is_class_gated`), and a greyed control
    // eats its click. So the two clicks below only mean anything with this one ahead of them.
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

    // The dropdown writes its global and applies too (the scroll function follows the mode).
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

/// The master's dependency rule, 1.12's verbatim: `SHOW_COMBAT_TEXT` off greys **every** other
/// row on the page including the dropdown, and back on wakes them — except Combo Points, which
/// carries the reference's second gate and stays greyed for anyone who is not a rogue or druid.
///
/// Since 1804 the page **opens** in the greyed state, because the master ships at the reference's
/// "0" (0578's `"1"` was the pin). So the walk here is off → on → off, and the sub-rows' first
/// wake is the master's own click rather than the load: the same rule, entered from the other end.
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
    // The shipped state: the master is off, so the whole family is greyed on arrival — and the
    // master itself is not, because it is the way in.
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "0");
    assert!(!enabled(&mut s, "RowAuras", "Check"));
    assert!(!enabled(&mut s, "RowFloatMode", "DropdownButton"));
    assert!(enabled(&mut s, "RowCombatText", "Check"));

    // On: the siblings wake. No player class in this VM, so Combo Points stays greyed while they
    // are live — the two gates are independent.
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

/// Defaults on a saved-variable page restores each global to **the value its own XML assigned at
/// file scope** — captured at the row's OnLoad, which the load order guarantees runs before the
/// saved chunk (1128). The panel restates no defaults of its own, so it cannot drift from
/// CombatText.xml the way the reference's hand-copied `default` field can. Since 1804 that
/// assignment is the reference's own value for all fourteen, so this is now also the check that
/// Defaults lands a player back on a stock 1.12 combat-text family.
#[test]
fn defaults_resets_the_combat_page_to_the_shipped_assignments() {
    benilla_formats::wow_data_or_skip!();
    let s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    // Move FOUR of them off their shipped values, in both directions and both flavors. The master
    // leads because the page ships with it off (1804) and a greyed row eats its click — so it is
    // both the way in and the fourth value Defaults has to walk back.
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
        "the master walks back too — the reference's own value since 1804"
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

/// The point of the whole page: what it writes is **remembered**. A toggle lands in the
/// saved-variables text (1128's serializer) under its own name, and a fresh VM that executes that
/// text comes up with the player's choice rather than CombatText.xml's shipped default.
#[test]
fn what_the_combat_page_writes_survives_a_restart() {
    benilla_formats::wow_data_or_skip!();
    let s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    // The master first: the page ships with it off (1804) and a greyed row eats its click, so
    // without this the Honor Gained click below would be a silent no-op. Both writes are then
    // held to the same standard — they have to come back after the restart.
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

    // The restart: a fresh tree at its shipped defaults, then the saved chunk over the top.
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
    // And the page paints the restored value the next time it is opened.
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

/// The **Action Bars page**'s lock row (decision 1136) — the one global on that group whose store
/// is a uvar. It is also the page whose setting had to be BUILT first: 1134 §3 listed
/// `LOCK_ACTIONBAR` as "not defined, and no guard exists". The five switches above it are 1500's
/// and have their own test below.
///
/// The end-to-end teeth are the last block: the row's write reaches the shipped bar's own drag
/// guard in the same VM. `action_bar_tests`/`pet_bar_tests` own the guard's full behaviour
/// (including the shift-click that deliberately still works); this owns the wire between them.
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

    // The real bar, in this same VM, now refuses the drag — the row and the guard are wired.
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

    // Defaults walks it back to ActionBar.xml's own assignment, and the bar drags again.
    s.run("BenillaOptionsFrameContainerDefaults:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return LOCK_ACTIONBAR").unwrap(), "0");
    s.run("this = ActionButton1 ActionButton1:GetScript(\"OnDragStart\")()")
        .unwrap();
    assert!(s.cursor_payload().is_some(), "unlocked again");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The **Interface page** (decision 1136) — the second store's second page, and the three globals
/// 1134 §3 named as ready: already defined, already consumed, needing only a row. Each box reads
/// its definer's own file-scope assignment, and a click writes the global and touches nothing else.
///
/// None of the three carries an `applyFunc`, and that is the property being asserted at the end:
/// the consumer reads the global as it acts, so the write alone changes behaviour — here, the
/// questgiver's instant-text arm, live on the very next show.
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

    // Read: each box shows its definer's own file-scope value. Two ship on — QuestLogFrame.xml's
    // "1" (the reference's advertised default, as a STRING — see that file on 1.12's own
    // number/string break) and GameTooltip.xml's "1" — and Instant Quest Text ships OFF, which is
    // QuestFrame.xml's `QUEST_FADING_DISABLE = "0"`: the reference's value, and ours since 1804
    // (it was pinned "1" by direction on 2026-07-17).
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

    // Write: the global moves, and the CVar table is never reached.
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

    // No applyFunc, because the write IS the apply: the questgiver's fade arm reads the global as
    // the panel shows, so turning instant text ON makes the very next show land its text whole.
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

/// The **target-of-target pair** (decision 1576) — the Interface page's first dependent rows and
/// the window's first `uvar` DROPDOWN. Three things here belong to no other row:
///
/// * the picker is dead while the switch is off, which is 1.12's own rule for exactly this pair
///   (`OptionsFrame_DisableDropDown`, UIOptionsFrame.lua l.694-700);
/// * both rows carry the same `applyFunc`, and unlike the three stock globals above they NEED one
///   — the frame's visibility is decided once and kept, and the saved chunk lands after the file
///   that decided it, so without the re-run a saved "1" would leave the frame hidden;
/// * the picker's five values are the reference's own strings, in its own order.
#[test]
fn the_target_of_target_rows_gate_each_other_and_write_their_globals() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();

    // Ships off, at "always" — 1.12's own two defaults, read off UnitFrames.xml's file scope.
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

    // The switch: the global moves, the CVar table is never reached, and the picker wakes.
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

    // The picker: five entries in the reference's order, the stored value checked, and a pick
    // writes the reference's own value string.
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

/// The **equipment-display rows** — Show Cloak and Show Helm (decision 1472, B123). They are the
/// window's third kind of row and the only one with no store at all: the preference is a bit of the
/// character's own server-side `PLAYER_FLAGS`, so the row reads an engine getter and writes an
/// engine setter, 1.12's own `func`/`setFunc` pair for exactly these two entries.
///
/// Three things are asserted that no other row shape can be: the read comes from the API rather
/// than a saved value (a page revisit re-asks), the write leaves both the CVar table and the global
/// namespace untouched and produces a **wire** intent instead, and the getter follows the click
/// immediately — before the server's descriptor answers — because the wire verb is a blind flip and
/// a second click inside that round trip would otherwise compute the wrong direction.
#[test]
fn the_equipment_display_rows_read_and_write_through_the_api_not_a_store() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowInterface:Click()")
        .unwrap();

    // Both ship shown — the model's own default, and the reference's hand-written one.
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

    // The read is a re-ask, not a remembered string: leave the page and come back.
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

    // The wire is the truth: a descriptor edge that disagrees wins over the optimistic belief.
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

/// **Show Tutorials** (decision 2077) — the fourth `func` row, and the one 1.12's own table hides:
/// its `UIOptionsFrameCheckButtons` entry is a bare `{ index = 28 }`, so a reading of the table
/// alone (decision 1140's census) files it as unbound. The store is the three special-case arms —
/// `TutorialsEnabled()` reads it, `ResetTutorials()`/`ClearTutorials()` write it — which is what
/// this asserts: the row reads the acknowledged bank, ticking it OFF clears, ticking it back ON
/// resets, and the reference's `~=` guard keeps a no-op click from re-arming every dismissed
/// tutorial.
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

    // Ticking it off is `ClearTutorials()` — every bit acknowledged.
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowShowTutorialsCheck:Click()")
        .unwrap();
    assert_eq!(s.take_tutorial_clears(), 1, "off clears the bank");
    assert_eq!(s.take_tutorial_resets(), 0);
    assert!(
        s.take_cvar_changes().is_empty(),
        "there is no tutorial CVar in 1.12 — the row must not invent one"
    );

    // The app answers by pushing the cleared bank back; the row now reads off.
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

    // Ticking it back on is `ResetTutorials()`.
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowShowTutorialsCheck:Click()")
        .unwrap();
    assert_eq!(s.take_tutorial_resets(), 1, "on resets the bank");
    assert_eq!(s.take_tutorial_clears(), 0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference's `~=` guard, which is the whole reason its Save arm is not a bare pair of calls:
/// setting the row to the value it already holds must send NOTHING. Without it, a Defaults click on
/// a page where tutorials are already on would `ResetTutorials()` and re-arm every popup the player
/// has dismissed — an option that silently undoes hours of play.
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

/// Defaults on an API row: both boxes go back to shown, and the flip is sent **only** for the one
/// that was actually off — the setter is a *set* over a wire verb that is a blind *toggle*, so a
/// no-op default must not queue a packet that would turn the preference on its head.
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

/// Defaults here restores the value **each global's own definer file assigns**, which is the
/// visible payoff of capturing the default at OnLoad instead of restating it (1134 §1): the page
/// holds no second copy that could disagree with QuestFrame.xml or GameTooltip.xml.
///
/// This test was `defaults_on_the_interface_page_restores_our_pin_not_the_references` until 1804,
/// and its premise is now inverted. `QUEST_FADING_DISABLE` shipped `"1"` by direction
/// (2026-07-17) where 1.12's own table hand-writes `default = "0"`, so Defaults used to walk the
/// page back to a value the reference did not have. It walks back to `"0"` now — not because the
/// mechanism changed, but because the assignment it reads **is** the reference's. That is the
/// property worth holding: the capture follows the definer wherever the definer stands.
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

/// The point of the page: what it writes is remembered. The toggle lands in the saved-variables
/// text under the **reference's** global name — `AUTO_QUEST_WATCH`, which is why 1136 renamed our
/// `BENILLA_AUTO_QUEST_WATCH` onto it — and a fresh tree replaying that text comes up on the
/// player's choice instead of QuestLogFrame.xml's shipped one.
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

/// **A saved value that has a SIDE EFFECT has to be applied when the chunk lands.** The whole UI's
/// XML runs before `benilla-config/saved-variables.lua` executes over it (1128), so every file-scope
/// consumer of a global ran against the *shipped* default: `CombatText_OnLoad` decides its six
/// event registrations from `SHOW_COMBAT_TEXT` once, at load, and nothing re-runs when the saved
/// value replaces it. The player's choice came back undone at every restart. 1.12 closes this with
/// a hand-written ladder in `UIOptionsFrame`'s `VARIABLES_LOADED` arm ("Option specific function
/// calls", UIOptionsFrame.lua l.204-220); we hold each side effect on its own row already, so the
/// window runs them there.
///
/// **The saved value here is `"1"`, and the direction is the whole point.** Until 1804 the master
/// shipped `"1"`, so the bug was a saved `"0"` coming back ON and the test drove the switch down.
/// The master ships at the reference's `"0"` now, so driving it down would assert nothing — the
/// file-scope default already leaves the six events unregistered. The load-bearing case is the
/// other one: the registrations were never armed at all, and only the walk can arm them.
#[test]
fn a_saved_switch_with_a_side_effect_is_applied_when_the_variables_land() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    // What the saved chunk does, verbatim: assign over the file-scope default, then the event.
    s.run("SHOW_COMBAT_TEXT = \"1\"").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    // The walk loaded the addon (1964); a frame passes before any message can arrive, and the
    // stock message placement reads the strings' screen positions, which that frame lays out.
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

/// The **Buff Durations** row (decision 1139) — the first setting in this window whose value has a
/// consequence nothing re-derives on its own, and so the first to carry an `applyFunc`. The timer
/// text needs no hook (the bar re-decides it every frame), but the ROW PITCH is stated once: with
/// timers the buff bar's three rows sit 45px apart, without them 35px — the reference's own two
/// geometries (`BuffButtons_UpdatePositions`). The click has to move it, and so does the saved
/// value landing at load, which is what the VARIABLES_LOADED walk is for.
///
/// The walk runs OFF → ON, because the bar ships without timers: `SHOW_BUFF_DURATIONS` is the
/// reference's `"0"` since 1804. It shipped `"1"` from 0255, which put the durations-shown
/// geometry in on the director's call, through 1139, which turned it into a setting without
/// re-weighing what it ships as. Nothing about the two geometries or the apply hook changed —
/// only which of them a fresh install opens on.
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

    // Restart: the fresh tree comes up on the shipped geometry, the chunk replaces the value, and
    // VARIABLES_LOADED is what puts the bar where the value says.
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

/// **Every category in this window now leads somewhere** (decision 1139). Chat — which was called
/// `Social` until 1589 — was the last one
/// that opened onto an empty page, and the arc that started at 1134 — rows over the second store —
/// closes here: Controls, Interface, Action Bars, Combat, Chat, Nameplates, Graphics and Audio
/// all carry rows, and Keybindings runs its own machinery. A category added without a page fails
/// this, which is the point: the honest tree (0950) is now a property the test holds, not a
/// promise the reader has to check.
///
/// The Defaults guard is keyed on ROWS, not on the category being real, so it is pinned directly
/// rather than through a stand-in page — there is no longer one to borrow.
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
            "{key} opens onto nothing — every category leads somewhere since 1139"
        );
        s.run(&format!("BenillaOptionsFrameCategoryListRow{key}:Click()"))
            .unwrap();
        assert!(
            s.eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
                .unwrap(),
            "{key}: Defaults is live on a page that has something to reset"
        );
    }

    // And the guard itself: a key with no rows behind it leaves Defaults asleep.
    s.run("BenillaOptionsFrame_SelectCategory(\"NotACategory\")")
        .unwrap();
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrameContainerDefaults:IsEnabled() ~= 0")
            .unwrap(),
        "Defaults is dead when the selected page has no rows"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The **Chat page**'s two bubble switches (decision 1139; the page was `Social` until 1589 grew
/// it into 1.12's own `CHAT_LABEL` box) — and the last category in
/// this window to stop opening onto nothing. Both are CVar rows: `chat_bubble.rs` transcribed the
/// client's `ChatBubbles`/`ChatBubblesParty` spawn gate faithfully in 0598 and then froze it at a
/// pair of `const bool`, so this is the action-bar lock's shape again — the knob had to become
/// real before the row could mean anything. The page reads the registered table on select, a
/// click queues the flag the host drains onto `BubbleConfig`, and the two are independent (the
/// client gates party lines on their own CVar, which is why party bubbles survive turning
/// say/yell bubbles off).
/// **Disable Spam Filter** (decision 2077) — the reference's own *inverted* row, and the one shape
/// on this page that would read backwards if it were wired like its neighbours.
///
/// `spamFilter` registers `"1"` (the filter ON), and the label says *Disable*: 1.12 checks the box
/// when `GetCVar == "0"` (`UIOptionsFrame.lua` l.253) and flips the value before writing it
/// (l.337). So a fresh client shows the box **unchecked**, and ticking it turns the filter off.
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

    // And back: the row round-trips rather than latching.
    s.run("BenillaOptionsFrameContainerBodyChatRowSpamFilterCheck:Click()")
        .unwrap();
    assert_eq!(s.cvar("spamFilter").as_deref(), Some("1"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Profanity Filter** (decision 2077) — an ordinary CVar row, asserted because its *seat* is the
/// surprising part: 1.12 files it under Basic Options' **Display** box (CheckButton5 anchors under
/// CheckButton66), not with the chat settings its name suggests, so it closes the Interface page's
/// column-C chain rather than joining the Chat page.
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
    // No host override: since 1804 the registered pair IS bubbles-on / party-off, so the page's
    // read below is of the shipped table rather than of a value this test planted.
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
    // Read from the table, not from a restated default: bubbles on, party bubbles off.
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

    // Turning say/yell bubbles off writes only its own switch — party keeps the value above.
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

    // Defaults walks the page back to the registered pair, which is the binary's own since 1804:
    // `ChatBubbles` "1", `ChatBubblesParty` "0" (the party half was the director's /p ask, 0598).
    // The party row was clicked ON above, so this is a real walk-back, not a no-op.
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

/// The **Chat page**'s two new rows (decision 1589 — B246, "no chat section in options"), the two
/// that make it 1.12's `CHAT_LABEL` box rather than a two-row bubble page.
///
/// They are deliberately over *different stores*, and that is what this pins: **Remove Chat Hover
/// Delay** is a saved-variable global with an `applyFunc` (its consumer is a pair of constants
/// nothing re-reads per frame, so the write alone would change nothing), and **Detailed Loot
/// Information** is a CVar with none (the loot-roll composer reads it as each line is built —
/// 1136's rule for when a row needs one, seen from both sides on one page).
#[test]
fn the_chat_page_writes_the_hover_delay_global_and_the_loot_spam_cvar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowChat:Click()")
        .unwrap();
    let _ = s.take_sounds();
    let _ = s.take_cvar_changes();

    // Read: the shipped values, off the two files that declare them — not restated here.
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

    // The hover-delay row: the write is the global, and the applyFunc is what makes it mean
    // something. Both fade constants collapse to zero — the reference's SetChatMouseOverDelay.
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

    // …and back, which is the half a one-way applyFunc would break.
    s.run("BenillaOptionsFrameContainerBodyChatRowRemoveChatDelayCheck:Click()")
        .unwrap();
    assert_eq!(s.eval::<String>("return REMOVE_CHAT_DELAY").unwrap(), "0");
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.2, 0.15)
    );

    // The loot-spam row: a plain CVar write, no applyFunc, nothing else on the page moved.
    s.run("BenillaOptionsFrameContainerBodyChatRowLootSpamCheck:Click()")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("showLootSpam".to_string(), "0".to_string())]
    );
    assert!(s
        .eval::<bool>("return BenillaOptionsFrameContainerBodyChatRowChatBubblesCheck:GetChecked()")
        .unwrap());

    // Defaults walks the page back across BOTH stores in one click.
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

/// The saved *Remove Chat Hover Delay* survives a restart — the `applyFunc` runs at
/// `VARIABLES_LOADED`, which is the only thing that can re-collapse the fade constants after a
/// fresh VM has re-run `ChatFrame.xml`'s file-scope `"0"`.
#[test]
fn a_saved_hover_delay_is_applied_when_the_variables_land() {
    benilla_formats::wow_data_or_skip!();
    let mut s = chat_harness();
    // What the saved-variables chunk does: assign the global, then the window's VARIABLES_LOADED.
    s.run("REMOVE_CHAT_DELAY = \"1\"").unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.2, 0.15),
        "the bare assignment changes nothing on its own — that is why the row has an applyFunc"
    );
    // The reference's own VARIABLES_LOADED arm, off the chain since 2115 — this row's apply is
    // `SetChatMouseOverDelay(REMOVE_CHAT_DELAY)` at `UIOptionsFrame.lua:216`, and the event is
    // what runs it. (It used to be our own `OptionsFrame_ApplySavedSettings` walk.)
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert_eq!(
        s.eval::<(f64, f64)>("return CHAT_TAB_SHOW_DELAY, CHAT_FRAME_FADE_TIME")
            .unwrap(),
        (0.0, 0.0)
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Status Bar Text** (decision 1140) — the row that finally reaches `TextStatusBar.xml`. That
/// file has been transcribed whole since 1082, with a `CVAR_UPDATE` watcher and a `statusBarText`
/// read on every repaint, and nothing in the client could move the variable: `GetCVar` answered nil
/// for a key the host never registered, which reads as off. So the numerals were hover-only, with
/// no way to pin them.
///
/// The row is also the only one in this window carrying a `cvarEvent`, and this is what that buys:
/// the XP bar's numerals appear on the click, not on the next XP tick. The event is 1.12's own —
/// `SetCVar(cvar, value, index)`'s third argument, handed back as arg1 — which is why the token
/// here is the uppercase display name and not the CVar's own spelling.
#[test]
fn the_status_bar_text_row_pins_the_numerals_the_moment_it_is_clicked() {
    benilla_formats::wow_data_or_skip!();
    let mut s = interface_harness();
    // The bar needs a real span before it decides anything about its numerals (its update bails
    // on valueMax == 0 and hides the strip instead).
    s.set_player_xp(1000, 10000);
    s.run("this = MainMenuExpBar; MainMenuExpBar_Update()")
        .unwrap();
    // Shipped default: off, so the numerals only show while hovered.
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
    // No repaint, no XP tick — only the CVAR_UPDATE the third argument queued.
    s.tick(0.0);
    assert!(
        s.eval::<bool>("return MainMenuBarExpText:IsShown()")
            .unwrap(),
        "the watcher woke on the click, not on the next value change"
    );

    // And back off again, the same way.
    s.run("BenillaOptionsFrameContainerBodyInterfaceRowStatusTextCheck:Click()")
        .unwrap();
    s.tick(0.0);
    assert!(!s
        .eval::<bool>("return MainMenuBarExpText:IsShown()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Mouse Sensitivity** (decision 1140) — the Controls page's first slider, and the third frozen
/// constant this arc has unfrozen: the camera's radians-per-pixel rate was a `const` with no way
/// to reach it. 1.12's own row (`UIOptionsFrameSliders`' MOUSE_SENSITIVITY): 0.5 … 1.5 by 0.05,
/// a multiplier, so the registered default 1 is the shipped feel and the percent readout reads it
/// straight. The slider snaps to the reference's step and writes the CVar the host drains onto
/// `LookConfig::sensitivity`.
#[test]
fn the_mouse_sensitivity_slider_snaps_to_the_reference_step() {
    benilla_formats::wow_data_or_skip!();
    let mut s = audio_harness();
    s.set_cvar_host("mousespeed", "1.25");
    let mut s = harness_on(s);
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();

    // Read from the table on select, with the era's rounded-percent readout.
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

    // A user move snaps to 0.05 and writes once.
    s.run("BenillaOptionsFrameContainerBodyControlsRowMouseSpeedControlSlider:SetValue(1.42)")
        .unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("mousespeed".to_string(), "1.4".to_string())]
    );

    // Defaults walks it back to the neutral notch — the shipped feel, not the slider's floor.
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

/// **Max Camera Distance** (decision 1140) — the fourth frozen constant, and the one with a wrinkle
/// worth pinning: 1.12 stores a FACTOR over `cameraDistanceMax`'s 15 yd base, so the value that
/// persists is `1.0 … 2.0` while the thing the player is choosing is a distance. The readout shows
/// the distance; the CVar carries the factor.
///
/// The slider's RANGE is unchanged and still reaches the 30 yd ceiling benilla used to ship at;
/// what moved is where it rests. Since 1804 the default is the registrar's own `1.0` — 15 yd, the
/// distance a fresh 1.12 client stops the wheel at — so the walk here is up the range and back
/// rather than down it.
#[test]
fn the_max_camera_distance_slider_stores_a_factor_and_reads_out_yards() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();

    // The shipped ceiling, shown as what it buys.
    assert_eq!(
        s.eval::<String>(
            "return BenillaOptionsFrameContainerBodyControlsRowMaxCameraDistanceControlValue:GetText()"
        )
        .unwrap(),
        "15 yd",
        "vanilla's own out-of-box ceiling"
    );

    // A move off the notch: the factor is what persists, the yards are the label — and the two
    // disagree numerically, which is why a mid-range stop is worth a step of its own.
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

    // All the way up — the slider's top, which is where this row's default sat until 1804.
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

/// **Camera Following Style** (decisions 1493/1502) — 1.12's `cameraSmoothStyle`, worn as a
/// Controls-page dropdown, and the setting that decides whether the camera returns to behind the
/// character at all. What is pinned here is the trap: the reference's own dropdown writes `1/2/3`,
/// but the ENGINE's tables are indexed `0 = Never · 1 = Smart · 2 = Always`, and `3` is not a style
/// — the validator (`0x50c060`) accepts it while the terrain-tilt consumer (`0x50dbc0`) indexes
/// off the end of its table. So our entries carry the engine's numbers in the
/// reference's display order, a stray `3` still reads as Never rather than as the numerically
/// nearest "Always", and the plate follows the SELECTION the way that dropdown's own does.
#[test]
fn the_camera_following_style_dropdown_carries_the_engine_enum_and_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness_on(audio_harness());
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowControls:Click()")
        .unwrap();

    // The registrar default is the reference's, and it is what the director asked to ship.
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

    // The list is the reference dropdown's, entry for entry and in its order.
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

    // Never stores "0" — the engine's own index, not the "3" the reference's dropdown writes —
    // and the row's plate becomes Never's own description.
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

    // Always is the middle entry and stores "2".
    s.run("BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownButton:Click()")
        .unwrap();
    s.run("DropDownList1Button2:Click()").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("cameraSmoothStyle".to_string(), "2".to_string())]
    );

    // A config written by a REAL 1.12 client says "3" for Never. It must not display as the
    // numerically nearest stop — which is Always, the opposite of what it means.
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

    // Defaults walks it back to Smart.
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

/// The **Action Bars page's five switches** (decision 1500) — the four bar toggles and the grid
/// option, on ONE page over TWO different stores, which is the whole point of the test.
///
/// The four bar rows are API (`func`) rows: there is nothing local to save, because the preference
/// is four bits of this character's server-side `PLAYER_FIELD_BYTES` byte 2. A click writes the
/// live Lua global, re-derives the bars and re-sends the whole byte — with **four** arguments, the
/// binding's verified arity. Always Show ActionBars is a saved-variable row instead, and NOT
/// because someone preferred it: the same binding silently drops the fifth argument the reference
/// passes it, so that switch has no server store to write.
///
/// The end-to-end teeth are the real bars moving in the same VM — the row's write reaches
/// `MultiActionBar_Update`, which reaches `UIParent_ManageFramePositions`.
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

    // Read: every switch ships OFF, and so does every bar.
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

    // Bar 4's row is DEAD while bar 3's is unticked — MultiBarLeft cannot stand without
    // MultiBarRight (the reference's own rule for this pair, UIOptionsFrame.lua l.722-726).
    assert!(
        !s.eval::<bool>(&format!(
            "return {}:IsEnabled() ~= 0",
            box_of("RowMultiBar4")
        ))
        .unwrap(),
        "Show Right ActionBar 2 is disabled until Show Right ActionBar is on"
    );

    // The write: a click raises the bar, writes the global as the number 1, and sends the byte.
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

    // …and the managed bottom stack moved with it (CONTAINER_OFFSET_Y's bottomEither 27).
    assert_eq!(s.eval::<f64>("return CONTAINER_OFFSET_Y").unwrap(), 97.0);

    // Ticking bar 3 wakes bar 4's row; ticking bar 4 then brings MultiBarLeft up beside it.
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

    // Untick bar 3 and MultiBarLeft goes with it, even though bar 4's own flag is still set — the
    // conjunction is in MultiActionBar_Update, not in the row.
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

    // The grid switch is the OTHER store: a saved-variable global, no packet, and an applyFunc
    // that opens every extra bar's empty wells.
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

    // Defaults walks the whole page back: every bar down, the grid off, the lock unlocked.
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

// ── The window tabs (decision 2028) ─────────────────────────────────────────────────────────────

/// A measurer that models the one property `FixedWidthFont` throws away: **glyph advances step to
/// whole physical pixels**, so a label is not proportionally the same width at two scales. Six
/// units per character at the DRAWN raster size (`6 × scale`, rounded), divided back by the scale
/// — the shape `ui_text::measurer` really has, small enough to predict by hand.
struct SteppedFont(f32);

impl benilla_ui::script::TextMeasure for SteppedFont {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let per_glyph = (self.0 * req.scale).round();
        let natural = req.text.chars().count() as f32 * per_glyph / req.scale;
        (natural, 12.0, natural)
    }
}

/// **Both tabs fit their labels once per show** — the era's width law (`MinimalTab.lua` l.7:
/// label + 40) on the 1.12 tab template's own seat (`<OnShow>` → `PanelTemplates_TabResize`,
/// decision 1993), with no OnUpdate poll behind it and no `fitted` latch. The engine's measurer
/// answers `GetStringWidth` inside the Lua call that asks it, so one call is the whole fit.
///
/// **The seat is a show and not the era's OnLoad, and that is what the numbers below pin.**
/// `GetStringWidth` answers in the region's OWN units, so its number depends on the owner's
/// effective scale — and this window's scale is set by `BenillaOptionsFrame_UpdateScale` in the window's
/// own `<OnShow>`, after every `<OnLoad>` has already run. At OnLoad these tabs measure at scale 1
/// (64 and 76 here); on the show, behind the window's own OnShow, they measure at the drawn 0.78.
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

    // The law, against each tab's own live measure.
    for tab in ["BenillaOptionsFrameGameTab", "BenillaOptionsFrameAddOnsTab"] {
        let w = num(&mut s, &format!("{tab}:GetWidth()"));
        let l = num(&mut s, &format!("{tab}Text:GetStringWidth()"));
        assert!(
            (w - (l + 40.0)).abs() < 0.01,
            "{tab} is {w} wide; the era's law is its label ({l}) + 40"
        );
    }
    // …and the numbers themselves, which say WHEN it ran. At 0.78 a 6-unit glyph rasterizes at 5
    // physical px and reads back 5/0.78 units, so "Game" (4) is 25.64 and "AddOns" (6) is 38.46.
    // An OnLoad fit — before the window's OnShow set the scale — would have measured at 1 and left
    // these at 64 and 76, which is 3.3 and 15.5 units off the drawn label.
    let close = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(close(
        num(&mut s, "BenillaOptionsFrameGameTab:GetWidth()"),
        65.641_03
    ));
    assert!(close(
        num(&mut s, "BenillaOptionsFrameAddOnsTab:GetWidth()"),
        78.461_54
    ));

    // Nothing re-fits them afterwards: the same two numbers with four frames run, which is the
    // value the retired OnUpdate settle used to converge to and latch.
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

    // A re-show re-fits rather than latching — the stock template's own behaviour, and what lets
    // a tab follow the window's scale when the screen changes under it.
    s.run("HideUIPanel(BenillaOptionsFrame) ShowUIPanel(BenillaOptionsFrame)")
        .unwrap();
    s.resolve();
    assert!(close(
        num(&mut s, "BenillaOptionsFrameGameTab:GetWidth()"),
        65.641_03
    ));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The falsifier: **with no measurer installed**, the same one-shot measures 0 and each tab comes
/// out 40 wide — the law's padding and nothing else. That is the engine-less truth (`benilla-ui`
/// `script/measure.rs`: absent is a supported state, metrics stay 0 until the host's batch
/// round-trip fills them), and it is why the fit is a *shown* frame's job: by then the app has
/// seated `AtlasMeasurer` for certain, whatever order the boot took.
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

// ── The reference table's CVar census (decision 2077) ────────────────────────────────────────────
//
// `UIOptionsFrameCheckButtons` in `OptionsFrame.xml` is a transcription of the reference's own
// table (`UIOptionsFrame.lua` l.4-84), and every entry names its store. Twenty-eight of them name a
// **CVar** — and a name in that table is a claim: `SetCVar(value.cvar, …)` against a CVar this
// client never registered writes nothing and reads back nil, so the entry is a setting benilla
// appears to offer and does not.
//
// Decision 1140 §4 wrote that census as prose in a decision record, and prose in a record is a
// snapshot: by the time anyone looked again it said SHOW_TUTORIALS had "no store at all" (it has
// three special-case arms, 2077) and had been amended once already for the multibar rows. So the
// census lives here instead, where it cannot go stale — the same posture as `cvars::Reference`,
// and checked in **both directions**: an unregistered name missing from this list fails, and a
// listed name that has since been registered fails too.
//
// A row on this list is not a defect. It is a 1.12 setting whose *feature* benilla does not have,
// and the reason is the row's whole point — "no key without a reader" (1134 §4) is why the CVar is
// absent, and this is where that decision is written down per setting.
const UNBACKED_REFERENCE_CVARS: &[(&str, &str)] = &[
    // **A blocker is now a SPEC, not a label.** Every row below has had its mechanism derived at
    // the bytes since 2115 wrote this list, so what is missing is the *feature*, never the
    // knowledge — and saying which is the difference between a row somebody can pick up and a row
    // that just reads "not done".
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
    // **The SLIDERS' half is empty, and that is the point.** 2115 extended this census from
    // `UIOptionsFrameCheckButtons` to `UIOptionsFrameSliders` and it caught exactly one row —
    // `cameraYawMoveSpeed`, the MOUSE_LOOK_SPEED slider — which was not cosmetic:
    // `UIOptionsFrame_Load` does `slider:SetValue(GetCVar(value.cvar))`, `Slider:SetValue` is a
    // shape-A binding (`0x790980`) that RAISES on a nil in the reference too, and that raise is
    // what stopped the stock window's `_Load()` (and `_SetDefaults`, through `GetCVarDefault`).
    // It is registered now, so all four slider CVars are backed and the window's `_Load()` runs to
    // completion. Every row left below is a check button, and a check button's
    // `GetCVar(x) == "1"` is nil-safe — so nothing on this list can raise any more. A new SLIDER
    // row arriving here is therefore a live raise, not a missing feature, and should be read that
    // way by whoever adds it.
];

/// **Every CVar the reference's own options table names is registered here, or listed above with
/// the feature it waits on** (decision 2077) — the check that turns 1140 §4's prose census into
/// something that cannot rot, in both directions.
///
/// This is the instrument for the whole class: a table entry that names a CVar benilla does not
/// register is a row that would tick and do nothing, and until now the only thing standing between
/// us and shipping one was somebody re-reading a decision record from months ago.
#[test]
fn every_cvar_the_reference_table_names_is_registered_or_listed_with_its_blocker() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    // The table as the file declares it: `name` (the display key) -> its `cvar`, if it has one.
    // **BOTH of the reference's option tables** (2115). The checkbuttons were the whole census
    // until the stock window went on the manifest and `UIOptionsFrameSliders` turned out to name a
    // CVar nothing here registers — a row the check could not see because it only read one table.
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

    // The other direction: a listed name that IS registered now means the feature landed and the
    // row is stale — exactly the drift `cvars::Reference` guards against on its own column.
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

/// **Nothing the reference lets a player change, and this client backs, is out of reach**
/// (decision 2180) — the other half of the census above, and the half that was missing.
///
/// The test above asks the ENGINE's question: is the CVar registered? It was green the entire time
/// the four camera toggles had no box on any window a player can open. 2149 built their
/// mechanisms and dropped them off `UNBACKED_REFERENCE_CVARS`; 2115 loads the stock window that
/// carries their checkboxes deliberately HIDDEN, so addons find real frames and the player never
/// sees it; and our own window — the one the ESC menu opens — simply had no rows. Two records were
/// individually right and the setting was unreachable, because no check compared them.
///
/// This asks the UI's question. A CVar that lands on the list below is one the reference lets a
/// player change and benilla does not, and it costs a stated reason at the row.
#[test]
fn every_registered_reference_cvar_has_a_row_on_our_own_window() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();

    // Both of the reference's own option tables, exactly as the registration census reads them.
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

    // What OUR window can actually move. A row's own `cvar`, plus the `partner` a handful of rows
    // write beside it (`PetSpellDamage`, `cameraPitchMoveSpeed`) — those are moved by a control
    // the player uses, which is what "reachable" means here, even though neither is table-named.
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

    // Two floors, because the failure this check is worth having is the one where it passes for
    // the wrong reason: a harness whose stock tables did not load reads no names, and one whose
    // window did not build reads no rows, and either way `missing` is empty and green.
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

    // The other direction, the one that keeps the list from rotting: an excuse for a row that now
    // exists is an excuse describing nothing.
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

/// **It is empty, and that is the finding, not an omission.** Every CVar the reference's own two
/// option tables name is now either unregistered (and on `UNBACKED_REFERENCE_CVARS` above with the
/// feature it waits on) or reachable from benilla's own options window. `autointeract` and
/// `UnitNamePlayerPVPTitle`, the last two unbacked rows, will each need a row here the day their
/// feature lands — which is exactly what the census above will say when it calls them stale.
///
/// A name arriving here needs a reason of the same shape as the list above's: what the *player*
/// cannot do, and why the row is not simply seated.
const UNREACHABLE_REFERENCE_CVARS: &[(&str, &str)] = &[];

/// The four camera toggles (2149's mechanisms, 2179's water-collision rebuild) on the page a
/// player can open. Each box opens showing the value the reference registers — two of them "1",
/// which is the half that makes this more than cosmetic: on `cameraPivot` and
/// `cameraWaterCollision` a player who wants them OFF had no way to say so at all.
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
    // The registrar's own values: pivot and water collision ON, terrain tilt and head bob OFF.
    assert!(checked(&mut s, "RowSmartPivot"));
    assert!(checked(&mut s, "RowWaterCollision"));
    assert!(!checked(&mut s, "RowFollowTerrain"));
    assert!(!checked(&mut s, "RowHeadBob"));

    // And the click writes — the first flip of each is the one that could not be expressed before.
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

/// **The partner write** (2180): two rows drive a second CVar that appears in neither option
/// table, because `UIOptionsFrame_Save` keeps a special-case arm for each. Without it the control
/// is a half-write, and a silent one — `SetCVar` on a name nobody wrote is not an error, it is a
/// value that stayed where it was, so the box would read "pet damage off" while pet SPELL damage
/// kept floating.
#[test]
fn the_pet_damage_box_and_the_look_slider_each_write_their_unnamed_twin() {
    benilla_formats::wow_data_or_skip!();
    let mut s = combat_harness();
    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    s.run("BenillaOptionsFrameCategoryListRowCombat:Click()")
        .unwrap();
    let _ = s.take_cvar_changes();

    // Pet Damage off: both pet CVars move, the melee one through the row and the spell one
    // through the partner (UIOptionsFrame_Save l.334-336).
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

    // Mouse Look Speed: the pitch twin at half the yaw value (l.355-356) — which is exactly the
    // 180/90 pair the two are registered at, so the ratio the reference ships is preserved by
    // dragging rather than broken by it.
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

/// **Show Target Damage is a SECOND master on the Combat page, and the floating-text one does not
/// own it.** The reference keeps the two dependency blocks apart —
/// `UIOptionsFrame_UpdateDependencies` greys 53…69 from CheckButton52 and greys 9 and 11 from 19,
/// in two separate `if`s — and collapsing them would tell a player that turning off scrolling
/// combat text also turns off damage numbers, which is the opposite of what `CombatDamage` does.
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

    // `SHOW_COMBAT_TEXT` ships "0", so the page arrives with the floating-text family greyed —
    // and the damage trio live beside it, which is the whole point of the split.
    assert_eq!(s.eval::<String>("return SHOW_COMBAT_TEXT").unwrap(), "0");
    assert!(!enabled(&mut s, "RowAuras"));
    assert!(enabled(&mut s, "RowShowDamage"));
    assert!(enabled(&mut s, "RowPeriodicDamage"));
    assert!(enabled(&mut s, "RowPetDamage"));

    // The other master: Show Target Damage off greys its two children and nothing else.
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

/// The two other dependencies 2180 seated, both the reference's own: Player Guild Names is dead
/// while player names are off (l.702-708 — there is no overhead stack for the guild line to be
/// the second row of), and Auto-Follow Speed is dead while the following style is Never
/// (l.715-719 — there is no follow to set a speed for).
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
    // …and the groove refuses the press as well as looking refused, which the reference never
    // needed: its slider has no track jump to refuse.
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
