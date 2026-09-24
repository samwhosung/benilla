//! The cast bar's lifecycle against the transcribed reference behavior (decision 0137 phase 1):
//! the extracted 1.12 `CastingBarFrame.lua` is the spec — orange fill that tracks the clock, green
//! completion flash + fade, red Failed with the 1 s hold, and the channel bar counting *down*.
//!
//! Two halves, and the second one is load-bearing. The state tests read the Lua back
//! (`GetValue`/`GetStatusBarColor`/`GetAlpha`), which is blind to what actually *paints* — the
//! original transcription dropped the reference's two `CastingBarFlash:Hide()` calls and every state
//! test still passed while a full-brightness additive bloom sat over the bar for the whole cast. The
//! [`draw list`](UiScript::extract) tests below are the ones that see it.

use benilla_ui::script::{ExtractedQuad, QuadContent, ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // The stock file writes `CastingBarText:SetText(FAILED)` rather than a literal, so the window
    // needs GlobalStrings — which the manifest has above it, and which our own retired
    // `CastingBar.xml` did not need because it hardcoded "Failed".
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

/// One tick of the app's real order (`tick_script` then `paint_script`): OnUpdate, resolve, then the draw list.
fn frame(s: &mut UiScript, dt: f32) -> Vec<ExtractedQuad> {
    s.tick(dt);
    s.resolve();
    s.extract()
}

/// The quad drawn from `Interface\...\<leaf>`, if any — texture regions keyed by their art.
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

    // The fill tracks GetTime: 1.5 s in (15 ticks) the value sits mid-window.
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

    // Completion: green, full, then the fade takes it below full alpha and eventually hides.
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
    // Pushback (`SMSG_SPELL_DELAYED` → `SPELLCAST_DELAYED`): a hit while casting must slide the
    // bar's window out (the spark jumps back and it keeps running), never hide or fail it —
    // decision 0256's open item, the "disappears on a hit" report.
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Fireball".into()), ScriptValue::Int(3000)],
    );
    // 1.5 s in — half-way through a 3 s cast.
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

    // The hit: a 0.5 s pushback.
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

    // The 1 s hold: still fully opaque half a second in…
    for _ in 0..5 {
        s.tick(0.1);
    }
    assert_eq!(
        s.eval::<f64>("return CastingBarFrame:GetAlpha()").unwrap(),
        1.0,
        "holds at full alpha inside CASTING_BAR_HOLD_TIME"
    );
    // …then fades out and hides.
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
    // SPELLCAST_CHANNEL_START(ms, name) — args reversed vs START, per the reference contract.
    // The name is whatever `ui_cast::channel_start_args` composed; for all but nine of the 323
    // channeled rows that is the literal word, which is what the feed hands Starshards too.
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

    // **In 1.12 the channel bar is ORANGE** — the same `SetStatusBarColor(1.0, 0.7, 0.0)` the cast
    // bar takes (stock `CastingBarFrame.lua` l.76 vs l.21, byte-identical to the copy in the
    // player's `patch.MPQ`); green is the COMPLETION flash and nothing else. A channel is told
    // apart by draining instead of filling, and by its label. **Classic Era's channel bar IS
    // green** (`CastingBarType.Channel`'s `classicFillColor = CASTBAR_CLASSIC_GREEN`) — a real
    // behaviour of a different client, which is why this is a gate and not a comment: if we ever
    // take Era's colour it is a deliberate deviation that has to come here first.
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

    // The server's mid-channel correction re-anchors the window (pushback shortens it).
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

/// **A completed channel closes on the BAR's own clock, because the server's stop runs a second
/// late** — and that is the path every naturally-finished channel in the game takes.
///
/// vmangos does not send `MSG_CHANNEL_UPDATE(0)` when a channel simply runs out.
/// `Spell::SendChannelUpdate(0, interrupted=false)` schedules a `ChannelResetEvent` **+1000 ms**
/// ("Else, we have some visual bugs (arcane projectile, last tick)"), and only that event's
/// `CancelSpellChannelingAnimationInstantly()` finally emits the packet. The *interrupt* path calls
/// the same function inline, which is why a broken channel stops at once and a finished one does
/// not.
///
/// So for a whole second after the fill empties there is no stop edge at all, and the stock
/// `OnUpdate` is what ends it: once `GetTime()` reaches `endTime` it clamps, `time == this.endTime`
/// trips, and the frame hands itself to `fadeOut` with **no green flash** — a finished channel
/// fades, where a cast completes green. The late packet then lands on a hidden frame and both of
/// `CastingBarFrame_OnEvent`'s stop-arm guards reject it.
///
/// Worth a test of its own because nothing else exercises it: the sibling test above fires
/// `SPELLCAST_CHANNEL_STOP` by hand, which is the *interrupt* timing, not this one.
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

    // The whole second the channel runs: still up, still orange, no stop packet has been sent.
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

    // Past endTime with nothing from the server: the frame ends itself.
    for _ in 0..40 {
        s.tick(0.1);
    }
    assert!(
        !s.eval::<bool>("return CastingBarFrame:IsShown()").unwrap(),
        "the bar closed on its own clock, a full second before the server says so"
    );

    // ~1 s after the fill emptied, the deferred ChannelResetEvent finally fires.
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

/// The completion bloom is an `alphaMode="ADD"` texture the size of the whole frame. The reference
/// keeps it hidden for the entire cast (`CastingBarFlash:Hide()` on every OnUpdate) and only shows
/// it, from alpha 0, once the cast lands. Dropping those two calls painted a full-brightness white
/// smear over the bar from the first frame — invisible to every state assertion above.
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

