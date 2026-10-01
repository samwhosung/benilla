//! The caret and selection law of [`EditBoxState`]: the row, line and edge moves, the Shift
//! extension (`0x77cd10`), `HighlightText` (`0x77cca0`) and the `numeric` insert (`0x77bee0`).

use super::*;

/// A multi-line box holding `text`, measured at 7 px per byte with its rows at `rows`.
fn multi_line(text: &str, rows: Vec<usize>) -> EditBoxState {
    EditBoxState {
        text: text.into(),
        multi_line: true,
        advances: (0..=text.len()).map(|i| i as f32 * 7.0).collect(),
        rows,
        cell_h: 14.0,
        ..EditBoxState::default()
    }
}

/// A single-line box holding `text` with the caret at `cursor` and nothing selected.
fn at(text: &str, cursor: usize) -> EditBoxState {
    let mut eb = EditBoxState {
        text: text.into(),
        cursor,
        ..EditBoxState::default()
    };
    eb.collapse();
    eb
}

fn mv(eb: &mut EditBoxState, unit: EditUnit, back: bool, extend: bool) {
    eb.apply(EditAction::Move { unit, back, extend });
}

/// UP/DOWN keep the caret's letter column across rows (`0x77bc80` then `0x77bb30`), and a walk
/// that reaches the next row's start steps back one (`0x77cbc6`–`0x77cbdd`), before the newline
/// that ends a row. On the last row it may end at the text's end (the declared deviation).
#[test]
fn up_and_down_keep_the_letter_column_across_rows() {
    // "abc\n" | "defgh\n" | "ij"
    let mut eb = multi_line("abc\ndefgh\nij", vec![0, 4, 10]);
    eb.cursor = 8; // "defg|h"
    eb.collapse();
    mv(&mut eb, EditUnit::Row, true, false);
    assert_eq!(
        eb.cursor, 3,
        "column 4 is past \"abc\": held before its newline"
    );
    mv(&mut eb, EditUnit::Row, false, false);
    assert_eq!(eb.cursor, 7, "column 3 on the middle row");
    mv(&mut eb, EditUnit::Row, false, false);
    assert_eq!(eb.cursor, 12, "column 3 on \"ij\": the text's end");
    assert_eq!(
        eb.dirty & EditBoxState::DIRTY_CURSOR,
        EditBoxState::DIRTY_CURSOR
    );
}

/// A soft-wrapped row is a row: its swallowed space belongs to it, so a column past its letters
/// holds before that space.
#[test]
fn a_wrapped_row_is_a_row() {
    // "ab " | "cdefgh", wrapped after "ab" with the space swallowed.
    let mut eb = multi_line("ab cdefgh", vec![0, 3]);
    eb.cursor = 9;
    eb.collapse();
    mv(&mut eb, EditUnit::Row, true, false);
    assert_eq!(
        eb.cursor, 2,
        "column 6 is past \"ab\": held before the space"
    );
    mv(&mut eb, EditUnit::Row, false, false);
    assert_eq!(eb.cursor, 5, "column 2 below");
    mv(&mut eb, EditUnit::Row, true, false);
    assert_eq!(eb.cursor, 2);
}

/// A trailing newline opens a row of its own (`0x5c250b`–`0x5c2522`), empty and starting at the
/// text's end: DOWN reaches it, and UP leaves it for column 0 above.
#[test]
fn a_trailing_newline_is_a_row_of_its_own() {
    let mut eb = multi_line("ab\n", vec![0, 3]);
    eb.cursor = 2;
    eb.collapse();
    mv(&mut eb, EditUnit::Row, false, false);
    assert_eq!(eb.cursor, 3);
    mv(&mut eb, EditUnit::Row, true, false);
    assert_eq!(eb.cursor, 0);
}

/// On the first or last row the caret stays, and a plain UP or DOWN clears the selection there
/// (`0x77cb50`–`0x77cb8f`) without raising the caret bit.
#[test]
fn up_on_the_first_row_moves_nothing_and_clears_the_selection() {
    let mut eb = multi_line("abc\ndef", vec![0, 4]);
    eb.cursor = 2;
    eb.highlight_text(0, 3);
    eb.dirty = 0;
    mv(&mut eb, EditUnit::Row, true, true);
    assert_eq!(
        (eb.cursor, eb.sel_start, eb.sel_end),
        (2, 0, 3),
        "Shift keeps it"
    );
    mv(&mut eb, EditUnit::Row, true, false);
    assert_eq!(eb.cursor, 2);
    assert_eq!(eb.sel_start, eb.sel_end, "the selection cleared");
    assert_eq!(
        eb.dirty & EditBoxState::DIRTY_CURSOR,
        0,
        "the caret did not move"
    );
}

/// Shift+DOWN extends by the row's whole delta through `0x77cd10`.
#[test]
fn shift_down_selects_to_the_same_column_below() {
    let mut eb = multi_line("abc\ndef", vec![0, 4]);
    eb.cursor = 1;
    eb.collapse();
    mv(&mut eb, EditUnit::Row, false, true);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (5, 1, 5));
    mv(&mut eb, EditUnit::Row, true, true);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (1, 1, 1));
}

/// A single-line box's UP and DOWN are its history's, which the caller recalls; the caret stays.
#[test]
fn a_single_line_box_has_no_rows_to_move_over() {
    let mut eb = at("abc", 1);
    mv(&mut eb, EditUnit::Row, false, false);
    assert_eq!(eb.cursor, 1);
}

