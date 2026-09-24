//! The runtime template path, `CreateFrame`'s fourth argument ([`crate::loader::apply_template`]),
//! and `<ScrollChild>`. The caller's name wins: a template made as `Mine` publishes `MineTexture`.

use super::common::script;
use crate::script::UiScript;

/// Registers templates by loading a FrameXML document, the path an addon's own `.xml` takes.
fn register(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "fixture document failed to load: {:?}",
        report.errors
    );
}

#[test]
fn a_runtime_template_brings_size_anchors_regions_and_a_fired_onload() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    register(
        &s,
        r#"<Ui>
             <Frame name="Host"/>
             <Frame name="ProbeTemplate" virtual="true">
               <Size><AbsDimension x="160" y="40"/></Size>
               <Anchors>
                 <Anchor point="TOPLEFT" relativeTo="$parent" relativePoint="BOTTOMLEFT">
                   <Offset><AbsDimension x="7" y="-3"/></Offset>
                 </Anchor>
               </Anchors>
               <Layers>
                 <Layer level="ARTWORK">
                   <Texture name="$parentTexture" file="Interface\Probe\Art"/>
                 </Layer>
               </Layers>
               <Scripts>
                 <OnLoad>ProbeLoadedAs = self:GetName(); ProbeLoadWidth = self:GetWidth()</OnLoad>
               </Scripts>
             </Frame>
           </Ui>"#,
    );

    s.run(r#"Mine = CreateFrame("Frame", "Mine", Host, "ProbeTemplate")"#)
        .unwrap();

    assert_eq!(
        s.eval::<(f32, f32)>("return Mine:GetWidth(), Mine:GetHeight()")
            .unwrap(),
        (160.0, 40.0),
        "the template's <Size>"
    );

    // A template's `$parent` in `<Anchors>` is the frame's first named ancestor, not the frame
    // (`0x76c5b0`).
    let (point, rel, rel_point, x, y): (String, String, String, f32, f32) = s
        .eval(
            "local p, r, rp, ox, oy = Mine:GetPoint(1) \
             return p, r:GetName(), rp, ox, oy",
        )
        .unwrap();
    assert_eq!(
        (point.as_str(), rel.as_str(), rel_point.as_str(), x, y),
        ("TOPLEFT", "Host", "BOTTOMLEFT", 7.0, -3.0)
    );

    assert!(
        s.eval::<bool>(r#"return getglobal("MineTexture") ~= nil"#)
            .unwrap(),
        "the template's $parentTexture resolved against the caller's name"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("ProbeTemplateTexture") == nil"#)
            .unwrap(),
        "nothing may be named after the template"
    );

    // The reference fires OnLoad inside `CreateFrame`, after the template is applied.
    assert_eq!(s.eval::<String>("return ProbeLoadedAs").unwrap(), "Mine");
    assert_eq!(s.eval::<f32>("return ProbeLoadWidth").unwrap(), 160.0);

    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// A nested child is parented to the new frame, and `$parent` composes as it does through XML.
#[test]
fn a_nested_frames_child_is_named_against_the_caller() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Frame name="NestTemplate" virtual="true">
               <Frames>
                 <Frame name="$parentInner">
                   <Size><AbsDimension x="11" y="12"/></Size>
                   <Frames>
                     <Frame name="$parentDeep"/>
                   </Frames>
                 </Frame>
               </Frames>
             </Frame>
           </Ui>"#,
    );
    s.run(r#"Outer = CreateFrame("Frame", "Outer", nil, "NestTemplate")"#)
        .unwrap();

    assert_eq!(
        s.eval::<String>("return OuterInner:GetParent():GetName()")
            .unwrap(),
        "Outer"
    );
    assert_eq!(
        s.eval::<(f32, f32)>("return OuterInner:GetWidth(), OuterInner:GetHeight()")
            .unwrap(),
        (11.0, 12.0)
    );
    // Two levels deep, so the chain is composing rather than substituting once.
    assert_eq!(
        s.eval::<String>("return OuterInnerDeep:GetParent():GetName()")
            .unwrap(),
        "OuterInner"
    );
    assert!(s
        .eval::<bool>(r#"return getglobal("NestTemplateInner") == nil"#)
        .unwrap());
    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// The template argument is one name: 1.12's `CreateFrame` hands the string to the XML loader's
/// lookup (`0x6ee6f0`, called at `0x7061dd`), which has no splitter; comma lists came later.
#[test]
fn a_comma_list_is_one_name_and_a_single_name_still_chains() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Frame name="TemplRoot" virtual="true" alpha="0.25">
               <Size><AbsDimension x="50" y="60"/></Size>
             </Frame>
             <Frame name="TemplA" virtual="true" inherits="TemplRoot"/>
             <Frame name="TemplB" virtual="true">
               <Layers>
                 <Layer level="ARTWORK">
                   <Texture name="$parentBTex" file="Interface\Probe\B"/>
                 </Layer>
               </Layers>
             </Frame>
           </Ui>"#,
    );

    s.run(r#"One = CreateFrame("Frame", "One", nil, "TemplA")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32)>("return One:GetWidth(), One:GetHeight()")
            .unwrap(),
        (50.0, 60.0)
    );
    assert_eq!(s.eval::<f32>("return One:GetAlpha()").unwrap(), 0.25);
    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());

    let err = s
        .run(r#"Both = CreateFrame("Frame", "Both", nil, "TemplA, TemplB")"#)
        .expect_err("a comma list is one name, and it misses");
    let text = err.to_string();
    assert!(
        text.contains("TemplA, TemplB"),
        "the miss must name the literal it looked up, unsplit: {text}"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("Both") == nil"#)
            .unwrap(),
        "a missed template must leave no frame behind"
    );
}

/// The lookup folds ASCII case: `SStrHash` (`0x64b3f0`) uppercases before hashing, so a mis-cased
/// name lands in the same bucket, and the compare is `SStrCmpI`.
#[test]
fn a_template_name_is_matched_case_insensitively() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Frame name="CT_RACheckButtonTemplate" virtual="true" alpha="0.25">
               <Size><AbsDimension x="50" y="60"/></Size>
             </Frame>
           </Ui>"#,
    );
    // The mis-casing `CT_RaidAssist/CT_RAOptions.xml` ships: `RA` written `Ra`.
    s.run(r#"Cased = CreateFrame("Frame", "Cased", nil, "CT_RaCheckButtonTemplate")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32)>("return Cased:GetWidth(), Cased:GetHeight()")
            .unwrap(),
        (50.0, 60.0)
    );
    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// A template name that misses the lookup raises at `0x7061dd`, before the frame is built or its
