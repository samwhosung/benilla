//! The XP bar's own tests — the hover plate, the rested/exhaustion-tick system and the
//! on-bar numerals — split from `action_bar_tests.rs`, which keeps the
//! action-button machinery. Same file, two concerns: the strip along the bar's top is the
//! player-progress readout; the buttons are the input surface.

use benilla_ui::script::{QuadContent, ScriptValue, UiScript, UnitState};

/// The XP bar's load set: the manifest prefix it actually needs — fonts, UIParent, the
/// tooltip, `TextStatusBar.xml` (the numerals machinery `BenillaExpBar_OnLoad` wires in),
/// then Cooldown + the bar itself (manifest order).
fn exp_bar_harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Through the shared both-stores loader, not a disk read off `assets/ui`: `TextStatusBar.xml`
    // is the reference's own since 1751, and a hand-rolled reader here would look for it in the
    // wrong store — and would leave its `<Script file="TextStatusBar.lua"/>` unresolved besides.
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
        // The reference declares the reputation WATCH BAR in `ReputationFrame.xml`, and
        // `ExhaustionTick_Update` reads `ReputationWatchBar:IsShown()` twice — the reference's own
        // coupling of MainMenuBar to that pane. So an action-bar harness loads it, and with it the
        // two template files its check boxes inherit through (1875).
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
    ] {
        super::test_ui::load_ui(&s, file);
    }
    // 1.12 ships detailed tips ON — `SHOW_NEWBIE_TIPS = "1"` is UIOptionsFrame_Init's (ref
    // UIOptionsFrame.lua l.100; ours sits in OptionsFrame.xml's uvar block, 1968), and a harness
    // without the options file says so itself, the way the reference's tooltip would read it.
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s
}

/// The XP bar's hover (ref-MainMenuBar.xml l.136-147): the strip takes the mouse, and the plate is
/// the ref's two-line `GameTooltip_AddNewbieTip` — "XP Bar" over the wrapped explanation.
///
/// `enableMouse` is the load-bearing half and the easy thing to lose: without it the strip is
/// transparent and the hover silently never fires, which no tooltip assertion alone would catch.
#[test]
fn the_xp_bar_takes_the_mouse_and_explains_itself() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.resolve();

    // Mid-height of the strip (the bar's top 13 px), a quarter of the way along — NOT its
    // horizontal center, where the page arrows straddle the strip's lower edge and rightly take
    // the mouse ahead of it.
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

/// The exhaustion tick (ref `ExhaustionTick_Update`): with 1000/10000 XP and a
/// rested pool of 700 base-XP, the doubled span is 1400 bar-XP, so the tick parks at
/// (1000+1400)/10000 of the strip's width and the pale fill stretches exactly to it; the bar
/// paints rested blue. Draining the pool (normal state) hides both and returns the purple; a
/// pool whose span runs past the level's end hides the tick but keeps the rested blue.
#[test]
fn the_exhaustion_tick_marks_where_rested_runs_out() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();

    s.set_player_xp(1000, 10000);
    s.set_rest_state(1, 700, true);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // The binding trio underneath (the app-feed shape the XML consumes) — the reference's
    // contract (`0x48d350`, `0x48d3f0`, `0x516ea0`).
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

    // The pool drains dry: state normal, no threshold — tick and fill gone, purple back.
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

    // The nil law is the BYTE's, not the pool's (0x48d3f0's `dec/jne`): a normal-
    // state player with a nonzero pool — vmangos's 0 < bonus ≤ 10 hysteresis window — still reads
    // nil, and a rested player with a drained pool reads the NUMBER 0 (the tick then parks at the
    // bar's current fill). An unmapped byte (0 here) is the binary's nil-triple fail path.
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

    // A pool past the level's end (6000×2 span from 1000/10000): off the strip → hidden tick,
    // still rested blue (the ref's exhaustionTickSet > width branch).
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

/// The max-level rail (ref `ReputationWatchBar_Update`'s no-watched-faction arm):
/// at MAX_PLAYER_LEVEL the XP strip and its tick give way to the flat brass rail; below it the
/// strip is back. Walked over a live ding — 59 → 60 via `PLAYER_LEVEL_UP`'s arg1, the value the
/// ref reads (not the level field, which may not have landed yet). The tick's own handler runs
/// first (it loaded first) and re-shows the tick off the rested pool; the rail's handler then
/// hides it — the ref's own load order, ending hidden.
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

/// The gryphon end caps stay over the strip and the rail across ANY show/hide history —
/// including the faithful hidden→visible tail re-stamp (0x76ae10 / 0884, which a stale-rail
/// recovery exercises: decision 1106's whole bug walked through here). The protection is
/// 0884's own key order: the caps ride the art frame's OVERLAY and the strip's fill its
/// ARTWORK, and within a `(strata, level)` bucket the LAYER outranks every stamp — no
/// re-stamp can lift a lower layer over them.
#[test]
fn the_gryphons_outrank_the_bars_across_hide_show_cycles() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.set_player_xp(300, 400);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    // The relog shape: the strip hidden (a stale max-level rail), then re-shown; the rail's
    // own arrival is ALWAYS a hidden→visible re-stamp (it is born hidden).
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

/// The hover numerals (ref TextStatusBar.lua over `MainMenuBarExpText`): entering
/// the strip shows "curr / max" centered on the bar, leaving hides it (the lockShow refcount's
/// 0↔1 edge — the `statusBarText` cvar that pins it on permanently reads OFF here).
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

    // `this` bound, because the engine binds it: the stock `HideTextStatusBarText` reads
    // `this.isZero` on one line where every other line reads `bar` (TextStatusBar.lua:95 — the
    // reference's own slip, benign there because `this` is always SOME frame during dispatch, and
    // a nil index only from bare Lua). Called from the bar's own `<OnLeave>` in play.
    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnLeave\")() this = nil")
        .unwrap();
    assert!(
        !s.eval::<bool>("return MainMenuBarExpText:IsShown()")
            .unwrap(),
        "leaving drops the lockShow refcount to 0 and hides the numerals"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The rest-state tooltip line (ref `ExhaustionToolTipText`'s newbie-tips branch):
/// hovering the BAR arms a 1 s timer and the canAddRestStateLine handshake; when the timer runs
/// out on the bar's OnUpdate, the "Rested / 200% of normal experience" line is APPENDED to the
/// held-open newbie tip, once (the handshake is consumed).
#[test]
fn the_rest_state_line_joins_the_held_open_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let mut s = exp_bar_harness();
    s.set_player_xp(1000, 10000);
    s.set_rest_state(1, 700, true);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    // 1.12's default posture: detailed newbie tips ON.
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s.run("this = MainMenuExpBar MainMenuExpBar:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return ExhaustionTick.timer").unwrap(),
        1.0,
        "the bar hover arms the tick's timer"
    );

    // Three OnUpdate ticks walk the timer 1 → 0.4 → −0.2 → fire (the ref's `< 0` edge).
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
