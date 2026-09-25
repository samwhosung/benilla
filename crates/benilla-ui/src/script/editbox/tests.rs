//! EditBox runtime tests, driven through the public keyboard API and the Lua methods.

use crate::script::{EditAction, EditUnit, QuadContent, UiScript};

fn script() -> UiScript {
    UiScript::new().expect("construct UiScript")
}

fn text_quad(s: &UiScript) -> Option<String> {
    s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Text { text, .. } => text,
        _ => None,
    })
}

// ── focus acquisition + routing ─────────────────────────────────────────────────────────────

/// A box created visible has had no show transition, yet with autoFocus it takes the keyboard on
/// the first key or char and processes that event.
#[test]
fn an_autofocus_box_self_acquires_on_the_first_event() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetAutoFocus(true)"#)
        .unwrap();
    assert!(!s.has_keyboard_focus());
    assert_eq!(s.focused_editbox_name(), None);

    assert!(
        s.char_input("a"),
        "an autoFocus box consumes the acquiring event"
    );
    assert!(s.has_keyboard_focus());
    assert_eq!(s.focused_editbox_name().as_deref(), Some("E"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "a");
}

/// The OnShow override (`0x77a750`, vtable `0x81c910` slot +0x30) focuses an autoFocus box when
/// nothing holds focus; there is no topmost or best choice.
#[test]
fn an_autofocus_box_takes_the_keyboard_when_it_is_shown() {
    let s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:Hide()
        E:ClearFocus()
    "#,
    )
    .unwrap();
    assert!(!s.has_keyboard_focus(), "hidden and unfocused to start");

    s.run("E:Show()").unwrap();
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("E"),
        "showing an autoFocus box focuses it",
    );

    // Hiding the focused box releases the keyboard (the mirror override, slot +0x34).
    s.run("E:Hide()").unwrap();
    assert!(
        !s.has_keyboard_focus(),
        "hiding the focused box releases the keyboard",
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Focus on show takes both halves of `[0xcf4dc8] == 0 && (flags & 1)`: no focus held, and
/// autoFocus.
#[test]
fn the_show_focus_is_refused_without_autofocus_or_with_the_keyboard_taken() {
    let s = script();
    s.run(
        r#"
        OPTED_OUT = CreateFrame("EditBox", "OptedOut")
        OPTED_OUT:SetAutoFocus(false)
        OPTED_OUT:Hide()
        OPTED_OUT:ClearFocus()
    "#,
    )
    .unwrap();
    s.run("OPTED_OUT:Show()").unwrap();
    assert!(
        !s.has_keyboard_focus(),
        "autoFocus=false is an opt-out that holds on show",
    );

    s.run(
        r#"
        HOLDER = CreateFrame("EditBox", "Holder")
        HOLDER:SetFocus()
        LATE = CreateFrame("EditBox", "Late")
        LATE:Hide()
    "#,
    )
    .unwrap();
    s.run("LATE:Show()").unwrap();
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("Holder"),
        "a shown autoFocus box does not steal a focus that is already held",
    );

    // Hiding a box without the keyboard leaves focus alone (`0x77e410`'s per-box guard).
    s.run("LATE:Hide()").unwrap();
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("Holder"),
        "hiding an unfocused box does not clear somebody else's focus",
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn non_autofocus_unfocused_box_ignores_input() {
    let mut s = script();
    // The ctor leaves autoFocus on (`0x779a29`/`0x779a2e`), so a bare box would self-acquire.
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetAutoFocus(false)"#)
        .unwrap();
    assert!(!s.char_input("a"), "no focus, no autoFocus → not consumed");
    assert!(!s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false
    }));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "");
    assert!(!s.has_keyboard_focus());
}

#[test]
fn focused_box_consumes_everything() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetFocus()"#)
        .unwrap();
    assert!(s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false
    }));
    assert!(s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false
    }));
    assert!(s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: false
    }));
    assert!(s.char_input("x"));
}

#[test]
fn set_focus_on_hidden_box_is_a_noop() {
    let s = script();
    s.run(
        r#"
        gained = 0
        E = CreateFrame("EditBox", "E")
        E:SetScript("OnEditFocusGained", function() gained = gained + 1 end)
        E:Hide()
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert_eq!(s.focused_editbox_name(), None);
    assert!(!s.has_keyboard_focus());
    assert_eq!(
        s.eval::<i64>("return gained").unwrap(),
        0,
        "no focus gained"
    );
}

#[test]
fn click_focuses_regardless_of_autofocus_and_transition_order_is_lost_then_gained() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        log = {}
        local function wire(name, y)
            local f = CreateFrame("EditBox", name)
            f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, y)
            f:SetWidth(100); f:SetHeight(20)
            f:SetScript("OnEditFocusGained", function() table.insert(log, "gained"..name) end)
            f:SetScript("OnEditFocusLost", function() table.insert(log, "lost"..name) end)
        end
        wire("A", 0)      -- rect bottom 0..20
        wire("B", 100)    -- rect bottom 100..120
    "#,
    )
    .unwrap();
    s.resolve();

    s.mouse_button(50.0, 10.0, "LeftButton", true);
    s.mouse_button(50.0, 10.0, "LeftButton", false);
    assert_eq!(s.focused_editbox_name().as_deref(), Some("A"));

    s.mouse_button(50.0, 110.0, "LeftButton", true);
    s.mouse_button(50.0, 110.0, "LeftButton", false);
    assert_eq!(s.focused_editbox_name().as_deref(), Some("B"));

    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["gainedA", "lostA", "gainedB"]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── text buffer + editing + OnTextChanged/OnTextSet ─────────────────────────────────────────

