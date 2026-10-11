//! The engine's own per-kind updates, each run in its frame's slot of the update walk (`0x765650`,
//! `vtable+0x38` at `0x765710`) right after that frame's `OnUpdate`: GameTooltip's fade
//! (`0x530b20`) and a Model pane's clock and completion (`0x76d7f0`). `OnUpdateModel` fires later,
//! from the draw pass (`0x7657d0`), in draw order.

use crate::script::UiScript;
use crate::widget::{ModelFileFacts, SequenceFacts};

use super::common::script;

/// `LOG` joined by spaces, then emptied.
fn take_log(s: &UiScript) -> String {
    s.eval::<String>("local l = table.concat(LOG, ' ') LOG = {} return l")
        .unwrap()
}

/// `UI-Cooldown-Indicator.m2`'s two clamped 1000 ms sequences.
const COOLDOWN_FILE: &str = r"Interface\Cooldown\UI-Cooldown-Indicator.mdx";
fn cooldown_facts() -> ModelFileFacts {
    ModelFileFacts {
        sequences: [(0, 1000), (1, 1000)]
            .into_iter()
            .map(|(anim_id, duration_ms)| SequenceFacts {
                anim_id,
                duration_ms,
                looping: false,
            })
            .collect(),
        bbox: ([0.0; 3], [0.0; 3]),
        cameras: 0,
    }
}

fn lua_path(path: &str) -> String {
    path.replace('\\', "\\\\")
}

/// A fading tooltip, owned and with a line, whose `OnHide` logs `hide`.
const FADING_TIP: &str = r#"
    LOG = {}
    local root = CreateFrame("Frame", "FadeRoot")
    local owner = CreateFrame("Button", "FadeOwner", root)
    owner:SetPoint("CENTER", 0, 0); owner:SetWidth(10); owner:SetHeight(10)
    local tt = CreateFrame("GameTooltip", "FadeTip", root)
    tt:SetFrameStrata("HIGH")
    tt:SetOwner(owner, "ANCHOR_RIGHT")
    tt:SetText("Fading")
    tt:SetScript("OnHide", function() table.insert(LOG, "hide") end)
"#;

/// The fade is the tooltip's own update (`0x530b9c`, after its `OnUpdate` at `0x530b2d`): a frame
/// walked after it reads this frame's alpha, and on the frame the fade ends the hide (`0x530be9`)
/// and its `OnHide` come before that frame's `OnUpdate`.
#[test]
fn a_frame_walked_after_the_tooltip_sees_this_frames_fade() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(FADING_TIP).unwrap();
    s.run(
        r#"
        local w = CreateFrame("Frame", "FadeWatch")
        w:SetFrameStrata("TOOLTIP")
        w:SetScript("OnUpdate", function()
            if FadeTip:IsShown() then
                table.insert(LOG, string.format("shown@%.1f", FadeTip:GetAlpha()))
            else
                table.insert(LOG, "hidden")
            end
        end)
        FadeTip:FadeOut()
    "#,
    )
    .unwrap();
    take_log(&s);
    s.tick(1.5);
    assert_eq!(take_log(&s), "shown@0.5", "this frame's ramp, not the last");
    s.tick(0.5);
    assert_eq!(
        take_log(&s),
        "hide hidden",
        "the hide lands in the tooltip's slot"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A pane's update (`0x76d7f0`) runs its `OnUpdate`, then advances its clock, whose completion
/// test drains `OnAnimFinished` in the same call (`0x7074d8`, `0x7075af`): in the pane's place,
/// before frames walked after it and before the draw pass fires `OnUpdateModel`.
#[test]
fn on_anim_finished_fires_in_the_panes_walk_slot() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"
        LOG = {{}}
        local p = CreateFrame("Model", "SlotPane")
        p:SetFrameStrata("LOW")
        p:SetModel("{}")
        p:SetSequence(1)
        p:SetScript("OnUpdate", function() table.insert(LOG, "pane") end)
        p:SetScript("OnAnimFinished", function() table.insert(LOG, "finished") end)
        p:SetScript("OnUpdateModel", function() table.insert(LOG, "model") end)
        local h = CreateFrame("Frame", "SlotHigh")
        h:SetFrameStrata("HIGH")
        h:SetScript("OnUpdate", function() table.insert(LOG, "high") end)
    "#,
        lua_path(COOLDOWN_FILE)
    ))
    .unwrap();
    take_log(&s);
    s.tick(0.6);
    assert_eq!(take_log(&s), "pane high model");
    s.tick(0.6); // 1200 ms ≥ the 1000 ms sequence
    assert_eq!(take_log(&s), "pane finished high model");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// One virtual call runs both halves (`0x76d7fa`, then `0x76d843`): a pane that hides itself in
