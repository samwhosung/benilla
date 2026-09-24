//! Rust-driven tests of the EditBox runtime: focus acquisition + routing, text/cursor/
//! selection editing, the specialized script fires, caps, numeric/password, and text-region sync.
//! Frames are built programmatically via `CreateFrame("EditBox", …)` and driven through the public
//! keyboard API (`char_input`/`key_input`/`editbox_action`/`has_keyboard_focus`) and the Lua
//! method surface.

use crate::script::{EditAction, EditUnit, QuadContent, UiScript};

fn script() -> UiScript {
    UiScript::new().expect("construct UiScript")
}

/// The display text of the (single) text quad an extract produces, if any.
fn text_quad(s: &UiScript) -> Option<String> {
    s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Text { text, .. } => text,
        _ => None,
    })
}

// ── focus acquisition + routing ─────────────────────────────────────────────────────────────

/// **The self-acquire path**, path 2 of the two: a box that has had no show TRANSITION (created
/// already visible, so `visibility_focus` never runs for it) still takes the keyboard on the first
/// key or char event, and processes that same event. Path 1, focus-on-show, is
/// [`an_autofocus_box_takes_the_keyboard_when_it_is_shown`] below — this test's name used to assert
/// its absence, which stopped being true at decision 1686.
#[test]
fn an_autofocus_box_self_acquires_on_the_first_event() {
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetAutoFocus(true)"#)
        .unwrap();
    // No show transition has run for this box, so nothing owns the keyboard yet.
    assert!(!s.has_keyboard_focus());
    assert_eq!(s.focused_editbox_name(), None);

    // The first char self-acquires focus AND processes that same event.
    assert!(
        s.char_input("a"),
        "an autoFocus box consumes the acquiring event"
    );
    assert!(s.has_keyboard_focus());
    assert_eq!(s.focused_editbox_name().as_deref(), Some("E"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "a");
}

/// **An `autoFocus` box takes the keyboard when it is shown** — the OnShow vtable override
/// (`0x81c910` slot +0x30, `0x77a750`), missing here until decision 1686. Gated on nothing else
/// holding focus, and the gate is the whole of it — no topmost/best choice.
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

    // Hiding the box that holds the keyboard releases it (the mirror override, slot +0x34).
    s.run("E:Hide()").unwrap();
    assert!(
        !s.has_keyboard_focus(),
        "hiding the focused box releases the keyboard",
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The two gates, each shown to bite. A box that opted out does not take the keyboard on show; and
/// a box that would have is refused while another still holds it — the reference's
/// `if ([0xcf4dc8] == 0 && (flags & 1))`, both halves.
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

    // Now give the keyboard away, and show an autoFocus box into an occupied focus.
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

    // And hiding a box that does NOT hold the keyboard leaves it where it is — `0x77e410`'s own
    // per-box guard (`cmp ecx,eax; jne ret`), which is what makes the override's unconditional
    // tail-jmp harmless.
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
    // `SetAutoFocus(false)` explicitly: the ctor's `flags = 1` leaves autoFocus **ON** by default
    // (`0x779a29`/`0x779a2e`, decision 1686), so a bare `CreateFrame("EditBox")` is an autoFocus
    // box and would self-acquire on the first char. This test is about the other kind.
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
    // Empty text: these do nothing, but a focused box still consumes them.
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

    // Neither box has autoFocus, yet a click focuses each.
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

/// **`OnTextChanged` is deferred and coalesced** (decision 1831). An edit only raises the
/// `textChanged` dirty bit; the fire belongs to the drain (`0x77d3e0`) that the box's own OnUpdate
/// runs. So three typed characters are three marks on ONE box and produce exactly ONE fire — this
/// test asserted three before the law was checked.
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

    // Nothing is pending now, so a second drain fires nothing.
    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return changed").unwrap(), 1);
}

/// The two fires part company (decision 1831): **`OnTextSet` is synchronous, inside `SetText`
/// itself** (`0x77be6b`), while `OnTextChanged` waits for the drain. The equality short-circuit
/// still suppresses both. So writing "hi", "hi", "bye" before any drain logs two `set`s and then a
/// SINGLE `changed` carrying only the last value — `A → B` coalesces.
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
    // Cursor sits at end after SetText → paste appends.
    assert!(s.paste("CD"), "focused box consumes the paste");
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abCD");
    // Select-all then paste replaces the whole selection.
    s.run("E:HighlightText(0, -1)").unwrap();
    assert!(s.paste("xyz"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "xyz");
    // Both pastes marked the same box, so the drain owes exactly one fire — not one per paste.
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

    // Multi-line box: the newline survives (a tab is still a dropped control char).
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetMultiLine(true); E:SetFocus()"#)
        .unwrap();
    assert!(s.paste("one\ntwo"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "one\ntwo");
}

#[test]
fn paste_honors_max_letters_and_numeric() {
    // maxLetters trims the paste from the end.
    let mut s = script();
    s.run(r#"E = CreateFrame("EditBox", "E"); E:SetMaxLetters(3); E:SetFocus()"#)
        .unwrap();
    assert!(s.paste("abcdef"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "abc");

    // numeric aborts a paste that carries any non-digit (matching the typed-insert rule).
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
    assert!(s.key_input("ESCAPE")); // fires, does NOT release focus
    assert_eq!(
        s.focused_editbox_name().as_deref(),
        Some("E"),
        "ESCAPE keeps focus"
    );
    assert!(s.key_input("TAB"));
    assert!(s.char_input(" ")); // a literal space fires OnSpacePressed

    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["enter", "escape", "tab", "space"]);
    // ENTER on a single-line box must NOT insert; only the space landed.
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

/// **Alt-arrow mode: the two real verbs, and the flag they share with the XML attribute.**
///
/// The reference settles four things this pins:
///
///  · 5875 has **no `SetIgnoreArrows`** — the 48-entry EditBox method table
///    `[0x87bb68, 0x87bce8)` carries `SetAltArrowKeyMode`/`GetAltArrowKeyMode` at 46/47 and no
///    entry whose name contains "Ignore". benilla published the invented name for two rounds and
///    was missing both real ones.
///  · The XML attribute `ignoreArrows` and the Lua verbs drive **one** flag (`[E+0x318] & 0x10`).
///  · The setter's argument is `GetBoolOrDefault(L, 2, default = 1)` (`0x6f1c10`), not Lua
///    truthiness — an **absent** argument ENABLES, `""` ENABLES, `0` and `"0"` disable.
///  · The getter answers the **number 1 or nil**, never a boolean.
///
/// And the negative that matters: the engine core no longer swallows anything. The gate is on the
/// KEY and lives in the host (`UiKeyboardCapture::arrows_fall_through`), so an `EditAction` that
/// arrives here moves the caret whatever the flag says — it only ever arrives when ALT was held or
/// the key was not an arrow.
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

    // The invented name is gone.
    assert!(
        s.eval::<bool>("return E.SetIgnoreArrows == nil").unwrap(),
        "5875 has no SetIgnoreArrows — publishing it was decision 1189's error"
    );

    // The getter: a number or nil, never a boolean.
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

    // `GetBoolOrDefault(default = 1)`, arm by arm — three of these are backwards under Lua
    // truthiness, which is why the coercion has its own helper.
    for (arg, want) in [
        ("", Some(1)),      // ABSENT -> the default 1 -> enabled
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

    // The engine core does NOT swallow a Char move any more — the gate is on the key, upstream.
    // `SetText` leaves the caret at the end, so one back-Char move puts it between 'b' and 'c'.
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

/// The XML attribute is the same flag under its other name (`ignoreArrows` occurs once in the
/// image, at `0x879b78`, with one xref: the attribute push at `0x77a13a`).
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
    // Non-numeric text → 0.
    s.run(r#"E:SetText("hello")"#).unwrap();
    assert_eq!(s.eval::<f64>("return E:GetNumber()").unwrap(), 0.0);
}

// ── the EditBox override never fires generic OnChar/OnKeyDown ─────────────────────────────────

#[test]
fn typing_fires_the_generic_on_char_with_what_was_inserted() {
    // **The half of the old law that was wrong** (corrected 2026-08-29; decision 1686).
    // `CSimpleEditBox` has its own input vtable which does not chain to the base — but Insert
    // itself fires the generic `OnChar` slot (`+0x180`) at `0x77c13c`, through the **varargs**
    // firer `0x7026f0` with fmt `"%s"` and the spliced string as the argument; the family's
    // other member is the fixed-arity firer `0x702690`.
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

    // `SetText` is NOT an insert path and fires nothing — the seam the reference has too.
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
    // The surviving half: the box's own key-down vtable (`0x77b160`) handles the event and never
    // chains to the base, so a focused box's typing does not also run a generic `OnKeyDown` bound
    // on that same box. Only the `OnChar` half of the original claim was corrected — this one
    // stands.
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

// ── history recall (`historyLines` / AddHistoryLine — decision 0288 P2) ──────────────────────

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
    // A typed char turns the recalled line into an ordinary draft: the next UP starts a FRESH
    // browse from the newest entry (stashing the edited line as the new draft).
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
    // Recall the newest, then rewrite the box the way the chat live parse does on a slash
    // recall ("/g two" → Guild + "two"). The browse walk must survive: the next UP lands on
    // the OLDER entry, not back on the newest (the "history only goes back 1" bug).
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
    // No historyLines: AddHistoryLine is a no-op, UP consumed but inert (a focused box eats every
    // key — both handlers `return 1` past the guard, `0x77a900`/`0x77b160`).
    s.run(r#"E:AddHistoryLine("x")"#).unwrap();
    assert!(s.editbox_action(EditAction::HistoryPrev));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "t");
}

// ── SetTextInsets (0288 P2 — the header-driven text-rect shrink) ─────────────────────────────

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
    // The getter echoes.
    let (l, r_, t, b) = s
        .eval::<(f32, f32, f32, f32)>("return E:GetTextInsets()")
        .unwrap();
    assert_eq!((l, r_, t, b), (15.0, 13.0, 2.0, 3.0));
}

// The FontInstance half an EditBox inherits in the client: SetTextColor tints the box's text
// region (`ChatEdit_UpdateHeader` colors the typed text this way — its absence killed the chat
// header chunk mid-run and left the /w insets stale, the caret-in-the-header bug).
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

/// Build a focused 200×32 box at BOTTOMLEFT(100,50), resolve it, type `text`, and answer its
/// advance table with a synthetic monospace 7 px/byte — the standard rig for the geometry tests.
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

/// The seam reports caret geometry from the advance table, and vanishes with focus or
/// visibility — the old prefix-string seam's laws, now in pixels.
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
    // The empty box still emits its Text quad (Some("")) — the caret's ride-along.
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
    // Drag back LEFT past the anchor to x=107 (byte 1): the selection flips to 1..3.
    s.mouse_move(107.0, 60.0);
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.selection, vec![(0, 7.0, 21.0)]);
    // Release ends the drag: further moves change nothing.
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

/// Copy needs a selection; cut copies then deletes; the password box yields the mask run, never
/// the real text (the client's fixed placeholder `0x882748`, mask stand-in).
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

/// The OS-native delete family (no 1.12 counterpart — the host keymap's Option/Cmd/Ctrl
/// Backspace-Delete chords): a word delete takes the adjacent alnum run (+ the separators toward
/// it), an edge delete clears to the line end — and both collapse to "delete the selection" when
/// one exists, like every deletion gesture.
#[test]
fn word_and_edge_deletes() {
    let mut s = script();
    seam_rig(&mut s, "abc def ghi");
    // Word-delete back from the end: "ghi" goes (boundary at 8).
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

    // Selection-first: with a live selection, a word delete removes exactly it.
    s.run(r#"E:SetText("hello world"); E:HighlightText(2, 5)"#)
        .unwrap();
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Word,
        back: true,
    });
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "he world");

    // Edge-delete back (macOS Cmd+Backspace): clears to the start — the whole line from the end.
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

/// SelectAll (the ref's Ctrl+A → `HighlightText(0, -1)` law): whole text selected, caret to the
/// end; typing replaces the lot.
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

/// The scroll window follows the cursor: typing past the box edge advances `display_from`
/// (whole chars); HOME snaps it back (`0x77da80`'s stay-visible invariant, the char-granular
/// `E+0x348` window).
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
    // HOME: the window snaps back to 0.
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    });
    let ui = s.focused_editbox_text_ui().unwrap();
    assert_eq!(ui.display_from, 0);
    assert_eq!(ui.caret_x, 0.0);
}