/// An edit only raises the box's `textChanged` bit; `OnTextChanged` fires once, at the drain its
/// OnUpdate runs (`0x77d3e0`).
#[test]
fn typing_coalesces_into_one_deferred_ontextchanged() {
    let mut s = script();
    s.run(
        r#"
        changed = 0
        E = CreateFrame("EditBox", "E")
        seen = ""
        E:SetScript("OnTextChanged", function() changed = changed + 1 seen = E:GetText() end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.char_input("a"));
    assert!(s.char_input("b"));
    assert!(s.char_input("c"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abc");
    assert_eq!(
        s.eval::<i64>("return changed").unwrap(),
        0,
        "the text is already there, but nothing has drained yet"
    );

    s.tick(0.0);
    assert_eq!(
        s.eval::<i64>("return changed").unwrap(),
        1,
        "three edits, one fire, carrying the final text"
    );
    assert_eq!(s.eval::<String>("return seen").unwrap(), "abc");

    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return changed").unwrap(), 1);
}

/// `OnTextSet` fires inside `SetText` (`0x77be6b`), `OnTextChanged` at the drain; an unchanged
/// text fires neither.
#[test]
fn set_text_fires_ontextset_at_once_and_ontextchanged_at_the_drain() {
    let mut s = script();
    s.run(
        r#"
        log = {}
        E = CreateFrame("EditBox", "E")
        E:SetScript("OnTextSet", function() table.insert(log, "set") end)
        E:SetScript("OnTextChanged", function() table.insert(log, "changed") end)
        E:SetText("hi")     -- fires set NOW, marks changed for the drain
        E:SetText("hi")     -- unchanged → short-circuit, no events at all
        E:SetText("bye")    -- fires set NOW; the pending change simply becomes "bye"
    "#,
    )
    .unwrap();
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["set", "set"], "no drain has run yet");

    s.tick(0.0);
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["set", "set", "changed"]);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "bye");
}

#[test]
fn numeric_aborts_a_mixed_insert_wholesale() {
    let s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetNumeric(true)
        E:Insert("12")     -- all digits: accepted
        E:Insert("3a")     -- one non-digit: the WHOLE insert aborts
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "12");
}

#[test]
fn max_letters_trims_from_the_end() {
    let s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetMaxLetters(3)
        E:Insert("abcde")
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abc");
    assert_eq!(s.eval::<i64>("return E:GetNumLetters()").unwrap(), 3);
}

// ── selection / highlight / keys ────────────────────────────────────────────────────────────

#[test]
fn highlight_all_then_typing_replaces_the_selection() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetText("hello")
        E:HighlightText(0, -1)   -- select all
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.char_input("X"), "focused: consumed");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "X");
}

#[test]
fn paste_inserts_at_the_cursor_and_replaces_the_selection() {
    let mut s = script();
    s.run(
        r#"
        changed = 0
        E = CreateFrame("EditBox", "E")
        E:SetScript("OnTextChanged", function() changed = changed + 1 end)
        E:SetText("ab")
        E:SetFocus()
        changed = 0   -- ignore the setup SetText's fire; count only the pastes below
    "#,
    )
    .unwrap();
    // SetText leaves the cursor at the end, so the paste appends.
    assert!(s.paste("CD"), "focused box consumes the paste");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abCD");
    s.run("E:HighlightText(0, -1)").unwrap();
    assert!(s.paste("xyz"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "xyz");
    // Both pastes marked the one box, so the drain fires once.
    assert_eq!(s.eval::<i64>("return changed").unwrap(), 0);
    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return changed").unwrap(), 1);
}

#[test]
fn paste_into_an_unfocused_box_is_not_consumed() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetAutoFocus(false)"#)
        .unwrap();
    assert!(!s.paste("hi"), "no focus, no autoFocus → not consumed");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "");
}

#[test]
fn paste_strips_newlines_in_a_single_line_box_but_keeps_them_when_multiline() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetFocus()"#)
        .unwrap();
    // Single-line: newlines/tabs and other control chars are dropped; spaces survive.
    assert!(s.paste("one\ntwo\tthree\r"));
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "onetwothree"
    );

    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetMultiLine(true); E:SetFocus()"#)
        .unwrap();
    assert!(s.paste("one\ntwo"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "one\ntwo");
}

#[test]
fn paste_honors_max_letters_and_numeric() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetMaxLetters(3); E:SetFocus()"#)
        .unwrap();
    assert!(s.paste("abcdef"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abc");

    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetNumeric(true); E:SetFocus()"#)
        .unwrap();
    assert!(s.paste("12a3"), "consumed even though the insert aborts");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "");
    assert!(s.paste("456"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "456");
}

#[test]
fn paste_does_not_fire_onspacepressed() {
    let mut s = script();
    s.run(
        r#"
        spaces = 0
        E = CreateFrame("EditBox", "E")
        E:SetScript("OnSpacePressed", function() spaces = spaces + 1 end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.paste("a b c"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "a b c");
    assert_eq!(
        s.eval::<i64>("return spaces").unwrap(),
        0,
        "a paste is not a typed space"
    );
}

#[test]
fn backspace_deletes_the_selection() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetText("hello")
        E:HighlightText(0, -1)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: true
    }));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "");
}

#[test]
fn enter_escape_tab_space_fire_their_slots() {
    let mut s = script();
    s.run(
        r#"
        log = {}
        E = CreateFrame("EditBox", "E")
        E:SetScript("OnEnterPressed", function() table.insert(log, "enter") end)
        E:SetScript("OnEscapePressed", function() table.insert(log, "escape") end)
        E:SetScript("OnTabPressed", function() table.insert(log, "tab") end)
        E:SetScript("OnSpacePressed", function() table.insert(log, "space") end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.key_input("ENTER")); // single-line → OnEnterPressed
    assert!(s.key_input("ESCAPE"));
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("E"),
        "ESCAPE keeps focus"
    );
    assert!(s.key_input("TAB"));
    assert!(s.char_input(" ")); // a literal space fires OnSpacePressed

    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["enter", "escape", "tab", "space"]);
    // Enter on a single-line box inserts nothing; only the space landed.
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), " ");
}

