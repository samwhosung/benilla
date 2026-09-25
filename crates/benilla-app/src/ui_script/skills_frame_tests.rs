//! The stock Skills page (`SkillFrame.xml`), driven engine-only. `SkillFrame.lua`'s
//! `skillMaxRank == 1` gate paints a gray bar with no rank text: for armor proficiencies (1/1 from
//! the server) and for single-rank lines (`mono`), whose max the reference forces to 1 off
//! `SkillRaceClassInfo.flags & 0x400` whatever the server sends (`0x4d3610`, branch `0x4d38b1`).

use benilla_ui::script::{QuadContent, SkillEntry, SkillsState, UiScript, UnitState};

fn skill(
    skill_id: u32,
    name: &str,
    value: u32,
    max: u32,
    mono: bool,
    (category_id, category_name, category_order): (u32, &str, u32),
) -> SkillEntry {
    SkillEntry {
        skill_id,
        name: name.into(),
        value,
        max,
        temp_bonus: 0,
        perm_bonus: 0,
        min_level: 0,
        cost_index: 0,
        category_id,
        category_name: category_name.into(),
        category_order,
        description: String::new(),
        abandonable: false,
        mono,
    }
}

/// The Skills page shown over a hunter's vmangos lines: single-rank `Beast Mastery 300/300`
/// (`SkillRaceClassInfo` flags 0x410), `Defense 12/60` and the server-capped `Cloth 1/1`.
fn shown_skills_page() -> UiScript {
    shown_skills_page_with(None)
}

/// [`shown_skills_page`] with a text measurer seated before the load, as at world entry.
fn shown_skills_page_with(measurer: Option<Box<dyn benilla_ui::script::TextMeasure>>) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    if let Some(m) = measurer {
        s.set_text_measurer(m);
    }
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    // Stock `PaperDollFrame_SetLevel` formats level, race and class unguarded on every show
    // (`PaperDollFrame.lua:100-104`).
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 60,
            race: Some("Tauren".into()),
            race_file: Some("Tauren".into()),
            class: Some("Hunter".into()),
            class_file: Some("HUNTER".into()),
            ..UnitState::default()
        }),
    );
    s.set_skills(SkillsState {
        entries: vec![
            skill(50, "Beast Mastery", 300, 300, true, (7, "Class Skills", 2)),
            skill(95, "Defense", 12, 60, false, (6, "Weapon Skills", 5)),
            skill(415, "Cloth", 1, 1, false, (8, "Armor Proficiencies", 6)),
        ],
    });
    s.run(r#"ToggleCharacter("SkillFrame")"#).unwrap();
    s.resolve();
    s
}

/// Row slot `i`'s rank text and bar colour, the two things the proficiency gate decides.
fn row(s: &mut UiScript, i: u32) -> (String, [f32; 4]) {
    let text = s
        .eval::<String>(&format!(
            "return SkillRankFrame{i}SkillRank:GetText() or \"\""
        ))
        .unwrap();
    let c = s
        .eval::<(f32, f32, f32, f32)>(&format!("return SkillRankFrame{i}:GetStatusBarColor()"))
        .unwrap();
    (text, [c.0, c.1, c.2, c.3])
}

/// The quads owned by the page's `Skill*` frames, not the rest of `CHARACTER_UI`.
fn page_quads(s: &UiScript) -> Vec<benilla_ui::script::ExtractedQuad> {
    s.extract()
        .into_iter()
        .filter(|q| {
            s.target_owner_name(q.target)
                .is_some_and(|n| n.starts_with("Skill"))
        })
        .collect()
}

/// Row slot `i`'s trough colour (`SkillFrame.lua:158` normal, `:167` proficiency).
fn row_bg(s: &mut UiScript, i: u32) -> [f32; 4] {
    let c = s
        .eval::<(f32, f32, f32, f32)>(&format!(
            "return SkillRankFrame{i}Background:GetVertexColor()"
        ))
        .unwrap();
    [c.0, c.1, c.2, c.3]
}

