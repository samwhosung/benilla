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

/// The EditBox ctor enables char and key input (`0x779ced`, `0x779cf8`); `IsKeyboardEnabled`
/// (`0x776f90`) answers 1 for it and nil for a plain frame, and `EnableKeyboard(false)` takes it
/// back out of the walk (`0x76af50`), where the box goes deaf.
#[test]
fn an_editbox_is_born_keyboard_enabled() {
    let mut s = script();
    s.run(
        r#"
        box = CreateFrame("EditBox", "KbBorn")
        box:SetAutoFocus(false)
        box:SetFocus()
        plain = CreateFrame("Frame", "KbPlain")
    "#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>("return box:IsKeyboardEnabled() == 1")
        .unwrap());
    assert!(s
        .eval::<bool>("return plain:IsKeyboardEnabled() == nil")
        .unwrap());
    assert!(s.char_input("a"));
    s.run("box:EnableKeyboard(false)").unwrap();
    assert!(
        !s.char_input("b"),
        "out of the bucket, the box takes nothing"
    );
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "a");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A focused box is a frame in the walk at its own place (`0x765df0`, `0x765f10`): a keyboard
/// frame in a lower stratum never sees the key, one in a higher stratum takes it first.
#[test]
fn a_focused_box_takes_keys_at_its_own_stratum() {
    let mut s = script();
    s.run(
        r#"
        seen = {}
        box = CreateFrame("EditBox", "KbPlaced")
        box:SetAutoFocus(false)
        box:SetFrameStrata("DIALOG")
        box:SetFocus()
        function mk(name, strata)
            local f = CreateFrame("Frame", name)
            f:SetFrameStrata(strata)
            f:EnableKeyboard(true)
            f:SetScript("OnChar", function() table.insert(seen, name) end)
            f:SetScript("OnKeyDown", function() table.insert(seen, name .. ":" .. arg1) end)
            return f
        end
        low = mk("KbLower", "MEDIUM")
    "#,
    )
    .unwrap();
    assert!(s.char_input("x"));
    assert!(s.key_input("ENTER"));
    assert_eq!(s.eval::<i64>("return table.getn(seen)").unwrap(), 0);
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "x");

    s.run(r#"mk("KbUpper", "FULLSCREEN_DIALOG")"#).unwrap();
    assert!(s.char_input("y"));
    assert!(s.key_input("ENTER"));
    assert_eq!(
        s.eval::<String>("return table.concat(seen, ',')").unwrap(),
        "KbUpper,KbUpper:ENTER",
        "a higher stratum pre-empts the box"
    );
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "x");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// With no box focused, the first `autoFocus` box in walk order takes the focus and the key at
/// its own position (`0x77a917`-`0x77a92b`): a keyboard frame below it never sees the key, one
/// above it does, and at one stratum and level the older registration wins.
#[test]
fn an_autofocus_box_takes_its_key_at_its_own_walk_position() {
    let mut s = script();
    s.run(
        r#"
        seen = {}
        function mkframe(name, strata)
            local f = CreateFrame("Frame", name)
            f:SetFrameStrata(strata)
            f:EnableKeyboard(true)
            f:SetScript("OnKeyDown", function() table.insert(seen, name) end)
            return f
        end
        function mkbox(name)
            local b = CreateFrame("EditBox", name)
            b:SetAutoFocus(true)
            b:Hide()
            b:SetFrameStrata("DIALOG")
            return b
        end
        a = mkbox("KbAutoA")
        b = mkbox("KbAutoB")
        mkframe("KbBelowBox", "MEDIUM")
        a:Show() b:Show()
        a:ClearFocus() b:ClearFocus()
    "#,
    )
    .unwrap();
    assert_eq!(s.focused_editbox_name(), None);
    assert!(s.key_input("ENTER"));
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("KbAutoA"),
        "the older of two boxes at one position takes the focus"
    );
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        0,
        "the frame below saw nothing"
    );

    s.run("KbAutoA:ClearFocus() mkframe('KbAboveBox', 'TOOLTIP')")
        .unwrap();
    assert!(s.key_input("ENTER"));
    assert_eq!(
        s.focused_editbox_name(),
        None,
        "the frame above took it first"
    );
    assert_eq!(
        s.eval::<String>("return table.concat(seen, ',')").unwrap(),
        "KbAboveBox"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The key-up (`0x765fd0`) goes to the frame that consumed the key's down, as `OnKeyUp` with the
/// key's name (`0x76bba0`); a key nobody consumed has no target, and the target is not cleared by
/// a delivered up (`0x76605b` sits on the no-target path).
#[test]
fn a_key_up_goes_to_the_frame_that_took_the_down() {
    let mut s = script();
    s.run(
        r#"
        log = {}
        f = CreateFrame("Frame", "KbUpA")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyDown", function() table.insert(log, "down:" .. arg1) end)
        f:SetScript("OnKeyUp", function() table.insert(log, "up:" .. arg1) end)
        g = CreateFrame("Frame", "KbUpB")
        g:EnableKeyboard(true)
        g:SetFrameStrata("LOW")
        g:SetScript("OnKeyUp", function() table.insert(log, "g:" .. arg1) end)
    "#,
    )
    .unwrap();
    assert!(
        !s.key_up("ENTER").taken(),
        "nothing consumed the down: no target"
    );
    assert!(s.key_input("ENTER"));
    assert!(s.key_up("ENTER").taken());
    assert!(
        s.key_up("ENTER").taken(),
        "the target outlives a delivered up"
    );
    assert!(!s.key_up("TAB").taken());
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "down:ENTER,up:ENTER,up:ENTER"
    );

    // The up follows the down's consumer, not whoever would take the key now.
    s.run("f:Hide()").unwrap();
    assert!(
        s.key_up("ENTER").taken(),
        "a hidden consumer still gets its up"
    );
    assert_eq!(
        s.eval::<String>("return log[table.getn(log)]").unwrap(),
        "up:ENTER"
    );
    assert!(
        s.key_input("ENTER"),
        "the lower frame's OnKeyUp-only slot takes the next down"
    );
    assert!(s.key_up("ENTER").taken());
    assert_eq!(
        s.eval::<String>("return log[table.getn(log)]").unwrap(),
        "g:ENTER"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A box in alt-arrow mode declines the four arrows without ALT, and every box declines
/// PRINTSCREEN, before it looks at focus (`0x77b17f`, `0x77b18e`-`0x77b1c4`): the walk goes on to
/// the frames below it, and a declined key gives an `autoFocus` box no focus.
#[test]
fn a_box_declines_printscreen_and_unaltered_arrows_and_the_walk_goes_on() {
    use crate::script::KeyWalk;
    let mut s = script();
    s.run(
        r#"
        seen = {}
        box = CreateFrame("EditBox", "KbChatLike")
        box:SetAutoFocus(false)
        box:SetAltArrowKeyMode(1)
        box:SetFrameStrata("DIALOG")
        box:SetFocus()
        spin = CreateFrame("Frame", "KbSpinner")
        spin:SetFrameStrata("MEDIUM")
        spin:EnableKeyboard(true)
        spin:SetScript("OnKeyDown", function() table.insert(seen, arg1) end)
    "#,
    )
    .unwrap();
    for key in ["LEFT", "UP", "RIGHT", "DOWN", "PRINTSCREEN"] {
        assert_eq!(
            s.key_down(key),
            KeyWalk::Frame,
            "{key} goes on to the frame below"
        );
    }
    assert_eq!(
        s.eval::<String>("return table.concat(seen, ',')").unwrap(),
        "LEFT,UP,RIGHT,DOWN,PRINTSCREEN"
    );
    // Any other key stops at the focused box, as before.
    assert_eq!(s.key_down("HOME"), KeyWalk::Box);
    assert_eq!(s.key_down("A"), KeyWalk::Box);
    // ALT up is the whole condition: with it down the box takes the arrows.
    s.set_alt_down(true);
    assert_eq!(s.key_down("RIGHT"), KeyWalk::Box);
    assert_eq!(
        s.key_down("PRINTSCREEN"),
        KeyWalk::Frame,
        "PRINTSCREEN is declined whatever ALT does"
    );
    s.set_alt_down(false);

    // Nothing below: the world frame's binding is next, and the unfocused autoFocus box gets
    // no focus from a key it declines.
    s.run(
        r#"
        spin:Hide()
        box:ClearFocus()
        box:Hide()
        auto = CreateFrame("EditBox", "KbAutoChat")
        auto:SetAutoFocus(true)
        auto:SetAltArrowKeyMode(1)
        auto:ClearFocus()
    "#,
    )
    .unwrap();
    assert_eq!(s.focused_editbox_name(), None);
    assert_eq!(s.key_down("LEFT"), KeyWalk::Declined);
    assert_eq!(s.key_down("PRINTSCREEN"), KeyWalk::Declined);
    assert_eq!(
        s.focused_editbox_name(),
        None,
        "a declined key grants no focus"
    );
    assert_eq!(s.key_down("X"), KeyWalk::Box, "any other key does");
    assert_eq!(s.focused_editbox_name().as_deref(), Some("KbAutoChat"));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The base key-up (`0x76bba0`) and a box's (`0x77b7c0`) return before anything when the frame is
/// not linked, so a consumer that has since hidden gets no `OnKeyUp`; the record outlives it.
#[test]
fn a_hidden_consumer_gets_no_key_up() {
    let mut s = script();
    s.run(
        r#"
        ups = 0
        f = CreateFrame("Frame", "KbUpHidden")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyDown", function() end)
        f:SetScript("OnKeyUp", function() ups = ups + 1 end)
    "#,
    )
    .unwrap();
    assert!(s.key_input("K"));
    s.run("f:Hide()").unwrap();
    assert!(
        s.key_up("K").taken(),
        "the record stands, the up ends there"
    );
    assert_eq!(s.eval::<i64>("return ups").unwrap(), 0);
    s.run("f:Show()").unwrap();
    assert!(s.key_up("K").taken());
    assert_eq!(
        s.eval::<i64>("return ups").unwrap(),
        1,
        "shown again, it gets it"
    );
}

/// A press a binding took is the world frame's (`0x483b70` returns the binding's result), so the
/// up and repeats go to it, and an earlier consumer of the same key is not reached.
#[test]
fn a_press_a_binding_took_moves_the_up_to_the_world_frame() {
    let mut s = script();
    s.run(
        r#"
        log = {}
        CreateFrame("WorldFrame", "WorldFrame")
        f = CreateFrame("Frame", "KbUpStale")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyDown", function() table.insert(log, "down") end)
        f:SetScript("OnKeyUp", function() table.insert(log, "up") end)
    "#,
    )
    .unwrap();
    assert!(s.key_input("J"));
    s.run("f:Hide()").unwrap();
    // The frame is gone from the walk, so the press is the binding's.
    assert!(!s.key_input("J"));
    s.note_world_key("J");
    s.run("f:Show()").unwrap();
    assert!(s.key_up("J").taken());
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "down",
        "the stale frame's OnKeyUp is not run"
    );
    assert_eq!(
        s.key_repeat("J"),
        crate::script::KeyWalk::Frame,
        "a stub: nothing runs"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The auto-repeat (`0x766070`) goes to the frame that took the press, whose `+0x58` is a stub on
/// a plain frame (`0x46ff90`) and the key-down again on a box (`0x77b770`).
#[test]
fn a_repeat_goes_to_the_consumer_not_the_walk() {
    use crate::script::KeyWalk;
    let mut s = script();
    s.run(
        r#"
        downs = 0
        f = CreateFrame("Frame", "KbRepeat")
        f:SetFrameStrata("MEDIUM")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyDown", function() downs = downs + 1 end)
        box = CreateFrame("EditBox", "KbRepeatBox")
        box:SetAutoFocus(false)
        box:SetFrameStrata("LOW")
        box:SetFocus()
        enters = 0
        box:SetScript("OnEnterPressed", function() enters = enters + 1 end)
    "#,
    )
    .unwrap();
    assert_eq!(
        s.key_repeat("RIGHT"),
        KeyWalk::Declined,
        "no press on record, nothing"
    );
    assert_eq!(s.key_down("RIGHT"), KeyWalk::Frame);
    for _ in 0..3 {
        assert_eq!(s.key_repeat("RIGHT"), KeyWalk::Frame);
    }
    assert_eq!(
        s.eval::<i64>("return downs").unwrap(),
        1,
        "holding the key fires OnKeyDown once, not once per repeat"
    );

    // A box that took the press gets its key-down again.
    s.run("f:Hide()").unwrap();
    assert_eq!(s.key_down("ENTER"), KeyWalk::Box);
    assert_eq!(s.key_repeat("ENTER"), KeyWalk::Box);
    assert_eq!(s.eval::<i64>("return enters").unwrap(), 2);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Equal-level frames walk in the order they joined the bucket (`0x764aa0` inserts after the
/// equals): a live `EnableKeyboard` inserts then, and a level change inserts again (`0x76a4f0`
/// takes the frame out and back in).
#[test]
fn equal_levels_walk_in_bucket_join_order() {
    let mut s = script();
    s.run(
        r#"
        winner = nil
        function mk(name)
            local f = CreateFrame("Frame", name)
            f:SetFrameStrata("HIGH")
            f:SetScript("OnKeyDown", function() winner = name end)
            return f
        end
        a = mk("KbJoinA")
        b = mk("KbJoinB")
        -- created A then B, but B joined first
        b:EnableKeyboard(true)
        a:EnableKeyboard(true)
    "#,
    )
    .unwrap();
    assert!(s.key_input("Z"));
    assert_eq!(s.eval::<String>("return winner").unwrap(), "KbJoinB");

    // A level change re-inserts the frame after its equals.
    s.run(
        r#"
        local l = b:GetFrameLevel()
        b:SetFrameLevel(l + 1)
        b:SetFrameLevel(l)
    "#,
    )
    .unwrap();
    assert!(s.key_input("Z"));
    assert_eq!(s.eval::<String>("return winner").unwrap(), "KbJoinA");

    // Already enabled: enabling again joins nothing.
    s.run("b:EnableKeyboard(true) a:EnableKeyboard(true)")
        .unwrap();
    assert!(s.key_input("Z"));
    assert_eq!(s.eval::<String>("return winner").unwrap(), "KbJoinA");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The XML walker enables one kind per handler (`0x769ef0`): `OnChar` the char bucket
/// (`0x769f6b`), `OnKeyDown` and `OnKeyUp` the key bucket (`0x769fa4`). A frame with only
/// `OnKeyDown` is in no char walk, and the reverse.
#[test]
fn an_xml_handler_enables_only_its_own_kind() {
    let mut s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Frame name="XmlKeysOnly"><Scripts><OnKeyDown>keys = (keys or 0) + 1</OnKeyDown></Scripts></Frame>
             <Frame name="XmlCharsOnly"><Scripts><OnChar>chars = (chars or 0) + 1</OnChar></Scripts></Frame>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(s.key_input("Q"));
    assert!(s.char_input("q"));
    assert_eq!(s.eval::<i64>("return keys").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return chars").unwrap(), 1);
    // A handler Lua adds later enables nothing (`0x7748d0`), so it shows which buckets each
    // frame is in: the keys-only frame is in no char walk, the chars-only one in no key walk.
    s.run(
        r#"
        XmlKeysOnly:SetScript("OnKeyDown", nil)
        XmlKeysOnly:SetScript("OnChar", function() chars = chars + 10 end)
        XmlCharsOnly:SetScript("OnChar", nil)
        XmlCharsOnly:SetScript("OnKeyDown", function() keys = keys + 10 end)
    "#,
    )
    .unwrap();
    assert!(
        !s.char_input("q"),
        "the keys-only frame is not in the char walk"
    );
    assert!(
        !s.key_input("Q"),
        "the chars-only frame is not in the key walk"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A consumer on record makes the repeat consumed whatever its `+0x58` does (`0x7660bb`), and a
/// box's (`0x77b770`) grants an `autoFocus` box the free focus (`0x77b799`) before its key-down
/// can decline.
#[test]
fn a_repeat_grants_autofocus_before_the_box_may_decline() {
    use crate::script::KeyWalk;
    let mut s = script();
    s.run(
        r#"
        box = CreateFrame("EditBox", "KbRepeatAuto")
        box:SetAutoFocus(true)
        box:Hide() box:Show()
        box:ClearFocus()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.key_down("LEFT"),
        KeyWalk::Box,
        "the box takes LEFT, a plain box"
    );
    s.run("box:ClearFocus() box:SetAltArrowKeyMode(1)").unwrap();
    assert_eq!(s.focused_editbox_name(), None);
    assert_eq!(
        s.key_repeat("LEFT"),
        KeyWalk::Frame,
        "consumed, though the box declines the arrow now"
    );
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("KbRepeatAuto"),
        "and the repeat gave it the focus"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// PRINTSCREEN's up with no consumer on record runs the key-down walk (`0x76603c`-`0x76604d`) and
/// leaves no record (`0x76605b`); any other key's up with none is nothing.
#[test]
fn a_printscreen_up_with_no_consumer_walks_as_a_key_down() {
    use crate::script::{KeyUp, KeyWalk};
    let mut s = script();
    assert_eq!(s.key_up("PRINTSCREEN"), KeyUp::Walked(KeyWalk::Declined));
    assert_eq!(s.key_up("G"), KeyUp::Unclaimed);
    s.run(
        r#"
        got = {}
        f = CreateFrame("Frame", "KbPrint")
        f:EnableKeyboard(true)
        f:SetScript("OnKeyDown", function() table.insert(got, arg1) end)
        f:SetScript("OnKeyUp", function() table.insert(got, "up") end)
    "#,
    )
    .unwrap();
    assert_eq!(s.key_up("PRINTSCREEN"), KeyUp::Walked(KeyWalk::Frame));
    assert_eq!(
        s.key_up("PRINTSCREEN"),
        KeyUp::Walked(KeyWalk::Frame),
        "the record the walk wrote is cleared, so the next up walks again"
    );
    assert_eq!(
        s.eval::<String>("return table.concat(got, ',')").unwrap(),
        "PRINTSCREEN,PRINTSCREEN"
    );
    // With a recorded down, the up is an ordinary delivery.
    assert!(s.key_input("PRINTSCREEN"));
    assert_eq!(s.key_up("PRINTSCREEN"), KeyUp::Delivered);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