#[test]
fn multiline_enter_inserts_a_newline_without_onspacepressed() {
    let mut s = script();
    s.run(
        r#"
        spaces = 0
        E = CreateFrame("EditBox", "E")
        E:SetMultiLine(true)
        E:SetScript("OnSpacePressed", function() spaces = spaces + 1 end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.key_input("ENTER"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "\n");
    assert_eq!(s.eval::<i64>("return spaces").unwrap(), 0);
}

/// The method table `[0x87bb68, 0x87bce8)` has `SetAltArrowKeyMode`/`GetAltArrowKeyMode` at 46/47
/// and no `SetIgnoreArrows`; the XML `ignoreArrows` is the same flag (`[E+0x318] & 0x10`). The
/// gate is on the key, in the host (`UiKeyboardCapture::arrows_fall_through`), so an `EditAction`
/// that arrives here acts whatever the flag says.
#[test]
fn alt_arrow_key_mode_is_the_flag_and_the_engine_core_no_longer_swallows_moves() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetText("abc")
        E:SetFocus()
    "#,
    )
    .unwrap();

    assert!(
        s.eval::<bool>("return E.SetIgnoreArrows == nil").unwrap(),
        "5875 has no SetIgnoreArrows — an EditBox must not publish it"
    );

    assert_eq!(
        s.eval::<Option<i64>>("return E:GetAltArrowKeyMode()")
            .unwrap(),
        None
    );
    s.run("E:SetAltArrowKeyMode(1)").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return E:GetAltArrowKeyMode()")
            .unwrap(),
        Some(1)
    );
    assert!(
        s.eval::<bool>("return type(E:GetAltArrowKeyMode()) == 'number'")
            .unwrap(),
        "the set arm pushes the double 1.0 (0x6f3810), not a boolean"
    );

    // `GetBoolOrDefault` (`0x6f1c10`, default 1), arm by arm; three are backwards under Lua
    // truthiness.
    for (arg, want) in [
        ("", Some(1)),      // absent -> the default 1 -> enabled
        ("nil", None),      // nil -> 0
        ("0", None),        // __ftol(0) -> false
        ("-1", Some(1)),    // __ftol(-1) != 0 -> true
        (r#""0""#, None),   // the string "0" -> false
        (r#""""#, Some(1)), // "" matches no arm -> the default -> enabled
    ] {
        s.run("E:SetAltArrowKeyMode(nil)").unwrap();
        s.run(&format!("E:SetAltArrowKeyMode({arg})")).unwrap();
        assert_eq!(
            s.eval::<Option<i64>>("return E:GetAltArrowKeyMode()")
                .unwrap(),
            want,
            "SetAltArrowKeyMode({arg})"
        );
    }

    // A flagged box still acts on a Char move. `SetText` leaves the caret at the end, so one back
    // move puts it between 'b' and 'c'.
    s.run(r#"E:SetAltArrowKeyMode(1) E:SetText("abc")"#)
        .unwrap();
    assert!(s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false
    }));
    assert!(s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: true
    }));
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "ac",
        "the caret moved, so BACKSPACE took 'b' — a flagged box that is handed the action acts on it"
    );
}

/// The XML `ignoreArrows` is the same flag: its string (`0x879b78`) has one use, the attribute
/// read at `0x77a13a`.
#[test]
fn the_ignore_arrows_xml_attribute_is_alt_arrow_key_mode() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E")"#).unwrap();
    s.run("E:SetAltArrowKeyMode(1)").unwrap();
    assert_eq!(
        s.eval::<Option<i64>>("return E:GetAltArrowKeyMode()")
            .unwrap(),
        Some(1)
    );
}

// ── password + GetNumber ─────────────────────────────────────────────────────────────────────

#[test]
fn password_masks_the_display_but_gettext_returns_the_real_text() {
    let s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetPassword(true)
        E:SetText("secret")
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "secret");
    assert_eq!(
        text_quad(&s).as_deref(),
        Some("******"),
        "the text region shows one '*' per character"
    );
}

#[test]
fn get_number_parses_the_text() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetText("12.5")"#)
        .unwrap();
    assert_eq!(s.eval::<f64>("return E:GetNumber()").unwrap(), 12.5);
    s.run(r#"E:SetText("hello")"#).unwrap();
    assert_eq!(s.eval::<f64>("return E:GetNumber()").unwrap(), 0.0);
}

// ── the generic OnChar and OnKeyDown ─────────────────────────────────────────────────────────

