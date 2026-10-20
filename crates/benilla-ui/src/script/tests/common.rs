//! Shared fixtures for the script test modules.

use crate::script::UiScript;

/// The 30 `TextLeft`/`TextRight` pairs a tooltip template declares (`GameTooltipTemplate.xml`), the
/// tooltip's line limit (`0x529650`, `0x530291`).
const TOOLTIP_PAIRS: usize = 30;

/// Declares `TestTooltipTemplate`, the stock template's line ladder: a tooltip made without it holds
/// no line, as on 1.12.1. A test makes one with
/// `CreateFrame("GameTooltip", name, parent, "TestTooltipTemplate")`. Its lines' two font objects
/// are declared first, face, height and justification only (`Fonts.xml`), as a line with no font
/// draws nothing; the lines' anchors are the stock template's (`GameTooltipTemplate.xml`).
pub(in crate::script) fn load_tooltip_template(s: &UiScript) {
    let mut xml = String::from(
        r#"<Ui><Font name="GameTooltipText" font="Fonts\FRIZQT__.TTF" justifyH="LEFT" virtual="true"><FontHeight><AbsValue val="12"/></FontHeight></Font><Font name="GameTooltipHeaderText" font="Fonts\FRIZQT__.TTF" justifyH="LEFT" virtual="true"><FontHeight><AbsValue val="14"/></FontHeight></Font><GameTooltip name="TestTooltipTemplate" virtual="true"><Layers><Layer level="ARTWORK">"#,
    );
    for i in 1..=TOOLTIP_PAIRS {
        let font = if i == 1 {
            "GameTooltipHeaderText"
        } else {
            "GameTooltipText"
        };
        xml.push_str(&tooltip_line_xml(i, font));
    }
    xml.push_str("</Layer></Layers></GameTooltip></Ui>");
    let doc = crate::framexml::parse(&xml).expect("tooltip template parses");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "template: {:?}", report.errors);
}

/// Line `i`'s pair as `GameTooltipTemplate.xml` declares it: the left text at the plate's TOPLEFT
/// (10, -10) or under the line above (0, -2), the right text's RIGHT on its left text's LEFT (40, 0).
fn tooltip_line_xml(i: usize, font: &str) -> String {
    let left_anchor = if i == 1 {
        r#"<Anchor point="TOPLEFT"><Offset><AbsDimension x="10" y="-10"/></Offset></Anchor>"#
            .to_string()
    } else {
        format!(
            r#"<Anchor point="TOPLEFT" relativeTo="$parentTextLeft{}" relativePoint="BOTTOMLEFT"><Offset><AbsDimension x="0" y="-2"/></Offset></Anchor>"#,
            i - 1
        )
    };
    format!(
        r#"<FontString name="$parentTextLeft{i}" inherits="{font}" hidden="true"><Anchors>{left_anchor}</Anchors></FontString><FontString name="$parentTextRight{i}" inherits="{font}" hidden="true"><Anchors><Anchor point="RIGHT" relativeTo="$parentTextLeft{i}" relativePoint="LEFT"><Offset><AbsDimension x="40" y="0"/></Offset></Anchor></Anchors></FontString>"#
    )
}

/// A VM with the tooltip template and the `UberTooltips` option the client registers at its
/// default "1" (`0x48fddd`), which `SetAction` and `SetShapeshift` read.
pub(super) fn script() -> UiScript {
    let s = UiScript::new().expect("construct UiScript");
    load_tooltip_template(&s);
    s.register_cvars([("UberTooltips", "1")]);
    s
}
