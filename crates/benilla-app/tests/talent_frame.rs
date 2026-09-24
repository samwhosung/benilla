//! Drives the reference's own `Blizzard_TalentUI` addon through the engine — the first test that
//! executes the transcribed talent Lua at all (before this, the window's machinery only ever
//! ran inside a live client session; both of the director's day-one reports — no prereq branch
//! lines, no tooltip on first hover — slipped through that gap).
//!
//! The harness loads the same file chain the app does (`ui_script/mod.rs`'s list, cut to the
//! talent window's dependency prefix), pushes a synthetic two-talent page shaped like the
//! warrior's Improved Rend → Deep Wounds column, opens the window, and asserts at three
//! depths: the Lua-visible state (`TALENT_BRANCH_ARRAY`), the logical frame state (`IsShown`),
//! and the extract the renderer actually draws (branch/arrow quads with their atlas coords).

mod common;

use benilla_ui::script::{
    QuadContent, SpellTooltipView, TalentPrereqView, TalentTabView, TalentUiState, TalentView,
    UiScript, UnitState,
};

/// The talent window's load prefix — the app's own order (`assets/ui/benilla.toc`), members only.
/// `ItemButtonTemplate.xml` is the `SetItemButton*` family the talent buttons grey through (the
/// reference's own verb; see TalentFrame.xml's header) — it sits at .toc line 32, above every
/// other entry here bar `Fonts.xml`.
const FILES: &[&str] = &[
    // `PLAYER_LEVEL` and the rest of the strings the stock file formats through.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    // `TEXT`.
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    // `ToggleTalentFrame` lives here now, not in the window's own file (decision 1833).
    "Interface\\FrameXML\\GameTooltip.xml",
    // `UIPanelScrollFrameTemplate` — the stock scroll frame's whole substance: its `$parentScrollBar`
    // Slider AND its `<OnMouseWheel>`. Nothing else in the tree declares it, and a missing template
    // is a loader WARNING, not an error, so without this the window still built — just with no
    // scrollbar and no wheel (decision 1833). `load_ui_strict` below is what makes that loud.
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine (1960)
    // `TalentTabTemplate` inherits `CharacterFrameTabButtonTemplate`, and `inherits=`
    // resolves at LOAD (1993).
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    "ScrollTemplates.xml",
    // Stock `TalentFrame_OnShow` opens with `SetButtonPulse(TalentMicroButton, 0, 1)` and then
    // `UpdateMicroButtons()` — both live here, and a nil `TalentMicroButton` throws out of OnShow
    // BEFORE `TalentFrame_Update()`, so the whole window comes up empty (decision 1833). Our
    // retired file's OnShow called neither, which is why this was never a dependency before.
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    // Five files behind one line: the `.xml` sources its own `.lua` and `<Include>`s the
    // templates file that declares TalentButton/Branch/Arrow/TabTemplate.
    "Interface\\AddOns\\Blizzard_TalentUI\\Blizzard_TalentUI.xml",
];

fn load_ui(script: &UiScript) {
    for file in FILES {
        common::load_ui(script, file);
    }
}

/// The Improved Rend → Deep Wounds shape: a rank-3 prereq two tiers straight up the same
/// column, the empty cell between them carrying the vertical branch (the classic vanilla look).
/// `points_spent` decides the line's color: ≥10 unlocks tier 3 (yellow, `1`); fewer leaves it
/// locked (gray, `-1`) — the reference draws the branch EITHER way.
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

/// [`fixture`] plus a talent on the LAST tier, so the tree is taller than the scroll frame.
///
/// The stock `TalentFrameScrollChildFrame` is 50px in XML and is never resized by Lua — the
/// reference derives its extent from the buttons anchored into it via `UpdateScrollChildRect`
/// (`Blizzard_TalentUI.lua:311`). So a two-talent tree genuinely does not scroll, and asking it to
/// would be asserting our retired file's behaviour rather than the reference's: ours pinned the
/// child to a fixed full-height rect, which made every tree scrollable regardless of content
/// (decision 1833).
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

/// Hover a talent button the way a player does — the pointer, not a handler call.
///
/// The stock `Blizzard_TalentUITemplates.xml` writes the tooltip code INLINE in the button's
/// `<OnEnter>` (`GameTooltip:SetOwner` + `GameTooltip:SetTalent`), so there is no named function to
/// invoke the way our retired file's `BenillaTalentButton_OnEnter` could be (decision 1833). Moving
/// the mouse is closer to the thing under test anyway: it drives the hit test as well as the
/// handler.
fn hover(script: &mut UiScript, frame: &str) {
    script.resolve();
    let (x, y): (f32, f32) = script
        .eval(&format!("return {frame}:GetCenter()"))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter() — {e}"));
    script.mouse_move(x, y);
}

