//! `SetFont`'s argument gate, one implementation (`0x79f210`) behind every kind's binding: the
//! Font object, FontString, EditBox, the two message frames, SimpleHTML and Button.

use super::common::script;
use crate::script::UiScript;

/// The seven receivers, each named so the usage line can name it, as `(global, kind)`.
const KINDS: &[(&str, &str)] = &[
    ("TFont", "Font"),
    ("TFS", "FontString"),
    ("TEB", "EditBox"),
    ("TMF", "MessageFrame"),
    ("TSMF", "ScrollingMessageFrame"),
    ("THTML", "SimpleHTML"),
    ("TB", "Button"),
];

fn receivers() -> UiScript {
    let s = script();
    s.run(
        r#"
        CreateFont("TFont")
        CreateFrame("Frame", "THost"):CreateFontString("TFS")
        CreateFrame("EditBox", "TEB")
        CreateFrame("MessageFrame", "TMF")
        CreateFrame("ScrollingMessageFrame", "TSMF")
        CreateFrame("SimpleHTML", "THTML")
        CreateFrame("Button", "TB")
    "#,
    )
    .unwrap();
    s
}

fn flags_of(s: &UiScript, obj: &str) -> String {
    s.eval::<(Option<String>, f32, String)>(&format!("return {obj}:GetFont()"))
        .unwrap()
        .2
}

/// The fourth argument is read only when `lua_isstring` (`0x79f287`) passes, a string or a number;
/// `false`, `nil` or anything else is no flags. pfUI's `SetFont(path, 13, outline and "OUTLINE")`
/// passes `false` when the option is off.
#[test]
fn set_font_reads_flags_only_from_a_string_or_a_number() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        for (arg, want) in [
            ("\"OUTLINE\"", "OUTLINE"),
            ("\"THICKOUTLINE\"", "THICKOUTLINE"),
            ("false", ""),
            ("nil", ""),
            ("7", ""),
            ("{}", ""),
        ] {
            s.run(&format!(
                "{obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 13, \"OUTLINE\")"
            ))
            .unwrap_or_else(|e| panic!("{kind}: SetFont with OUTLINE raised {e}"));
            s.run(&format!(
                "{obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 13, {arg})"
            ))
            .unwrap_or_else(|e| panic!("{kind}: SetFont(path, 13, {arg}) raised {e}"));
            // A flags word is written on every load (`0x79f334`), zero when the gate fails, so no
            // flags clears an outline the instance had.
            assert_eq!(flags_of(&s, obj), want, "{kind}: SetFont(path, 13, {arg})");
        }
    }
}

/// A missing third argument is the same as `nil`: no flags.
#[test]
fn set_font_without_flags_clears_the_outline() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        s.run(&format!(
            "{obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 13, \"OUTLINE\")
             {obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 13)"
        ))
        .unwrap();
        assert_eq!(flags_of(&s, obj), "", "{kind}");
    }
}

/// Only the path (`lua_isstring`, `0x79f225`) and the height (`lua_isnumber`, `0x79f239`) raise,
/// with `Usage: %s:SetFont("font", fontHeight [, flags])` (`0x87c69c`), `%s` being the receiver's
/// name or `<unnamed>` (`0x84c7f0`), as every shim passes it.
#[test]
fn a_bad_path_or_height_raises_the_usage_line_naming_the_receiver() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        let want = format!("Usage: {obj}:SetFont(\"font\", fontHeight [, flags])");
        for bad in [
            "",
            "\"Fonts\\\\FRIZQT__.TTF\"",
            "nil, 12",
            "{}, 12",
            "false, 12",
            "\"Fonts\\\\FRIZQT__.TTF\", \"tall\"",
            "\"Fonts\\\\FRIZQT__.TTF\", false",
        ] {
            let err = s
                .run(&format!("{obj}:SetFont({bad})"))
                .expect_err("a bad path or height raises");
            assert!(
                err.to_string().contains(&want),
                "{kind}: SetFont({bad}) raised {err}, not {want:?}"
            );
        }
    }
    s.run(
        r#"
        U1 = CreateFrame("Frame"):CreateFontString()
        U2 = CreateFrame("EditBox")
        U3 = CreateFrame("Button")
    "#,
    )
    .unwrap();
    for obj in ["U1", "U2", "U3"] {
        let err = s.run(&format!("{obj}:SetFont()")).unwrap_err();
        assert!(
            err.to_string()
                .contains("Usage: <unnamed>:SetFont(\"font\", fontHeight [, flags])"),
            "{obj}: {err}"
        );
    }
}

/// Both gates coerce: a number path is its string form and a numeric string height passes,
/// whitespace and all, as `lua_isnumber`'s string conversion allows.
#[test]
fn the_path_and_height_gates_coerce() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        s.run(&format!(
            "{obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", \" 14 \")"
        ))
        .unwrap_or_else(|e| panic!("{kind}: a padded numeric string height raised {e}"));
        let (_, h, _) = s
            .eval::<(Option<String>, f32, String)>(&format!("return {obj}:GetFont()"))
            .unwrap();
        assert_eq!(h, 14.0, "{kind}");
    }
}
