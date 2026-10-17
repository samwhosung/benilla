//! The wrap laws of the line kernel (`0x5c7470`) and the ellipsis truncate (`0x771ec0`) on the
//! client's real face: where a line breaks, what `nonspacewrap` leaves a force-broken line and what
//! `maxLines` cuts.

use super::*;
use crate::ui_text::engine::test_engine;

fn spec(non_space_wrap: bool, max_lines: u32) -> FontSpec<'static> {
    FontSpec {
        path: Some("Fonts\\FRIZQT__.TTF"),
        height: Some(12.0),
        outline: Outline::None,
        monochrome: false,
        spacing: 0.0,
        shadow_x: 0.0,
        alpha_gradient: None,
        non_space_wrap,
        max_lines,
    }
}

/// The rows of one source line at `width`, as the draw lays them.
fn rows(e: &mut TextEngine, text: &str, width: f32, non_space_wrap: bool) -> Vec<String> {
    let s = spec(non_space_wrap, 0);
    let r = measure::resolve(e, &s);
    e.ensure_metrics(r.face, r.ppem, text);
    let lines = fontstring_lines(text, [1.0; 4]);
    measure::wrap_rows(e, r, &lines, width, non_space_wrap)
        .into_iter()
        .map(|(row, _)| row.iter().map(|run| run.text.as_str()).collect())
        .collect()
}

/// Stock text that fills a line inside a word joined by `-`, `:` or `/`: the reference ends the
/// line after the joiner (`0x5c778e`-`0x5c77a8`), where a wrap at whitespace alone moved the
/// whole word or cut it mid-letter. The box is the prefix's own width and a little more, less
/// than the next letter.
#[test]
fn a_line_ends_after_the_joiner_where_it_fills() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    for (text, first) in [
        // `TUTORIAL20` and Raptor Mastery's description.
        (
            "the merchant pane is open, right-clicking an item",
            "the merchant pane is open, right-",
        ),
        (
            "So you think your hunting skills are in tip-top shape",
            "So you think your hunting skills are in tip-",
        ),
        ("Report to SI:7 agents at the docks", "Report to SI:"),
        ("Meet me at the inn/tavern tonight", "Meet me at the inn/"),
    ] {
        let own = measure_text(&mut e, first, None, spec(false, 0)).0;
        let got = rows(&mut e, text, own + 2.0, false);
        assert_eq!(got[0], first, "{text:?} in {own} px");
        assert!(got.len() > 1);
    }
}

/// `nonspacewrap` off, a fixed box with lines to spare holding a word wider than itself: the
/// fit walk stops at the force-broken line (`0x5c22e8`-`0x5c22f5`), so the text ends in "..."
/// inside the word, on the one line that fits it. On, the word wraps mid-letter and shows whole.
#[test]
fn the_ellipsis_cuts_inside_a_word_wider_than_the_box_unless_nonspacewrap() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let word = "Supercalifragilisticexpialidocious";
    // 60 wide and five lines of 12 tall: the word needs four rows by the wrap.
    assert!(rows(&mut e, word, 60.0, true).len() <= 5);
    assert_eq!(
        ellipsize_text(&mut e, word, 60.0, 60.0, spec(true, 0)),
        None
    );

    let cut = ellipsize_text(&mut e, word, 60.0, 60.0, spec(false, 0)).expect("cut");
    assert!(cut.ends_with("..."), "{cut:?}");
    assert!(cut.len() < word.len());
    // The longest prefix whose "..." still lays out on one line.
    let r = measure::resolve(&mut e, &spec(false, 0));
    assert_eq!(rows(&mut e, &cut, 60.0, false).len(), 1, "{cut:?}");
    let longer = format!("{}...", &word[..cut.len() - 2]);
    e.ensure_metrics(r.face, r.ppem, &longer);
    assert!(
        measure::fit_rows_for_test(&e, r, &longer, 60.0, 6, false) > 1,
        "{longer:?} would still fit"
    );

    // Words that break at their spaces are untouched by the flag.
    let words = "alpha beta gamma delta epsilon";
    for nsw in [false, true] {
        assert_eq!(
            ellipsize_text(&mut e, words, 60.0, 60.0, spec(nsw, 0)),
            None
        );
    }
}