/// The blink law (`0x77a790`): 0.5 s half-period toggle, reset shown by every edit.
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
    // An edit shows the caret immediately (blink reset).
    s.char_input("b");
    answer_advances(&mut s);
    assert!(s.focused_editbox_text_ui().unwrap().caret_on);
}

// ── A hyperlink is ONE keypress (`AdvanceTokens 0x77bb30`, decision 1077) ─────────────────────
//
// The engine-level law lives in `markup`; these drive it the way a player does — through the
// focused box's public keyboard API — because that is the level the reported defect lived at:
// backspacing a shift-clicked item link chewed it one invisible byte at a time, and the byte that
// went first was the `r` of its trailing `|r`, which turned everything typed afterwards the item's
// colour.

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

    // And from the far side of following text: the link survives until the text is gone, then goes
    // whole — never half-eaten, which is what left a dangling `|c` colouring the rest of the line.
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
    // One RIGHT from the start lands past the entire link — so typing there appends, and does not
    // land between the `]` and its `|h`.
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
    // The same keystroke at the link's leading edge is allowed — the guard tests the PREVIOUS
    // token too, which is what keeps the boundary usable.
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
    // 43 bytes, but "[Ironfoe]" is 9 letters — comfortably under a 20-letter cap that a raw char
    // count would have blown through, trimming the link's own tail off.
    assert_eq!(text_of(&s), LINK);
    assert_eq!(s.eval::<i64>("return E:GetNumLetters()").unwrap(), 9);
}

