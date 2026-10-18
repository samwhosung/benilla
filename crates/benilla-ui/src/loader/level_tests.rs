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

/// `0x789580`: the Slider's values go through `SStrToFloat` (`0x64aaa0`), so a numeric prefix
/// reads, empty ones are skipped, and an unknown `orientation` is logged, never raised.
#[test]
fn a_slider_reads_its_values_and_orientation_like_the_reference() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Slider name="SlPercent" minValue="0" maxValue="100%" valueStep="5x" defaultValue="40"/>
            <Slider name="SlSpace" minValue=" 5" maxValue="10"/>
            <Slider name="SlEmptyMax" minValue="5" maxValue="" defaultValue="40"/>
            <Slider name="SlDiagonal" orientation="DIAGONAL"/>
            <Slider name="SlBlank" orientation=""/>
            <Slider name="SlFlat" orientation="horizontal"/>
        </Ui>"#,
    );
    assert!(
        report.errors.is_empty(),
        "never a raise: {:?}",
        report.errors
    );
    let state = |name: &str| {
        s.eval::<(f32, f32, f32, f32)>(&format!(
            "local lo, hi = {name}:GetMinMaxValues() \
             return lo, hi, {name}:GetValueStep(), {name}:GetValue()"
        ))
        .unwrap()
    };
    assert_eq!(state("SlPercent"), (0.0, 100.0, 5.0, 40.0));
    // `SStrToFloat` skips no whitespace (`0x64aad3` reads the first byte), so `" 5"` is 0.
    let (lo, hi, _, _) = state("SlSpace");
    assert_eq!((lo, hi), (0.0, 10.0));
    let fresh = s
        .eval::<(f32, f32)>("return CreateFrame('Slider'):GetMinMaxValues()")
        .unwrap();
    let (lo, hi, _, _) = state("SlEmptyMax");
    assert_eq!((lo, hi), fresh, "an empty bound sets nothing");
    let orientation = |name: &str| {
        s.eval::<String>(&format!("return {name}:GetOrientation()"))
            .unwrap()
    };
    assert_eq!(orientation("SlDiagonal"), "VERTICAL", "the ctor's stands");
    assert_eq!(orientation("SlBlank"), "VERTICAL");
    assert_eq!(orientation("SlFlat"), "HORIZONTAL");
    assert!(
        !report.warnings.iter().any(|w| w.contains("SlBlank")),
        "an empty orientation is skipped, not logged: {:?}",
        report.warnings
    );
    assert!(has_line(
        &report,
        "Frame SlDiagonal: Unknown orientation DIAGONAL in element Slider"
    ));
}

/// `0x789580` runs once per level (`0x769820` re-enters `LoadXML` on the `inherits` node at
/// `0x76985c`): an instance with one bound leaves the template's pair, and a pair of its own
/// replaces it whole.
#[test]
fn a_slider_reads_its_range_per_level() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Slider name="SlRangeTemplate" minValue="0" maxValue="10" valueStep="2" defaultValue="4"
                orientation="HORIZONTAL" virtual="true"/>
            <Slider name="SlOneBound" inherits="SlRangeTemplate" maxValue="100"/>
            <Slider name="SlOwnPair" inherits="SlRangeTemplate" minValue="5" maxValue="50"/>
        </Ui>"#,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let state = |name: &str| {
        s.eval::<(f32, f32, f32, f32, String)>(&format!(
            "local lo, hi = {name}:GetMinMaxValues() \
             return lo, hi, {name}:GetValueStep(), {name}:GetValue(), {name}:GetOrientation()"
        ))
        .unwrap()
    };
    assert_eq!(
        state("SlOneBound"),
        (0.0, 10.0, 2.0, 4.0, "HORIZONTAL".into())
    );
    let (lo, hi, step, _, _) = state("SlOwnPair");
    assert_eq!((lo, hi, step), (5.0, 50.0, 2.0));
}