/// Without `nonspacewrap`, the row after a force-broken one does not record its first break
/// opportunity (`0x5c75e4`-`0x5c75f5`): the "WW" left of the run goes on into the next word,
/// which it breaks mid-letter, where with the flag it ends at its space.
#[test]
fn a_force_broken_row_makes_the_next_row_lose_its_first_break() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let text = "WWWWWWWWWWWWWWWWWW abcdefghijklmnopqrstuvwxyz";
    let off = rows(&mut e, text, 60.0, false);
    let on = rows(&mut e, text, 60.0, true);
    assert_eq!(&on[..4], ["WWWW"; 4], "{on:?}");
    assert_eq!(on[4], "WW", "on, the run ends at its space: {on:?}");
    assert!(on[5].starts_with("abcd"), "{on:?}");
    assert_eq!(&off[..4], ["WWWW"; 4], "{off:?}");
    assert!(
        off[4].starts_with("WW ab") && off[4].len() > "WW ab".len(),
        "off, the run goes on into the word and cuts it: {off:?}"
    );
    // The same characters either way, bar the space dropped at the break.
    let joined = |rows: &[String]| rows.concat().replace(' ', "");
    assert_eq!(joined(&off), joined(&on));
}

/// `maxLines` fits the text into that many lines whatever the box holds (`0x771f48`-`0x771f61`):
/// the height is the line count times the pitch, so a taller box and no box give the same cut.
#[test]
fn maxlines_cuts_the_text_to_that_many_lines() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let text = "Two-Handed Weapon Specialization and a great many more words besides";
    for m in [1u32, 2, 3] {
        for box_h in [0.0, 12.0, 200.0] {
            let cut = ellipsize_text(&mut e, text, 103.0, box_h, spec(false, m))
                .unwrap_or_else(|| panic!("maxLines {m} cuts {text:?}"));
            assert!(cut.ends_with("..."), "{cut:?}");
            assert_eq!(
                rows(&mut e, &cut, 103.0, false).len(),
                m as usize,
                "maxLines {m}, box {box_h}: {cut:?}"
            );
        }
    }
    // Text that fits its lines is not cut.
    assert_eq!(
        ellipsize_text(&mut e, "Two-Handed", 103.0, 0.0, spec(false, 2)),
        None
    );
}

/// A glyph wider than the row ends the fit walk short of the text (`0x5c22c0`), so a box with
/// lines to spare still cuts: "i" fits 6 px, the "M" after it does not.
#[test]
fn a_glyph_wider_than_the_box_cuts_the_text_with_an_ellipsis() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let text = "i M cd";
    let cut = ellipsize_text(&mut e, text, 6.0, 120.0, spec(true, 0)).expect("cut");
    assert!(cut.ends_with("..."), "{cut:?}");
    // The same text in a box the "M" fits is whole.
    assert_eq!(
        ellipsize_text(&mut e, text, 60.0, 120.0, spec(true, 0)),
        None
    );
}

/// The EditBox's row enumerator records a row's start before it asks the kernel (`0x5c24a9`), so
/// the row no byte of which fits counts: "i" is row 0, the "M" the row that ends the layout.
#[test]
fn the_editbox_row_table_counts_the_row_that_bails() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let (rows, _) = line_rows(&mut e, "i M cd", 6.0, spec(true, 0));
    assert_eq!(rows, [0, 2]);
    let (rows, _) = line_rows(&mut e, "i M cd", 60.0, spec(true, 0));
    assert_eq!(rows, [0], "wide enough, one row");
}

/// A row that opens on a glyph with codes just before it starts past them: the break is recorded
/// at the glyph's cursor (`0x5c75f0`). "a-" fits, the "b" after the colour code does not.
#[test]
fn an_editbox_row_after_a_code_starts_past_it() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let width = measure_text(&mut e, "a-", None, spec(true, 0)).0 + 1.0;
    let (rows, _) = line_rows(&mut e, "a-|cffff0000bc", width, spec(true, 0));
    assert_eq!(rows[..2], [0, 12], "{rows:?}");
}

/// A text ending in LF gets its length as a last offset (`0x5c250b`) even when the wrap's skip
/// ate the LF, and after a bail.
#[test]
fn an_editbox_text_ending_in_lf_ends_its_table_with_its_length() {
    benilla_formats::wow_data_or_skip!();
    let mut e = test_engine(1.0).expect("the client faces open");
    let width = measure_text(&mut e, "ab", None, spec(true, 0)).0 + 1.0;
    let (rows, _) = line_rows(&mut e, "ab \n", width, spec(true, 0));
    assert_eq!(rows, [0, 4]);
    let (rows, _) = line_rows(&mut e, "i M\n", 6.0, spec(true, 0));
    assert_eq!(rows, [0, 2, 4], "the bailing row, then the length");
    // No LF, no extra offset: a trailing `|n` ends the walk with the last line.
    let (rows, _) = line_rows(&mut e, "ab|n", 60.0, spec(true, 0));
    assert_eq!(rows, [0]);
}