/// name published (`0x706208`, `0x70622d`). The registry holds only `virtual="true"` elements, so
/// a non-virtual name misses like an undeclared one.
#[test]
fn an_unresolvable_template_raises_and_creates_nothing() {
    let s = script();
    register(&s, r#"<Ui><Frame name="PlainFrame"/></Ui>"#);

    for (frame, template) in [("Orphan", "NoSuchTemplate"), ("Plainer", "PlainFrame")] {
        let err = s
            .run(&format!(
                r#"{frame} = CreateFrame("Button", "{frame}", nil, "{template}")"#
            ))
            .expect_err("an unresolvable template must raise");
        let text = err.to_string();
        assert!(
            text.contains("Couldn't find inherited node") && text.contains(template),
            "the raise must carry the reference's message and the name: {text}"
        );

        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("{frame}") == nil"#))
                .unwrap(),
            "{frame} was published despite the miss"
        );
    }

    // An ordinary Lua error: `pcall` catches it, as the widget dispatcher's `lua_pcall` does.
    assert!(
        !s.eval::<bool>(
            r#"return (pcall(CreateFrame, "Button", "Caught", nil, "NoSuchTemplate"))"#
        )
        .unwrap(),
        "the raise must be pcall-catchable"
    );
}

/// A region template on a frame applies its frame-shaped content and warns; a template whose tag
/// disagrees with the kind keeps the kind, and its tag-only parts are skipped, not attempted.
#[test]
fn a_region_template_or_a_mismatched_kind_is_named_never_fatal() {
    let mut s = script();
    register(
        &s,
        r#"<Ui>
             <Texture name="RegionTemplate" virtual="true" file="Interface\Probe\Art">
               <Size><AbsDimension x="33" y="44"/></Size>
             </Texture>
             <Button name="ProbeButtonTemplate" virtual="true">
               <Size><AbsDimension x="90" y="22"/></Size>
               <NormalTexture file="Interface\Probe\Normal"/>
               <ButtonText name="$parentText"/>
             </Button>
           </Ui>"#,
    );

    // 1 · a region template on a frame.
    s.run(r#"FromRegion = CreateFrame("Frame", "FromRegion", nil, "RegionTemplate")"#)
        .unwrap();
    assert_eq!(
        s.eval::<(f32, f32)>("return FromRegion:GetWidth(), FromRegion:GetHeight()")
            .unwrap(),
        (33.0, 44.0),
        "the frame-shaped half of a region template still applies"
    );
    let warnings = s.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("RegionTemplate") && w.contains("REGION template")),
        "the region/frame mismatch must be named: {warnings:?}"
    );
    assert!(s.take_errors().is_empty());

    // 2 · a Button template asked for as a Frame.
    s.run(r#"AsFrame = CreateFrame("Frame", "AsFrame", nil, "ProbeButtonTemplate")"#)
        .unwrap();
    assert_eq!(
        s.eval::<f32>("return AsFrame:GetWidth()").unwrap(),
        90.0,
        "the generic half of the template still applies"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("AsFrameText") == nil"#)
            .unwrap(),
        "a Frame has no ButtonText slot, so nothing was invented for it"
    );
    let warnings = s.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("ProbeButtonTemplate") && w.contains("<Button>")),
        "the kind mismatch must be named: {warnings:?}"
    );
    assert!(
        s.take_errors().is_empty(),
        "the Button-only steps are SKIPPED, not attempted and failed"
    );

    // 3 · the control: the same template, asked for as the kind it was written as.
    s.run(r#"AsButton = CreateFrame("Button", "AsButton", nil, "ProbeButtonTemplate")"#)
        .unwrap();
    assert_eq!(s.eval::<f32>("return AsButton:GetWidth()").unwrap(), 90.0);
    assert!(
        s.eval::<bool>(r#"return getglobal("AsButtonText") ~= nil"#)
            .unwrap(),
        "the <ButtonText name=\"$parentText\"> label, named against the caller"
    );
    assert!(s.take_errors().is_empty());
    assert!(
        s.take_warnings().is_empty(),
        "a matching kind has nothing to report"
    );
}

/// `<ScrollChild>` builds its one child through the `<Frames>` path (`0x6ee280`) and stores it as
/// the scroll child, which gives the frame its range.
#[test]
fn a_scroll_child_element_gives_the_frame_a_real_scroll_range() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    register(
        &s,
        r#"<Ui>
            <ScrollFrame name="Roller">
              <Size><AbsDimension x="200" y="100"/></Size>
              <Anchors><Anchor point="TOPLEFT"/></Anchors>
              <ScrollChild>
                <Frame name="$parentChild">
                  <Size><AbsDimension x="200" y="400"/></Size>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
          </Ui>"#,
    );
    s.resolve();

    // `$parent` names the child against the scroll frame (`0x76c5b0`).
    assert!(s.eval::<bool>("return RollerChild ~= nil").unwrap());
    assert!(s
        .eval::<bool>("return Roller:GetScrollChild() == RollerChild")
        .unwrap());
    // The range is the overhang: a 400-tall child in a 100-tall window scrolls 300.
    assert_eq!(
        s.eval::<f64>("return Roller:GetVerticalScrollRange()")
            .unwrap(),
        300.0,
        "no child means range 0, which is the silent failure this element fixes"
    );
    s.run("Roller:SetVerticalScroll(120)").unwrap();
    assert_eq!(
        s.eval::<f64>("return Roller:GetVerticalScroll()").unwrap(),
        120.0
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The scroll range is the child's whole subtree, not its own height: `0x786e30` seeds a box and
/// `0x786f80` walks the child's regions and child frames. The shape is `ItemTextPageScrollChild`,
/// 10×10 around a 270×304 page (`ItemTextFrame.xml:198`).
#[test]
fn the_scroll_range_is_the_childs_whole_subtree() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    register(
        &s,
        r#"<Ui>
            <ScrollFrame name="Reader">
              <Size><AbsDimension x="280" y="100"/></Size>
              <Anchors><Anchor point="TOPLEFT"/></Anchors>
              <ScrollChild>
                <Frame name="$parentChild">
                  <Size><AbsDimension x="10" y="10"/></Size>
                  <Frames>
                    <Frame name="Page">
                      <Size><AbsDimension x="270" y="304"/></Size>
                      <Anchors><Anchor point="TOPLEFT"/></Anchors>
                    </Frame>
                  </Frames>
                </Frame>
              </ScrollChild>
            </ScrollFrame>
          </Ui>"#,
    );
    s.resolve();

    assert_eq!(
        s.eval::<f64>("return Reader:GetVerticalScrollRange()")
            .unwrap(),
        204.0,
        "the 304-tall page inside the 10-tall child, less the 100-tall window"
    );
    assert_eq!(
        s.eval::<f64>("return ReaderChild:GetHeight()").unwrap(),
        10.0,
        "and the child itself really is the reference's 10 — the range does not come from it"
    );

    // A hidden branch adds nothing: the reference guards both list walks on visibility.
    s.run("Page:Hide()").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return Reader:GetVerticalScrollRange()")
            .unwrap(),
        0.0,
        "hidden subtree, no range"
    );
}

/// The reference reports an empty `<ScrollChild>` as an error (`0x786bc0`) and keeps the frame.
#[test]
fn an_empty_scroll_child_is_reported() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui><ScrollFrame name="Hollow"><ScrollChild/></ScrollFrame></Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("<ScrollChild> is empty")),
        "errors: {:?}",
        report.errors
    );
    assert!(s.eval::<bool>("return Hollow ~= nil").unwrap());
}
