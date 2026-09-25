//! End-to-end tests of the stock `Blizzard_TrainerUI` window, engine-only: loaded behind its
//! FrameXML dependencies and fed a synthetic service list.

use benilla_ui::script::{
    ExtractedQuad, QuadContent, ScriptValue, SoundRequest, TrainerAbilityReq, TrainerService,
    TrainerServiceCategory, TrainerSkillReq, TrainerState, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// The trainer window with every state filter on, for the full tree at fixed indices.
fn trainer_script() -> UiScript {
    let mut s = trainer_script_base();
    load_xml(
        &s,
        "Interface\\AddOns\\Blizzard_TrainerUI\\Blizzard_TrainerUI.xml",
    );
    finish_trainer_load(&mut s);
    s
}

/// What the manifest loads before the trainer addon, with a text measurer: the stock row's label
/// is a width-0 `<ButtonText>`, sized to its text.
fn trainer_script_base() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(7.0)));
    // In manifest order (ScrollTemplates before UIPanelTemplates): the window calls
    // `UpdateMicroButtons` and inherits the panel templates.
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

/// The rest of a LoadOnDemand load: `ADDON_LOADED` pushes the filter globals, "used" on, into the
/// engine. The title reads `UnitName("npc")`, so the NPC is seated.
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

/// Whether any text quad renders in `color`: the red cost's `(1.0, 0.1, 0.1)` is distinct from an
/// unavailable row's `(0.9, 0, 0)`.
fn has_text_color(quads: &[ExtractedQuad], color: [f32; 3]) -> bool {
    quads.iter().any(|q| match &q.content {
        QuadContent::Text { color: Some(c), .. } => (0..3).all(|i| (c[i] - color[i]).abs() < 0.02),
        _ => false,
    })
}

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

/// A two-line warrior menu. Groups sort by name, services by level then name, so the tree is:
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
                // Gated by level and skill, but the prerequisite is learned, so it reads white.
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