/// **Creating a box is not showing it** — and that is what keeps the on-show self-focus safe now
/// that autoFocus defaults ON (decision 1686). A frame born visible is not an effective-visibility
/// *transition*, so it never runs the OnShow override; only a real `Show()` does. Without this,
/// loading the shipped chain would hand the keyboard to whichever edit box happened to be
/// constructed first, and typing would go into it instead of to the game.
///
/// Pinned because it is load-bearing by *absence*: nothing in the on-show path mentions creation,
/// so the day frame construction starts firing OnShow, this is the test that says what it costs.
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

/// **`SetMaxLetters` gates the COUNT and not the type** — one of four widget bindings in the whole
/// registrar that calls `lua_gettop` (`0x6f3070`), and its gate is exact (`cmp eax,2`), while the
/// value goes through a bare `lua_tonumber` (`0x6f3620`) with no `isnumber` guard.
///
/// That pairing is the opposite of the usual one, which is why it earns a test: benilla typed the
/// argument `i64` and so raised on `SetMaxLetters(nil)` — aux-addon's `gui/core.lua:288` writes
/// exactly that, and died at load on it — while accepting the wrong *number* of arguments
/// silently.
///
/// And `0` is **no limit**, not "no letters".
#[test]
fn set_max_letters_gates_the_argument_count_and_coerces_the_value() {
    let s = script();
    s.run(r#"E = CreateFrame("EditBox", "E") E:SetFocus()"#)
        .unwrap();
    let max = |s: &UiScript| s.eval::<i64>("return E:GetMaxLetters()").unwrap();

    s.run("E:SetMaxLetters(12)").unwrap();
    assert_eq!(max(&s), 12);

    // nil is 0 is UNLIMITED — the call completes, which is the whole point.
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

    // The COUNT is exact — too few AND too many both raise.
    assert!(s.run("E:SetMaxLetters()").is_err(), "too few raises");
    assert!(
        s.run("E:SetMaxLetters(50, 60)").is_err(),
        "too many raises too"
    );
}

/// **A `CSimpleEditBox` is born with FIVE regions, and `GetRegions` hands them to Lua before any
/// authored one.** The ctor builds the text FontString (`E+0x328`, `0x779bee`), three
/// selection-highlight `CSimpleTexture`s (`E+0x350/0x354/0x358`, loop `0x779c41`–`0x779c72`) and
/// the caret (`E+0x368`, `0x779c86`) — in that order. `GetRegions 0x773f60` walks
/// `[frame+0x1b8]`, one flat creation-ordered list, oldest first, no filter, and insertion is at
/// the TAIL. So the authored `<Layers>` regions start at index 6.
///
/// The report is pfUI's `skins/blizzard/friends.lua` l.379 —
/// `local _,_,_,_,_,left,right = GuildControlPopupFrameEditBox:GetRegions()` — which skips exactly
/// those five and takes the box's two border textures. It died on a nil `left` for as long as
/// benilla returned only the authored regions.
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

    // Five engine regions, then the two authored textures — and NOT a sixth from the embedded
    // `<FontString>`, which declares the ctor's object rather than adding one (the reference's
    // EditBox LoadXML, `0x779fb0`).
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

    // pfUI's own read, verbatim in shape: skip five, take the authored pair.
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

    // The first is the text FontString the box actually types into.
    s.run(r#"Box:SetText("typed")"#).unwrap();
    assert_eq!(
        s.eval::<String>("local t = Box:GetRegions() return t:GetText()")
            .unwrap(),
        "typed",
        "region 1 is the ctor's embedded text FontString"
    );
    // 2..5 are the three selection quads and the caret: real regions, carrying no art of their own
    // (benilla paints both host-side — the gap named on EditBoxState).
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

/// `SetNumber` is `SetText` with Lua's own `%.14g` in front of it — the two bindings are
/// byte-identical in the reference (`0x798690` / `0x7984c0`), so the numeric verb does no numeric
/// work of its own. Decision 1831; the expected strings are the reference's executed output.
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

    // A STRING is accepted and passed through verbatim — the gate is `lua_isstring`, so the
    // argument is never parsed as a number.
    s.run(r#"box:SetNumber("abc")"#).unwrap();
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "abc");

    // Everything else raises, including an ABSENT argument. `luaL_error` does not return.
    for bad in [
        "box:SetNumber()",
        "box:SetNumber(nil)",
        "box:SetNumber(true)",
    ] {
        assert!(s.run(bad).is_err(), "{bad} must raise");
    }
}

/// A `numeric` box takes the reference's wholesale abort: the clear-all has already run when the
/// insert fails the digit test, so the box is left EMPTY rather than partly filled. The money
/// boxes (`MoneyInputFrame.xml`) are numeric, which is why this matters — though the money path
/// itself only ever passes non-negative integers.
#[test]
fn set_number_on_a_numeric_box_empties_it_when_the_text_is_not_all_digits() {
    let s = UiScript::new().unwrap();
    s.run(r#"box = CreateFrame("EditBox", "NumOnly", UIParent) box:SetNumeric(true)"#)
        .unwrap();

    s.run("box:SetNumber(1234)").unwrap();
    assert_eq!(s.eval::<String>("return box:GetText()").unwrap(), "1234");

    // The minus sign fails the digit test, and the abort is wholesale.
    s.run("box:SetNumber(-5)").unwrap();
    assert_eq!(
        s.eval::<String>("return box:GetText()").unwrap(),
        "",
        "a sign empties a numeric box rather than partly filling it"
    );
}

/// `SetMaxBytes` (1960): the exact count gate and raw coerce of its sibling, a BYTE cap on the
/// buffer, and -1 as the no-limit sentinel where `SetMaxLetters` reads 0.
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

/// **`OnCursorChanged(x, y, w, h)` fires when the caret moves** — the edge the shipped
/// `MailFrame.xml` and `HelpFrame.xml` wire `ScrollingEdit_OnCursorChanged` to, and the reason a
/// multiline box follows its caret as you type past the bottom (decision 2141).
///
/// The four args are the reference's (`0x77dd5f`, each scaled by `f`): `x` the caret's
/// advance along its line, `y` **negative-downward** by row (which is what
/// `ScrollingEdit_OnCursorChanged`'s `cursorOffset = y` then `-this.cursorOffset` reads back as a
/// positive distance), `w` the constant `4.0`, `h` the line height.
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
    // Three bytes on row 0, then a wrap onto row 1 — the host answers the rows and the pitch.
    for ch in "abcdef".chars() {
        s.char_input(&ch.to_string());
    }
    s.resolve();
    if let Some(req) = s.editbox_advances_request() {
        // A plain monotonic 7 px/byte table (the rig's own), wrapped into two rows at byte 3 —
        // `caret_row_x` subtracts the row start's cumulative advance, which is the whole point.
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

    // **The edge is a CHANGE.** A tick that moves nothing fires nothing — which is what makes
    // `ScrollingEdit_OnUpdate`'s `if (this.cursorOffset)` loop terminate instead of re-scrolling
    // every frame.
    let before: i64 = s.eval("return table.getn(fires)").unwrap();
    s.tick(0.016);
    s.tick(0.016);
    assert_eq!(
        s.eval::<i64>("return table.getn(fires)").unwrap(),
        before,
        "two quiet ticks fire nothing"
    );

    // …and a caret move fires again.
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
