//! Drives the stock `Blizzard_TradeSkillUI` and `Blizzard_CraftUI` addons off the player's chain
//! with a synthetic book and the app's own show events: the CollapseAll tab, the filter menus,
//! the reagent slots and the list rows' selection and hover.

use benilla_ui::script::{
    CraftRecipe, CraftState, CraftTooltip, TradeSkillDifficulty, TradeSkillReagent,
    TradeSkillRecipe, TradeSkillState, UiScript,
};

mod common;

/// The load prefix of both windows, in the app's order.
const FILES: &[&str] = &[
    "Interface\\FrameXML\\Fonts.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "ScrollTemplates.xml",
    // Both windows open from UIParent's TRADE_SKILL_SHOW / CRAFT_SHOW arms (`*_LoadUI`, then
    // `*_Show`). With no addon registry the addons load as chain files, and `*_LoadUI` reports
    // ADDON_LOAD_FAILED through `message`, a shown frame, not an error.
    // Both addons inherit the trainer window's list and detail templates.
    r"Interface\FrameXML\ClassTrainerFrameTemplates.xml",
    // The reagent slots inherit `QuestItemTemplate` and are painted with `SetItemButtonTexture`
    // and `SetItemButtonCount` (`ItemButtonTemplate.lua`).
    r"Interface\FrameXML\ItemButtonTemplate.xml",
    r"Interface\FrameXML\QuestFrameTemplates.xml",
    r"Interface\AddOns\Blizzard_TradeSkillUI\Blizzard_TradeSkillUI.xml",
    r"Interface\AddOns\Blizzard_CraftUI\Blizzard_CraftUI.xml",
];

fn load_ui(script: &UiScript) {
    // `common::load_ui`: an entry with a path separator is a stock file read off the player's
    // chain, not from `assets/ui`.
    for file in FILES {
        common::load_ui(script, file);
    }
}

fn recipe(
    spell_id: u32,
    name: &str,
    group: (u32, u32, &str),
    product_inv_type: u32,
) -> TradeSkillRecipe {
    TradeSkillRecipe {
        group: Some((group.0, group.1, group.2.to_string())),
        spell_id,
        name: name.into(),
        difficulty: TradeSkillDifficulty::Medium,
        num_available: 2,
        icon: Some("Interface\\Icons\\INV_Misc_ArmorKit_04".into()),
        min_made: 1,
        max_made: 1,
        cooldown_secs: None,
        product_item: spell_id + 10_000,
        product_inv_type,
        product_item_level: 0, // neutral: ordering falls through to the name
        reagents: vec![TradeSkillReagent {
            item: 2840,
            name: Some("Copper Bar".into()),
            icon: Some("Interface\\Icons\\INV_Ingot_02".into()),
            need: 2,
            have: 5,
        }],
        tools: vec![("Anvil".into(), true)],
    }
}

/// A two-group Blacksmithing book: Mail (a chest and a legs product) and Trade Goods (a stone,
/// which lands in the 0x800000 not-equippable slot).
fn state() -> TradeSkillState {
    TradeSkillState {
        line: 164,
        line_name: "Blacksmithing".into(),
        rank: 1,
        max_rank: 75,
        recipes: vec![
            recipe(2661, "Copper Chain Vest", (4, 3, "Mail"), 5),
            recipe(2662, "Copper Chain Pants", (4, 3, "Mail"), 7),
            recipe(3320, "Rough Sharpening Stone", (7, 0, "Trade Goods"), 0),
        ],
        repeat_count: 0,
    }
}

/// What the app does after a list mutator: drain the touched flag and fire `TRADE_SKILL_UPDATE`.
fn pump(script: &mut UiScript) {
    if script.take_trade_skill_touched() {
        script.fire_event("TRADE_SKILL_UPDATE", vec![]);
    }
}

