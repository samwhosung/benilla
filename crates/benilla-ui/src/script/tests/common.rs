//! Shared fixtures for the script test modules.

use crate::script::UiScript;

/// The 30 `TextLeft`/`TextRight` pairs a tooltip template declares (`GameTooltipTemplate.xml`), the
/// tooltip's line limit (`0x529650`, `0x530291`).
const TOOLTIP_PAIRS: usize = 30;

/// Declares `TestTooltipTemplate`, the stock template's line ladder: a tooltip made without it holds
/// no line, as on 1.12.1. A test makes one with
/// `CreateFrame("GameTooltip", name, parent, "TestTooltipTemplate")`.
pub(in crate::script) fn load_tooltip_template(s: &UiScript) {
    let mut xml = String::from(
        r#"<Ui><GameTooltip name="TestTooltipTemplate" virtual="true"><Layers><Layer level="ARTWORK">"#,
    );
    for i in 1..=TOOLTIP_PAIRS {
        let font = if i == 1 {
            "GameTooltipHeaderText"
        } else {
            "GameTooltipText"
        };
        for side in ["Left", "Right"] {
            xml.push_str(&format!(
                r#"<FontString name="$parentText{side}{i}" inherits="{font}" hidden="true"/>"#
            ));
        }
    }
    xml.push_str("</Layer></Layers></GameTooltip></Ui>");
    let doc = crate::framexml::parse(&xml).expect("tooltip template parses");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "template: {:?}", report.errors);
}

pub(super) fn script() -> UiScript {
    let s = UiScript::new().expect("construct UiScript");
    load_tooltip_template(&s);
    s
}
