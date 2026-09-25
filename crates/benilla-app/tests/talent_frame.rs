//! Drives the stock `Blizzard_TalentUI` addon through the engine with a synthetic two-talent page
//! shaped like the warrior's Improved Rend and Deep Wounds column, asserting the Lua state
//! (`TALENT_BRANCH_ARRAY`), the frame state (`IsShown`) and the quads the renderer draws.

mod common;

use benilla_ui::script::{
    QuadContent, SpellTooltipView, TalentPrereqView, TalentTabView, TalentUiState, TalentView,
    UiScript, UnitState,
};

/// The talent window's load prefix, in `assets/ui/benilla.toc` order.
const FILES: &[&str] = &[
    // `PLAYER_LEVEL` and the other strings the stock file formats.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    // `TEXT`.
    "Interface\\FrameXML\\BasicControls.xml",
    // `SetItemButtonDesaturated`, which the talent buttons grey through.
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // `ToggleTalentFrame` (`UIParent.lua:205`).
    r"Interface\FrameXML\UIParent.xml",
    "Interface\\FrameXML\\GameTooltip.xml",
    // `UIPanelScrollFrameTemplate`: the scroll bar and the `<OnMouseWheel>`. A missing template
    // is only a loader warning, so the window would build without them.
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine
    // `TalentTabTemplate` inherits `CharacterFrameTabButtonTemplate`; `inherits=` resolves at load.
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    "ScrollTemplates.xml",
    // `TalentFrame_OnShow` pulses `TalentMicroButton` and calls `UpdateMicroButtons()`
    // (`Blizzard_TalentUI.lua:97-100`); a nil button throws before `TalentFrame_Update()`.
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    // Sources its own `.lua` and `<Include>`s the templates file.
    "Interface\\AddOns\\Blizzard_TalentUI\\Blizzard_TalentUI.xml",
];

fn load_ui(script: &UiScript) {
    for file in FILES {
        common::load_ui(script, file);
    }
}

/// A rank-3 prereq two tiers up the same column, the empty cell between carrying the branch.
/// `points_spent` of 10 or more unlocks tier 3 (yellow, `1`); fewer draws it gray (`-1`).
fn fixture(points_spent: u32) -> TalentUiState {
    let rend = TalentView {
        name: "Improved Rend".into(),
        texture: Some("Interface\\Icons\\Ability_Gouge".into()),
        tier: 1,
        column: 3,
        rank: 3,
        max_rank: 3,
        exceptional: false,
        meets_prereq: true,
        prereqs: Vec::new(),
        display_spell: 201,
        next_spell: 0,
        req_lines: Vec::new(),
        learnable: false,
    };
    let deep_wounds = TalentView {
        name: "Deep Wounds".into(),
        texture: Some("Interface\\Icons\\Ability_BackStab".into()),
        tier: 3,
        column: 3,
        rank: 0,
        max_rank: 3,
        exceptional: false,
        meets_prereq: true,
        prereqs: vec![TalentPrereqView {
            tier: 1,
            column: 3,
            learnable: true,
        }],
        display_spell: 301,
        next_spell: 0,
        req_lines: Vec::new(),
        learnable: true,
    };
    TalentUiState {
        tabs: vec![TalentTabView {
            name: "Arms".into(),
            background: "WarriorArms".into(),
            points_spent,
        }],
        talents: vec![vec![rend, deep_wounds]],
        points: (2, 0),
    }
}

/// [`fixture`] plus a last-tier talent, so the tree scrolls: the scroll child's extent comes from
/// the buttons in it via `UpdateScrollChildRect` (`Blizzard_TalentUI.lua:311`).
fn tall_fixture() -> TalentUiState {
    let mut state = fixture(10);
    let mut deep = state.talents[0][1].clone();
    deep.name = "Mortal Strike".into();
    deep.tier = 7;
    deep.rank = 0;
    deep.prereqs = Vec::new();
    deep.display_spell = 401;
    state.talents[0].push(deep);
    state
}

fn view(name: &str, desc: &str) -> SpellTooltipView {
    SpellTooltipView {
        name: name.into(),
        description: desc.into(),
        ..Default::default()
    }
}