#[test]
fn collapse_all_tab_and_filter_dropdowns_work_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);

    s.set_trade_skill(Some(state()));
    s.fire_event("TRADE_SKILL_SHOW", vec![]);
    assert!(
        s.eval::<bool>("return TradeSkillFrame:IsShown()").unwrap(),
        "the window opens on TRADE_SKILL_SHOW"
    );
    // 2 headers + 3 recipes.
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 5);

    // The CollapseAll tab carries its GlobalString text.
    assert_eq!(
        s.eval::<String>("return TradeSkillCollapseAllButton:GetText()")
            .unwrap(),
        "All"
    );

    // The click never calls Update() itself: the repaint rides TRADE_SKILL_UPDATE (`pump`).
    s.run("TradeSkillCollapseAllButton:Click()").unwrap();
    pump(&mut s);
    assert_eq!(
        s.eval::<i64>("return GetNumTradeSkills()").unwrap(),
        2,
        "collapse-all leaves only the two headers"
    );
    s.run("TradeSkillCollapseAllButton:Click()").unwrap();
    pump(&mut s);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 5);

    // The dropdowns default to the "All …" texts.
    assert_eq!(
        s.eval::<String>("return TradeSkillSubClassDropDownText:GetText()")
            .unwrap(),
        "All Subclasses"
    );
    assert_eq!(
        s.eval::<String>("return TradeSkillInvSlotDropDownText:GetText()")
            .unwrap(),
        "All Slots"
    );
    // Chest (5) is bit 4, Legs (7) bit 6, the stone (0) the not-equippable slot.
    assert_eq!(
        s.eval::<(String, String, String)>("return GetTradeSkillInvSlots()")
            .unwrap(),
        (
            "Chest".to_string(),
            "Legs".to_string(),
            "Not equippable.".to_string()
        )
    );

    // A menu-row click: "Trade Goods" is row 3 (All, then the two groups).
    s.run("ToggleDropDownMenu(1, nil, TradeSkillSubClassDropDown)")
        .unwrap();
    s.run("DropDownList1Button3:Click()").unwrap();
    pump(&mut s);
    assert_eq!(
        s.eval::<i64>("return GetNumTradeSkills()").unwrap(),
        2,
        "exclusive Trade Goods: its header + one recipe"
    );
    assert_eq!(
        s.eval::<String>("local n = GetTradeSkillInfo(1) return n")
            .unwrap(),
        "Trade Goods"
    );
    // The dropdown text follows the picked row on the next initialize (OnShow re-runs it).
    s.run("TradeSkillSubClassDropDown:Hide() TradeSkillSubClassDropDown:Show()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return TradeSkillSubClassDropDownText:GetText()")
            .unwrap(),
        "Trade Goods"
    );

    // The "All Subclasses" row (row 1) restores the full list.
    s.run("ToggleDropDownMenu(1, nil, TradeSkillSubClassDropDown)")
        .unwrap();
    s.run("DropDownList1Button1:Click()").unwrap();
    pump(&mut s);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 5);
}

/// A 1.12 craft list has no headers, so `Blizzard_CraftUI.lua:269-282` always hides the Craft
/// window's CollapseAll tab.
#[test]
fn craft_collapse_tab_loads_with_text_and_stays_hidden_for_a_flat_list() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);

    assert_eq!(
        s.eval::<String>("return CraftCollapseAllButton:GetText()")
            .unwrap(),
        "All"
    );
    s.fire_event("CRAFT_SHOW", vec![]);
    assert!(
        s.eval::<bool>("return CraftFrame:IsShown()").unwrap(),
        "the craft window opens on CRAFT_SHOW"
    );
    assert!(
        !s.eval::<bool>("return CraftExpandButtonFrame:IsShown()")
            .unwrap(),
        "zero headers → the tab hides (ref l.269-282's own scan)"
    );
}

