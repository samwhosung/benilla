//! The stock cast bar (`CastingBarFrame.lua`): the fill tracking the clock, the green completion
//! flash and fade, red Failed with its 1 s hold, the channel bar draining. The state tests read the
//! Lua back, which cannot see what paints; the draw-list tests below see the flash.

use benilla_ui::script::{ExtractedQuad, QuadContent, ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // The stock file sets `CastingBarText` to the `FAILED` global (`CastingBarFrame.lua:61`).
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\CastingBarFrame.xml");
    s
}

fn bar_value(s: &UiScript) -> f64 {
    s.eval::<f64>("return CastingBarFrame:GetValue()").unwrap()
}

fn bar_color(s: &UiScript) -> (f64, f64, f64) {
    s.eval::<(f64, f64, f64)>("local r, g, b = CastingBarFrame:GetStatusBarColor(); return r, g, b")
        .unwrap()
}

/// One frame in the app's order (`tick_script`, then `paint_script`): OnUpdate, resolve, draw list.
fn frame(s: &mut UiScript, dt: f32) -> Vec<ExtractedQuad> {
    s.tick(dt);
    s.resolve();
    s.extract()
}

/// The texture quad whose art path ends in `leaf`, if any.
fn tex_quad<'a>(quads: &'a [ExtractedQuad], leaf: &str) -> Option<&'a ExtractedQuad> {
    quads.iter().find(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } => p.ends_with(leaf),
        _ => false,
    })
}

