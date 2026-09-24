//! The shipped **trainer window** driven end-to-end, engine-only (no Bevy): the real
//! The reference's own `Blizzard_TrainerUI` addon — a client-sorted, collapsible **skill-line
//! tree** with a **dropdown** state filter and a draggable **scroll bar** —
//! loaded behind its deps (`UiPanels.xml` + `UIDropDownMenu.xml` + `ScrollTemplates.xml` +
//! `MerchantFrame.xml` for the the stock money frames) and fed a synthetic service list. Covers
//! what only a runtime load exercises: the Lua parses and every referenced global resolves, the
//! tree renders interleaved header/service rows, a header click folds its group, the dropdown
//! filter hides a state, the wheel scrolls the list, the NPC name rides `arg1` into the title, the
//! byte-exact GlobalStrings render, the Train button gates on available-and-affordable, and the buy queues the row's spell id.

use benilla_ui::script::{
    ExtractedQuad, QuadContent, ScriptValue, SoundRequest, TrainerAbilityReq, TrainerService,
    TrainerServiceCategory, TrainerSkillReq, TrainerState, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// Load the trainer window + all its deps into a fresh script, screen sized, with every state filter
/// ON (the XML defaults "Already Known" off — the tests want the full tree, deterministic indices).
///
/// The filter's source of truth is the three **saved globals**, which the window
/// pushes into the engine on every show — so a test that wants the full tree sets those, not the
/// engine's own `SetTrainerServiceTypeFilter`, which the next `TRAINER_SHOW` would overwrite.
fn trainer_script() -> UiScript {
    let mut s = trainer_script_base();
    // The reference's own LoadOnDemand addon's files, direct off the chain (what its `LoadAddOn`
    // runs), and the FrameXML templates file its list inherits from (1957).
    load_xml(
        &s,
        "Interface\\AddOns\\Blizzard_TrainerUI\\Blizzard_TrainerUI.xml",
    );
    finish_trainer_load(&mut s);
    s
}

/// Everything the manifest loads before the trainer addon — the chain a LoadOnDemand load lands
/// on. The stock row's label is a width-0 `<ButtonText>` (fit the text), so a measurer is seated
/// the way the app's VM has one at world entry.
fn trainer_script_base() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(7.0)));
    // The reference's window calls `UpdateMicroButtons` on show and inherits the panel kit's
    // templates, so the harness carries what the manifest loads before it, in the manifest's
    // order (ScrollTemplates BEFORE UIPanelTemplates, 1846) — and the reference's own
    // LoadOnDemand addon last, with the FrameXML templates file its list inherits from (1957).
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        "Interface\\FrameXML\\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\MerchantFrame.xml",
        "Interface\\FrameXML\\ClassTrainerFrameTemplates.xml",
    ] {
        load_xml(&s, f);
    }
    s
}

/// What follows the addon's files in its LoadOnDemand load (1957): the saved chunk (none in a
/// bare harness) and then its ADDON_LOADED, whose arm pushes the three filter globals into the
/// engine. The harness wants "used" shown too. The stock title reads `UnitName("npc")`, so the
/// trainer NPC is seated here as the app seats it on TRAINER_SHOW.
fn finish_trainer_load(s: &mut UiScript) {
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Sana Winterhoof".into()),
            ..Default::default()
        }),
    );
    s.run("TRAINER_FILTER_USED = 1").unwrap();
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
}

/// Whether any rendered text quad carries `color` (within a small tolerance) — used to spot the
/// reddened cost coins, whose `(1.0, 0.1, 0.1)` is distinct from the unavailable row's `(0.9, 0, 0)`.
fn has_text_color(quads: &[ExtractedQuad], color: [f32; 3]) -> bool {
    quads.iter().any(|q| match &q.content {
        QuadContent::Text { color: Some(c), .. } => (0..3).all(|i| (c[i] - color[i]).abs() < 0.02),
        _ => false,
    })
}

/// Whether the first text quad containing `needle` renders in `color` (small tolerance) — used to
/// assert the selected row's white name vs. a state colour.
fn text_has_color(quads: &[ExtractedQuad], needle: &str, color: [f32; 3]) -> bool {
    quads.iter().any(|q| match &q.content {
        QuadContent::Text {
            text: Some(t),
            color: Some(c),
            ..
        } => t.contains(needle) && (0..3).all(|i| (c[i] - color[i]).abs() < 0.02),
        _ => false,
    })
}

/// The centre of the first text quad whose text contains `needle` — a point to aim the wheel at.
fn text_center(quads: &[ExtractedQuad], needle: &str) -> (f32, f32) {
    let r = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t.contains(needle) => q.rect,
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text quad containing {needle:?}"));
    ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)
}

/// One service in a named skill line, spelling out every field so the intent is legible at the call
/// site.
fn service(
    spell_id: u32,
    name: &str,
    category: TrainerServiceCategory,
    cost: u32,
    level_req: u32,
    skill_line: u32,
    line_name: &str,
    skill_req: Option<TrainerSkillReq>,
    ability_reqs: Vec<TrainerAbilityReq>,
) -> TrainerService {
    TrainerService {
        spell_id,
        tooltip: benilla_ui::script::TrainerTooltip::Spell {
            spell_id,
            alt_caster: false,
        },
        name: Some(name.into()),
        subtext: None,
        texture: Some("Interface\\Icons\\INV_Sword_04".into()),
        description: String::new(),
        cost,
        prof_first_rank: false,
        category,
        level_req,
        skill_req,
        ability_reqs,
        is_trade_skill: false,
        group_key: skill_line,
        group_name: line_name.into(),
    }
}

