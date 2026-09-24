//! Stock `Interface\FrameXML\UIParent.xml`'s addon-facing helpers, driven from Lua the way an addon drives them.
//!
//! The panel/ESC machinery in that file is covered by `panel_tests` and `escape_tests`; this is for
//! the loose functions the reference's `UIParent.lua` also defines, which benilla itself may never
//! call and an addon calls constantly.

use benilla_ui::script::UiScript;

/// Fonts (for any `inherits=`), then UIParent — the manifest's order.
fn ui_parent() -> UiScript {
    let mut s = UiScript::new().unwrap();
    // Through the chain-aware reader: this list names chain files now, and a
    // reader that joins `assets/ui` cannot resolve one (1838, 1887, 1888).
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
    ] {
        crate::ui_script::test_ui::load_ui(&s, file);
    }
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"Box = CreateFrame("Frame", "Box", UIParent)
           Box:SetWidth(100) Box:SetHeight(50)
           Box:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 200, 300)"#,
    )
    .unwrap();
    s.resolve();
    s
}

/// **`MouseIsOver` is the hover idiom, and its edge cases are the reference's, not the obvious
/// ones** — 9 corpus addons call it.
///
/// Three behaviours that a re-implementation would get wrong and a transcription gets right: the
/// offsets are *added* to every edge (so a positive `bottomOffset` raises the bottom rather than
/// lowering it), a nil `topOffset` zeroes **all four** even if the others were passed, and the
/// bounds are strict, so a cursor exactly on an edge is outside.
#[test]
fn mouse_is_over_is_the_references_own_box_test() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_parent();
    // The box is x 200..300, y 300..350.
    s.mouse_move(250.0, 325.0);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box)").unwrap(),
        Some(1),
        "dead centre"
    );

    s.mouse_move(150.0, 325.0);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box)").unwrap(),
        None,
        "100px to the left is out, and the miss is nil rather than false"
    );
    // ...but a leftOffset of -60 pushes the left edge out to 140 and takes it in.
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box, 0, 0, -60, 0)")
            .unwrap(),
        Some(1),
        "offsets are ADDED to each edge: leftOffset -60 grows the box leftward"
    );

    // A positive bottomOffset SHRINKS from below — the same addition, read the other way.
    s.mouse_move(250.0, 305.0);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box)").unwrap(),
        Some(1)
    );
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box, 0, 10, 0, 0)")
            .unwrap(),
        None,
        "bottom + 10 = 310, and the cursor is at 305"
    );

    // The reference's `if ( not topOffset )` zeroes ALL FOUR — so this 10 is ignored entirely.
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box, nil, 10, 0, 0)")
            .unwrap(),
        Some(1),
        "a nil topOffset discards the other three, exactly as 1.12 does"
    );

    // Strict bounds: exactly on an edge is outside.
    s.mouse_move(200.0, 325.0);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box)").unwrap(),
        None,
        "`x > left`, not `>=`"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **`RaiseFrameLevel` / `LowerFrameLevel` are FrameXML, not engine** — an addon calling them is
