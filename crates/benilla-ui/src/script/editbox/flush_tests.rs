//! The box's flush (`0x77d3e0`) in its place in the tick's walk: after the box's own `OnUpdate`
//! (`0x77a790`), `OnCursorChanged` then `OnTextChanged`, on the dirty word `[E+0x31c]`.

use crate::script::{
    EditAction, EditBoxAdvanceRequest, EditUnit, MeasureRequest, QuadContent, TextMeasure, UiScript,
};

/// A stand-in font engine: each byte 7 wide and each line 14 tall, answering the advance table
/// inline as the host's engine does. A multi-line box's rows start after each newline, a trailing
/// one opening an empty row, as the reference's break routine adds one (`0x5c250b`–`0x5c2522`).
struct Mono;

impl TextMeasure for Mono {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let w = req.text.len() as f32 * 7.0;
        (w, 14.0, w)
    }

    fn editbox_advances(
        &mut self,
        req: &EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let cum = (0..=req.text.len()).map(|i| i as f32 * 7.0).collect();
        let Some(_) = req.wrap_width else {
            return Some((cum, vec![0], 0.0));
        };
        let breaks = req.text.match_indices('\n').map(|(i, _)| i + 1);
        let rows = std::iter::once(0).chain(breaks).collect();
        Some((cum, rows, 14.0))
    }
}

fn script() -> UiScript {
    let mut s = UiScript::new().expect("construct UiScript");
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(Mono));
    s
}

/// The `LOG` entries since the last call, joined by spaces.
fn take_log(s: &UiScript) -> String {
    s.eval::<String>("local l = table.concat(LOG, ' ') LOG = {} return l")
        .unwrap()
}

/// A 200×32 box `E` at `(100, 50)` with nothing focused, logging its `OnUpdate`, `OnCursorChanged`
/// (with `x`) and `OnTextChanged` (with the text) into `LOG`, between a frame made before it and
/// one made after, whose `OnUpdate`s log too; its first-show fires are drained.
fn logged_box(s: &mut UiScript) {
    s.run(
        r#"
        LOG = {}
        EARLY = CreateFrame("Frame", "Early")
        EARLY:SetScript("OnUpdate", function() table.insert(LOG, "early") end)
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(32)
        E:SetScript("OnUpdate", function() table.insert(LOG, "update") end)
        E:SetScript("OnCursorChanged", function() table.insert(LOG, "cursor@" .. arg1) end)
        E:SetScript("OnTextChanged", function() table.insert(LOG, "text:" .. this:GetText()) end)
        LATE = CreateFrame("Frame", "Late")
        LATE:SetScript("OnUpdate", function() table.insert(LOG, "late") end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.016);
    s.resolve();
    take_log(s);
}

/// The box's update fires its own `OnUpdate`, then the flush (`0x77a79a`, then `0x77a7a1`), whose
/// caret leg fires `OnCursorChanged` (`0x77d475` → `0x77de1b`) before `OnTextChanged` fires
/// (`0x77d498`); a frame walked after the box updates after all three.
#[test]
fn the_flush_follows_the_boxs_own_on_update_cursor_then_text() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("abc")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update cursor@21 text:abc late",
        "one tick, each box flushing in its place in the walk"
    );
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update late",
        "a quiet tick fires nothing"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The box's place in the walk is its stratum's, not its creation's: a BACKGROUND box updates and
/// flushes before the MEDIUM frames made around it (`0x7656c8`).
#[test]
fn a_box_flushes_after_its_on_update_in_its_strata_place() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetFrameStrata("BACKGROUND") E:SetText("abc")"#)
        .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "update cursor@21 text:abc early late");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// An edit a box makes in its own `OnUpdate` is flushed by the same update, so its events fire in