#[test]
fn a_single_rank_line_paints_gray_with_no_rank_text() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_skills_page();
    assert!(
        s.eval::<bool>("return SkillFrame:IsVisible()").unwrap(),
        "the Skills page is up"
    );

    // Rows: 1 Class Skills, 2 Beast Mastery, 3 Weapon, 4 Defense, 5 Armor, 6 Cloth.
    let (text, color) = row(&mut s, 2);
    assert_eq!(
        text, "",
        "Beast Mastery is single-rank: no rank text, though the server said 300/300"
    );
    assert_eq!(color, [0.5, 0.5, 0.5, 1.0], "and a gray bar");
    assert_eq!(
        row_bg(&mut s, 2),
        [1.0, 1.0, 1.0, 0.5],
        "over the proficiency branch's WHITE trough (ref SkillFrame.lua:167)"
    );

    let (text, color) = row(&mut s, 6);
    assert_eq!(text, "", "Cloth is 1/1: no rank text");
    assert_eq!(color, [0.5, 0.5, 0.5, 1.0], "gray too");
    assert_eq!(row_bg(&mut s, 6), [1.0, 1.0, 1.0, 0.5], "white trough too");

    let (text, color) = row(&mut s, 4);
    assert_eq!(text, "12/60", "Defense keeps its rank text");
    assert_eq!(color, [0.0, 0.0, 1.0, 0.5], "and the blue fill");
    assert_eq!(
        row_bg(&mut s, 4),
        [0.0, 0.0, 0.75, 0.5],
        "over the normal branch's DARK BLUE trough (ref SkillFrame.lua:158)"
    );

    // The trough texture is `<Color 1,1,1,0.2>` and the vertex colour multiplies it, alpha
    // included, so white draws at 0.2 x 0.5 = 0.1; every solid quad the page emits is a trough.
    let solids: Vec<[f32; 4]> = page_quads(&s)
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: None,
                color: Some(c),
                ..
            } => Some(*c),
            _ => None,
        })
        .collect();
    assert!(
        solids.contains(&[1.0, 1.0, 1.0, 0.1]),
        "a proficiency trough draws white at 0.1; got {solids:?}"
    );
    assert!(
        solids.contains(&[0.0, 0.0, 0.75, 0.1]),
        "a normal row's trough draws dark blue at 0.1; got {solids:?}"
    );
    assert!(
        !solids.iter().any(|c| c[3] == 0.5),
        "nothing draws at the raw vertex alpha — that was the replace bug; got {solids:?}"
    );

    // The detail pane's bar takes the same gate (`SkillFrame.lua:370-376`).
    s.run("SetSelectedSkill(2) SkillFrame_UpdateSkills()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return SkillDetailStatusBarSkillRank:GetText() or \"\"")
            .unwrap(),
        "",
        "the detail pane's own bar is a proficiency too"
    );
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return SkillDetailStatusBarBackground:GetVertexColor()")
            .unwrap(),
        (1.0, 1.0, 1.0, 0.5),
        "including its trough (the shared PaintBar; ref SkillFrame.lua:376)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A level-60 tauren hunter's vmangos skill block, through [`crate::ui_char::skills_row`] and the