/// calling a function the client's own UI defines, and `BetterCharacterStats` dies at load without
/// them (`attempt to call global 'RaiseFrameLevel'`).
///
/// Two lines each in the reference and worth a test only because the direction is easy to swap:
/// Raise is `+1`, Lower is `-1`, and both read the frame's CURRENT level rather than a stored one,
/// so repeated calls accumulate.
#[test]
fn raise_and_lower_frame_level_step_the_frames_own_level() {
    benilla_formats::wow_data_or_skip!();
    let s = ui_parent();
    let base: i64 = s.eval("return Box:GetFrameLevel()").unwrap();

    s.run("RaiseFrameLevel(Box) RaiseFrameLevel(Box)").unwrap();
    assert_eq!(
        s.eval::<i64>("return Box:GetFrameLevel()").unwrap(),
        base + 2,
        "each call reads the level back, so they accumulate"
    );
    s.run("LowerFrameLevel(Box)").unwrap();
    assert_eq!(
        s.eval::<i64>("return Box:GetFrameLevel()").unwrap(),
        base + 1
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `randomseed` is an engine global in 1.12 beside `random`, and it was the missing half —
/// `IgniteStatus` calls it at file scope and dies on `attempt to call global`.
#[test]
fn randomseed_is_a_bare_global_like_random() {
    benilla_formats::wow_data_or_skip!();
    let s = ui_parent();
    // Seeding twice with the same value must produce the same first draw; that is the whole
    // contract an addon wants from it.
    let a: i64 = s
        .eval("randomseed(12345) return random(1, 1000000)")
        .unwrap();
    let b: i64 = s
        .eval("randomseed(12345) return random(1, 1000000)")
        .unwrap();
    assert_eq!(a, b, "the same seed gives the same sequence");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The reference's own "hack to fix a symptom not the real issue": a frame that has never been
/// laid out answers nil from `GetLeft()`, and `MouseIsOver` must return nil rather than raise.
#[test]
fn mouse_is_over_survives_a_frame_with_no_resolved_rect() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_parent();
    s.mouse_move(250.0, 325.0);
    s.run(r#"Floating = CreateFrame("Frame", "Floating", UIParent)"#)
        .unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Floating)")
            .unwrap(),
        None,
        "an unanchored frame is a nil answer, not an error"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── Fonts.xml's shared colour globals, as an addon reaches them ───────────────────────────────

/// **`RAID_CLASS_COLORS` is the table every raid addon paints names with** — 22 corpus addons, and
/// its absence was the whole of the harness's remaining `bad argument #1 to 'pairs' (table
/// expected, got nil)` row.
///
/// The shape matters as much as the values: addons walk it with `pairs` and index it by the
/// UPPERCASE class file token that `UnitClass`'s second return gives them, so the test does exactly
/// that rather than reading one key.
#[test]
fn raid_class_colors_is_the_references_own_nine() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // RAID_CLASS_COLORS comes off the chain with the font registry since 1888.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    s.set_screen_size(1024.0, 768.0);

    // PaintChips-2.0's own line, verbatim in shape — the one that was raising.
    let n: i64 = s
        .eval("local n = 0 for i, v in pairs(RAID_CLASS_COLORS) do n = n + 1 end return n")
        .unwrap();
    assert_eq!(n, 9, "1.12 has nine playable classes and one row each");

    let keys: String = s
        .eval(
            "local t = {} for k in pairs(RAID_CLASS_COLORS) do table.insert(t, k) end \
             table.sort(t) return table.concat(t, \" \")",
        )
        .unwrap();
    assert_eq!(
        keys, "DRUID HUNTER MAGE PALADIN PRIEST ROGUE SHAMAN WARLOCK WARRIOR",
        "keyed by UnitClass's uppercase second return, which is how addons index it"
    );

    // Spot the two the reference makes identical — a "fix" here would be a divergence.
    assert_eq!(
        s.eval::<(f64, f64, f64)>(
            "local p, h = RAID_CLASS_COLORS.PALADIN, RAID_CLASS_COLORS.SHAMAN \
             return p.r - h.r, p.g - h.g, p.b - h.b"
        )
        .unwrap(),
        (0.0, 0.0, 0.0),
        "SHAMAN shares PALADIN's pink in 1.12 — transcribed, not corrected"
    );
    assert_eq!(
        s.eval::<(f64, f64, f64)>("local c = RAID_CLASS_COLORS.HUNTER return c.r, c.g, c.b")
            .unwrap(),
        (0.67, 0.83, 0.45)
    );
}

/// **Every global `Fonts.xml` l.4-20 assigns** — the whole block, not the names someone noticed
/// were missing (1718).
///
/// The four font-PATH globals came first (l.4-7): this file had taken the colour block at l.8-19
/// and skipped the four lines above it. `STANDARD_TEXT_FONT` is why they matter — `Dewdrop-2.0`
/// calls `button.text:SetFont(STANDARD_TEXT_FONT, height)` **unguarded** at two of its three
/// sites, so a nil global is a failed `SetFont` on every menu button the Ace2 ecosystem draws.
///
/// Then the same class struck INSIDE the block this test was standing next to:
/// `LIGHTYELLOW_FONT_COLOR_CODE` (l.13) was absent, with `Fonts.xml`'s own comments skipping from
/// `-- l.12` to `-- l.14`, and aux-addon — which concatenates it at file scope — died on its first
/// line. This test could not catch it, because it enumerated the four names the previous round had
/// found rather than the block those four came out of. It now enumerates the block.
#[test]
fn the_font_path_globals_are_the_references_own_four() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // The four font-path globals come off the chain with the registry since 1888.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    s.set_screen_size(1024.0, 768.0);

    for name in [
        "STANDARD_TEXT_FONT",
        "UNIT_NAME_FONT",
        "DAMAGE_TEXT_FONT",
        "NAMEPLATE_FONT",
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return {name}")).unwrap(),
            "Fonts\\FRIZQT__.TTF",
            "{name} — 1.12 assigns the same face to all four, which is the reference's own \
             identity and not a simplification"
        );
    }

    // Dewdrop's own unguarded line, in shape: a nil global here is a failed SetFont on every
    // Ace2 menu button.
    s.run(
        r#"F = CreateFrame("Frame", "F", UIParent)
           T = F:CreateFontString("T", "ARTWORK")
           T:SetFont(STANDARD_TEXT_FONT, 10)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // ── The WHOLE block, not the names someone noticed were missing (1718) ──────────────────
    //
    // This test was written because the colour block was transcribed and the four font-path lines
    // above it were dropped. It then enumerated those four — and `LIGHTYELLOW_FONT_COLOR_CODE`,
    // l.13, sat missing INSIDE the colour block the whole time, with the comments in `Fonts.xml`
    // skipping from `-- l.12` to `-- l.14` where it should have been. aux-addon concatenates it at
    // file scope and died on its own first line.
    //
    // A list of "the names we found absent" cannot catch the next absent name. This is the
    // reference's `Fonts.xml` l.4-20 in full, so the next dropped line fails here instead of in an
    // addon: every global that block assigns, and nothing else.
    for name in [
        "STANDARD_TEXT_FONT",          // l.4
        "UNIT_NAME_FONT",              // l.5
        "DAMAGE_TEXT_FONT",            // l.6
        "NAMEPLATE_FONT",              // l.7
        "NORMAL_FONT_COLOR_CODE",      // l.8
        "HIGHLIGHT_FONT_COLOR_CODE",   // l.9
        "RED_FONT_COLOR_CODE",         // l.10
        "GREEN_FONT_COLOR_CODE",       // l.11
        "GRAY_FONT_COLOR_CODE",        // l.12
        "LIGHTYELLOW_FONT_COLOR_CODE", // l.13
        "FONT_COLOR_CODE_CLOSE",       // l.14
        "NORMAL_FONT_COLOR",           // l.15
        "HIGHLIGHT_FONT_COLOR",        // l.16
        "GRAY_FONT_COLOR",             // l.17
        "GREEN_FONT_COLOR",            // l.18
        "RED_FONT_COLOR",              // l.19
        "PASSIVE_SPELL_FONT_COLOR",    // l.20
    ] {
        assert!(
            s.eval::<bool>(&format!("return {name} ~= nil")).unwrap(),
            "{name} — ref Fonts.xml assigns it; a dropped line from a transcribed block is how              LIGHTYELLOW_FONT_COLOR_CODE went missing"
        );
    }
    // The one whose value the addons splice into a string, spelled out (ref l.13).
    assert_eq!(
        s.eval::<String>("return LIGHTYELLOW_FONT_COLOR_CODE")
            .unwrap(),
        "|cffffff9a"
    );
}

/// `MouseIsOver` divides the cursor by the frame's effective scale, as the reference does
/// (UIParent.lua l.1389-1390): a frame scaled to 0.5 whose screen footprint holds the cursor
/// answers 1 — with the division written out (the old "the scale is the constant 1" note) it
/// answered nil under the very cursor that had just entered it.
#[test]
fn mouse_is_over_reads_a_scaled_frame_in_its_own_units() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_parent();
    s.run(
        r#"Scaled = CreateFrame("Frame", "Scaled", UIParent) Scaled:SetWidth(200) Scaled:SetHeight(100)
           Scaled:SetPoint("BOTTOMLEFT", 100, 100) Scaled:SetScale(0.5)"#,
    )
    .unwrap();
    s.resolve();
    // The frame's screen footprint is its own-unit box times its effective scale (offsets and
    // size both scale); the probe sits at its centre, then just past its right edge.
    let (l, r, b, t, eff) = s
        .eval::<(f64, f64, f64, f64, f64)>(
            "return Scaled:GetLeft(), Scaled:GetRight(), Scaled:GetBottom(), Scaled:GetTop(), Scaled:GetEffectiveScale()",
        )
        .unwrap();
    assert!(
        (eff - 0.5).abs() < 1e-6 && (r - l - 200.0).abs() < 1e-3,
        "own units: {l}..{r} at {eff}"
    );
    let (cx, cy) = (((l + r) * 0.5 * eff) as f32, ((b + t) * 0.5 * eff) as f32);
    s.mouse_move(cx, cy);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Scaled)").unwrap(),
        Some(1),
        "the cursor at the scaled footprint's centre ({cx}, {cy})"
    );
    s.mouse_move((r * eff) as f32 + 10.0, cy);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Scaled)").unwrap(),
        None,
        "past the scaled footprint (inside the unscaled one) is outside"
    );
}