#[test]
fn typing_fires_the_generic_on_char_with_what_was_inserted() {
    // The box's input vtable does not chain to the base, but Insert fires the generic `OnChar`
    // (`+0x180`) at `0x77c13c`, through the varargs firer `0x7026f0` (not the fixed-arity
    // `0x702690`) with the inserted string.
    let mut s = script();
    s.run(
        r#"
        got = {}
        E = CreateFrame("EditBox", "E")
        E:EnableKeyboard(true)
        E:SetScript("OnChar", function() table.insert(got, arg1) end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.char_input("k"), "the focused box consumed the character");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "k");
    assert_eq!(
        s.eval::<String>("return table.concat(got, ',')").unwrap(),
        "k",
        "OnChar fires once, with the inserted string",
    );

    // `SetText` is not an insert and fires no `OnChar`, as in the reference.
    s.run(r#"E:SetText("zzz")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(got, ',')").unwrap(),
        "k",
        "SetText does not run Insert, so it does not fire OnChar",
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn key_paths_never_fire_the_generic_on_key_down() {
    // The box's key-down handler (`0x77b160`) never chains to the base, so a generic `OnKeyDown`
    // on the focused box does not fire.
    let mut s = script();
    s.run(
        r#"
        fired = 0
        E = CreateFrame("EditBox", "E")
        E:EnableKeyboard(true)
        E:SetScript("OnKeyDown", function() fired = fired + 1 end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert!(s.char_input("k"), "the focused box consumed the character");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "k");
    assert_eq!(
        s.eval::<i64>("return fired").unwrap(),
        0,
        "the box's own vtable never chains to the generic OnKeyDown"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── history recall (`historyLines`, AddHistoryLine) ──────────────────────────────────────────

#[test]
fn history_recall_walks_up_and_down_and_restores_the_draft() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetHistoryLines(32)
        E:AddHistoryLine("/say one")
        E:AddHistoryLine("/g two")
        E:SetText("draft")
        E:SetFocus()
    "#,
    )
    .unwrap();
    // UP recalls newest-first; a second UP walks older; at the oldest it holds.
    assert!(s.editbox_action(EditAction::HistoryPrev));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/g two");
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/say one");
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/say one");
    // DOWN walks newer; past the newest the stashed live draft comes back.
    s.editbox_action(EditAction::HistoryNext);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/g two");
    s.editbox_action(EditAction::HistoryNext);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "draft");
    // Not browsing: DOWN does nothing.
    s.editbox_action(EditAction::HistoryNext);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "draft");
}

#[test]
fn typing_ends_the_history_browse() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetHistoryLines(32)
        E:AddHistoryLine("older")
        E:AddHistoryLine("newer")
        E:SetFocus()
    "#,
    )
    .unwrap();
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "newer");
    // A typed char turns the recalled line into a draft: the next UP starts a fresh browse from
    // the newest entry, stashing the edited line as the draft.
    s.char_input("!");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "newer!");
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "newer");
    s.editbox_action(EditAction::HistoryNext);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "newer!");
}

#[test]
fn programmatic_set_text_keeps_the_browse_and_focus_gain_resets_it() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetHistoryLines(32)
        E:AddHistoryLine("/say one")
        E:AddHistoryLine("/g two")
        E:SetFocus()
    "#,
    )
    .unwrap();
    // Rewrite the recalled line as the chat parser does ("/g two" → Guild + "two"); the next UP
    // must reach the older entry, not the newest again.
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/g two");
    s.run(r#"E:SetText("two")"#).unwrap();
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/say one");
    // Refocusing starts a fresh session: the stale walk drops, UP recalls the newest again.
    s.run(r#"E:ClearFocus() E:SetFocus()"#).unwrap();
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "/g two");
}

#[test]
fn history_caps_at_history_lines_drop_oldest() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetHistoryLines(2)
        E:AddHistoryLine("a")
        E:AddHistoryLine("b")
        E:AddHistoryLine("c")
        E:SetFocus()
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return E:GetHistoryLines()").unwrap(), 2);
    // Only b/c survive: two UPs land on 'b', a third holds there.
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "c");
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "b");
    s.editbox_action(EditAction::HistoryPrev);
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "b");
}

#[test]
fn history_off_by_default_and_up_is_still_consumed() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetText("t"); E:SetFocus()"#)
        .unwrap();
    // No historyLines: AddHistoryLine does nothing, and UP is consumed but inert, since a focused
    // box eats every key (both handlers return 1, `0x77a900`/`0x77b160`).
    s.run(r#"E:AddHistoryLine("x")"#).unwrap();
    assert!(s.editbox_action(EditAction::HistoryPrev));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "t");
}

// ── SetTextInsets ────────────────────────────────────────────────────────────────────────────

#[test]
fn text_insets_shrink_the_text_region_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(400)
        E:SetHeight(32)
        E:SetTextInsets(15, 13, 2, 3)
        E:SetText("hello")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let rect = quads
        .iter()
        .find_map(|q| match (&q.content, q.rect) {
            (crate::script::QuadContent::Text { text: Some(t), .. }, Some(r)) if t == "hello" => {
                Some(r)
            }
            _ => None,
        })
        .expect("the edit box text renders");
    assert!((rect.left - 115.0).abs() < 0.01, "left inset 15: {rect:?}");
    assert!(
        (rect.right - 487.0).abs() < 0.01,
        "right inset 13: {rect:?}"
    );
    assert!((rect.top - 80.0).abs() < 0.01, "top inset 2: {rect:?}");
    assert!(
        (rect.bottom - 53.0).abs() < 0.01,
        "bottom inset 3: {rect:?}"
    );
    let (l, r_, t, b) = s
        .eval::<(f32, f32, f32, f32)>("return E:GetTextInsets()")
        .unwrap();
    assert_eq!((l, r_, t, b), (15.0, 13.0, 2.0, 3.0));
}

// SetTextColor tints the box's text region, as `ChatEdit_UpdateHeader` colours the typed text.
#[test]
fn set_text_color_tints_the_text_region() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(400)
        E:SetHeight(32)
        E:SetText("hello")
        E:SetTextColor(1.0, 0.5, 0.25)
    "#,
    )
    .unwrap();
    s.resolve();
    let color = s
        .extract()
        .iter()
        .find_map(|q| match &q.content {
            crate::script::QuadContent::Text {
                text: Some(t),
                color,
                ..
            } if t == "hello" => Some(*color),
            _ => None,
        })
        .expect("the edit box text renders");
    assert_eq!(color, Some([1.0, 0.5, 0.25, 1.0]));
}

// ── the host text-UI seam: advances, caret/selection geometry, mouse, clipboard ─────────────

/// A focused 200×32 box at BOTTOMLEFT(100, 50) holding `text`, its advances answered at 7 px per
/// byte.
fn seam_rig(s: &mut UiScript, text: &str) {
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetWidth(200); E:SetHeight(32)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetFocus()
    "#,
    )
    .unwrap();
    for ch in text.chars() {
        s.char_input(&ch.to_string());
    }
    s.resolve();
    answer_advances(s);
}

/// Answer any pending advance request at 7 px per byte.
fn answer_advances(s: &mut UiScript) {
    if let Some(req) = s.editbox_advances_request() {
        let cum: Vec<f32> = (0..=req.text.len()).map(|i| i as f32 * 7.0).collect();
        s.set_editbox_advances(req.id, req.key, cum, vec![0], 0.0);
    }
}