#[test]
fn cast_fills_then_completes_green_and_fades() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "hidden at load"
    );

    // SPELLCAST_START(name, ms): shown, orange, named, anchored to the clock.
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    assert!(s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return CastingBarText:GetText()").unwrap(),
        "Frostbolt"
    );
    let (r, g, b) = bar_color(&s);
    assert!(
        (r - 1.0).abs() < 1e-6 && (g - 0.7).abs() < 1e-6 && b.abs() < 1e-6,
        "casting is orange (got {r} {g} {b})"
    );

    let start = bar_value(&s);
    for _ in 0..15 {
        s.tick(0.1);
    }
    let mid = bar_value(&s);
    assert!(
        (mid - start - 1.5).abs() < 0.05,
        "1.5s of ticks advance the fill by 1.5 (got {})",
        mid - start
    );

    s.fire_event("SPELLCAST_STOP", vec![]);
    assert_eq!(bar_color(&s), (0.0, 1.0, 0.0), "completed is green");
    for _ in 0..30 {
        s.tick(0.1);
    }
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "the flash+fade ends hidden"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn a_hit_pushes_the_bar_back_it_does_not_cancel() {
    benilla_formats::wow_data_or_skip!();
    // Pushback (`SMSG_SPELL_DELAYED` fires `SPELLCAST_DELAYED`) slides the window out and never
    // hides or fails the bar (`CastingBarFrame.lua:69-74`).
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Fireball".into()), ScriptValue::Int(3000)],
    );
    for _ in 0..15 {
        s.tick(0.1);
    }
    let minmax = "local a, b = CastingBarFrame:GetMinMaxValues(); return a, b";
    let (_, max_before) = s.eval::<(f64, f64)>(minmax).unwrap();
    let remaining_before = max_before - bar_value(&s);
    assert!(
        (remaining_before - 1.5).abs() < 0.05,
        "half-way: ~1.5 s left (got {remaining_before})"
    );

    s.fire_event("SPELLCAST_DELAYED", vec![ScriptValue::Int(500)]);
    assert!(
        s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "a hit never hides the bar"
    );
    let (r, g, b) = bar_color(&s);
    assert!(
        (r - 1.0).abs() < 1e-6 && (g - 0.7).abs() < 1e-6 && b.abs() < 1e-6,
        "still orange — not failed/red (got {r} {g} {b})"
    );
    let (_, max_after) = s.eval::<(f64, f64)>(minmax).unwrap();
    assert!(
        (max_after - max_before - 0.5).abs() < 1e-6,
        "the window end moved out by the 0.5 s pushback"
    );
    let remaining_after = max_after - bar_value(&s);
    assert!(
        (remaining_after - remaining_before - 0.5).abs() < 0.05,
        "the cast now has ~0.5 s more to run (spark jumped back): {remaining_before} -> {remaining_after}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn failed_cast_turns_red_holds_then_fades() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    s.fire_event("SPELLCAST_FAILED", vec![]);
    assert_eq!(bar_color(&s), (1.0, 0.0, 0.0), "failed is red");
    assert_eq!(
        s.eval::<String>("return CastingBarText:GetText()").unwrap(),
        "Failed"
    );

    for _ in 0..5 {
        s.tick(0.1);
    }
    assert_eq!(
        s.eval::<f64>("return CastingBarFrame:GetAlpha()").unwrap(),
        1.0,
        "holds at full alpha inside CASTING_BAR_HOLD_TIME"
    );
    for _ in 0..30 {
        s.tick(0.1);
    }
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "after the hold the fade ends hidden"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn channel_counts_down_not_up() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // SPELLCAST_CHANNEL_START is (ms, name), reversed from START. The name is the word
    // "Channeling" for all but 9 of the 323 channeled spells (`ui_cast::channel_start_args`).
    s.fire_event(
        "SPELLCAST_CHANNEL_START",
        vec![
            ScriptValue::Int(6000),
            ScriptValue::Str("Channeling".into()),
        ],
    );
    assert!(s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return CastingBarText:GetText()").unwrap(),
        "Channeling"
    );

    // A channel opens orange like a cast (`CastingBarFrame.lua:76`, `:21`); green is only the
    // completion flash. The Era client's green channel bar is not 1.12's.
    let (r, g, b) = bar_color(&s);
    assert!(
        (r - 1.0).abs() < 1e-6 && (g - 0.7).abs() < 1e-6 && b.abs() < 1e-6,
        "a channel opens orange, exactly like a cast (got {r} {g} {b})"
    );

    let full = bar_value(&s);
    for _ in 0..10 {
        s.tick(0.1);
    }
    let after_1s = bar_value(&s);
    assert!(
        (full - after_1s - 1.0).abs() < 0.05,
        "a channel drains: 1s of ticks take the value DOWN by 1 (got {})",
        full - after_1s
    );

    // The server's mid-channel update re-anchors the window to the time it has left.
    s.fire_event("SPELLCAST_CHANNEL_UPDATE", vec![ScriptValue::Int(2000)]);
    s.tick(0.1);
    let corrected = bar_value(&s);
    assert!(
        corrected < after_1s,
        "an update to 2s-left pulls the fill further down ({corrected} < {after_1s})"
    );

    s.fire_event("SPELLCAST_CHANNEL_STOP", vec![]);
    assert_eq!(bar_color(&s), (0.0, 1.0, 0.0), "channel end flashes green");
    for _ in 0..30 {
        s.tick(0.1);
    }
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "ends hidden"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A channel that runs out closes on the bar's own clock: vmangos defers the stop by 1000 ms on a
/// natural end and sends it at once only on an interrupt (`Spell.cpp:4906-4916`). The stock
/// `OnUpdate` clamps at `endTime` and fades with no green flash, and the late stop lands on a
/// hidden frame, which both stop-arm guards ignore.
#[test]
fn a_finished_channel_fades_on_its_own_clock_and_the_late_stop_is_inert() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_CHANNEL_START",
        vec![
            ScriptValue::Int(1000),
            ScriptValue::Str("Channeling".into()),
        ],
    );
    assert!(s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap());

    for _ in 0..9 {
        s.tick(0.1);
    }
    assert!(
        s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "still channelling at 0.9 s of a 1 s channel"
    );
    let (r, g, b) = bar_color(&s);
    assert!(
        (r - 1.0).abs() < 1e-6 && (g - 0.7).abs() < 1e-6 && b.abs() < 1e-6,
        "never flashes green — a finished channel fades, it does not complete (got {r} {g} {b})"
    );

    for _ in 0..40 {
        s.tick(0.1);
    }
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "the bar closed on its own clock, a full second before the server says so"
    );

    // The deferred stop, about 1 s after the fill emptied.
    s.fire_event("SPELLCAST_CHANNEL_STOP", vec![]);
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "the late stop lands on a hidden frame and does nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The draw list ────────────────────────────────────────────────────────────────────────────────