/// `0x76fe20`: every `<TexCoords>` child applies in order, each over 0, 1, 0, 1 with its edges read
/// by `atof`, and the instance's level comes after its template's, so the last one wins whole.
#[test]
fn every_tex_coords_applies_in_order_over_the_default_edges() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Texture name="CropTemplate" file="Interface\A" virtual="true">
                <TexCoords left="0" right="0.578125" top="0" bottom="0.75"/>
            </Texture>
            <Frame name="Host">
                <Layers><Layer>
                    <Texture name="Partial" file="Interface\A"><TexCoords left="0.5"/></Texture>
                    <Texture name="Twice" file="Interface\A">
                        <TexCoords left="0.1" right="0.2" top="0.3" bottom="0.4"/>
                        <TexCoords left="0.5"/>
                    </Texture>
                    <Texture name="Trailing" file="Interface\A">
                        <TexCoords left="0.25x" right="0.75" top="" bottom="1"/>
                    </Texture>
                    <Texture name="Cropped" inherits="CropTemplate">
                        <TexCoords left="0.025" right="0.535" top="0" bottom="0.75"/>
                    </Texture>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    );
    let coords = |name: &str| {
        s.eval::<Vec<f32>>(&format!("return {{{name}:GetTexCoord()}}"))
            .unwrap()
    };
    // UL, LL, UR, LR: (l, t), (l, b), (r, t), (r, b).
    let rect = |l: f32, r: f32, t: f32, b: f32| vec![l, t, l, b, r, t, r, b];
    assert_eq!(coords("Partial"), rect(0.5, 1.0, 0.0, 1.0));
    assert_eq!(
        coords("Twice"),
        rect(0.5, 1.0, 0.0, 1.0),
        "the second, whole"
    );
    assert_eq!(coords("Trailing"), rect(0.25, 0.75, 0.0, 1.0));
    assert_eq!(
        coords("Cropped"),
        rect(0.025, 0.535, 0.0, 0.75),
        "the instance's crop"
    );
}