/// Caret geometry comes from the advance table, and there is none without focus or visibility.
#[test]
fn text_ui_reports_caret_geometry_and_focus() {
    let mut s = script();
    seam_rig(&mut s, "");
    // Empty box: the advance table settles engine-side; the caret stands at the origin.
    let ui = s
        .focused_editbox_text_ui()
        .expect("focused box has text-UI");
    assert_eq!(ui.caret_x, 0.0);
    assert_eq!(ui.display_from, 0);
    assert!(ui.selection.is_empty());
    assert!(
        s.extract().iter().any(|q| matches!(
            (&q.target, &q.content),
            (t, QuadContent::Text { text: Some(x), .. }) if *t == ui.target && x.is_empty()
        )),
        "an empty focused box must still emit its text-region quad"
    );

    s.char_input("a");
    s.char_input("b");
    answer_advances(&mut s);
    assert_eq!(s.focused_editbox_text_ui().unwrap().caret_x, 14.0);
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false,
    });
    assert_eq!(s.focused_editbox_text_ui().unwrap().caret_x, 7.0);

    s.run("E:ClearFocus()").unwrap();
    assert!(s.focused_editbox_text_ui().is_none(), "no focus, no caret");
    s.run("E:SetFocus(); E:Hide()").unwrap();
    assert!(s.focused_editbox_text_ui().is_none(), "hidden box: none");
}

/// Click places the cursor at the nearest char boundary (`0x77b800` → `0x77d0d0`), collapsing
/// any selection; dragging extends it (`0x77a860`); release ends the drag.
#[test]
fn click_places_cursor_and_drag_selects() {
    let mut s = script();
    seam_rig(&mut s, "hello world");
    // Box left = 100; the text region fills it (no insets). Byte i sits at x = 100 + 7i.
    // Click at x=121 → advance-space 21 → boundary 3.
    assert!(s.mouse_button(121.0, 60.0, "LeftButton", true));
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.caret_x, 21.0, "cursor at byte 3");
    assert!(ui.selection.is_empty(), "click collapses");
    // Drag right to x=149 (byte 7): selection 3..7, cursor at 7.
    s.mouse_move(149.0, 60.0);
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.selection, vec![(0, 21.0, 49.0)]);
    assert_eq!(ui.caret_x, 49.0);
    // Drag back left past the anchor to x=107 (byte 1): the selection flips to 1..3.
    s.mouse_move(107.0, 60.0);
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.selection, vec![(0, 7.0, 21.0)]);
    s.mouse_button(107.0, 60.0, "LeftButton", false);
    s.mouse_move(170.0, 60.0);
    assert_eq!(
        s.focused_editbox_text_ui().unwrap().selection,
        vec![(0, 7.0, 21.0)]
    );
    // Nearest-boundary rounding: x=124.9 (advance 24.9): |24.9−21|=3.9 vs |28−24.9|=3.1 → byte 4.
    s.mouse_button(124.9, 60.0, "LeftButton", true);
    assert_eq!(s.focused_editbox_text_ui().unwrap().caret_x, 28.0);
    s.mouse_button(124.9, 60.0, "LeftButton", false);
}

/// Ctrl+arrows jump word boundaries (alnum runs); Ctrl+Shift extends the selection there.
#[test]
fn ctrl_arrows_jump_words() {
    let mut s = script();
    seam_rig(&mut s, "abc def");
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    });
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Word,
        back: false,
        extend: false,
    });
    assert_eq!(
        s.focused_editbox_text_ui().unwrap().caret_x,
        21.0,
        "end of 'abc'"
    );
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Word,
        back: false,
        extend: false,
    });
    assert_eq!(
        s.focused_editbox_text_ui().unwrap().caret_x,
        49.0,
        "end of 'def'"
    );
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Word,
        back: true,
        extend: false,
    });
    assert_eq!(
        s.focused_editbox_text_ui().unwrap().caret_x,
        28.0,
        "start of 'def'"
    );
    // Ctrl+Shift+LEFT from here selects back across 'abc'.
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Word,
        back: true,
        extend: true,
    });
    assert_eq!(
        s.focused_editbox_text_ui().unwrap().selection,
        vec![(0, 0.0, 28.0)]
    );
}

/// Copy needs a selection, cut copies then deletes, and a password box copies its mask run, never
/// the text (the client copies the empty string, `0x882748`).
#[test]
fn copy_cut_and_the_password_placeholder() {
    let mut s = script();
    seam_rig(&mut s, "secret");
    assert_eq!(s.editbox_copy(), None, "no selection, no copy");
    s.run("E:HighlightText(0, 3)").unwrap();
    assert_eq!(s.editbox_copy().as_deref(), Some("sec"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "secret");
    assert_eq!(s.editbox_cut().as_deref(), Some("sec"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "ret");

    s.run(r#"P = CreateFrame("EditBox", "P"); P:SetPassword(true); P:SetFocus()"#)
        .unwrap();
    s.char_input("h");
    s.char_input("i");
    s.run("P:HighlightText(0, -1)").unwrap();
    assert_eq!(
        s.editbox_copy().as_deref(),
        Some("**"),
        "password copy must never yield the real text"
    );
}

/// Word and edge deletes, the host's OS-native chords with no 1.12 counterpart: a word delete
/// takes the adjacent alphanumeric run and the separators toward it, an edge delete clears to the
/// line's end, and either deletes a live selection instead.
#[test]
fn word_and_edge_deletes() {
    let mut s = script();
    seam_rig(&mut s, "abc def ghi");
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Word,
        back: true,
    });
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abc def ");
    // Again: the space + "def" go together (back-walk skips separators first).
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Word,
        back: true,
    });
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abc ");
    // Word-delete forward from HOME: the "abc" run goes, its trailing space survives.
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    });
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Word,
        back: false,
    });
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), " ");

    s.run(r#"E:SetText("hello world"); E:HighlightText(2, 5)"#)
        .unwrap();
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Word,
        back: true,
    });
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "he world");

    // Edge-delete back (macOS Cmd+Backspace) from the end clears the whole line.
    s.run(r#"E:SetText("clear me")"#).unwrap();
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Edge,
        back: true,
    });
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "");
    // Inert at the boundary: nothing to delete consumes without firing OnTextChanged.
    s.run("n = 0; E:SetScript('OnTextChanged', function() n = n + 1 end)")
        .unwrap();
    assert!(s.editbox_action(EditAction::Delete {
        unit: EditUnit::Edge,
        back: true
    }));
    assert_eq!(s.eval::<i64>("return n").unwrap(), 0);
}

