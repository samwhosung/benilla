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
            ("\"THICKOUTLINE\"", "OUTLINE, THICKOUTLINE"),
            ("\"THICK\"", ""),
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

/// The kinds behind the shared implementation (`0x79f210`), which answers nil and writes nothing
/// on a failed load (`0x79f30f` → `0x79f361`, before the stores at `0x79f321`-`0x79f337`) or for a
/// height of 0 or less, without loading (`0x79f2f1`-`0x79f2ff`).
#[test]
fn a_refused_load_or_a_non_positive_height_changes_nothing_on_the_shared_path() {
    let mut s = receivers();
    s.set_font_probe(Box::new(|path| {
        path.eq_ignore_ascii_case("fonts\\frizqt__.ttf")
    }));
    for &(obj, kind) in KINDS.iter().filter(|(_, k)| *k != "FontString") {
        s.run(&format!(
            "{obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 12, \"OUTLINE\")"
        ))
        .unwrap();
        for bad in [
            "\"Interface\\\\AddOns\\\\X\\\\gone.ttf\", 30",
            "\"\", 30",
            "\"Fonts\\\\FRIZQT__.TTF\", 0",
            "\"Fonts\\\\FRIZQT__.TTF\", -3",
        ] {
            let answer = s
                .eval::<mlua::MultiValue>(&format!("return {obj}:SetFont({bad})"))
                .unwrap_or_else(|e| panic!("{kind}: SetFont({bad}) raised {e}"));
            // Button's binding discards the answer (`0x780880`); the others push nil.
            if kind != "Button" {
                assert!(
                    matches!(answer.iter().next(), Some(mlua::Value::Nil)),
                    "{kind}: SetFont({bad}) answered {answer:?}"
                );
            }
            let got = s
                .eval::<(Option<String>, f32, String)>(&format!("return {obj}:GetFont()"))
                .unwrap();
            assert_eq!(
                got,
                (
                    Some("Fonts\\FRIZQT__.TTF".to_string()),
                    12.0,
                    "OUTLINE".to_string()
                ),
                "{kind}: SetFont({bad}) must change nothing"
            );
        }
    }
}

/// The FontString's own load (`0x7714f0`) returns before its height store (`0x771571`-`0x77157b`)
/// when the face fails; a height of exactly 0 skips the load, stores 0, frees the font (`0x7715b3`)
/// and answers 1.
#[test]
fn a_fontstrings_refused_load_changes_nothing_and_a_zero_height_frees_its_font() {
    let mut s = receivers();
    s.set_font_probe(Box::new(|path| {
        path.eq_ignore_ascii_case("fonts\\frizqt__.ttf")
    }));
    s.run("TFS:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 12, \"OUTLINE\")")
        .unwrap();
    assert!(s
        .eval::<bool>("return TFS:SetFont(\"Interface\\\\AddOns\\\\X\\\\gone.ttf\", 30) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<(Option<String>, f32, String)>("return TFS:GetFont()")
            .unwrap(),
        (
            Some("Fonts\\FRIZQT__.TTF".to_string()),
            12.0,
            "OUTLINE".to_string()
        )
    );
    assert!(s
        .eval::<bool>("return TFS:SetFont(\"Interface\\\\AddOns\\\\X\\\\gone.ttf\", 0) == 1")
        .unwrap());
    assert_eq!(
        s.eval::<(Option<String>, f32, String)>("return TFS:GetFont()")
            .unwrap(),
        (None, 0.0, String::new())
    );
}

/// `0x6f1a90` ORs in every name of `0x811b10` found as a case-insensitive substring, so `THICK`
/// alone sets nothing, and `GetFont` (`0x6f1ad0`) joins every set bit's name in table order with
/// `", "`. SimpleHTML's element fonts hold no MONOCHROME yet, so it sits out the monochrome rows.
#[test]
fn get_font_joins_every_set_flag_in_table_order() {
    let s = receivers();
    for &(obj, kind) in KINDS.iter().filter(|(_, kind)| *kind != "SimpleHTML") {
        for (arg, want) in [
            ("MONOCHROME", "MONOCHROME"),
            ("OUTLINE,MONOCHROME", "OUTLINE, MONOCHROME"),
            (
                "monochrome thickoutline",
                "OUTLINE, THICKOUTLINE, MONOCHROME",
            ),
            ("THICK", ""),
        ] {
            s.run(&format!(
                "{obj}:SetFont(\"Fonts\\\\FRIZQT__.TTF\", 13, \"{arg}\")"
            ))
            .unwrap();
            assert_eq!(flags_of(&s, obj), want, "{kind}: {arg}");
        }
    }
}

/// The XML legs set the word whole (`0x771194`-`0x771204`, `0x783db1`-`0x783e14`): `THICK` is
/// 5, both outline bits, and a non-empty `monochrome=` adds 2.
#[test]
fn an_xml_thick_monochrome_font_reads_back_every_flag() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <Font name="XmlFlagsFont" font="Fonts\FRIZQT__.TTF" outline="THICK" monochrome="1"
                  virtual="true">
                <FontHeight><AbsValue val="12"/></FontHeight>
            </Font>
            <Frame name="XmlFlagsHost">
                <Layers><Layer>
                    <FontString name="XmlFlagsString" font="Fonts\FRIZQT__.TTF" outline="THICK"
                                monochrome="1">
                        <FontHeight><AbsValue val="12"/></FontHeight>
                    </FontString>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for obj in ["XmlFlagsFont", "XmlFlagsString"] {
        assert_eq!(
            flags_of(&s, obj),
            "OUTLINE, THICKOUTLINE, MONOCHROME",
            "{obj}"
        );
    }
}