/// Hover a talent button with the pointer: the stock `<OnEnter>` is inline, with no named
/// function to call.
fn hover(script: &mut UiScript, frame: &str) {
    script.resolve();
    let (x, y): (f32, f32) = script
        .eval(&format!("return {frame}:GetCenter()"))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter() — {e}"));
    script.mouse_move(x, y);
}

/// A level-60 player: `ToggleTalentFrame` opens nothing below level 10 (`UIParent.lua:206`).
fn probe_player() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Probefour".into()),
        level: 60,
        race: Some("Human".into()),
        class: Some("Warrior".into()),
        ..Default::default()
    }
}

fn open_window(points_spent: u32) -> UiScript {
    let mut script = UiScript::new().expect("engine");
    script.set_screen_size(1024.0, 768.0);
    load_ui(&script);
    script.set_unit("player", Some(probe_player()));
    script.set_talents(fixture(points_spent));
    script.run("ToggleTalentFrame()").expect("toggle");
    script
}

#[test]
fn branch_lines_draw_for_a_same_column_prereq() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut script = open_window(10);
    // The Lua state: tier 1's down edge, the empty tier-2 cell's up and down, tier 3's top arrow.
    script
        .run(
            r#"
            assert(TalentFrame:IsVisible(), "window visible")
            local a = TALENT_BRANCH_ARRAY
            assert(a[1][3].down == 1, "tier1 down, got " .. tostring(a[1][3].down))
            assert(a[2][3].up == 1, "tier2 up, got " .. tostring(a[2][3].up))
            assert(a[2][3].down == 1, "tier2 down, got " .. tostring(a[2][3].down))
            assert(a[3][3].topArrow == 1, "tier3 topArrow, got " .. tostring(a[3][3].topArrow))
            "#,
        )
        .expect("branch array");
    // The pooled textures: at least one branch and one arrow shown.
    script
        .run(
            r#"
            assert(TalentFrameBranch1:IsShown(), "branch 1 shown")
            assert(TalentFrameArrow1:IsShown(), "arrow 1 shown")
            "#,
        )
        .expect("pool state");
    // The extract: atlas-cropped quads (a full-sheet crop means SetTexCoord never landed).
    script.resolve();
    let quads = script.extract();
    let branch = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                tex_coords,
                ..
            } if p.contains("UI-TalentBranches") => Some(*tex_coords),
            _ => None,
        })
        .next();
    let Some(tex_coords) = branch else {
        panic!("no UI-TalentBranches quad in the extract");
    };
    assert!(
        tex_coords.is_some_and(|c| c.edges() != [0.0, 1.0, 0.0, 1.0]),
        "branch quad must be atlas-cropped, got {tex_coords:?}"
    );
    assert!(
        quads.iter().any(|q| matches!(
            &q.content,
            QuadContent::Texture { path: Some(p), .. } if p.contains("UI-TalentArrows")
        )),
        "no UI-TalentArrows quad in the extract"
    );
}

#[test]
fn tooltip_is_complete_on_the_first_hover() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut script = open_window(10);
    // With the talent spell views stored up front, the first OnEnter renders the full tooltip.
    script.set_spell_tooltip(201, view("Improved Rend", "Bleed harder."));
    script.set_spell_tooltip(301, view("Deep Wounds", "Bleed on crit."));
    hover(&mut script, "TalentFrameTalent2");
    script
        .run(
            r#"
            assert(GameTooltip:IsShown(), "tooltip shown on first hover")
            assert(GameTooltipTextLeft1:GetText() == "Deep Wounds",
                "name line, got " .. tostring(GameTooltipTextLeft1:GetText()))
            "#,
        )
        .expect("first hover");
}

#[test]
fn tooltip_miss_still_shows_the_rank_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut script = open_window(10);
    // An empty store: the fallback still shows the talent head and records the ask.
    hover(&mut script, "TalentFrameTalent2");
    script
        .run(
            r#"
            assert(GameTooltip:IsShown(), "tooltip shown on a store miss")
            assert(GameTooltipTextLeft1:GetText() == "Rank 0/3",
                "rank fallback, got " .. tostring(GameTooltipTextLeft1:GetText()))
            "#,
        )
        .expect("miss hover");
    let asks = script.take_spell_tooltip_asks();
    assert!(
        asks.contains(&301),
        "the miss records the ask, got {asks:?}"
    );
}