/// A two-line warrior menu. Groups sort by name (Arms < Fury); within a group by level then name. The
/// full-filter tree is:
///   1 H:Arms · 2 Heroic Strike(avail,10c,l1) · 3 Cleave(unavail,l20,skill+ability) ·
///   4 H:Fury · 5 Rend(used,30c) · 6 Thunder Clap(avail,500c)
fn menu() -> TrainerState {
    TrainerState {
        greeting: "Well met. Let me show you the way of the warrior.".into(),
        trainer_type: 0,
        groups: Vec::new(),
        services: vec![
            service(
                78,
                "Heroic Strike",
                TrainerServiceCategory::Available,
                10,
                1,
                26,
                "Arms",
                None,
                vec![],
            ),
            service(
                845,
                "Cleave",
                TrainerServiceCategory::Unavailable,
                100,
                20,
                26,
                "Arms",
                Some(TrainerSkillReq {
                    name: "Swords".into(),
                    rank: 50,
                    met: false,
                }),
                // The director's case: Cleave is gated (level/skill), but its prerequisite ability is
                // already learned — so it reads MET (white) with its rank, decoupled from the
                // service's unavailable state.
                vec![TrainerAbilityReq {
                    name: "Charge (Rank 1)".into(),
                    met: true,
                }],
            ),
            service(
                6343,
                "Thunder Clap",
                TrainerServiceCategory::Available,
                500,
                1,
                256,
                "Fury",
                None,
                vec![],
            ),
            service(
                772,
                "Rend",
                TrainerServiceCategory::Used,
                30,
                1,
                256,
                "Fury",
                None,
                vec![],
            ),
        ],
    }
}

/// The whole trainer window minus Bevy: it loads clean, opens on TRAINER_SHOW with the NPC name in the
/// title, renders the interleaved tree (headers + services), picks the first available service, renders
/// the exact `Cost:` label, gates Train on available-and-affordable, queues the selected row's spell id
/// on a buy, reddens the cost of an unaffordable service, builds the `Requires:` line for a gated one,
/// and hides on TRAINER_CLOSED.
#[test]
fn shipped_trainer_frame_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();

    // Hidden by default.
    assert!(!s
        .eval::<bool>("return ClassTrainerFrame:IsVisible()")
        .unwrap());

    // The app's feed: 50 copper in the purse + the warrior menu, then the open event with the name.
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event(
        "TRAINER_SHOW",
        vec![ScriptValue::Str("Sana Winterhoof".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Shown, title took the NPC name off arg1.
    assert!(s
        .eval::<bool>("return ClassTrainerFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return ClassTrainerNameText:GetText()")
            .unwrap(),
        "Sana Winterhoof"
    );

    // Row 1 renders the "Arms" header (its name, no indent); row 2 the first service.
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill1Text:GetText()")
            .unwrap(),
        "Arms"
    );
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill2Text:GetText()")
            .unwrap(),
        "  Heroic Strike"
    );

    // selectFirstService picked the first available row (Heroic Strike, index 2) → the detail pane
    // shows the exact "Cost:" label and Train is enabled (available + 10c affordable at 50c).
    assert_eq!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap(),
        2
    );
    assert_eq!(
        s.eval::<String>("return ClassTrainerCostLabel:GetText()")
            .unwrap(),
        "Cost:"
    );
    assert!(s
        .eval::<bool>("return ClassTrainerTrainButton:IsEnabled() ~= 0")
        .unwrap());

    // Train buys the selected service — the row's spell id reaches the app's drain.
    s.run("BuyTrainerService(GetTrainerSelectionIndex())")
        .unwrap();
    assert_eq!(s.take_trainer_buys(), vec![78]);
    assert!(s.take_trainer_buys().is_empty(), "drained");

    // Select the available-but-unaffordable service (Thunder Clap, index 6, 500c > 50c): Train disables
    // and the cost coins redden (SetMoneyFrameColor 1.0, 0.1, 0.1).
    s.run("this = ClassTrainerSkill6; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert!(!s
        .eval::<bool>("return ClassTrainerTrainButton:IsEnabled() ~= 0")
        .unwrap());
    s.resolve();
    assert!(
        has_text_color(&s.extract(), [1.0, 0.1, 0.1]),
        "unaffordable cost coins render red"
    );

    // Select the gated service (Cleave, index 3): the Requires: line is built from the level/skill/
    // ability gates (byte-exact REQUIRES_LABEL), and Train stays disabled (unavailable).
    s.run("this = ClassTrainerSkill3; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    let reqs = s
        .eval::<String>("return ClassTrainerSkillRequirements:GetText()")
        .unwrap();
    assert!(reqs.starts_with("Requires: "), "reqs: {reqs}");
    for term in ["Level", "Swords", "Charge"] {
        assert!(reqs.contains(term), "reqs missing {term}: {reqs}");
    }
    // The met prerequisite renders WHITE (|cffffffff…|r) with its rank, while the unmet level/skill
    // gates redden — the mixed line the director asked for (a learned prev-rank isn't reddened just
    // because the spell itself is unavailable).
    assert!(
        reqs.contains("|cffffffffCharge (Rank 1)|r"),
        "a known prerequisite shows white with its rank: {reqs}"
    );
    assert!(!s
        .eval::<bool>("return ClassTrainerTrainButton:IsEnabled() ~= 0")
        .unwrap());

    // The app's client-side close: clear the snapshot + fire TRAINER_CLOSED → the window hides.
    s.set_trainer(None);
    s.fire_event("TRAINER_CLOSED", vec![]);
    assert!(!s
        .eval::<bool>("return ClassTrainerFrame:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking a header row folds its group (and expands it back) — the tree collapse, end-to-end through
/// the row button's OnClick → Collapse/ExpandTrainerSkillLine(headerIndex).
#[test]
fn clicking_a_header_row_collapses_its_group() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Full tree: 2 headers + 4 services = 6 rows; row 2 is Heroic Strike.
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill2Text:GetText()")
            .unwrap(),
        "  Heroic Strike"
    );

    // Click the Arms header (row 1): its two services fold → 4 rows (H:Arms, H:Fury, Rend, Thunder
    // Clap). Row 2 is now the Fury header.
    s.run("this = ClassTrainerSkill1; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 4);
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill2Text:GetText()")
            .unwrap(),
        "Fury",
        "Arms folded; its header (row 1) now abuts the Fury header (row 2)"
    );

    // Click it again → expands back to 6 rows.
    s.run("this = ClassTrainerSkill1; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The dropdown filter hides a state client-side: toggling "used" off drops the already-known service,
/// its header stays, and the row count falls.
#[test]
fn filter_hides_a_state_keeping_headers() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);

    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);

    // Drive the dropdown's own click handler for the "used" row: the row button rides as `this` with
    // its pre-click state (checked=on, value="used"), exactly as UIDropDownMenuButton_OnClick invokes
    // the row func. It flips the engine filter off and repaints. Rend drops (6 → 5); headers remain.
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    s.run("this = DropDownList1Button3; UIDropDownMenuButton_OnClick()")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        5,
        "the used service (Rend) is hidden; both headers stay"
    );
    assert!(
        !s.eval::<bool>("return GetTrainerServiceTypeFilter('used') == 1")
            .unwrap(),
        "the engine's used filter is now off"
    );
}