/// HOME and END stop at a newline (`0x77c99a`, `0x77ca0e`); the text's edges are Ctrl+HOME and
/// Ctrl+END (`0x77ca60`, `0x77cac0`).
#[test]
fn home_and_end_stop_at_the_lines_newlines() {
    let mut eb = multi_line("ab\ncd\nef", vec![0, 3, 6]);
    eb.cursor = 4; // "c|d"
    eb.collapse();
    mv(&mut eb, EditUnit::Line, true, false);
    assert_eq!(eb.cursor, 3, "HOME: after the newline before it");
    mv(&mut eb, EditUnit::Line, true, false);
    assert_eq!(eb.cursor, 3, "HOME again stays");
    mv(&mut eb, EditUnit::Line, false, true);
    assert_eq!(
        (eb.cursor, eb.sel_start, eb.sel_end),
        (5, 3, 5),
        "Shift+END"
    );
    mv(&mut eb, EditUnit::Edge, true, false);
    assert_eq!(
        (eb.cursor, eb.sel_start, eb.sel_end),
        (0, 0, 0),
        "Ctrl+HOME"
    );
    mv(&mut eb, EditUnit::Edge, false, false);
    assert_eq!(eb.cursor, 8, "Ctrl+END");
    // A line is not a wrapped row: with no newline, HOME goes to the text's start.
    let mut eb = multi_line("the quick brown", vec![0, 10]);
    eb.cursor = 13;
    eb.collapse();
    mv(&mut eb, EditUnit::Line, true, false);
    assert_eq!(eb.cursor, 0);
}

/// `HighlightText` writes only the selection (`0x77cca0`): the caret stays put and its bit stays
/// down, so neither the blink nor `OnCursorChanged` hears of it.
#[test]
fn highlight_text_leaves_the_caret_where_it_is() {
    let mut eb = at("hello world", 3);
    eb.dirty = 0;
    eb.caret_shown = false;
    eb.highlight_text(0, -1);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (3, 0, 11));
    assert_eq!(eb.dirty & EditBoxState::DIRTY_CURSOR, 0);
    assert!(!eb.caret_shown, "the blink runs on");
    eb.apply(EditAction::SelectAll);
    assert_eq!(eb.cursor, 3, "Ctrl+A is the same call");
}

/// Shift+arrow moves one selection edge by the step (`0x77cd10`): from a caret inside a select-all,
/// Shift+Right trims the start up to the caret and Shift+Left trims the end down to it.
#[test]
fn shift_arrow_from_inside_a_selection_trims_it_toward_the_caret() {
    let mut eb = at("hello world", 5);
    eb.highlight_text(0, -1);
    mv(&mut eb, EditUnit::Char, false, true);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (6, 6, 11));
    let mut eb = at("hello world", 5);
    eb.highlight_text(0, -1);
    mv(&mut eb, EditUnit::Char, true, true);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (4, 0, 4));
    // Back at the selection's end, Shift+Right extends it again.
    mv(&mut eb, EditUnit::Char, false, true);
    assert_eq!((eb.sel_start, eb.sel_end), (0, 5));
}

/// The word, line and edge moves are loops of single steps (`0x77c6b0`), each extending through
/// `0x77cd10`: from inside a selection, Shift+END trims it to nothing at its end, then selects on
/// from there.
#[test]
fn a_multi_step_shift_move_extends_step_by_step() {
    let mut eb = at("0123456789", 5);
    eb.highlight_text(2, 8);
    mv(&mut eb, EditUnit::Line, false, true);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (10, 8, 10));
}

/// Left at the text's start does not step (`0x77c870` tests `cursor > 0`), so neither the caret
/// bit nor the blink's restart comes; a real step brings both.
#[test]
fn a_char_move_that_goes_nowhere_leaves_the_blink() {
    let mut eb = at("ab", 0);
    eb.dirty = 0;
    eb.caret_shown = false;
    eb.blink_accum = 0.3;
    mv(&mut eb, EditUnit::Char, true, false);
    assert_eq!((eb.cursor, eb.dirty, eb.caret_shown), (0, 0, false));
    assert_eq!(eb.blink_accum, 0.3);
    mv(&mut eb, EditUnit::Char, false, false);
    assert_eq!(eb.cursor, 1);
    assert_eq!(eb.dirty, EditBoxState::DIRTY_CURSOR);
    assert!(eb.caret_shown && eb.blink_accum == 0.0);
}

/// A plain Left or Right with a selection collapses it to that edge (the declared deviation).
#[test]
fn a_plain_arrow_collapses_a_selection_to_its_edge() {
    let mut eb = at("hello world", 5);
    eb.highlight_text(2, 8);
    mv(&mut eb, EditUnit::Char, false, false);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (8, 8, 8));
    let mut eb = at("hello world", 5);
    eb.highlight_text(2, 8);
    mv(&mut eb, EditUnit::Char, true, false);
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (2, 2, 2));
}

/// `Insert` deletes the selection before the `numeric` test refuses a non-digit (`0x77bf13`, then
/// `0x77bf41`): the text changes, though nothing went in.
#[test]
fn a_non_digit_over_a_numeric_selection_deletes_it() {
    let mut eb = EditBoxState {
        numeric: true,
        ..at("1234", 4)
    };
    eb.highlight_text(1, 3);
    eb.dirty = 0;
    let out = eb.insert("a");
    assert_eq!(eb.text, "14");
    assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (1, 1, 1));
    assert!(out.text_changed && !out.inserted, "{out:?}");
    assert_eq!(
        eb.dirty & EditBoxState::DIRTY_TEXT,
        EditBoxState::DIRTY_TEXT
    );
    // With nothing selected the refusal changes nothing.
    eb.dirty = 0;
    assert_eq!(eb.insert("b"), EditOutcome::default());
    assert_eq!((eb.text.as_str(), eb.dirty), ("14", 0));
}
