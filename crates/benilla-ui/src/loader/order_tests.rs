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

/// A Button's children load in one document-order loop (`0x7788c0`), so a label after its
/// highlight anchors to it as it is read, before any `<Frames>` child's `OnLoad`; a retry after
/// the children would leave the label on its implicit anchor until then.
#[test]
fn a_label_after_its_highlight_anchors_to_it_as_it_is_read() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Button name="Row">
                <Size><AbsDimension x="200" y="16"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <HighlightTexture name="$parentHighlight">
                    <Size><AbsDimension x="16" y="16"/></Size>
                    <Anchors><Anchor point="LEFT"/></Anchors>
                </HighlightTexture>
                <ButtonText name="$parentText">
                    <Anchors>
                        <Anchor point="LEFT" relativeTo="$parentHighlight" relativePoint="RIGHT"/>
                    </Anchors>
                </ButtonText>
                <Frames>
                    <Frame name="RowChild">
                        <Scripts><OnLoad>
                            local _, to = RowText:GetPoint(1)
                            seen = RowText:GetNumPoints() .. " " .. tostring(to == RowHighlight)
                        </OnLoad></Scripts>
                    </Frame>
                </Frames>
            </Button>
        </Ui>"#,
    );
    assert_eq!(s.eval::<String>("return seen").unwrap(), "1 true");
}

/// The Button loop's state textures and labels link into `GetRegions` in document order, and a
/// template's whole level, its type's part included, comes before the instance's: the template's
/// `PushedTexture` precedes the instance's `<Layers>` texture, and the instance's `NormalTexture`
/// replaces the template's.
#[test]
fn a_buttons_regions_link_in_document_order_level_by_level() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Button name="StateTemplate" virtual="true">
                <Layers><Layer><Texture name="$parentTplArt"/></Layer></Layers>
                <NormalTexture name="$parentTplNormal"/>
                <PushedTexture name="$parentTplPushed"/>
            </Button>
            <Button name="Ordered">
                <HighlightTexture name="$parentHighlight"/>
                <ButtonText name="$parentText"/>
                <NormalTexture name="$parentNormal"/>
            </Button>
            <Button name="Leveled" inherits="StateTemplate">
                <Layers><Layer><Texture name="$parentArt"/></Layer></Layers>
                <NormalTexture name="$parentNormal"/>
            </Button>
        </Ui>"#,
    );
    let names = |frame: &str| {
        s.eval::<String>(&format!(
            "local t = {{}} for _, r in ipairs({{{frame}:GetRegions()}}) do \
             table.insert(t, r:GetName() or '?') end return table.concat(t, ' ')"
        ))
        .unwrap()
    };
    assert_eq!(
        names("Ordered"),
        "OrderedHighlight OrderedText OrderedNormal"
    );
    assert_eq!(
        names("Leveled"),
        "LeveledTplArt LeveledTplPushed LeveledArt LeveledNormal"
    );
}

/// An EditBox's part runs per level (`0x779fb0`): a template's `<FontString>` loads the frame's
/// own font while it follows nothing, then the instance's `font=` links it, with no override line;
/// the reverse logs `0x879b3c`. Each flag is skipped when empty on its own node, so an instance's
/// `autoFocus=""` leaves its template's `"false"`.
#[test]
fn an_edit_boxs_part_reads_level_by_level() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Font name="LinkedFont" font="Fonts\FRIZQT__.TTF"><FontHeight val="10"/></Font>
            <EditBox name="FontStringTemplate" virtual="true" autoFocus="false">
                <FontString font="Fonts\FRIZQT__.TTF"><FontHeight val="12"/></FontString>
            </EditBox>
            <EditBox name="FontLinked" inherits="FontStringTemplate" font="LinkedFont"
                     autoFocus=""/>
            <EditBox name="FontNameTemplate" virtual="true" font="LinkedFont"/>
            <EditBox name="FontOverridden" inherits="FontNameTemplate">
                <FontString font="Fonts\FRIZQT__.TTF"><FontHeight val="12"/></FontString>
            </EditBox>
        </Ui>"#,
    );
    let lines = report.status.lines();
    assert!(
        !lines.iter().any(|l| l.starts_with("FontLinked:")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "FontOverridden: FontString element overriding font LinkedFont"),
        "{lines:?}"
    );
    s.run("FontLinked:Hide() FontLinked:ClearFocus() FontOverridden:Hide() FontOverridden:ClearFocus()")
        .unwrap();
    s.run("FontLinked:Show()").unwrap();
    assert_eq!(
        s.focused_editbox_name(),
        None,
        "the template's autoFocus=\"false\" stands"
    );
}

