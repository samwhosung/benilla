//! The stock **Skills tab** (`Interface\FrameXML\SkillFrame.xml`, off the player's chain since
//! 1956) driven end-to-end, engine-only (no Bevy) — the per-window test module the spellbook/trainer/bank files already establish, split
//! out of `character_tests.rs` (which owns the paperdoll + the tab round-trip) so the skills-pane
//! paint law has a home of its own.
//!
//! What it pins is the **proficiency row**: `SkillFrame.lua`'s own `skillMaxRank == 1` gate draws
//! a full GRAY bar with NO rank text, and two different kinds of line land in it — the armor
//! proficiencies, which the server really does report as `1/1`, and the **single-rank** lines
//! (class skills, Dual Wield, racials, the per-mount riding lines), whose `skillMaxRank` the
//! engine overrides to `1` off `SkillRaceClassInfo.flags & 0x400` however high the server's own
//! descriptor is (`benilla-ui`'s `SkillEntry::mono`; `0x4d3610`'s `4d38b1` branch). A
//! hunter's `Beast Mastery` on vmangos arrives as `300/300` and must still read gray and
//! numberless, exactly as it does in the real client.
//!
//! **The whole window is the reference's own** — `CharacterFrame.xml` and `PaperDollFrame.xml`
//! since 1751, the page itself since 1956 — so every test that opens the page loads
//! [`super::test_ui::CHARACTER_UI`] and opens with `wow_data_or_skip!()`.

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

/// The Skills page, shown, with one line of each shape — a hunter's real vmangos numbers:
/// `Beast Mastery 300/300` (single-rank, `SkillRaceClassInfo` 0x410), `Defense 12/60` (a normal
/// weapon line), `Cloth 1/1` (an armor proficiency the server itself caps).
///
/// The window around the page is the reference's own since 1751, so the file list is
/// [`super::test_ui::CHARACTER_UI`] — which already carries `SkillFrame.xml` (stock
/// `CharacterFrame_ShowSubFrame` hides all five pages by name, unguarded) and the
/// `UIPanelTemplates.xml` this list learned by hand a year ago: a MISSING template is a loader
/// *warning*, not an error, so an under-loaded list passes and then fails later on geometry that
/// silently never got built (`SkillDetailScrollFrame` inherits `UIPanelScrollFrameTemplate`).
/// [`super::test_ui::load_ui_strict`] is that lesson made into a check.
fn shown_skills_page() -> UiScript {
    shown_skills_page_with(None)
}

/// [`shown_skills_page`] with a text measurer seated BEFORE the files load — the state the app's
/// VM is in at world entry (the glue screens' frames seat it), which a stock `OnLoad` that measures
/// text relies on.
fn shown_skills_page_with(measurer: Option<Box<dyn benilla_ui::script::TextMeasure>>) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    if let Some(m) = measurer {
        s.set_text_measurer(m);
    }
    for f in super::test_ui::CHARACTER_UI {
        super::test_ui::load_ui_strict(&s, f);
    }
    // A player behind the window, carrying BOTH halves of the race and class pairs:
    // `UnitRace`/`UnitClass` answer `(localized, file)` or `nil, nil` — the binding `zip`s them —
    // and stock `PaperDollFrame_SetLevel` formats level, race and class into `CharacterLevelText`
    // unguarded (`PaperDollFrame.lua:100-104`) every time this window is shown. Ours never did,
    // which is why this page could be opened with no player at all.
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

/// Row slot `i`'s rank text and bar color — the two things the proficiency gate decides.
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

/// The quads **this page** draws — `s.extract()` narrowed to targets whose nearest named owner is
/// one of the page's own frames.
///
/// Not a convenience: since 1751 the window around the page is the reference's own, so a test VM
/// carries the whole [`super::test_ui::CHARACTER_UI`] — the action bar, the micro menu and the unit
/// frames are loaded and *visible*, and their quads land in the same extract. An unfiltered
/// "nothing draws at alpha 0.5" over that list asserts something about the action bar, not about
/// this page's troughs.
fn page_quads(s: &UiScript) -> Vec<benilla_ui::script::ExtractedQuad> {
    s.extract()
        .into_iter()
        .filter(|q| {
            s.target_owner_name(q.target)
                .is_some_and(|n| n.starts_with("Skill"))
        })
        .collect()
}