/// A level-60 player. The reference's `ToggleTalentFrame` opens nothing below level 10 (decision
/// 1833) — real behaviour, not a guard — so every opener here needs one. Our retired file had no
/// such gate, which is why this fixture did not exist before.
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
    // The reference's `ToggleTalentFrame` opens nothing below level 10 (decision 1833) — that is
    // real behaviour, not a guard, so the window needs a player who has talents at all. Our
    // retired file had no such gate, which is why this fixture was never needed before.
    script.set_unit("player", Some(probe_player()));
    script.set_talents(fixture(points_spent));
    script.run("ToggleTalentFrame()").expect("toggle");
    script
}

#[test]
fn branch_lines_draw_for_a_same_column_prereq() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut script = open_window(10);
    // Depth 1 — the Lua machinery: Update ran, the branch array carries the vertical chain
    // (tier 1's down-edge, the empty tier-2 cell's up+down, the button's own top arrow).
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
    // Depth 2 — the pooled textures took the work: at least one branch + one arrow shown.
    script
        .run(
            r#"
            assert(TalentFrameBranch1:IsShown(), "branch 1 shown")
            assert(TalentFrameArrow1:IsShown(), "arrow 1 shown")
            "#,
        )
        .expect("pool state");
    // Depth 3 — the renderer sees them: branch/arrow quads in the extract, cropped into the
    // atlas (a full-sheet [0,1] crop means SetTexCoord never landed).
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
    // The app's feed owes the store every talent spell view up front (the spellbook's own
    // arrival-driven contract) — with the views in place, the FIRST OnEnter must render the
    // full tooltip: name line, description, the green hint.
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
    // The store is EMPTY (no views pushed): the ask-once fallback must still show a box with
    // the talent head — never a blank hover — and record the ask for the app's resolver.
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
    // The director's own day-one state: 2 points spent, tier 3 locked — the reference draws
    // the chain anyway, gray (the `-1` keys of the texcoord tables; a typo'd negative key
    // would error the draw loop and hide every line while the grid stays perfect).
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

/// **B162 — an unavailable talent goes GREYSCALE, not merely dim.**
///
/// The reporter's own A/B: 1.12.1 at 0 talent points draws every unlearned icon in black and
/// white; benilla drew them in full colour. The Lua was never wrong — it asked for the grey-out
/// and the ask reached the region — but the engine had no `Texture:SetDesaturated`, so
/// `SetItemButtonDesaturated`'s no-shader arm was the only one available and "greyed" meant a
/// `SetVertexColor(0.65)` brightness multiply. On colourful art that reads as a slightly dimmer
/// colourful icon, which is exactly what was reported.
///
/// So the assertion is on the DESATURATION FLAG reaching the renderer, not on the tint: the tint
/// was always there and was never the thing that was missing. The fixture is the reporter's state
/// — points spent, none left — and Deep Wounds' tier is locked besides.
///
/// The control is the same extract's learned talent: full colour, no flag. A regression that
/// greys the whole tree passes the first assertion and fails this one.
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

    // Deep Wounds: tier 3 with 2 points spent — locked, and the player has 2 points to spend, so
    // this is the tier gate alone, not the no-points force-desaturate.
    let (grey, tint) = icon(&quads, "Ability_BackStab");
    assert!(
        grey,
        "an unavailable talent's icon must carry the greyscale flag — the whole of B162"
    );
    // The reference's own `(1, 0.65, 0.65, 0.65)` still SETS that tint, and on shader-capable
    // hardware it has no effect on colour — the desaturated fragment discards the vertex RGB
    // entirely (`Shaders\Pixel\Desaturate.bls`; decision 1330 corrected 1327 here). It is
    // pinned anyway because the value must keep reaching the quad: it is what the no-shader arm
    // would have drawn with, and its ALPHA is read on both paths.
    assert!(
        (tint[0] - 0.65).abs() < 1e-3,
        "the ref's 0.65 still lands on the quad (inert on RGB, live on alpha), got {tint:?}"
    );

    // The control: Improved Rend is learned to max, so it is available — full colour, no flag.
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
    // The wheel's whole chain, driven from the pointer entry point the app feeds: hit-test on
    // a talent button, bubble to the ScrollFrame's OnMouseWheel, step the Slider, whose
    // OnValueChanged pans the frame. This is the chain the 2051f4f8 rename broke silently —
    // the handler called a helper by a name that no longer existed, and only a real spin (not
    // a load) executes it.
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
    // One notch DOWN (WoW convention: -1) — the handler steps the bar one valueStep forward.
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
    // And the notch back up re-seats the top.
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