/// its `OnUpdate` still advances this frame, and a completion its advance reaches still fires.
/// A tooltip shows nothing here: its hide disarms the fade it would step (`0x530002`).
#[test]
fn a_pane_hidden_by_its_own_on_update_still_advances() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"
        LOG = {{}}
        local p = CreateFrame("Model", "SelfHide")
        p:SetModel("{}")
        p:SetSequence(1)
        p:SetScript("OnAnimFinished", function() table.insert(LOG, "finished") end)
    "#,
        lua_path(COOLDOWN_FILE)
    ))
    .unwrap();
    s.tick(0.6);
    s.run(r#"SelfHide:SetScript("OnUpdate", function() this:Hide() end)"#)
        .unwrap();
    take_log(&s);
    s.tick(0.6);
    assert!(!s.frame_visible("SelfHide"));
    assert_eq!(super::model_clock::pane(&s, "SelfHide").clock_ms, 1200);
    assert_eq!(take_log(&s), "finished");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A zero advance returns before the completion test and the drain (`0x7074b7`): a sequence
/// already past its end fires nothing until a frame advances the clock.
#[test]
fn a_zero_advance_tests_no_completion() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"
        LOG = {{}}
        local p = CreateFrame("Model", "ZeroDt")
        p:SetModel("{}")
        p:SetSequenceTime(1, 1000)
        p:SetScript("OnAnimFinished", function() table.insert(LOG, "finished") end)
    "#,
        lua_path(COOLDOWN_FILE)
    ))
    .unwrap();
    take_log(&s);
    s.tick(0.0009); // trunc(0.9 ms) = 0
    assert_eq!(super::model_clock::pane(&s, "ZeroDt").clock_ms, 0);
    assert_eq!(take_log(&s), "", "no advance, no test");
    s.tick(0.016);
    assert_eq!(take_log(&s), "finished");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Completion is judged against the arm the previous paint left: an `OnUpdateModel` that scrubs
/// the sequence back each paint does not hide the completion the walk already reported.
#[test]
fn completion_is_read_before_the_paint_re_arms() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"
        LOG = {{}}
        local p = CreateFrame("Model", "Scrub")
        p:SetModel("{}")
        p:SetSequence(1)
        p:SetScript("OnAnimFinished", function() table.insert(LOG, "finished") end)
    "#,
        lua_path(COOLDOWN_FILE)
    ))
    .unwrap();
    s.tick(0.6);
    s.run(r#"Scrub:SetScript("OnUpdateModel", function() this:SetSequenceTime(1, 0) end)"#)
        .unwrap();
    take_log(&s);
    s.tick(0.6); // the arm from tick one ends at 1000 ms; this tick's clock reads 1200
    assert_eq!(take_log(&s), "finished");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnUpdateModel` fires from the paint (`0x76d1bc`), in the draw pass's order: stratum from 0
/// (`0x7657ff`), level from 0 (`0x7658c0`), a level's frames first linked first (`0x765920`).
#[test]
fn on_update_model_fires_in_draw_order() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_model_facts(COOLDOWN_FILE, cooldown_facts());
    s.run(&format!(
        r#"
        LOG = {{}}
        function Pane(name, tag, strata, level)
            local p = CreateFrame("Model", name)
            p:SetFrameStrata(strata)
            if level then p:SetFrameLevel(level) end
            p:SetModel("{}")
            p:SetScript("OnUpdateModel", function() table.insert(LOG, tag) end)
            return p
        end
        Pane("PaneDialog", "dialog", "DIALOG")
        Pane("PaneLow", "low", "LOW")
        Pane("PaneLevel9", "level9", "MEDIUM", 9)
        Pane("PaneLevel2", "level2", "MEDIUM", 2)
    "#,
        lua_path(COOLDOWN_FILE)
    ))
    .unwrap();
    take_log(&s);
    s.tick(0.016);
    assert_eq!(take_log(&s), "low level2 level9 dialog");

    // Within a level the latest link draws last: a pane shown again goes after its peers.
    s.run(
        r#"
        Pane("LinkA", "a", "BACKGROUND", 3)
        Pane("LinkB", "b", "BACKGROUND", 3)
        LinkA:Hide() LinkA:Show()
    "#,
    )
    .unwrap();
    take_log(&s);
    s.tick(0.016);
    assert_eq!(take_log(&s), "b a low level2 level9 dialog");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