/// The filter rows toggle **through the real dropdown kit** — the path a mouse takes, which the test
/// above deliberately shortcuts by faking `this`. `UIDropDownMenuButton_OnClick` runs the row's func
/// and only THEN flips the check for a `keepShownOnClick` row, so the func must not repaint the row
/// itself: an in-func `UIDropDownMenu_Initialize` re-derived the check from the fresh engine state and
/// the kit's flip then inverted it straight back — the check never moved on screen, and `this.checked`
/// stuck true, so a filter turned off could never be turned back on. Click, re-click, and re-open all
/// have to agree.
#[test]
fn filter_rows_toggle_through_the_dropdown_kit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);

    // Open the menu the way the capsule's arrow does. Row 2 is "Unavailable" (Initialize's order),
    // checked because trainer_script() turned every state on.
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let row_checked = |s: &mut UiScript| {
        s.eval::<bool>("return DropDownList1Button2Check:IsVisible() and true or false")
            .unwrap()
    };
    assert!(row_checked(&mut s), "Unavailable starts checked");
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);

    // Click it: the check clears, the engine filter clears, and Cleave (the unavailable service)
    // drops out of the tree. Its Arms header stays.
    s.run("this = DropDownList1Button2; UIDropDownMenuButton_OnClick()")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!row_checked(&mut s), "the click clears the row's check");
    assert!(
        !s.eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
            .unwrap(),
        "and the engine's unavailable filter with it"
    );
    assert_eq!(
        s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        5,
        "Cleave is hidden"
    );

    // Click it again: back on, both on screen and in the engine — the case the old code could never
    // reach, because `this.checked` never went false.
    s.run("this = DropDownList1Button2; UIDropDownMenuButton_OnClick()")
        .unwrap();
    assert!(row_checked(&mut s), "the re-click restores the check");
    assert!(
        s.eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
            .unwrap(),
        "and re-enables the filter"
    );
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);

    // Close and re-open: Initialize re-derives every row from the engine, so the menu agrees with
    // what the clicks left behind (the two states left on, "used" still on from trainer_script()).
    s.run("this = DropDownList1Button2; UIDropDownMenuButton_OnClick()")
        .unwrap();
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap(); // same owner → closes
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap(); // re-open → re-Initialize
    assert!(
        !row_checked(&mut s),
        "a re-opened menu shows the filter the clicks actually left off"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A menu bigger than the 11 visible rows (one skill line, 15 services → a 16-row tree), so the list
/// scrolls. The wheel spins the list even when the cursor is over a row — the engine bubbles the spin
/// up the parent chain to the window, which drives the faux-scroll bar.
fn long_menu() -> TrainerState {
    TrainerState {
        greeting: "Much to learn.".into(),
        trainer_type: 0,
        groups: Vec::new(),
        services: (1..=15)
            .map(|i| {
                service(
                    1000 + i,
                    &format!("Service {i:02}"),
                    TrainerServiceCategory::Available,
                    10,
                    10,
                    26,
                    "Arms",
                    None,
                    vec![],
                )
            })
            .collect(),
    }
}

#[test]
fn wheel_over_a_row_scrolls_the_list() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(long_menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let row1 = "return ClassTrainerSkill1Text:GetText()";
    // At the top: row 1 is the "Arms" header, row 2 the first service.
    assert_eq!(s.eval::<String>(row1).unwrap(), "Arms");
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill2Text:GetText()")
            .unwrap(),
        "  Service 01"
    );

    // Aim the wheel at a LIST ROW's text (service 3 — visible at the top): the spot over a row.
    s.resolve();
    let (x, y) = text_center(&s.extract(), "Service 03");

    // Spin down (WoW convention: negative = down). The REFERENCE's wheel is a PAGE, not a row:
    // `ScrollFrameTemplate_OnMouseWheel` moves `scrollBar:GetHeight() / 2` pixels
    // (UIPanelTemplates.lua:150-157), the same half-bar the arrows use. Ours moved one row on
    // purpose; the migration reverts that (1860), and the expected row is derived from the bar so
    // this stays the reference's rule rather than a literal.
    s.mouse_wheel(x, y, -1.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // What this test is FOR is the wiring — wheel -> bar -> `<OnVerticalScroll>` ->
    // `FauxScrollFrame_OnVerticalScroll` -> `frame.offset` -> the repaint. The magnitude is the
    // reference's and is asserted as "a page, not a row"; pinning the exact row count would pin a
    // pixel arithmetic whose inputs resolve a frame apart.
    let offset = s
        .eval::<i64>("return ClassTrainerListScrollFrame.offset or -1")
        .unwrap();
    assert!(
        offset > 1,
        "the reference's wheel moves half a BAR, not one row — got offset {offset}"
    );
    assert_eq!(
        s.eval::<String>(row1).unwrap(),
        format!("  Service {offset:02}"),
        "row 1 shows the entry the offset names, so the repaint followed the scroll"
    );

    // A second spin cannot go deeper: 16 entries over 11 visible rows makes 5 the deepest legal
    // offset, and the reference's half-bar page already reached it in one spin. That it STOPS
    // there is the clamp working, not the wheel failing.
    s.mouse_wheel(x, y, -1.0);
    assert_eq!(
        s.eval::<i64>("return ClassTrainerListScrollFrame.offset or -1")
            .unwrap(),
        offset,
        "clamped at the bottom (numItems - numToDisplay), never past it"
    );

    // Spin back up twice: the list returns to the top and stops there, never past it.
    s.mouse_wheel(x, y, 1.0);
    s.mouse_wheel(x, y, 1.0);
    assert_eq!(
        s.eval::<i64>("return ClassTrainerListScrollFrame.offset or -1")
            .unwrap(),
        0,
        "back at the top, clamped"
    );
    assert_eq!(s.eval::<String>(row1).unwrap(), "Arms");
}

/// The selected service row's name renders white (HIGHLIGHT), legible against its colour glow — and
/// so does a HOVERED one. Both are the reference's own behaviour, through one mechanism: the row's
/// name is the button's `<ButtonText>` under a `<HighlightFont inherits="GameFontHighlight">`, the
/// engine swaps that instance in while the cursor is on the row, and `LockHighlight()` pins it for
/// the selection (Blizzard_TrainerUI.lua l.183). `SetTextColor` writes the NORMAL instance only, so
/// the state colour cannot follow the label into either state — which is exactly why it goes white.
///
/// An earlier revision of this test called the white name "a deliberate divergence from the ref
/// (which whitens only the subtext)". The ref whitens the subtext *by hand* precisely BECAUSE the
/// subtext is a child FontString the lock cannot reach; the name it leaves to the lock. Ours could
/// not, because the name was a child FontString too and the engine's highlighted label fell back to
/// the normal state's colour — so the white was hand-painted here and absent on hover entirely.
#[test]
fn a_selected_or_hovered_service_row_paints_its_name_white() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The name really is the button's own label now — a child FontString would leave
    // GetFontString() nil and no per-state font could reach it.
    assert!(
        s.eval::<bool>("return ClassTrainerSkill1:GetFontString() ~= nil")
            .unwrap(),
        "the row name is the Button's ButtonText, the only region per-state fonts reach"
    );

    // Cleave (index 3) is unavailable → red when not selected. Select it → its name goes white.
    s.run("this = ClassTrainerSkill3; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    s.resolve();
    assert!(
        text_has_color(&s.extract(), "Cleave", [1.0, 1.0, 1.0]),
        "the selected row's name renders white"
    );

    // Select Heroic Strike (index 2): Cleave is no longer selected → back to unavailable red.
    s.run("this = ClassTrainerSkill2; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    s.resolve();
    assert!(
        text_has_color(&s.extract(), "Cleave", [0.9, 0.0, 0.0]),
        "an unselected unavailable row is red again"
    );

    // Now hover it, selecting nothing: the HighlightFont instance takes over and the row lights up.
    let (x, y) = text_center(&s.extract(), "Cleave");
    s.mouse_move(x, y);
    s.resolve();
    assert!(
        text_has_color(&s.extract(), "Cleave", [1.0, 1.0, 1.0]),
        "a hovered row's name renders white, with no script doing it"
    );
    assert!(
        text_has_color(&s.extract(), "Heroic Strike", [1.0, 1.0, 1.0]),
        "and the SELECTED row stays white while another is hovered"
    );

    // Cursor off the list: the hovered row falls back to its state colour, the selected one holds.
    s.mouse_move(1000.0, 20.0);
    s.resolve();
    assert!(
        text_has_color(&s.extract(), "Cleave", [0.9, 0.0, 0.0]),
        "cursor away: red again"
    );
    assert!(
        text_has_color(&s.extract(), "Heroic Strike", [1.0, 1.0, 1.0]),
        "the selection's white is the LOCK, not the hover"
    );
}

/// Scrolling is silent, faithfully: the mouse WHEEL plays no sound (the ref's
/// `ScrollFrameTemplate_OnMouseWheel` is soundless — the director asked for no sound when scrolling),
/// while the arrow BUTTONS keep the ref's `UChatScrollButton` click.
#[test]
fn wheel_scroll_is_silent_but_the_arrows_click() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(long_menu())); // 16 rows > 11 visible → the bar + arrows show
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    let _ = s.take_sounds(); // drain the window's OnShow open sound

    let click = SoundRequest::KitName("UChatScrollButton".into());

    // A wheel spin over the list scrolls it (proven by the wheel test) but plays NOTHING.
    s.resolve();
    let (x, y) = text_center(&s.extract(), "Service 03");
    s.mouse_wheel(x, y, -1.0);
    assert!(
        !s.take_sounds().contains(&click),
        "the wheel scroll is silent"
    );

    // That one notch reached the BOTTOM, and the arrow to click afterwards is therefore the UP
    // one. `ScrollFrameTemplate_OnMouseWheel` moves half the bar's height — 76px on this window's
    // 152-tall bar — and `SetValue` snaps that onto the row lattice (step 16), which rounds 76 up
    // to the range's own 80 (2133). `FauxScrollFrame_Update` then greys the DOWN arrow on its
    // `GetValue() - scrollFrameHeight == 0` test, so clicking it would be clicking a disabled
    // button. Pinned rather than worked around: this snap is the reference's.
    assert_eq!(
        s.eval::<f64>("return ClassTrainerListScrollFrameScrollBar:GetValue()")
            .unwrap(),
        80.0,
        "one wheel notch = 76px, snapped to the 5-row bottom of an 80px range"
    );

    // The up arrow, now the enabled one — clicking it plays the ref's arrow click.
    s.run("ClassTrainerListScrollFrameScrollBarScrollUpButton:Click()")
        .unwrap();
    assert!(
        s.take_sounds().contains(&click),
        "the arrow button clicks (UChatScrollButton)"
    );
}