/// `TradeSkillItemTemplate` (`Blizzard_TradeSkillUI.xml:11-35`) and `CraftItemTemplate`
/// (`Blizzard_CraftUI.xml:29-53`) inherit `QuestItemTemplate` and override only scripts: a 147×41
/// slot, a 39×39 icon, the name plate on the icon's right edge with the name centred on it, and
/// the count on the icon's bottom-right corner.
#[test]
fn reagent_slots_carry_the_questitemtemplate_shape_in_both_windows() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);

    // Both windows open so the anchors resolve against a laid-out parent; the slots are shown as
    // SetSelection shows one per reagent.
    s.set_trade_skill(Some(state()));
    s.fire_event("TRADE_SKILL_SHOW", vec![]);
    s.fire_event("CRAFT_SHOW", vec![]);
    for w in ["TradeSkillReagent", "CraftReagent"] {
        for i in 1..=3 {
            s.run(&format!("{w}{i}:Show()")).unwrap();
        }
    }

    for w in ["TradeSkillReagent", "CraftReagent"] {
        let num = |expr: &str| {
            s.eval::<f64>(&format!("return {expr}"))
                .unwrap_or_else(|e| panic!("{expr}: {e}"))
        };

        // QuestItemTemplate's <Size>.
        assert_eq!(
            (
                num(&format!("{w}1:GetWidth()")),
                num(&format!("{w}1:GetHeight()"))
            ),
            (147.0, 41.0),
            "{w}1 row box"
        );

        // Reagent2 anchors LEFT to Reagent1's RIGHT at zero offset, so the pitch is the row width,
        assert_eq!(
            num(&format!("{w}2:GetLeft()")) - num(&format!("{w}1:GetLeft()")),
            147.0,
            "{w} column pitch"
        );
        // and row 2 drops one row height plus a 2px gutter.
        assert_eq!(
            num(&format!("{w}3:GetTop()")) - num(&format!("{w}1:GetTop()")),
            -43.0,
            "{w} row step"
        );

        // `$parentIconTexture`: 39×39, flush in the row's TOPLEFT corner.
        assert_eq!(
            (
                num(&format!("{w}1IconTexture:GetWidth()")),
                num(&format!("{w}1IconTexture:GetHeight()"))
            ),
            (39.0, 39.0),
            "{w}1 icon"
        );
        assert_eq!(
            num(&format!("{w}1IconTexture:GetLeft()")),
            num(&format!("{w}1:GetLeft()")),
            "{w}1 icon flush left"
        );
        assert_eq!(
            num(&format!("{w}1IconTexture:GetTop()")),
            num(&format!("{w}1:GetTop()")),
            "{w}1 icon flush top"
        );

        // The 128×64 name plate starts 10px inside the icon's right edge, centred on the icon.
        assert_eq!(
            s.eval::<String>(&format!("return {w}1NameFrame:GetTexture()"))
                .unwrap(),
            "Interface\\QuestFrame\\UI-QuestItemNameFrame",
            "{w}1 name plate art"
        );
        assert_eq!(
            (
                num(&format!("{w}1NameFrame:GetWidth()")),
                num(&format!("{w}1NameFrame:GetHeight()"))
            ),
            (128.0, 64.0),
            "{w}1 plate size"
        );
        assert_eq!(
            num(&format!("{w}1NameFrame:GetLeft()")) - num(&format!("{w}1IconTexture:GetRight()")),
            -10.0,
            "{w}1 plate rides the icon's right edge"
        );

        // The name sits on the plate, 15 from its left, vertically centred.
        assert_eq!(
            num(&format!("{w}1Name:GetLeft()")) - num(&format!("{w}1NameFrame:GetLeft()")),
            15.0,
            "{w}1 name inset"
        );
        let (nc, pc) = (
            num(&format!("({w}1Name:GetTop() + {w}1Name:GetBottom()) / 2")),
            num(&format!(
                "({w}1NameFrame:GetTop() + {w}1NameFrame:GetBottom()) / 2"
            )),
        );
        assert!(
            (nc - pc).abs() < 0.01,
            "{w}1 name is centred on the plate ({nc} vs {pc})"
        );

        // The count sits on the icon's bottom-right corner (-4, +1).
        assert_eq!(
            num(&format!("{w}1Count:GetRight()")) - num(&format!("{w}1IconTexture:GetRight()")),
            -4.0,
            "{w}1 count x"
        );
        assert_eq!(
            num(&format!("{w}1Count:GetBottom()")) - num(&format!("{w}1IconTexture:GetBottom()")),
            1.0,
            "{w}1 count y"
        );
        assert!(
            num(&format!("{w}1Count:GetBottom()")) >= num(&format!("{w}1IconTexture:GetBottom()")),
            "{w}1 count sits ON the icon, not below the row"
        );
    }
}

