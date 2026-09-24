//! **Shape C, closed as a CLASS**: every shape-C numeric position must take a `nil` — or a
//! table, or a string — as **0.0** and complete, because the reference reads it with a bare
//! `lua_tonumber 0x6f3620` and no `lua_isnumber` gate at all.
//!
//! This is one test and not six because which shape a given argument takes is per binding, not a
//! global law: the reference's 408 widget-registrar positions (110 gated, 64 ungated) are a table
//! a re-implementation is checked against, not an instinct. Our half of that table had drifted: 1973 closed `SetTextColor` and left every sibling
//! strict, so mlua's converter went on being the gate for five more years' worth of call sites.
//!
//! The live symptom that forced the audit: stock `QuestLogFrame.lua:337` does
//! `QuestLogSkillHighlight:SetVertexColor(titleButton.r, titleButton.g, titleButton.b)`, and those
//! three fields are only ever assigned in `QuestLog_Update` — so **any** path that selects a
//! quest-log entry before the window has painted hands `SetVertexColor` three nils. Questie's
//! `syncQuestWatch` (`QuestieTracker.lua:1160`) is exactly such a path: it calls
//! `QuestLog_SetSelection` first. On the reference that is a no-op that stores black; for us it
//! raised, and because the raise happened inside Questie's own event-queue drain — which nils an
//! entry only *after* its handler returns — the queue jammed at that entry permanently. Every
//! later entry (`SYNCLOG`, `DRAWNOTES`, `TRACKER`) was stranded for the rest of the session, which
//! is why the minimap button's right-click could turn the quest icons OFF (a direct call) but
//! never back ON (a `DRAWNOTES` enqueue).
//!
//! **Shape B is not shape C and is not tested here.** The alpha slot of every colour setter is B —
//! a pre-staged default, `1.0f` at `0x778220` — so its absence is a *default*, not a zero, and
//! `SetVertexColor`'s own B-slot carries a separate, deliberately-open divergence (1782).

use super::common::script;

/// Each row: the Lua that must not raise, and what a reader must say afterwards.
#[test]
fn every_shape_c_colour_position_takes_nil_as_zero_and_never_raises() {
    let s = script();
    s.run(
        r#"
        F = CreateFrame("Frame", "F")
        T = F:CreateTexture(nil, "ARTWORK")
        FS = F:CreateFontString(nil, "ARTWORK")
        SB = CreateFrame("StatusBar", "SB")
        CS = CreateFrame("ColorSelect", "CS")
        "#,
    )
    .unwrap();

    // `Texture:SetVertexColor 0x79abd0` — `2=C 3=C 4=C`. The QuestLogFrame.lua:337 shape verbatim.
    s.run("T:SetVertexColor(nil, nil, nil)")
        .expect("SetVertexColor(nil,nil,nil) is three bare lua_tonumbers, not a raise");
    s.run(
        r#"
        local r, g, b = T:GetVertexColor()
        assert(r == 0 and g == 0 and b == 0, "a nil channel stores 0.0, it does not keep the old one")
        "#,
    )
    .unwrap();
    // A table and a string are the same 0.0 — `lua_tonumber` tests nothing.
    s.run("T:SetVertexColor({}, \"abc\", true)")
        .expect("no tag is rejected at a shape-C position");

    // `Texture:SetTexCoord 0x79beb0` — every coordinate C; only the ARITY raises.
    s.run("T:SetTexCoord(nil, nil, nil, nil)")
        .expect("four nil coordinates are four zeroes");
    s.run("T:SetTexCoord(0, 1, 0, 1, 0, 1, 0, 1)")
        .expect("the 8-corner form still takes numbers");
    s.run("T:SetTexCoord(1, 2, 3)")
        .expect_err("but 3 args is neither 4 nor 8, and the arity DOES raise (0x79bf5d)");

    // `FontString:SetShadowColor 0x79dd40` and `StatusBar:SetStatusBarColor 0x78fc20`.
    s.run("FS:SetShadowColor(nil, nil, nil)")
        .expect("FontString:SetShadowColor is shape C on r/g/b");
    s.run("SB:SetStatusBarColor(nil, nil, nil)")
        .expect("StatusBar:SetStatusBarColor is shape C on r/g/b");
    s.run(
        r#"
        local r, g, b = SB:GetStatusBarColor()
        assert(r == 0 and g == 0 and b == 0, "and it stored the zeroes")
        "#,
    )
    .unwrap();

    // `ColorSelect:SetColorRGB 0x78eae0` — C on all three, and it fires OnColorSelect either way.
    s.run("CS:SetColorRGB(nil, nil, nil)")
        .expect("ColorSelect:SetColorRGB is shape C on r/g/b");

    // `Texture:SetGradient 0x79ae30` / `SetGradientAlpha 0x79b180` — already C before this pass;
    // pinned so they cannot regress into the family's old strictness.
    s.run("T:SetGradient(\"HORIZONTAL\", nil, nil, nil, nil, nil, nil)")
        .expect("SetGradient's six stops are all C");
    s.run("T:SetGradientAlpha(\"VERTICAL\", nil, nil, nil, nil, nil, nil, nil, nil)")
        .expect("SetGradientAlpha's eight are C/B");

    // `FontString:SetTextColor 0x79d9c0` — closed by 1973; pinned here so the class stays closed.
    s.run("FS:SetTextColor(nil, nil, nil)")
        .expect("the sibling 1973 already fixed");
}
