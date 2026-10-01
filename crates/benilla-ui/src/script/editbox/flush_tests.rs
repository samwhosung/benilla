//! The box's flush (`0x77d3e0`) in its place in the tick's walk: after the box's own `OnUpdate`
//! (`0x77a790`), `OnCursorChanged` then `OnTextChanged`, on the dirty word `[E+0x31c]`.

use crate::script::{
    EditAction, EditBoxAdvanceRequest, EditUnit, MeasureRequest, TextMeasure, UiScript,
};

/// A stand-in font engine: each byte 7 wide and each line 14 tall, answering the advance table
/// inline as the host's engine does.
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
        Some((
            cum,
            vec![0],
            if req.wrap_width.is_some() { 14.0 } else { 0.0 },
        ))
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