#[test]
fn shipped_trainer_frame_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();

    assert!(!s
        .eval::<bool>("return ClassTrainerFrame:IsVisible()")
        .unwrap());

    // 50 copper: Heroic Strike (10c) is affordable, Thunder Clap (500c) is not.
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event(
        "TRAINER_SHOW",
        vec![ScriptValue::Str("Sana Winterhoof".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s
        .eval::<bool>("return ClassTrainerFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return ClassTrainerNameText:GetText()")
            .unwrap(),
        "Sana Winterhoof"
    );

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

    // `ClassTrainer_SelectFirstLearnableSkill` selects row 2, Heroic Strike: Train is enabled.
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

    s.run("BuyTrainerService(GetTrainerSelectionIndex())")
        .unwrap();
    assert_eq!(s.take_trainer_buys(), vec![78]);
    assert!(s.take_trainer_buys().is_empty(), "drained");

    // Thunder Clap (row 6) is available but unaffordable: Train disables and the cost reddens.
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

    // Cleave (row 3) is gated: the `Requires:` line lists its level, skill and ability gates.
    s.run("this = ClassTrainerSkill3; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    let reqs = s
        .eval::<String>("return ClassTrainerSkillRequirements:GetText()")
        .unwrap();
    assert!(reqs.starts_with("Requires: "), "reqs: {reqs}");
    for term in ["Level", "Swords", "Charge"] {
        assert!(reqs.contains(term), "reqs missing {term}: {reqs}");
    }
    // A met ability renders white (`TRAINER_REQ_ABILITY`) beside the red unmet gates.
    assert!(
        reqs.contains("|cffffffffCharge (Rank 1)|r"),
        "a known prerequisite shows white with its rank: {reqs}"
    );
    assert!(!s
        .eval::<bool>("return ClassTrainerTrainButton:IsEnabled() ~= 0")
        .unwrap());

    s.set_trainer(None);
    s.fire_event("TRAINER_CLOSED", vec![]);
    assert!(!s
        .eval::<bool>("return ClassTrainerFrame:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

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

    // Folding Arms leaves 4 rows: H:Arms, H:Fury, Rend, Thunder Clap.
    s.run("this = ClassTrainerSkill1; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 4);
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill2Text:GetText()")
            .unwrap(),
        "Fury",
        "Arms folded; its header (row 1) now abuts the Fury header (row 2)"
    );

    s.run("this = ClassTrainerSkill1; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn filter_hides_a_state_keeping_headers() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);

    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);

    // Button 3 is the "used" row, clicked through the kit's own handler.
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

/// `UIDropDownMenuButton_OnClick` flips a `keepShownOnClick` row's check after running its func:
/// click, re-click and re-open must agree on screen and in the engine.
#[test]
fn filter_rows_toggle_through_the_dropdown_kit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);

    // Row 2 is "Unavailable", checked since `trainer_script` turns every state on.
    s.run("ToggleDropDownMenu(1, nil, ClassTrainerFrameFilterDropDown)")
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let row_checked = |s: &mut UiScript| {
        s.eval::<bool>("return DropDownList1Button2Check:IsVisible() and true or false")
            .unwrap()
    };
    assert!(row_checked(&mut s), "Unavailable starts checked");
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);

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

    s.run("this = DropDownList1Button2; UIDropDownMenuButton_OnClick()")
        .unwrap();
    assert!(row_checked(&mut s), "the re-click restores the check");
    assert!(
        s.eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
            .unwrap(),
        "and re-enables the filter"
    );
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 6);

    // Off again, then close and re-open: `Initialize` re-derives every row from the engine.
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

/// One skill line of 15 services, a 16-row tree over 11 visible rows, so the list scrolls.
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
    assert_eq!(s.eval::<String>(row1).unwrap(), "Arms");
    assert_eq!(
        s.eval::<String>("return ClassTrainerSkill2Text:GetText()")
            .unwrap(),
        "  Service 01"
    );

    // Aim over a row's text: the spin must reach the list from a row.
    s.resolve();
    let (x, y) = text_center(&s.extract(), "Service 03");

    // Negative is down. The reference's wheel moves half the bar's height, a page and not a row
    // (`ScrollFrameTemplate_OnMouseWheel`, `UIPanelTemplates.lua:150-157`).
    s.mouse_wheel(x, y, -1.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The wiring is the subject (wheel, bar, `FauxScrollFrame_OnVerticalScroll`, repaint), so the
    // magnitude is asserted only as more than a row.
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

    // 16 rows over 11 make 5 the deepest offset, which one half-bar page already reaches.
    s.mouse_wheel(x, y, -1.0);
    assert_eq!(
        s.eval::<i64>("return ClassTrainerListScrollFrame.offset or -1")
            .unwrap(),
        offset,
        "clamped at the bottom (numItems - numToDisplay), never past it"
    );

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

/// The row's name is its button's `<ButtonText>`: hover swaps in the `GameFontHighlight` font and
/// `LockHighlight()` pins it for the selection (`Blizzard_TrainerUI.lua:183`), while `SetTextColor`
/// writes only the normal font, so both states read white.
#[test]
fn a_selected_or_hovered_service_row_paints_its_name_white() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        s.eval::<bool>("return ClassTrainerSkill1:GetFontString() ~= nil")
            .unwrap(),
        "the row name is the Button's ButtonText, the only region per-state fonts reach"
    );

    s.run("this = ClassTrainerSkill3; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    s.resolve();
    assert!(
        text_has_color(&s.extract(), "Cleave", [1.0, 1.0, 1.0]),
        "the selected row's name renders white"
    );

    s.run("this = ClassTrainerSkill2; ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    s.resolve();
    assert!(
        text_has_color(&s.extract(), "Cleave", [0.9, 0.0, 0.0]),
        "an unselected unavailable row is red again"
    );

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

/// The wheel scrolls silently, as the stock `ScrollFrameTemplate_OnMouseWheel` plays no sound; the
/// arrow buttons play `UChatScrollButton`.
#[test]
fn wheel_scroll_is_silent_but_the_arrows_click() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(long_menu())); // 16 rows > 11 visible → the bar + arrows show
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);
    let _ = s.take_sounds(); // drain the window's OnShow open sound

    let click = SoundRequest::KitName("UChatScrollButton".into());

    s.resolve();
    let (x, y) = text_center(&s.extract(), "Service 03");
    s.mouse_wheel(x, y, -1.0);
    assert!(
        !s.take_sounds().contains(&click),
        "the wheel scroll is silent"
    );

    // One notch reaches the bottom: half the 152-tall bar is 76px, which `SetValue` snaps to the
    // 16px step as the range's 80, as the reference does; `FauxScrollFrame_Update` then disables
    // the down arrow (`UIPanelTemplates.lua:199`).
    assert_eq!(
        s.eval::<f64>("return ClassTrainerListScrollFrameScrollBar:GetValue()")
            .unwrap(),
        80.0,
        "one wheel notch = 76px, snapped to the 5-row bottom of an 80px range"
    );

    s.run("ClassTrainerListScrollFrameScrollBarScrollUpButton:Click()")
        .unwrap();
    assert!(
        s.take_sounds().contains(&click),
        "the arrow button clicks (UChatScrollButton)"
    );
}

/// The arrows step half the bar's height (`UIPanelScrollBarTemplate`,
/// `UIPanelTemplates.xml:137-149`), several rows at a time, and stop at the top.
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
    // Pixels round to rows as `floor(v/itemHeight + 0.5)` (`UIPanelTemplates.lua:231`).
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

/// A dropdown click writes the global, `## SavedVariables` saves it, and on a restart `LoadAddOn`
/// runs the files, the saved file, then `ADDON_LOADED`, whose arm pushes it into the engine.
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
    // The stock default `TRAINER_FILTER_USED = 0` hides the known service: 2 headers, 3 services.
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

    // The restart: a fresh VM loads the addon on demand, and its first paint is filtered.
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

/// A new list resets the filter mask to 3 (available|unavailable), or 5 (available|used) at a
/// mount trainer, and clears the collapse set, as the reference's list builder does (`0x4d75d9`).
#[test]
fn a_new_list_packet_resets_the_filter_mask_and_the_collapse_set() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_trainer(Some(menu()));
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
    // Engine-side state only: a collapsed group, and `used` set with no global.
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

    // A mount trainer gets available|used, which is what shows a known mount at all.
    s.reset_trainer_list_state(1);
    assert!(s
        .eval::<bool>("return GetTrainerServiceTypeFilter('used') == 1")
        .unwrap());
    assert!(!s
        .eval::<bool>("return GetTrainerServiceTypeFilter('unavailable') == 1")
        .unwrap());
}