/// A Slider's `drawLayer` is its own level's (`0x78959b`), read for that level's `<ThumbTexture>`:
/// an instance's `drawLayer` with no thumb of its own leaves the template's thumb where it was.
#[test]
fn a_sliders_thumb_takes_its_own_levels_draw_layer() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Slider name="ThumbTemplate" virtual="true" drawLayer="ARTWORK">
                <ThumbTexture name="$parentThumb"/>
            </Slider>
            <Slider name="Thumbed" inherits="ThumbTemplate" drawLayer="OVERLAY"/>
        </Ui>"#,
    );
    assert_eq!(
        s.eval::<String>("return Thumbed:GetThumbTexture():GetDrawLayer()")
            .unwrap(),
        "ARTWORK"
    );
}

/// A ScrollFrame reads its node's first `<ScrollChild>` alone (`0x786bd5` → `0x6f2c50`).
#[test]
fn a_scroll_frame_builds_its_first_scroll_child_only() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <ScrollFrame name="Scroller">
                <ScrollChild><Frame name="FirstChild"/></ScrollChild>
                <ScrollChild><Frame name="SecondChild"/></ScrollChild>
            </ScrollFrame>
        </Ui>"#,
    );
    assert!(s
        .eval::<bool>("return FirstChild ~= nil and Scroller:GetScrollChild() == FirstChild")
        .unwrap());
    assert!(s.eval::<bool>("return SecondChild == nil").unwrap());
}

/// A region's anchor resolves as its `<Layers>` arm is read (`0x767964`), before the frame's
/// `<Frames>` exist (`0x76a060` runs from the post-load hook), so a FontString anchored to its
/// frame's own child frame logs the miss as spelled (`0x878440`) and drops the anchor
/// (`0x76797b`); the line comes before any line the child's own load writes.
#[test]
fn a_region_anchored_to_its_frames_own_child_frame_logs_and_drops() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="Tip">
                <Size><AbsDimension x="100" y="100"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Layers><Layer>
                    <FontString name="$parentLabel">
                        <Anchors>
                            <Anchor point="LEFT" relativeTo="$parentInner" relativePoint="RIGHT"/>
                        </Anchors>
                    </FontString>
                </Layer></Layers>
                <Frames>
                    <Frame name="$parentInner" frameStrata="NOWHERE"/>
                </Frames>
            </Frame>
        </Ui>"#,
    );
    let miss = at(&report, "Couldn't find relative frame: $parentInner");
    let child = at(&report, "Frame TipInner: Unknown frame strata: NOWHERE");
    assert!(miss < child, "{:?}", report.status.lines());
    // No point from the dropped anchor: only the region's implicit creation anchor.
    assert!(s
        .eval::<bool>("local _, to = TipLabel:GetPoint(1) return to ~= TipInner")
        .unwrap());
}

/// A frame anchored to a later sibling, or to a region of its own level, names something not yet
/// built when its geometry is read (`0x76987c` precedes the level's child loop), so each logs and
/// drops.
#[test]
fn a_frame_anchored_to_what_is_built_after_it_logs_and_drops() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="Early">
                <Anchors><Anchor point="TOPLEFT" relativeTo="Late"/></Anchors>
            </Frame>
            <Frame name="Late"/>
            <Frame name="OwnArt">
                <Anchors><Anchor point="TOPLEFT" relativeTo="OwnArtTexture"/></Anchors>
                <Layers><Layer>
                    <Texture name="$parentTexture">
                        <Anchors><Anchor point="CENTER" relativeTo="Late"/></Anchors>
                    </Texture>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    );
    at(&report, "Couldn't find relative frame: Late");
    at(&report, "Couldn't find relative frame: OwnArtTexture");
    assert_eq!(s.eval::<i64>("return Early:GetNumPoints()").unwrap(), 0);
    assert_eq!(s.eval::<i64>("return OwnArt:GetNumPoints()").unwrap(), 0);
}

/// The reverse of the label-after-highlight shape: a `<ButtonText>` read before the highlight it
/// names finds nothing (`0x7788c0` is one document-order loop), logs the miss, and keeps only its
/// implicit anchor.
#[test]
fn a_label_before_its_highlight_logs_and_keeps_its_implicit_anchor() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Button name="Reversed">
                <Size><AbsDimension x="200" y="16"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <ButtonText name="$parentText">
                    <Anchors>
                        <Anchor point="LEFT" relativeTo="$parentHighlight" relativePoint="RIGHT"/>
                    </Anchors>
                </ButtonText>
                <HighlightTexture name="$parentHighlight">
                    <Size><AbsDimension x="16" y="16"/></Size>
                    <Anchors><Anchor point="LEFT"/></Anchors>
                </HighlightTexture>
            </Button>
        </Ui>"#,
    );
    at(&report, "Couldn't find relative frame: $parentHighlight");
    assert_eq!(
        s.eval::<(i64, String, bool)>(
            "local p, to = ReversedText:GetPoint(1) \
             return ReversedText:GetNumPoints(), p, to == Reversed"
        )
        .unwrap(),
        (1, "CENTER".to_string(), true)
    );
}

