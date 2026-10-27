//! The order a frame is built in: one inherited level at a time, the template first
//! (`0x76985c`), each level's geometry, flags and child loop in turn (`0x769820`), then its type's
//! own part, and only after every level the `<Frames>` (`0x76a060`). An anchor resolves as it is
//! read (`0x767964`), so what it can name is what this order has built by then.

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

/// The index of `line` in the status record, which must hold it once.
fn at(report: &LoadReport, line: &str) -> usize {
    let lines = report.status.lines();
    let hits: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == line).collect();
    assert_eq!(hits.len(), 1, "{line}: {lines:?}");
    hits[0]
}

/// `0x769820` reads a node's geometry (`0x76987c`) before its flags (`0x769881`), and its
/// `<Scripts>` and `<Layers>` are arms of one document-order loop (`0x769b1c`-`0x769d22`), so a
/// level's lines come in that order.
#[test]
fn a_level_reads_its_geometry_then_its_flags_then_its_children_in_document_order() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="Order" frameStrata="NOWHERE">
                <Layers><Bogus1/></Layers>
                <Scripts><OnNothing1/></Scripts>
                <Layers><Bogus2/></Layers>
                <Anchors><Anchor point="NOWHERE"/></Anchors>
            </Frame>
        </Ui>"#,
    );
    let geometry = at(&report, "Invalid anchor point in frame: NOWHERE");
    let flags = at(&report, "Frame Order: Unknown frame strata: NOWHERE");
    let first = at(
        &report,
        "Frame Order: Unknown child node in Layers element: Bogus1",
    );
    let script = at(&report, "Frame Order: Unknown script element OnNothing1");
    let second = at(
        &report,
        "Frame Order: Unknown child node in Layers element: Bogus2",
    );
    assert!(geometry < flags, "{:?}", report.status.lines());
    assert!(flags < first, "{:?}", report.status.lines());
    assert!(
        first < script && script < second,
        "{:?}",
        report.status.lines()
    );
}

/// A template's whole level, its `<Layers>` included, loads before the instance's geometry
/// (`0x76985c` before `0x76987c`), so an instance anchored to a template region by its full name
/// holds that point at once, before any `<Frames>` child's `OnLoad` runs (`0x76a060`). The region
/// hangs off another frame, so the frame does not depend on itself.
#[test]
fn an_instance_anchor_finds_a_template_region_before_the_frames_children_load() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Frame name="RegionTemplate" virtual="true">
                <Size><AbsDimension x="100" y="40"/></Size>
                <Layers><Layer>
                    <Texture name="$parentArt">
                        <Size><AbsDimension x="10" y="10"/></Size>
                        <Anchors><Anchor point="CENTER" relativeTo="Holder"/></Anchors>
                    </Texture>
                </Layer></Layers>
            </Frame>
            <Frame name="Holder">
                <Size><AbsDimension x="300" y="300"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Frames>
                    <Frame name="Pinned" inherits="RegionTemplate">
                        <Anchors><Anchor point="TOPLEFT" relativeTo="PinnedArt"/></Anchors>
                        <Frames>
                            <Frame name="PinnedChild">
                                <Scripts><OnLoad>seen = this:GetParent():GetNumPoints()</OnLoad></Scripts>
                            </Frame>
                        </Frames>
                    </Frame>
                </Frames>
            </Frame>
        </Ui>"#,
    );
    // `PinnedArt` exists before `Pinned` reads its own anchors, so the point is there when the
    // child's OnLoad looks; a retry after the children would leave it 0.
    assert_eq!(s.eval::<i64>("return seen").unwrap(), 1);
}

/// `<TitleRegion>` is an arm of the base child loop (`0x769b2f`), read where the document places
/// it among the frame's `<Layers>`, not after all of them.
#[test]
fn a_title_region_is_read_at_its_place_in_the_child_loop() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="Titled">
                <Layers><Bogus1/></Layers>
                <TitleRegion><Anchors><Anchor point="NOWHERE"/></Anchors></TitleRegion>
                <Layers><Bogus2/></Layers>
            </Frame>
        </Ui>"#,
    );
    let first = at(
        &report,
        "Frame Titled: Unknown child node in Layers element: Bogus1",
    );
    let title = at(&report, "Invalid anchor point in frame: NOWHERE");
    let second = at(
        &report,
        "Frame Titled: Unknown child node in Layers element: Bogus2",
    );
    assert!(
        first < title && title < second,
        "{:?}",
        report.status.lines()
    );
}