#[test]
fn a_locked_tier_still_draws_the_branch_gray() {
    let _data = benilla_formats::wow_data_or_skip!();
    // 2 points spent, tier 3 locked: the chain still draws, gray (the texcoord tables' `-1` keys).
    let script = open_window(2);
    script
        .run(
            r#"
            local a = TALENT_BRANCH_ARRAY
            assert(a[1][3].down == -1, "tier1 down gray, got " .. tostring(a[1][3].down))
            assert(a[3][3].topArrow == -1, "tier3 topArrow gray, got " .. tostring(a[3][3].topArrow))
            assert(TalentFrameBranch1:IsShown(), "gray branch shown")
            assert(TalentFrameArrow1:IsShown(), "gray arrow shown")
            "#,
        )
        .expect("gray branch");
}

/// An unavailable talent reaches the renderer desaturated (`Texture:SetDesaturated`, via
/// `SetItemButtonDesaturated`), not merely tinted; the learned talent is the control.
#[test]
fn an_unavailable_talent_reaches_the_renderer_desaturated() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut script = open_window(2);
    script.resolve();
    let icon = |quads: &[benilla_ui::script::ExtractedQuad], leaf: &str| -> (bool, [f32; 4]) {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color,
                    desaturated,
                    ..
                } if p.ends_with(leaf) => Some((*desaturated, color.unwrap_or([1.0; 4]))),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {leaf} icon quad in the extract"))
    };
    let quads = script.extract();

    // Deep Wounds: locked by the tier gate alone, since the player still has points to spend.
    let (grey, tint) = icon(&quads, "Ability_BackStab");
    assert!(
        grey,
        "an unavailable talent's icon must carry the greyscale flag — the whole of B162"
    );
    // The stock 0.65 tint still reaches the quad: the desaturate shader
    // (`Shaders\Pixel\Desaturate.bls`) discards its RGB, but its alpha is read on both paths.
    assert!(
        (tint[0] - 0.65).abs() < 1e-3,
        "the ref's 0.65 still lands on the quad (inert on RGB, live on alpha), got {tint:?}"
    );

    // The control: Improved Rend, learned to max.
    let (grey, tint) = icon(&quads, "Ability_Gouge");
    assert!(!grey, "a learned talent must NOT be greyed");
    assert!(
        (tint[0] - 1.0).abs() < 1e-3,
        "…and draws at full colour, got {tint:?}"
    );
}

#[test]
fn a_wheel_spin_over_the_grid_scrolls_the_tree() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The whole chain: hit test on a talent button, bubble to the ScrollFrame's OnMouseWheel,
    // step the Slider, whose OnValueChanged pans the frame.
    let mut script = UiScript::new().expect("engine");
    script.set_screen_size(1024.0, 768.0);
    load_ui(&script);
    script.set_unit("player", Some(probe_player()));
    script.set_talents(tall_fixture());
    script.run("ToggleTalentFrame()").expect("toggle");
    // Rects must be resolved before the wheel's hit-test can land on the grid.
    script.resolve();
    let (x, y): (f32, f32) = script
        .eval("return TalentFrameTalent1:GetCenter()")
        .expect("talent 1 center");
    // One notch down is -1; the handler steps the bar one valueStep forward.
    script.mouse_wheel(x, y, -1.0);
    script
        .run(
            r#"
            local scroll = TalentFrameScrollFrame:GetVerticalScroll()
            assert(scroll > 0,
                "one notch down pans the frame, got " .. tostring(scroll))
            assert(TalentFrameScrollFrameScrollBar:GetValue() == scroll,
                "the bar follows the frame, got " .. tostring(TalentFrameScrollFrameScrollBar:GetValue()))
            "#,
        )
        .expect("wheel down");
    script.mouse_wheel(x, y, 1.0);
    script
        .run(
            r#"
            assert(TalentFrameScrollFrame:GetVerticalScroll() == 0,
                "a notch up rewinds to the top, got " .. tostring(TalentFrameScrollFrame:GetVerticalScroll()))
            "#,
        )
        .expect("wheel up");
    assert!(script.errors().is_empty(), "{:?}", script.errors());
}