/// `0x770102`-`0x770171`: a level's `file=` that fails to load logs `0x878d24` and fills the
/// texture opaque green, even over a template level whose file loaded; with no probe installed the
/// path is stored as before.
#[test]
fn an_xml_texture_whose_file_fails_to_load_fills_green() {
    let xml = r#"<Ui>
        <Texture name="GoodTemplate" file="Interface\Good" virtual="true"/>
        <Frame name="Host">
            <Size><AbsDimension x="64" y="64"/></Size>
            <Anchors><Anchor point="CENTER"/></Anchors>
            <Layers><Layer>
                <Texture name="Bad" file="Interface\Nope" setAllPoints="true"/>
                <Texture name="OverGood" inherits="GoodTemplate" file="Interface\NopeToo"/>
                <Texture file="Interface\Nope"/>
                <Texture name="Fine" file="Interface\Good"/>
            </Layer></Layers>
        </Frame>
    </Ui>"#;
    let mut s = ui();
    s.set_texture_probe(Box::new(|path: &str| !path.contains("Nope")));
    let report = load_str(&s, xml);
    let texture = |s: &UiScript, name: &str| {
        s.eval::<Option<String>>(&format!("return {name}:GetTexture()"))
            .unwrap()
    };
    assert_eq!(texture(&s, "Bad").as_deref(), Some("Solid Texture"));
    assert_eq!(texture(&s, "OverGood").as_deref(), Some("Solid Texture"));
    assert_eq!(texture(&s, "Fine").as_deref(), Some("Interface\\Good"));
    for line in [
        r"Texture Bad: Unable to load texture file Interface\Nope",
        r"Texture OverGood: Unable to load texture file Interface\NopeToo",
        r"Texture <unnamed>: Unable to load texture file Interface\Nope",
    ] {
        assert!(has_line(&report, line), "{line}: {:?}", report.warnings);
    }
    s.resolve();
    let green = s.extract().iter().any(|q| {
        matches!(
            &q.content,
            crate::script::QuadContent::Texture { color: Some(c), .. } if *c == [0.0, 1.0, 0.0, 1.0]
        )
    });
    assert!(green, "the failed texture draws opaque green");

    let unprobed = ui();
    let report = load_str(&unprobed, xml);
    assert_eq!(
        texture(&unprobed, "Bad").as_deref(),
        Some("Interface\\Nope")
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

const TEST_FONT: &str = r#"<Font name="LevelTestFont" font="Fonts\FRIZQT__.TTF" virtual="true">
    <FontHeight><AbsValue val="12"/></FontHeight>
</Font>"#;

/// `0x770f40`: a `font=` file needs the node's own `<FontHeight>`, and a refused one returns before
/// `spacing`, the justifies and the colour (`0x771191`); an empty `font=` is absent (`0x7710f7`).
#[test]
fn a_fontstring_font_file_needs_its_own_nodes_height_and_a_refusal_stops_the_level() {
    let s = ui();
    let report = load_str(
        &s,
        &format!(
            r#"<Ui>{TEST_FONT}
            <FontString name="HeightTemplate" font="Fonts\ARIALN.TTF" virtual="true">
                <FontHeight><AbsValue val="20"/></FontHeight>
            </FontString>
            <Frame name="Host">
                <Layers><Layer>
                    <FontString name="Control" inherits="LevelTestFont"/>
                    <FontString name="Refused" inherits="LevelTestFont" font="Fonts\MORPHEUS.TTF"
                                justifyH="LEFT" justifyV="TOP" spacing="3">
                        <Color r="1" g="0" b="0"/>
                    </FontString>
                    <FontString name="LeansOnTemplate" inherits="HeightTemplate"
                                font="Fonts\MORPHEUS.TTF"/>
                    <FontString name="OwnWins" inherits="HeightTemplate" font="Fonts\MORPHEUS.TTF">
                        <FontHeight><AbsValue val="16"/></FontHeight>
                    </FontString>
                    <FontString name="EmptyFont" inherits="LevelTestFont" font="">
                        <FontHeight><AbsValue val="30"/></FontHeight>
                    </FontString>
                </Layer></Layers>
            </Frame>
        </Ui>"#
        ),
    );
    let paint = |name: &str| {
        s.eval::<String>(&format!(
            "local r, g, b = {name}:GetTextColor() \
             return table.concat({{{name}:GetJustifyH(), {name}:GetJustifyV(), \
             {name}:GetSpacing(), r, g, b}}, ' ')"
        ))
        .unwrap()
    };
    assert_eq!(paint("Refused"), paint("Control"), "nothing after the font");
    let font = |name: &str| {
        s.eval::<(String, f32)>(&format!("local f, h = {name}:GetFont() return f, h"))
            .unwrap()
    };
    assert_eq!(font("LeansOnTemplate"), ("Fonts\\ARIALN.TTF".into(), 20.0));
    assert_eq!(font("OwnWins"), ("Fonts\\MORPHEUS.TTF".into(), 16.0));
    assert_eq!(font("EmptyFont"), font("Control"));
    for name in ["Refused", "LeansOnTemplate"] {
        let line = format!("FontString {name}: Missing font height in FontString element");
        assert!(has_line(&report, &line), "{line}: {:?}", report.warnings);
    }
    assert_eq!(
        report.warnings.len(),
        2,
        "no other line: {:?}",
        report.warnings
    );
}

/// `0x7713ca`-`0x771454`: a FontString's own `<Shadow>` takes its first `<Color>` over opaque black
/// and its first `<Offset>`, whose missing coordinate is 0 (`0x6f1ebe`), else (0.001, -0.001).
#[test]
fn a_fontstrings_own_shadow_element_is_read() {
    let s = ui();
    load_str(
        &s,
        &format!(
            r#"<Ui>{TEST_FONT}
            <Frame name="Host">
                <Layers><Layer>
                    <FontString name="Shadowed" inherits="LevelTestFont">
                        <Shadow>
                            <Offset><AbsDimension x="2" y="-3"/></Offset>
                            <Color r="1" g="0" b="0" a="0.5"/>
                        </Shadow>
                    </FontString>
                    <FontString name="BareShadow" inherits="LevelTestFont"><Shadow/></FontString>
                    <FontString name="HalfOffset" inherits="LevelTestFont">
                        <Shadow><Offset><AbsDimension x="2"/></Offset></Shadow>
                    </FontString>
                </Layer></Layers>
            </Frame>
        </Ui>"#
        ),
    );
    let shadow = |name: &str| {
        s.eval::<Vec<f32>>(&format!(
            "local x, y = {name}:GetShadowOffset() \
             local r, g, b, a = {name}:GetShadowColor() return {{x, y, r, g, b, a}}"
        ))
        .unwrap()
    };
    let read = crate::color::quant::read32;
    assert_eq!(
        shadow("Shadowed"),
        vec![2.0, -3.0, read(1.0), 0.0, 0.0, read(0.5)]
    );
    assert_eq!(
        shadow("BareShadow"),
        vec![0.001, -0.001, 0.0, 0.0, 0.0, read(1.0)]
    );
    assert_eq!(
        shadow("HalfOffset"),
        vec![2.0, 0.0, 0.0, 0.0, 0.0, read(1.0)],
        "an <Offset> zeroes the coordinate it leaves out"
    );
}

/// `0x771006`-`0x771032`, `0x778c07`-`0x778c31`: an empty `text=` is skipped, so a template's text
/// stands, and a global holding `""` shows its key.
#[test]
fn an_empty_text_is_skipped_and_an_empty_global_shows_its_key() {
    let s = ui();
    s.eval::<()>("LEVEL_TEST_EMPTY = \"\"").unwrap();
    load_str(
        &s,
        &format!(
            r#"<Ui>{TEST_FONT}
            <FontString name="TextTemplate" inherits="LevelTestFont" text="A" virtual="true"/>
            <Button name="ButtonTemplateA" text="A" virtual="true">
                <ButtonText name="$parentText" inherits="TextTemplate"/>
            </Button>
            <Frame name="Host">
                <Layers><Layer>
                    <FontString name="KeyShown" inherits="LevelTestFont" text="LEVEL_TEST_EMPTY"/>
                    <FontString name="Kept" inherits="TextTemplate" text=""/>
                </Layer></Layers>
            </Frame>
            <Button name="KeyButton" text="LEVEL_TEST_EMPTY">
                <ButtonText name="$parentText" inherits="LevelTestFont"/>
            </Button>
            <Button name="KeptButton" inherits="ButtonTemplateA" text=""/>
            <Button name="LabelKept">
                <ButtonText name="$parentText" inherits="TextTemplate" text=""/>
            </Button>
        </Ui>"#
        ),
    );
    let text = |expr: &str| s.eval::<Option<String>>(&format!("return {expr}")).unwrap();
    assert_eq!(
        text("KeyShown:GetText()").as_deref(),
        Some("LEVEL_TEST_EMPTY")
    );
    assert_eq!(text("Kept:GetText()").as_deref(), Some("A"));
    assert_eq!(
        text("KeyButton:GetText()").as_deref(),
        Some("LEVEL_TEST_EMPTY")
    );
    assert_eq!(text("KeptButton:GetText()").as_deref(), Some("A"));
    assert_eq!(text("LabelKept:GetText()").as_deref(), Some("A"));
}

/// `0x783c30`: a `<Font>` copies the object `inherits=` names, then reads `outline=` and
/// `monochrome=` only beside its own `font=` file, whose own `<FontHeight>` it needs; a refused
/// height skips the rest of the node.
#[test]
fn a_font_element_reads_its_flags_only_beside_its_own_font_file() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Font name="LevelParentFont" font="Fonts\FRIZQT__.TTF" outline="NORMAL" virtual="true">
                <FontHeight><AbsValue val="12"/></FontHeight>
                <Color r="1" g="0" b="0"/>
            </Font>
            <Font name="LevelOutlineOnly" inherits="LevelParentFont" outline="THICK"
                  monochrome="1" virtual="true"/>
            <Font name="LevelOwnFile" inherits="LevelParentFont" font="Fonts\ARIALN.TTF"
                  virtual="true">
                <FontHeight><AbsValue val="14"/></FontHeight>
            </Font>
            <Font name="LevelNoHeight" inherits="LevelParentFont" font="Fonts\ARIALN.TTF"
                  justifyH="LEFT" virtual="true">
                <Color r="0" g="1" b="0"/>
            </Font>
            <Font name="LevelNoParent" inherits="LevelNoSuchFont" virtual="true"/>
        </Ui>"#,
    );
    let font = |name: &str| {
        s.eval::<String>(&format!(
            "local f, h, flags = {name}:GetFont() local r, g, b = {name}:GetTextColor() \
             return table.concat({{f, h, flags or '', {name}:GetJustifyH(), r, g, b}}, ' ')"
        ))
        .unwrap()
    };
    let parent = font("LevelParentFont");
    assert_eq!(
        font("LevelOutlineOnly"),
        parent,
        "no file of its own, no flags of its own"
    );
    assert_eq!(
        font("LevelOwnFile"),
        parent.replacen("Fonts\\FRIZQT__.TTF 12 OUTLINE", "Fonts\\ARIALN.TTF 14 ", 1),
        "its own file sets the whole flags word"
    );
    assert_eq!(
        font("LevelNoHeight"),
        parent,
        "a refused height skips the node"
    );
    assert!(has_line(
        &report,
        "Font LevelNoHeight: Missing font height in Font element"
    ));
    assert!(has_line(
        &report,
        "Couldn't find inherited font: LevelNoSuchFont"
    ));
}

