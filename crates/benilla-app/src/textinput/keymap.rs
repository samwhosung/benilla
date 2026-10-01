//! The per-OS text-editing keymap: a keypress and modifiers to an engine [`EditAction`] or a
//! host-side clipboard operation. The Windows/Linux table is the reference's key handler
//! (`0x77b160`: Ctrl+arrows by word, Ctrl+HOME/END to the text's edge, Ctrl+A/C/X/V,
//! Ctrl/Shift+Insert, Shift+Delete).
//! Deviation: macOS takes the Cocoa text-field chords and Ctrl+Backspace/Delete delete a word,
//! because editing follows each OS's own text fields.

use benilla_ui::script::{EditAction, EditUnit};
use bevy::input::keyboard::KeyCode;

use crate::bindings::chord::KeyName;

/// The modifier snapshot a chord is read against; `sup` is Cmd on macOS, the OS key elsewhere.
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
    /// A semantic edit for `UiScript::editbox_action`.
    Edit(EditAction),
    /// Copy the selection to the OS clipboard (`UiScript::editbox_copy` + host write).
    Copy,
    /// Copy, then delete the selection.
    Cut,
    /// Paste the OS clipboard (host read + `UiScript::paste`).
    Paste,
}

/// What `key`, which the layout names `name`, means under `m`; `None` falls through to character
/// input.
pub(crate) fn chord(key: KeyCode, name: Option<KeyName>, m: Mods, mac: bool) -> Option<Chord> {
    // Ctrl (Cmd on a Mac) with the keys the layout names A, C, X and V: the reference's handler
    // switches on the key's code (`0x77b1fb`), the name its bindings use, so on AZERTY select-all
    // is the key labelled A. On the PC `!m.alt` excludes AltGr, which arrives as Ctrl+Alt and types
    // letters on European layouts (Polish AltGr+A is `ą`); must agree with the char-input branch
    // in `input.rs`.
    let held = if mac { m.sup } else { m.ctrl && !m.alt };
    if let (true, Some(KeyName::Char(letter))) = (held, name) {
        match letter {
            'A' => return Some(Chord::Edit(EditAction::SelectAll)),
            'C' => return Some(Chord::Copy),
            'X' => return Some(Chord::Cut),
            'V' => return Some(Chord::Paste),
            _ => {}
        }
    }
    if mac {
        chord_mac(key, m)
    } else {
        chord_pc(key, m)
    }
}

/// macOS, the Cocoa text-field chords: Option+Left/Right is a word and Cmd+Left/Right the line's
/// edge; Cmd+Up/Down and Shift+Up/Down go to the text's edge, and Up/Down, plain or with Option,
/// move a row, or recall history in a single-line box. Cocoa's Emacs Ctrl set is left unbound.
fn chord_mac(key: KeyCode, m: Mods) -> Option<Chord> {
    use EditUnit::{Char, Edge, Line, Row, Word};
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
                mv(Line, back)
            } else if m.alt {
                mv(Word, back)
            } else {
                mv(Char, back)
            }
        }
        // Cmd+Up/Down go to the text's start or end and Shift+Up/Down select to it, as a Cocoa
        // text field's do. Option is the reference's Alt, which its UP and DOWN arms do not read
        // (`0x77b64e`, `0x77b675`), and the one modifier that keeps the arrows in an alt-arrow box
        // (`0x77b1b3`): the chat box's history recall.
        KeyCode::ArrowUp | KeyCode::ArrowDown => {
            let back = key == KeyCode::ArrowUp;
            if m.sup || m.shift {
                mv(Edge, back)
            } else if m.ctrl {
                None
            } else {
                mv(Row, back)
            }
        }
        KeyCode::Home => mv(Edge, true),
        KeyCode::End => mv(Edge, false),
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
        _ => None,
    }
}