/// One profession skill line: a long recipe name and a short one, each with a rank subtext.
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

/// The stock row is a flow: its `<ButtonText>` has width 0 (ClassTrainerFrameTemplates.xml), so a
/// long name stays on one line in its 16 px row, and the rank is anchored 10 px past the name's
/// right edge (`Blizzard_TrainerUI.lua:157`).
#[test]
fn a_long_row_name_stays_on_one_line_and_carries_its_rank_along() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(5000);
    s.set_trainer(Some(recipe_menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Nadyia".into())]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // A host measure of 6 px per character and 14 px per line; the name's requests are kept, since
    // what it asks for is the fact under test.
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

/// A purchase repaints in place, as the reference's state re-evaluator (`0x4d7d40`) does, so the
/// player's filter and collapse stand; only a fresh list resets the mask (`0x4d75d9`).
#[test]
fn learning_a_spell_keeps_the_filter_and_the_collapse_a_re_open_still_resets() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_trainer::TrainerOpen;
    const DAZALAR: u64 = 0xabc;

    let mut s = trainer_script();
    // The stock file-scope defaults.
    s.run("TRAINER_FILTER_AVAILABLE = 1 TRAINER_FILTER_UNAVAILABLE = 1 TRAINER_FILTER_USED = 0")
        .unwrap();
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
    s.set_money(5000);

    // `ui_trainer::feed_trainer`, reduced to its reset-then-fire.
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
    s.run("CollapseTrainerSkillLine(1)").unwrap();
    assert_eq!(rows(&mut s), 3, "the folded group keeps its header only");

    // He trains: the state re-evaluator (the reference's `0x4d7d40`) repaints the bought row gray
    // in place, with no second list to reset anything.
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

    // He walks away and back: a fresh list resets the mask to 3 (`0x4d75d9`), and the stock addon
    // pushes its globals only on its one `ADDON_LOADED`, so nothing restores his choice; the
    // collapse clears too.
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

/// After a purchase the reference shows an empty detail pane and no highlight: the Train click
/// clears `showSkillDetails`, which `TRAINER_UPDATE` restores only for a selection index of 0 or
/// 1, and the engine answers the learned service's index in the hidden tail, past
/// `GetNumTrainerServices()` (`0x4d7520`), so the stock arm hides the pane.
#[test]
fn learning_a_spell_takes_the_detail_pane_with_it_instead_of_stranding_the_last_one() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_trainer::TrainerOpen;
    const DAZALAR: u64 = 0xabc;

    let mut s = trainer_script();
    // The stock defaults: "already known" off, so a learned service leaves the list.
    s.run("TRAINER_FILTER_AVAILABLE = 1 TRAINER_FILTER_UNAVAILABLE = 1 TRAINER_FILTER_USED = 0")
        .unwrap();
    s.fire_event(
        "ADDON_LOADED",
        vec![ScriptValue::Str("Blizzard_TrainerUI".into())],
    );
    s.set_money(5000);

    // `ui_trainer::feed_trainer`, reduced to its reset-then-fire.
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

    // The re-evaluator greys the bought row in place; with "already known" off it leaves the
    // list, and Cleave slides up into row 2.
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

    s.run("this = ClassTrainerSkill2 ClassTrainerSkillButton_OnClick('LeftButton')")
        .unwrap();
    assert_eq!(pane(&mut s), (true, "Cleave".into()));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A filter click repaints the row buttons, not just the engine's count: the stock click handler
/// fires no update, because in the reference the engine's mask-commit thunk fires `TRAINER_UPDATE`
/// (`0x4d8c90`: `mov ecx,0x136; jmp 0x703e50`), whose arm runs `ClassTrainerFrame_Update`.
#[test]
fn a_filter_click_repaints_the_rows_the_player_is_looking_at() {
    benilla_formats::wow_data_or_skip!();
    let mut s = trainer_script();
    s.set_money(50);
    s.set_trainer(Some(menu()));
    s.fire_event("TRAINER_SHOW", vec![ScriptValue::Str("Sana".into())]);

    // The painted row buttons, as the player sees them.
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
    // The engine's queued `TRAINER_UPDATE` lands on the next tick: a binding cannot re-enter the
    // handler dispatch from inside Lua.
    s.tick(0.016);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        count(&mut s),
        5,
        "the already-known row must LEAVE THE SCREEN, not just the engine's count — this is the \
         reported bug: the checkbox moved and the list underneath did not"
    );
}