const NOPE: &str = r"Fonts\NOPE.TTF";

/// `0x770f40`, `0x783c30`: a `font=` file that fails to load writes nothing, is logged
/// (`0x878dc8`, `0x87a4c8`) and the node goes on to `spacing`, so a FontString or a Font keeps the
/// font it follows, and keeps following it. The font already held skips the load, keeps its
/// spelling and clears the bit (`0x771541`, `0x783e5d` → `0x783ea9`).
#[test]
fn an_xml_font_file_that_fails_to_load_keeps_the_followed_font() {
    let mut s = ui();
    load_str(
        &s,
        r#"<Ui><Font name="LoadHeld" font="Fonts\HELD.TTF" virtual="true">
            <FontHeight><AbsValue val="10"/></FontHeight>
        </Font></Ui>"#,
    );
    s.set_font_probe(Box::new(|path: &str| {
        !path.contains("NOPE") && !path.contains("HELD")
    }));
    let report = load_str(
        &s,
        r#"<Ui>
            <Font name="LoadParent" font="Fonts\FRIZQT__.TTF" outline="NORMAL" virtual="true">
                <FontHeight><AbsValue val="12"/></FontHeight>
            </Font>
            <Font name="LoadChild" inherits="LoadParent" font="Fonts\NOPE.TTF" spacing="2"
                  virtual="true">
                <FontHeight><AbsValue val="20"/></FontHeight>
            </Font>
            <Font name="LoadHeldAgain" inherits="LoadHeld" font="fonts\held.ttf" virtual="true">
                <FontHeight><AbsValue val="10"/></FontHeight>
            </Font>
            <Frame name="LoadHost">
                <Layers><Layer>
                    <FontString name="LoadMissed" inherits="LoadParent" font="Fonts\NOPE.TTF"
                                spacing="3">
                        <FontHeight><AbsValue val="20"/></FontHeight>
                    </FontString>
                    <FontString name="LoadKept" inherits="LoadHeld" font="FONTS\HELD.TTF">
                        <FontHeight><AbsValue val="10"/></FontHeight>
                    </FontString>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    );
    let font = |s: &UiScript, name: &str| {
        s.eval::<String>(&format!(
            "local f, h, flags = {name}:GetFont() \
             return table.concat({{f, h, flags, {name}:GetSpacing()}}, ' ')"
        ))
        .unwrap()
    };
    assert_eq!(font(&s, "LoadChild"), r"Fonts\FRIZQT__.TTF 12 OUTLINE 2");
    assert_eq!(font(&s, "LoadMissed"), r"Fonts\FRIZQT__.TTF 12 OUTLINE 3");
    for line in [
        format!("Font LoadChild: Unable to load font file {NOPE}"),
        format!("FontString LoadMissed: Unable to load font file {NOPE}"),
    ] {
        assert!(has_line(&report, &line), "{line}: {:?}", report.warnings);
    }
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    // Their font bit is still set, so the parent's next font reaches them; the held font's
    // string and font cleared theirs.
    s.run(r#"LoadParent:SetFont("Fonts\\ARIALN.TTF", 9) LoadHeld:SetFont("Fonts\\ARIALN.TTF", 9)"#)
        .unwrap();
    assert_eq!(font(&s, "LoadChild"), r"Fonts\ARIALN.TTF 9  2");
    assert_eq!(font(&s, "LoadMissed"), r"Fonts\ARIALN.TTF 9  3");
    assert!(font(&s, "LoadKept").starts_with(r"Fonts\HELD.TTF 10 "));
    assert!(font(&s, "LoadHeldAgain").starts_with(r"Fonts\HELD.TTF 10 "));
}

/// `0x770f69`-`0x770f88`: a FontString's `inherits=` is looked up as a font first, and a hit jumps
/// past the template lookup, so a template of the same name is never a level, on a Button's
/// `<NormalText>` (which links nothing) too.
#[test]
fn a_fontstring_inheriting_a_font_loads_no_template_of_that_name() {
    let s = ui();
    load_str(
        &s,
        &format!(
            r#"<Ui>{TEST_FONT}
            <Font name="LevelBoth" font="Fonts\ARIALN.TTF" virtual="true">
                <FontHeight><AbsValue val="14"/></FontHeight>
            </Font>
            <FontString name="LevelBoth" text="T" justifyH="LEFT" virtual="true">
                <Anchors><Anchor point="BOTTOMRIGHT"/></Anchors>
            </FontString>
            <Frame name="BothHost">
                <Size><AbsDimension x="64" y="64"/></Size>
                <Layers><Layer>
                    <FontString name="BothKid" inherits="LevelBoth"/>
                    <FontString name="BothControl" inherits="LevelTestFont"/>
                </Layer></Layers>
            </Frame>
            <Button name="BothButton">
                <NormalText inherits="LevelBoth"/>
            </Button>
        </Ui>"#
        ),
    );
    let got = |expr: &str| {
        s.eval::<String>(&format!("return tostring({expr})"))
            .unwrap()
    };
    assert_eq!(got("BothKid:GetText()"), "nil", "no template text");
    assert_eq!(got("BothKid:GetFontObject() == LevelBoth"), "true");
    assert_eq!(
        got("BothKid:GetJustifyH()"),
        "CENTER",
        "no template justify"
    );
    assert_eq!(
        got("BothKid:GetNumPoints() .. (BothKid:GetPoint(1))"),
        got("BothControl:GetNumPoints() .. (BothControl:GetPoint(1))"),
        "only the implicit anchor"
    );
    assert_ne!(
        got("BothButton:GetText()"),
        "T",
        "<NormalText> loads no template either"
    );
}

/// `0x783870` is the one font table, which `CreateFont` fills (`0x706077`): a FontString's
/// `font=` and `inherits=` find a font a script made.
#[test]
fn a_fontstring_finds_a_font_made_by_create_font() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Script>CreateFont("LevelLuaFont"):SetFont("Fonts\\ARIALN.TTF", 15)</Script>
            <Frame name="LuaFontHost">
                <Layers><Layer>
                    <FontString name="LuaFontKid" font="LevelLuaFont"/>
                    <FontString name="LuaInheritKid" inherits="levelluafont"/>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    );
    for name in ["LuaFontKid", "LuaInheritKid"] {
        let (linked, path, height) = s
            .eval::<(bool, String, f32)>(&format!(
                "local f, h = {name}:GetFont() return {name}:GetFontObject() == LevelLuaFont, f, h"
            ))
            .unwrap();
        assert!(linked, "{name} follows the font");
        assert_eq!(
            (path.as_str(), height),
            (r"Fonts\ARIALN.TTF", 15.0),
            "{name}"
        );
    }
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

