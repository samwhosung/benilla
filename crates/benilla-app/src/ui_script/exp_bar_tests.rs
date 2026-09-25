//! The XP bar: its hover plate, the exhaustion tick and the on-bar numerals.

use benilla_ui::script::{QuadContent, ScriptValue, UiScript, UnitState};

fn exp_bar_harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // `ExhaustionTick_Update` reads `ReputationWatchBar`, which `ReputationFrame.xml`
        // declares; the two template files before it are what its check boxes inherit.
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
    ] {
        super::test_ui::load_ui(&s, file);
    }
    // The stock default, detailed tips on (`UIOptionsFrame.lua:100`); no options file loads here.
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s
}

#[test]
fn the_xp_bar_takes_the_mouse_and_explains_itself() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.resolve();

    // A quarter along, not the centre, where the page arrows overlap the strip and take the mouse.
    let (x, y) = s
        .eval::<(f64, f64)>(
            "return MainMenuExpBar:GetLeft() + MainMenuExpBar:GetWidth() / 4, \
                    (MainMenuExpBar:GetBottom() + MainMenuExpBar:GetTop()) / 2",
        )
        .unwrap();
    assert_eq!(
        s.hit_test_name(x as f32, y as f32).as_deref(),
        Some("MainMenuExpBar"),
        "the XP strip must be mouse-enabled or the hover never fires"
    );

    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "XP Bar"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft2:GetText()")
            .unwrap(),
        s.eval::<String>("return NEWBIE_TOOLTIP_XPBAR").unwrap(),
        "line 2 is the ref's NEWBIE_TOOLTIP_XPBAR, verbatim"
    );
    assert_eq!(
        s.eval::<i64>("return GameTooltip.default").unwrap(),
        1,
        "the default-corner anchor"
    );

    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnLeave\")()")
        .unwrap();
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "leaving hides the plate"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A 700 rested pool doubles to a 1400 XP span, so at 1000/10000 the tick sits at 2400/10000 of
/// the strip (`ExhaustionTick_Update`).
#[test]
fn the_exhaustion_tick_marks_where_rested_runs_out() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();

    s.set_player_xp(1000, 10000);
    s.set_rest_state(1, 700, true);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // The three bindings under the bar (`0x48d350`, `0x48d3f0`, `0x516ea0`).
    let (id, name, mult) = s
        .eval::<(i64, String, f64)>("return GetRestState()")
        .unwrap();
    assert_eq!((id, name.as_str(), mult), (1, "Rested", 2.0));
    assert_eq!(
        s.eval::<Option<i64>>("return GetXPExhaustion()").unwrap(),
        Some(1400),
        "the pool × Exhaustion.dbc row 1's factor (2.0) — bar-XP, not wire units"
    );
    assert_eq!(
        s.eval::<Option<i64>>("return IsResting()").unwrap(),
        Some(1)
    );

    let ok: bool = s
        .eval(
            r#"
            local tick, fill = ExhaustionTick, ExhaustionLevelFillBar
            local expected = (1000 + 1400) / 10000 * MainMenuExpBar:GetWidth()
            local x = tick:GetCenter()
            local r, g, b = MainMenuExpBar:GetStatusBarColor()
            -- fill's width via its RESOLVED edges: the authored size is 0 x 13, and the engine
            -- derives the span from the TOPLEFT + runtime-TOPRIGHT anchor pair (layout.rs's
            -- zero-size law), exactly as the real client does.
            return tick:IsVisible() and fill:IsShown()
               and math.abs((x - MainMenuExpBar:GetLeft()) - expected) < 0.5
               and math.abs((fill:GetRight() - fill:GetLeft()) - expected) < 0.5
               and r == 0.0 and math.abs(g - 0.39) < 0.001 and math.abs(b - 0.88) < 0.001
        "#,
        )
        .unwrap();
    assert!(ok, "tick at the rested boundary, fill up to it, bar blue");

    s.set_rest_state(2, 0, false);
    s.fire_event("UPDATE_EXHAUSTION", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local r, g, b = MainMenuExpBar:GetStatusBarColor()
            return not ExhaustionTick:IsShown()
               and not ExhaustionLevelFillBar:IsShown()
               and GetXPExhaustion() == nil
               and math.abs(r - 0.58) < 0.001 and g == 0.0 and math.abs(b - 0.55) < 0.001
        "#,
        )
        .unwrap();
    assert!(ok, "a dry pool hides the tick and returns the purple bar");

    // `GetXPExhaustion`'s nil follows the rest-state byte, not the pool (`0x48d3f0`): normal with
    // a remnant pool (the server's 0 < bonus <= 10 window) is nil, rested and drained is 0. An
    // unmapped byte fails `GetRestState` to three nils.
    s.set_rest_state(2, 5, false);
    s.fire_event("UPDATE_EXHAUSTION", vec![]);
    assert_eq!(
        s.eval::<Option<i64>>("return GetXPExhaustion()").unwrap(),
        None,
        "normal state hides the pool even while it holds a remnant"
    );
    s.set_rest_state(1, 0, true);
    s.fire_event("UPDATE_EXHAUSTION", vec![]);
    assert_eq!(
        s.eval::<Option<i64>>("return GetXPExhaustion()").unwrap(),
        Some(0),
        "rested with a drained pool is the number 0, not nil"
    );
    s.set_rest_state(0, 0, false);
    assert!(
        s.eval::<bool>("local a, b, c = GetRestState() return a == nil and b == nil and c == nil")
            .unwrap(),
        "an unmapped byte is the binary's (nil, nil, nil) fail path"
    );
    s.set_rest_state(2, 0, false);
    s.fire_event("UPDATE_EXHAUSTION", vec![]);

    // 6000 doubled runs past the level's end: `exhaustionTickSet > width` hides the tick.
    s.set_rest_state(1, 6000, false);
    s.fire_event("UPDATE_EXHAUSTION", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local r, g, b = MainMenuExpBar:GetStatusBarColor()
            return not ExhaustionTick:IsShown()
               and not ExhaustionLevelFillBar:IsShown()
               and math.abs(g - 0.39) < 0.001
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "a span past the level end hides the tick but keeps the blue"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ReputationWatchBar_Update` swaps in the rail at `MAX_PLAYER_LEVEL`, read from
/// `PLAYER_LEVEL_UP`'s arg1; the tick's handler runs first by load order, so the rail's hides it.
#[test]
fn the_max_level_rail_replaces_the_xp_bar_at_60() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.set_player_xp(1000, 10000);
    s.set_rest_state(1, 700, true);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 59,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    let ok: bool = s
        .eval(
            r#"
            return MainMenuExpBar:IsShown() and not MainMenuBarMaxLevelBar:IsShown()
               and ExhaustionTick:IsShown()
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "below 60 the strip and tick show, the rail stays hidden"
    );

    s.fire_event("PLAYER_LEVEL_UP", vec![ScriptValue::Int(60)]);
    let ok: bool = s
        .eval(
            r#"
            return not MainMenuExpBar:IsShown() and MainMenuBarMaxLevelBar:IsShown()
               and not ExhaustionTick:IsShown()
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "at 60 the rail replaces the strip and the tick goes with it"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A hidden-to-visible show re-stamps a frame to the tail (`0x76ae10`), but within a
/// `(strata, level)` bucket the layer outranks every stamp, so the `OVERLAY` caps stay on top.
#[test]
fn the_gryphons_outrank_the_bars_across_hide_show_cycles() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.set_player_xp(300, 400);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    // The strip hidden then re-shown, and the rail, born hidden, shown: both re-stamp.
    s.run("MainMenuExpBar:Hide() MainMenuExpBar:Show()")
        .unwrap();
    s.run("MainMenuBarMaxLevelBar:Show()").unwrap();
    s.resolve();
    let quads = s.extract();
    let z_max = |suffix: &str| {
        quads
            .iter()
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.ends_with(suffix))
            })
            .map(|q| q.z)
            .max()
    };
    let fill = z_max("UI-StatusBar").expect("the strip's fill");
    let rail = z_max("UI-MainMenuBar-MaxLevel").expect("the rail's plates");
    let cap = z_max("UI-MainMenuBar-EndCap-Dwarf").expect("the gryphon end caps");
    assert!(
        fill < cap && rail < cap,
        "the end caps must paint over the re-shown bars \
         (fill z={fill:#x}, rail z={rail:#x}, cap z={cap:#x})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `TextStatusBar.lua`'s `lockShow` count shows the numerals on hover; `statusBarText` is off.
#[test]
fn the_xp_bar_numerals_show_on_hover() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.set_player_xp(1234, 5678);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return MainMenuBarExpText:GetText()")
            .unwrap(),
        "1234 / 5678"
    );
    assert!(s
        .eval::<bool>("return MainMenuBarExpText:IsShown()")
        .unwrap());

    // `HideTextStatusBarText` reads `this.isZero` where it means `bar` (`TextStatusBar.lua:95`);
    // the engine always binds `this` during dispatch, so the test does too.
    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnLeave\")() this = nil")
        .unwrap();
    assert!(
        !s.eval::<bool>("return MainMenuBarExpText:IsShown()")
            .unwrap(),
        "leaving drops the lockShow refcount to 0 and hides the numerals"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_rest_state_line_joins_the_held_open_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.set_player_xp(1000, 10000);
    s.set_rest_state(1, 700, true);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return ExhaustionTick.timer").unwrap(),
        1.0,
        "the bar hover arms the tick's timer"
    );

    // Three 0.6 s ticks take the timer from 1 to below 0, the `< 0` edge.
    for _ in 0..3 {
        s.run("ExhaustionTick_OnUpdate(0.6)").unwrap();
    }
    let appended: String = s.eval("return GameTooltipTextLeft3:GetText()").unwrap();
    assert!(
        appended.contains("Rested") && appended.contains("200%"),
        "the rest-state line rides the open plate: {appended:?}"
    );
    assert!(
        s.eval::<bool>("return GameTooltip.canAddRestStateLine == nil")
            .unwrap(),
        "the handshake is consumed — the line never doubles"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