/// SelectAll, the reference's Ctrl+A (`HighlightText(0, -1)`): the whole text selected, the caret
/// at the end, and typing replaces it all.
#[test]
fn select_all_action() {
    let mut s = script();
    seam_rig(&mut s, "abc def");
    s.editbox_action(EditAction::SelectAll);
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.selection, vec![(0, 0.0, 49.0)], "all 7 bytes selected");
    assert_eq!(ui.caret_x, 49.0, "caret at the end");
    s.char_input("z");
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "z",
        "an insert replaces the whole selection"
    );
}

/// The scroll window keeps the cursor visible (`0x77da80`) in whole chars: typing past the edge
/// advances `display_from`, HOME snaps it back.
#[test]
fn scroll_window_keeps_the_cursor_visible() {
    let mut s = script();
    // 40 bytes × 7px = 280px in a 200px box (avail 198): the window must slide.
    seam_rig(&mut s, &"x".repeat(40));
    let ui = s.focused_editbox_text_ui().unwrap();
    assert!(ui.display_from > 0, "overlong text must scroll: {ui:?}");
    assert!(ui.caret_x <= 198.0, "caret inside the window: {ui:?}");
    assert_eq!(
        ui.caret_x,
        (40 - ui.display_from) as f32 * 7.0,
        "caret x measures from the window origin"
    );
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    });
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.display_from, 0);
    assert_eq!(ui.caret_x, 0.0);
}

/// The caret blink (`0x77a790`): a 0.5 s half-period, reset to shown by every edit.
#[test]
fn caret_blinks_on_tick_and_resets_on_edit() {
    let mut s = script();
    seam_rig(&mut s, "a");
    assert!(s.focused_editbox_text_ui().unwrap().caret_on);
    s.tick(0.6); // crosses the 0.5 default → hidden
    assert!(!s.focused_editbox_text_ui().unwrap().caret_on);
    s.tick(0.6); // crosses again → shown
    assert!(s.focused_editbox_text_ui().unwrap().caret_on);
    s.tick(0.6);
    assert!(!s.focused_editbox_text_ui().unwrap().caret_on);
    s.char_input("b");
    answer_advances(&mut s);
    assert!(s.focused_editbox_text_ui().unwrap().caret_on);
}

// ── a hyperlink is one keypress (`AdvanceTokens 0x77bb30`) ─────────────────────
//
// Driven through the keyboard API: a Backspace that took only the link's trailing `|r` would
// colour everything typed after it.

/// An epic link exactly as the client's own builder formats one (`0x52adb0`).
const LINK: &str = "|cffa335ee|Hitem:11684:0:0:0|h[Ironfoe]|h|r";

fn box_with_link(suffix: &str) -> UiScript {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetFocus()"#)
        .unwrap();
    s.run(&format!("E:Insert(\"{LINK}{suffix}\")")).unwrap();
    s
}

fn text_of(s: &UiScript) -> String {
    s.eval::<String>("return E:GetText()").unwrap()
}

#[test]
fn one_backspace_deletes_a_whole_item_link() {
    let mut s = box_with_link("");
    assert!(s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: true
    }));
    assert_eq!(text_of(&s), "", "colour prefix and trailing |r go with it");

    // Behind following text: the text goes first, then the whole link, never leaving a `|c`.
    let mut s = box_with_link("ab");
    for expected in ["|cffa335ee|Hitem:11684:0:0:0|h[Ironfoe]|h|ra", LINK, ""] {
        s.editbox_action(EditAction::Delete {
            unit: EditUnit::Char,
            back: true,
        });
        assert_eq!(text_of(&s), expected);
    }
}

#[test]
fn one_arrow_crosses_a_whole_item_link() {
    let mut s = box_with_link("");
    let cursor = |s: &mut UiScript| {
        s.editbox_action(EditAction::Move {
            unit: EditUnit::Edge,
            back: true,
            extend: false,
        });
        s.editbox_action(EditAction::Move {
            unit: EditUnit::Char,
            back: false,
            extend: false,
        });
    };
    cursor(&mut s);
    // One RIGHT from the start passes the whole link, so typing there appends.
    s.char_input("x");
    assert_eq!(text_of(&s), format!("{LINK}x"));
}

#[test]
fn shift_arrow_selects_the_whole_link_and_typing_replaces_it() {
    let mut s = box_with_link("");
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: true,
    });
    assert_eq!(
        s.editbox_copy().as_deref(),
        Some(LINK),
        "one Shift+LEFT takes the whole unit"
    );
    s.char_input("x");
    assert_eq!(text_of(&s), "x");
}

#[test]
fn typing_strictly_inside_a_link_is_refused() {
    // Only the mouse can put the caret there, so place it the way a click would.
    let mut s = box_with_link("");
    s.run("E:HighlightText(35, 35)").unwrap(); // mid-"Ironfoe"
    s.char_input("x");
    assert_eq!(
        text_of(&s),
        LINK,
        "the client swallows it rather than splitting the link"
    );
    // At the link's leading edge the keystroke lands: the guard also tests the previous token.
    s.run("E:HighlightText(0, 0)").unwrap();
    s.char_input("x");
    assert_eq!(text_of(&s), format!("x{LINK}"));
}

