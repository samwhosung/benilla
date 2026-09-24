//! Frame keyboard delivery: the walk (`0x765f10`) and its existence gate (`0x76b7d0`).

use super::common::script;

/// Bucket membership (`0x76af00`) is the keyboard-enabled flag, which `SetScript` never sets.
#[test]
fn a_key_script_alone_does_not_put_a_frame_in_the_walk() {
    let mut s = script();
    s.run(
        r#"
        got = nil
        f = CreateFrame("Frame", "KbUnenabled")
        f:SetScript("OnChar", function() got = arg1 end)
    "#,
    )
    .unwrap();
    assert!(
        !s.char_input("7"),
        "an unenabled frame is not in the bucket"
    );
    assert_eq!(s.eval::<Option<String>>("return got").unwrap(), None);

    s.run("f:EnableKeyboard(true)").unwrap();
    assert!(
        s.char_input("7"),
        "enabled: now in the walk, and it consumes"
    );
    assert_eq!(
        s.eval::<Option<String>>("return got").unwrap().as_deref(),
        Some("7")
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The existence gate (`0x76b7d0`) takes either key slot, so a frame with only an `OnKeyUp`
/// consumes every key-down and runs nothing.
#[test]
fn only_an_onkeyup_still_swallows_the_key_down() {
    let mut s = script();
    s.run(
        r#"
        downs = 0
        f = CreateFrame("Frame", "KbUpOnly")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyUp", function() downs = downs + 1 end)
    "#,
    )
    .unwrap();
    assert!(s.key_input("ESCAPE"), "the OnKeyUp slot alone consumes");
    assert_eq!(
        s.eval::<i64>("return downs").unwrap(),
        0,
        "…and fires nothing"
    );

    s.run("f:SetScript(\"OnKeyUp\", nil)").unwrap();
    assert!(!s.key_input("ESCAPE"), "no key slot at all: declines");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The walk (`0x765f10`, `0x764ae2`) runs strata descending, then level, then registration order.
#[test]
fn the_walk_is_strata_then_level_then_registration() {
    let mut s = script();
    s.run(
        r#"
        winner = nil
        function mk(name, strata, level)
            local f = CreateFrame("Frame", name)
            f:SetFrameStrata(strata)
            f:SetFrameLevel(level)
            f:EnableKeyboard(true)
            f:SetScript("OnKeyDown", function() winner = name end)
            return f
        end
        -- registered first, but the LOWEST stratum: must lose despite its huge level
        mk("KbLow", "LOW", 99)
        mk("KbHigh", "HIGH", 1)
    "#,
    )
    .unwrap();
    assert!(s.key_input("ESCAPE"));
    assert_eq!(
        s.eval::<String>("return winner").unwrap(),
        "KbHigh",
        "strata beats level"
    );

    s.run(
        r#"
        winner = nil
        mk("KbHighTop", "HIGH", 5)
        mk("KbHighTie", "HIGH", 5)
    "#,
    )
    .unwrap();
    assert!(s.key_input("ESCAPE"));
    assert_eq!(
        s.eval::<String>("return winner").unwrap(),
        "KbHighTop",
        "level 5 beats level 1; the earlier of the two 5s wins the tie"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The walk stops at the first consumer (`0x765f10`).
#[test]
fn the_first_consumer_ends_the_walk() {
    let mut s = script();
    s.run(
        r#"
        seen = {}
        function mk(name, strata)
            local f = CreateFrame("Frame", name)
            f:SetFrameStrata(strata)
            f:EnableKeyboard(true)
            f:SetScript("OnChar", function() table.insert(seen, name) end)
        end
        mk("KbTip", "TOOLTIP")
        mk("KbDlg", "DIALOG")
    "#,
    )
    .unwrap();
    assert!(s.char_input("x"));
    assert_eq!(s.eval::<i64>("return table.getn(seen)").unwrap(), 1);
    assert_eq!(s.eval::<String>("return seen[1]").unwrap(), "KbTip");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The walk shares the draw order's link gate, so a hidden frame is in no bucket.
#[test]
fn a_hidden_frame_is_not_in_the_walk() {
    let mut s = script();
    s.run(
        r#"
        winner = nil
        function mk(name, strata)
            local f = CreateFrame("Frame", name)
            f:SetFrameStrata(strata)
            f:EnableKeyboard(true)
            f:SetScript("OnChar", function() winner = name end)
            return f
        end
        top = mk("KbHidden", "TOOLTIP")
        mk("KbBelow", "DIALOG")
        top:Hide()
    "#,
    )
    .unwrap();
    assert!(s.char_input("q"));
    assert_eq!(s.eval::<String>("return winner").unwrap(), "KbBelow");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Consumption is the gate's decision, never the handler's (`0x7026f0`).
#[test]
fn a_raising_handler_still_consumes_and_is_recorded() {
    let mut s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "KbBoom")
        f:EnableKeyboard(true)
        f:SetScript("OnChar", function() error("boom") end)
    "#,
    )
    .unwrap();
    assert!(s.char_input("z"), "the gate consumed regardless");
    assert!(
        s.errors().iter().any(|e| e.contains("boom")),
        "the raise is recorded: {:?}",
        s.errors()
    );
}

/// An editbox in the walk is asked about focus, never a script slot (`0x77a900`): vtable
/// `0x81c910` overrides `+0x5c`/`+0x60` without calling the base gate `0x76b760`, so an unfocused
/// box declines at `0x77a956` and the walk continues.
#[test]
fn an_unfocused_editbox_declines_rather_than_eating_its_neighbours_keys() {
    let mut s = script();
    s.run(
        r#"
        decoyChars = 0
        decoy = CreateFrame("EditBox", "KbDecoyBox")
        decoy:SetAutoFocus(false)
        decoy:EnableKeyboard(true)
        decoy:SetScript("OnChar", function() decoyChars = decoyChars + 1 end)
        target = CreateFrame("EditBox", "KbTargetBox")
        target:SetAutoFocus(false)
        target:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.char_input("k"), "the focused box consumes");
    assert_eq!(
        s.eval::<String>("return KbTargetBox:GetText()").unwrap(),
        "k",
        "the keystroke reaches the box that holds the focus"
    );
    assert_eq!(
        s.eval::<i64>("return decoyChars").unwrap(),
        0,
        "the unfocused box's own OnChar is unreachable from the walk — it declines"
    );

    // Focused, the decoy's `OnChar` fires from the insert (`0x77c200`), not from the walk's gate.
    s.run("KbDecoyBox:SetFocus()").unwrap();
    assert!(s.char_input("j"), "…and now it is the one that consumes");
    assert_eq!(s.eval::<i64>("return decoyChars").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return KbDecoyBox:GetText()").unwrap(),
        "j"
    );
    assert_eq!(
        s.eval::<String>("return KbTargetBox:GetText()").unwrap(),
        "k",
        "and the box that lost focus keeps what it had"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The same on the key-down channel (`+0x60` is `0x77b160`, focus guard at `0x77b1c7`): an
/// editbox never reaches the base gate, so an `OnKeyUp` alone does not make it consume.
#[test]
fn an_unfocused_editbox_does_not_swallow_a_focused_boxs_editing_key() {
    let mut s = script();
    s.run(
        r#"
        decoy = CreateFrame("EditBox", "KbDecoyBox2")
        decoy:SetAutoFocus(false)
        decoy:EnableKeyboard(true)
        decoy:SetScript("OnKeyUp", function() end)
        target = CreateFrame("EditBox", "KbTargetBox2")
        target:SetAutoFocus(false)
        target:SetText("ab")
        target:SetFocus()
    "#,
    )
    .unwrap();
    s.editbox_action(crate::script::EditAction::Delete {
        unit: crate::script::EditUnit::Char,
        back: true,
    });
    assert_eq!(
        s.eval::<String>("return KbTargetBox2:GetText()").unwrap(),
        "a",
        "the backspace reached the focused box"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