/// `0x76a060` reads each level's first `<Frames>` alone (`0x6f2c50`), the template's level first,
/// and builds each child whole before its next sibling (`0x6ee280`), so a child's `OnLoad` sees
/// the siblings before it and none after.
#[test]
fn frames_build_per_level_from_each_levels_first_frames_element() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Frame name="KidsTemplate" virtual="true">
                <Frames>
                    <Frame name="$parentTplKid">
                        <Scripts><OnLoad>
                            order = (order or "") .. "tpl:" .. tostring(OwnKidsOwnKid ~= nil) .. " "
                        </OnLoad></Scripts>
                    </Frame>
                </Frames>
            </Frame>
            <Frame name="OwnKids" inherits="KidsTemplate">
                <Frames>
                    <Frame name="$parentOwnKid">
                        <Scripts><OnLoad>
                            order = (order or "") .. "own:" .. tostring(OwnKidsTplKid ~= nil)
                        </OnLoad></Scripts>
                    </Frame>
                </Frames>
                <Frames>
                    <Frame name="$parentSecondFrames"/>
                </Frames>
            </Frame>
        </Ui>"#,
    );
    assert_eq!(
        s.eval::<String>("return order").unwrap(),
        "tpl:false own:true"
    );
    assert!(s.eval::<bool>("return OwnKidsSecondFrames == nil").unwrap());
}

/// `parent=` is the node's own, else the first up its `inherits` chain (`0x6ee390`-`0x6ee3c4`); a
/// present empty one ends the walk on the default parent (`0x6ee3c7`).
#[test]
fn a_parent_attribute_is_found_up_the_inherits_chain() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Frame name="Home"/>
            <Frame name="ParentedTemplate" virtual="true" parent="Home"/>
            <Frame name="Lexical">
                <Frames>
                    <Frame name="Moved" inherits="ParentedTemplate"/>
                    <Frame name="Kept" inherits="ParentedTemplate" parent=""/>
                </Frames>
            </Frame>
        </Ui>"#,
    );
    assert!(s.eval::<bool>("return Moved:GetParent() == Home").unwrap());
    assert!(s
        .eval::<bool>("return Kept:GetParent() == Lexical")
        .unwrap());
}

/// Deviation: an `inherits` cycle stops at the repeated name with a warning, and each template in
/// it loads once. The reference hangs in its unguarded `parent=` walk (`0x6ee390`-`0x6ee3c2`), or,
/// with a `parent=` on the cycle, overflows the stack in the level recursion (`0x76985c`).
#[test]
fn an_inherits_cycle_warns_and_loads_each_template_once() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="CycleA" virtual="true" inherits="CycleB">
                <Scripts><OnLoad>count = (count or 0) + 1</OnLoad></Scripts>
            </Frame>
            <Frame name="CycleB" virtual="true" inherits="CycleA" alpha="0.5"/>
            <Frame name="Cycled" inherits="CycleA"/>
        </Ui>"#,
    );
    assert!(
        report.warnings.iter().any(
            |w| w == "inheritance cycle detected at template 'CycleA'; skipping this reference"
        ),
        "{:?}",
        report.warnings
    );
    assert_eq!(s.eval::<i64>("return count").unwrap(), 1);
    assert!(s
        .eval::<bool>("return math.abs(Cycled:GetAlpha() - 0.5) < 0.01")
        .unwrap());
}

/// Each `<TitleRegion>` allocates its own region and stores it over the frame's (`0x769b3e`-
/// `0x769b97`), so an instance's element replaces its template's whole: the frame keeps the
/// instance's anchors and size alone, none of the template's `<Size>`. A frame with the template's
/// element only keeps the template's.
#[test]
fn each_title_region_element_is_a_region_of_its_own() {
    let mut s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Frame name="TitledTemplate" virtual="true">
                <Size><AbsDimension x="200" y="100"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <TitleRegion>
                    <Size><AbsDimension x="50" y="40"/></Size>
                    <Anchors>
                        <Anchor point="TOPLEFT"/>
                        <Anchor point="BOTTOMRIGHT"/>
                    </Anchors>
                </TitleRegion>
            </Frame>
            <Frame name="Retitled" inherits="TitledTemplate">
                <TitleRegion>
                    <Anchors><Anchor point="TOP"/></Anchors>
                </TitleRegion>
            </Frame>
            <Frame name="Inherited" inherits="TitledTemplate"/>
        </Ui>"#,
    );
    assert_eq!(
        s.eval::<(i64, String, bool)>(
            "local t = Retitled:GetTitleRegion() local p, to = t:GetPoint(1) \
             return t:GetNumPoints(), p, to == Retitled"
        )
        .unwrap(),
        (1, "TOP".to_string(), true),
        "the instance's one point, on the frame itself"
    );
    assert_eq!(
        s.eval::<i64>("return Inherited:GetTitleRegion():GetNumPoints()")
            .unwrap(),
        2
    );
    s.resolve();
    assert_eq!(
        s.eval::<(f64, f64)>(
            "local t = Retitled:GetTitleRegion() return t:GetWidth(), t:GetHeight()"
        )
        .unwrap(),
        (0.0, 0.0),
        "the template's <Size> stayed on the template's own region"
    );
    assert_eq!(s.eval::<i64>("return Retitled:GetNumRegions()").unwrap(), 0);
}

