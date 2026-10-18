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

/// `0x7699ad`: `frameLevel` is read with `atoi` per level and set only above 0; otherwise the line
/// `0x878628` is logged and the level stays.
#[test]
fn a_frame_level_of_zero_or_less_is_logged_and_kept_per_level() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="Host" frameLevel="3"/>
            <Frame name="Plain" parent="Host"/>
            <Frame name="Zero" parent="Host" frameLevel="0"/>
            <Frame name="Negative" parent="Host" frameLevel="-3"/>
            <Frame name="Trailing" parent="Host" frameLevel="5a"/>
            <Frame name="Plus" parent="Host" frameLevel="+5"/>
            <Frame name="LevelSeven" frameLevel="7" virtual="true"/>
            <Frame name="OverSeven" parent="Host" inherits="LevelSeven" frameLevel="0"/>
        </Ui>"#,
    );
    let level = |name: &str| {
        s.eval::<i64>(&format!("return {name}:GetFrameLevel()"))
            .unwrap()
    };
    let plain = level("Plain");
    assert_eq!(plain, 4, "a child sits one above its parent");
    assert_eq!(level("Zero"), plain);
    assert_eq!(level("Negative"), plain);
    assert_eq!(level("Plus"), plain);
    assert_eq!(level("Trailing"), 5);
    assert_eq!(level("OverSeven"), 7, "the template's level stands");
    for (name, value) in [
        ("Zero", "0"),
        ("Negative", "-3"),
        ("Plus", "+5"),
        ("OverSeven", "0"),
    ] {
        assert!(
            has_line(
                &report,
                &format!("Frame {name}: Unknown frame level: {value}")
            ),
            "{name}: {:?}",
            report.warnings
        );
    }
}

/// `0x782ef0`: per level, the range needs both bounds (read with `atof`) and `defaultValue` follows
/// it; `orientation` is skipped when empty and logged, never raised, when unknown.
#[test]
fn a_status_bar_reads_its_range_and_orientation_per_level() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <StatusBar name="Fresh"/>
            <StatusBar name="MaxOnly" maxValue="100" defaultValue="50"/>
            <StatusBar name="Both" minValue="0" maxValue="100x" defaultValue="40"/>
            <StatusBar name="RangeTemplate" minValue="0" maxValue="10" defaultValue="3" virtual="true"/>
            <StatusBar name="OverRange" inherits="RangeTemplate" maxValue="100"/>
            <StatusBar name="Diagonal" orientation="DIAGONAL"/>
            <StatusBar name="Blank" orientation=""/>
            <StatusBar name="Upright" orientation="vertical"/>
        </Ui>"#,
    );
    let state = |name: &str| {
        s.eval::<(f32, f32, f32)>(&format!(
            "local lo, hi = {name}:GetMinMaxValues() return lo, hi, {name}:GetValue()"
        ))
        .unwrap()
    };
    assert_eq!(state("MaxOnly"), state("Fresh"), "one bound sets nothing");
    assert_eq!(state("Both"), (0.0, 100.0, 40.0));
    assert_eq!(
        state("OverRange"),
        (0.0, 10.0, 3.0),
        "the template's pair stands"
    );
    let orientation = |name: &str| {
        s.eval::<String>(&format!("return {name}:GetOrientation()"))
            .unwrap()
    };
    assert_eq!(orientation("Diagonal"), "HORIZONTAL");
    assert_eq!(orientation("Blank"), "HORIZONTAL");
    assert_eq!(orientation("Upright"), "VERTICAL");
    assert!(has_line(
        &report,
        "Frame Diagonal: Unknown orientation DIAGONAL in element StatusBar"
    ));
}