/// Windows/Linux: the reference's chords (`0x77b160`) plus the Ctrl word deletes.
fn chord_pc(key: KeyCode, m: Mods) -> Option<Chord> {
    use EditUnit::{Char, Edge, Line, Row, Word};
    let mv = |unit, back| {
        Some(Chord::Edit(EditAction::Move {
            unit,
            back,
            extend: m.shift,
        }))
    };
    let del = |unit, back| Some(Chord::Edit(EditAction::Delete { unit, back }));
    match key {
        // Ctrl picks the word helper, as the reference forks on its Ctrl test `0x41f8f0(1)`.
        KeyCode::ArrowLeft | KeyCode::ArrowRight => {
            let back = key == KeyCode::ArrowLeft;
            if m.ctrl {
                mv(Word, back)
            } else {
                mv(Char, back)
            }
        }
        // UP/DOWN read only Shift, as the extend flag (`0x77b65d`, `0x77b684`): a row in a
        // multi-line box, history in a single-line one.
        KeyCode::ArrowUp | KeyCode::ArrowDown if !m.sup => mv(Row, key == KeyCode::ArrowUp),
        // HOME/END stop at a newline; Ctrl goes to the text's edge (`0x77b49e`, `0x77b4de`).
        KeyCode::Home | KeyCode::End => mv(if m.ctrl { Edge } else { Line }, key == KeyCode::Home),
        // Shift+Delete cuts, as the reference does (`0x77b160`).
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
        // The reference's other CUA mirrors: Ctrl+Insert copies, Shift+Insert pastes.
        KeyCode::Insert if m.ctrl => Some(Chord::Copy),
        KeyCode::Insert if m.shift => Some(Chord::Paste),
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

    /// A key's name on a US layout.
    fn us(key: KeyCode) -> Option<KeyName> {
        crate::bindings::chord::key_token(key, &Default::default())
    }

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
        assert_eq!(
            edit(chord(
                KeyCode::ArrowLeft,
                us(KeyCode::ArrowLeft),
                NONE,
                true
            )),
            Move {
                unit: Char,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::ArrowRight,
                us(KeyCode::ArrowRight),
                ALT,
                true
            )),
            Move {
                unit: Word,
                back: false,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowLeft, us(KeyCode::ArrowLeft), SUP, true)),
            Move {
                unit: Line,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::ArrowRight,
                us(KeyCode::ArrowRight),
                Mods { shift: true, ..ALT },
                true
            )),
            Move {
                unit: Word,
                back: false,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowUp, us(KeyCode::ArrowUp), NONE, true)),
            Move {
                unit: Row,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::ArrowDown,
                us(KeyCode::ArrowDown),
                NONE,
                true
            )),
            Move {
                unit: Row,
                back: false,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowUp, us(KeyCode::ArrowUp), SHIFT, true)),
            Move {
                unit: Edge,
                back: true,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowDown, us(KeyCode::ArrowDown), SUP, true)),
            Move {
                unit: Edge,
                back: false,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Backspace, us(KeyCode::Backspace), SUP, true)),
            Delete {
                unit: Edge,
                back: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Backspace, us(KeyCode::Backspace), ALT, true)),
            Delete {
                unit: Word,
                back: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Delete, us(KeyCode::Delete), ALT, true)),
            Delete {
                unit: Word,
                back: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::KeyA, us(KeyCode::KeyA), SUP, true)),
            SelectAll
        );
        assert_eq!(
            chord(KeyCode::KeyC, us(KeyCode::KeyC), SUP, true),
            Some(Chord::Copy)
        );
        assert_eq!(
            chord(KeyCode::KeyX, us(KeyCode::KeyX), SUP, true),
            Some(Chord::Cut)
        );
        assert_eq!(
            chord(KeyCode::KeyV, us(KeyCode::KeyV), SUP, true),
            Some(Chord::Paste)
        );
        assert_eq!(chord(KeyCode::KeyA, us(KeyCode::KeyA), CTRL, true), None);
        assert_eq!(chord(KeyCode::KeyA, us(KeyCode::KeyA), NONE, true), None);
    }

    #[test]
    fn pc_table() {
        use EditAction::*;
        use EditUnit::*;
        assert_eq!(
            edit(chord(
                KeyCode::ArrowLeft,
                us(KeyCode::ArrowLeft),
                CTRL,
                false
            )),
            Move {
                unit: Word,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::ArrowRight,
                us(KeyCode::ArrowRight),
                SHIFT,
                false
            )),
            Move {
                unit: Char,
                back: false,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::ArrowUp, us(KeyCode::ArrowUp), NONE, false)),
            Move {
                unit: Row,
                back: true,
                extend: false
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::ArrowDown,
                us(KeyCode::ArrowDown),
                SHIFT,
                false
            )),
            Move {
                unit: Row,
                back: false,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::End, us(KeyCode::End), SHIFT, false)),
            Move {
                unit: Line,
                back: false,
                extend: true
            }
        );
        assert_eq!(
            edit(chord(
                KeyCode::Backspace,
                us(KeyCode::Backspace),
                CTRL,
                false
            )),
            Delete {
                unit: Word,
                back: true
            }
        );
        assert_eq!(
            edit(chord(KeyCode::Delete, us(KeyCode::Delete), CTRL, false)),
            Delete {
                unit: Word,
                back: false
            }
        );
        assert_eq!(
            edit(chord(KeyCode::KeyA, us(KeyCode::KeyA), CTRL, false)),
            SelectAll
        );
        assert_eq!(
            chord(KeyCode::KeyC, us(KeyCode::KeyC), CTRL, false),
            Some(Chord::Copy)
        );
        assert_eq!(
            chord(KeyCode::KeyV, us(KeyCode::KeyV), CTRL, false),
            Some(Chord::Paste)
        );
        assert_eq!(
            chord(KeyCode::Insert, us(KeyCode::Insert), CTRL, false),
            Some(Chord::Copy)
        );
        assert_eq!(
            chord(KeyCode::Insert, us(KeyCode::Insert), SHIFT, false),
            Some(Chord::Paste)
        );
        assert_eq!(
            chord(KeyCode::Delete, us(KeyCode::Delete), SHIFT, false),
            Some(Chord::Cut)
        );
        assert_eq!(chord(KeyCode::KeyA, us(KeyCode::KeyA), SUP, false), None);
    }

    /// On a Mac, Cmd+Left/Right go to the caret's line's start or end, as a Cocoa text view's do;
    /// in a single-line box, with no newline typed, that is the text's edge.
    #[test]
    fn mac_cmd_left_right_go_to_the_lines_edge() {
        use EditAction::Move;
        use EditUnit::Line;
        for (key, back) in [(KeyCode::ArrowLeft, true), (KeyCode::ArrowRight, false)] {
            assert_eq!(
                edit(chord(key, us(key), SUP, true)),
                Move {
                    unit: Line,
                    back,
                    extend: false
                }
            );
            assert_eq!(
                edit(chord(key, us(key), Mods { shift: true, ..SUP }, true)),
                Move {
                    unit: Line,
                    back,
                    extend: true
                }
            );
        }
    }

    /// On a Mac, Option is the reference's Alt: Option+Up/Down reach the UP and DOWN arms
    /// (`0x77b64e`, `0x77b675`), which read only Shift, so the alt-arrow chat box recalls its
    /// history with them (`0x77b1b3`, then `0x77d030`/`0x77cfd0`). Ctrl stays unbound.
    #[test]
    fn mac_option_up_down_move_a_row_or_recall_history() {
        use EditAction::Move;
        use EditUnit::Row;
        for (key, back) in [(KeyCode::ArrowUp, true), (KeyCode::ArrowDown, false)] {
            assert_eq!(
                edit(chord(key, us(key), ALT, true)),
                Move {
                    unit: Row,
                    back,
                    extend: false
                }
            );
            assert_eq!(chord(key, us(key), CTRL, true), None);
        }
    }

    /// The reference's UP and DOWN arms read only Shift (`0x77b64e`, `0x77b675`), so Ctrl and Alt
    /// reach them too: Alt+Up is how the alt-arrow chat box recalls its history. HOME and END
    /// stop at a newline and Ctrl+HOME/END go to the text's edge (`0x77b499`, `0x77b4d9`).
    #[test]
    fn pc_up_down_take_any_modifier_and_home_end_split_on_ctrl() {
        use EditAction::Move;
        use EditUnit::*;
        for m in [NONE, CTRL, ALT, Mods { shift: true, ..ALT }] {
            assert_eq!(
                edit(chord(KeyCode::ArrowUp, us(KeyCode::ArrowUp), m, false)),
                Move {
                    unit: Row,
                    back: true,
                    extend: m.shift
                }
            );
        }
        assert_eq!(
            chord(KeyCode::ArrowDown, us(KeyCode::ArrowDown), SUP, false),
            None
        );
        for (key, back) in [(KeyCode::Home, true), (KeyCode::End, false)] {
            assert_eq!(
                edit(chord(key, us(key), NONE, false)),
                Move {
                    unit: Line,
                    back,
                    extend: false
                }
            );
            assert_eq!(
                edit(chord(
                    key,
                    us(key),
                    Mods {
                        shift: true,
                        ..CTRL
                    },
                    false
                )),
                Move {
                    unit: Edge,
                    back,
                    extend: true
                }
            );
            // macOS keeps the text-field chord: HOME and END go to the text's edge.
            assert_eq!(
                edit(chord(key, us(key), NONE, true)),
                Move {
                    unit: Edge,
                    back,
                    extend: false
                }
            );
        }
    }

    /// AltGr arrives as Ctrl+Alt and types letters on European layouts (Polish `ą`, `ć`, `ź`).
    #[test]
    fn altgr_letters_are_not_clipboard_chords() {
        const ALTGR: Mods = Mods {
            ctrl: true,
            alt: true,
            ..NONE
        };
        for key in [KeyCode::KeyA, KeyCode::KeyC, KeyCode::KeyX, KeyCode::KeyV] {
            assert_eq!(
                chord(key, us(key), ALTGR, false),
                None,
                "AltGr+{key:?} must reach character input, not act as a clipboard chord"
            );
        }
        assert_eq!(
            edit(chord(KeyCode::KeyA, us(KeyCode::KeyA), CTRL, false)),
            EditAction::SelectAll
        );
        assert_eq!(
            chord(KeyCode::KeyC, us(KeyCode::KeyC), CTRL, false),
            Some(Chord::Copy)
        );
        assert_eq!(
            chord(KeyCode::KeyX, us(KeyCode::KeyX), CTRL, false),
            Some(Chord::Cut)
        );
        assert_eq!(
            chord(KeyCode::KeyV, us(KeyCode::KeyV), CTRL, false),
            Some(Chord::Paste)
        );
    }

    /// Select-all, copy, cut and paste follow the layout's names, as the reference's handler
    /// switches on the key's code (`0x77b1fb`): on AZERTY they sit on the keys labelled A, C, X
    /// and V, wherever those are.
    #[test]
    fn the_clipboard_chords_sit_on_the_keys_the_layout_names() {
        let azerty_a = Some(KeyName::Char('A'));
        let azerty_q = Some(KeyName::Char('Q'));
        for mac in [false, true] {
            let held = if mac { SUP } else { CTRL };
            assert_eq!(
                edit(chord(KeyCode::KeyQ, azerty_a, held, mac)),
                EditAction::SelectAll,
                "the key labelled A (mac: {mac})"
            );
            assert_eq!(
                chord(KeyCode::KeyA, azerty_q, held, mac),
                None,
                "the key labelled Q (mac: {mac})"
            );
        }
    }
}