/// real DBCs, lists what the reference lists: four headers and fourteen lines, with no
/// `Dual Wield`, `Tauren Racial` or `GENERIC (DND)`, though the server sends them.
#[test]
fn a_real_hunters_block_lists_exactly_what_the_reference_client_lists() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");

    // (skill line, value, max) for a tauren (race 6) hunter (class 3) at level 60.
    const BLOCK: &[(u16, u16, u16)] = &[
        (44, 300, 300),  // Axes
        (46, 300, 300),  // Guns
        (50, 300, 300),  // Beast Mastery: single-rank
        (51, 300, 300),  // Survival: single-rank
        (95, 300, 300),  // Defense
        (109, 300, 300), // Language: Orcish
        (115, 300, 300), // Language: Taurahe
        (118, 300, 300), // Dual Wield: hidden
        (124, 300, 300), // Tauren Racial: hidden
        (162, 300, 300), // Unarmed
        (163, 300, 300), // Marksmanship: single-rank
        (173, 300, 300), // Daggers
        (183, 300, 300), // GENERIC (DND): hidden
        (226, 300, 300), // Crossbows
        (413, 1, 1),     // Mail
        (414, 1, 1),     // Leather
        (415, 1, 1),     // Cloth
    ];
    let entries: Vec<SkillEntry> = BLOCK
        .iter()
        .filter_map(|&(skill_id, value, max)| {
            let slot = benilla_protocol::messages::PlayerSkillSlot {
                skill_id,
                step: 0,
                value,
                max,
                temp_bonus: 0,
                perm_bonus: 0,
            };
            crate::ui_char::skills_row(&slot, &catalog, 6, 3, 60)
        })
        .collect();

    let mut s = UiScript::new().unwrap();
    s.set_skills(SkillsState { entries });

    let n = s.eval::<i64>("return GetNumSkillLines()").unwrap();
    let rows: Vec<(String, bool)> = (1..=n)
        .map(|i| {
            s.eval::<(String, Option<i64>)>(&format!(
                "local n,h = GetSkillLineInfo({i}) return n,h"
            ))
            .map(|(name, header)| (name, header.is_some()))
            .unwrap()
        })
        .collect();

    let expected: Vec<(&str, bool)> = vec![
        ("Class Skills", true),
        ("Beast Mastery", false),
        ("Marksmanship", false),
        ("Survival", false),
        ("Weapon Skills", true),
        ("Axes", false),
        ("Crossbows", false),
        ("Daggers", false),
        ("Defense", false),
        ("Guns", false),
        ("Unarmed", false),
        ("Armor Proficiencies", true),
        ("Cloth", false),
        ("Leather", false),
        ("Mail", false),
        ("Languages", true),
        ("Language: Orcish", false),
        ("Language: Taurahe", false),
    ];
    let got: Vec<(&str, bool)> = rows.iter().map(|(n, h)| (n.as_str(), *h)).collect();
    assert_eq!(got, expected, "the pane, row for row");

    for hidden in ["Dual Wield", "Tauren Racial", "GENERIC (DND)"] {
        assert!(
            !got.iter().any(|(n, _)| *n == hidden),
            "{hidden} must not appear"
        );
    }
    // Class lines read as proficiencies despite their 300/300.
    assert_eq!(
        s.eval::<i64>("local _,_,_,_,_,_,mx = GetSkillLineInfo(2) return mx")
            .unwrap(),
        1,
        "Beast Mastery's skillMaxRank"
    );
}