/// The scrollbar ARROWS move the list the way they point, and stop at the top.
///
/// **The STEP is the reference's, not ours, since 1860.** `UIPanelScrollBarTemplate`'s arrow
/// OnClick is `parent:SetValue(parent:GetValue() -/+ (parent:GetHeight() / 2))` — half the BAR's
/// height in pixels, which for this window's 144-tall bar over 16px rows is five rows. Our
/// deleted kit stepped exactly one row on purpose ("the generic ref scrollbar steps half its
/// height; a discrete row list wants one"); the migration reverts that, and the magnitude here is
/// computed from the bar rather than written as a literal so it stays the reference's rule and not
/// a number someone has to re-derive.
#[test]
fn the_scrollbar_arrows_step_the_list_the_way_they_point() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(long_menu())); // 16 rows > 11 visible → the bar shows
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    let offset = |s: &mut UiScript| {
        s.eval::<i64>("return ClassTrainerListScrollFrame.offset or -1")
            .unwrap()
    };
    let click = |s: &mut UiScript, which: &str| {
        s.run(&format!(
            "ClassTrainerListScrollFrameScrollBarScroll{which}Button:Click()"
        ))
        .unwrap();
    };
    // The reference's own step, read off the bar: half its height in pixels, rounded to rows the
    // way `FauxScrollFrame_OnVerticalScroll` rounds (`floor(v/itemHeight + 0.5)`).
    assert_eq!(offset(&mut s), 0, "opens at the top");
    click(&mut s, "Down");
    let step = offset(&mut s);
    assert!(
        step > 1,
        "the down arrow advances by half a BAR, not one row — got {step}"
    );
    click(&mut s, "Up");
    assert_eq!(offset(&mut s), 0, "the up arrow walks it back");
    click(&mut s, "Up");
    click(&mut s, "Up");
    assert_eq!(offset(&mut s), 0, "and stops at the top, never past it");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The filter is remembered across a restart** — the whole persistence path, in