/// Row slot `i`'s **trough** colour — the `$parentBackground` texture behind the fill, which the
/// ref recolours per branch alongside the fill (`SkillFrame.lua:158` normal / `:167` proficiency).
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

    // Visible rows: 1 header "Class Skills", 2 Beast Mastery, 3 header "Weapon Skills",
    // 4 Defense, 5 header "Armor Proficiencies", 6 Cloth — one list slot each.
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

    // The armor proficiency reaches the same branch by the server's own 1/1.
    let (text, color) = row(&mut s, 6);
    assert_eq!(text, "", "Cloth is 1/1: no rank text");
    assert_eq!(color, [0.5, 0.5, 0.5, 1.0], "gray too");
    assert_eq!(row_bg(&mut s, 6), [1.0, 1.0, 1.0, 0.5], "white trough too");

    // The control: a normal weapon line still reads its numbers, in blue.
    let (text, color) = row(&mut s, 4);
    assert_eq!(text, "12/60", "Defense keeps its rank text");
    assert_eq!(color, [0.0, 0.0, 1.0, 0.5], "and the blue fill");
    assert_eq!(
        row_bg(&mut s, 4),
        [0.0, 0.0, 0.75, 0.5],
        "over the normal branch's DARK BLUE trough (ref SkillFrame.lua:158)"
    );

    // And what those troughs actually DRAW at. The template declares the texture `<Color 1,1,1,0.2>`
    // — a real texel — and the vertex colour MULTIPLIES it, alpha included (the composition law,
    // `benilla-ui` `script::tests::regions`): the proficiency's white reaches the screen at
    // `0.2 x 0.5 = 0.1`, not 0.5. Every solid-colour quad this page emits is one of these troughs.
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

    // Selecting the single-rank row paints the detail pane the same way (the shared PaintBar).
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

/// **The director's reference screenshot, reproduced row for row**. A real
/// level-60 tauren hunter's `PLAYER_SKILL_INFO` block — read straight out of the live vmangos
/// `character_skills` rows the A/B was taken on — fed through the app's own display predicate
/// ([`crate::ui_char::skills_row`]) and the engine's grouping, against the REAL shipped DBCs.
///
/// What the reference client shows for exactly this block is the expectation below: four headers,
/// fourteen rows — and **no** `Dual Wield`, `Tauren Racial` or `GENERIC (DND)`, and no `Secondary
/// Skills` header, though the server sends all three lines at 300/300 like any other. Skips
/// without client data.
#[test]
fn a_real_hunters_block_lists_exactly_what_the_reference_client_lists() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");

    // (skill line, value, max) — Twohunter's rows, verbatim. Tauren (race 6) hunter (class 3), 60.
    const BLOCK: &[(u16, u16, u16)] = &[
        (44, 300, 300),  // Axes
        (46, 300, 300),  // Guns
        (50, 300, 300),  // Beast Mastery       — single-rank
        (51, 300, 300),  // Survival            — single-rank
        (95, 300, 300),  // Defense
        (109, 300, 300), // Language: Orcish
        (115, 300, 300), // Language: Taurahe
        (118, 300, 300), // Dual Wield          — HIDDEN
        (124, 300, 300), // Tauren Racial       — HIDDEN
        (162, 300, 300), // Unarmed
        (163, 300, 300), // Marksmanship        — single-rank
        (173, 300, 300), // Daggers
        (183, 300, 300), // GENERIC (DND)       — HIDDEN
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

    // The three the server sends and the client never lists.
    for hidden in ["Dual Wield", "Tauren Racial", "GENERIC (DND)"] {
        assert!(
            !got.iter().any(|(n, _)| *n == hidden),
            "{hidden} must not appear"
        );
    }
    // And the class lines still read as proficiencies despite their 300/300 descriptor.
    assert_eq!(
        s.eval::<i64>("local _,_,_,_,_,_,mx = GetSkillLineInfo(2) return mx")
            .unwrap(),
        1,
        "Beast Mastery's skillMaxRank"
    );
}