/// On completion the flash appears at alpha 0 and ramps by CASTING_BAR_FLASH_STEP (0.2) per
/// REFERENCE tick — the ref steps per rendered frame, and decision 0454 normalizes our steps to
/// its 30 Hz tick (`arg1 × CASTING_BAR_REF_TICK`), so a 1/30 s update advances exactly one
/// reference step and the tail is wall-clock stable at any render rate. The spark goes away.
/// The frame is still fully opaque through the ramp — the fade only starts once the flash has
/// finished.
#[test]
fn the_flash_blooms_from_zero_only_on_completion() {
    benilla_formats::wow_data_or_skip!();
    const REF_TICK: f32 = 1.0 / 30.0; // one reference tick of wall clock
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(3000)],
    );
    frame(&mut s, 0.1);
    s.fire_event("SPELLCAST_STOP", vec![]);

    // The event alone shows it at 0; each update adds a step. (The frame is opaque until the ramp
    // completes, so the quad's alpha IS the flash's own.)
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

/// A failed cast never flashes — the reference only arms the ramp on STOP/CHANNEL_STOP.
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

/// `drawLayer="BORDER"` on the `<StatusBar>`: the orange fill draws *under* the frame art, so the
/// border's bevel is never painted over. (Omitting the attribute defaults the fill to ARTWORK, the
/// border art's own layer, where declaration order put the fill on top.)
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

/// A StatusBar fill CROPs its texture — it never squeezes it (`0x770410`). At a
/// fraction f the quad is f·width wide AND samples u ∈ [0, f], so `UI-StatusBar`'s left-to-right
/// ramp keeps its true gradient at every fill level.
#[test]
fn the_fill_crops_its_texture_rather_than_stretching_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "SPELLCAST_START",
        vec![ScriptValue::Str("Frostbolt".into()), ScriptValue::Int(4000)],
    );
    // 1.0 s into a 4 s cast: a quarter filled.
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

/// The managed bottom-stack positions: the ref's UIParent.lua re-anchors the
/// cast bar and the chat window over whatever bottom bars are showing — the XML anchors
/// (55 / 85) are only pre-manage defaults. The bar visibilities are the mechanism's only
/// inputs, so plain Lua stubs exercising IsShown() stand in for the real always-on multibars
/// and the stance bar; the arithmetic asserted is the ref table's own
/// (base + bottomEither/bottomLeft + pet, and chat's bottomLeft-and-pet +23 extra).
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
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // the dropdown kit's MenuBackdrop reads TOOLTIP_DEFAULT_COLOR
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");

    // The loader's post-load bootstrap, replayed with no bars in existence: the bare bases.
    s.run("UIParent_ManageFramePositions()").unwrap();
    s.resolve();
    assert_eq!(
        bottom(&s, "CastingBarFrame"),
        60.0,
        "baseY replaces the XML 55"
    );
    assert_eq!(bottom(&s, "ChatFrame1"), 85.0, "chat baseY");

    // The always-on bottom multibars appear: bottomEither for the bar, bottomLeft for chat.
    // The bar stubs carry a no-op SetPoint: `MultiBarBottomLeft` and `ShapeshiftBarFrame` are
    // themselves rows in UIPARENT_MANAGED_FRAME_POSITIONS, so since those frames wear their
    // reference names the pass positions them as well as reading their visibility.
    // **The bottom-bar flags come from the SAVED GLOBALS, not from the frames.** The stock pass
    // reads `SHOW_MULTI_ACTIONBAR_1`/`_2` (`UIParent.lua:1598-1606`) and never asks the bars
    // whether they are shown — our retired copy asked, which is why this drive used to fake a
    // frame with an `IsShown`. The fake also had no `IsObjectType`, which the pass calls on every
    // row it seats (1988).
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s, "CastingBarFrame"), 100.0, "60 + bottomEither 40");
    assert_eq!(bottom(&s, "ChatFrame1"), 102.0, "85 + bottomLeft 17");

    // The stance bar shows (the warrior at login): the pet term, plus chat's both-flags extra.
    // The pass's shapeshift-appearance arm (the reference's own, UIParent.lua:1705-1732 — in ours
    // since 1938) touches the bar's three shelf textures by name, unguarded as the reference has
    // it; a stand-in frame needs stand-in textures.
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

    // It hides again (a druid leaving forms is the live case): everything settles back.
    s.run("ShapeshiftBarFrame:Hide() UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();
    assert_eq!(bottom(&s, "CastingBarFrame"), 100.0);
    assert_eq!(bottom(&s, "ChatFrame1"), 102.0);
}

/// **`CastingBarFrameStatusBar` is published, and it IS `CastingBarFrame`.**
///
/// `CastingBarFrame.lua:16` ends `CastingBarFrame_OnLoad` with `CastingBarFrameStatusBar =
/// CastingBarFrame` — a plain alias, because in 1.12 the casting bar is itself the StatusBar and
/// there is no separate child. Our transcription copied the eight `RegisterEvent`s and both field
/// inits and dropped that one line.
///
/// It cost 11 corpus addons, the largest single name in the session-start `attempt to index global`
/// row, and implementing it moved survivors 74 -> 85.
///
/// **This is the second time this file has been caught dropping lines from the same function** (see
/// the module doc: the original transcription also lost two `CastingBarFlash:Hide()` calls). A line
/// missing from INSIDE an otherwise-verbatim function is invisible to every instrument here —
/// `framexml-transcription-diff.py` compares elements and handlers, not bodies.
///
/// Asserted as identity, not just non-nil: an alias that pointed at some other frame would satisfy
/// "exists" and still break every addon that drives the bar through it.
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
    // And it drives: the addons that index it call StatusBar methods straight through.
    s.run("CastingBarFrameStatusBar:SetMinMaxValues(0, 10) CastingBarFrameStatusBar:SetValue(4)")
        .unwrap();
    assert!((bar_value(&s) - 4.0).abs() < 1e-9);
}