/// A GameTooltip's post-load override chains to the frame's first (`0x529668` → `0x76a2f0`), so
/// its `TextLeft%d`/`TextRight%d` lookup runs after the tooltip's `<Frames>`, `OnLoad` and
/// `OnShow`, and a pair its `OnLoad` makes is a line.
#[test]
fn a_tooltip_finds_the_lines_its_own_onload_made() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <GameTooltip name="LateTip">
                <Scripts><OnLoad>
                    this:CreateFontString("LateTipTextLeft1")
                    this:CreateFontString("LateTipTextRight1")
                </OnLoad></Scripts>
            </GameTooltip>
        </Ui>"#,
    );
    s.run(r#"LateTip:AddLine("one") LateTip:AddLine("two")"#)
        .unwrap();
    assert_eq!(s.eval::<i64>("return LateTip:NumLines()").unwrap(), 1);
}

/// A Minimap's model attributes come from the instance's own node alone (`0x4ee2c5`), the stock
/// model only for an absent one (`0x4ee2d3`): a template's attribute is never read, and an empty
/// player model clears the arrow and logs `0x84c6fc` at severity 1, where an empty arrow model
/// loads the error cube and logs nothing (`0x707350`).
#[test]
fn a_minimap_reads_its_models_on_its_own_node_and_an_empty_one_is_passed_on() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Minimap name="ModelTemplate" virtual="true"
                     minimapPlayerModel="Interface\Minimap\Template.mdx"/>
            <Minimap name="FromTemplate" inherits="ModelTemplate"/>
            <Minimap name="Emptied" minimapArrowModel="" minimapPlayerModel=""/>
        </Ui>"#,
    );
    assert_eq!(
        s.eval::<String>("return ({FromTemplate:GetChildren()})[9]:GetModel()")
            .unwrap(),
        crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL
    );
    assert_eq!(
        report.status.lines(),
        ["Invalid minimapPlayerModel in Minimap.xml"]
    );
    assert!(s
        .eval::<bool>(
            "local m = ({Emptied:GetChildren()})[9]:GetModel() return m == nil or m == ''"
        )
        .unwrap());
}

/// `Instantiate 0x6ee280` looks `parent=` up (`0x6ee3fa`) before it builds the frame, and the
/// missing template is logged only inside the load (`0x769861`), so the parent miss comes first.
#[test]
fn a_parent_miss_is_logged_before_a_missing_template() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui><Frame name="O" inherits="Missing" parent="Nowhere"/></Ui>"#,
    );
    assert_eq!(
        report.status.lines(),
        [
            "Couldn't find frame parent: Nowhere",
            "Couldn't find inherited node: Missing"
        ]
    );
}

/// A model pane's `file=` goes to `SetModel` as written (`0x76caf2`), never through the global
/// strings, and a file the pane takes logs nothing (`0x76cb03`).
#[test]
fn a_model_file_is_passed_as_written() {
    let s = ui();
    s.run(r#"MODEL_KEY = "Some\\Other.mdx""#).unwrap();
    let report = load_str(&s, r#"<Ui><Model name="Pane" file="MODEL_KEY"/></Ui>"#);
    assert_eq!(
        s.eval::<String>("return Pane:GetModel()").unwrap(),
        "MODEL_KEY"
    );
    assert!(
        report.status.lines().is_empty(),
        "{:?}",
        report.status.lines()
    );
}

/// Each `<ButtonText>` builds a new label (`0x6f2780`) and the install destroys the one it
/// displaces (`0x778d38`), so an instance's unnamed element replaces its template's label object.
#[test]
fn an_instances_unnamed_label_replaces_its_templates() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Button name="LabelTemplate" virtual="true">
                <ButtonText name="TplLabel" text="T"/>
            </Button>
            <Button name="Relabeled" inherits="LabelTemplate">
                <ButtonText text="I"/>
            </Button>
        </Ui>"#,
    );
    assert!(s
        .eval::<bool>("return Relabeled:GetFontString() ~= TplLabel")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return Relabeled:GetNumRegions()").unwrap(),
        1
    );
    assert_eq!(s.eval::<String>("return Relabeled:GetText()").unwrap(), "I");
}