/// `0x770f8a`-`0x770fb1`: an `inherits=` that names neither a font nor a template is logged
/// (`0x8786a8`) whoever owns the string's font, and links nothing. A `<NormalText>` also loads into
/// the button's Normal font (`0x783c30`), which logs its own miss (`0x87a51c`).
#[test]
fn a_fontstring_inheriting_nothing_is_logged() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Frame name="NothingHost">
                <Layers><Layer>
                    <FontString name="NothingKid" inherits="LevelNothing"/>
                </Layer></Layers>
            </Frame>
            <Button name="NothingButton">
                <NormalText inherits="LevelNothingToo"/>
            </Button>
        </Ui>"#,
    );
    for line in [
        "Couldn't find inherited node: LevelNothing",
        "Couldn't find inherited node: LevelNothingToo",
        "Couldn't find inherited font: LevelNothingToo",
    ] {
        assert!(has_line(&report, line), "{line}: {:?}", report.warnings);
    }
    assert_eq!(report.warnings.len(), 3, "{:?}", report.warnings);
    assert!(s
        .eval::<bool>("return NothingKid:GetFontObject() == nil")
        .unwrap());
}

/// The one reader of `alpha` (`0x873fe8`) is the frame loader (`0x769a10`): the texture
/// (`0x76fe20`) and FontString (`0x770f40`) loaders never read it.
#[test]
fn a_region_reads_no_alpha() {
    let s = ui();
    load_str(
        &s,
        &format!(
            r#"<Ui>{TEST_FONT}
            <Frame name="AlphaHost" alpha="0.5">
                <Layers><Layer>
                    <Texture name="AlphaTexture" file="Interface\Good" alpha="0.5"/>
                    <FontString name="AlphaText" inherits="LevelTestFont" alpha="0.25"/>
                </Layer></Layers>
            </Frame>
        </Ui>"#
        ),
    );
    let alpha = |name: &str| s.eval::<f32>(&format!("return {name}:GetAlpha()")).unwrap();
    assert_eq!(alpha("AlphaTexture"), 1.0);
    assert_eq!(alpha("AlphaText"), 1.0);
    assert!((alpha("AlphaHost") - 0.5).abs() < 0.01, "a frame reads it");
}

