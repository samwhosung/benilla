//! The loose helpers stock `UIParent.lua` defines for addons, driven from Lua as an addon does.

use benilla_ui::script::UiScript;

/// Fonts (for any `inherits=`), then UIParent, in manifest order.
fn ui_parent() -> UiScript {
    let mut s = UiScript::new().unwrap();
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

/// Stock `MouseIsOver` (`UIParent.lua:1388`) adds each offset to its edge, zeroes all four on a
/// nil `topOffset`, and tests strictly, so a cursor on an edge is outside.
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
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box, 0, 0, -60, 0)")
            .unwrap(),
        Some(1),
        "offsets are ADDED to each edge: leftOffset -60 grows the box leftward"
    );

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

    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box, nil, 10, 0, 0)")
            .unwrap(),
        Some(1),
        "a nil topOffset discards the other three, exactly as 1.12 does"
    );

    s.mouse_move(200.0, 325.0);
    assert_eq!(
        s.eval::<Option<i64>>("return MouseIsOver(Box)").unwrap(),
        None,
        "`x > left`, not `>=`"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `RaiseFrameLevel` and `LowerFrameLevel` are FrameXML, not engine (`UIParent.lua:1890`).
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

/// `randomseed` is an engine global in 1.12 beside `random`.
#[test]
fn randomseed_is_a_bare_global_like_random() {
    benilla_formats::wow_data_or_skip!();
    let s = ui_parent();
    let a: i64 = s
        .eval("randomseed(12345) return random(1, 1000000)")
        .unwrap();
    let b: i64 = s
        .eval("randomseed(12345) return random(1, 1000000)")
        .unwrap();
    assert_eq!(a, b, "the same seed gives the same sequence");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `MouseIsOver` returns nil for a frame with no `GetLeft()` (`UIParent.lua:1405`).
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

// ── Fonts.xml's shared globals ────────────────────────────────────────────────────────────────

/// Addons walk `RAID_CLASS_COLORS` (`Fonts.xml:43`) with `pairs` and index it by the class file
/// token, `UnitClass`'s second return.
#[test]
fn raid_class_colors_is_the_references_own_nine() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\Fonts.xml");
    s.set_screen_size(1024.0, 768.0);

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

    // SHAMAN has PALADIN's colour in the reference; not a typo to fix.
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

/// The four font-path globals (`Fonts.xml:4-7`) and every other global `Fonts.xml:4-20` assigns.
#[test]
fn the_font_path_globals_are_the_references_own_four() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
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

    // Addons call `SetFont(STANDARD_TEXT_FONT, …)` unguarded.
    s.run(
        r#"F = CreateFrame("Frame", "F", UIParent)
           T = F:CreateFontString("T", "ARTWORK")
           T:SetFont(STANDARD_TEXT_FONT, 10)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // Every global `Fonts.xml:4-20` assigns, so a dropped line fails here.
    for name in [
        "STANDARD_TEXT_FONT",
        "UNIT_NAME_FONT",
        "DAMAGE_TEXT_FONT",
        "NAMEPLATE_FONT",
        "NORMAL_FONT_COLOR_CODE",
        "HIGHLIGHT_FONT_COLOR_CODE",
        "RED_FONT_COLOR_CODE",
        "GREEN_FONT_COLOR_CODE",
        "GRAY_FONT_COLOR_CODE",
        "LIGHTYELLOW_FONT_COLOR_CODE",
        "FONT_COLOR_CODE_CLOSE",
        "NORMAL_FONT_COLOR",
        "HIGHLIGHT_FONT_COLOR",
        "GRAY_FONT_COLOR",
        "GREEN_FONT_COLOR",
        "RED_FONT_COLOR",
        "PASSIVE_SPELL_FONT_COLOR",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {name} ~= nil")).unwrap(),
            "{name} — ref Fonts.xml assigns it; a dropped line from a transcribed block is how              LIGHTYELLOW_FONT_COLOR_CODE went missing"
        );
    }
    // The value addons splice into strings (`Fonts.xml:13`).
    assert_eq!(
        s.eval::<String>("return LIGHTYELLOW_FONT_COLOR_CODE")
            .unwrap(),
        "|cffffff9a"
    );
}

/// `MouseIsOver` divides the cursor by the frame's effective scale (`UIParent.lua:1389`).
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
    // The screen footprint is the own-unit box times the effective scale.
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