/// The page's own **CLOSE** button. `SkillFrameCancelButton` is live in the
/// reference — the XML comment that swallows its `SkillFrameAcceptButton` neighbour closes one
/// line above it (ref `SkillFrame.xml` l.337/339) — and the director's screenshot of an empty
/// bottom-right seat is what that misread cost. Pins the button's existence, the ref's own seat
/// (80x22 centred on the page's TOPLEFT + (305,-422)), and that it closes the window.
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

    // The ref's geometry, read page-relative so the assertion is the ref's own numbers.
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

    // Its label is the CLOSE global string's seat, in the panel-button gold.
    // **"Close", not "CLOSE".** An XML `text=` attribute is a GlobalStrings LOOKUP, not a literal
    // (`loader::Loader::resolve_text` — `FrameScript_GetText 0x703bf0`), and `CLOSE = "Close"`
    // (`GlobalStrings.lua:760`). This test read "CLOSE" while the page's list carried no
    // `GlobalStrings.lua`: it was asserting the miss-fallback (the raw attribute), which is the
    // REFERENCE's own arm too (`0x778c31` / `0x771032`), not the label a client with the strings
    // loaded draws.
    // [`super::test_ui::CHARACTER_UI`] loads the player's own strings first, as the app does, so
    // this is the real label now.
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

    // And it does what the ref's OnClick does: page down, window down.
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

/// The **ALL** fold's face and seat. Both halves of the director's report: the
/// label is the row font — `GameFontHighlight`, WHITE at 12 — not `GameFontNormalSmall`'s yellow
/// 10, and the button rides 3px BELOW the tab cap's centre (the ref's own `(-3,-3)` off the left
/// cap), not 3px above it, which is where the offset copied from `TrainerFrame.xml` put it.
#[test]
fn the_collapse_all_fold_wears_the_row_font_and_the_references_seat() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = shown_skills_page();

    // "All", not "ALL" — the same GlobalStrings lookup as the CLOSE button above
    // (`ALL = "All"`, `GlobalStrings.lua:45`). The face is what this test is about, not the word.
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

/// The tab's **fit law** (ref `SkillFrameExpandButtonFrame`'s OnLoad:
/// `SetWidth(SkillFrameCollapseAllButton:GetTextWidth()+45)`), which the reference applies ONCE,
/// at load, with its font engine already up. The app's VM is in that state at world entry (the
/// glue screens seat the measurer on it), so the test seats one first; the transcription used to
/// guard a 0 measure and re-fit on the first Update, which the reference never does (1956).
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
    // The middle slab is the span between the two caps, so the fit reaches the art for free.
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
    // An Update leaves the fit alone — the reference sizes the tab at load and never again.
    s.run("SkillFrame_UpdateSkills()").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return SkillFrameExpandButtonFrame:GetWidth()")
            .unwrap(),
        66.0
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// **B370 — the list reaches its last rows.** The symptom: the Skills tab stops three rows short
/// of the end, the knob mid-track, Maces the last row shown. The reference's own
/// `FauxScrollFrame_Update` sizes the bar to `(n − 12) × 15` and the scroll child to `n × 15`, and
/// `SkillListScrollFrame` is 220 tall (stock `SkillFrame.xml` l.468) against twelve rows of
/// fifteen — so the child's overflow past the frame, `n × 15 − 220`, is forty pixels short of
/// where the bar goes. An engine that clamped `SetVerticalScroll` into that overflow stopped the
/// row offset at `n − 15`; the reference stores the bar's value as given.
///
/// Drives the bar to its end and reads the twelfth row: the block's last line. The control is the
/// pre-fix mechanism itself — the overflow really is shorter than the bar's range.
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
    // A warrior's kind of block — twenty-two lines under three headers, twenty-five rows: more than
    // twelve by more than the frame's slack, so the tail is only reachable past the overflow.
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

    // The reference's own numbers: the bar runs to (n − 12) × 15, the child's overflow past the
    // 220-tall frame is 40 px less — the control that the reported mechanism is the real one.
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

    // The knob dragged to the end: bar → SetVerticalScroll → <OnVerticalScroll> →
    // FauxScrollFrame_OnVerticalScroll → SkillFrame_UpdateSkills.
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
