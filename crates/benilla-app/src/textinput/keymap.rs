//! The per-OS text-editing keymap: one pure table from a physical keypress + modifier snapshot
//! to what it *means* for a focused EditBox — an engine [`EditAction`], or one of the three
//! clipboard operations (kept host-side: they need the OS pasteboard). The engine owns what each
//! action *does* (the byte-verified box law); this module owns only which chord means which
//! action on which OS — the director's "everything OS-native" call (decision 0301).
//!
//! The Windows/Linux table doubles as the reference law where the 1.12 client had a chord at all
//! (the key handler `0x77b160`: Ctrl+arrows word-granular, Ctrl+A/C/X/V, the Ctrl/Shift+Insert +
//! Shift+Delete CUA mirrors). Ctrl+Backspace/Delete word deletes are modern-OS additions with no
//! 1.12 counterpart; the whole macOS table is the platform's native law (Cmd/Option families),
//! not the (Windows) reference's.
//!
//! One rule spans both of those platforms and is easy to miss: **AltGr is not Ctrl**. Windows and
//! Linux both deliver AltGr as Ctrl+Alt, and European layouts type real letters with it, so the
//! Ctrl letter chords all exclude it (decision 0702) — see [`chord_pc`].

use benilla_ui::script::{EditAction, EditUnit};
use bevy::input::keyboard::KeyCode;

/// The modifier snapshot a chord is read against. `sup` is the Super family — Cmd on macOS, the
/// OS key elsewhere.
#[derive(Clone, Copy, Default)]
pub(crate) struct Mods {
    pub(crate) shift: bool,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) sup: bool,
}

/// What a keypress means for the focused box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Chord {
    /// A semantic edit — hand to `UiScript::editbox_action`.
    Edit(EditAction),
    /// Copy the selection to the OS clipboard (`UiScript::editbox_copy` + host write).
    Copy,
    /// Cut: copy + delete the selection.
    Cut,
    /// Paste the OS clipboard (host read + `UiScript::paste`).
    Paste,
}

/// The chord table: what `key` under `m` means, `None` when it means nothing (an unbound key —
/// the caller falls through to plain character input, minus command-modified chars).
pub(crate) fn chord(key: KeyCode, m: Mods, mac: bool) -> Option<Chord> {
    if mac {
        chord_mac(key, m)
    } else {
        chord_pc(key, m)
    }
}

/// macOS: the Cocoa text-field law. Option = word, Cmd = line edge (moves and deletes alike);
/// Cmd+A/C/X/V; plain Up/Down = history recall, Shift/Cmd'd Up/Down = the line edges. The
/// Ctrl-plane (Cocoa's Emacs set: Ctrl+A/E/K…) is deliberately unbound — decision 0301.
fn chord_mac(key: KeyCode, m: Mods) -> Option<Chord> {
    use EditUnit::{Char, Edge, Word};
    let mv = |unit, back| {
        Some(Chord::Edit(EditAction::Move {
            unit,
            back,
            extend: m.shift,
        }))
    };
    let del = |unit, back| Some(Chord::Edit(EditAction::Delete { unit, back }));
    match key {
        KeyCode::ArrowLeft | KeyCode::ArrowRight => {
            let back = key == KeyCode::ArrowLeft;
            if m.sup {
                mv(Edge, back)
            } else if m.alt {
                mv(Word, back)
            } else {
                mv(Char, back)
            }
        }
        // Cmd+Up/Down = document start/end (the edges of a single-line box); Shift+Up/Down
        // extends there; plain Up/Down = the chat box's history recall.
        KeyCode::ArrowUp | KeyCode::ArrowDown => {
            let back = key == KeyCode::ArrowUp;
            if m.sup || m.shift {
                mv(Edge, back)
            } else if m.alt || m.ctrl {
                None
            } else if back {
                Some(Chord::Edit(EditAction::HistoryPrev))
            } else {
                Some(Chord::Edit(EditAction::HistoryNext))
            }
        }
        KeyCode::Home => mv(Edge, true),
        KeyCode::End => mv(Edge, false),
        // The delete family: Cmd = clear to the line edge ("clear the whole input" from the
        // end), Option = one word.
        KeyCode::Backspace => {
            if m.sup {
                del(Edge, true)
            } else if m.alt {
                del(Word, true)
            } else {
                del(Char, true)
            }
        }
        KeyCode::Delete => {
            if m.sup {
                del(Edge, false)
            } else if m.alt {
                del(Word, false)
            } else {
                del(Char, false)
            }
        }
        KeyCode::KeyA if m.sup => Some(Chord::Edit(EditAction::SelectAll)),
        KeyCode::KeyC if m.sup => Some(Chord::Copy),
        KeyCode::KeyX if m.sup => Some(Chord::Cut),
        KeyCode::KeyV if m.sup => Some(Chord::Paste),
        _ => None,
    }
}

