//! The update walk (`0x765650`): strata from the bottom up (`0x7656c8`–`0x765793`), each
//! stratum's levels from 0 up (`0x765774`), and each level's list of linked frames oldest link
//! first, the list being head-inserted (`0x7648c9`) and walked from its tail (`0x7656e5`–`0x765707`).
//! The walk saves the next frame before each call (`0x765707`), a hide repairs that cursor
//! (`0x76498f`), and a frame linked into the level being walked is staged until the level ends
//! (`0x76489b`, `0x76573e`).

use crate::script::UiScript;

use super::common::script;

/// `LOG` joined by spaces, then emptied.
fn take_log(s: &UiScript) -> String {
    s.eval::<String>("local l = table.concat(LOG, ' ') LOG = {} return l")
        .unwrap()
}

/// Lua that makes a frame `name`, logging `tag` from its `OnUpdate`.
const LOGGED: &str = r#"
    LOG = {}
    function Logged(name, tag)
        local f = CreateFrame("Frame", name)
        f:SetScript("OnUpdate", function() table.insert(LOG, tag) end)
        return f
    end
"#;

#[test]
fn the_walk_runs_strata_then_level_not_creation_order() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("WalkTip", "tooltip"):SetFrameStrata("TOOLTIP")
        Logged("WalkBack", "background"):SetFrameStrata("BACKGROUND")
        Logged("WalkHigh", "level5"):SetFrameLevel(5)
        Logged("WalkLow", "level1"):SetFrameLevel(1)
    "#,
    )
    .unwrap();
    take_log(&s);
    s.tick(0.016);
    assert_eq!(take_log(&s), "background level1 level5 tooltip");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Within a level the oldest link goes first, and a show links at the end: a frame hidden and
/// shown again walks after one created later.
#[test]
fn a_reshown_frame_walks_after_its_levels_older_links() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("LinkA", "a")
        Logged("LinkB", "b")
        LinkA:Hide()
        LinkA:Show()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "b a");
}

#[test]
fn a_frame_hidden_by_an_earlier_on_update_is_not_updated() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("HideA", "a"):SetScript("OnUpdate", function()
            table.insert(LOG, "a")
            HideB:Hide()
        end)
        Logged("HideB", "b")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "a");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A frame hidden and shown again before its turn relinks into the level being walked, which
/// stages it: it waits for the next tick. (A blink every tick would stage it every tick.)
#[test]
fn a_frame_reshown_before_its_turn_in_the_walked_level_waits_a_tick() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("BlinkA", "a"):SetScript("OnUpdate", function()
            table.insert(LOG, "a")
            if not BLINKED then
                BLINKED = 1
                BlinkB:Hide()
                BlinkB:Show()
            end
        end)
        Logged("BlinkB", "b")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "a");
    s.tick(0.016);
    assert_eq!(take_log(&s), "a b");
}

/// A frame shown into a level the walk has not reached yet is updated that tick; one shown into
/// the level being walked or one already walked waits for the next.
#[test]
fn a_frame_shown_mid_walk_is_updated_only_if_its_level_is_still_ahead() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("ShowA", "a"):SetScript("OnUpdate", function()
            table.insert(LOG, "a")
            ShowTip:Show()
            ShowSame:Show()
            ShowBack:Show()
        end)
        Logged("ShowTip", "tip"):SetFrameStrata("TOOLTIP")
        Logged("ShowSame", "same")
        Logged("ShowBack", "back"):SetFrameStrata("BACKGROUND")
        ShowTip:Hide()
        ShowSame:Hide()
        ShowBack:Hide()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "a tip");
    s.tick(0.016);
    assert_eq!(take_log(&s), "back a same tip");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A level change relinks a shown frame at the head of its new level (`0x76a54c`), so a frame that
/// moves itself to a level still ahead is walked again there.
#[test]
fn a_frame_that_moves_itself_to_a_level_ahead_is_walked_again() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("Mover", "m"):SetScript("OnUpdate", function()
            table.insert(LOG, "m" .. this:GetFrameLevel())
            if this:GetFrameLevel() == 0 then this:SetFrameLevel(3) end
        end)
        Logged("Mid", "mid"):SetFrameLevel(2)
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "m0 mid m3");
    s.tick(0.016);
    assert_eq!(take_log(&s), "mid m3");
}

/// The update reads the frame's handler when its turn comes (`0x76b2c3`), so an `OnUpdate` set by
/// an earlier frame's runs that tick.
#[test]
fn an_on_update_set_before_the_frames_turn_runs_that_tick() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        Logged("Setter", "setter"):SetScript("OnUpdate", function()
            table.insert(LOG, "setter")
            if not SET then
                SET = true
                Earlier:SetScript("OnUpdate", function() table.insert(LOG, "earlier") end)
                Later:SetScript("OnUpdate", function() table.insert(LOG, "later") end)
            end
        end)
        CreateFrame("Frame", "Earlier"):SetFrameStrata("BACKGROUND")
        CreateFrame("Frame", "Later"):SetFrameStrata("HIGH")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "setter later");
    s.tick(0.016);
    assert_eq!(take_log(&s), "earlier setter later");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// "Ahead" is by stratum first: a frame shown into a later stratum is updated that tick even at a
/// lower level than the one being walked, as the reference walks every stratum after this one.
#[test]
fn a_frame_shown_into_a_later_stratum_at_a_lower_level_runs_that_tick() {
    let mut s = script();
    s.run(LOGGED).unwrap();
    s.run(
        r#"
        local late = Logged("LateHigh", "high")
        late:SetFrameStrata("HIGH")
        late:SetFrameLevel(1)
        late:Hide()
        local shower = Logged("ShowerMed", "medium")
        shower:SetFrameLevel(5)
        shower:SetScript("OnUpdate", function()
            table.insert(LOG, "medium")
            LateHigh:Show()
        end)
    "#,
    )
    .unwrap();
    take_log(&s);
    s.tick(0.016);
    assert_eq!(take_log(&s), "medium high");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