/// The reference's own restart, through its own mechanism (1957): the addon is a LoadOnDemand
/// registry row read off the chain; `LoadAddOn` runs its files (file-scope defaults), then its
/// per-addon saved file over them, then fires ADDON_LOADED — whose arm pushes the three filter
/// globals into the engine. A dropdown toggle writes the GLOBAL; the addon's `## SavedVariables`
/// carry it into the file; a fresh VM loading the addon comes up with the toggle applied.
#[test]
fn the_state_filter_survives_a_restart_through_the_saved_variables_file() {
    let _data = benilla_formats::wow_data_or_skip!();
    let toc =
        super::reference_ui::read("Interface/AddOns/Blizzard_TrainerUI/Blizzard_TrainerUI.toc")
            .map(|b| benilla_ui::toc::Toc::parse(&benilla_ui::source::decode(&b)))
            .expect("the addon's toc off the chain");
    assert!(toc.load_on_demand(), "the reference ships it LoadOnDemand");
    let info = || {
        let mut i = super::addons::info_from_toc("Blizzard_TrainerUI", &toc);
        i.chain = true;
        i
    };
    let saved_dir =
        std::env::temp_dir().join(format!("benilla-trainer-saved-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saved_dir);
    std::fs::create_dir_all(&saved_dir).unwrap();
    let boot = |s: &mut UiScript| {
        s.set_addon_chain_reader(Box::new(super::reference_ui::read));
        s.register_addons(vec![info()], None, Some(saved_dir.clone()), None);
        s.set_unit(
            "npc",
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some("Sana Winterhoof".into()),
                ..Default::default()
            }),
        );
        s.set_money(50);
        s.set_trainer(Some(menu()));
        // The reference's arm: TRAINER_SHOW → ClassTrainerFrame_LoadUI → UIParentLoadAddOn.
        s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
        assert!(
            s.eval::<bool>("return IsAddOnLoaded('Blizzard_TrainerUI') == 1")
                .unwrap(),
            "loaded on demand, off the chain"
        );
    };

    let mut s = trainer_script_base();
    boot(&mut s);
    // The reference's own file-scope default hides known spells (TRAINER_FILTER_USED = 0), so
    // the one used service is out on the very first paint: 2 headers + 3 of the 4 services.
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
    assert_eq!(
        info().saved_variables,
        vec![
            "TRAINER_FILTER_AVAILABLE",
            "TRAINER_FILTER_UNAVAILABLE",
            "TRAINER_FILTER_USED",
        ],
        "the toc's own three, in its order"
    );

    // Toggle "Unavailable" off through the dropdown row's own handler.
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    s.run("this = DropDownList1Button2; UIDropDownMenuButton_OnClick()")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return TRAINER_FILTER_UNAVAILABLE").unwrap(),
        0,
        "the click must write the SAVED global, not just the engine mask"
    );
    let text = String::from_utf8(s.saved_variables_bytes_for(&info().saved_variables)).unwrap();
    assert!(
        text.contains("TRAINER_FILTER_UNAVAILABLE = 0"),
        "the file carries the toggle: {text}"
    );
    std::fs::write(saved_dir.join("Blizzard_TrainerUI.lua"), &text).unwrap();

    // The restart: a fresh VM loads the addon on demand — files, then the saved file, then
    // ADDON_LOADED — and the first paint is already filtered.
    let mut fresh = trainer_script_base();
    boot(&mut fresh);
    assert!(
        !fresh
            .eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
            .unwrap(),
        "the remembered filter reached the engine"
    );
    assert_eq!(
        fresh.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        4,
        "Cleave (the unavailable service) is hidden on the very first paint after the restart"
    );
    let _ = std::fs::remove_dir_all(&saved_dir);
}