const FLASH: &str = "UI-CastingBar-Flash";
const SPARK: &str = "UI-CastingBar-Spark";
const BORDER: &str = "UI-CastingBar-Border";
const FILL: &str = "UI-StatusBar";

/// The completion flash, an additive texture over the whole frame, stays hidden through the cast:
/// every casting `OnUpdate` hides it (`CastingBarFrame.lua:109`).
#[test]
fn the_flash_stays_hidden_for_the_whole_cast() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    for i in 0..25 {
        let quads = frame(&mut s, 0.1);
        assert!(
            tex_quad(&quads, FLASH).is_none(),
            "tick {i}: the flash must not draw during a cast"
        );
        assert!(
            tex_quad(&quads, SPARK).is_some(),
            "tick {i}: the spark rides the fill's leading edge"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// On completion the flash shows at alpha 0 and each `OnUpdate` adds `CASTING_BAR_FLASH_STEP`
/// (0.2), once per frame whatever its length (`CastingBarFrame.lua:132-139`). The spark hides, and
/// the frame stays opaque until the ramp ends.
#[test]
fn the_flash_blooms_from_zero_only_on_completion() {
    benilla_formats::wow_data_or_skip!();
    const REF_TICK: f32 = 1.0 / 30.0; // any frame time: the ramp steps once per OnUpdate
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    frame(&mut s, 0.1);
    s.fire_event("SPELLCAST_STOP", vec![]);

    // The frame is opaque through the ramp, so the quad's alpha is the flash's own.
    for (i, expected) in [0.2f32, 0.4, 0.6, 0.8, 1.0].into_iter().enumerate() {
        let quads = frame(&mut s, REF_TICK);
        let flash = tex_quad(&quads, FLASH).expect("the flash draws after completion");
        assert!(
            (flash.alpha - expected).abs() < 1e-5,
            "ramp step {i}: flash alpha {} != {expected}",
            flash.alpha
        );
        assert!(
            tex_quad(&quads, SPARK).is_none(),
            "ramp step {i}: the spark is hidden once the cast lands"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Only `SPELLCAST_STOP` and `SPELLCAST_CHANNEL_STOP` arm the ramp (`CastingBarFrame.lua:35-54`).
#[test]
fn a_failed_cast_never_flashes() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    frame(&mut s, 0.1);
    s.fire_event("SPELLCAST_FAILED", vec![]);
    for i in 0..25 {
        let quads = frame(&mut s, 0.1);
        assert!(
            tex_quad(&quads, FLASH).is_none(),
            "tick {i}: a failed cast must not bloom"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `drawLayer="BORDER"` on the `<StatusBar>` (`CastingBarFrame.xml:4`) draws the fill under the
/// ARTWORK border art.
#[test]
fn the_fill_draws_beneath_the_border_art() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    let quads = frame(&mut s, 0.1);
    let fill = tex_quad(&quads, FILL).expect("fill");
    let border = tex_quad(&quads, BORDER).expect("border");
    assert!(
        fill.z < border.z,
        "the fill (z {}) draws before the border art (z {})",
        fill.z,
        border.z
    );
}

/// A StatusBar fill crops its texture rather than squeezing it (`0x770410`): at fraction f the
/// quad is f of the width and samples u in [0, f].
#[test]
fn the_fill_crops_its_texture_rather_than_stretching_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(4000)],
    );
    for _ in 0..10 {
        frame(&mut s, 0.1);
    }
    let quads = frame(&mut s, 0.0);
    let fill = tex_quad(&quads, FILL).expect("fill");
    let QuadContent::Texture { tex_coords, .. } = &fill.content else {
        panic!("fill is a texture")
    };
    let [l, r, t, b] = tex_coords
        .expect("a bar fill always carries its crop")
        .edges();
    let rect = fill.rect.expect("fill rect");
    let frac = rect.width() / 195.0;
    assert!((frac - 0.25).abs() < 0.02, "a quarter filled (got {frac})");
    assert!(
        (l - 0.0).abs() < 1e-6 && (t - 0.0).abs() < 1e-6 && (b - 1.0).abs() < 1e-6,
        "a horizontal bar crops only the u axis (got {l} {r} {t} {b})"
    );
    assert!(
        (r - frac).abs() < 1e-5,
        "u1 tracks the fill fraction: {r} != {frac}"
    );
}

/// The resolved bottom edge of a named frame, in UI units (y-up from the screen bottom).
fn bottom(s: &UiScript, name: &str) -> f64 {
    s.eval::<f64>(&format!("return {name}:GetBottom()"))
        .unwrap()
}

/// The stock manage pass re-seats the cast bar and `ChatFrame1` over the bottom bars shown; the
/// XML anchors (55, 85) are pre-pass defaults. Each sum is the stock table's base plus its bar
/// and `pet` terms, with chat's extra 23 when both apply (`UIParent.lua:1657-1659`).
#[test]
fn managed_positions_track_the_bottom_bar_stack() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\CastingBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit the chat menus build from
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TOOLTIP_DEFAULT_COLOR for the dropdowns
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");

    // The load-time pass with no bars shown: the bare bases.
    s.run("UIParent_ManageFramePositions()").unwrap();
    s.resolve();
    assert_eq!(
        bottom(&s, "CastingBarFrame"),
        60.0,
        "baseY replaces the XML 55"
    );
    assert_eq!(bottom(&s, "ChatFrame1"), 85.0, "chat baseY");

    // Both bottom multibars on: `bottomEither` for the cast bar, `bottomLeft` for chat. The flags
    // come from `SHOW_MULTI_ACTIONBAR_1`/`_2`, not the frames (`UIParent.lua:1599-1607`).
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s, "CastingBarFrame"), 100.0, "60 + bottomEither 40");
    assert_eq!(bottom(&s, "ChatFrame1"), 102.0, "85 + bottomLeft 17");

    // The stance bar shows: the pet term, plus chat's both-flags extra. The pass's stance-bar arm
    // (`UIParent.lua:1705-1732`) shows and hides the bar's three shelf textures by name, unguarded.
    s.run(
        "local t = { Show = function() end, Hide = function() end } \
         ShapeshiftBarLeft, ShapeshiftBarMiddle, ShapeshiftBarRight = t, t, t",
    )
    .unwrap();
    s.run(
        "ShapeshiftBarFrame = ShapeshiftBarFrame or CreateFrame(\"Frame\", \"ShapeshiftBarFrame\") \
         ShapeshiftBarFrame:Show() UIParent_ManageFramePositions()",
    )
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s, "CastingBarFrame"), 140.0, "60 + 40 + pet 40");
    assert_eq!(
        bottom(&s, "ChatFrame1"),
        142.0,
        "85 + 17 + pet 17 + both-flags 23"
    );

    s.run("ShapeshiftBarFrame:Hide() UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s, "CastingBarFrame"), 100.0);
    assert_eq!(bottom(&s, "ChatFrame1"), 102.0);
}

/// `CastingBarFrame_OnLoad` publishes `CastingBarFrameStatusBar` as an alias of the bar itself
/// (`CastingBarFrame.lua:16`): in 1.12 the casting bar is the StatusBar, with no child. Addons
/// drive the bar through it, so this asserts identity, not just existence.
#[test]
fn the_casting_bar_publishes_its_status_bar_alias() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    assert!(
        s.eval::<bool>("return CastingBarFrameStatusBar ~= nil")
            .unwrap(),
        "the reference publishes this name in CastingBarFrame_OnLoad"
    );
    assert!(
        s.eval::<bool>("return CastingBarFrameStatusBar == CastingBarFrame")
            .unwrap(),
        "the alias IS the bar — 1.12 has no separate child StatusBar"
    );
    s.run("CastingBarFrameStatusBar:SetMinMaxValues(0, 10) CastingBarFrameStatusBar:SetValue(4)")
        .unwrap();
    assert!((bar_value(&s) - 4.0).abs() < 1e-9);
}