/// The selection glow is `TradeSkillHighlightFrame`, a `hidden="true"` frame the update moves
/// and shows (`Blizzard_TradeSkillUI.lua:99`, `:142-143`); only the vertex colour goes on the
/// `TradeSkillHighlight` texture (`:200`). A list row has no tooltip: its template overrides
/// `OnClick` alone, and `ClassTrainerSkillButtonTemplate` only recolours `$parentSubText` on hover.
#[test]
fn a_row_click_shows_the_selection_glow_and_a_row_hover_shows_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_trade_skill(Some(state()));
    s.fire_event("TRADE_SKILL_SHOW", vec![]);

    // Row 1 is the "Mail" header, row 2 the first recipe.
    assert_eq!(
        s.eval::<String>("local _, t = GetTradeSkillInfo(2) return t")
            .unwrap(),
        "medium",
        "row 2 is a recipe, not a header (state()'s own difficulty)"
    );

    // Opening selects `GetFirstTradeSkill()` (OnEvent), so the glow is up before any click.
    let glow_on_row = |s: &UiScript, n: i64| {
        let (glow, row) = (
            s.eval::<f64>("return TradeSkillHighlightFrame:GetTop()")
                .unwrap(),
            s.eval::<f64>(&format!("return TradeSkillSkill{n}:GetTop()"))
                .unwrap(),
        );
        (glow - row).abs() < 0.01
    };
    assert!(
        s.eval::<bool>("return TradeSkillHighlightFrame:IsShown()")
            .unwrap(),
        "the show-time auto-selection glows — the FRAME, not just its texture"
    );
    assert!(glow_on_row(&s, 2), "and it is parked on the first recipe");

    // A click on the other Mail recipe moves it.
    s.run("TradeSkillSkill3:Click()").unwrap();
    pump(&mut s);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        3,
        "the click selected row 3"
    );
    assert!(
        s.eval::<bool>("return TradeSkillHighlightFrame:IsShown()")
            .unwrap()
            && glow_on_row(&s, 3),
        "the glow followed the click to row 3"
    );

    // With every group folded no recipe row is visible to carry it.
    s.run("TradeSkillCollapseAllButton:Click()").unwrap();
    pump(&mut s);
    assert!(
        !s.eval::<bool>("return TradeSkillHighlightFrame:IsShown()")
            .unwrap(),
        "no recipe row on screen → no glow (headers never take the selection)"
    );

    // Control: a reagent slot has an OnEnter, so `GetScript` reports real handlers.
    assert!(
        s.eval::<bool>("return TradeSkillReagent1:GetScript(\"OnEnter\") ~= nil")
            .unwrap(),
        "control: a reagent slot has an OnEnter, so GetScript reports real handlers"
    );
    // The list row's hover is the trainer template's, which opens no tooltip.
    s.run("GameTooltip:Hide() this = TradeSkillSkill2 TradeSkillSkill2:GetScript(\"OnEnter\")()")
        .unwrap();
    assert!(
        !s.eval::<bool>("return GameTooltip:IsShown()").unwrap(),
        "a list row's hover opens no tooltip — the reference's rows never tooltip"
    );
    s.run("this = TradeSkillSkill2 TradeSkillSkill2:GetScript(\"OnLeave\")()")
        .unwrap();
}

/// The hovered and the selected recipe rows are white, the rest their difficulty colour:
/// `ClassTrainerSkillButtonTemplate` declares a white `<HighlightFont>`, and
/// `TradeSkillFrame_Update` sets the difficulty with `SetTextColor`, which writes the normal font
/// only, then `LockHighlight`s the selection (`Blizzard_TradeSkillUI.lua:113`, `:144`). The colour
/// is read off the extracted text quad.
#[test]
fn a_hovered_or_selected_recipe_row_paints_its_label_white() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_screen_size(1024.0, 768.0);
    s.set_trade_skill(Some(state()));
    s.fire_event("TRADE_SKILL_SHOW", vec![]);

    // Row 1 is the "Mail" header; rows 2 and 3 are its Medium recipes, and opening selects row 2.
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        2,
        "the show-time auto-selection landed on the first recipe"
    );

    // The label is the button's ButtonText, the only region per-state fonts reach.
    assert!(
        s.eval::<bool>("return TradeSkillSkill2:GetFontString() ~= nil")
            .unwrap(),
        "the row label is the Button's ButtonText, the only region per-state fonts reach"
    );

    let row_color = |s: &mut UiScript, n: i64| -> [f32; 4] {
        let text = s
            .eval::<String>(&format!("return TradeSkillSkill{n}:GetText()"))
            .unwrap();
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                benilla_ui::script::QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == text => color,
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text quad for row {n} (\"{text}\")"))
    };
    let park_cursor_off_the_list = |s: &mut UiScript| {
        s.resolve();
        s.mouse_move(1000.0, 20.0);
    };
    let hover_row = |s: &mut UiScript, n: i64| {
        s.resolve();
        let (x, y) = (
            s.eval::<f64>(&format!(
                "return (TradeSkillSkill{n}:GetLeft() + TradeSkillSkill{n}:GetRight()) / 2"
            ))
            .unwrap(),
            s.eval::<f64>(&format!(
                "return (TradeSkillSkill{n}:GetTop() + TradeSkillSkill{n}:GetBottom()) / 2"
            ))
            .unwrap(),
        );
        s.mouse_move(x as f32, y as f32);
    };

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const MEDIUM: [f32; 4] = [1.0, 1.0, 0.0, 1.0]; // TradeSkillTypeColor["medium"], the book's own

    park_cursor_off_the_list(&mut s);
    assert_eq!(
        row_color(&mut s, 2),
        WHITE,
        "the SELECTED row is white with the cursor nowhere near it — LockHighlight, not a repaint"
    );
    assert_eq!(
        row_color(&mut s, 3),
        MEDIUM,
        "an unselected, unhovered recipe wears its difficulty colour"
    );

    hover_row(&mut s, 3);
    assert_eq!(
        row_color(&mut s, 3),
        WHITE,
        "hovered: the HighlightFont instance is in force over SetTextColor's difficulty paint"
    );
    assert_eq!(
        row_color(&mut s, 2),
        WHITE,
        "and the selected row stays lit while another row is hovered"
    );

    // Click row 3: the white follows the selection, and row 2 falls back to its difficulty colour.
    s.run("TradeSkillSkill3:Click()").unwrap();
    pump(&mut s);
    park_cursor_off_the_list(&mut s);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        3
    );
    assert_eq!(row_color(&mut s, 3), WHITE, "the new selection is white");
    assert_eq!(
        row_color(&mut s, 2),
        MEDIUM,
        "the old selection is UNLOCKED back to its difficulty colour, not left lit"
    );
}