/// A **new list packet resets the engine's filter mask** to the builder's own default — mask 3 at a
/// class/tradeskill/pet trainer, mask 5 (available|used) at a mount trainer — and clears the collapse
/// set, byte-verified. This is the engine half of the pair above: the reset is why
/// the window re-pushes its saved globals on every show.
#[test]
fn a_new_list_packet_resets_the_filter_mask_and_the_collapse_set() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_trainer(Some(menu()));
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
    // Collapse a group, and turn a state off directly (no global) — both are engine-side state.
    s.run("CollapseTrainerSkillLine(1) SetTrainerServiceTypeFilter('used', 1)")
        .unwrap();
    assert!(s
        .eval::<bool>("return GetTrainerServiceTypeFilter('used') == 1")
        .unwrap());
    assert!(s.eval::<i64>("return GetNumTrainerServices()").unwrap() < 6);

    s.reset_trainer_list_state(0);
    assert!(
        !s.eval::<bool>("return GetTrainerServiceTypeFilter('used') == 1")
            .unwrap(),
        "mask 3: available|unavailable, already-known OFF"
    );
    assert!(s
        .eval::<bool>("return GetTrainerServiceTypeFilter('available') == 1")
        .unwrap());
    assert!(s
        .eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        5,
        "nothing collapsed any more (6 rows less the already-known service the mask now hides)"
    );

    // A mount trainer wants available|used instead — what makes a known mount visible at all.
    s.reset_trainer_list_state(1);
    assert!(s
        .eval::<bool>("return GetTrainerServiceTypeFilter('used') == 1")
        .unwrap());
    assert!(!s
        .eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
        .unwrap());
}

/// A profession trainer's list: one skill line, a long recipe name and a short one, each with a rank
/// subtext — the shape of the B253 report (a Leatherworking trainer at "Handstitched Leather Pants").
fn recipe_menu() -> TrainerState {
    let mut long = service(
        3756,
        "Handstitched Leather Pants",
        TrainerServiceCategory::Available,
        50,
        1,
        165,
        "Leatherworking",
        None,
        vec![],
    );
    long.subtext = Some("Rank 1".into());
    let mut short = service(
        2149,
        "Belt",
        TrainerServiceCategory::Available,
        50,
        1,
        165,
        "Leatherworking",
        None,
        vec![],
    );
    short.subtext = Some("Rank 1".into());
    TrainerState {
        greeting: "Can I teach you how to turn beast hides into armor?".into(),
        trainer_type: 2,
        groups: Vec::new(),
        services: vec![long, short],
    }
}