/// `0x783f47`-`0x783fd8`, `0x77129f`-`0x771366`: an XML justify token is matched as `0x6f1990`
/// does. One that matches nothing is skipped and clears no bit, so the axis still follows its
/// font; the other axis's token zeroes the axis; neither raises.
#[test]
fn an_xml_justify_token_is_matched_and_a_miss_skipped() {
    let s = ui();
    load_str(
        &s,
        &format!(
            r#"<Ui>{TEST_FONT}
            <Font name="JustParent" inherits="LevelTestFont" justifyH="LEFT" virtual="true"/>
            <Font name="JustBogus" inherits="JustParent" justifyH="BOGUS" justifyV="NOPE"
                  virtual="true"/>
            <Font name="JustCross" inherits="JustParent" justifyH="TOP" justifyV="LEFT"
                  virtual="true"/>
            <Frame name="JustHost">
                <Layers><Layer>
                    <FontString name="JustString" inherits="JustParent" justifyH="BOGUS"/>
                    <FontString name="JustStringCross" inherits="JustParent" justifyV="RIGHT"/>
                </Layer></Layers>
            </Frame>
        </Ui>"#
        ),
    );
    let justify = |name: &str| {
        s.eval::<String>(&format!(
            "return {name}:GetJustifyH() .. ' ' .. {name}:GetJustifyV()"
        ))
        .unwrap()
    };
    assert_eq!(justify("JustBogus"), "LEFT MIDDLE");
    assert_eq!(justify("JustCross"), "UNKNOWN UNKNOWN");
    assert_eq!(justify("JustString"), "LEFT MIDDLE");
    assert_eq!(justify("JustStringCross"), "LEFT UNKNOWN");
    s.run(r#"JustParent:SetJustifyH("RIGHT")"#).unwrap();
    assert_eq!(justify("JustBogus"), "RIGHT MIDDLE", "still following");
    assert_eq!(justify("JustString"), "RIGHT MIDDLE", "still following");
    assert_eq!(justify("JustCross"), "UNKNOWN UNKNOWN");
}

/// `0x771502`-`0x771541`, `0x783e17`-`0x783e5d`: the held-font shortcut needs the path, the height
/// and the flags all equal; only the flags or only the height differing goes to the load, which a
/// refused file fails (logged, the font kept).
#[test]
fn a_held_font_differing_only_in_height_or_flags_reloads() {
    let mut s = ui();
    load_str(
        &s,
        r#"<Ui><Font name="ReloadHeld" font="Fonts\HELD.TTF" virtual="true">
            <FontHeight><AbsValue val="10"/></FontHeight>
        </Font></Ui>"#,
    );
    s.set_font_probe(Box::new(|path: &str| !path.contains("HELD")));
    let report = load_str(
        &s,
        r#"<Ui>
            <Font name="ReloadFlags" inherits="ReloadHeld" font="Fonts\HELD.TTF" outline="NORMAL"
                  virtual="true">
                <FontHeight><AbsValue val="10"/></FontHeight>
            </Font>
            <Font name="ReloadHeight" inherits="ReloadHeld" font="Fonts\HELD.TTF" virtual="true">
                <FontHeight><AbsValue val="11"/></FontHeight>
            </Font>
            <Frame name="ReloadHost">
                <Layers><Layer>
                    <FontString name="ReloadFlagsString" inherits="ReloadHeld"
                                font="Fonts\HELD.TTF" monochrome="1">
                        <FontHeight><AbsValue val="10"/></FontHeight>
                    </FontString>
                    <FontString name="ReloadHeightString" inherits="ReloadHeld"
                                font="Fonts\HELD.TTF">
                        <FontHeight><AbsValue val="11"/></FontHeight>
                    </FontString>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    );
    for line in [
        r"Font ReloadFlags: Unable to load font file Fonts\HELD.TTF",
        r"Font ReloadHeight: Unable to load font file Fonts\HELD.TTF",
        r"FontString ReloadFlagsString: Unable to load font file Fonts\HELD.TTF",
        r"FontString ReloadHeightString: Unable to load font file Fonts\HELD.TTF",
    ] {
        assert!(has_line(&report, line), "{line}: {:?}", report.warnings);
    }
    for name in [
        "ReloadFlags",
        "ReloadHeight",
        "ReloadFlagsString",
        "ReloadHeightString",
    ] {
        let font = s
            .eval::<String>(&format!(
                "local f, h, flags = {name}:GetFont() return table.concat({{f, h, flags}}, ' ')"
            ))
            .unwrap();
        assert_eq!(font, r"Fonts\HELD.TTF 10 ", "{name} keeps the held font");
    }
}

/// `0x770f69`: each level's `inherits` is asked as a font first, so a template chain stops at the
/// font it reaches, and a template of that font's name is never loaded, even down the chain.
#[test]
fn a_template_chain_stops_at_a_font_that_shares_a_templates_name() {
    let s = ui();
    load_str(
        &s,
        r#"<Ui>
            <Font name="ChainFont" font="Fonts\ARIALN.TTF" virtual="true">
                <FontHeight><AbsValue val="14"/></FontHeight>
            </Font>
            <FontString name="ChainFont" text="FROM_TEMPLATE" justifyV="TOP" virtual="true"/>
            <FontString name="ChainMid" inherits="ChainFont" justifyH="RIGHT" virtual="true"/>
            <Frame name="ChainHost">
                <Layers><Layer>
                    <FontString name="ChainKid" inherits="ChainMid"/>
                </Layer></Layers>
            </Frame>
        </Ui>"#,
    );
    let got = |expr: &str| {
        s.eval::<String>(&format!("return tostring({expr})"))
            .unwrap()
    };
    assert_eq!(
        got("ChainKid:GetText()"),
        "nil",
        "the same-named template is no level"
    );
    assert_eq!(got("ChainKid:GetJustifyV()"), "MIDDLE");
    assert_eq!(
        got("ChainKid:GetJustifyH()"),
        "RIGHT",
        "the middle template is"
    );
    assert_eq!(got("ChainKid:GetFontObject() == ChainFont"), "true");
}

/// `0x78a152`-`0x78a18f`: SimpleHTML's `font=` looks in the one font table, which `CreateFont`
/// fills (`0x706077`), and links all four element fonts.
#[test]
fn a_simplehtml_font_finds_a_font_made_by_create_font() {
    let s = ui();
    let report = load_str(
        &s,
        r#"<Ui>
            <Script>CreateFont("LevelHtmlFont"):SetFont("Fonts\\ARIALN.TTF", 15)</Script>
            <SimpleHTML name="LevelHtml" font="LevelHtmlFont">
                <Size><AbsDimension x="100" y="100"/></Size>
            </SimpleHTML>
        </Ui>"#,
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    for elem in ["P", "H1", "H2", "H3"] {
        let (path, height) = s
            .eval::<(String, f32)>(&format!(
                "local f, h = LevelHtml:GetFont('{elem}') return f, h"
            ))
            .unwrap();
        assert_eq!(
            (path.as_str(), height),
            (r"Fonts\ARIALN.TTF", 15.0),
            "{elem}"
        );
    }
}