#[test]
fn a_typed_pipe_is_doubled_and_costs_one_letter() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetFocus()"#)
        .unwrap();
    s.char_input("|");
    assert_eq!(
        text_of(&s),
        "||",
        "OnChar 0x77c200 inserts the literal at 0x879cac"
    );
    assert_eq!(s.eval::<i64>("return E:GetNumLetters()").unwrap(), 1);
}

#[test]
fn max_letters_counts_visible_letters_not_escape_bytes() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetFocus(); E:SetMaxLetters(20)"#)
        .unwrap();
    s.run(&format!("E:Insert(\"{LINK}\")")).unwrap();
    // 43 bytes, but "[Ironfoe]" is 9 letters, under the 20-letter cap.
    assert_eq!(text_of(&s), LINK);
    assert_eq!(s.eval::<i64>("return E:GetNumLetters()").unwrap(), 9);
}

/// A box born visible has had no show transition, so the OnShow focus never runs for it; if it
/// did, loading the UI would hand the keyboard to the first edit box built.
#[test]
fn creating_a_box_does_not_focus_it_the_way_showing_one_does() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E")"#).unwrap();
    assert!(
        !s.has_keyboard_focus(),
        "a box born visible has not been SHOWN, so it takes no keyboard",
    );
    assert_eq!(s.focused_editbox_name(), None);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetMaxLetters` gates the exact count (`lua_gettop`, `0x6f3070`) and reads the value with a bare
/// `lua_tonumber` (`0x6f3620`); 0 is no limit.
#[test]
fn set_max_letters_gates_the_argument_count_and_coerces_the_value() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E") E:SetFocus()"#)
        .unwrap();
    let max = |s: &UiScript| s.eval::<i64>("return E:GetMaxLetters()").unwrap();

    s.run("E:SetMaxLetters(12)").unwrap();
    assert_eq!(max(&s), 12);

    // nil stores 0, which is unlimited.
    s.run("E:SetMaxLetters(nil)").unwrap();
    assert_eq!(max(&s), 0);
    s.run(r#"E:SetMaxLetters(5) E:SetText("abcdefgh")"#)
        .unwrap();
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap().len(), 5);
    s.run(r#"E:SetMaxLetters(nil) E:SetText("abcdefgh")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "abcdefgh",
        "0 skips the trim block whole (0x77c085) — it is no limit, not no letters"
    );

    // A numeric string coerces; anything else is 0.
    s.run(r#"E:SetMaxLetters("12")"#).unwrap();
    assert_eq!(max(&s), 12);
    s.run("E:SetMaxLetters({})").unwrap();
    assert_eq!(max(&s), 0);

    assert!(s.run("E:SetMaxLetters()").is_err(), "too few raises");
    assert!(
        s.run("E:SetMaxLetters(50, 60)").is_err(),
        "too many raises too"
    );
}

/// The ctor builds five regions in order, the text FontString (`0x779bee`), three selection
/// textures (`0x779c41`-`0x779c72`) and the caret (`0x779c86`), and `GetRegions` (`0x773f60`)
/// walks one creation-ordered list, so authored regions start at index 6.
#[test]
fn an_editbox_is_born_with_the_ctors_five_regions_ahead_of_its_authored_ones() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <EditBox name="Box">
                <Size><AbsDimension x="120" y="20"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Layers>
                    <Layer level="BACKGROUND">
                        <Texture name="BoxLeft" file="Interface\Left"/>
                        <Texture name="BoxRight" file="Interface\Right"/>
                    </Layer>
                </Layers>
                <FontString inherits="ChatFontNormal"/>
            </EditBox>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

    // The embedded `<FontString>` declares the ctor's text region, not a sixth (`0x779fb0`).
    assert_eq!(
        s.eval::<i64>("return Box:GetNumRegions()").unwrap(),
        7,
        "5 ctor regions + 2 authored textures, and the <FontString> adds none"
    );
    assert_eq!(
        s.arity("Box:GetRegions()").unwrap(),
        7,
        "GetNumRegions is exactly the length GetRegions enumerates"
    );

    let (left, right) = s
        .eval::<(String, String)>(
            "local _,_,_,_,_,l,r = Box:GetRegions() return l:GetTexture(), r:GetTexture()",
        )
        .unwrap();
    assert_eq!(
        left, r"Interface\Left",
        "region 6 is the first authored one"
    );
    assert_eq!(right, r"Interface\Right", "region 7 is the second");

    s.run(r#"Box:SetText("typed")"#).unwrap();
    assert_eq!(
        s.eval::<String>("local t = Box:GetRegions() return t:GetText()")
            .unwrap(),
        "typed",
        "region 1 is the ctor's embedded text FontString"
    );
    // 2..5, the selection quads and the caret, carry no art: benilla paints both host-side.
    assert!(
        s.eval::<bool>(
            "local a,b,c,d,e = Box:GetRegions() \
             return b:GetTexture() == nil and c:GetTexture() == nil \
                and d:GetTexture() == nil and e:GetTexture() == nil"
        )
        .unwrap(),
        "the selection trio and the caret are blank quads"
    );
}

/// `SetNumber` is `SetText` (`0x798690` and `0x7984c0` are byte-identical), so a number arrives
/// through Lua's `%.14g`; the expected strings are the reference's own output.
#[test]
fn set_number_formats_like_the_references_printf() {
    let s = UiScript::new().unwrap();
    s.run(r#"box = CreateFrame("EditBox", "NumBox", UIParent)"#)
        .unwrap();

    for (input, want) in [
        ("3", "3"),
        ("0.8", "0.8"),
        ("-0.5", "-0.5"),
        ("0.1 + 0.2", "0.3"),
        ("1/3", "0.33333333333333"),
        ("1e13", "10000000000000"),
        // The three-digit exponent, which is MSVC's `%g` and not C99's. `1e+020`, never `1e+20`.
        ("1e14", "1e+014"),
        ("1e20", "1e+020"),
        ("1e-5", "1e-005"),
        // Just inside the fixed-notation window.
        ("1e-4", "0.0001"),
        // `%g` drops the sign on negative zero.
        ("-0.0", "0"),
    ] {
        s.run(&format!("box:SetNumber({input})")).unwrap();
        assert_eq!(
            s.eval::<String>("return box:GetText()").unwrap(),
            want,
            "SetNumber({input})"
        );
    }

    // A string passes verbatim: the gate is `lua_isstring`, and nothing parses it.
    s.run(r#"box:SetNumber("abc")"#).unwrap();
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "abc");

    // Anything else raises, no argument included.
    for bad in [
        "box:SetNumber()",
        "box:SetNumber(nil)",
        "box:SetNumber(true)",
    ] {
        assert!(s.run(bad).is_err(), "{bad} must raise");
    }
}

/// A numeric box aborts the whole insert after the clear has run, so text that fails the digit
/// test leaves it empty, not partly filled.
#[test]
fn set_number_on_a_numeric_box_empties_it_when_the_text_is_not_all_digits() {
    let s = UiScript::new().unwrap();
    s.run(r#"box = CreateFrame("EditBox", "NumOnly", UIParent) box:SetNumeric(true)"#)
        .unwrap();

    s.run("box:SetNumber(1234)").unwrap();
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "1234");

    s.run("box:SetNumber(-5)").unwrap();
    assert_eq!(
        s.eval::<String>("return box:GetText()").unwrap(),
        "",
        "a sign empties a numeric box rather than partly filling it"
    );
}

/// `SetMaxBytes`: `SetMaxLetters`'s exact count gate and raw coercion, a byte cap, and -1 as the
/// no-limit sentinel.
#[test]
fn set_max_bytes_caps_the_buffer_in_bytes_with_minus_one_unlimited() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E") E:SetFocus()"#)
        .unwrap();
    let max = |s: &UiScript| s.eval::<i64>("return E:GetMaxBytes()").unwrap();
    assert_eq!(max(&s), -1, "unlimited from the ctor");
    s.run("E:SetMaxBytes(4)").unwrap();
    assert_eq!(max(&s), 4);
    // Four bytes hold two two-byte letters and not a third.
    s.run(r#"E:SetText("ééé")"#).unwrap();
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "éé");
    s.run(r#"E:SetMaxBytes(0) E:SetText("ééé")"#).unwrap();
    assert_eq!(max(&s), -1, "a non-positive value is unlimited");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "ééé");
    s.run(r#"E:SetMaxBytes("3") E:SetText("abcd")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "abc",
        "a numeric string coerces"
    );
    s.run("E:SetMaxBytes(nil)").unwrap();
    assert_eq!(max(&s), -1, "nil coerces to 0, which is unlimited");
    assert!(s.run("E:SetMaxBytes()").is_err(), "too few raises");
    assert!(s.run("E:SetMaxBytes(1, 2)").is_err(), "too many raises");
}

/// `OnCursorChanged(x, y, w, h)` fires when the caret moves, with the reference's args
/// (`0x77dd5f`, each scaled): `x` the advance along the line, `y` negative downward by row, `w`
/// the constant 4, `h` the line height. Stock `ScrollingEdit_OnCursorChanged` reads `-y` as the
/// distance down.
#[test]
fn the_caret_flush_fires_on_cursor_changed_with_the_references_four_args() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires = {}
        E = CreateFrame("EditBox", "E")
        E:SetWidth(200); E:SetHeight(64)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetMultiLine(true)
        E:SetScript("OnCursorChanged", function()
            table.insert(fires, { x = arg1, y = arg2, w = arg3, h = arg4 })
        end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    s.resolve();
    // Three bytes on row 0, then a wrap onto row 1; the host answers the rows and the pitch.
    for ch in "abcdef".chars() {
        s.char_input(&ch.to_string());
    }
    s.resolve();
    if let Some(req) = s.editbox_advances_request() {
        // 7 px per byte, wrapped into two rows at byte 3: `x` must count from the row's start.
        let cum: Vec<f32> = (0..=req.text.len()).map(|i| i as f32 * 7.0).collect();
        s.set_editbox_advances(req.id, req.key, cum, vec![0, 3], 12.0);
    }
    s.tick(0.016);
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    let n: i64 = s.eval("return table.getn(fires)").unwrap();
    assert!(n >= 1, "the flush fired at least once");
    let (x, y, w, h): (f64, f64, f64, f64) = s
        .eval("local f = fires[table.getn(fires)] return f.x, f.y, f.w, f.h")
        .unwrap();
    // The caret sits after 6 bytes: row 1 (rows start at 0 and 3), 3 bytes along it.
    assert_eq!(
        x, 21.0,
        "x is the advance from the ROW's start, not the text's"
    );
    assert_eq!(
        y, -12.0,
        "y is minus the row index times the pitch — downward is negative"
    );
    assert_eq!(w, 4.0, "w is the reference's constant, not a measurement");
    assert_eq!(h, 12.0, "h is the line height");

    // It fires on a change only, which lets `ScrollingEdit_OnUpdate`'s `if (this.cursorOffset)`
    // settle instead of scrolling every frame.
    let before: i64 = s.eval("return table.getn(fires)").unwrap();
    s.tick(0.016);
    s.tick(0.016);
    assert_eq!(
        s.eval::<i64>("return table.getn(fires)").unwrap(),
        before,
        "two quiet ticks fire nothing"
    );

    s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false,
    });
    s.tick(0.016);
    assert!(
        s.eval::<i64>("return table.getn(fires)").unwrap() > before,
        "moving the caret one char fires the flush"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
