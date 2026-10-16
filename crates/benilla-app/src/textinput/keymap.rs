//! The per-OS text-editing keymap: a keypress and modifiers to an engine [`EditAction`] or a
//! host-side clipboard operation. The Windows/Linux table is the reference's key handler
//! (`0x77b160`: Ctrl+arrows by word, Ctrl+HOME/END to the text's edge, Ctrl+A/C/X/V, the Emacs
//! set Ctrl+B/F/D/W/U/K/P/N, Ctrl/Shift+Insert, Shift+Delete).
//! Deviation: macOS takes the Cocoa text-field chords for Cmd and Option, because editing follows
//! each OS's own text fields. Ctrl is its own key there, and its letters and word deletes are the
//! reference's.

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
    // The keys the layout names A, C, X and V under Cmd on a Mac, and the reference's Ctrl set
    // under Ctrl on both: the reference's handler switches on the key's code (`0x77b1fb`), the
    // name its bindings use, so on AZERTY select-all is the key labelled A. `!m.alt` excludes
    // AltGr on the PC, which arrives as Ctrl+Alt and types letters on European layouts (Polish
    // AltGr+A is `ą`), and Option on a Mac likewise; Cmd+Ctrl is no Ctrl chord there. Must agree
    // with the char-input branch in `input.rs`, which types nothing for a Ctrl or Cmd letter.
    if let Some(KeyName::Char(letter)) = name {
        if mac && m.sup {
            if let Some(chord) = clipboard_letter(letter) {
                return Some(chord);
            }
        } else if m.ctrl && !m.alt {
            if let Some(chord) = clipboard_letter(letter).or_else(|| ctrl_letter(letter, m)) {
                return Some(chord);
            }
        }
    }
    if mac {
        chord_mac(key, m)
    } else {
        chord_pc(key, m)
    }
}

/// Select-all and the clipboard: A, C, X and V of the jump table at `0x77b69c`.
fn clipboard_letter(letter: char) -> Option<Chord> {
    match letter {
        'A' => Some(Chord::Edit(EditAction::SelectAll)),
        'C' => Some(Chord::Copy),
        'X' => Some(Chord::Cut),
        'V' => Some(Chord::Paste),
        _ => None,
    }
}

/// The Emacs-style Ctrl+letter arms, the rest of the jump table at `0x77b69c` (letters
/// `0x77b6cc`; every other letter does nothing, `0x77b35e`). Each arm tests Ctrl alone
/// (`0x41f8f0(1)`), so Shift reaches only B and F, as the extend flag of the arrows' own helpers.
fn ctrl_letter(letter: char, m: Mods) -> Option<Chord> {
    use EditUnit::{Char, Word};
    let edit = |action| Some(Chord::Edit(action));
    match letter {
        // Back and forward a character (`0x77c870`, `0x77c750`).
        'B' | 'F' => edit(EditAction::Move {
            unit: Char,
            back: letter == 'B',
            extend: m.shift,
        }),
        // The selection or the character after the caret (`0x77c2e0`), as Delete does.
        'D' => edit(EditAction::Delete {
            unit: Char,
            back: false,
        }),
        // The selection or the spaces and the word before the caret (`0x77c3f0`), as Ctrl+Backspace.
        'W' => edit(EditAction::Delete {
            unit: Word,
            back: true,
        }),
        // To the text's start and end (`0x77c4a0`, `0x77c4d0`).
        'U' | 'K' => edit(EditAction::DeleteToEdge {
            back: letter == 'U',
        }),
        // The history line before and after (`0x77d030`, `0x77cfd0`), multi-line boxes included.
        'P' | 'N' => edit(EditAction::History {
            older: letter == 'P',
        }),
        _ => None,
    }
}

/// macOS, the Cocoa text-field chords: Option+Left/Right is a word and Cmd+Left/Right the line's
/// edge; Cmd+Up/Down and Shift+Up/Down go to the text's edge, and Up/Down, plain or with Option,
/// move a row, or recall history in a single-line box. Ctrl is the reference's: its letters are in
/// [`chord`], and Ctrl+Backspace/Delete delete a word (`0x77c3f0`, `0x77c310`). Ctrl+Left/Right
/// (Spaces) and Ctrl+Up/Down (Mission Control) are the OS's: the first stay a character step and
/// the second unbound.
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
            } else if m.alt || m.ctrl {
                del(Word, true)
            } else {
                del(Char, true)
            }
        }
        KeyCode::Delete => {
            if m.sup {
                del(Edge, false)
            } else if m.alt || m.ctrl {
                del(Word, false)
            } else {
                del(Char, false)
            }
        }
        _ => None,
    }
}

