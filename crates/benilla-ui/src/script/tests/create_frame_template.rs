//! `CreateFrame(kind, name, parent, inherits)` — the **runtime** template path
//! ([`crate::loader::apply_template`]).
//!
//! `local b = CreateFrame("Button", "MyButton", UIParent, "UIPanelButtonTemplate")` is the single
//! most common line in the addon corpus, and until the loader's post-`CreateFrame` steps were
//! factored into `Loader::decorate` the fourth argument was dropped on the floor: the addon got a
//! bare frame with no art, no regions, no scripts, and a warning on a channel it never reads.
//!
//! The load-bearing property every test here turns on is that **the caller's own name wins** — a
//! template instantiated as `Mine` publishes `MineTexture`, never `ThatTemplateTexture`, because
//! `getglobal("MineTexture")` is what the addon's next line calls.

use super::common::script;
use crate::script::UiScript;

/// Register templates by loading a small FrameXML document — the same door an addon's own `.xml`
/// comes through, so nothing here is a test-only shortcut into the registry.
fn register(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "fixture document failed to load: {:?}",
        report.errors
    );
}

/// The whole shape at once: a virtual template's `<Size>`, `<Anchors>`, `<Layers>` texture and
/// `<Scripts><OnLoad>` all reach a frame built by `CreateFrame`'s fourth argument — and the region
/// is named against **the instance**, not the template.
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

    // The template's own <Anchors> substitute `$parent` against the frame's PARENT (`0x76c5b0`) —
    // the instance's enclosing frame, not the instance itself.
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

    // The region carries the INSTANCE's name. This is the whole point: `MineTexture` is what an
    // addon reaches for, and `ProbeTemplateTexture` would be useless to it.
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

    // OnLoad fired inside CreateFrame, after the frame was decorated — the reference's ordering,
    // and what `local f = CreateFrame(...); f:Foo()` on the next line assumes.
    assert_eq!(s.eval::<String>("return ProbeLoadedAs").unwrap(), "Mine");
    assert_eq!(s.eval::<f32>("return ProbeLoadWidth").unwrap(), 160.0);

    assert!(s.take_errors().is_empty());
    assert!(s.take_warnings().is_empty());
}

/// A template's nested `<Frames>` child is named against the caller too, and is really parented to
/// the new frame — the `$parent` chain composes through `CreateFrame` exactly as through XML.
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

/// The template argument is **one name**, on this path exactly as on the XML one — and a chain
/// through a single name still resolves.
///
/// This asserted the opposite until the registry lookup was read at the bytes. `"TemplA, TemplB"`
/// is not a list, it is a literal name nothing declared: 1.12's `CreateFrame` reaches the same
/// `0x6ee6f0` the XML loader does (call site `0x7061dd`) with the string Lua handed it, and the
/// loader has no splitter anywhere — comma lists are a **later**-client feature. Corroborated by
/// the corpus: 6842 `inherits=` across 282 vanilla XML files contain **zero** comma lists, while
/// the modern Blizzard UI source is full of them.
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

    // A single name still walks its whole chain. This is what keeps the narrowing honest: it
    // removes the list, not the inheritance.
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

    // The comma form is one literal name, nothing declared it, so it RAISES — naming the whole
    // unsplit string, which is the proof it was never split.
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

/// The case fold, with the four names the corpus actually mis-cases.
///
/// The compare is `SStrCmpI` → `_strnicmp`, and the trap that decides it is the bucket hash:
/// `SStrHash 0x64b3f0` uppercases before mixing, so a mis-cased name lands in the same bucket and
/// the stored-hash pre-check passes rather than short-circuiting. ASCII-only, like every other fold
/// in this engine.
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
    // Exactly the shape `CT_RaidAssist/CT_RAOptions.xml` ships: `RA` written `Ra`.
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

