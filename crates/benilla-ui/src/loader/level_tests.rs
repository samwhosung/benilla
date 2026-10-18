//! The loader's per-level reads: what the reference reads on each `inherits` node in turn
//! ([`Loader::levels`]) rather than on the merged element.

use crate::framexml;
use crate::loader::*;
use crate::script::UiScript;

fn no_files(_: &str) -> Option<Vec<u8>> {
    None
}

fn ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(800.0, 600.0);
    s
}

fn load_str(s: &UiScript, xml: &str) -> LoadReport {
    let report = load(s, &framexml::parse(xml).expect("valid FrameXML"), &no_files);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    report
}

fn has_line(report: &LoadReport, line: &str) -> bool {
    report.status.lines().iter().any(|l| l == line) && report.warnings.iter().any(|w| w == line)
}

/// `0x767800`: `setAllPoints` beside the same node's `<Anchors>` is logged and ignored; at another
/// level it stands, and `SetAllPoints` clears the points an earlier level added.
#[test]
fn set_all_points_yields_to_its_own_nodes_anchors_only() {
    let mut s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="Host">
                <Size><AbsDimension x="100" y="50"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Layers><Layer>
                    <Texture name="HostBoth" setAllPoints="true">
                        <Anchors><Anchor point="TOPLEFT"/></Anchors>
                    </Texture>
                </Layer></Layers>
            </Frame>
            <Frame name="Both" parent="Host" setAllPoints="true">
                <Size><AbsDimension x="10" y="10"/></Size>
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
            </Frame>
            <Frame name="AllPointsTemplate" setAllPoints="true" virtual="true"/>
            <Frame name="CenterTemplate" virtual="true">
                <Anchors><Anchor point="CENTER"/></Anchors>
            </Frame>
            <Frame name="TemplateAll" parent="Host" inherits="AllPointsTemplate">
                <Anchors>
                    <Anchor point="CENTER"><Offset><AbsDimension x="0" y="-1"/></Offset></Anchor>
                </Anchors>
            </Frame>
            <Frame name="InstanceAll" parent="Host" inherits="CenterTemplate" setAllPoints="true"/>
        </Ui>"#,
    );
    s.resolve();
    assert!(has_line(
        &report,
        "SETALLPOINTS set to true in frame with anchors (ignored)"
    ));
    let points = |name: &str| {
        s.eval::<i64>(&format!("return {name}:GetNumPoints()"))
            .unwrap()
    };
    assert_eq!(points("Both"), 1, "the frame's own anchors alone");
    assert_eq!(points("HostBoth"), 1, "the region's own anchors alone");
    // The cooldown shape: the template's all-points, then the instance's anchor on top.
    assert_eq!(points("TemplateAll"), 3);
    assert_eq!(
        s.eval::<(f32, f32)>("return TemplateAll:GetWidth(), TemplateAll:GetHeight()")
            .unwrap(),
        (100.0, 50.0)
    );
    // The instance's all-points clears the template's CENTER.
    assert_eq!(points("InstanceAll"), 2);
}