/// Windows/Linux: the Ctrl law — the reference client's own chords where 1.12 had them
/// (`0x77b160`), plus the modern Ctrl word-deletes.
fn chord_pc(key: KeyCode, m: Mods) -> Option<Chord> {
    use EditUnit::{Char, Edge, Word};
    let mv = |unit, back| {
        Some(Chord::Edit(EditAction::Move {
            unit,
            back,
            extend: m.shift,
        }))
    };
    let del = |unit, back| Some(Chord::Edit(EditAction::Delete { unit, back }));
    match key {
        // Ctrl picks the word-granular helper — the ref's own fork on its Ctrl test `0x41f8f0(1)`.
        KeyCode::ArrowLeft | KeyCode::ArrowRight => {
            let back = key == KeyCode::ArrowLeft;
            if m.ctrl {
                mv(Word, back)
            } else {
                mv(Char, back)
            }
        }
        // Plain Up/Down only: the history recall (a modified arrow means nothing here).
        KeyCode::ArrowUp if !(m.ctrl || m.alt || m.shift || m.sup) => {
            Some(Chord::Edit(EditAction::HistoryPrev))
        }
        KeyCode::ArrowDown if !(m.ctrl || m.alt || m.shift || m.sup) => {
            Some(Chord::Edit(EditAction::HistoryNext))
        }
        // Ctrl+Home/End = plain Home/End in a single-line box.
        KeyCode::Home => mv(Edge, true),
        KeyCode::End => mv(Edge, false),
        // Shift+Delete = Cut — the CUA mirror the ref itself honors (`0x77b160`) — else the
        // modern Ctrl word-delete, else one char.
        KeyCode::Backspace => {
            if m.ctrl {
                del(Word, true)
            } else {
                del(Char, true)
            }
        }
        KeyCode::Delete => {
            if m.shift && !m.ctrl {
                Some(Chord::Cut)
            } else if m.ctrl {
                del(Word, false)
            } else {
                del(Char, false)
            }
        }
        // The other CUA mirrors the ref honors: Ctrl+Insert = copy, Shift+Insert = paste.
        KeyCode::Insert if m.ctrl => Some(Chord::Copy),
        KeyCode::Insert if m.shift => Some(Chord::Paste),
        // `&& !m.alt` is AltGr, and it is load-bearing on both Windows and Linux. AltGr is
        // delivered as Ctrl+Alt, and it is how European layouts type real letters: on a Polish
        // layout AltGr+A is `ą`, AltGr+C `ć`, AltGr+X `ź`. Without the exclusion those four
        // keystrokes are eaten as Select-All/Copy/Cut/Paste and the letter never reaches the box —
        // the character is simply untypeable in chat. Both platforms' own edit controls resolve it
        // this way (Ctrl+Alt+A is not Select All anywhere AltGr exists), and the char-input branch
        // in `input.rs` already lets the AltGr plane through for exactly this reason; the chord
        // table has to agree with it or it just intercepts the key first. Decision 0702.
        KeyCode::KeyA if m.ctrl && !m.alt => Some(Chord::Edit(EditAction::SelectAll)),
        KeyCode::KeyC if m.ctrl && !m.alt => Some(Chord::Copy),
        KeyCode::KeyX if m.ctrl && !m.alt => Some(Chord::Cut),
        KeyCode::KeyV if m.ctrl && !m.alt => Some(Chord::Paste),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
        sup: false,
    };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const SUP: Mods = Mods { sup: true, ..NONE };

    fn edit(c: Option<Chord>) -> EditAction {
        match c {
            Some(Chord::Edit(a)) => a,
            other => panic!("expected an edit action, got {other:?}"),
        }
    }

    #[test]
    fn mac_table() {
        use EditAction::*;
        use EditUnit::*;
        // Plain arrows: char moves; Option: word; Cmd: edge; Shift extends.
        assert_eq!(
            edit(chord(KeyCode::ArrowLeft, NONE, true)),
            Move {
                unit: Char,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowRight, ALT, true)),
            Move {
                unit: Word,
                back: false,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowLeft, SUP, true)),
            Move {
                unit: Edge,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::ArrowRight,
                Mods { shift: true, ..ALT },
                true
            )),
            Move {
                unit: Word,
                back: false,
                extend: true
            }
        );
        // Up/Down: plain = history; Shift or Cmd = the edges.
        assert_eq!(edit(chord(KeyCode::ArrowUp, NONE, true)), HistoryPrev);
        assert_eq!(edit(chord(KeyCode::ArrowDown, NONE, true)), HistoryNext);
        assert_eq!(
            edit(chord(KeyCode::ArrowUp, SHIFT, true)),
            Move {
                unit: Edge,
                back: true,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowDown, SUP, true)),
            Move {
                unit: Edge,
                back: false,
                extend: false
            }
        );
        // The delete family: Cmd+Backspace clears to the start, Option+Backspace one word.
        assert_eq!(
            edit(chord(KeyCode::Backspace, SUP, true)),
            Delete {
                unit: Edge,
                back: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Backspace, ALT, true)),
            Delete {
                unit: Word,
                back: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Delete, ALT, true)),
            Delete {
                unit: Word,
                back: false
            }
        );
        // Cmd+A/C/X/V; the Ctrl plane is unbound; plain letters mean nothing.
        assert_eq!(edit(chord(KeyCode::KeyA, SUP, true)), SelectAll);
        assert_eq!(chord(KeyCode::KeyC, SUP, true), Some(Chord::Copy));
        assert_eq!(chord(KeyCode::KeyX, SUP, true), Some(Chord::Cut));
        assert_eq!(chord(KeyCode::KeyV, SUP, true), Some(Chord::Paste));
        assert_eq!(chord(KeyCode::KeyA, CTRL, true), None);
        assert_eq!(chord(KeyCode::KeyA, NONE, true), None);
    }

    #[test]
    fn pc_table() {
        use EditAction::*;
        use EditUnit::*;
        // Ctrl+arrows = the ref's word fork; plain = char.
        assert_eq!(
            edit(chord(KeyCode::ArrowLeft, CTRL, false)),
            Move {
                unit: Word,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowRight, SHIFT, false)),
            Move {
                unit: Char,
                back: false,
                extend: true
            }
        );
        // Plain Up/Down = history; any modifier unbinds them.
        assert_eq!(edit(chord(KeyCode::ArrowUp, NONE, false)), HistoryPrev);
        assert_eq!(chord(KeyCode::ArrowUp, SHIFT, false), None);
        // Home/End; Ctrl+Backspace/Delete word deletes.
        assert_eq!(
            edit(chord(KeyCode::End, SHIFT, false)),
            Move {
                unit: Edge,
                back: false,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Backspace, CTRL, false)),
            Delete {
                unit: Word,
                back: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Delete, CTRL, false)),
            Delete {
                unit: Word,
                back: false
            }
        );
        // Ctrl+A/C/X/V + the CUA mirrors (Ctrl/Shift+Insert, Shift+Delete).
        assert_eq!(edit(chord(KeyCode::KeyA, CTRL, false)), SelectAll);
        assert_eq!(chord(KeyCode::KeyC, CTRL, false), Some(Chord::Copy));
        assert_eq!(chord(KeyCode::KeyV, CTRL, false), Some(Chord::Paste));
        assert_eq!(chord(KeyCode::Insert, CTRL, false), Some(Chord::Copy));
        assert_eq!(chord(KeyCode::Insert, SHIFT, false), Some(Chord::Paste));
        assert_eq!(chord(KeyCode::Delete, SHIFT, false), Some(Chord::Cut));
        // Super means nothing on this side.
        assert_eq!(chord(KeyCode::KeyA, SUP, false), None);
    }

    /// AltGr — delivered as Ctrl+Alt on both Windows and Linux — types real letters on European
    /// layouts (`ą`/`ć`/`ź` on a Polish one), so it must fall through to character input rather
    /// than being swallowed as the Ctrl clipboard chord. Regression for decision 0702.
    #[test]
    fn altgr_letters_are_not_clipboard_chords() {
        const ALTGR: Mods = Mods {
            ctrl: true,
            alt: true,
            ..NONE
        };
        for key in [KeyCode::KeyA, KeyCode::KeyC, KeyCode::KeyX, KeyCode::KeyV] {
            assert_eq!(
                chord(key, ALTGR, false),
                None,
                "AltGr+{key:?} must reach character input, not act as a clipboard chord"
            );
        }
        // The exclusion is exactly AltGr: plain Ctrl is still the chord.
        assert_eq!(
            edit(chord(KeyCode::KeyA, CTRL, false)),
            EditAction::SelectAll
        );
        assert_eq!(chord(KeyCode::KeyC, CTRL, false), Some(Chord::Copy));
        assert_eq!(chord(KeyCode::KeyX, CTRL, false), Some(Chord::Cut));
        assert_eq!(chord(KeyCode::KeyV, CTRL, false), Some(Chord::Paste));
    }
}