/// The craft list lights the same way: `CraftButtonTemplate` inherits the same base template, and
/// the update locks the selected row (`Blizzard_CraftUI.lua:234`).
#[test]
fn a_hovered_or_selected_craft_row_paints_its_label_white() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_screen_size(1024.0, 768.0);

    let recipe = |spell_id: u32, name: &str| CraftRecipe {
        spell_id,
        name: name.into(),
        sub_name: String::new(),
        difficulty: TradeSkillDifficulty::Medium,
        num_available: 1,
        icon: Some("Interface\\Icons\\Spell_Holy_Heal".into()),
        description: None,
        needs_item_target: false,
        reagents: vec![],
        tools: vec![],
        tooltip: CraftTooltip::Spell(spell_id),
        spell_level: 0,
    };
    s.set_craft(Some(CraftState {
        name: "Enchanting".into(),
        rank: 100,
        max_rank: 150,
        craft_type: 3,
        recipes: vec![
            recipe(7420, "Enchant Bracer - Minor Health"),
            recipe(7426, "Enchant Chest - Minor Absorption"),
        ],
    }));
    s.fire_event("CRAFT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        s.eval::<bool>("return Craft1:GetFontString() ~= nil")
            .unwrap(),
        "the craft row name is the Button's ButtonText"
    );

    let row_color = |s: &mut UiScript, n: i64| -> [f32; 4] {
        let text = s
            .eval::<String>(&format!("return Craft{n}:GetText()"))
            .unwrap();
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                benilla_ui::script::QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == text => color,
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text quad for craft row {n} (\"{text}\")"))
    };

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const MEDIUM: [f32; 4] = [1.0, 1.0, 0.0, 1.0]; // CraftTypeColor["medium"]

    // Row 1 is the show-time selection; row 2 is not.
    s.run("SelectCraft(1); CraftFrame_Update()").unwrap();
    s.resolve();
    s.mouse_move(1000.0, 20.0);
    assert_eq!(
        row_color(&mut s, 1),
        WHITE,
        "the selected craft row is white"
    );
    assert_eq!(
        row_color(&mut s, 2),
        MEDIUM,
        "an unselected one wears its difficulty colour"
    );

    // Hover row 2.
    s.resolve();
    let (x, y) = (
        s.eval::<f64>("return (Craft2:GetLeft() + Craft2:GetRight()) / 2")
            .unwrap(),
        s.eval::<f64>("return (Craft2:GetTop() + Craft2:GetBottom()) / 2")
            .unwrap(),
    );
    s.mouse_move(x as f32, y as f32);
    assert_eq!(row_color(&mut s, 2), WHITE, "hovered: white");

    // And the selection follows a click, releasing the old row's lock.
    s.run("SelectCraft(2); CraftFrame_Update()").unwrap();
    s.resolve();
    s.mouse_move(1000.0, 20.0);
    assert_eq!(row_color(&mut s, 2), WHITE);
    assert_eq!(
        row_color(&mut s, 1),
        MEDIUM,
        "the old selection is UNLOCKED, not left lit"
    );
}