/// the same tick.
#[test]
fn an_edit_in_the_boxs_own_on_update_fires_its_events_the_same_tick() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnUpdate", function()
            table.insert(LOG, "update")
            if not EDITED then EDITED = true this:SetText("hello") end
        end)
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@35 text:hello late");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The ctor raises dirty bit 0 (`0x779a34`), so a new box fires `OnTextChanged` once at its first
/// flush, with its text unchanged; a hidden box is not walked, so its bit waits for the show.
#[test]
fn a_new_box_fires_on_text_changed_once_at_its_first_flush_once_shown() {
    let mut s = script();
    s.run(
        r#"
        SHOWN, HIDDEN = 0, 0
        local a = CreateFrame("EditBox", "A")
        a:SetAutoFocus(false)
        a:SetScript("OnTextChanged", function() SHOWN = SHOWN + 1 end)
        local b = CreateFrame("EditBox", "B")
        b:SetAutoFocus(false)
        b:Hide()
        b:SetScript("OnTextChanged", function() HIDDEN = HIDDEN + 1 end)
    "#,
    )
    .unwrap();
    let counts = |s: &UiScript| s.eval::<(i64, i64)>("return SHOWN, HIDDEN").unwrap();
    s.tick(0.016);
    assert_eq!(counts(&s), (1, 0), "the shown box's first flush");
    s.tick(0.016);
    assert_eq!(counts(&s), (1, 0), "once");
    s.run("B:Show()").unwrap();
    s.tick(0.016);
    assert_eq!(
        counts(&s),
        (1, 1),
        "the hidden box's bit waited for the show"
    );
    s.tick(0.016);
    assert_eq!(counts(&s), (1, 1));
    assert_eq!(s.eval::<String>("return B:GetText()").unwrap(), "");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Every run of the text seat `0x77b8c0` on a box with a rect ends in `or eax,7` (`0x77ba7f`), so
/// its next flush fires `OnTextChanged` once, with the text unchanged: from a resize (the
/// `OnSizeChanged` override, `0x77a8e7`), `SetTextInsets` (`0x77a707`), `GetTextInsets`
/// (`0x77a73f`), `SetMultiLine` (`0x77a63f`) and a font change (`0x77e2b5`).
#[test]
fn each_re_seat_of_the_text_fires_one_on_text_changed_at_the_next_flush() {
    let mut s = script();
    s.run(
        r#"
        N = 0
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(32)
        E:SetText("same")
        E:SetScript("OnTextChanged", function() N = N + 1 end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.016);
    let mut fires = Vec::new();
    for (what, lua, resolve) in [
        ("a resize", "E:SetWidth(250)", true),
        ("SetTextInsets", "E:SetTextInsets(4, 4, 2, 2)", false),
        ("GetTextInsets", "E:GetTextInsets()", false),
        (
            "a font change",
            r#"E:SetFont("Fonts\\FRIZQT__.TTF", 14)"#,
            false,
        ),
        ("SetMultiLine", "E:SetMultiLine(true)", false),
    ] {
        s.run(&format!("N = 0 {lua}")).unwrap();
        if resolve {
            s.resolve();
        }
        s.tick(0.016);
        fires.push((what, s.eval::<i64>("return N").unwrap()));
    }
    assert_eq!(
        fires,
        [
            ("a resize", 1),
            ("SetTextInsets", 1),
            ("GetTextInsets", 1),
            ("a font change", 1),
            ("SetMultiLine", 1),
        ]
    );
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "same");

    // A box with no rect yet returns before the seat (`0x77b8c7`) and raises nothing.
    s.run(
        r#"
        M = 0
        F = CreateFrame("EditBox", "F")
        F:SetAutoFocus(false)
        F:SetScript("OnTextChanged", function() M = M + 1 end)
    "#,
    )
    .unwrap();
    s.tick(0.016);
    s.run("F:SetTextInsets(4, 4, 2, 2)").unwrap();
    s.tick(0.016);
    assert_eq!(
        s.eval::<i64>("return M").unwrap(),
        1,
        "the ctor's fire alone"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A handler's edit raises the bits the flush has already cleared (`0x77d44c`, `0x77d47a`), so an
/// `OnTextChanged` that sets its own text fires once more, at the next frame's flush, and never
/// loops inside one; `SetText` of an equal text raises nothing (`0x77be4b`), which ends it.
#[test]
fn an_on_text_changed_that_sets_its_own_text_fires_once_more_next_frame() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnTextChanged", function()
            table.insert(LOG, "text:" .. this:GetText())
            this:SetText(strupper(this:GetText()))
        end)
        E:SetText("abc")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@21 text:abc late");
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@21 text:ABC late");
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update late");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Bit 2 clears after the caret leg (`0x77d47a`), so a caret an `OnCursorChanged` handler moves is
/// not reported again; the text it inserts keeps bit 0 for the next frame.
#[test]
fn a_caret_moved_by_on_cursor_changed_is_cleared_with_the_bit() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function()
            table.insert(LOG, "cursor@" .. arg1)
            if not EDITED then EDITED = true this:Insert("!") end
        end)
        E:SetText("ab")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@14 text:ab! late");
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update text:ab! late",
        "the insert's text bit, without its caret bit"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The focus transition raises bit 2 on both boxes (`0x77af50`, `0x77afa8`): each fires
/// `OnCursorChanged` at its next flush, the one that lost the keyboard included.
#[test]
fn a_focus_change_runs_the_caret_leg_of_both_boxes() {
    let mut s = script();
    s.run(
        r#"
        LOG = {}
        for _, name in ipairs({ "A", "B" }) do
            local f = CreateFrame("EditBox", name)
            f:SetAutoFocus(false)
            f:SetScript("OnCursorChanged", function() table.insert(LOG, this:GetName()) end)
        end
    "#,
    )
    .unwrap();
    s.tick(0.016);
    take_log(&s);
    s.run("A:SetFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "A");
    s.run("B:SetFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "A B");
    s.run("B:ClearFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "B");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The move helpers step the caret only while it can (`0x77c750` `cursor < len`, `0x77c870`
/// `cursor > 0`), and only the step raises bit 2 (`0x77c73b`): Right at the end and Home at 0 fire
/// nothing.
#[test]
fn a_caret_move_that_goes_nowhere_raises_nothing() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("ab") E:SetFocus()"#).unwrap();
    s.tick(0.016);
    take_log(&s);
    let mut logs = Vec::new();
    for (what, unit, back) in [
        ("Right at the end", EditUnit::Char, false),
        ("Home from the end", EditUnit::Edge, true),
        ("Home at 0", EditUnit::Edge, true),
    ] {
        assert!(s.editbox_action(EditAction::Move {
            unit,
            back,
            extend: false,
        }));
        s.tick(0.016);
        logs.push((what, take_log(&s)));
    }
    assert_eq!(
        logs,
        [
            ("Right at the end", "early update late".to_string()),
            (
                "Home from the end",
                "early update cursor@0 late".to_string()
            ),
            ("Home at 0", "early update late".to_string()),
        ]
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnKeyDown` flushes the box before it handles the key (`0x77b1e2`), and a char's key-down comes
/// before it: keys typed within one frame fire their events per key, ahead of the next key's own
/// script, so `OnEnterPressed` sees the `OnTextChanged` of the text it ends.
#[test]
fn a_key_flushes_the_box_before_it_acts() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnEnterPressed", function() table.insert(LOG, "enter") end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    take_log(&s);
    assert!(s.char_input("a"));
    assert!(s.char_input("b"));
    assert!(s.key_input("ENTER"));
    assert_eq!(
        take_log(&s),
        "cursor@7 text:a cursor@14 text:ab enter",
        "one frame, no tick between the keys"
    );
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update late",
        "nothing left for the walk"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `HighlightText` writes only the selection (`0x77cca0`), and Ctrl+A is that call (`0x77b239`):
/// the caret bit stays down, so no `OnCursorChanged` fires at the flush.
#[test]
fn highlight_text_and_ctrl_a_fire_no_on_cursor_changed() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("hello") E:SetFocus()"#).unwrap();
    s.tick(0.016);
    take_log(&s);
    assert!(s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    }));
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@0 late");
    s.run("E:HighlightText()").unwrap();
    s.tick(0.016);
    assert!(s.editbox_action(EditAction::SelectAll));
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update late early update late",
        "the caret stayed at 0"
    );
    s.char_input("X");
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "X",
        "and the selection is all of it"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A focus change raises bit 2 (`0x77afa8`), and the caret leg restarts the blink (`0x77dd68`)
/// and shows the caret in the focused box (`0x77de35`): a box that takes the keyboard back starts
/// solid, whatever phase it lost it in.
#[test]
fn taking_the_keyboard_restarts_the_caret_blink() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("ab") E:SetFocus()"#).unwrap();
    s.tick(0.016);
    s.tick(0.6);
    s.resolve();
    assert!(
        !s.focused_editbox_text_ui().unwrap().caret_on,
        "past the half-period: off"
    );
    s.run("E:ClearFocus()").unwrap();
    s.tick(0.3);
    s.run("E:SetFocus()").unwrap();
    s.tick(0.016);
    assert!(
        s.focused_editbox_text_ui().unwrap().caret_on,
        "the focus's flush shows it"
    );
    s.tick(0.45);
    assert!(
        s.focused_editbox_text_ui().unwrap().caret_on,
        "with a whole half-period ahead"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// UP and DOWN in a multi-line box move a row (`0x77cb20`), and `OnCursorChanged` reports it a
/// row pitch down (`0x77db31`); a single-line box would recall history instead.
#[test]
fn up_and_down_move_a_multi_line_boxs_caret_by_rows() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function()
            table.insert(LOG, format("cursor@%d,%d", arg1, arg2))
        end)
        E:SetMultiLine(true)
        E:SetText("abc\ndefg")
        E:SetFocus()
    "#,
    )
    .unwrap();
    // The flush sizes the box to its two rows; the drain after the walk resizes it, and the
    // re-seat that runs fires once more at the next flush (`0x77a8d0` → `0x77b8c0`).
    s.tick(0.016);
    s.tick(0.016);
    take_log(&s);
    let row = |back| EditAction::Move {
        unit: EditUnit::Row,
        back,
        extend: false,
    };
    assert!(s.editbox_action(row(true)));
    s.tick(0.016);
    assert!(s.editbox_action(row(false)));
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update cursor@21,0 late early update cursor@21,-14 late",
        "column 4 holds before the newline above, then comes back down to column 3"
    );
    s.char_input("|");
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "abc\ndef||g"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnCursorChanged`'s `y` starts at 0 and loses one row pitch per row above the caret
/// (`0x77dae1`, `0x77db31`): a single-line box, and a multi-line box's first row, report +0, which
/// Lua prints as "0", never "-0".
#[test]
fn on_cursor_changed_reports_the_first_row_at_plus_zero() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function() table.insert(LOG, tostring(arg2)) end)
        E:SetText("ab")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update 0 text:ab late", "single-line");
    s.run(r#"E:SetMultiLine(true) E:SetText("a\nb")"#).unwrap();
    s.tick(0.016);
    s.run(r#"E:SetText("c")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update -14 text:a\nb late early update 0 text:c late",
        "multi-line, the second row and then the first"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnCursorChanged`'s `h` is the caret's height, which the text seat sets to one line of the
/// box's font (`0x77b970` → `0x77ba76`) whether or not the box is multi-line, and its `w` the
/// constant 4 (`0x77ba67`).
#[test]
fn on_cursor_changed_reports_one_line_tall_in_a_single_line_box() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function()
            table.insert(LOG, format("cursor@%g,%g,%g,%g", arg1, arg2, arg3, arg4))
        end)
        E:SetText("ab")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@14,0,4,14 text:ab late");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// 40 distinct characters, 280 wide in [`Mono`].
const ALPHA40: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCD";

/// A 200×32 single-line box `W` at `(100, 50)` holding [`ALPHA40`], nothing focused, flushed once.
fn long_box(s: &mut UiScript) {
    s.run(&format!(
        r#"
        W = CreateFrame("EditBox", "W")
        W:SetAutoFocus(false)
        W:SetPoint("BOTTOMLEFT", 100, 50)
        W:SetWidth(200); W:SetHeight(32)
        W:SetText("{ALPHA40}")
    "#
    ))
    .unwrap();
    s.resolve();
    s.tick(0.016);
    s.resolve();
}

/// What the box's text region holds.
fn window(s: &UiScript) -> String {
    s.eval("return (W:GetRegions()):GetText()").unwrap()
}

/// Every shown box runs its flush, focused or not, and the relayout hands its text region only
/// the window (`0x77d840`–`0x77d858`): the 28 characters that fit 200 from the start, re-windowed
/// once the caret at the end has left them, to the 21 characters that fit three quarters of the
/// width back from it (`0x77d7df` → `0x77d320`). The region is the inset box, one `TOPLEFT` point
/// and an explicit size (`0x77b905`, `0x77b940`, `0x77b96b`), and its quad draws as one line.
#[test]
fn an_unfocused_box_holds_the_window_its_caret_left() {
    let mut s = script();
    long_box(&mut s);
    assert_eq!(window(&s), &ALPHA40[19..]);
    crate::assert_near!(
        s.eval::<(f64, f64, i64)>(
            "local t = W:GetRegions() return t:GetWidth(), t:GetHeight(), t:GetNumPoints()"
        )
        .unwrap(),
        (200.0, 32.0, 1)
    );
    let line = s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Text {
            text: Some(t),
            edit_line,
            ..
        } if t == ALPHA40[19..] => Some(edit_line),
        _ => None,
    });
    assert_eq!(line, Some(true), "the window's quad draws as an edit line");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The window moves only when the caret leaves it (`0x77d3f5`–`0x77d436` promote the caret's bit
/// to the relayout's): leaving by the left starts it a quarter of the width back from the caret
/// (`0x77d7d4`), and losing the focus leaves it where it is.
#[test]
fn the_window_moves_only_when_the_caret_leaves_it() {
    let mut s = script();
    long_box(&mut s);
    s.run("W:SetFocus()").unwrap();
    let left = EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false,
    };
    for _ in 0..21 {
        s.editbox_action(left);
    }
    s.tick(0.016);
    assert_eq!(window(&s), &ALPHA40[19..], "the caret at its left edge");
    s.editbox_action(left);
    s.tick(0.016);
    assert_eq!(
        window(&s),
        &ALPHA40[11..39],
        "7 back from the caret, then 28"
    );
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    });
    s.tick(0.016);
    assert_eq!(window(&s), &ALPHA40[..28]);
    s.run("W:ClearFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(window(&s), &ALPHA40[..28], "the window outlives the focus");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnCursorChanged`'s `x` counts from the caret's line's first byte in the line table
/// (`0x77dcd8`–`0x77dcfc`), in a single-line box the window's (`0x77d750`): a scrolled box reports
/// its caret inside its width.
#[test]
fn on_cursor_changed_measures_x_from_the_windows_start() {
    let mut s = script();
    long_box(&mut s);
    s.run(
        r#"
        LOG = {}
        W:SetScript("OnCursorChanged", function() table.insert(LOG, "cursor@" .. arg1) end)
        W:SetFocus()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Char,
        back: true,
        extend: false,
    });
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "cursor@147 cursor@140",
        "21 then 20 characters past the window's start, at 19"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The caret is tested against the window's start as it stands (`0x77d7b4`–`0x77d7c2`): a text
/// that shrinks below a scrolled start re-windows around the caret, never to an empty window.
#[test]
fn a_shorter_text_re_windows_a_scrolled_box() {
    let mut s = script();
    long_box(&mut s);
    s.run(r#"W:SetText("abc")"#).unwrap();
    s.tick(0.016);
    assert_eq!(window(&s), "abc");
}

/// A password box never re-windows (`0x77d769` skips both re-window calls): its window is the mask
/// that fits from its start, one `*` per byte of the text (`0x77d4fc`, `0x77d533`), and a caret
/// past it fires no `OnCursorChanged` (`0x77da97` → `0x77de49`) until it comes back.
#[test]
fn a_password_box_keeps_its_window_at_the_start() {
    let mut s = script();
    s.run(&format!(
        r#"
        LOG = {{}}
        W = CreateFrame("EditBox", "W")
        W:SetAutoFocus(false)
        W:SetPoint("BOTTOMLEFT", 100, 50)
        W:SetWidth(200); W:SetHeight(32)
        W:SetPassword(true)
        W:SetScript("OnCursorChanged", function() table.insert(LOG, "cursor@" .. arg1) end)
        W:SetText("{ALPHA40}")
    "#
    ))
    .unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(window(&s), "*".repeat(28));
    s.run("W:SetFocus()").unwrap();
    s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    });
    s.tick(0.016);
    assert_eq!(take_log(&s), "cursor@0", "only the caret inside the window");

    s.run(r"W:SetText('a\195\169')").unwrap();
    s.tick(0.016);
    assert_eq!(window(&s), "***", "a two-byte letter masks as two");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A window inside a colour run keeps its colour (`0x77d7f8`–`0x77d93c`): an unescaped `|r` met
/// before any `|c` puts the run's `|cAARRGGBB` before the window.
#[test]
fn a_window_inside_a_colour_run_carries_its_colour() {
    let mut s = script();
    long_box(&mut s);
    s.run(&format!(r#"W:SetText("|cff00ff00{ALPHA40}|r")"#))
        .unwrap();
    s.tick(0.016);
    assert_eq!(window(&s), format!("|cff00ff00{}|r", &ALPHA40[21..]));
}

/// `OnCursorChanged`'s `x` follows justifyH: CENTER is the advance from the line's start less half
/// the line (`0x77dc1d`–`0x77dc7b`), RIGHT minus the advance to the line's end (`0x77db89`–
/// `0x77dbb0`), 0 with the caret at the end.
#[test]
fn on_cursor_changed_x_follows_justify_h() {
    for (justify, want) in [
        ("CENTER", "cursor@14 cursor@7"),
        ("RIGHT", "cursor@0 cursor@-7"),
    ] {
        let mut s = script();
        logged_box(&mut s);
        s.run(&format!(
            r#"
            E:SetJustifyH("{justify}")
            E:SetScript("OnCursorChanged", function() table.insert(LOG, "cursor@" .. arg1) end)
            E:SetText("abcd")
            E:SetFocus()
        "#
        ))
        .unwrap();
        s.tick(0.016);
        s.editbox_action(EditAction::Move {
            unit: EditUnit::Char,
            back: true,
            extend: false,
        });
        s.tick(0.016);
        let log = take_log(&s);
        let cursors: Vec<&str> = log
            .split(' ')
            .filter(|e| e.starts_with("cursor@"))
            .collect();
        assert_eq!(cursors.join(" "), want, "{justify}");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}

/// [`Mono`] as the host answers a scaled box: glyphs 7 and lines 14 in the box's own units, the
/// table in root units (measured at the box's scale, divided by the seam alone).
struct ScaledMono;

impl TextMeasure for ScaledMono {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        Mono.measure(req)
    }

    fn editbox_advances(
        &mut self,
        req: &EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let (cum, rows, pitch) = Mono.editbox_advances(req)?;
        let k = req.scale;
        Some((cum.into_iter().map(|a| a * k).collect(), rows, pitch * k))
    }
}

/// `OnCursorChanged` answers in the box's own units: the reference divides by its effective scale
/// (`0x77db23 fdiv [esi+0x7c]`), so a half-scale box reports what a full-scale one does.
#[test]
fn on_cursor_changed_answers_in_the_boxs_own_units() {
    let mut s = script();
    s.set_text_measurer(Box::new(ScaledMono));
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScale(0.5)
        E:SetScript("OnCursorChanged", function()
            table.insert(LOG, format("cursor@%g,%g,%g", arg1, arg2, arg4))
        end)
        E:SetMultiLine(true)
        E:SetText("abc\nd")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update cursor@7,-14,14 text:abc\nd late",
        "one glyph along the second row, a row pitch down, one pitch tall"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A box created and anchored between frames, as an event handler does: the pump's drain before
/// the walk (`0x7656ba`) gives it its first rect, whose resize runs the box's `OnSizeChanged`
/// override (`0x76b63e` → `0x77a8d0`); the re-seat ORs its bits into the ctor's (`0x77ba7f`), so the
/// walk's flush (`0x77a7a1`) fires `OnTextChanged` once, and the frames after it fire nothing.
#[test]
fn a_box_anchored_between_frames_fires_on_text_changed_once() {
    let mut s = script();
    s.run(
        r#"
        N = 0
        local e = CreateFrame("EditBox", "Fresh")
        e:SetAutoFocus(false)
        e:SetPoint("BOTTOMLEFT", 100, 50)
        e:SetWidth(200); e:SetHeight(32)
        e:SetScript("OnTextChanged", function() N = N + 1 end)
    "#,
    )
    .unwrap();
    // The host's frame: the tick, then its own resolve for the draw.
    for _ in 0..3 {
        s.tick(0.016);
        s.resolve();
    }
    assert_eq!(s.eval::<i64>("return N").unwrap(), 1);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