/// `SkillFrameCancelButton` is live: the XML comment around `SkillFrameAcceptButton` closes just
/// above it (`SkillFrame.xml:338-339`). It is 80x22, centred at the page's TOPLEFT + (305, -422).
#[test]
fn the_pages_close_button_sits_where_the_reference_seats_it_and_closes_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_skills_page();

    assert!(
        s.eval::<bool>("return SkillFrameCancelButton ~= nil")
            .unwrap(),
        "the ref's SkillFrameCancelButton is built"
    );
    assert!(
        s.eval::<bool>("return SkillFrameCancelButton:IsVisible()")
            .unwrap(),
        "and shown — nothing in the ref's SkillFrame.lua ever hides it"
    );

    let (page_top, page_left) = s
        .eval::<(f64, f64)>("return SkillFrame:GetTop(), SkillFrame:GetLeft()")
        .unwrap();
    let (top, bottom, left, right) = s
        .eval::<(f64, f64, f64, f64)>(
            "local b = SkillFrameCancelButton \
             return b:GetTop(), b:GetBottom(), b:GetLeft(), b:GetRight()",
        )
        .unwrap();
    assert_eq!(
        (
            (left + right) / 2.0 - page_left,
            (top + bottom) / 2.0 - page_top,
            right - left,
            top - bottom,
        ),
        (305.0, -422.0, 80.0, 22.0),
        "CENTER of the page's TOPLEFT at (305,-422), 80x22 (ref l.339-348)"
    );

    // `text=` looks up GlobalStrings (`0x703bf0`), else keeps the raw attribute (`0x778c31`,
    // `0x771032`); `CLOSE` is "Close" (`GlobalStrings.lua:760`).
    let label = page_quads(&s)
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color,
                ..
            } if t == "Close" => Some(*color),
            _ => None,
        })
        .expect("the Close label draws");
    assert_eq!(
        label,
        Some([1.0, 0.82, 0.0, 1.0]),
        "GameFontNormal — the UIPanelButtonTemplate face's own normal font"
    );

    // The stock `OnClick` hides the page and the window (`SkillFrame.xml:351-355`).
    s.run("SkillFrameCancelButton:Click()").unwrap();
    s.resolve();
    assert!(
        !s.eval::<bool>("return SkillFrame:IsVisible()").unwrap(),
        "the page hides"
    );
    assert!(
        !s.eval::<bool>("return CharacterFrame:IsVisible()").unwrap(),
        "and the window with it (ref l.352-355)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The fold inherits `SkillLabelTemplate`, the row font, and sits at `(-3, -3)` off the left cap's
/// right edge (`SkillFrame.xml:292-300`).
#[test]
fn the_collapse_all_fold_wears_the_row_font_and_the_references_seat() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = shown_skills_page();

    // Its `OnLoad` sets `ALL`, which is "All" (`SkillFrame.xml:305`, `GlobalStrings.lua:45`).
    let label = page_quads(&s)
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color,
                font_height,
                ..
            } if t == "All" => Some((*color, *font_height)),
            _ => None,
        })
        .expect("the All label draws");
    assert_eq!(
        label,
        (Some([1.0, 1.0, 1.0, 1.0]), Some(12.0)),
        "GameFontHighlight — the SAME face every SkillTypeLabel row wears"
    );

    let (tab_top, tab_bottom) = s
        .eval::<(f64, f64)>(
            "return SkillFrameExpandTabLeft:GetTop(), SkillFrameExpandTabLeft:GetBottom()",
        )
        .unwrap();
    let (btn_top, btn_bottom, btn_left) = s
        .eval::<(f64, f64, f64)>(
            "local b = SkillFrameCollapseAllButton \
             return b:GetTop(), b:GetBottom(), b:GetLeft()",
        )
        .unwrap();
    assert_eq!(
        (tab_top + tab_bottom) / 2.0 - (btn_top + btn_bottom) / 2.0,
        3.0,
        "the fold sits 3px BELOW the cap's centre (ref's -3); +3 above was the bug"
    );
    let cap_right = s
        .eval::<f64>("return SkillFrameExpandTabLeft:GetRight()")
        .unwrap();
    assert_eq!(
        btn_left - cap_right,
        -3.0,
        "and 3px back over the cap (ref's -3 on x)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Stock `SkillFrameExpandButtonFrame` sizes itself once, in `OnLoad`, to the label's width + 45
/// (`SkillFrame.xml:314`), so the measurer must be seated before the load.
#[test]
fn the_expand_tab_fits_its_label_at_load() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = shown_skills_page_with(Some(Box::new(super::FixedWidthFont(7.0))));
    assert_eq!(
        s.eval::<f64>("return SkillFrameExpandButtonFrame:GetWidth()")
            .unwrap(),
        7.0 * 3.0 + 45.0,
        "the ref's own law at load: the ALL label's width + 45"
    );
    // The middle slab spans the two caps, so the art follows the fit.
    let (mid_l, mid_r, cap_r_l) = s
        .eval::<(f64, f64, f64)>(
            "return SkillFrameExpandTabMiddle:GetLeft(), SkillFrameExpandTabMiddle:GetRight(), \
             SkillFrameExpandTabRight:GetLeft()",
        )
        .unwrap();
    assert_eq!(mid_r, cap_r_l, "the middle stretches to the right cap");
    assert_eq!(
        mid_r - mid_l,
        66.0 - 16.0,
        "and carries the whole span minus the two caps"
    );
    s.run("SkillFrame_UpdateSkills()").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return SkillFrameExpandButtonFrame:GetWidth()")
            .unwrap(),
        66.0
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `SkillListScrollFrame` is 220 tall (`SkillFrame.xml:468`), so its overflow `n * 15 - 220` is
/// 40px short of the bar's `(n - 12) * 15`; the reference stores the scroll value unclamped
/// (`0x786db0`), so the bar's end shows the last line.
#[test]
fn the_list_reaches_its_last_row_at_the_bars_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 60,
            race: Some("Human".into()),
            race_file: Some("Human".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..UnitState::default()
        }),
    );
    // 22 lines under 3 headers, 25 rows: enough that the tail lies past the overflow.
    let mut entries = Vec::new();
    for (i, name) in [
        "Axes",
        "Bows",
        "Crossbows",
        "Daggers",
        "Defense",
        "Guns",
        "Maces",
        "Polearms",
        "Staves",
        "Swords",
        "Thrown",
        "Two-Handed Axes",
        "Two-Handed Maces",
        "Two-Handed Swords",
        "Unarmed",
    ]
    .iter()
    .enumerate()
    {
        entries.push(skill(
            100 + i as u32,
            name,
            300,
            300,
            false,
            (6, "Weapon Skills", 5),
        ));
    }
    for (i, name) in ["Cloth", "Leather", "Mail", "Plate", "Shield"]
        .iter()
        .enumerate()
    {
        entries.push(skill(
            400 + i as u32,
            name,
            1,
            1,
            false,
            (8, "Armor Proficiencies", 6),
        ));
    }
    for (i, name) in ["Language: Common", "Language: Dwarven"].iter().enumerate() {
        entries.push(skill(
            500 + i as u32,
            name,
            300,
            300,
            false,
            (10, "Languages", 8),
        ));
    }
    s.set_skills(SkillsState { entries });
    s.run(r#"ToggleCharacter("SkillFrame")"#).unwrap();
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let n = s.eval::<i64>("return GetNumSkillLines()").unwrap();
    assert_eq!(n, 25, "22 lines under 3 headers");
    let (last_name, last_is_header) = s
        .eval::<(String, Option<i64>)>(&format!("local n, h = GetSkillLineInfo({n}) return n, h"))
        .unwrap();
    assert!(
        last_is_header.is_none(),
        "the tail row is a line, read off SkillRankFrame12"
    );

    // The control: the overflow is 40px shorter than the bar's range.
    let (_, bar_max) = s
        .eval::<(f64, f64)>("return SkillListScrollFrameScrollBar:GetMinMaxValues()")
        .unwrap();
    assert_eq!(bar_max, f64::from((n as i32 - 12) * 15));
    let overflow = s
        .eval::<f64>("return SkillListScrollFrame:GetVerticalScrollRange()")
        .unwrap();
    assert_eq!(overflow, f64::from(n as i32 * 15 - 220));
    assert!(
        overflow < bar_max,
        "the frame is taller than its twelve rows"
    );

    // The bar to its end, through `<OnVerticalScroll>` to `SkillFrame_UpdateSkills`.
    s.run(
        "local _, hi = SkillListScrollFrameScrollBar:GetMinMaxValues() \
         SkillListScrollFrameScrollBar:SetValue(hi)",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(SkillListScrollFrame)")
            .unwrap(),
        n - 12,
        "the row offset reaches the bar's end, not the overflow's"
    );
    assert_eq!(
        s.eval::<String>("return SkillRankFrame12SkillName:GetText()")
            .unwrap(),
        last_name,
        "the twelfth row is the block's last line"
    );
}