/// Windows/Linux: the reference's chords (`0x77b160`); its Ctrl+letter arms are [`ctrl_letter`].
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
        KeyCode::Backspace => {
            if m.ctrl {
                del(Word, true)
            } else {
                del(Char, true)
            }
        }
        // Shift is tested before Ctrl (`0x77b56e`), so Ctrl+Shift+Delete cuts too.
        KeyCode::Delete => {
            if m.shift {
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
        assert_eq!(
            edit(chord(KeyCode::KeyA, us(KeyCode::KeyA), CTRL, true)),
            SelectAll,
            "Ctrl is the reference's on a Mac too"
        );
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
        for key in [
            KeyCode::KeyA,
            KeyCode::KeyC,
            KeyCode::KeyX,
            KeyCode::KeyV,
            KeyCode::KeyB,
            KeyCode::KeyD,
            KeyCode::KeyF,
            KeyCode::KeyK,
            KeyCode::KeyN,
            KeyCode::KeyP,
            KeyCode::KeyU,
            KeyCode::KeyW,
        ] {
            assert_eq!(
                chord(key, us(key), ALTGR, false),
                None,
                "AltGr+{key:?} must reach character input, not act as a chord"
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

    /// The PC's Emacs arms (`0x77b69c`, letters `0x77b6cc`): B and F step a character (Shift
    /// extends), D deletes forward, W a word back, U and K to the text's edges, P and N recall
    /// the history before and after. Every other letter does nothing.
    #[test]
    fn pc_ctrl_letters_are_the_references_emacs_set() {
        use EditAction::*;
        use EditUnit::*;
        let key_of = |c: char| match c {
            'B' => KeyCode::KeyB,
            'D' => KeyCode::KeyD,
            'F' => KeyCode::KeyF,
            'K' => KeyCode::KeyK,
            'N' => KeyCode::KeyN,
            'P' => KeyCode::KeyP,
            'U' => KeyCode::KeyU,
            'W' => KeyCode::KeyW,
            _ => unreachable!(),
        };
        let want = |c: char, shift: bool| match c {
            'B' | 'F' => Move {
                unit: Char,
                back: c == 'B',
                extend: shift,
            },
            'D' => Delete {
                unit: Char,
                back: false,
            },
            'W' => Delete {
                unit: Word,
                back: true,
            },
            'U' | 'K' => DeleteToEdge { back: c == 'U' },
            'P' | 'N' => History { older: c == 'P' },
            _ => unreachable!(),
        };
        for c in ['B', 'D', 'F', 'K', 'N', 'P', 'U', 'W'] {
            let key = key_of(c);
            for shift in [false, true] {
                let m = Mods { shift, ..CTRL };
                assert_eq!(
                    edit(chord(key, us(key), m, false)),
                    want(c, shift),
                    "Ctrl+{c}"
                );
            }
            assert_eq!(chord(key, us(key), NONE, false), None, "{c} alone types");
        }
        for key in [KeyCode::KeyE, KeyCode::KeyZ, KeyCode::KeyH, KeyCode::KeyT] {
            assert_eq!(chord(key, us(key), CTRL, false), None, "{key:?}");
        }
        // The arms are keyed by the layout's name: on AZERTY the key labelled W is the Z key.
        assert_eq!(
            edit(chord(KeyCode::KeyZ, Some(KeyName::Char('W')), CTRL, false)),
            want('W', false)
        );
        assert_eq!(
            chord(KeyCode::KeyW, Some(KeyName::Char('Z')), CTRL, false),
            None
        );
    }

    /// On a Mac Ctrl is its own key, and its letters are the reference's, the PC's table whole;
    /// the Cmd letters stay the clipboard four, and Ctrl+Up/Down stay unbound.
    #[test]
    fn mac_takes_the_references_ctrl_set() {
        let ctrl_shift = Mods {
            shift: true,
            ..CTRL
        };
        let keys = [
            KeyCode::KeyA,
            KeyCode::KeyB,
            KeyCode::KeyC,
            KeyCode::KeyD,
            KeyCode::KeyF,
            KeyCode::KeyK,
            KeyCode::KeyN,
            KeyCode::KeyP,
            KeyCode::KeyU,
            KeyCode::KeyV,
            KeyCode::KeyW,
            KeyCode::KeyX,
        ];
        for key in keys {
            let on_pc = chord(key, us(key), CTRL, false);
            assert!(on_pc.is_some(), "Ctrl+{key:?} on the PC");
            for m in [CTRL, ctrl_shift] {
                assert_eq!(
                    chord(key, us(key), m, true),
                    chord(key, us(key), m, false),
                    "Ctrl+{key:?}"
                );
            }
            // Option makes it a different chord, and Cmd+Ctrl none of Ctrl's.
            let ctrl_alt = Mods { alt: true, ..CTRL };
            let cmd_ctrl = Mods { sup: true, ..CTRL };
            assert_eq!(
                chord(key, us(key), ctrl_alt, true),
                None,
                "Ctrl+Opt+{key:?}"
            );
            let cmd = chord(key, us(key), cmd_ctrl, true);
            assert_eq!(cmd, chord(key, us(key), SUP, true), "Cmd+Ctrl+{key:?}");
        }
        // Cmd keeps the clipboard four only.
        for key in [
            KeyCode::KeyB,
            KeyCode::KeyD,
            KeyCode::KeyF,
            KeyCode::KeyK,
            KeyCode::KeyN,
            KeyCode::KeyP,
            KeyCode::KeyU,
            KeyCode::KeyW,
        ] {
            assert_eq!(chord(key, us(key), SUP, true), None, "Cmd+{key:?}");
        }
        for key in [KeyCode::ArrowUp, KeyCode::ArrowDown] {
            assert_eq!(chord(key, us(key), CTRL, true), None, "Ctrl+{key:?}");
        }
        // Ctrl+Backspace and Ctrl+Delete delete a word, as on the PC; Cmd and Option keep theirs.
        for (key, back) in [(KeyCode::Backspace, true), (KeyCode::Delete, false)] {
            let word = EditAction::Delete {
                unit: EditUnit::Word,
                back,
            };
            assert_eq!(edit(chord(key, us(key), CTRL, true)), word, "Ctrl+{key:?}");
            assert_eq!(edit(chord(key, us(key), ALT, true)), word, "Option+{key:?}");
            assert_eq!(
                edit(chord(key, us(key), SUP, true)),
                EditAction::Delete {
                    unit: EditUnit::Edge,
                    back
                },
                "Cmd+{key:?}"
            );
        }
        // A letter outside the table types nothing and does nothing, as on the PC.
        assert_eq!(chord(KeyCode::KeyE, us(KeyCode::KeyE), CTRL, true), None);
    }

    /// Delete's Shift branch is tested before Ctrl (`0x77b56e`), so Ctrl+Shift+Delete cuts; Ctrl
    /// alone deletes a word.
    #[test]
    fn ctrl_shift_delete_cuts() {
        let ctrl_shift = Mods {
            shift: true,
            ..CTRL
        };
        for m in [SHIFT, ctrl_shift] {
            assert_eq!(
                chord(KeyCode::Delete, us(KeyCode::Delete), m, false),
                Some(Chord::Cut)
            );
        }
        assert_eq!(
            edit(chord(KeyCode::Delete, us(KeyCode::Delete), CTRL, false)),
            EditAction::Delete {
                unit: EditUnit::Word,
                back: false
            }
        );
    }

    /// A box holding `text` with the caret at `cursor`, nothing selected.
    fn boxed(text: &str, cursor: usize, multi_line: bool) -> benilla_ui::widget::EditBoxState {
        let mut eb = benilla_ui::widget::EditBoxState {
            text: text.into(),
            cursor,
            multi_line,
            ..Default::default()
        };
        eb.collapse();
        eb
    }

    /// Press `key` with Ctrl (and Shift when `shift`) on a box, through the PC table.
    fn press(eb: &mut benilla_ui::widget::EditBoxState, key: KeyCode, shift: bool) {
        let m = Mods { shift, ..CTRL };
        eb.apply(edit(chord(key, us(key), m, false)));
    }

    #[test]
    fn ctrl_b_and_f_step_a_character_and_shift_extends() {
        let mut eb = boxed("h\u{e9}llo", 3, false); // after the two-byte é
        press(&mut eb, KeyCode::KeyB, false);
        assert_eq!(eb.cursor, 1);
        press(&mut eb, KeyCode::KeyF, false);
        assert_eq!(eb.cursor, 3);
        press(&mut eb, KeyCode::KeyB, true);
        assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (1, 1, 3));
    }

    #[test]
    fn ctrl_d_deletes_the_selection_or_the_character_after() {
        let mut eb = boxed("abc", 1, false);
        press(&mut eb, KeyCode::KeyD, false);
        assert_eq!((eb.text.as_str(), eb.cursor), ("ac", 1));
        eb.highlight_text(0, 1); // "a", the caret stays at 1
        press(&mut eb, KeyCode::KeyD, false);
        assert_eq!(eb.text, "c");
    }

    #[test]
    fn ctrl_w_deletes_the_spaces_and_the_word_before_the_caret() {
        let mut eb = boxed("one two  ", 9, false);
        press(&mut eb, KeyCode::KeyW, false);
        assert_eq!(eb.text, "one ");
        press(&mut eb, KeyCode::KeyW, false);
        assert_eq!(eb.text, "");
    }

    /// U and K reach the whole text's edges, newlines included, and ignore a selection: they
    /// delete the span between the caret and the edge (`0x77c4a0`, `0x77c4d0`).
    #[test]
    fn ctrl_u_and_k_delete_to_the_texts_edges_whatever_is_selected() {
        let mut eb = boxed("ab\ncd", 4, true); // "ab\nc|d"
        press(&mut eb, KeyCode::KeyU, false);
        assert_eq!((eb.text.as_str(), eb.cursor), ("d", 0));

        let mut eb = boxed("ab\ncd", 1, true); // "a|b\ncd"
        press(&mut eb, KeyCode::KeyK, false);
        assert_eq!(eb.text, "a");

        let mut eb = boxed("abcdef", 2, false);
        eb.highlight_text(4, -1); // "ef", away from the caret
        press(&mut eb, KeyCode::KeyU, false);
        assert_eq!(eb.text, "cdef", "the selection is not what U deletes");
        assert_eq!(eb.sel_start, eb.sel_end, "and the delete clears it");

        let mut eb = boxed("abcdef", 4, false);
        eb.highlight_text(0, 2);
        press(&mut eb, KeyCode::KeyK, false);
        assert_eq!(eb.text, "abcd");
    }

    /// At the edge there is nothing to delete, and the selection stands (the reference's count
    /// is zero, so `0x77c280` is not called).
    #[test]
    fn ctrl_u_and_k_at_the_edge_change_nothing() {
        let mut eb = boxed("abc", 0, false);
        eb.highlight_text(0, -1);
        press(&mut eb, KeyCode::KeyU, false);
        assert_eq!((eb.text.as_str(), eb.sel_start, eb.sel_end), ("abc", 0, 3));
        eb.cursor = 3;
        press(&mut eb, KeyCode::KeyK, false);
        assert_eq!((eb.text.as_str(), eb.sel_start, eb.sel_end), ("abc", 0, 3));
    }

    /// Ctrl+P and Ctrl+N recall history in a multi-line box too, where Up and Down move a row.
    #[test]
    fn ctrl_p_and_n_recall_history_in_a_multi_line_box() {
        use benilla_ui::script::UiScript;
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            E = CreateFrame("EditBox", "E")
            E:SetMultiLine(true)
            E:SetHistoryLines(5)
            E:AddHistoryLine("first")
            E:AddHistoryLine("second")
            "#,
        )
        .unwrap();
        assert!(s.focus_editbox("E"));
        let text = |s: &UiScript| s.eval::<String>("return E:GetText()").unwrap();
        let ctrl = |s: &mut UiScript, key| {
            s.editbox_action(edit(chord(key, us(key), CTRL, false)));
        };
        ctrl(&mut s, KeyCode::KeyP);
        assert_eq!(text(&s), "second");
        ctrl(&mut s, KeyCode::KeyP);
        assert_eq!(text(&s), "first");
        ctrl(&mut s, KeyCode::KeyN);
        assert_eq!(text(&s), "second");
        // Up in the same box moves a row and leaves the text alone.
        s.editbox_action(edit(chord(
            KeyCode::ArrowUp,
            us(KeyCode::ArrowUp),
            NONE,
            false,
        )));
        assert_eq!(text(&s), "second");
    }

    /// A word is a run of non-spaces, punctuation inside it (`_iswspace`, `0x77c3f0`): Ctrl+W
    /// takes "foo.bar" whole, and Ctrl+Delete likewise forward (`0x77c310`).
    #[test]
    fn a_word_is_a_run_of_non_spaces_punctuation_included() {
        let mut eb = boxed("foo.bar", 7, false);
        press(&mut eb, KeyCode::KeyW, false);
        assert_eq!(eb.text, "");
        let mut eb = boxed("foo.bar baz", 0, false);
        press(&mut eb, KeyCode::Delete, false);
        assert_eq!(eb.text, " baz");
        // Spaces first, then the word, each way.
        let mut eb = boxed("foo.bar  baz.", 7, false);
        press(&mut eb, KeyCode::Delete, false);
        assert_eq!(
            eb.text, "foo.bar",
            "the spaces after the caret, then 'baz.'"
        );
    }

    /// Ctrl+Right crosses the rest of the word, then the spaces, and so stops at the next word's
    /// start (`0x77c7a0`).
    #[test]
    fn ctrl_right_lands_on_the_next_words_start() {
        let mut eb = boxed("foo bar. baz", 5, false); // "foo b|ar. baz"
        press(&mut eb, KeyCode::ArrowRight, false);
        assert_eq!(eb.cursor, 9, "start of 'baz', past 'ar.' and the space");
        press(&mut eb, KeyCode::ArrowRight, false);
        assert_eq!(eb.cursor, 12, "the text's end");
        let mut eb = boxed("foo   bar", 0, false);
        press(&mut eb, KeyCode::ArrowRight, true);
        assert_eq!((eb.cursor, eb.sel_start, eb.sel_end), (6, 0, 6));
    }

    /// Ctrl+Left crosses the spaces before the caret, then the word before them (`0x77c8c0`).
    #[test]
    fn ctrl_left_crosses_spaces_then_the_word() {
        let mut eb = boxed("a, b", 3, false); // "a, |b"
        press(&mut eb, KeyCode::ArrowLeft, false);
        assert_eq!(eb.cursor, 0, "the space, then 'a,'");
        let mut eb = boxed("a.b, c", 3, false); // "a.b|, c"
        press(&mut eb, KeyCode::ArrowLeft, false);
        assert_eq!(eb.cursor, 0, "'a.b' is one word");
        let mut eb = boxed("one two  ", 9, false);
        press(&mut eb, KeyCode::ArrowLeft, false);
        assert_eq!(eb.cursor, 4);
    }

    /// `_iswspace` on the box's UTF-8: the C table below U+0100 has TAB to CR and the space only,
    /// so NBSP is a letter; U+3000 and the other Unicode spaces count (`GetStringTypeW`).
    #[test]
    fn which_characters_are_spaces() {
        let mut eb = boxed("a\u{a0}b", 4, false);
        press(&mut eb, KeyCode::KeyW, false);
        assert_eq!(eb.text, "", "NBSP does not end the word");
        let mut eb = boxed("a\u{3000}b", 5, false);
        press(&mut eb, KeyCode::KeyW, false);
        assert_eq!(eb.text, "a\u{3000}", "the ideographic space does");
        let mut eb = boxed("a\tb", 3, false);
        press(&mut eb, KeyCode::KeyW, false);
        assert_eq!(eb.text, "a\t", "TAB does");
    }

    /// Ctrl+Left reads the whole character before the caret, as the deletes do, so a multi-byte
    /// space ends the word: from the end of "a U+3000 b" it stops after the space.
    #[test]
    fn ctrl_left_reads_the_whole_character_before_the_caret() {
        let mut eb = boxed("a\u{3000}b", 5, false);
        press(&mut eb, KeyCode::ArrowLeft, false);
        assert_eq!(eb.cursor, 4);
        press(&mut eb, KeyCode::ArrowLeft, false);
        assert_eq!(eb.cursor, 0, "the space, then 'a'");
    }
}