/// An unresolvable template name **raises, and creates nothing**.
///
/// This test asserted the exact opposite — *"the reference creates the frame anyway, and an addon's
/// next line already assumes it did"* — until the bytes were read. `0x7061dd` looks the name up and
/// on NULL falls into `luaL_error(L, "CreateFrame(): Couldn't find inherited node \"%s\"")`, which
/// **never returns**: `luaG_errormsg` and `luaD_throw` contain no `ret` between them and end in
/// `longjmp`, so the `xor eax,eax; ret` that follows the call is dead code. (That trailing `ret` is
/// a fact about `luaL_error`'s `int` return type in C, not about reachability — which is exactly
/// how the old claim was arrived at.)
///
/// Both flavours of "unusable" raise, because the registry only ever holds `virtual="true"`
/// elements, so a name declared without it misses the lookup identically to a name nothing declared.
///
/// The ordering is asserted too: the miss precedes construction and name publication (`0x706208` /
/// `0x70622d`), so nothing partial is left behind — no frame, and no global.
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

        // Nothing partial: no global published, and the name never reached the arena.
        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("{frame}") == nil"#))
                .unwrap(),
            "{frame} was published despite the miss"
        );
    }

    // And it is ordinary Lua propagation — `pcall` catches it, which is what keeps one bad
    // CreateFrame inside a handler from taking the client down (the widget dispatcher runs every
    // handler under `lua_pcall`).
    assert!(
        !s.eval::<bool>(
            r#"return (pcall(CreateFrame, "Button", "Caught", nil, "NoSuchTemplate"))"#
        )
        .unwrap(),
        "the raise must be pcall-catchable"
    );
}

/// The two shape mismatches, neither of which may panic or drop the frame.
///
/// A **region** template (a virtual `<Texture>`) asked for as a frame: its frame-shaped content
/// still lands, its region-only content cannot, and the message says which.
///
/// A **kind that disagrees with the template's tag**: the frame keeps the kind `CreateFrame` was
/// given — no template can retype a frame that already exists — so the `<Button>`-only parts of a
/// Button template simply do not apply to a Frame, and, crucially, are not *attempted* (calling
/// `SetNormalTexture` on a plain Frame would be an error, not a warning).
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

    // 3 · the same template, asked for as the kind it was written as — the control.
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

/// **`<ScrollChild>` gives a ScrollFrame its range** — the loader element that was missing, and
/// the whole remaining distance for an addon's scrolling list.
///
/// The single child is instantiated via the same `0x6ee280` path `<Frames>` uses, then stored as
/// the scroll child. Without it a `ScrollFrame` has nothing to pan, so `GetVerticalScrollRange()`
/// is 0, `SetVerticalScroll` clamps to 0, and `OnVerticalScroll` fires with 0 forever — the list
/// never moves and nothing errors, which is why no instrument could see it.
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

    // The child exists, is named against the FRAME (`$parent` `0x76c5b0`), and is the scroll child.
    assert!(s.eval::<bool>("return RollerChild ~= nil").unwrap());
    assert!(s
        .eval::<bool>("return Roller:GetScrollChild() == RollerChild")
        .unwrap());
    // ...and the range is the overhang: a 400-tall child in a 100-tall window scrolls 300.
    assert_eq!(
        s.eval::<f64>("return Roller:GetVerticalScrollRange()")
            .unwrap(),
        300.0,
        "no child means range 0, which is the silent failure this element fixes"
    );
    // The scroll actually takes, rather than clamping to zero.
    s.run("Roller:SetVerticalScroll(120)").unwrap();
    assert_eq!(
        s.eval::<f64>("return Roller:GetVerticalScroll()").unwrap(),
        120.0
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The scroll range is the child's SUBTREE, not the child frame's own height** (
/// `0x786e30` seeds a bbox and walks `0x786f80` recursively over the child's region and child-frame
/// lists).
///
/// The geometry is the reference reader's own: `ItemTextPageScrollChild` is declared **10×10**
/// around a 270×304 page. Measured as the child's height that is range 0 — a book that cannot be
/// scrolled — which is exactly what shipped before this. The second assertion is the mutation
/// check welded in: it is the number the old law returned.
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
    // The mutation check: under the old law the child's own 10 never reaches the window's 100, so
    // the range clamps to zero and the reader's scrollbar has nowhere to go.
    assert_eq!(
        s.eval::<f64>("return ReaderChild:GetHeight()").unwrap(),
        10.0,
        "and the child itself really is the reference's 10 — the range does not come from it"
    );

    // A hidden branch contributes nothing: the client guards both list walks on visibility, which
    // is what stops a window's parked art from inventing travel nobody can use.
    s.run("Page:Hide()").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return Reader:GetVerticalScrollRange()")
            .unwrap(),
        0.0,
        "hidden subtree, no range"
    );
}

/// An empty `<ScrollChild>` is an error (`0x786bc0`) — the reference's own behaviour, and the
/// honest one: a ScrollFrame that declares a child and has none is a typo, not a design.
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
    // ...and the frame still exists — the client logs and carries on.
    assert!(s.eval::<bool>("return Hollow ~= nil").unwrap());
}