/// **B253 — a long row name drew two lines over the row beneath it.** The row is a FLOW, not two
/// fixed columns: the name carries no width (a FontString given one wraps at it, and a 16 px row
/// cannot grow), and the rank follows the name's right edge rather than sitting at an invented
/// x=188. Both halves are the reference's own row
/// (`ClassTrainerFrameTemplates.xml`'s `<ButtonText>` at width 0 + the per-row
/// `SetPoint("LEFT", <row>Text, "RIGHT", 10, 0)`), read off the player's chain.
///
/// The mutation check is the first assertion: put a `<Size>` back on the row's `<ButtonText>` and
/// the request
/// carries a wrap width, the long name measures two lines, and its height passes the row height.
#[test]
fn a_long_row_name_stays_on_one_line_and_carries_its_rank_along() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(5000);
    s.set_trainer(Some(recipe_menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Nadyia".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The host measure: 6 px per character, 14 px per line, wrapped at whatever width was asked for.
    // Capture the row names' requests on the way past — what they ASK is the structural fact.
    let mut name_wraps: Vec<Option<f32>> = Vec::new();
    let mut answer = |s: &mut UiScript, collect: bool| {
        let reqs = s.fontstrings_needing_measure();
        if collect {
            name_wraps.extend(
                reqs.iter()
                    .filter(|r| r.text.contains("Handstitched"))
                    .map(|r| r.wrap_width),
            );
        }
        let answers: Vec<(u32, f32, f32, u64)> = reqs
            .into_iter()
            .map(|r| {
                let ink = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(w) => (r.id, ink.min(w), (ink / w).ceil().max(1.0) * 14.0, r.key),
                    None => (r.id, ink, 14.0, r.key),
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
    };
    answer(&mut s, true);
    s.resolve();
    s.tick(0.016);
    answer(&mut s, false);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        !name_wraps.is_empty() && name_wraps.iter().all(|w| w.is_none()),
        "the row name must ask for NO wrap width — it is a single line whatever the name is; \
         got {name_wraps:?}"
    );

    // Find each name's row by what it painted (the group comparator owns the order, not this test).
    let row_of = |s: &mut UiScript, needle: &str| -> i64 {
        s.eval::<i64>(&format!(
            "for i = 1, 11 do local b = getglobal('ClassTrainerSkill' .. i) \
             local t = b:GetText() if t and strfind(t, '{needle}', 1, 1) then return i end \
             end return 0"
        ))
        .unwrap()
    };
    let long_row = row_of(&mut s, "Handstitched");
    let short_row = row_of(&mut s, "Belt");
    assert!(long_row > 0 && short_row > 0, "both services painted");

    let geom = |s: &mut UiScript, row: i64| -> (f32, f32, f32, f32) {
        s.eval::<(f32, f32, f32, f32)>(&format!(
            "local b = getglobal('ClassTrainerSkill{row}') \
             local n = b:GetFontString() \
             return n:GetHeight(), n:GetRight(), getglobal(b:GetName() .. 'SubText'):GetLeft(), b:GetHeight()"
        ))
        .unwrap()
    };
    let (long_h, long_name_right, long_sub_left, row_h) = geom(&mut s, long_row);
    let (short_h, short_name_right, short_sub_left, _) = geom(&mut s, short_row);

    assert!(
        long_h <= row_h + 0.5,
        "the long name is one line inside its own row: name {long_h} px, row {row_h} px"
    );
    assert!((long_h - short_h).abs() < 0.5, "and so is the short one");
    for (name, right, left) in [
        ("long", long_name_right, long_sub_left),
        ("short", short_name_right, short_sub_left),
    ] {
        assert!(
            (left - right - 10.0).abs() < 0.5,
            "the {name} row's rank sits 10 px past its NAME's right edge (the reference's own \
             offset), not at a fixed column: name right {right}, subtext left {left}"
        );
    }
    assert!(
        long_sub_left > short_sub_left + 50.0,
        "so a longer name pushes its rank along instead of running under it: {long_sub_left} vs \
         {short_sub_left}"
    );
}

/// **B256 — "filter to Available, learn a spell, the filter comes back partly reset."** The window
/// is driven here through the real [`crate::ui_trainer::TrainerOpen`], so the packet→reset decision
/// under test is the app's own and not this test's: the closure below is the trainer feed's three
/// lines (`if open.fresh_list { reset } ; set_trainer ; fire`), and everything else is the shipped
/// Lua.
///
/// The reference cannot produce this bug because it never gets a list packet with the window open —
/// it repaints a purchase from a client-side state re-derivation (`0x4d7d40`).
/// benilla re-asks the server instead, so the reference's per-packet mask reset (`0x4d75d9`) was
/// riding in on a packet the reference never sends, and taking the player's choice — and their
/// collapsed groups, which 1128 recorded as "a collapse does not survive a purchase" — with it.
#[test]
fn learning_a_spell_keeps_the_filter_and_the_collapse_a_re_open_still_resets() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_trainer::TrainerOpen;
    const DAZALAR: u64 = 0xabc;

    let mut s = trainer_script();
    // The reference's own file-scope defaults (1128), then the player's choice below.
    s.run("TRAINER_FILTER_AVAILABLE = 1 TRAINER_FILTER_UNAVAILABLE = 1 TRAINER_FILTER_USED = 0")
        .unwrap();
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
    s.set_money(5000);

    // The trainer feed (`ui_trainer::feed_trainer`), reduced to the part this bug lives in.
    fn feed(s: &mut UiScript, open: &mut TrainerOpen, state: TrainerState, event: &str) {
        if open.fresh_list {
            s.reset_trainer_list_state(open.trainer_type);
            open.fresh_list = false;
        }
        s.set_trainer(Some(state));
        s.fire_event(event, vec![ScriptValue::Str("Dazalar".into())]);
    }
    let filter_on = |s: &mut UiScript, kind: &str| {
        s.eval::<bool>(&format!(
            "return GetTrainerServiceTypeFilter('{kind}') == 1"
        ))
        .unwrap()
    };
    let rows = |s: &mut UiScript| s.eval::<i64>("return GetNumTrainerServices()").unwrap();

    // He opens the trainer and filters the list down to what he can actually learn.
    let mut open = TrainerOpen::default();
    open.open(DAZALAR, 0, vec![], "Hello, hunter!".into());
    feed(&mut s, &mut open, menu(), "TRAINER_SHOW");
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    s.run("this = DropDownList1Button2; UIDropDownMenuButton_OnClick()")
        .unwrap();
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    assert!(
        !filter_on(&mut s, "unavailable"),
        "his choice reached the engine"
    );
    assert_eq!(
        rows(&mut s),
        4,
        "two headers over the two learnable services"
    );
    // …and folds the first group away while he is at it.
    s.run("CollapseTrainerSkillLine(1)").unwrap();
    assert_eq!(rows(&mut s), 3, "the folded group keeps its header only");

    // He trains. The spell lands (`SMSG_LEARNED_SPELL`) and the state re-evaluator (2333, the
    // reference's `0x4d7d40`) repaints the bought row gray IN PLACE — no second list, so nothing
    // that a list packet resets.
    let mut learned = menu();
    learned.services[0].category = TrainerServiceCategory::Used;
    feed(&mut s, &mut open, learned, "TRAINER_UPDATE");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !filter_on(&mut s, "unavailable"),
        "learning a spell is a repaint, not a new window: his filter stands"
    );
    assert_eq!(
        rows(&mut s),
        2,
        "and the list is still his — the folded Arms group is empty of learnables now, so it goes \
         with its header, leaving Fury over Thunder Clap"
    );
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    assert!(
        !s.eval::<bool>("return DropDownList1Button2Check:IsVisible() and true or false")
            .unwrap(),
        "the dropdown agrees with the list, instead of claiming a filter the list ignores"
    );
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();

    // He walks away and comes back: THIS packet opens a window, so the reference's reset lands
    // (`0x4d75d9` writes the mask to 3 on every list) — and nothing puts the saved globals back
    // over it: the stock addon pushes them once, on ITS ADDON_LOADED, which a LoadOnDemand
    // addon fires on the session's first trainer. The collapse is engine-side and clears too
    // (1957; the transcription used to re-push on show, which the reference never does).
    open.clear();
    open.open(DAZALAR, 0, vec![], "Hello, hunter!".into());
    let mut learned = menu();
    learned.services[0].category = TrainerServiceCategory::Used;
    feed(&mut s, &mut open, learned, "TRAINER_SHOW");
    assert!(
        filter_on(&mut s, "unavailable"),
        "a fresh list is the reference's reset: unavailable shows again"
    );
    assert!(filter_on(&mut s, "available"));
    assert!(
        !filter_on(&mut s, "used"),
        "and the reset's mask is 3 — used stays hidden"
    );
    assert_eq!(
        s.eval::<i64>("return TRAINER_FILTER_UNAVAILABLE").unwrap(),
        0,
        "his saved choice is still the global's, for the next session's first trainer"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The detail pane after learning a spell** — the director's report: the trained row vanished, the
/// highlight landed on the row that slid into its place, and the name/icon/cost/`Requires:` block
/// below went on describing the spell that was gone.
///
/// The stock Lua repaints that pane on `TRAINER_UPDATE` through **one** door:
/// `ClassTrainer_SelectFirstLearnableSkill`, the only thing that restores
/// `ClassTrainerFrame.showSkillDetails` after `ClassTrainerTrainButton_OnClick` cleared it —
/// `ClassTrainer_SetSelection` early-returns while it is nil. Which door `TRAINER_UPDATE` takes is
/// decided entirely by the engine's `GetTrainerSelectionIndex()`, and the reference's answer for a
/// service that has gone off screen is its index in the hidden **tail** — past
/// `GetNumTrainerServices()`, never a live row (`0x4d7520`; see `benilla_ui`'s `selected_row`). So
/// the reference takes the `> 1` branch, resets the scroll, and hides the pane.
///
/// **What a player sees after training, in the reference and now here: an empty detail pane and no
/// highlighted row.** It does not advance to the next spell — that is the *fresh window's*
/// behaviour, which is what a selection reading 0 or 1 would have produced.
///
/// Driven through the real [`crate::ui_trainer::TrainerOpen`] like B256's test above, for the same
/// reason: the packet-vs-repaint decision under test is the app's own.
#[test]
fn learning_a_spell_takes_the_detail_pane_with_it_instead_of_stranding_the_last_one() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_trainer::TrainerOpen;
    const DAZALAR: u64 = 0xabc;

    let mut s = trainer_script();
    // The reference's own file-scope defaults (1128) — "already known" OFF, which is what makes a
    // learned service leave the list at all.
    s.run("TRAINER_FILTER_AVAILABLE = 1 TRAINER_FILTER_UNAVAILABLE = 1 TRAINER_FILTER_USED = 0")
        .unwrap();
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
    s.set_money(5000);

    // The trainer feed (`ui_trainer::feed_trainer`), reduced to the part this bug lives in.
    fn feed(s: &mut UiScript, open: &mut TrainerOpen, state: TrainerState, event: &str) {
        if open.fresh_list {
            s.reset_trainer_list_state(open.trainer_type);
            open.fresh_list = false;
        }
        s.set_trainer(Some(state));
        s.fire_event(event, vec![ScriptValue::Str("Dazalar".into())]);
    }
    let pane = |s: &mut UiScript| -> (bool, String) {
        s.eval::<(bool, String)>(
            "return ClassTrainerSkillName:IsVisible() and true or false, \
                    ClassTrainerSkillName:GetText() or ''",
        )
        .unwrap()
    };
    let row_name = |s: &mut UiScript, row: i64| {
        s.eval::<String>(&format!("return (GetTrainerServiceInfo({row})) or ''"))
            .unwrap()
    };

    let mut open = TrainerOpen::default();
    open.open(DAZALAR, 0, vec![], "Hello, warrior!".into());
    feed(&mut s, &mut open, menu(), "TRAINER_SHOW");

    // Row 2 is the first learnable — the row the window opens on, and the one he trains.
    assert_eq!(row_name(&mut s, 2), "Heroic Strike");
    assert_eq!(
        pane(&mut s),
        (true, "Heroic Strike".into()),
        "the window opens describing its own selection"
    );
    s.run("ClassTrainerTrainButton:Click()").unwrap();
    assert_eq!(
        s.take_trainer_buys(),
        vec![78],
        "the Train button bought the selected row"
    );

    // The spell lands and the re-evaluator (2333) repaints the bought row gray in place; with
    // "already known" off it leaves the list, and Cleave slides up into row 2 under where the
    // selection used to be. (Until 2333 this was a second list packet marked as a refresh —
    // B256.)
    let mut learned = menu();
    learned.services[0].category = TrainerServiceCategory::Used;
    feed(&mut s, &mut open, learned, "TRAINER_UPDATE");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        row_name(&mut s, 2),
        "Cleave",
        "the trained row is gone from the list and the next one took its place"
    );
    assert!(
        !pane(&mut s).0,
        "and the pane below is empty rather than still describing the spell he just learned: {:?}",
        pane(&mut s).1
    );
    assert!(
        !s.eval::<bool>("return ClassTrainerSkillHighlightFrame:IsVisible()")
            .unwrap(),
        "nothing is highlighted either — the selection is off screen, not on Cleave"
    );
    assert!(
        s.eval::<i64>("return GetTrainerSelectionIndex()").unwrap()
            > s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        "because the engine answers with the hidden row, which is what steers the window there"
    );

    // And he can pick the next one up by hand, which is the whole of the recovery.
    s.run("this = ClassTrainerSkill2 ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert_eq!(pane(&mut s), (true, "Cleave".into()));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The filter click has to repaint the LIST, not just the engine** — the
/// director's "filter no longer work", with the dropdown showing Available only and red rows still
/// under it.
///
/// Every other test in this file asserts `GetNumTrainerServices()`, which is the engine's own
/// answer and moves the instant the mask does. What the player looks at is the row buttons, and
/// those are painted by `ClassTrainerFrame_Update` — which after a filter click is reached from
/// exactly one place: `ClassTrainerFrame_OnEvent`'s `TRAINER_UPDATE` arm. The stock click handler
/// fires no event and calls no update (its `ScrollBar:SetValue(0)` is a no-op at zero), because in
/// the reference the **engine** fires it from the mask-commit thunk `0x4d8c90`
/// (`mov ecx,0x136; jmp 0x703e50`). Ours did not, so the checkbox moved and the list did not.
#[test]
fn a_filter_click_repaints_the_rows_the_player_is_looking_at() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);

    // What the player sees: the painted row buttons, counted the way the eye does.
    let painted = "\
local n = 0
for i = 1, 11 do
    local b = getglobal(\"ClassTrainerSkill\"..i)
    if b and b:IsVisible() then n = n + 1 end
end
return n";
    let count = |s: &mut UiScript| s.eval::<i64>(painted).unwrap();
    assert_eq!(count(&mut s), 6, "six rows on screen before any filtering");

    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    s.run("this = DropDownList1Button3; UIDropDownMenuButton_OnClick()")
        .unwrap();
    // The engine's queued `TRAINER_UPDATE` lands on the next tick (a binding cannot re-enter the
    // handler dispatch from inside Lua — the queue's own contract).
    s.tick(0.016);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        count(&mut s),
        5,
        "the already-known row must LEAVE THE SCREEN, not just the engine's count — this is the \
         director's report: the checkbox moved and the list underneath did not"
    );
}
